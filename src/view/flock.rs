// The Flock page's data (#376): a card per branch the watcher cached at
// <state>/branches/<repo>-<hash>/<branch>.json. key_stage moves into #335's
// STEPS table when that lands.

use crate::val::{at, n, on, t};
use serde_json::{json, Value};
use std::fs;
use crate::watch::json_files;
use std::path::Path;

pub const STAGES: [&str; 7] = ["build", "push", "ci", "review", "bot", "threads", "merge"];

/// Each stage's NEXT keys, with the word a lit stage shows (tests check
/// the keys against src/next.rs).
const KEYS: [(&str, &[(&str, &str)]); 7] = [
    ("build", &[("implement", "implement"), ("nothing", "nothing to ship"), ("nothing-dirty", "nothing to ship"),
                ("reset-dirty", "reset away"), ("dirty", "uncommitted"), ("default", "start")]),
    ("push", &[("push", "push"), ("behind", "behind"), ("conflict", "conflict"), ("restack-undone", "restack undone"),
               ("open-pr", "open PR"), ("retarget", "retarget"), ("fetch", "fetch"), ("ship-parent", "ship parent"),
               ("take", "take back"), ("foreign-authors", "others' commits")]),
    ("ci", &[("ci-red", "fix"), ("ci-wait", "running"), ("ci-rerun", "rerun"), ("ci-blocked", "blocked"),
             ("ci-config", "set up CI"), ("verify-red", "verify")]),
    ("review", &[("review", "queued"), ("review-fix", "fix"), ("review-wait", "waiting"), ("review-decide", "decide"),
                 ("review-held", "held"), ("submit", "submit"), ("yours", "yours")]),
    ("bot", &[("bot-trigger", "ask"), ("bot-wait", "waiting"), ("bot-silent", "silent")]),
    // "bot" is sd act address on the bot's threads.
    ("threads", &[("bot", "address"), ("answer", "answer"), ("human", "reply"), ("read", "read")]),
    ("merge", &[("ready", "ready"), ("draft", "mark ready"), ("merge-wait", "merging"), ("blocked", "blocked"),
                ("merged", "merged"), ("closed", "closed"), ("parent-wait", "after parent"), ("author-wait", "author"),
                ("parent-closed", "parent closed")]),
];

fn lookup(key: &str) -> Option<(&'static str, &'static str)> {
    KEYS.iter().find_map(|(stage, ks)| ks.iter().find(|(k, _)| *k == key).map(|(_, label)| (*stage, *label)))
}

fn key_stage(key: &str) -> Option<&'static str> {
    lookup(key).map(|(stage, _)| stage)
}

/// The stage a NEXT step in <s> lights: answer (chrismo's answer, waiting)
/// belongs to the stage of the step that asked, ci-fix or address.
pub(super) fn step_stage(key: &str, s: &Value) -> Option<&'static str> {
    match (key, t(s, "question.step").as_str()) {
        ("answer", "ci-fix") => Some("ci"),
        _ => key_stage(key),
    }
}

fn label(key: &str) -> String {
    lookup(key).map_or(key.to_string(), |(_, l)| l.to_string())
}

fn count(n: f64, one: &str) -> String {
    format!("{n} {one}{}", if n == 1.0 { "" } else { "s" })
}

/// A review pin covering HEAD, as sd state's covers() reads it.
fn covers(s: &Value, pin: &str) -> bool {
    matches!(t(s, &format!("{pin}.status")).as_str(), "covered" | "rewritten-same")
}

/// Checks on an older SHA give no verdict (O5).
fn ci(s: &Value) -> (&'static str, String) {
    let (status, text) = match t(s, "ci.verdict").as_str() {
        "none" => ("skip", "no checks"),
        _ if at(s, "ci.for_head") == &Value::Bool(false) => ("todo", "old head"),
        "green" => ("done", "green"),
        "red" => ("red", "red"),
        "pending" => ("todo", "running"),
        "inconclusive" => ("todo", "cancelled"),
        v => return ("todo", v.into()),
    };
    (status, text.into())
}

