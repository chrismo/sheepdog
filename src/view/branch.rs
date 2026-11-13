// A branch's drill-in (#376): its card, a flow chart's nodes, all of NEXT,
// and what's owed, from the watcher's cache and usage.sup.

use super::flock::{self, Cached};
use crate::usage::{shaped, sup_record};
use crate::val::{at, on, t};
use crate::watch::repo_key;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// The fence's seven stages, and the two loops back to push: ci-fix off CI,
/// fix findings off review.
pub const NODES: [&str; 9] = ["build", "push", "ci", "review", "bot", "threads", "merge", "cifix", "fix"];

fn key_node(key: &str, s: &Value) -> Option<&'static str> {
    match (key, t(s, "question.step").as_str()) {
        ("ci-red", _) | ("answer", "ci-fix") => Some("cifix"),
        ("review-fix", _) => Some("fix"),
        (k, _) => flock::step_stage(k, s),
    }
}

/// The branch's usage.sup records since sd act forget's last reset row (#349).
pub(super) fn records(usage: &str, repo_dir: &str, branch: &str) -> Vec<HashMap<String, Value>> {
    let mut out = vec![];
    for r in usage.lines().filter(|l| shaped(l)).filter_map(sup_record) {
        let field = |k: &str| r.get(k).and_then(Value::as_str).unwrap_or_default();
        if field("branch") != branch || repo_key(field("repo")) != repo_dir {
            continue;
        }
        if r.get("reset").is_some_and(|v| v == true) {
            out.clear();
        } else {
            out.push(r);
        }
    }
    out
}

/// Agent steps per node. usage.sup records a review-fix round as implement,
/// so an implement after the branch's first review counts as a fix.
fn counts(usage: &str, repo_dir: &str, branch: &str) -> Map<String, Value> {
    let mut n: Map<String, Value> = NODES.iter().map(|k| (k.to_string(), json!(0))).collect();
    let mut reviewed = false;
    for r in records(usage, repo_dir, branch) {
        let node = match r.get("step").and_then(Value::as_str).unwrap_or_default() {
            "implement" if reviewed => "fix",
            "implement" => "build",
            "review" => {
                reviewed = true;
                "review"
            }
            "ci-fix" => "cifix",
            "address" => "threads",
            _ => continue,
        };
        n[node] = json!(n[node].as_u64().unwrap_or(0) + 1);
    }
    n
}

/// {node: {count, who}}: who is the first NEXT step lighting it, else null.
fn flow(s: &Value, usage: &str, repo_dir: &str, branch: &str) -> Value {
    let n = counts(usage, repo_dir, branch);
    let next = at(s, "next").as_array().cloned().unwrap_or_default();
    let nodes: Map<String, Value> = NODES
        .iter()
        .map(|&k| {
            let who = next.iter().find(|x| key_node(&t(x, "key"), s) == Some(k)).map_or(Value::Null, |x| at(x, "who").clone());
            (k.to_string(), json!({"count": n[k], "who": who}))
        })
        .collect();
    Value::Object(nodes)
}

fn owed(s: &Value) -> Value {
    let findings: Vec<&Value> = match at(s, "review.findings.items").as_array() {
        Some(items) if on(s, "review.findings.at_head") => items.iter().filter(|x| t(x, "action") != "declined").collect(),
        _ => vec![],
    };
    let failing = if at(s, "ci.for_head") == &Value::Bool(false) { json!([]) } else { at(s, "ci.failing").clone() };
    let question = if at(s, "question.answer").is_null() { at(s, "question") } else { &Value::Null };
    // null when gh couldn't be read: unknown, not none.
    let threads = if at(s, "threads.blocking").is_null() { &Value::Null } else { at(s, "threads.items") };
    json!({"findings": findings, "failing": failing, "question": question, "threads": threads})
}

pub(super) fn usage(state: &Path) -> String {
    fs::read(state.join("usage.sup")).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default()
}

/// The cached branch with this id: looked up in the listing, never made into a path.
pub(super) fn find(state: &Path, id: &str) -> Option<Cached> {
    flock::cached(state).into_iter().find(|c| c.id == id)
}

