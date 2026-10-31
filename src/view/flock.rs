// The Flock page's data (#376): a card per branch the watcher cached at
// <state>/branches/<repo>-<hash>/<branch>.json. key_stage moves into #335's
// STEPS table when that lands.

use crate::val::{at, n, on, t};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

pub const STAGES: [&str; 7] = ["build", "push", "ci", "review", "bot", "threads", "merge"];

/// Each stage's NEXT keys (tests check them against src/next.rs).
const KEYS: [(&str, &[&str]); 7] = [
    ("build", &["implement", "nothing", "nothing-dirty", "reset-dirty", "dirty", "default"]),
    ("push", &["push", "behind", "conflict", "restack-undone", "open-pr", "retarget", "fetch", "ship-parent", "take"]),
    ("ci", &["ci-red", "ci-wait", "ci-rerun", "ci-blocked", "ci-config", "verify-red"]),
    ("review", &["review", "review-fix", "review-wait", "review-decide", "submit", "yours"]),
    ("bot", &["bot-trigger", "bot-wait", "bot-silent"]),
    // "bot" is sd act address on the bot's threads.
    ("threads", &["bot", "answer", "human", "read"]),
    ("merge", &["ready", "draft", "merge-wait", "blocked", "merged", "closed", "parent-wait", "author-wait", "parent-closed"]),
];

fn key_stage(key: &str) -> Option<&'static str> {
    KEYS.iter().find(|(_, ks)| ks.contains(&key)).map(|(stage, _)| *stage)
}

fn count(n: f64, one: &str) -> String {
    format!("{n} {one}{}", if n == 1.0 { "" } else { "s" })
}

/// skip is "not set up": no CI checks, no botCommand.
fn base(s: &Value, stage: &str) -> (&'static str, String) {
    let pr = !at(s, "pr").is_null();
    match stage {
        "build" if n(s, "sync.ahead") > 0.0 => ("done", count(n(s, "sync.ahead"), "commit")),
        "push" if !pr => ("todo", "no PR".into()),
        "push" if n(s, "push.ahead") > 0.0 => ("todo", format!("{} unpushed", n(s, "push.ahead"))),
        "push" if on(s, "sync.conflict") => ("red", "conflict".into()),
        "push" if n(s, "sync.behind") > 0.0 => ("done", format!("{} behind", n(s, "sync.behind"))),
        "push" => ("done", String::new()),
        "ci" => match t(s, "ci.verdict").as_str() {
            "green" => ("done", "green".into()),
            "red" => ("red", "red".into()),
            "none" => ("skip", "no checks".into()),
            "pending" => ("todo", "running".into()),
            "inconclusive" => ("todo", "cancelled".into()),
            v => ("todo", v.into()),
        },
        "review" if t(s, "review.local.status") == "covered" => ("done", "clean".into()),
        "review" if on(s, "review.findings.at_head") && n(s, "review.findings.n") > 0.0 => (
            "red",
            format!("{} · round {}", count(n(s, "review.findings.n"), "finding"), n(s, "review.findings.round")),
        ),
        "bot" => match t(s, "review.bot.status").as_str() {
            "covered" | "replied" => ("done", String::new()),
            "none" | "" if t(s, "bot.command").is_empty() => ("skip", "no botCommand".into()),
            st => ("todo", st.into()),
        },
        "threads" if !pr => ("todo", String::new()),
        "threads" => match n(s, "threads.owed_bot") + n(s, "threads.owed_human") {
            0.0 => ("done", String::new()),
            o => ("todo", format!("{o} owed")),
        },
        "merge" if t(s, "pr.state") == "MERGED" => ("done", "merged".into()),
        _ => ("todo", String::new()),
    }
}

/// {stage, status, text, who}: a stage a NEXT step lights is cur, in its who.
pub fn stages(s: &Value) -> Vec<Value> {
    let next = at(s, "next").as_array().cloned().unwrap_or_default();
    STAGES
        .iter()
        .map(|&stage| {
            let lit = next.iter().find(|x| key_stage(&t(x, "key")) == Some(stage));
            let (status, text) = base(s, stage);
            match lit.map(|x| (t(x, "key"), at(x, "who"))) {
                Some((k, _)) if k == "merged" => json!({"stage": stage, "status": "done", "text": k, "who": null}),
                Some((k, _)) if k == "closed" => json!({"stage": stage, "status": "red", "text": k, "who": null}),
                Some((k, who)) => json!({"stage": stage, "status": "cur", "text": k, "who": who}),
                None => json!({"stage": stage, "status": status, "text": text, "who": null}),
            }
        })
        .collect()
}

