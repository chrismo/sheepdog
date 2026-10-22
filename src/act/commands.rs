// verify (A10) and autofix (A11). libexec/sd-act's verify_run and
// autofix_run still do the same for the bash agent steps: change both. One
// difference until #203 ports those: there, a failed git status still reads
// as a clean tree.

use super::{cfg, die, plural};
use crate::git::{git, git_out, worktree_for};
use crate::proc;

const VERIFY_TAIL: usize = 40;

fn worktree(b: &str) -> String {
    let wt = worktree_for(".", b);
    if wt.is_empty() {
        die(format!("{b} is not checked out in any worktree"));
    }
    wt
}

fn command(name: &str, b: &str) -> String {
    let c = cfg(&format!("settings.{name}"), b);
    if c.contains('\n') {
        die(format!("{name} must be a single command, not a list"));
    }
    c
}

pub(super) fn step_timeout(b: &str) -> u64 {
    let t = cfg("settings.stepTimeout", b);
    let whole = !t.starts_with('0') && t.bytes().all(|c| c.is_ascii_digit());
    match t.parse() {
        Ok(secs) if whole => secs,
        _ => die(format!("settings.stepTimeout must be whole seconds: {t}")),
    }
}

// A failed status isn't a clean tree: it would pass verify and hide
// autofix's changes.
pub(super) fn status(wt: &str) -> String {
    let g = git(wt, &["status", "--porcelain"]);
    if !g.ok {
        die(format!("git status failed in {wt}: {}", g.stderr.trim()));
    }
    g.stdout
}

fn print_tail(out: &str) {
    let lines: Vec<&str> = out.split_terminator('\n').collect();
    let n = lines.len();
    if n > VERIFY_TAIL {
        println!("… (last {VERIFY_TAIL} of {n} lines)");
    }
    for l in &lines[lines.len().saturating_sub(VERIFY_TAIL)..] {
        println!("{l}");
    }
}

/// settings.verify in <wt>: Ok(the command) when it passed, or empty when
/// none is configured; Err(the command) when it failed, after printing why.
pub(super) fn verify_run(wt: &str, b: &str) -> Result<String, String> {
    let cmd = command("verify", b);
    if cmd.is_empty() {
        return Ok(cmd);
    }
    let secs = step_timeout(b);
    let before = status(wt);
    let o = proc::with_timeout_merged(secs, "sh", &["-c", &cmd], Some(wt));
    if o.ok && status(wt) != before {
        println!("verify changed files in {wt} (a formatter?); commit them and rerun");
    } else if !o.ok {
        print_tail(&o.stdout);
    } else {
        return Ok(cmd);
    }
    Err(cmd)
}

pub fn verify(b: &str) {
    match verify_run(&worktree(b), b) {
        Ok(cmd) if cmd.is_empty() => println!("no settings.verify configured; nothing to run"),
        Ok(cmd) => println!("verify passed ({cmd})"),
        Err(cmd) => die(format!("verify failed ({cmd})")),
    }
}

pub fn autofix(b: &str) {
    let wt = worktree(b);
    let cmd = command("autofix", b);
    if cmd.is_empty() {
        println!("no settings.autofix configured; nothing to run");
        return;
    }
    if !status(&wt).is_empty() {
        die(format!("{wt} has uncommitted changes; autofix needs a clean tree to commit from"));
    }
    let secs = step_timeout(b);
    let o = proc::with_timeout_merged(secs, "sh", &["-c", &cmd], Some(&wt));
    let changed = status(&wt).lines().count();
    if !o.ok {
        print_tail(&o.stdout);
        if changed > 0 {
            println!("autofix changed {} in {wt} before failing; left uncommitted", plural(changed, "file"));
        }
        die(format!("autofix failed ({cmd})"));
    }
    if changed == 0 {
        println!("autofix changed nothing ({cmd})");
        return;
    }
    for args in [&["add", "-A"][..], &["commit", "-q", "-m", &format!("autofix: {cmd}")]] {
        let g = git(&wt, args);
        // The hooks' output: git sends theirs to stderr.
        print!("{}", g.stdout);
        eprint!("{}", g.stderr);
        if !g.ok {
            die(format!("commit failed; autofix's changes are left in {wt}"));
        }
    }
    let sha = git_out(&wt, &["rev-parse", "--short", "HEAD"]).unwrap_or_default();
    println!("autofix committed {sha}: {} ({cmd})", plural(changed, "file"));
}