/// Covering HEAD with replied false is a trigger the bot hasn't answered.
fn bot(s: &Value) -> (&'static str, String) {
    if covers(s, "review.bot") {
        return match at(s, "review.bot.replied") {
            Value::Bool(false) => ("todo", "waiting".into()),
            _ => ("done", String::new()),
        };
    }
    match t(s, "review.bot.status").as_str() {
        _ if t(s, "bot.command").is_empty() => ("skip", "no botCommand".into()),
        "uncovered" => ("todo", format!("{} since", n(s, "review.bot.uncovered"))),
        "rewritten-changed" => ("todo", "rewritten".into()),
        _ => ("todo", "not asked".into()),
    }
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
        "ci" => ci(s),
        "review" if covers(s, "review.local") => ("done", "clean".into()),
        "review" if covers(s, "review.accepted") => ("done", "accepted".into()),
        "review" if on(s, "review.findings.at_head") && n(s, "review.findings.n") > 0.0 => (
            "red",
            format!("{} · round {}", count(n(s, "review.findings.n"), "finding"), n(s, "review.findings.round")),
        ),
        "bot" => bot(s),
        "threads" if !pr => ("todo", String::new()),
        // gh unread leaves the counts null.
        "threads" if at(s, "threads.blocking").is_null() => ("todo", "unknown".into()),
        "threads" => match n(s, "threads.blocking") {
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
            let lit = next.iter().find(|x| step_stage(&t(x, "key"), s) == Some(stage));
            let (status, text) = base(s, stage);
            match lit.map(|x| (t(x, "key"), at(x, "who"))) {
                Some((k, _)) if k == "merged" => json!({"stage": stage, "status": "done", "text": k, "who": null}),
                Some((k, _)) if k == "closed" => json!({"stage": stage, "status": "red", "text": k, "who": null}),
                Some((k, who)) => json!({"stage": stage, "status": "cur", "text": label(&k), "who": who}),
                None => json!({"stage": stage, "status": status, "text": text, "who": null}),
            }
        })
        .collect()
}

/// A branch's file in the watcher's cache. id is its path under
/// branches/, less .json: <repo>-<hash>/<branch>.
pub struct Cached {
    pub id: String,
    pub repo: String,
    pub branch: String,
    /// None when the file was unreadable.
    pub doc: Option<Value>,
}

pub fn cached(state: &Path) -> Vec<Cached> {
    let mut out = vec![];
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
            out.push(Cached { id: format!("{name}/{branch}"), repo: repo.clone(), branch, doc });
        }
    }
    out.sort_by(|a, b| (&a.repo, &a.branch).cmp(&(&b.repo, &b.branch)));
    out
}

pub fn card(c: &Cached) -> Value {
    let (id, repo, branch) = (&c.id, &c.repo, &c.branch);
    let Some(doc) = &c.doc else {
        let say = "the watcher's cached state is unreadable; the next pass rewrites it";
        return json!({"id": id, "repo": repo, "branch": branch, "pr": null, "usd": null, "stages": [],
                      "lead": {"key": "unreadable", "who": "wait", "say": say, "cmd": null}});
    };
    let s = at(doc, "state");
    let lead = match at(s, "next").get(0) {
        // The watcher records no steps as sd state being unsure.
        None => json!({"key": "unsure", "who": "wait", "cmd": null,
                       "say": at(doc, "note").as_str().unwrap_or("sd state gave no next steps")}),
        Some(x) => json!({"key": at(x, "key"), "who": at(x, "who"), "say": at(x, "say"), "cmd": at(x, "cmd")}),
    };
    let pr = match at(s, "pr") {
        Value::Null => Value::Null,
        p => json!({"number": at(p, "number"), "url": at(p, "url"), "draft": on(p, "draft")}),
    };
    json!({"id": id, "repo": repo, "branch": branch, "glyph": at(doc, "glyph"), "pr": pr,
           "usd": at(s, "spend.usd"), "lead": lead, "stages": stages(s)})
}

