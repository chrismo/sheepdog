// A branch's drill-in (#376): its card, a flow chart's nodes, all of NEXT,
// and what's owed, from the watcher's cache and usage.sup.

use super::flock::{self, Cached};
use crate::usage::{shaped, sup_record};
use crate::val::{at, on, t};
use crate::watch::repo_key;
use serde_json::{json, Map, Value};
use std::fs;
use std::path::Path;

/// Implement → push → CI → review → bot & threads → merge, and the two
/// loops back to push.
pub const NODES: [&str; 8] = ["implement", "push", "ci", "review", "threads", "merge", "cifix", "fix"];

fn key_node(key: &str) -> Option<&'static str> {
    match key {
        "ci-red" => Some("cifix"),
        "review-fix" => Some("fix"),
        k => match flock::key_stage(k)? {
            "build" => Some("implement"),
            "bot" => Some("threads"),
            s => Some(s),
        },
    }
}

/// Agent steps per node from usage.sup's records for the branch, since sd
/// act forget's last reset row (#349). usage.sup records a review-fix round
/// as implement, so an implement after the branch's first review counts as
/// a fix.
fn counts(usage: &str, repo_dir: &str, branch: &str) -> Map<String, Value> {
    let zero = || NODES.iter().map(|k| (k.to_string(), json!(0))).collect::<Map<_, _>>();
    let (mut n, mut reviewed) = (zero(), false);
    for r in usage.lines().filter(|l| shaped(l)).filter_map(sup_record) {
        let field = |k: &str| r.get(k).and_then(Value::as_str).unwrap_or_default();
        if field("branch") != branch || repo_key(field("repo")) != repo_dir {
            continue;
        }
        if r.get("reset").is_some_and(|v| v == true) {
            (n, reviewed) = (zero(), false);
            continue;
        }
        let node = match field("step") {
            "implement" if reviewed => "fix",
            "implement" => "implement",
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
            let who = next.iter().find(|x| key_node(&t(x, "key")) == Some(k)).map_or(Value::Null, |x| at(x, "who").clone());
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
    json!({"findings": findings, "failing": failing, "question": question,
           "threads": at(s, "threads.items")})
}

pub fn detail(state: &Path, id: &str) -> Option<Value> {
    let c: Cached = flock::cached(state).into_iter().find(|c| c.id == id)?;
    let mut card = flock::card(&c);
    let s = c.doc.as_ref().map_or(&Value::Null, |d| at(d, "state"));
    let usage = fs::read(state.join("usage.sup")).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let repo_dir = id.split_once('/').map_or("", |(d, _)| d);
    card["flow"] = flow(s, &usage, repo_dir, &c.branch);
    card["next"] = at(s, "next").clone();
    card["owed"] = owed(s);
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
    fn an_implement_after_the_first_review_is_a_fix() {
        let u = ["implement", "review", "implement", "review", "implement"].map(|s| rec("/src/r", "b", s));
        assert_eq!((count_of(&u, "implement"), count_of(&u, "review"), count_of(&u, "fix")), (1, 2, 2));
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
        assert_eq!((count_of(&u, "implement"), count_of(&u, "review"), count_of(&u, "fix")), (1, 0, 0));
    }

    #[test]
    fn next_lights_the_loops_for_the_fix_keys() {
        let s = json!({"next": [{"key": "ci-red", "who": "agent"}, {"key": "review-fix", "who": "agent"},
                                {"key": "bot", "who": "agent"}]});
        let f = flow(&s, "", "r-0", "b");
        assert_eq!((t(&f, "cifix.who"), t(&f, "fix.who"), t(&f, "ci.who")), ("agent".into(), "agent".into(), String::new()));
    }

    #[test]
    fn bot_and_thread_steps_light_the_threads_node_and_address_counts_there() {
        let s = json!({"next": [{"key": "bot", "who": "agent"}]});
        assert_eq!(t(&flow(&s, "", "r-0", "b"), "threads.who"), "agent");
        for k in ["bot-trigger", "bot-wait", "bot-silent", "answer", "human", "read"] {
            assert_eq!(key_node(k), Some("threads"), "{k}");
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
