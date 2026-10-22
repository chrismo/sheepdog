// verify (A10) and autofix (A11), and implement's and ci-fix's hook into
// them. libexec/sd-act's verify_run, autofix_run and verify_autofix still do
// the same for the bash agent steps: change both. One difference until #203
// ports those: there, a failed git status still reads as a clean tree.

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

pub(super) fn command(name: &str, b: &str) -> String {
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

/// <text>'s last <n> lines, after a note when that cut some.
pub(super) fn tail(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.split_terminator('\n').collect();
    let mut out = match lines.len() {
        all if all > n => format!("… (last {n} of {all} lines)\n"),
        _ => String::new(),
    };
    for l in &lines[lines.len().saturating_sub(n)..] {
        out += &format!("{l}\n");
    }
    out
}

/// Where verify and autofix say what they did: stdout, or kept for a step
/// that hands it on (ci-fix's prompt).
pub(super) enum Report<'a> {
    Print,
    Keep(&'a mut String),
}

impl Report<'_> {
    fn say(&mut self, s: &str) {
        match self {
            Report::Print => print!("{s}"),
            Report::Keep(buf) => buf.push_str(s),
        }
    }

    fn line(&mut self, l: &str) {
        self.say(&format!("{l}\n"));
    }
}

/// settings.verify in <wt>: Ok(the command) when it passed, or empty when
/// none is configured; Err(the command) when it failed, after printing why.
pub(super) fn verify_run(wt: &str, b: &str) -> Result<String, String> {
    verify_to(wt, b, &mut Report::Print)
}

fn verify_to(wt: &str, b: &str, r: &mut Report) -> Result<String, String> {
    let cmd = command("verify", b);
    if cmd.is_empty() {
        return Ok(cmd);
    }
    let secs = step_timeout(b);
    let before = status(wt);
    let o = proc::with_timeout_merged(secs, "sh", &["-c", &cmd], Some(wt));
    if o.ok && status(wt) != before {
        r.line(&format!("verify changed files in {wt} (a formatter?); commit them and rerun"));
    } else if !o.ok {
        r.say(&tail(&o.stdout, VERIFY_TAIL));
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
    if command("autofix", b).is_empty() {
        println!("no settings.autofix configured; nothing to run");
        return;
    }
    if let Err(cmd) = autofix_run(&wt, b, false, &mut Report::Print) {
        die(format!("autofix failed ({cmd})"));
    }
}

/// settings.autofix in <wt>, from a clean tree, and a commit of what it
/// changed: Ok(Some(its short sha)) when it committed, else Ok(None), as when
/// none is configured.
/// Err(the command) when it failed, after printing its output's tail; its
/// changes are left uncommitted to look at, or with <revert> (the hook) put
/// back, so the next step starts from the commit it knows.
fn autofix_run(wt: &str, b: &str, revert: bool, r: &mut Report) -> Result<Option<String>, String> {
    let cmd = command("autofix", b);
    if cmd.is_empty() {
        return Ok(None);
    }
    if !status(wt).is_empty() {
        die(format!("{wt} has uncommitted changes; autofix needs a clean tree to commit from"));
    }
    let secs = step_timeout(b);
    let o = proc::with_timeout_merged(secs, "sh", &["-c", &cmd], Some(wt));
    let changed = status(wt).lines().count();
    if !o.ok {
        r.say(&tail(&o.stdout, VERIFY_TAIL));
        if changed > 0 && revert {
            for args in [&["reset", "-q", "--hard"][..], &["clean", "-qfd"]] {
                eprint!("{}", git(wt, args).stderr);
            }
            r.line(&format!("autofix changed {} before failing; reverted", plural(changed, "file")));
        } else if changed > 0 {
            r.line(&format!("autofix changed {} in {wt} before failing; left uncommitted", plural(changed, "file")));
        }
        return Err(cmd);
    }
    if changed == 0 {
        r.line(&format!("autofix changed nothing ({cmd})"));
        return Ok(None);
    }
    for args in [&["add", "-A"][..], &["commit", "-q", "-m", &format!("autofix: {cmd}")]] {
        let g = git(wt, args);
        // The hooks' output: git sends theirs to stderr.
        r.say(&g.stdout);
        eprint!("{}", g.stderr);
        if !g.ok {
            die(format!("commit failed; autofix's changes are left in {wt}"));
        }
    }
    let sha = git_out(wt, &["rev-parse", "--short", "HEAD"]).unwrap_or_default();
    r.line(&format!("autofix committed {sha}: {} ({cmd})", plural(changed, "file")));
    Ok(Some(sha))
}

/// verify; when it fails, autofix, and when that committed something, verify
/// again (A11). The zero-token pass after a fix step. A verify that dirtied
/// the tree is verify's own failure: no autofix. Err is the verify command.
pub(super) fn verify_autofix(wt: &str, b: &str) -> Result<(), String> {
    verify_autofix_to(wt, b, &mut Report::Print).0
}

/// verify_autofix, saying so to <r>, and autofix's commit if it made one.
pub(super) fn verify_autofix_to(wt: &str, b: &str, r: &mut Report) -> (Result<(), String>, Option<String>) {
    let Err(cmd) = verify_to(wt, b, r) else { return (Ok(()), None) };
    if !status(wt).is_empty() {
        return (Err(cmd), None);
    }
    let Ok(Some(sha)) = autofix_run(wt, b, true, r) else { return (Err(cmd), None) };
    let verified = verify_to(wt, b, r);
    if let Ok(cmd) = &verified {
        r.line(&format!("verify passed after autofix ({cmd})"));
    }
    (verified.map(|_| ()), Some(sha))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_keeps_the_last_lines_and_says_when_it_cut() {
        assert_eq!(tail("a\nb\nc\n", 3), "a\nb\nc\n");
        assert_eq!(tail("a\nb\nc", 2), "… (last 2 of 3 lines)\nb\nc\n");
        assert_eq!(tail("", 2), "");
    }
}
