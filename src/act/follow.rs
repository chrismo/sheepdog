// follow (#250): a teammate's PR, to review, never to author. Its head ref
// (refs/pull/<n>/head, which GitHub keeps for a fork's PR too) fetched to
// refs/sheepdog/review/<n>/pr-head; the branch review/<n> there, in a
// worktree beside the main one with start's setup hook run; the PR number and
// reviewer mode recorded; enrolled. stdout is only the worktree's path.
//
// fetch (A1): the author's pushes, force-pushes too, taken as they come: the
// branch reset to the PR's head, unless the worktree has changes or the
// branch has commits no fetched head had, which are chrismo's.

use super::bookkeeping::enroll_mark;
use super::commands::status;
use super::die;
use super::review::short;
use super::start::{destination, git_or_die, start_hook};
use crate::git::{git, git_out, is_ancestor, main_worktree, rev, show_ref, worktree_for};
use std::path::Path;

pub const REVIEWER: &str = "reviewer";

/// Refused on a reviewer's branch.
pub const AUTHORS: &[&str] =
    &["implement", "address", "ci-fix", "restack", "retarget", "push", "ship", "describe", "trigger", "answer", "autofix"];

pub fn reviewing(b: &str) -> bool {
    git_out(".", &["config", "--get", &format!("branch.{b}.sd-mode")]).as_deref() == Some(REVIEWER)
}

pub fn branch(n: &str) -> String {
    format!("review/{n}")
}

fn pr_head_ref(b: &str) -> String {
    format!("refs/sheepdog/{b}/pr-head")
}

/// PR <n>'s head into <b>'s pr-head ref, and <remote>'s branches, for the
/// base; the head's SHA.
fn fetch_pr(b: &str, n: &str, remote: &str) -> String {
    let pr_head = pr_head_ref(b);
    let pull = format!("refs/pull/{n}/head");
    let ls = git(".", &["ls-remote", remote, &pull]);
    if ls.ok && ls.stdout.trim().is_empty() {
        die(format!("no PR #{n} on {remote}"));
    }
    let f = git(".", &["fetch", "--quiet", remote, &format!("+{pull}:{pr_head}")]);
    if !f.ok {
        eprint!("{}", f.stderr);
        die(format!("fetch from {remote} failed"));
    }
    let f = git(".", &["fetch", "--quiet", "--prune", remote]);
    eprint!("{}", f.stderr);
    if !f.ok {
        die(format!("fetch from {remote} failed"));
    }
    rev(".", &pr_head)
}

pub fn follow(b: &str, n: &str, remote: &str) -> i32 {
    let src = git_out(".", &["rev-parse", "--show-toplevel"]).unwrap_or_else(|| die("not inside a worktree"));
    let dst = destination(&main_worktree("."), b);
    if Path::new(&dst).exists() {
        die(format!("{dst} already exists"));
    }
    if show_ref(".", &format!("refs/heads/{b}")) {
        die(format!("{b} already exists"));
    }
    let pr_head = fetch_pr(b, n, remote);
    // Before the worktree, so a failure leaves nothing a rerun trips on.
    for (k, v) in [("sd-pr", n), ("sd-mode", REVIEWER)] {
        let o = git(".", &["config", &format!("branch.{b}.{k}"), v]);
        if !o.ok {
            eprint!("{}", o.stderr);
            std::process::exit(o.code);
        }
    }
    git_or_die(&["worktree", "add", "-b", b, &dst, &pr_head], "git worktree add");
    enroll_mark(b);
    eprintln!("enrolled {b}");
    let rc = if start_hook(&src, &dst, b) { 0 } else { 1 };
    println!("{dst}");
    rc
}

pub fn fetch(b: &str, remote: &str) {
    if !reviewing(b) {
        die(format!("{b} isn't a PR you follow (sd act follow <number>)"));
    }
    let n = git_out(".", &["config", "--get", &format!("branch.{b}.sd-pr")]).unwrap_or_default();
    if n.is_empty() {
        die(format!("{b} has no PR number (branch.{b}.sd-pr)"));
    }
    let wt = worktree_for(".", b);
    if !wt.is_empty() && !status(&wt).is_empty() {
        die(format!("{wt} has uncommitted changes"));
    }
    let head = rev(".", &format!("refs/heads/{b}"));
    let last = rev(".", &pr_head_ref(b));
    if head != last && (last.is_empty() || !is_ancestor(".", &head, &last)) {
        die(format!("{b} has commits the PR never had: they're yours; move them to a branch of your own, then git reset --hard {}", pr_head_ref(b)));
    }
    let theirs = fetch_pr(b, &n, remote);
    if theirs != head {
        let o = if wt.is_empty() {
            git(".", &["update-ref", "-m", "sd act fetch", &format!("refs/heads/{b}"), &theirs, &head])
        } else {
            git(&wt, &["reset", "--quiet", "--keep", &theirs])
        };
        if !o.ok {
            eprint!("{}", o.stderr);
            die(format!("couldn't move {b} to the PR's head"));
        }
    }
    println!("{b} at the PR's head, {}", short(&theirs));
}
