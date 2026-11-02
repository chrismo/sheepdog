// The Atlas's data (#377): every NEXT step by stage from src/next/steps.rs
// and each verb's gate rules from src/act/gates.rs, so the page can't drift
// from the code. No state is read: each rule list comes off
// Derived::default(), and refuses is ignored.

use crate::act::verbs::{Scope, Verb, VERBS};
use crate::act::{Level, Rule};
use crate::next::steps::{Stage, StepDef, STEPS};
use crate::next::Derived;
use serde_json::{json, Value};

/// What --force does with a rule at <level>, as #377 words it.
fn means(level: Level) -> &'static str {
    match level {
        Level::Hard => "force can't",
        Level::Limit => "setting holds",
        Level::Done => "nothing to do",
        Level::Soft => "force passes",
        Level::Warn => "warns",
    }
}

fn rule(r: &Rule) -> Value {
    json!({"name": r.name, "level": format!("{:?}", r.level), "means": means(r.level), "clears": r.clears})
}

/// The step's verb as src/act/verbs.rs declares it: resolve is restack --resolve.
fn declared(verb: &str) -> &str {
    VERBS.iter().find(|v| v.step() == verb).map_or(verb, |v| v.name)
}

fn step(d: &StepDef) -> Value {
    json!({"key": d.key, "who": d.who, "label": d.label, "meaning": d.meaning, "verb": d.verb.map(declared), "glyph": d.glyph})
}

/// What act::run checks before a verb's own gate (src/act/mod.rs): the
/// running marker for a verb that writes the branch (running::idle, no
/// --force), and the sd-lock for an agent step (attached_check, --force
/// runs anyway).
const CHECKS: [(&str, &str, Level, &str, &str); 2] = [
    ("running", "another step holds the branch's running marker (<git dir>/sheepdog/running/<branch>)", Level::Hard,
     "it ends; if it's hung, kill its pid or remove the marker", "every verb that writes the worktree, the branch or its remote"),
    ("attached", "the branch is attached: its session is open interactively (branch.<b>.sd-lock)", Level::Soft,
     "exit that session, or sd act <verb> --force", "every agent step"),
];

fn checks_of(v: &Verb) -> Vec<&'static str> {
    let mut out = vec![];
    if v.scope != Scope::None {
        out.push("running");
    }
    if v.agent {
        out.push("attached");
    }
    out
}

fn verb(v: &Verb) -> Value {
    let rules = v.gate.as_ref().map(|g| (g.rules)(&Derived::default()).iter().map(rule).collect::<Vec<_>>());
    json!({"name": v.name, "agent": v.agent, "spends": v.spends, "checks": checks_of(v), "rules": rules})
}

