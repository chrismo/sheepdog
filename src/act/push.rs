// push (A3), and ship's and ci-fix's push.

use super::{die, enforce, gates, plural, state_json, Go};
use crate::git::{self, commits_not_ours, count, git, git_ok, git_out, made_here, remote_default, rev, strip_nl, worktree_for};
use crate::next::Derived;

pub fn push(b: &str, remote: &str, force: bool) {
    if !git::branch_exists(b) {
        die(format!("no such branch: {b}"));
    }
    let lock = git_out(".", &["config", "--get", &format!("branch.{b}.sd-lock")]).unwrap_or_default();
    if !force && !lock.is_empty() {
        die(format!("not pushing {b}: taken over (branch.{b}.sd-lock is set; --force to push anyway)"));
    }
    if b == remote_default(".", remote) {
        die(format!("not pushing {b}: it's the default branch; start a branch (sd act start <branch> \"<task>\")"));
    }
    let s = state_json(".", b).unwrap_or_else(|e| {
        eprint!("{e}");
        die("sd state failed")
    });
    let d = Derived::new(&s);
    if let Some(go) = enforce(gates::push(&d), force, &format!("pushing {b}")) {
        publish(go, b, remote);
        if d.foreign_authors && gates::rewrites(&d) {
            seen_authors(b, &s);
        }
    }
    // chrismo pushing is the approval of what an agent changed in CI config.
    let held = format!("branch.{b}.sd-held");
    if git_ok(".", &["config", "--get", &held]) {
        let o = git(".", &["config", "--unset", &held]);
        if !o.ok {
            eprint!("{}", o.stderr);
            std::process::exit(o.code);
        }
        println!("released the CI config hold on {b}");
    }
}

fn publish(_: Go, b: &str, remote: &str) {
    push_branch(b, remote).unwrap_or_else(|e| die(e));
}

/// Forced past commits someone else made: chrismo has looked at them, so
/// sd state stops counting them, restacked or not (sync.foreign_commits'
/// patch-ids, #356).
fn seen_authors(b: &str, s: &serde_json::Value) {
    let key = format!("branch.{b}.sd-authors-seen");
    for id in s.pointer("/sync/foreign_commits").and_then(|v| v.as_array()).into_iter().flatten().filter_map(|v| v.as_str()) {
        let o = git(".", &["config", "--add", &key, id]);
        if !o.ok {
            eprint!("{}", o.stderr);
            eprintln!("sd act: warning: couldn't record {b}'s commits as looked at; the next push asks again");
            return;
        }
    }
}

/// Publish <b> as fetched just now. Rewritten history, or a pushed commit
/// reset away, is force-pushed with a lease on that remote head, and only when
/// the head is in the branch's reflog (we put it there). Err is why not.
pub(super) fn push_branch(b: &str, remote: &str) -> Result<(), String> {
    let f = git(".", &["fetch", "--quiet", "--prune", remote]);
    eprint!("{}", f.stderr);
    if !f.ok {
        return Err(format!("fetch from {remote} failed"));
    }
    let rsha = rev(".", &format!("refs/remotes/{remote}/{b}"));
    if rsha.is_empty() {
        push_ref(b, remote, "", None)?;
        println!("pushed {b} to {remote} (new branch)");
        track(b, remote);
        return Ok(());
    }
    let ahead = count(".", &[&format!("{rsha}..refs/heads/{b}")]);
    let behind = count(".", &[&format!("refs/heads/{b}..{rsha}")]);
    match (ahead, behind) {
        (0, 0) => println!("nothing to push on {b}"),
        (_, 0) => {
            push_ref(b, remote, &rsha, None)?;
            println!("pushed {b} to {remote} ({})", plural(ahead as usize, "commit"));
        }
        _ => {
            over_ours(b, remote, &rsha, ahead, behind)?;
            push_ref(b, remote, &rsha, Some(&format!("--force-with-lease=refs/heads/{b}:{rsha}")))?;
            let short = &rsha[..7];
            if ahead == 0 {
                println!("force-pushed {b} to {remote} (dropped {}; the remote head {short} was ours)", plural(behind as usize, "commit"));
            } else {
                println!("force-pushed {b} to {remote} (rewritten; the remote head {short} was ours)");
            }
        }
    }
    track(b, remote);
    Ok(())
}

