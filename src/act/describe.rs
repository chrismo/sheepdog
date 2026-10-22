// describe (J6): the agent writes the PR title and body from the task, the
// commits and the diff, read-only, in the branch's session. Stored for HEAD
// and the PR's target, where ship finds it.

use super::commands::status;
use super::implement::blank;
use super::restack::state;
use super::review::short;
use super::step::{step_session, step_store};
use super::{attached_check, die, no_base_why, Args};
use crate::git::{self, git_out, rev, worktree_for};
use crate::guidance::{lib_dir, step_guidance};
use crate::harness::{self, Step, READONLY_ALLOW, READONLY_PI_TOOLS};
use crate::json;
use crate::val::{at, on, t};
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

// The describe step's prompt: guidance (settings.describePrompt, else
// .sheepdog/describe.md in the base commit, else lib/prompts/describe.md),
// then this contract.
const DESCRIBE_CONTRACT: &str = "You are read-only here; look at the worktree when the diff needs context.";

const DESCRIBE_SCHEMA: &str = r#"{"type": "object", "required": ["title", "body"],
 "properties": {"title": {"type": "string", "minLength": 1}, "body": {"type": "string"}}}"#;

/// Where the branch's description is stored, a / in its name made __; None
/// outside a repo.
pub fn file(b: &str) -> Option<String> {
    git::common_dir(".").map(|dir| format!("{dir}/sheepdog/{}.describe.json", b.replace('/', "__")))
}

/// The prompt's line on what the PR targets (sync.target, the base ship
/// sets), so the body doesn't describe a merged parent as pending (#105).
fn base_line(s: &Value) -> String {
    let target = t(s, "sync.target");
    if on(s, "parent.merged") {
        let parent = t(s, "parent.name");
        format!("Base: {parent} has merged into {target}, so this PR targets {target}; don't describe the parent as pending.")
    } else if target != t(s, "sync.default") {
        format!("Base: this PR is stacked on {target}; describe only this branch's change.")
    } else {
        format!("Base: this PR targets {target}.")
    }
}

/// The description to store: the agent's title and body, for <head> and the
/// PR's <target>. None when the result has no title.
fn record(head: &str, target: &str, result: &Value) -> Option<Value> {
    let title = at(result, "title").as_str().filter(|title| !blank(title))?;
    let body = match at(result, "body") {
        Value::Null => json!(""),
        body => body.clone(),
    };
    Some(json!({"head": head, "target": target, "title": title, "body": body}))
}

/// The stored description <text>, when it is for <head> and <target>. A new
/// target (a parent merged under an unchanged HEAD) makes it stale, and so
/// does none: one stored before #105 is written again once.
pub(super) fn stored(text: &str, head: &str, target: &str) -> Option<Value> {
    let rec = crate::config::one_value(text)?;
    let target_is = |v: &Value| v.as_str() == Some(target);
    (at(&rec, "head").as_str() == Some(head) && target_is(at(&rec, "target"))).then_some(rec)
}

/// The describe step in the branch's session (a new one if none), read-only,
/// from the worktree it runs in. Stores the description where ship finds it,
/// and returns it.
pub(super) fn describe_run(b: &str, s: &Value, remote: &str, a: &Args) -> Value {
    let base = t(s, "sync.old_base");
    if base.is_empty() {
        die(format!("can't find {b}'s base: {}", no_base_why(s, remote)));
    }
    let head = rev(".", &format!("refs/heads/{b}"));
    let ss = step_session(b, "describe", &a.harness, a.force, false);
    let task = git_out(".", &["config", "--get", &format!("branch.{b}.sd-task")]).unwrap_or_default();
    let task = if task.is_empty() { "(none recorded)" } else { &task };
    let commits = git_out(".", &["log", "--format=- %s", &format!("{base}..{head}")]).unwrap_or_default();
    let diff = git_out(".", &["diff", &base, &head]).unwrap_or_default();
    let guidance = step_guidance("describePrompt", b, &base, &lib_dir().join("prompts/describe.md")).unwrap_or_else(|e| {
        eprintln!("{e}");
        die("can't read the describe guidance")
    });
    let prompt = format!(
        "{guidance}\n\n{DESCRIBE_CONTRACT}\n\nTask: {task}\n\n{}\n\nCommits ({}..{}):\n{commits}\n\nDiff:\n{diff}",
        base_line(s),
        short(&base),
        short(&head)
    );

    let (h, sid) = (ss.harness.as_str(), ss.id.as_str());
    eprintln!("sd act: {h} ({} session {sid}) is describing {b}…", ss.mode);
    let step = Step {
        harness: h,
        session: sid,
        mode: ss.mode,
        timeout: ss.timeout,
        prompt: &prompt,
        schema: DESCRIBE_SCHEMA,
        settings: &ss.cfg,
        name: "describe",
        branch: b,
        claude_allow: READONLY_ALLOW,
        pi_tools: READONLY_PI_TOOLS,
    };
    let Some(result) = harness::run_step(&step).ok().and_then(|rs| rs.into_iter().last()) else {
        die(format!("the {h} describe step failed or returned no usable result (session {sid})"))
    };
    let rec = record(&head, &t(s, "sync.target"), &result).unwrap_or_else(|| die(format!("the {h} describe step returned no title")));
    step_store(b, &ss);
    let f = file(b).unwrap_or_else(|| die("can't find the repo's git directory"));
    let dir = Path::new(&f).parent().unwrap_or(Path::new("."));
    if let Err(e) = fs::create_dir_all(dir).and_then(|_| fs::write(&f, json::to_string(&rec) + "\n")) {
        die(format!("can't write {f}: {e}"));
    }
    rec
}