/// GET /api/atlas: {holds: [step], stages: [{stage, steps: [step]}],
/// verbs: [{name, agent, spends, checks: [id], rules: [{name, level, means,
/// clears}] or null}], checks: [{id, name, level, means, clears, applies}]}.
/// A step is {key, who, label, meaning, verb, glyph}.
pub fn atlas() -> Value {
    let holds: Vec<Value> = STEPS.iter().filter(|d| d.stage == Stage::Hold).map(step).collect();
    let stages: Vec<Value> = Stage::FLOW
        .iter()
        .map(|&stage| json!({"stage": stage.name(), "steps": STEPS.iter().filter(|d| d.stage == stage).map(step).collect::<Vec<_>>()}))
        .collect();
    let checks: Vec<Value> = CHECKS
        .iter()
        .map(|(id, name, level, clears, applies)| {
            json!({"id": id, "name": name, "level": format!("{level:?}"), "means": means(*level), "clears": clears, "applies": applies})
        })
        .collect();
    json!({"holds": holds, "stages": stages, "verbs": VERBS.iter().map(verb).collect::<Vec<_>>(), "checks": checks})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::act::verbs::VERBS;
    use crate::next::steps::{Stage, STEPS};
    use crate::next::Derived;
    use crate::val::t;

    fn steps_in(a: &Value) -> Vec<&Value> {
        let held = a["holds"].as_array().into_iter().flatten();
        held.chain(a["stages"].as_array().into_iter().flatten().flat_map(|s| s["steps"].as_array().into_iter().flatten())).collect()
    }

    fn verb<'a>(a: &'a Value, name: &str) -> &'a Value {
        a["verbs"].as_array().unwrap().iter().find(|v| v["name"] == name).unwrap_or_else(|| panic!("no {name} in {a}"))
    }

    fn rule<'a>(v: &'a Value, name: &str) -> &'a Value {
        v["rules"].as_array().unwrap().iter().find(|r| r["name"] == name).unwrap_or_else(|| panic!("no rule {name} in {v}"))
    }

    #[test]
    fn every_steps_key_is_in_the_atlas_once_with_its_who_meaning_and_verb() {
        let a = atlas();
        let listed = steps_in(&a);
        assert_eq!(listed.len(), STEPS.len());
        for d in STEPS {
            let s = listed.iter().find(|s| s["key"] == d.key).unwrap_or_else(|| panic!("{} missing", d.key));
            assert_eq!(s["who"], json!(d.who), "{}", d.key);
            assert_eq!(s["meaning"], d.meaning);
            assert_eq!(s["label"], d.label);
        }
        let conflict = listed.iter().find(|s| s["key"] == "conflict").unwrap();
        assert_eq!(conflict["verb"], "restack --resolve");
        assert_eq!(listed.iter().find(|s| s["key"] == "ci-red").unwrap()["verb"], "ci-fix");
        assert_eq!(listed.iter().find(|s| s["key"] == "ready").unwrap()["verb"], Value::Null);
    }

    #[test]
    fn the_holds_come_first_and_the_stages_are_the_flocks_seven_in_order() {
        let a = atlas();
        let holds: Vec<&str> = a["holds"].as_array().unwrap().iter().map(|s| s["key"].as_str().unwrap()).collect();
        assert!(holds.contains(&"running") && holds.contains(&"question") && !holds.contains(&"push"), "{holds:?}");
        let stages: Vec<String> = a["stages"].as_array().unwrap().iter().map(|s| t(s, "stage")).collect();
        assert_eq!(stages, Stage::FLOW.map(Stage::name));
        for s in a["stages"].as_array().unwrap() {
            assert!(!s["steps"].as_array().unwrap().is_empty(), "{}", s["stage"]);
        }
    }

    #[test]
    fn every_gated_verb_lists_its_rules_at_their_levels_and_the_ungated_none() {
        let a = atlas();
        for v in VERBS {
            let got = verb(&a, v.name);
            match &v.gate {
                Some(g) => {
                    let want: Vec<(&str, String, &str)> = (g.rules)(&Derived::default()).iter().map(|r| (r.name, format!("{:?}", r.level), r.clears)).collect();
                    let rules: Vec<(&str, String, &str)> =
                        got["rules"].as_array().unwrap().iter().map(|r| (r["name"].as_str().unwrap(), t(r, "level"), r["clears"].as_str().unwrap())).collect();
                    assert_eq!(rules, want, "{}", v.name);
                }
                None => assert_eq!(got["rules"], Value::Null, "{}", v.name),
            }
        }
        assert_eq!(a["verbs"].as_array().unwrap().len(), VERBS.len());
    }

    #[test]
    fn levels_are_rendered_as_what_force_does() {
        let a = atlas();
        let push = verb(&a, "push");
        let undone = rule(push, "not pushing: the restack was undone: some of the target's commits are copies here, and pushing would publish them (if they aren't, git push by hand)");
        assert_eq!((t(undone, "level"), t(undone, "means")), ("Hard".into(), "force can't".into()));
        let closed = rule(push, "the PR is merged or closed");
        assert_eq!((t(closed, "level"), t(closed, "means"), t(closed, "clears")), ("Soft".into(), "force passes".into(), "reopen it, or start a new branch".into()));
        let address = verb(&a, "address");
        assert_eq!(t(rule(address, "nothing to address"), "means"), "nothing to do");
        assert_eq!(t(rule(address, "behind the target"), "means"), "warns");
        assert_eq!(t(rule(address, crate::act::gates::SPEND), "means"), "setting holds");
    }

    #[test]
    fn the_shared_checks_apply_by_the_verbs_declaration() {
        let a = atlas();
        let ids: Vec<&str> = a["checks"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["running", "attached"]);
        let running = &a["checks"][0];
        assert_eq!((t(running, "level"), t(running, "means")), ("Hard".into(), "force can't".into()));
        assert_eq!((t(&a["checks"][1], "level"), t(&a["checks"][1], "means")), ("Soft".into(), "force passes".into()));
        assert_eq!(verb(&a, "ci-fix")["checks"], json!(["running", "attached"]));
        assert_eq!(verb(&a, "push")["checks"], json!(["running"]));
        assert_eq!(verb(&a, "review --accept")["checks"], json!(["attached"]));
        assert_eq!(verb(&a, "trigger")["checks"], json!([]));
    }
}