/// The remote has commits this branch doesn't. Ours (in the branch's reflog:
/// an amend, or a pushed commit reset away, #86) may be replaced with a lease;
/// anyone else's are never pushed over.
fn over_ours(b: &str, remote: &str, rsha: &str, ahead: i64, behind: i64) -> Result<(), String> {
    let behind_n = plural(behind as usize, "commit");
    let reflog = git_out(".", &["reflog", "show", "--format=%H", &format!("refs/heads/{b}")]).unwrap_or_default();
    if !reflog.lines().any(|l| l == rsha) {
        if ahead == 0 {
            return Err(format!("{remote}/{b} has {behind_n} you don't; take them with: git merge --ff-only {remote}/{b}"));
        }
        let theirs = commits_not_ours(".", &reflog, rsha).map_or("commits".into(), |n| plural(n as usize, "commit"));
        return Err(format!(
            "{remote}/{b} has {theirs} this copy has never had; not pushing over it. \
             Look: git log {b}..{remote}/{b}, then rebase onto or merge {remote}/{b} and push again"
        ));
    }
    if ahead > 0 {
        return Ok(());
    }
    // Someone else's commit taken (merge --ff-only, a pull, a rebase onto the
    // remote) and reset away is in the reflog too: every commit being dropped
    // must have been made here.
    if !made_here(".", &format!("refs/heads/{b}..{rsha}")) {
        return Err(format!(
            "{remote}/{b} has {behind_n} you don't; this branch had them but didn't make them all, \
             so take them back (git merge --ff-only {remote}/{b}), or drop them by hand"
        ));
    }
    // A soft or mixed reset looks the same as a deliberate drop, except the
    // changes are still in the worktree: don't throw the remote commit away
    // under them.
    let wt = worktree_for(".", b);
    if wt.is_empty() {
        return Ok(());
    }
    let st = git(&wt, &["status", "--porcelain"]);
    if !st.ok {
        return Err(format!("can't read {wt}'s status; not dropping the remote commit"));
    }
    if !st.stdout.is_empty() {
        return Err(format!(
            "{remote}/{b} has {behind_n} this branch was reset off, and {wt} has uncommitted or untracked changes: \
             commit them (then push replaces the remote commit), or drop them (git reset --hard; git clean -n shows the untracked ones)"
        ));
    }
    Ok(())
}

fn track(b: &str, remote: &str) {
    let set = |key: String, val: &str| {
        let o = git(".", &["config", &key, val]);
        eprint!("{}", o.stderr);
        o.ok
    };
    if !(set(format!("branch.{b}.remote"), remote) && set(format!("branch.{b}.merge"), &format!("refs/heads/{b}"))) {
        eprintln!("sd act: warning: {b} is pushed, but its upstream isn't {remote}/{b}: git branch --set-upstream-to={remote}/{b} {b}");
    }
}

/// Push refs/heads/<b> to <remote>, where its head was <expected> (empty: not
/// there yet).
fn push_ref(b: &str, remote: &str, expected: &str, lease: Option<&str>) -> Result<(), String> {
    let refspec = format!("refs/heads/{b}:refs/heads/{b}");
    let mut args = vec!["push", "--quiet"];
    args.extend(lease);
    args.extend([remote, refspec.as_str()]);
    let o = git(".", &args);
    if o.ok {
        return Ok(());
    }
    let now = git_out(".", &["ls-remote", remote, &format!("refs/heads/{b}")]).unwrap_or_default();
    let now = now.split('\t').next().unwrap_or_default();
    Err(push_failed(b, remote, expected, now, &(o.stdout + &o.stderr)))
}

fn push_failed(b: &str, remote: &str, expected: &str, now: &str, out: &str) -> String {
    if now != expected {
        return format!("{remote}/{b} moved while pushing; nothing pushed. Run it again to see what changed");
    }
    format!("push to {remote} failed: {}", strip_nl(out.to_string()).replace('\n', " "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_failed_is_a_move_when_the_remote_head_changed() {
        let moved = "origin/feat moved while pushing; nothing pushed. Run it again to see what changed";
        assert_eq!(push_failed("feat", "origin", "abc", "def", "rejected\n"), moved);
        assert_eq!(push_failed("feat", "origin", "", "def", "rejected\n"), moved);
    }

    #[test]
    fn push_failed_otherwise_is_gits_output_on_one_line() {
        let out = "remote: denied\nerror: failed to push some refs\n\n";
        let want = "push to up failed: remote: denied error: failed to push some refs";
        assert_eq!(push_failed("feat", "up", "abc", "abc", out), want);
        assert_eq!(push_failed("feat", "up", "", "", out), want);
    }
}
