// start (A0): a worktree beside the main one on the branch, the task, parent
// and base recorded, the branch enrolled; then implement. stdout is only the
// worktree's path.
//
// Duplicates agent-rig's wt-new (#14), hook locations and branch resolution
// included, so a repo set up for new_wt needs nothing more. Placement differs
// (#85): beside the main worktree, not beside the current one.

use super::bookkeeping::enroll_mark;
use super::{die, Args};
use crate::git::{common_dir, git, git_ok, git_out, main_worktree, rev, show_ref};
use crate::proc::{self, executable};
use std::io::stderr;
use std::os::fd::AsFd;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};

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
fn start_point(remote: &str, b: &str) -> Option<String> {
    [format!("refs/remotes/{remote}/{b}"), format!("refs/heads/{b}")].into_iter().find(|r| show_ref(".", r))
}

/// Where the new worktree goes: beside the main worktree, whichever worktree
/// this runs from, so a stack built with cd "$(sd act start ...)" stays in one
/// place; a slash in the branch name would make directories, so it becomes a
/// dash (#85).
fn destination(main: &str, b: &str) -> String {
    let parent = main.rsplit_once('/').map_or(".", |(p, _)| if p.is_empty() { "/" } else { p });
    format!("{}/{}", parent.trim_end_matches('/'), b.replace('/', "-"))
}

/// <c> with its stdout on sd's stderr: start's stdout is only the path.
fn on_stderr(mut c: Command) -> std::io::Result<ExitStatus> {
    let err = stderr().as_fd().try_clone_to_owned()?;
    c.stdout(Stdio::from(err)).status()
}

/// The shell's $? for <st>, 1 when it couldn't run.
fn code(st: std::io::Result<ExitStatus>) -> i32 {
    st.map_or(1, proc::code)
}

/// The repo's setup hook in the new worktree, as wt-new runs it:
/// $GIT_COMMON_DIR/worktree-setup, else git config wt.setup. All its output
/// goes to stderr. false when it failed.
fn start_hook(src: &str, dst: &str, b: &str) -> bool {
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

/// git, its output on stderr; dies saying <what> failed.
fn git_or_die(args: &[&str], what: &str) {
    let o = git(".", args);
    eprint!("{}{}", o.stdout, o.stderr);
    if !o.ok {
        die(format!("{what} failed"));
    }
}

/// implement in the new worktree, in its own process as bash's subshell had
/// it, so that whatever ends implement, start still prints the path. The
/// binary itself, not the link this ran as: on macOS current_exe is the
/// sd-act link, which would take "act" for its verb.
fn implement(dst: &str, b: &str, a: &Args) -> i32 {
    let exe = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .unwrap_or_else(|e| die(format!("can't find sd itself: {e}")));
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
    let dst = destination(&main_worktree("."), b);
    if Path::new(&dst).exists() {
        die(format!("{dst} already exists"));
    }
    let f = git(".", &["fetch", "--quiet", "--prune", remote]);
    eprint!("{}", f.stderr);
    if !f.ok {
        die(format!("fetch from {remote} failed"));
    }

    let mut base = String::new();
    if show_ref(".", &format!("refs/heads/{b}")) {
        git_or_die(&["worktree", "add", &dst, b], "git worktree add");
    } else if show_ref(".", &format!("refs/remotes/{remote}/{b}")) {
        eprintln!("sd act: tracking {remote}/{b}");
        git_or_die(&["worktree", "add", "--track", "-b", b, &dst, &format!("{remote}/{b}")], "git worktree add");
    } else {
        let from = if !a.parent.is_empty() {
            let local = format!("refs/heads/{}", a.parent);
            if show_ref(".", &local) {
                local
            } else {
                start_point(remote, &a.parent).unwrap_or_else(|| die(format!("no such parent branch: {}", a.parent)))
            }
        } else {
            let def = start_default(remote)
                .unwrap_or_else(|| die(format!("can't tell the default branch (no {remote}/HEAD, main or master)")));
            start_point(remote, &def).unwrap_or_else(|| die(format!("no {remote}/{def} or {def} to start from")))
        };
        base = rev(".", &from);
        let name = from.strip_prefix("refs/").and_then(|r| r.split_once('/')).map_or(from.as_str(), |(_, n)| n);
        eprintln!("sd act: branching {b} off {name} ({})", base.get(..7).unwrap_or(&base));
        // From the SHA, not the ref: a remote-tracking start point would become the upstream.
        git_or_die(&["worktree", "add", "-b", b, &dst, &base], "git worktree add");
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
    if !base.is_empty() {
        set(&["update-ref", "-m", "sd act start", &format!("refs/sheepdog/{b}/base"), &base]);
    }
    enroll_mark(b);
    eprintln!("enrolled {b}");

    let mut rc = 0;
    if !start_hook(&src, &dst, b) {
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
