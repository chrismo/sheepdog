// The verbs that write to GitHub, or mark what it said as read: ack and
// trigger (A5).

use super::{cfg, die, no_base_why, state_json};
use crate::config::one_value;
use crate::git::{self, git, rev, strip_nl};
use crate::proc;
use crate::time;
use crate::val::{at, n, on, sj, t, text};
use regex::Regex;
use serde_json::Value;
use std::sync::OnceLock;

fn state(b: &str) -> Value {
    state_json(".", b).unwrap_or_else(|e| {
        eprint!("{e}");
        die("sd state failed")
    })
}

fn git_or_exit(args: &[&str]) {
    let o = git(".", args);
    if !o.ok {
        eprint!("{}", o.stderr);
        std::process::exit(o.code);
    }
}

// ── ack ──────────────────────────────────────────────────────────────────────

/// The owed items ack takes from sd state: the named ones, else every info
/// item. Ok is each one's id and what it is; Err is why some can't be acked,
/// and then none are.
fn ack_items(s: &Value, ids: &[String], b: &str) -> Result<Vec<(String, String)>, Vec<String>> {
    let (mut picked, mut bad, mut found) = (Vec::new(), Vec::new(), Vec::new());
    for it in at(s, "threads.items").as_array().into_iter().flatten() {
        let id = t(it, "id");
        if id.is_empty() || !(ids.contains(&id) || ids.is_empty() && t(it, "mark") == "i") {
            continue;
        }
        let ty = t(it, "type");
        let why = match (ty.as_str(), t(it, "kind").as_str()) {
            ("thread", _) => "is a thread; resolve it on GitHub".to_string(),
            (_, "bot") => String::new(),
            _ => format!("is a human {ty}; reply to it"),
        };
        if why.is_empty() {
            let who = [at(it, "author"), at(it, "last")].into_iter().find(|v| !v.is_null()).map_or(String::new(), text);
            picked.push((id.clone(), format!("{ty}  {who}  {}  {}", t(it, "excerpt"), t(it, "url"))));
        } else {
            bad.push(format!("{id} {why}"));
        }
        found.push(id);
    }
    bad.extend(ids.iter().filter(|id| !found.contains(id)).map(|id| format!("{id} isn't owed on {b}")));
    if bad.is_empty() {
        Ok(picked)
    } else {
        Err(bad)
    }
}

/// Mark owed bot items read (branch.<b>.sd-handled) without replying; with
/// --question, drop the agent's standing question instead.
pub fn ack(b: &str, ids: &[String], question: bool) {
    if question {
        if !ids.is_empty() {
            die("--question takes no --id");
        }
        let r = format!("refs/sheepdog/{b}/question");
        let q = git(".", &["cat-file", "blob", &r]);
        if !q.ok {
            println!("no question to ack on {b}");
            return;
        }
        git_or_exit(&["update-ref", "-d", &r]);
        super::step::answer_drop(b);
        let step = match one_value(&q.stdout) {
            Some(v) if !at(&v, "step").is_null() => t(&v, "step"),
            _ => "?".into(),
        };
        println!("acked the agent's question on {b} ({step})");
        return;
    }
    let s = state(b);
    if at(&s, "threads.owed_bot").is_null() {
        die(format!("can't read the PR from GitHub: {}", sj(at(&s, "errors"), "; ")));
    }
    let picked = ack_items(&s, ids, b).unwrap_or_else(|bad| {
        for l in bad {
            eprintln!("sd act: {l}");
        }
        std::process::exit(1)
    });
    if picked.is_empty() {
        println!("nothing to ack on {b}");
        return;
    }
    for (id, _) in &picked {
        git_or_exit(&["config", "--add", &format!("branch.{b}.sd-handled"), id]);
    }
    println!("acked {} on {b}:", picked.len());
    for (_, what) in &picked {
        println!("  {what}");
    }
}

// ── trigger ──────────────────────────────────────────────────────────────────

/// What sd state says of triggering: Hard can't be forced, Soft can, and
/// Done is done already (--force does it again).
#[derive(Debug, PartialEq)]
enum Gate {
    Hard(String),
    Done(String),
    Soft(String),
    Go,
}

