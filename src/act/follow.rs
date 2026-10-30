// follow (#250): a teammate's PR, to review, never to author. Its head ref
// (refs/pull/<n>/head, which GitHub keeps for a fork's PR too) fetched; the
// branch review/<n> there, in a worktree beside the main one with start's
// setup hook run; the PR number, reviewer mode and the head as pr-head
// recorded; enrolled. stdout is only the worktree's path.
//
// A checkout already there is used as it is (#358), as start does (#252): no
// fetch into it, no reset, no setup hook. Which one, first match:
// - the branch already following PR <n>, whatever it's called (no fetch at
//   all, so a repeat follow just prints its path);
// - the branch checked out where follow runs, when its upstream, as its
//   remote has it now, is the PR's head: gh pr checkout tracks
//   refs/pull/<n>/head, the PR's branch, or a fork's branch by URL. Not
//   mere shared history, which a stacked PR's base has too. Never the
//   default branch;
// - review/<n>, when it has a commit of the PR's past the default branch
//   (with no default branch to tell by, only one at the PR's head).
// A branch's worktree is made, with the hook, when it has none. pr-head is
// where branch and PR last met (their merge-base), so fetch keeps any
// commits of the branch's own. Refused: a branch that is an
// author's or another PR's, two branches following the PR, a review/<n> that
// isn't the PR's, a registered worktree whose directory is gone.
//
// fetch (A1): the author's pushes, force-pushes too, taken as they come: the
// branch reset to the PR's head, unless the worktree has changes or the
// branch has commits no fetched head had, which are chrismo's.

use super::bookkeeping::enroll_mark;
use super::commands::status;
use super::die;
use super::review::short;
use super::start::{destination, git_on_stderr, start_default, start_hook, start_point};
use crate::git::{git, git_out, is_ancestor, main_worktree, pr_head_ref, rev, show_ref, worktree_for};
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

/// PR <n>'s head into <dest>, and <remote>'s branches, for the base; the
/// head's SHA.
fn fetch_pr(dest: &str, n: &str, remote: &str) -> String {
    let pull = format!("refs/pull/{n}/head");
    let ls = git(".", &["ls-remote", remote, &pull]);
    if ls.ok && ls.stdout.trim().is_empty() {
        die(format!("no PR #{n} on {remote}"));
    }
    let f = git(".", &["fetch", "--quiet", remote, &format!("+{pull}:{dest}")]);
    if !f.ok {
        eprint!("{}", f.stderr);
        die(format!("fetch from {remote} failed"));
    }
    let f = git(".", &["fetch", "--quiet", remote]);
    eprint!("{}", f.stderr);
    if !f.ok {
        die(format!("fetch from {remote} failed"));
    }
    rev(".", dest)
}

/// The branch following PR <n>; dies when more than one does.
fn following(n: &str) -> Option<String> {
    let all = git_out(".", &["config", "--get-regexp", r"^branch\..*\.sd-pr$"]).unwrap_or_default();
    let found: Vec<&str> = all
        .lines()
        .filter_map(|l| l.split_once(' ').filter(|(_, v)| *v == n))
        .filter_map(|(k, _)| k.strip_prefix("branch.")?.strip_suffix(".sd-pr"))
        .filter(|b| reviewing(b) && show_ref(".", &format!("refs/heads/{b}")))
        .collect();
    if found.len() > 1 {
        die(format!("PR #{n} is followed on more than one branch: {}", found.join(" ")));
    }
    found.first().map(|b| b.to_string())
}

/// Where <b> and the PR's head <pr_head> last met.
fn met(b: &str, pr_head: &str) -> Option<String> {
    git_out(".", &["merge-base", &format!("refs/heads/{b}"), pr_head])
}

/// Where <b> and the PR's head last met, when that's past the default
/// branch <main>: <b> has a commit of the PR's.
fn past(b: &str, pr_head: &str, main: &str) -> Option<String> {
    met(b, pr_head).filter(|mb| !is_ancestor(".", mb, main))
}

/// <b>'s upstream, asked of its remote, is at <pr_head>.
fn tracks(b: &str, pr_head: &str) -> bool {
    let get = |k: &str| git_out(".", &["config", "--get", &format!("branch.{b}.{k}")]);
    let (Some(remote), Some(merge)) = (get("remote"), get("merge")) else {
        return false;
    };
    let ls = git_out(".", &["ls-remote", &remote, &merge]).unwrap_or_default();
    ls.lines().any(|l| l.split_once('\t') == Some((pr_head, merge.as_str())))
}

/// Dies when <b> is another PR's or an author's.
fn unclaimed(b: &str, n: &str) {
    let get = |k: &str| git_out(".", &["config", "--get", &format!("branch.{b}.{k}")]).filter(|v| !v.is_empty());
    if let Some(m) = get("sd-pr").filter(|m| m != n) {
        if reviewing(b) {
            die(format!("{b} already follows PR #{m}"));
        }
        die(format!("{b} is PR #{m}'s"));
    }
    if !reviewing(b) && (get("sheepdog").as_deref() == Some("true") || get("sd-task").is_some()) {
        die(format!("{b} is an author's branch (sd act start or enroll): sd act follow won't make it a reviewer's"));
    }
}