pub fn describe(b: &str, remote: &str, a: &Args) {
    let wt = worktree_for(".", b);
    if wt.is_empty() {
        die(format!("{b} isn't checked out in any worktree"));
    }
    if !status(&wt).is_empty() {
        die(format!("{wt} has uncommitted changes"));
    }
    attached_check(b, &format!("describing {b}"), a.force);
    if let Err(e) = std::env::set_current_dir(&wt) {
        die(format!("can't work in {wt}: {e}"));
    }
    println!("{}", json::to_string(&describe_run(b, &state(&wt, b), remote, a)));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(target: &str, default: &str, merged: Value) -> Value {
        json!({"parent": {"name": "A", "merged": merged}, "sync": {"target": target, "default": default}})
    }

    #[test]
    fn base_line_says_what_the_pr_targets() {
        assert_eq!(base_line(&state("main", "main", json!(false))), "Base: this PR targets main.");
        assert_eq!(base_line(&state("main", "main", Value::Null)), "Base: this PR targets main.");
        assert_eq!(
            base_line(&state("A", "main", json!(false))),
            "Base: this PR is stacked on A; describe only this branch's change."
        );
    }

    #[test]
    fn base_line_names_a_merged_parent_so_the_body_doesnt_call_it_pending() {
        assert_eq!(
            base_line(&state("main", "main", json!(true))),
            "Base: A has merged into main, so this PR targets main; don't describe the parent as pending."
        );
    }

    #[test]
    fn record_is_head_target_title_and_body_in_order_one_line() {
        let r = record("h1", "main", &json!({"body": "Adds c.\n\nRefs #1", "title": "Add c"})).unwrap();
        assert_eq!(json::to_string(&r), r#"{"head":"h1","target":"main","title":"Add c","body":"Adds c.\n\nRefs #1"}"#);
        let r = record("h1", "A", &json!({"title": "Add c"})).unwrap();
        assert_eq!(json::to_string(&r), r#"{"head":"h1","target":"A","title":"Add c","body":""}"#);
    }

    #[test]
    fn record_needs_a_title_with_something_in_it() {
        for result in [json!({"body": "b"}), json!({"title": null}), json!({"title": 7}), json!({"title": " \t\n"})] {
            assert_eq!(record("h1", "main", &result), None, "{result}");
        }
    }

    #[test]
    fn stored_only_for_the_same_head_and_target() {
        let text = "{\"head\":\"h1\",\"target\":\"A\",\"title\":\"Add c\",\"body\":\"\"}\n";
        assert_eq!(t(&stored(text, "h1", "A").unwrap(), "title"), "Add c");
        assert!(stored(text, "h2", "A").is_none());
        assert!(stored(text, "h1", "main").is_none());
    }

    #[test]
    fn stored_none_from_before_105_with_no_target_or_from_a_file_that_isnt_one() {
        assert!(stored(r#"{"head":"h1","title":"Add c","body":""}"#, "h1", "main").is_none());
        for text in ["", "{", "{} {}", "[]"] {
            assert!(stored(text, "h1", "main").is_none(), "{text:?}");
        }
    }
}