fn trigger_gate(s: &Value) -> Gate {
    let b = t(s, "branch");
    let state = at(s, "pr.state");
    let (ahead, behind) = (n(s, "push.ahead"), n(s, "push.behind"));
    let ours = on(s, "push.remote_ours") && on(s, "push.remote_made");
    let not_pushed = |todo: &str| Gate::Hard(format!("{b} isn't on the remote at HEAD; the bot reviews what GitHub has: {todo}"));
    let covers = |k: &str| ["covered", "rewritten-same"].contains(&t(s, &format!("review.{k}.status")).as_str());
    if at(s, "pr").is_null() {
        let why = sj(at(s, "errors"), "; ");
        Gate::Hard(format!("no PR for {b}{}", if why.is_empty() { why } else { format!(": {why}") }))
    } else if !state.is_null() && state != "OPEN" {
        Gate::Hard(format!("PR #{} is {}", t(s, "pr.number"), text(state).to_lowercase()))
    } else if behind > 0.0 && ahead == 0.0 && ours && on(s, "push.dirty") {
        not_pushed("commit or remove the worktree's changes, then sd act push")
    } else if at(s, "push.remote_sha").is_null() || ahead > 0.0 || on(s, "push.diverged") || behind > 0.0 && ours {
        not_pushed("sd act push first")
    } else if behind > 0.0 {
        not_pushed("take its commits first (git merge --ff-only)")
    } else if t(s, "review.bot.status") == "covered" {
        Gate::Done(format!("the bot was already triggered on {}", t(s, "head").chars().take(7).collect::<String>()))
    } else if !(covers("local") || covers("accepted")) {
        Gate::Soft("neither the local review nor an accept covers HEAD (sd act review)".into())
    } else {
        Gate::Go
    }
}

/// The host and owner/repo of a PR's URL.
fn pr_repo(url: &str) -> Option<(String, String)> {
    static URL: OnceLock<Regex> = OnceLock::new();
    let r = URL.get_or_init(|| Regex::new(r"^https://([^/]+)/([^/]+/[^/]+)/pull/[0-9]+$").expect("a fixed pattern compiles"));
    let c = r.captures(url)?;
    Some((c[1].to_string(), c[2].to_string()))
}

