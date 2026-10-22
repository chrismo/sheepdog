// ship: describe unless a description for HEAD and the target is stored,
// check HEAD didn't move, push, then open a draft PR on the parent and
// enroll; with a PR already open, fix up its title, body and base instead.

use super::bookkeeping::{enroll, record_lineage};
use super::commands::status;
use super::describe::{self, describe_run, stored};
use super::hold::hold_gate;
use super::push::push_branch;
use super::restack::{parent_to_default, state};
use super::review::short;
use super::{die, no_base_why, Args};
use crate::git::{git, git_out, rev, show_ref, worktree_for};
use crate::proc;
use crate::val::{at, on, sj, t, text};
use serde_json::Value;

#[derive(Debug, PartialEq)]
enum Parent {
    Open,
    /// sd state couldn't look its PR up.
    Unknown,
    Closed,
    /// Merged, the branch not yet on the target: restack first.
    Merged,
    /// Merged by fast-forward or rebase, the branch already on the target.
    MergedOnTarget,
}

/// Like restack's gate: a parent whose state is unknown or closed is
/// nobody's to ship onto; a merged one means restack first, unless the
/// branch is already on the target, in which case the parent is the default
/// branch from here on; an open parent must be on the remote before its
/// child can be: ship the parent first.
fn parent_state(s: &Value) -> Parent {
    if at(s, "parent.merged").is_null() {
        Parent::Unknown
    } else if at(s, "parent.pr.state").as_str() == Some("CLOSED") {
        Parent::Closed
    } else if on(s, "parent.merged") && on(s, "sync.up_to_date") {
        Parent::MergedOnTarget
    } else if on(s, "parent.merged") {
        Parent::Merged
    } else {
        Parent::Open
    }
}

/// The local review covers HEAD, so the PR needn't stay a draft for it.
fn reviewed(s: &Value) -> bool {
    matches!(at(s, "review.local.status").as_str(), Some("covered" | "rewritten-same"))
}

/// The PR's title and body from the description, trailing newlines dropped
/// as bash's $( ) dropped them.
fn pr_text(desc: &Value) -> (String, String) {
    let trim = |s: String| s.trim_end_matches('\n').to_string();
    (trim(t(desc, "title")), trim(text(at(desc, "body"))))
}

fn branch_parent(b: &str) -> String {
    git_out(".", &["config", "--get", &format!("branch.{b}.sd-parent")]).unwrap_or_default()
}