/// <b> following PR <n> in its worktree, made beside the main one if it has
/// none (<b> made too, at <from>, if given) and the setup hook run there;
/// pr-head recorded at <pr_head>, if given. Prints the worktree's path; the
/// exit code.
fn settle(src: &str, b: &str, n: &str, pr_head: Option<&str>, from: Option<&str>) -> i32 {
    let wt = worktree_for(".", b);
    if !wt.is_empty() && !Path::new(&wt).is_dir() {
        die(format!("{b} is checked out at {wt}, which is gone: git worktree prune, then follow again"));
    }
    let made = wt.is_empty();
    let dst = if made { destination(&main_worktree("."), b) } else { wt };
    if made {
        if Path::new(&dst).exists() {
            die(format!("{dst} already exists"));
        }
        let add = match from {
            Some(f) => ["worktree", "add", "-b", b, &dst, f].to_vec(),
            None => ["worktree", "add", &dst, b].to_vec(),
        };
        if !git_on_stderr(&add) {
            die("git worktree add failed");
        }
    } else {
        eprintln!("sd act: {b} is already checked out at {dst}; using it");
    }
    for (k, v) in [("sd-pr", n), ("sd-mode", REVIEWER)] {
        let o = git(".", &["config", &format!("branch.{b}.{k}"), v]);
        if !o.ok {
            eprint!("{}", o.stderr);
            std::process::exit(o.code);
        }
    }
    if let Some(h) = pr_head {
        let o = git(".", &["update-ref", &pr_head_ref(b), h]);
        if !o.ok {
            eprint!("{}", o.stderr);
            die(format!("couldn't record the PR's head for {b}"));
        }
    }
    enroll_mark(b);
    eprintln!("enrolled {b}");
    let rc = if made && !start_hook(src, &dst, b) { 1 } else { 0 };
    println!("{dst}");
    rc
}

pub fn follow(b: &str, n: &str, remote: &str) -> i32 {
    let src = git_out(".", &["rev-parse", "--show-toplevel"]).unwrap_or_else(|| die("not inside a worktree"));
    if let Some(f) = following(n) {
        return settle(&src, &f, n, None, None);
    }
    let fetched = format!("refs/sheepdog/{b}/pr-fetched");
    let pr_head = fetch_pr(&fetched, n, remote);
    git(".", &["update-ref", "-d", &fetched]);
    let default = start_default(remote);
    let main = default.as_deref().and_then(|d| start_point(remote, d));
    let here = git_out(".", &["branch", "--show-current"]).unwrap_or_default();
    if !here.is_empty() && Some(&here) != default.as_ref() && tracks(&here, &pr_head) {
        unclaimed(&here, n);
        let mb = met(&here, &pr_head);
        if mb.is_none() {
            git(".", &["update-ref", "-d", &pr_head_ref(&here)]);
        }
        return settle(&src, &here, n, mb.as_deref(), None);
    }
    if here != b && main.as_deref().and_then(|m| past(&here, &pr_head, m)).is_some() {
        eprintln!("sd act: not taking {here} for PR #{n}: its upstream, if any, isn't at the PR's head as its remote has it");
    }
    if !show_ref(".", &format!("refs/heads/{b}")) {
        return settle(&src, b, n, Some(&pr_head), Some(&pr_head));
    }
    unclaimed(b, n);
    if rev(".", &format!("refs/heads/{b}")) == pr_head {
        return settle(&src, b, n, Some(&pr_head), None);
    }
    let main = main.unwrap_or_else(|| die(format!("{b} already exists, and with no default branch to tell by, follow can't say it's PR #{n}'s")));
    let mb = past(b, &pr_head, &main).unwrap_or_else(|| die(format!("{b} already exists and isn't PR #{n}'s")));
    settle(&src, b, n, Some(&mb), None)
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
    // pr-head moves only once the branch has: it's what this check trusts.
    // With none recorded, the head just fetched stands in.
    let fetched = format!("refs/sheepdog/{b}/pr-fetched");
    let theirs = fetch_pr(&fetched, &n, remote);
    let known = if last.is_empty() { &theirs } else { &last };
    if head != *known && !is_ancestor(".", &head, known) {
        git(".", &["update-ref", "-d", &fetched]);
        die(format!("{b} has commits the PR never had: they're yours; move them to a branch of your own, then git reset --hard {} && sd act fetch", pr_head_ref(b)));
    }
    let moved = if theirs == head {
        None
    } else if wt.is_empty() {
        Some(git(".", &["update-ref", "-m", "sd act fetch", &format!("refs/heads/{b}"), &theirs, &head]))
    } else {
        Some(git(&wt, &["reset", "--quiet", "--keep", &theirs]))
    };
    git(".", &["update-ref", "-d", &fetched]);
    if let Some(o) = moved.filter(|o| !o.ok) {
        eprint!("{}", o.stderr);
        die(format!("couldn't move {b} to the PR's head"));
    }
    let o = git(".", &["update-ref", &pr_head_ref(b), &theirs]);
    if !o.ok {
        eprint!("{}", o.stderr);
        die(format!("couldn't record the PR's head for {b}"));
    }
    println!("{b} at the PR's head, {}", short(&theirs));
}