fn is_time(at: &str) -> bool {
    static TIME: OnceLock<Regex> = OnceLock::new();
    TIME.get_or_init(|| Regex::new(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T").expect("a fixed pattern compiles")).is_match(at)
}

/// The comment goes over REST, which answers with its created_at: the
/// trigger time by GitHub's clock, which sd state compares with the bot's
/// replies (also GitHub's clock), so a skewed laptop can't make a reply look
/// older than the trigger (#63). The repo comes from the PR's URL, not
/// {owner}/{repo}: gh's default repo may be a fork.
pub fn trigger(b: &str, remote: &str, force: bool) {
    let command = cfg("settings.botCommand", b);
    if command.is_empty() {
        die("no settings.botCommand configured: the comment that makes the review bot run");
    }
    let s = state(b);
    match trigger_gate(&s) {
        Gate::Hard(why) => die(why),
        Gate::Done(why) if !force => {
            println!("{why} (--force to trigger again)");
            return;
        }
        Gate::Soft(why) if !force => die(format!("not triggering the bot on {b}: {why} (--force to trigger anyway)")),
        _ => {}
    }
    let head = rev(".", &format!("refs/heads/{b}"));
    if head.is_empty() {
        die(format!("no such branch: {b}"));
    }
    let base = t(&s, "sync.old_base");
    if base.is_empty() {
        die(format!("can't find {b}'s base: {}", no_base_why(&s, remote)));
    }
    let (number, url) = (t(&s, "pr.number"), t(&s, "pr.url"));
    let Some((host, repo)) = pr_repo(&url) else { die(format!("can't tell PR #{number}'s repo from its URL: {url}")) };
    let path = format!("repos/{repo}/issues/{number}/comments");
    let body = format!("body={command}");
    let o = proc::run("gh", &["api", &path, "--hostname", &host, "-f", &body, "--jq", r#".created_at // """#], None, None);
    eprint!("{}", o.stderr);
    if !o.ok {
        die("posting the trigger comment failed; nothing pinned");
    }
    let mut when = strip_nl(o.stdout);
    if !is_time(&when) {
        when = time::now_utc().0;
        eprintln!("sd act: warning: GitHub's answer had no created_at; the trigger time is this machine's local clock");
    }
    git::pin_write(".", b, "bot", &head, &base).unwrap_or_else(|e| die(e));
    git_or_exit(&["config", &format!("branch.{b}.sd-bot-triggered"), &when]);
    println!("triggered the review bot on PR #{number} with \"{command}\"; bot pinned at {}, waiting for its reply", &head[..7]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn item(id: &str, ty: &str, kind: &str, mark: &str) -> Value {
        json!({"id": id, "type": ty, "kind": kind, "mark": mark, "author": "bot1", "last": "bot1",
               "url": format!("https://example/{id}"), "excerpt": format!("about {id}")})
    }

    fn owed(items: &[Value]) -> Value {
        json!({"threads": {"owed_bot": 1, "items": items}})
    }

    fn ids(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn ack_items_with_no_ids_are_the_info_items() {
        let s = owed(&[item("R1", "review", "bot", "i"), item("T1", "thread", "bot", "!"), item("C1", "comment", "bot", "i")]);
        let got = ack_items(&s, &[], "feat").unwrap();
        assert_eq!(got.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), ["R1", "C1"]);
        assert_eq!(got[0].1, "review  bot1  about R1  https://example/R1");
    }

    #[test]
    fn ack_items_by_id_in_sd_states_order_any_mark() {
        let s = owed(&[item("R1", "review", "bot", "i"), item("C1", "comment", "bot", "i"), item("R2", "review", "bot", "!")]);
        let got = ack_items(&s, &ids(&["R2", "C1"]), "feat").unwrap();
        assert_eq!(got.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), ["C1", "R2"]);
    }

    #[test]
    fn ack_items_refuse_threads_human_items_and_ids_not_owed() {
        let s = owed(&[item("T1", "thread", "bot", "!"), item("H1", "review", "human", "!"), item("R1", "review", "bot", "i")]);
        let got = ack_items(&s, &ids(&["NOPE", "R1", "H1", "T1"]), "feat");
        assert_eq!(
            got,
            Err(vec![
                "T1 is a thread; resolve it on GitHub".to_string(),
                "H1 is a human review; reply to it".to_string(),
                "NOPE isn't owed on feat".to_string(),
            ])
        );
    }

    #[test]
    fn ack_items_what_is_type_author_excerpt_and_url() {
        // A format test: the line began as the bash version's and may change
        // (#379).
        let s = owed(&[
            json!({"id": "T1", "type": "thread", "kind": "bot", "last": "copilot", "mark": "!", "author": "copilot",
                   "url": "https://example/pr/7#t1", "excerpt": "Heading"}),
            json!({"id": "R2", "type": "review", "kind": "bot", "last": "claude", "mark": "!", "author": "claude",
                   "url": "https://example/pr/7#R2", "excerpt": "Please fix"}),
            json!({"id": "R1", "type": "review", "kind": "bot", "last": "claude", "mark": "i", "author": "claude",
                   "url": "https://example/pr/7#R1", "excerpt": "Looks fine"}),
        ]);
        assert_eq!(ack_items(&s, &ids(&["R1", "T1"]), "feat"), Err(vec!["T1 is a thread; resolve it on GitHub".to_string()]));
        assert_eq!(ack_items(&s, &ids(&["R1"]), "feat"), Ok(vec![("R1".into(), "review  claude  Looks fine  https://example/pr/7#R1".into())]));
    }

    #[test]
    fn ack_items_author_else_last_and_null_as_nothing() {
        let s = owed(&[json!({"id": "R1", "type": "review", "kind": "bot", "mark": "i", "author": null, "last": "claude", "url": null, "excerpt": null})]);
        assert_eq!(ack_items(&s, &[], "feat"), Ok(vec![("R1".into(), "review  claude    ".into())]));
        assert_eq!(ack_items(&owed(&[]), &[], "feat"), Ok(vec![]));
    }

    fn st(v: Value) -> Value {
        let mut s = json!({
            "branch": "feat", "head": "a1b2c3d4e5f6",
            "pr": {"number": 7, "state": "OPEN"},
            "push": {"remote_sha": "a1b2c3d4e5f6", "ahead": 0, "behind": 0, "diverged": false, "remote_ours": true, "remote_made": true, "dirty": false},
            "review": {"local": {"status": "covered"}, "bot": {"status": "uncovered"}, "accepted": {"status": "none"}},
            "errors": []
        });
        merge(&mut s, v);
        s
    }

    fn merge(s: &mut Value, v: Value) {
        match (s, v) {
            (Value::Object(s), Value::Object(v)) => {
                for (k, x) in v {
                    merge(s.entry(k).or_insert(Value::Null), x);
                }
            }
            (s, v) => *s = v,
        }
    }

    const NOT_PUSHED: &str = "feat isn't on the remote at HEAD; the bot reviews what GitHub has: sd act push first";

    #[test]
    fn trigger_gate_goes_when_pushed_and_reviewed() {
        assert_eq!(trigger_gate(&st(json!({}))), Gate::Go);
        assert_eq!(trigger_gate(&st(json!({"review": {"local": {"status": "rewritten-same"}}}))), Gate::Go);
        let accepted = json!({"review": {"local": {"status": "uncovered"}, "accepted": {"status": "covered"}}});
        assert_eq!(trigger_gate(&st(accepted)), Gate::Go);
    }

    #[test]
    fn trigger_gate_no_pr_or_a_closed_one_is_hard() {
        assert_eq!(trigger_gate(&st(json!({"pr": null}))), Gate::Hard("no PR for feat".into()));
        let why = json!({"pr": null, "errors": ["gh: boom", "again"]});
        assert_eq!(trigger_gate(&st(why)), Gate::Hard("no PR for feat: gh: boom; again".into()));
        assert_eq!(trigger_gate(&st(json!({"pr": {"state": "MERGED"}}))), Gate::Hard("PR #7 is merged".into()));
        assert_eq!(trigger_gate(&st(json!({"pr": {"state": null}}))), Gate::Go);
    }

    #[test]
    fn trigger_gate_head_not_on_the_remote_is_hard() {
        assert_eq!(trigger_gate(&st(json!({"push": {"remote_sha": null}}))), Gate::Hard(NOT_PUSHED.into()));
        assert_eq!(trigger_gate(&st(json!({"push": {"ahead": 1}}))), Gate::Hard(NOT_PUSHED.into()));
        assert_eq!(trigger_gate(&st(json!({"push": {"diverged": true}}))), Gate::Hard(NOT_PUSHED.into()));
        // A pushed commit of ours dropped here: push drops it too.
        assert_eq!(trigger_gate(&st(json!({"push": {"behind": 1}}))), Gate::Hard(NOT_PUSHED.into()));
        let dirty = json!({"push": {"behind": 1, "dirty": true}});
        let want = "feat isn't on the remote at HEAD; the bot reviews what GitHub has: commit or remove the worktree's changes, then sd act push";
        assert_eq!(trigger_gate(&st(dirty)), Gate::Hard(want.into()));
        let theirs = json!({"push": {"behind": 1, "remote_made": false}});
        let want = "feat isn't on the remote at HEAD; the bot reviews what GitHub has: take its commits first (git merge --ff-only)";
        assert_eq!(trigger_gate(&st(theirs)), Gate::Hard(want.into()));
    }

    #[test]
    fn trigger_gate_bot_already_on_head_is_done_before_the_review_check() {
        let s = st(json!({"review": {"bot": {"status": "covered"}, "local": {"status": "none"}}}));
        assert_eq!(trigger_gate(&s), Gate::Done("the bot was already triggered on a1b2c3d".into()));
    }

    #[test]
    fn trigger_gate_no_review_covering_head_is_soft() {
        let want = Gate::Soft("neither the local review nor an accept covers HEAD (sd act review)".into());
        assert_eq!(trigger_gate(&st(json!({"review": {"local": {"status": "uncovered"}}}))), want);
        assert_eq!(trigger_gate(&st(json!({"review": null}))), want);
    }

    #[test]
    fn pr_repo_is_the_host_and_repo_of_a_pull_url() {
        assert_eq!(pr_repo("https://github.com/upstream/proj/pull/7"), Some(("github.com".into(), "upstream/proj".into())));
        assert_eq!(pr_repo("https://ghe.example/o/r/pull/12"), Some(("ghe.example".into(), "o/r".into())));
        assert_eq!(pr_repo("https://github.com/upstream/proj/issues/7"), None);
        assert_eq!(pr_repo("https://github.com/upstream/proj/pull/7/files"), None);
        assert_eq!(pr_repo(""), None);
    }

    #[test]
    fn is_time_wants_a_date_and_a_t() {
        assert!(is_time("2026-10-16T12:34:56Z"));
        assert!(!is_time(""));
        assert!(!is_time("null"));
        assert!(!is_time("2026-10-16 12:34:56"));
    }
}
