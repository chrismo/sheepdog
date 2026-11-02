// The links between the three pages (#375). An Atlas step's files, in the
// Map's paths, so a step opens into the Map and a Map file lists the steps
// that live in it; and the cached branches by NEXT's first step, so a step
// lists the branches at it now and opens back into the Flock.

use super::flock::Cached;
use crate::act::verbs::VERBS;
use crate::next::steps::{StepDef, STEPS};
use crate::val::t;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

const GATES: &str = "src/act/gates.rs";
const TABLE: &str = "src/next/steps.rs";

/// The files behind <d>: its verb's, derived from the verb's name (each
/// word as src/act/<word>.rs, - as _: restack --resolve is restack.rs and
/// resolve.rs) where <exists> says the Map has it; gates.rs when the verb
/// has a gate; and the step table, which declares every step.
pub fn files_of(d: &StepDef, exists: &dyn Fn(&str) -> bool) -> Vec<String> {
    let mut out = vec![];
    if let Some(v) = d.verb.and_then(|step| VERBS.iter().find(|v| v.step() == step)) {
        for word in v.name.split_whitespace() {
            let f = format!("src/act/{}.rs", word.trim_start_matches('-').replace('-', "_"));
            if exists(&f) {
                out.push(f);
            }
        }
        if v.gate.is_some() {
            out.push(GATES.into());
        }
    }
    out.push(TABLE.into());
    out
}

/// Each file of the Map's groups gets steps: the keys of the Atlas steps
/// whose files include it, in the step table's order.
pub fn add_steps(m: &mut Value) {
    fn files(m: &mut Value) -> impl Iterator<Item = &mut Value> {
        m["groups"].as_array_mut().into_iter().flatten().flat_map(|g| g["files"].as_array_mut().into_iter().flatten())
    }
    let paths: BTreeSet<String> = files(m).map(|f| t(f, "path")).collect();
    let mut by_path: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for d in STEPS {
        for f in files_of(d, &|p| paths.contains(p)) {
            by_path.entry(f).or_default().push(d.key);
        }
    }
    for f in files(m) {
        f["steps"] = json!(by_path.get(&t(f, "path")).cloned().unwrap_or_default());
    }
}

/// {key: [{id, repo, branch}]}: the cached branches whose NEXT first step
/// is key. A branch with no steps, or an unreadable file, is at none.
pub fn branches_at(cached: &[Cached]) -> BTreeMap<String, Vec<Value>> {
    let mut out: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for c in cached {
        let key = c.doc.as_ref().and_then(|d| d.pointer("/state/next/0/key")).and_then(Value::as_str);
        if let Some(key) = key {
            out.entry(key.into()).or_default().push(json!({"id": c.id, "repo": c.repo, "branch": c.branch}));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::next::steps::find;

    const HAVE: [&str; 4] = ["src/act/implement.rs", "src/act/restack.rs", "src/act/resolve.rs", "src/act/push.rs"];
    fn have(p: &str) -> bool {
        HAVE.contains(&p)
    }

    #[test]
    fn a_step_s_files_are_its_verb_s_its_gate_and_the_table() {
        let files = |k: &str| files_of(find(k).unwrap(), &have);
        assert_eq!(files("review-fix"), ["src/act/implement.rs", GATES, TABLE]);
        assert_eq!(files("conflict"), ["src/act/restack.rs", "src/act/resolve.rs", GATES, TABLE]);
        assert_eq!(files("behind"), ["src/act/restack.rs", GATES, TABLE]);
        // trigger has no file the Map lists and no gate; ready has no verb
        assert_eq!(files("bot-trigger"), [TABLE]);
        assert_eq!(files("ready"), [TABLE]);
        assert_eq!(files_of(find("ci-red").unwrap(), &|p| p == "src/act/ci_fix.rs"), ["src/act/ci_fix.rs", GATES, TABLE]);
    }

    #[test]
    fn each_map_file_lists_the_steps_whose_files_include_it() {
        let file = |p: &str| json!({"path": p, "lines": 1});
        let mut m = json!({"groups": [{"name": "act", "files": [file("src/act/push.rs"), file(GATES)]}, {"name": "next", "files": [file(TABLE), file("src/next.rs")]}]});
        add_steps(&mut m);
        assert_eq!(m["groups"][0]["files"][0]["steps"], json!(["push"]));
        let gated: Vec<&str> = m["groups"][0]["files"][1]["steps"].as_array().unwrap().iter().map(|k| k.as_str().unwrap()).collect();
        assert!(gated.contains(&"push") && gated.contains(&"review-fix") && !gated.contains(&"ready"), "{gated:?}");
        assert_eq!(m["groups"][1]["files"][0]["steps"].as_array().unwrap().len(), STEPS.len());
        assert_eq!(m["groups"][1]["files"][1]["steps"], json!([]));
    }

    #[test]
    fn branches_are_grouped_by_next_s_first_step() {
        let cached = |id: &str, doc: Option<Value>| Cached { id: id.into(), repo: "r".into(), branch: id.rsplit('/').next().unwrap().into(), doc };
        let next = |keys: &[&str]| Some(json!({"state": {"next": keys.iter().map(|k| json!({"key": k})).collect::<Vec<_>>()}}));
        let at = branches_at(&[cached("r-1/a", next(&["push", "ci-wait"])), cached("r-1/b", next(&["push"])), cached("r-1/c", next(&[])), cached("r-1/d", None)]);
        assert_eq!(at.keys().collect::<Vec<_>>(), ["push"]);
        assert_eq!(json!(at["push"]), json!([{"id": "r-1/a", "repo": "r", "branch": "a"}, {"id": "r-1/b", "repo": "r", "branch": "b"}]));
    }
}
