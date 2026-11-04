// The Atlas's data (#377): every NEXT step by stage from src/next/steps.rs
// and each verb's gate rules from src/act/gates.rs, so the page can't drift
// from the code, and the settings each step reads (steps::settings_of) with
// this repo's value and layer from one sd config explain per request, and
// its links (view/links.rs, #375): the files behind it and the branches at
// it now. No branch state is read for the rules: each rule list comes off
// probes(), with a rule worded by state listed once per wording, and
// refuses is ignored.

use super::flock::{self, Cached};
use super::links;
use crate::act::gates::Gated;
use crate::act::verbs::{Scope, Verb, VERBS};
use crate::act::{Level, Rule};
use crate::next::steps::{settings_of, HoldKind, Stage, StepDef, STEPS};
use crate::next::Derived;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;

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

/// The branches at each key, and whether the Map has a file.
struct Links<'a> {
    at: BTreeMap<String, Vec<Value>>,
    exists: &'a dyn Fn(&str) -> bool,
}

fn step(d: &StepDef, traced: &[Value], links: &Links) -> Value {
    let settings: Vec<Value> = settings_of(d).into_iter().map(|k| setting(k, traced)).collect();
    let branches = links.at.get(d.key).cloned().unwrap_or_default();
    json!({"key": d.key, "who": d.who, "label": d.label, "meaning": d.meaning, "verb": d.verb.map(declared), "glyph": d.glyph,
           "path": d.path, "when": d.when, "waits": d.who[0], "hold": d.hold.map(HoldKind::name),
           "settings": settings, "files": links::files_of(d, links.exists), "branches": branches})
}

/// settings.<key> with this repo's value and layer (null when config can't
/// be read) and the sd config set command for it.
fn setting(key: &str, traced: &[Value]) -> Value {
    let path = format!("settings.{key}");
    let found = traced.iter().find(|t| t["path"] == path);
    let value = found.map_or(Value::Null, |t| t["value"].clone());
    let layer = found.map_or(Value::Null, |t| t["source"].clone());
    json!({"path": path, "value": value, "layer": layer, "set": set_command(key, &value)})
}

/// sd config set for settings.<key> at <value>, as sd config --help has it:
/// --global for a global-file-only key, else --repo; a string quoted for the
/// shell, --json for a list or object key (by its type, so an unset one too),
/// and <value> where there is none.
fn set_command(key: &str, value: &Value) -> String {
    let scope = if crate::config::global_only(key) { "--global" } else { "--repo" };
    let json = if crate::config::takes_json(key) { " --json" } else { "" };
    let v = match value {
        Value::Null => "<value>".to_string(),
        Value::String(s) => quoted(s),
        Value::Array(_) | Value::Object(_) => quoted(&value.to_string()),
        v => v.to_string(),
    };
    format!("sd config set {scope}{json} settings.{key} {v}")
}