/// None for an id the cache listing doesn't have. An unreadable file's
/// card has null flow, next, owed and act. act is the step the drill-in's
/// button runs (view/act.rs, #396), acting its key while it runs (or the
/// step holding the branch's running marker, flock::live, #503), and
/// settings the branch's settings.* with their layers (view/settings.rs),
/// null when its repo isn't registered.
pub fn detail(state: &Path, id: &str) -> Option<Value> {
    let c = find(state, id)?;
    let mut card = flock::card(&c);
    for k in ["flow", "next", "owed", "act"] {
        card[k] = Value::Null;
    }
    if let Some(d) = &c.doc {
        let s = at(d, "state");
        let repo_dir = id.split_once('/').map_or("", |(d, _)| d);
        card["flow"] = flow(s, &usage(state), repo_dir, &c.branch);
        card["next"] = at(s, "next").clone();
        card["owed"] = owed(s);
        card["act"] = super::act::offered(s);
    }
    card["acting"] = super::act::running(id);
    let repo = super::act::repo_of(id);
    let run = repo.as_deref().and_then(|r| crate::running::live(r).remove(&c.branch)).unwrap_or(Value::Null);
    flock::live(&mut card, &run);
    card["settings"] = repo.map_or(Value::Null, |repo| super::settings::list(&repo, &c.branch));
    Some(card)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(repo: &str, b: &str, step: &str) -> String {
        format!("{{time:2026-10-31T10:00:00Z,repo:\"{repo}\",branch:\"{b}\",step:\"{step}\",cost_usd:0.5,context:1}}")
    }

    fn count_of(usage: &[String], node: &str) -> u64 {
        counts(&usage.join("\n"), &repo_key("/src/r"), "b")[node].as_u64().unwrap()
    }

    #[test]
    fn the_flow_s_nodes_are_the_fence_s_stages_and_the_two_fix_loops() {
        assert_eq!(NODES[..7], flock::STAGES);
        assert_eq!(NODES[7..], ["cifix", "fix"]);
    }

    #[test]
    fn an_implement_after_the_first_review_is_a_fix() {
        let u = ["implement", "review", "implement", "review", "implement"].map(|s| rec("/src/r", "b", s));
        assert_eq!((count_of(&u, "build"), count_of(&u, "review"), count_of(&u, "fix")), (1, 2, 2));
    }

    #[test]
    fn only_this_repo_s_branch_counts() {
        let u = [rec("/src/r", "b", "ci-fix"), rec("/src/r", "c", "ci-fix"), rec("/other/r", "b", "ci-fix")];
        assert_eq!(count_of(&u, "cifix"), 1);
    }

    #[test]
    fn a_reset_starts_the_count_again() {
        let u = [rec("/src/r", "b", "implement"), rec("/src/r", "b", "review"),
                 "{time:2026-10-31T11:00:00Z,repo:\"/src/r\",branch:\"b\",reset:true}".into(),
                 rec("/src/r", "b", "implement")];
        assert_eq!((count_of(&u, "build"), count_of(&u, "review"), count_of(&u, "fix")), (1, 0, 0));
    }

    #[test]
    fn next_lights_the_loops_for_the_fix_keys() {
        let s = json!({"next": [{"key": "ci-red", "who": "agent"}, {"key": "review-fix", "who": "agent"},
                                {"key": "bot", "who": "agent"}]});
        let f = flow(&s, "", "r-0", "b");
        assert_eq!((t(&f, "cifix.who"), t(&f, "fix.who"), t(&f, "ci.who")), ("agent".into(), "agent".into(), String::new()));
    }

    #[test]
    fn bot_steps_light_the_bot_node_thread_steps_the_threads_node_and_address_counts_there() {
        let s = json!({"next": [{"key": "bot", "who": "agent"}]});
        assert_eq!(t(&flow(&s, "", "r-0", "b"), "threads.who"), "agent");
        for k in ["bot-trigger", "bot-wait", "bot-silent"] {
            assert_eq!(key_node(k, &Value::Null), Some("bot"), "{k}");
        }
        for k in ["bot", "human", "read"] {
            assert_eq!(key_node(k, &Value::Null), Some("threads"), "{k}");
        }
        assert_eq!(count_of(&[rec("/src/r", "b", "address")], "threads"), 1);
    }

    #[test]
    fn findings_and_failing_checks_are_owed_only_at_head() {
        let s = json!({"review": {"findings": {"at_head": false, "items": [{"body": "x"}]}},
                       "ci": {"for_head": false, "failing": ["t"]}});
        assert_eq!(owed(&s)["findings"], json!([]));
        assert_eq!(owed(&s)["failing"], json!([]));
    }

    #[test]
    fn a_waiting_answer_lights_its_question_s_step() {
        let s = |step: &str| json!({"question": {"step": step}, "next": [{"key": "answer", "who": "wait"}]});
        assert_eq!(t(&flow(&s("ci-fix"), "", "r-0", "b"), "cifix.who"), "wait");
        assert_eq!(t(&flow(&s("address"), "", "r-0", "b"), "threads.who"), "wait");
    }

    #[test]
    fn threads_gh_couldnt_read_are_null_not_none() {
        let s = json!({"threads": {"blocking": null, "items": []}});
        assert_eq!(owed(&s)["threads"], Value::Null);
    }

    #[test]
    fn an_answered_question_isn_t_owed() {
        let q = |answer: Value| json!({"question": {"text": "which?", "answer": answer}});
        assert_eq!(owed(&q(json!("this one")))["question"], Value::Null);
        assert_eq!(t(&owed(&q(Value::Null)), "question.text"), "which?");
    }

    #[test]
    fn a_declined_finding_isn_t_owed() {
        let s = json!({"review": {"findings": {"at_head": true, "items": [
            {"body": "a", "action": "declined"}, {"body": "b", "severity": "nit"}]}}});
        assert_eq!(owed(&s)["findings"], json!([{"body": "b", "severity": "nit"}]));
    }
}
