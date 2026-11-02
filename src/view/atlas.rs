// The Atlas's data (#377): every NEXT step by stage from src/next/steps.rs
// and each verb's gate rules from src/act/gates.rs, so the page can't drift
// from the code. No state is read: each rule list comes off probes(), with
// a rule worded by state listed once per wording, and refuses is ignored.

use crate::act::gates::Gated;
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

/// What act::run checks besides a verb's own gate (src/act/mod.rs): the
/// running marker for a verb that writes the branch, before the gate
/// (running::idle, no --force), and the sd-lock for an agent step
/// (attached_check, --force runs anyway): first for an ungated one, but
/// armed for a gated one, so it runs only once enforce says Go.
const CHECKS: [(&str, &str, Level, &str, &str); 2] = [
    ("running", "another step holds the branch's running marker (<git dir>/sheepdog/running/<branch>)", Level::Hard,
     "it ends; if it's hung, kill its pid or remove the marker", "every verb that writes the worktree, the branch or its remote, before its gate"),
    ("attached", "the branch is attached: its session is open interactively (branch.<b>.sd-lock)", Level::Soft,
     "exit that session, or sd act <verb> --force", "every agent step: once its gate lets it run, or first if it has no gate"),
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

/// States that between them give every wording of a rule whose name or
/// clears is picked from state: address's push relation (unpushed ×
/// behind_remote), review's catch-up (rv_yours, rv_fetch) and findings
/// clears (fix_due), and ci-fix's drop clears (whether NEXT offers a push).
/// Each with and without a push NEXT would offer. A gate that adds another
/// fails every_rule_a_generated_state_gives_is_in_the_atlas until it's here.
fn probes() -> Vec<Derived> {
    (0..32u8)
        .flat_map(|bits| {
            let on = |i: u8| bits & (1 << i) != 0;
            let d = Derived { unpushed: on(0), behind_remote: on(1), rv_yours: on(2), rv_fetch: on(3), fix_due: on(4), ..Derived::default() };
            let pushes = Derived { open: true, to_push: true, foreign_authors: Some(false), ..d.clone() };
            [d, pushes]
        })
        .collect()
}

/// <g>'s rules in source order, each in every wording the probes give.
fn rules_of(g: &Gated) -> Vec<Value> {
    let mut slots: Vec<Vec<Rule>> = vec![];
    for d in probes() {
        for (i, r) in (g.rules)(&d).into_iter().enumerate() {
            if slots.len() <= i {
                slots.push(vec![]);
            }
            if !slots[i].iter().any(|s| (s.name, s.level, s.clears) == (r.name, r.level, r.clears)) {
                slots[i].push(r);
            }
        }
    }
    slots.iter().flatten().map(rule).collect()
}

fn verb(v: &Verb) -> Value {
    let rules = v.gate.as_ref().map(rules_of);
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
    use crate::act::gates::{addr, rvw};
    use crate::act::verbs::VERBS;
    use crate::next::derived_arb::check;
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

    fn listed(v: &Value) -> Vec<(&str, String, &str)> {
        v["rules"].as_array().unwrap().iter().map(|r| (r["name"].as_str().unwrap(), t(r, "level"), r["clears"].as_str().unwrap())).collect()
    }

    #[test]
    fn every_gated_verb_lists_its_rules_at_their_levels_in_source_order_and_the_ungated_none() {
        let a = atlas();
        for v in VERBS {
            let got = verb(&a, v.name);
            match &v.gate {
                Some(g) => {
                    let rules = listed(got);
                    let mut at = rules.iter();
                    for r in (g.rules)(&Derived::default()) {
                        let want = (r.name, format!("{:?}", r.level), r.clears);
                        assert!(at.any(|x| *x == want), "{}: {want:?} missing or out of order in {rules:?}", v.name);
                    }
                    let mut seen = rules.clone();
                    seen.sort();
                    seen.dedup();
                    assert_eq!(seen.len(), rules.len(), "{}: a rule twice", v.name);
                }
                None => assert_eq!(got["rules"], Value::Null, "{}", v.name),
            }
        }
        assert_eq!(a["verbs"].as_array().unwrap().len(), VERBS.len());
    }

    #[test]
    fn a_rule_whose_name_or_clears_depends_on_state_is_listed_in_each_wording() {
        let a = atlas();
        let names = |v| listed(verb(&a, v)).into_iter().map(|r| r.0).collect::<Vec<_>>();
        let address = names("address");
        for n in [addr::UNPUSHED, addr::REMOTE_MORE, addr::DIVERGED] {
            assert!(address.contains(&n), "{n} not in {address:?}");
        }
        let review = names("review");
        for n in [rvw::NOT_OPEN, rvw::YOURS, rvw::FETCH] {
            assert!(review.contains(&n), "{n} not in {review:?}");
        }
        let findings: Vec<_> = listed(verb(&a, "review")).into_iter().filter(|r| r.0 == rvw::FINDINGS).map(|r| r.2).collect();
        assert_eq!(findings.len(), 2, "{findings:?}");
        let drop = "the remote has commits reset away here, which a push drops";
        assert_eq!(listed(verb(&a, "ci-fix")).iter().filter(|r| r.0 == drop).count(), 2);
    }

    #[test]
    fn every_rule_a_generated_state_gives_is_in_the_atlas() {
        let a = atlas();
        check("the Atlas lists the rule", |d| {
            for v in VERBS {
                let Some(g) = &v.gate else { continue };
                let rules = listed(verb(&a, v.name));
                for r in (g.rules)(d) {
                    let got = (r.name, format!("{:?}", r.level), r.clears);
                    if !rules.contains(&got) {
                        return Err(format!("{}: {got:?} not listed", v.name));
                    }
                }
            }
            Ok(())
        });
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
        assert_eq!(t(rule(verb(&a, "review --accept"), rvw::NO_BASE), "means"), "force can't");
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