pub fn ship(b: &str, remote: &str, a: &Args) {
    let wt = worktree_for(".", b);
    if wt.is_empty() {
        die(format!("{b} isn't checked out in any worktree"));
    }
    if !status(&wt).is_empty() {
        die(format!("{wt} has uncommitted changes"));
    }
    let lock = git_out(".", &["config", "--get", &format!("branch.{b}.sd-lock")]).unwrap_or_default();
    if !a.force && !lock.is_empty() {
        die(format!("not shipping {b}: taken over (branch.{b}.sd-lock is set; --force to ship anyway)"));
    }
    if let Err(e) = std::env::set_current_dir(&wt) {
        die(format!("can't work in {wt}: {e}"));
    }
    let f = git(".", &["fetch", "--quiet", "--prune", remote]);
    eprint!("{}", f.stderr);
    if !f.ok {
        die(format!("fetch from {remote} failed"));
    }
    hold_gate(b, &format!("shipping {b}"), "");
    let mut s = state(&wt, b);
    if b == t(&s, "sync.default") {
        die(format!("not shipping {b}: it's the default branch; start a branch (sd act start <branch> \"<task>\")"));
    }
    let pr_state = at(&s, "pr.state").as_str().unwrap_or("none").to_string();
    if !matches!(pr_state.as_str(), "none" | "OPEN") {
        die(format!("PR #{} is {}; nothing to ship", t(&s, "pr.number"), pr_state.to_lowercase()));
    }
    if branch_parent(b).is_empty() && pr_state == "none" {
        // A plain git branch from another's tip (#94) would open on the default branch.
        if let Err(e) = record_lineage(b, &s, remote) {
            eprintln!("sd act: {e}");
            die(format!("not shipping {b} until its parent is recorded"));
        }
        if !branch_parent(b).is_empty() {
            s = state(&wt, b);
        }
    }
    // sd state's parent: sd-parent, else the PR's base, else the default branch.
    let mut parent = t(&s, "parent.name");
    if parent != t(&s, "sync.default") {
        match parent_state(&s) {
            Parent::Open => {
                if !show_ref(".", &format!("refs/remotes/{remote}/{parent}")) {
                    die(format!("can't ship {b} onto {parent}: {}", no_base_why(&s, remote)));
                }
            }
            Parent::Unknown => die(format!("can't tell whether {parent} merged: {}", sj(at(&s, "errors"), "; "))),
            Parent::Closed => die(format!(
                "parent {parent} (#{}) was closed without merging: decide what {b} targets (git config branch.{b}.sd-parent <branch>), then ship",
                t(&s, "parent.pr.number")
            )),
            Parent::Merged => die(format!(
                "parent {parent} has merged: sd act restack first, which lands {b} on {} and rewrites sd-parent",
                t(&s, "sync.target")
            )),
            Parent::MergedOnTarget => {
                parent_to_default(b, &s);
                parent = t(&s, "sync.default");
            }
        }
    }
    if !reviewed(&s) {
        eprintln!("sd act: warning: the local review doesn't cover HEAD (sd act review); the PR stays a draft until it does");
    }
    let head = rev(".", &format!("refs/heads/{b}"));
    let target = t(&s, "sync.target");
    let kept = describe::file(b).and_then(|f| std::fs::read_to_string(f).ok());
    let desc = match kept.and_then(|text| stored(&text, &head, &target)) {
        Some(d) => d,
        None => {
            let d = describe_run(b, &s, remote, a);
            let now = rev(".", &format!("refs/heads/{b}"));
            if now != head {
                let now = git_out(".", &["rev-parse", "--short", &now]).unwrap_or(now);
                die(format!(
                    "HEAD moved during the describe step ({} → {now}); nothing pushed. Look, then ship again",
                    short(&head)
                ));
            }
            d
        }
    };
    let (title, body) = pr_text(&desc);
    push_branch(b, remote).unwrap_or_else(|e| die(e));
    // GH_REPO, which act::run set, points gh at settings.remote's repo.
    if pr_state == "none" {
        let o = proc::run("gh", &["pr", "create", "--draft", "--base", &parent, "--head", b, "--title", &title, "--body", &body], None, None);
        eprint!("{}", o.stderr);
        if !o.ok {
            die(format!("gh pr create failed; {b} is pushed. Fix the cause and ship again"));
        }
        println!("opened a draft PR for {b} on {parent}: {}", o.stdout.trim_end_matches('\n'));
        enroll(b, remote);
        return;
    }
    let number = t(&s, "pr.number");
    let mut args = vec!["pr", "edit", &number, "--title", &title, "--body", &body];
    let retarget = t(&s, "pr.base") != parent;
    if retarget {
        args.extend(["--base", &parent]);
    }
    let o = proc::run("gh", &args, None, None);
    eprint!("{}", o.stderr);
    if !o.ok {
        die(format!("gh pr edit failed for PR #{number}; {b} is pushed"));
    }
    let base_now = if retarget { format!(", base now {parent}") } else { String::new() };
    println!("shipped {b}: PR #{number} updated{base_now}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn with(merged: Value, pr_state: &str, up_to_date: bool) -> Value {
        json!({"parent": {"name": "A", "merged": merged, "pr": {"state": pr_state}}, "sync": {"up_to_date": up_to_date}})
    }

    #[test]
    fn parent_state_open_merged_or_merged_on_the_target() {
        assert_eq!(parent_state(&with(json!(false), "OPEN", false)), Parent::Open);
        assert_eq!(parent_state(&with(json!(true), "MERGED", false)), Parent::Merged);
        assert_eq!(parent_state(&with(json!(true), "MERGED", true)), Parent::MergedOnTarget);
    }

    #[test]
    fn parent_state_unknown_when_sd_state_couldnt_say_whether_it_merged() {
        assert_eq!(parent_state(&with(Value::Null, "CLOSED", false)), Parent::Unknown);
        assert_eq!(parent_state(&json!({"parent": {"name": "A"}})), Parent::Unknown);
    }

    #[test]
    fn parent_state_closed_before_anything_else_it_knows() {
        assert_eq!(parent_state(&with(json!(false), "CLOSED", false)), Parent::Closed);
        assert_eq!(parent_state(&with(json!(true), "CLOSED", true)), Parent::Closed);
    }

    #[test]
    fn pr_text_drops_trailing_newlines_only() {
        let desc = json!({"title": "Add c\n", "body": "Adds c.\n\nRefs #1\n\n"});
        assert_eq!(pr_text(&desc), ("Add c".to_string(), "Adds c.\n\nRefs #1".to_string()));
        assert_eq!(pr_text(&json!({"title": "Add c", "body": null})), ("Add c".to_string(), String::new()));
    }

    #[test]
    fn reviewed_only_when_the_local_review_covers_head() {
        for (status, want) in [("covered", true), ("rewritten-same", true), ("stale", false), ("none", false)] {
            assert_eq!(reviewed(&json!({"review": {"local": {"status": status}}})), want, "{status}");
        }
        assert!(!reviewed(&json!({})));
    }
}
