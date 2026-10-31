// start (A0): a worktree beside the main one on the branch, the task, parent
// and base recorded, the branch enrolled; then implement. stdout is only the
// worktree's path. --profile picks the branch's config profiles (#74, #293),
// checked before anything is made.
//
// A worktree that already has the branch checked out, wherever it is (new_wt
// puts it beside the current one; the main worktree too, as sd act implement
// run there would be), is used as it is (#252): no git worktree
// add, no setup hook, the rest as for a new one. So a second start on a branch
// just rewrites its task. A branch that was there already gets base pinned at
// its tip only when none is pinned and it has no commits past its start point.
// The default branch is refused, as is a registered worktree whose directory
// is gone: git worktree add refused both before, and start would use them.
//
// Duplicates agent-rig's wt-new (#14), hook locations and branch resolution
// included, so a repo set up for new_wt needs nothing more. Placement differs
// (#85): beside the main worktree, not beside the current one.

use super::bookkeeping::enroll_mark;
use super::{die, or_die, Args};
use crate::config::set_branch_profile;
use crate::git::{common_dir, git, git_ok, git_out, is_ancestor, main_worktree, rev, show_ref, worktree_for, Git};
use crate::proc::{self, executable, on_stderr};
use std::path::Path;
use std::process::{Command, ExitStatus};

/// The remote's default branch as fetched: <remote>/HEAD, else main or
/// master, remote or local.
pub fn start_default(remote: &str) -> Option<String> {
    let head = git_out(".", &["symbolic-ref", "-q", "--short", &format!("refs/remotes/{remote}/HEAD")]);
    if let Some(h) = head {
        return Some(h.strip_prefix(&format!("{remote}/")).unwrap_or(&h).to_string());
    }
    ["main", "master"]
        .into_iter()
        .find(|br| show_ref(".", &format!("refs/remotes/{remote}/{br}")) || show_ref(".", &format!("refs/heads/{br}")))
        .map(str::to_string)
}

/// <b> on <remote>, else here, as a full ref.
pub(super) fn start_point(remote: &str, b: &str) -> Option<String> {
    [format!("refs/remotes/{remote}/{b}"), format!("refs/heads/{b}")].into_iter().find(|r| show_ref(".", r))
}

/// Where a branch starts, as a full ref: --parent, local else on <remote>,
/// else the default branch. Dies on a --parent that doesn't exist; Err says
/// why there's no default branch to start from.
fn branch_from(remote: &str, parent: &str) -> Result<String, String> {
    if !parent.is_empty() {
        let local = format!("refs/heads/{parent}");
        if show_ref(".", &local) {
            return Ok(local);
        }
        return Ok(start_point(remote, parent).unwrap_or_else(|| die(format!("no such parent branch: {parent}"))));
    }
    let def = start_default(remote).ok_or(format!("can't tell the default branch (no {remote}/HEAD, main or master)"))?;
    start_point(remote, &def).ok_or(format!("no {remote}/{def} or {def} to start from"))
}

/// Where the new worktree goes: beside the main worktree, whichever worktree
/// this runs from, so a stack built with cd "$(sd act start ...)" stays in one
/// place; a slash in the branch name would make directories, so it becomes a
/// dash (#85).
pub(super) fn destination(main: &str, b: &str) -> String {
    let parent = main.rsplit_once('/').map_or(".", |(p, _)| if p.is_empty() { "/" } else { p });
    format!("{}/{}", parent.trim_end_matches('/'), b.replace('/', "-"))
}

/// The shell's $? for <st>, 1 when it couldn't run.
fn code(st: std::io::Result<ExitStatus>) -> i32 {
    st.map_or(1, proc::code)
}

/// The repo's setup hook in the new worktree, as wt-new runs it:
/// $GIT_COMMON_DIR/worktree-setup, else git config wt.setup. All its output
/// goes to stderr. false when it failed.
pub(super) fn start_hook(src: &str, dst: &str, b: &str) -> bool {
    let script = format!("{}/worktree-setup", common_dir(".").unwrap_or_default());
    let mut c = if executable(Path::new(&script)) {
        eprintln!("sd act: running {script}");
        Command::new(&script)
    } else {
        let cmd = git_out(".", &["config", "--get", "wt.setup"]).unwrap_or_default();
        if cmd.is_empty() {
            return true;
        }
        eprintln!("sd act: running wt.setup: {cmd}");
        let mut c = Command::new("sh");
        c.args(["-c", &cmd]);
        c
    };
    c.current_dir(dst).env("WT_SRC", src).env("WT_DST", dst).env("WT_BRANCH", b);
    let rc = code(on_stderr(c));
    if rc != 0 {
        eprintln!("sd act: the setup hook failed (exit {rc}); the worktree is kept at {dst}");
    }
    rc == 0
}

/// git, its output on stderr as git writes it (so in git's order); dies
/// saying <what> failed.
pub(super) fn git_or_die(args: &[&str], what: &str) {
    if !git_on_stderr(args) {
        die(format!("{what} failed"));
    }
}

/// git, its output on stderr as git writes it; false when it failed.
pub(super) fn git_on_stderr(args: &[&str]) -> bool {
    Git::new(".", args).on_stderr()
}