/// GET /api/flock: {at: the last pass's time or null, branches: [card]}.
pub fn flock(state: &Path) -> Value {
    let at = fs::read_to_string(state.join("last-pass"))
        .ok()
        .and_then(|s| s.lines().next().and_then(|l| l.strip_prefix("last pass: ")).map(str::to_string));
    json!({"at": at, "branches": cached(state).iter().map(card).collect::<Vec<_>>()})
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

    fn cached(doc: Value) -> Cached {
        Cached { id: "r-0123abcd/b".into(), repo: "r".into(), branch: "b".into(), doc: Some(doc) }
    }

    fn open() -> Value {
        json!({"pr": {"number": 1}, "sync": {"ahead": 1}, "ci": {"verdict": "green"},
               "review": {"local": {"status": "covered"}, "bot": {"status": "none"}},
               "threads": {"owed_bot": 0, "owed_human": 0, "blocking": 0, "info": 0}, "next": []})
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
        assert_eq!(one(s, "bot"), "todo:not asked");
    }

    #[test]
    fn a_bot_covering_head_is_done_once_it_replied() {
        let bot = |v: Value| with(with(open(), "bot.command", json!("@bot review")), "review.bot", v);
        assert_eq!(one(bot(json!({"status": "covered", "replied": false})), "bot"), "todo:waiting");
        assert_eq!(one(bot(json!({"status": "covered", "replied": true})), "bot"), "done:");
        assert_eq!(one(bot(json!({"status": "rewritten-same", "replied": null})), "bot"), "done:");
        assert_eq!(one(bot(json!({"status": "uncovered", "uncovered": 2})), "bot"), "todo:2 since");
        assert_eq!(one(bot(json!({"status": "rewritten-changed"})), "bot"), "todo:rewritten");
    }

    #[test]
    fn review_is_done_when_local_or_accepted_covers_head() {
        let s = with(open(), "review.local.status", json!("rewritten-same"));
        assert_eq!(one(s, "review"), "done:clean");
        let s = with(with(open(), "review.local.status", json!("uncovered")), "review.accepted.status", json!("covered"));
        assert_eq!(one(s, "review"), "done:accepted");
    }

    #[test]
    fn ci_on_an_older_head_is_no_verdict() {
        let s = with(with(open(), "ci.verdict", json!("red")), "ci.for_head", json!(false));
        assert_eq!(one(s, "ci"), "todo:old head");
        let s = with(with(open(), "ci.verdict", json!("none")), "ci.for_head", json!(false));
        assert_eq!(one(s, "ci"), "skip:no checks");
    }

    #[test]
    fn push_is_red_on_a_conflict_and_todo_without_a_pr() {
        assert_eq!(one(with(open(), "sync.conflict", json!(true)), "push"), "red:conflict");
        assert_eq!(one(with(open(), "push.ahead", json!(2)), "push"), "todo:2 unpushed");
        assert_eq!(one(with(open(), "pr", Value::Null), "push"), "todo:no PR");
    }

    #[test]
    fn owed_threads_are_todo() {
        assert_eq!(one(with(open(), "threads.blocking", json!(3)), "threads"), "todo:3 owed");
    }

    #[test]
    fn every_next_step_lights_its_stage_in_its_whos_color() {
        let s = with(open(), "next", json!([
            {"key": "review", "who": "agent"}, {"key": "ci-wait", "who": "wait"}, {"key": "running", "who": "wait"},
        ]));
        let got = stages(&s);
        assert_eq!((t(&got[2], "status"), t(&got[2], "who")), ("cur".into(), "wait".into()));
        assert_eq!((t(&got[3], "status"), t(&got[3], "text"), t(&got[3], "who")),
                   ("cur".into(), "queued".into(), "agent".into()));
        assert_eq!(got.iter().filter(|x| t(x, "status") == "cur").count(), 2);
    }

    #[test]
    fn a_card_leads_with_next_s_first_step() {
        let doc = json!({"state": with(open(), "next", json!([
            {"key": "ready", "who": "you", "say": "merge it", "cmd": "gh pr merge"},
            {"key": "parent-wait", "who": "wait", "say": "w", "cmd": null},
        ]))});
        let c = card(&cached(doc));
        assert_eq!(t(&c, "lead.who"), "you");
        assert_eq!(t(&c, "lead.cmd"), "gh pr merge");
    }

    #[test]
    fn no_next_steps_is_unsure_as_the_watcher_records_it() {
        let doc = json!({"note": "sd state gave no next steps", "state": open()});
        let c = card(&cached(doc));
        assert_eq!((t(&c, "lead.key"), t(&c, "lead.who")), ("unsure".into(), "wait".into()));
        assert_eq!(t(&c, "lead.say"), "sd state gave no next steps");
    }

    #[test]
    fn threads_gh_couldnt_read_are_unknown_not_done() {
        assert_eq!(one(with(open(), "threads.blocking", Value::Null), "threads"), "todo:unknown");
    }

    #[test]
    fn info_items_aren_t_owed() {
        let s = with(with(open(), "threads.blocking", json!(1)), "threads.info", json!(2));
        assert_eq!(one(s, "threads"), "todo:1 owed");
        let s = with(with(open(), "threads.blocking", json!(0)), "threads.info", json!(2));
        assert_eq!(one(s, "threads"), "done:");
    }

    #[test]
    fn a_waiting_answer_lights_its_question_s_stage() {
        let s = with(with(open(), "question", json!({"step": "ci-fix"})), "next", json!([{"key": "answer", "who": "wait"}]));
        assert_eq!(one(s, "ci"), "cur:answer");
    }

    #[test]
    fn a_lit_stage_shows_its_step_s_word_not_its_key() {
        let s = with(open(), "next", json!([{"key": "open-pr", "who": "run"}, {"key": "ci-red", "who": "agent"}]));
        assert_eq!(one(s.clone(), "push"), "cur:open PR");
        assert_eq!(one(s, "ci"), "cur:fix");
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
        for k in KEYS.iter().flat_map(|(_, ks)| ks.iter().map(|(k, _)| k)).chain(GUARDS.iter()) {
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