/// <s> as one shell word: bare when it needs no quoting, else in single quotes.
fn quoted(s: &str) -> String {
    if !s.is_empty() && s.bytes().all(|c| c.is_ascii_alphanumeric() || b"@%+=:,./-_".contains(&c)) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// What act::run checks besides a verb's own gate (src/act/mod.rs): the
/// running marker for a verb that writes the branch, before the gate
/// (running::idle, no --force), and the sd-lock for an agent step
/// (attached_check, --force runs anyway): first for an ungated one, but
/// armed for a gated one, so it runs only once enforce says Go.
const CHECKS: [(&str, &str, Level, &str, &str); 2] = [
    ("running", "another step holds the branch's running marker (<git dir>/sheepdog/running/<branch>)", Level::Hard,
     "it ends; if it's hung, kill its pid or remove the marker", "every verb that writes the worktree, the branch or its remote, before its gate"),
    ("attached", "the branch is attached: its session is open interactively, or held (branch.<b>.sd-lock)", Level::Soft,
     "exit that session, sd act release a hold, or sd act <verb> --force", "every agent step: once its gate lets it run, or first if it has no gate"),
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
/// clears}] or null}], checks: [{id, name, level, means, clears, applies}],
/// configError: what sd config said, or null}. A step is {key, who, label,
/// meaning, verb, glyph, path, when, waits, hold, settings: [{path, value,
/// layer, set}], files: [path], branches: [{id, repo, branch}]}: path its
/// place on the stage's happy path or null for an exception (StepDef::path),
/// when the condition a path step is taken under or null for one always
/// taken (path_if), waits the first actor in who, hold what a Hold step does
/// to the list (busy, unsure, top, standin) or null; its settings' values and
/// layers from the repo sd view runs in, its branches those of the watcher's
/// cache under <state> whose NEXT first step it is.
pub fn atlas(state: &Path) -> Value {
    build(crate::config::explain_settings("."), &flock::cached(state))
}

fn build(explained: Result<Vec<Value>, Vec<String>>, cached: &[Cached]) -> Value {
    let (traced, error) = match explained {
        Ok(t) => (t, Value::Null),
        Err(lines) => (vec![], json!(lines.join("\n"))),
    };
    let root = super::map::root();
    let links = Links { at: links::branches_at(cached), exists: &|p| root.join(p).is_file() };
    let step = |d: &StepDef| step(d, &traced, &links);
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
    json!({"holds": holds, "stages": stages, "verbs": VERBS.iter().map(verb).collect::<Vec<_>>(), "checks": checks, "configError": error})
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

    /// The Atlas without reading any config file: atlas() reads the real
    /// global and repo files, and reading one can rewrite it (RENAMED).
    fn unconfigured() -> Value {
        with_config(Ok(vec![]))
    }

    fn with_config(explained: Result<Vec<Value>, Vec<String>>) -> Value {
        build(explained, &[])
    }

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
        let a = unconfigured();
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

    /// The happy path is declared on each step's row, never in the page:
    /// a stage's path steps are numbered 1..n in the order they happen,
    /// and every other step is an exception with the actor it waits on.
    #[test]
    fn each_stages_happy_path_is_numbered_from_one_and_the_rest_are_exceptions() {
        let a = unconfigured();
        let path = |stage: &str| -> Vec<String> {
            let st = a["stages"].as_array().unwrap().iter().find(|s| s["stage"] == stage).unwrap();
            let mut on: Vec<(u64, String)> =
                st["steps"].as_array().unwrap().iter().filter_map(|s| s["path"].as_u64().map(|n| (n, t(s, "key")))).collect();
            on.sort();
            for (i, (n, k)) in on.iter().enumerate() {
                assert_eq!(*n as usize, i + 1, "{stage}: {k} out of sequence in {on:?}");
            }
            on.into_iter().map(|x| x.1).collect()
        };
        assert_eq!(path("build"), ["implement"]);
        assert_eq!(path("push"), ["behind", "push", "open-pr", "retarget"]);
        assert_eq!(path("ci"), ["ci-wait"]);
        assert_eq!(path("review"), ["review", "review-fix"]);
        assert_eq!(path("bot"), ["bot-trigger", "bot-wait"]);
        assert_eq!(path("threads"), ["bot"]);
        assert_eq!(path("merge"), ["draft", "ready", "merged"]);
        let on: usize = a["stages"].as_array().unwrap().iter().map(|s| path(s["stage"].as_str().unwrap()).len()).sum();
        assert_eq!(on, 14);
        for s in steps_in(&a) {
            assert!(s.get("path").is_some(), "{s}");
            assert!(["you", "agent", "run", "wait"].contains(&s["waits"].as_str().unwrap()), "{s}");
        }
        assert!(a["holds"].as_array().unwrap().iter().all(|s| s["path"].is_null()), "a hold is never on the path");
        let when = |k: &str| steps_in(&a).into_iter().find(|s| s["key"] == k).unwrap()["when"].clone();
        assert_eq!(when("behind"), "the target moved");
        assert_eq!(when("retarget"), "the parent merged");
        assert_eq!(when("push"), Value::Null, "always taken");
        assert!(steps_in(&a).iter().all(|s| s["path"].is_u64() || s["when"].is_null()), "when is only for a path step");
    }

    /// The nine steps above the stages do four different things in NEXT's
    /// list, declared on each row (StepDef::hold) so the page names them:
    /// busy collapses the list to itself, unsure warns above it, top sits
    /// above it by whose move, standin replaces the agent steps.
    #[test]
    fn each_hold_declares_what_it_does_to_the_list_and_no_stage_step_does() {
        let a = unconfigured();
        let hold = |k: &str| t(a["holds"].as_array().unwrap().iter().find(|s| s["key"] == k).unwrap(), "hold");
        assert_eq!(hold("running"), "busy");
        assert_eq!(hold("errors"), "unsure");
        assert_eq!(hold("stale"), "unsure");
        assert_eq!(hold("gh-wait"), "unsure");
        assert_eq!(hold("attached"), "top");
        assert_eq!(hold("foreign"), "top");
        assert_eq!(hold("question"), "top");
        assert_eq!(hold("budget"), "standin");
        assert_eq!(hold("failed"), "standin");
        // atlas.html lays the holds out by these four; any other drops off the page
        for s in a["holds"].as_array().unwrap() {
            assert!(["busy", "unsure", "top", "standin"].contains(&t(s, "hold").as_str()), "{s}");
        }
        for st in a["stages"].as_array().unwrap() {
            for s in st["steps"].as_array().unwrap() {
                assert!(s["hold"].is_null(), "{s}");
            }
        }
    }

    #[test]
    fn the_holds_come_first_and_the_stages_are_the_flocks_seven_in_order() {
        let a = unconfigured();
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
        let a = unconfigured();
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
        let a = unconfigured();
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
        let a = unconfigured();
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
        let a = unconfigured();
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

    fn traced(path: &str, value: Value, source: &str) -> Value {
        json!({"path": path, "value": value, "source": source, "overrides": []})
    }

    #[test]
    fn each_step_lists_its_settings_with_the_traced_value_layer_and_set_command() {
        let a = with_config(Ok(vec![
            traced("settings.reviewRounds", json!(5), "repo"),
            traced("settings.botCommand", json!("@bot review"), "global"),
            traced("settings.botLogin", json!(["marge"]), "defaults"),
            traced("settings.agentSteps", json!(2), "defaults"),
            traced("settings.protectedPaths", Value::Null, "unset"),
        ]));
        assert_eq!(a["configError"], Value::Null);
        let find = |k: &str| steps_in(&a).into_iter().find(|s| s["key"] == k).unwrap().clone();
        let of = |k: &str, path: &str| find(k)["settings"].as_array().unwrap().iter().find(|s| s["path"] == path).cloned().unwrap_or_else(|| panic!("{k} has no {path}"));
        assert_eq!(of("review-fix", "settings.reviewRounds"), json!({"path": "settings.reviewRounds", "value": 5, "layer": "repo", "set": "sd config set --repo settings.reviewRounds 5"}));
        assert_eq!(t(&of("bot-trigger", "settings.botCommand"), "set"), "sd config set --repo settings.botCommand '@bot review'");
        assert_eq!(t(&of("read", "settings.botLogin"), "set"), r#"sd config set --repo --json settings.botLogin '["marge"]'"#);
        assert_eq!(t(&of("ci-red", "settings.agentSteps"), "set"), "sd config set --global settings.agentSteps 2");
        let unset = of("ci-config", "settings.protectedPaths");
        assert_eq!((&unset["value"], &unset["layer"], &unset["set"]), (&Value::Null, &json!("unset"), &json!("sd config set --repo --json settings.protectedPaths <value>")));
        let paths: Vec<String> = find("ci-red")["settings"].as_array().unwrap().iter().map(|s| t(s, "path")).collect();
        assert_eq!(paths.iter().map(|p| p.strip_prefix("settings.").unwrap()).collect::<Vec<_>>(), settings_of(crate::next::steps::find("ci-red").unwrap()));
        assert_eq!(find("ready")["settings"], json!([]));
    }

    #[test]
    fn a_config_that_cant_be_read_names_the_settings_without_values() {
        let a = with_config(Err(vec!["x.json: not a JSON object".into()]));
        assert_eq!(a["configError"], "x.json: not a JSON object");
        let s = &steps_in(&a).into_iter().find(|s| s["key"] == "budget").unwrap()["settings"][0];
        assert_eq!((&s["path"], &s["value"], &s["layer"]), (&json!("settings.branchBudget"), &Value::Null, &Value::Null));
    }

    #[test]
    fn a_value_is_quoted_for_the_shell_only_when_it_needs_it() {
        assert_eq!(quoted("cargo"), "cargo");
        assert_eq!(quoted("./scripts/ci.sh"), "./scripts/ci.sh");
        assert_eq!(quoted("make -C app test"), "'make -C app test'");
        assert_eq!(quoted("it's"), r"'it'\''s'");
        assert_eq!(quoted(""), "''");
        assert_eq!(set_command("autoRun", &json!(true)), "sd config set --repo settings.autoRun true");
        assert_eq!(set_command("interval", &json!(300)), "sd config set --global settings.interval 300");
    }

    #[test]
    fn the_shared_checks_apply_by_the_verbs_declaration() {
        let a = unconfigured();
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