/// implement in the new worktree, in its own process as bash's subshell had
/// it, so that whatever ends implement, start still prints the path.
fn implement(dst: &str, b: &str, a: &Args) -> i32 {
    let exe = proc::sd_itself().unwrap_or_else(|e| die(format!("can't find sd itself: {e}")));
    let mut c = Command::new(exe);
    c.args(["act", "implement"]).current_dir(dst);
    if !a.harness.is_empty() {
        c.args(["--harness", &a.harness]);
    }
    if a.force {
        c.arg("--force");
    }
    c.arg(b);
    code(on_stderr(c))
}

pub fn start(b: &str, remote: &str, a: &Args) -> i32 {
    if !git_ok(".", &["check-ref-format", "--branch", b]) {
        die(format!("not a valid branch name: {b}"));
    }
    if !a.no_implement && a.task.is_empty() {
        die(format!("no task: sd act start {b} \"<task>\" (or --no-implement)"));
    }
    let src = git_out(".", &["rev-parse", "--show-toplevel"]).unwrap_or_else(|| die("not inside a worktree"));
    // Before #252 git worktree add refused it where it was checked out; now
    // start would use that checkout, so it's refused outright.
    if start_default(remote).as_deref() == Some(b) {
        die(format!("{b} is the default branch: start makes a branch off it"));
    }
    let adopted = worktree_for(".", b);
    if !adopted.is_empty() && !Path::new(&adopted).is_dir() {
        die(format!("{b} is checked out at {adopted}, which is gone: git worktree prune, then start again"));
    }
    let dst = if adopted.is_empty() { destination(&main_worktree("."), b) } else { adopted.clone() };
    if adopted.is_empty() && Path::new(&dst).exists() {
        die(format!("{dst} already exists"));
    }
    if !a.profiles.is_empty() {
        or_die(set_branch_profile(b, &a.profiles, true));
    }
    let f = git(".", &["fetch", "--quiet", "--prune", remote]);
    eprint!("{}", f.stderr);
    if !f.ok {
        die(format!("fetch from {remote} failed"));
    }

    let from = branch_from(remote, &a.parent);
    let mut base = String::new();
    if !adopted.is_empty() {
        eprintln!("sd act: {b} is already checked out at {dst}; using it");
    } else if show_ref(".", &format!("refs/heads/{b}")) {
        git_or_die(&["worktree", "add", &dst, b], "git worktree add");
    } else if show_ref(".", &format!("refs/remotes/{remote}/{b}")) {
        eprintln!("sd act: tracking {remote}/{b}");
        git_or_die(&["worktree", "add", "--track", "-b", b, &dst, &format!("{remote}/{b}")], "git worktree add");
    } else {
        let from = from.as_deref().unwrap_or_else(|e| die(e));
        base = rev(".", from);
        let name = from.strip_prefix("refs/").and_then(|r| r.split_once('/')).map_or(from, |(_, n)| n);
        eprintln!("sd act: branching {b} off {name} ({})", base.get(..7).unwrap_or(&base));
        // From the SHA, not the ref: a remote-tracking start point would become the upstream.
        git_or_die(&["worktree", "add", "-b", b, &dst, &base], "git worktree add");
    }
    // A branch that was there already: base at its tip when nothing pinned one
    // and it has no commits of its own past where it would start (#252).
    if base.is_empty() && !show_ref(".", &format!("refs/sheepdog/{b}/base")) {
        if let Ok(from) = &from {
            let tip = rev(".", &format!("refs/heads/{b}"));
            if !tip.is_empty() && is_ancestor(".", &tip, from) {
                base = tip;
            }
        }
    }

    let set = |args: &[&str]| {
        let o = git(".", args);
        if !o.ok {
            eprint!("{}", o.stderr);
            std::process::exit(o.code);
        }
    };
    if !a.task.is_empty() {
        set(&["config", &format!("branch.{b}.sd-task"), &a.task]);
    }
    if !a.parent.is_empty() {
        set(&["config", &format!("branch.{b}.sd-parent"), &a.parent]);
    }
    // Before the first step, which may fail (#242); a branch with a session
    // keeps its session's harness.
    let session = git_out(".", &["config", "--get", &format!("branch.{b}.sd-session")]);
    if !a.harness.is_empty() && session.is_none() {
        set(&["config", &format!("branch.{b}.sd-harness"), &a.harness]);
    }
    // Before implement too, whose first verify is the profile's (#293).
    if !a.profiles.is_empty() {
        or_die(set_branch_profile(b, &a.profiles, false));
    }
    if !base.is_empty() {
        set(&["update-ref", "-m", "sd act start", &format!("refs/sheepdog/{b}/base"), &base]);
    }
    enroll_mark(b);
    eprintln!("enrolled {b}");

    let mut rc = 0;
    if adopted.is_empty() && !start_hook(&src, &dst, b) {
        rc = 1;
        if !a.no_implement {
            eprintln!("sd act: fix it, then: sd act implement {b}");
        }
    } else if !a.no_implement {
        rc = implement(&dst, b, a);
    }
    println!("{dst}");
    rc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destination_is_beside_the_main_worktree_slashes_as_dashes() {
        assert_eq!(destination("/tmp/x/repo", "feat-x"), "/tmp/x/feat-x");
        assert_eq!(destination("/tmp/x/repo", "12/stack-child"), "/tmp/x/12-stack-child");
        assert_eq!(destination("/repo", "feat"), "/feat");
    }
}