/// <doc> is None when the cache file was unreadable.
pub fn card(repo: &str, branch: &str, doc: Option<&Value>) -> Value {
    let Some(doc) = doc else {
        let say = "the watcher's cached state is unreadable; the next pass rewrites it";
        return json!({"repo": repo, "branch": branch, "pr": null, "usd": null, "stages": [],
                      "lead": {"key": "unreadable", "who": "wait", "say": say, "cmd": null}});
    };
    let s = at(doc, "state");
    let lead = match at(s, "next").get(0) {
        None => json!({"key": "idle", "who": "wait", "say": "nothing to do", "cmd": null}),
        Some(x) => json!({"key": at(x, "key"), "who": at(x, "who"), "say": at(x, "say"), "cmd": at(x, "cmd")}),
    };
    let pr = match at(s, "pr") {
        Value::Null => Value::Null,
        p => json!({"number": at(p, "number"), "url": at(p, "url"), "draft": on(p, "draft")}),
    };
    json!({"repo": repo, "branch": branch, "glyph": at(doc, "glyph"), "pr": pr,
           "usd": at(s, "spend.usd"), "lead": lead, "stages": stages(s)})
}

fn json_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            json_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "json") {
            out.push(p);
        }
    }
}

/// GET /api/flock: {at: the last pass's time or null, branches: [card]},
/// sorted by repo, then branch.
pub fn flock(state: &Path) -> Value {
    let at = fs::read_to_string(state.join("last-pass"))
        .ok()
        .and_then(|s| s.lines().next().and_then(|l| l.strip_prefix("last pass: ")).map(str::to_string));
    let mut cards = vec![];
    for repo_dir in fs::read_dir(state.join("branches")).into_iter().flatten().flatten() {
        let dir = repo_dir.path();
        let name = repo_dir.file_name().to_string_lossy().into_owned();
        let repo = name.rsplit_once('-').map_or(name.as_str(), |(r, _)| r).to_string();
        let mut files = vec![];
        json_files(&dir, &mut files);
        for f in files {
            let rel = f.strip_prefix(&dir).unwrap_or(&f).to_string_lossy();
            let branch = rel.strip_suffix(".json").unwrap_or(&rel).to_string();
            let doc = fs::read_to_string(&f).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok());
            cards.push(((repo.clone(), branch.clone()), card(&repo, &branch, doc.as_ref())));
        }
    }
    cards.sort_by(|a, b| a.0.cmp(&b.0));
    json!({"at": at, "branches": cards.into_iter().map(|(_, c)| c).collect::<Vec<_>>()})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(s: &Value) -> Vec<String> {
        stages(s).iter().map(|x| format!("{}:{}", t(x, "status"), t(x, "text"))).collect()
    }

    fn one(s: Value, stage: &str) -> String {
        let i = STAGES.iter().position(|x| *x == stage).unwrap();
        st(&s)[i].clone()
    }

    fn open() -> Value {
        json!({"pr": {"number": 1}, "sync": {"ahead": 1}, "ci": {"verdict": "green"},
               "review": {"local": {"status": "covered"}, "bot": {"status": "none"}}, "next": []})
    }

    fn with(mut s: Value, path: &str, v: Value) -> Value {
        let mut cur = &mut s;
        for k in path.split('.') {
            cur = cur.as_object_mut().unwrap().entry(k).or_insert(json!({}));
        }
        *cur = v;
        s
    }

    #[test]
    fn an_open_reviewed_green_pr_is_done_to_threads() {
        assert_eq!(
            st(&open()),
            ["done:1 commit", "done:", "done:green", "done:clean", "skip:no botCommand", "done:", "todo:"]
        );
    }

    #[test]
    fn ci_status_follows_the_verdict() {
        for (v, want) in [
            ("green", "done:green"),
            ("red", "red:red"),
            ("none", "skip:no checks"),
            ("pending", "todo:running"),
            ("inconclusive", "todo:cancelled"),
            ("blocked", "todo:blocked"),
        ] {
            assert_eq!(one(with(open(), "ci.verdict", json!(v)), "ci"), want, "{v}");
        }
    }

    #[test]
    fn review_findings_at_head_are_red() {
        let s = with(open(), "review", json!({"local": {"status": "uncovered"},
            "findings": {"at_head": true, "n": 2, "round": 3}}));
        assert_eq!(one(s, "review"), "red:2 findings · round 3");
        let s = with(open(), "review", json!({"local": {"status": "uncovered"},
            "findings": {"at_head": false, "n": 2, "round": 3}}));
        assert_eq!(one(s, "review"), "todo:");
    }

    #[test]
    fn a_bot_is_not_set_up_without_a_bot_command() {
        assert_eq!(one(open(), "bot"), "skip:no botCommand");
        let s = with(open(), "bot.command", json!("@bot review"));
        assert_eq!(one(s, "bot"), "todo:none");
        let s = with(open(), "review.bot.status", json!("replied"));
        assert_eq!(one(s, "bot"), "done:");
    }

    #[test]
    fn push_is_red_on_a_conflict_and_todo_without_a_pr() {
        assert_eq!(one(with(open(), "sync.conflict", json!(true)), "push"), "red:conflict");
        assert_eq!(one(with(open(), "push.ahead", json!(2)), "push"), "todo:2 unpushed");
        assert_eq!(one(with(open(), "pr", Value::Null), "push"), "todo:no PR");
    }

    #[test]
    fn owed_threads_are_todo() {
        let s = with(with(open(), "threads.owed_bot", json!(1)), "threads.owed_human", json!(2));
        assert_eq!(one(s, "threads"), "todo:3 owed");
    }

    #[test]
    fn every_next_step_lights_its_stage_in_its_whos_color() {
        let s = with(open(), "next", json!([
            {"key": "review", "who": "agent"}, {"key": "ci-wait", "who": "wait"}, {"key": "running", "who": "wait"},
        ]));
        let got = stages(&s);
        assert_eq!((t(&got[2], "status"), t(&got[2], "who")), ("cur".into(), "wait".into()));
        assert_eq!((t(&got[3], "status"), t(&got[3], "text"), t(&got[3], "who")),
                   ("cur".into(), "review".into(), "agent".into()));
        assert_eq!(got.iter().filter(|x| t(x, "status") == "cur").count(), 2);
    }

    #[test]
    fn a_card_leads_with_next_s_first_step() {
        let doc = json!({"state": with(open(), "next", json!([
            {"key": "ready", "who": "you", "say": "merge it", "cmd": "gh pr merge"},
            {"key": "parent-wait", "who": "wait", "say": "w", "cmd": null},
        ]))});
        let c = card("r", "b", Some(&doc));
        assert_eq!(t(&c, "lead.who"), "you");
        assert_eq!(t(&c, "lead.cmd"), "gh pr merge");
        let idle = card("r", "b", Some(&json!({"state": open()})));
        assert_eq!(t(&idle, "lead.key"), "idle");
    }

    /// Keys that hold every stage, so light none.
    const GUARDS: [&str; 9] = ["attached", "budget", "errors", "failed", "foreign", "gh-wait", "question", "running", "stale"];

    /// The keys src/next.rs gives its steps.
    fn next_keys() -> Vec<String> {
        let re = regex::Regex::new(r#"\b(?:step|agent)\((?:hold, *)?"([a-z-]+)""#).unwrap();
        let src = [include_str!("../next.rs"), include_str!("../next/derived.rs")].concat();
        let mut keys: Vec<String> = re.captures_iter(&src).map(|c| c[1].to_string()).collect();
        keys.sort();
        keys.dedup();
        keys
    }

    #[test]
    fn every_key_next_gives_has_a_stage_or_is_a_guard() {
        let keys = next_keys();
        assert!(keys.len() > 40, "{keys:?}");
        for k in &keys {
            assert!(key_stage(k).is_some() != GUARDS.contains(&k.as_str()), "{k}: a stage, or a guard, not both");
        }
    }

    #[test]
    fn the_stage_table_names_no_key_next_never_gives() {
        let keys = next_keys();
        for k in KEYS.iter().flat_map(|(_, ks)| ks.iter()).chain(GUARDS.iter()) {
            assert!(keys.iter().any(|x| x == k), "{k}");
        }
    }

    #[test]
    fn the_bot_key_addresses_threads_and_the_fix_keys_light_their_stages() {
        assert_eq!(key_stage("bot"), Some("threads"));
        assert_eq!(key_stage("ci-red"), Some("ci"));
        assert_eq!(key_stage("review-fix"), Some("review"));
    }

    #[test]
    fn a_merged_pr_is_done_and_a_closed_one_red() {
        let s = with(open(), "next", json!([{"key": "merged", "who": "you"}]));
        assert_eq!(one(s, "merge"), "done:merged");
        let s = with(open(), "next", json!([{"key": "closed", "who": "you"}]));
        assert_eq!(one(s, "merge"), "red:closed");
    }
}
