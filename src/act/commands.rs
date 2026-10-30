// verify (A10) and autofix (A11), and implement's and ci-fix's hook into
// them.

use super::{cfg, die, lock_recheck, plural};
use crate::git::{git, git_out, worktree_for};
use crate::{proc, verify_red};

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

/// A line for each TAP plan (1..M) in <out> whose numbers no ok / not ok
/// line reports, as bats' "Executed N instead of expected M" leaves it with
/// no "not ok" to point at (#325). TAP gives a test that never reports no
/// name, so each gap is placed by the line reported just before it. A plan
/// line starts a new run's count, or, with no plan above them, ends the
/// results above it (TAP allows either end); output that mixes the two
/// isn't told apart. A heuristic: verify's output is any command's, so a
/// plan with no results under it adds nothing.
fn tap_missing(out: &str) -> Option<String> {
    let mut notes = String::new();
    let (mut plan, mut seen) = (None, Vec::new());
    for l in out.lines() {
        let plan_line = l.strip_prefix("1..").and_then(|r| r.split(' ').next()?.parse().ok());
        if let Some(m) = plan_line.filter(|&m: &usize| m < usize::MAX) {
            match plan {
                None if !seen.is_empty() => notes += &tap_gaps(m, std::mem::take(&mut seen)),
                Some(p) => {
                    notes += &tap_gaps(p, std::mem::take(&mut seen));
                    plan = Some(m);
                }
                None => plan = Some(m),
            }
        } else {
            let result = l.strip_prefix("not ok ").or(l.strip_prefix("ok "));
            if let Some(n) = result.and_then(|r| r.split(' ').next()?.parse::<usize>().ok()) {
                seen.push((n, l));
            }
        }
    }
    if let Some(p) = plan {
        notes += &tap_gaps(p, seen);
    }
    (!notes.is_empty()).then_some(notes)
}

/// tap_missing's line for one plan, or empty when every number reported or
/// none did.
fn tap_gaps(m: usize, mut seen: Vec<(usize, &str)>) -> String {
    seen.retain(|&(n, _)| (1..=m).contains(&n));
    if seen.is_empty() {
        return String::new();
    }
    seen.sort_by_key(|&(n, _)| n);
    seen.dedup_by_key(|&mut (n, _)| n);
    let (mut gaps, mut prev) = (Vec::new(), (0, ""));
    for (n, l) in seen.iter().copied().chain([(m + 1, "")]) {
        let at = match prev {
            (0, _) => "at the start".to_string(),
            (_, before) => format!("after: {before}"),
        };
        match n - prev.0 {
            1 => {}
            2 => gaps.push(format!("{} ({at})", n - 1)),
            _ => gaps.push(format!("{}-{} ({at})", prev.0 + 1, n - 1)),
        }
        prev = (n, l);
    }
    if gaps.is_empty() {
        return String::new();
    }
    format!("TAP plan 1..{m} but {} reported; no line for {}. TAP doesn't name the missing tests.\n", seen.len(), gaps.join(", "))
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

/// settings.verify in <wt>, run for <step>: Ok(the command) when it passed,
/// or empty when none is configured; Err(the command) when it failed, after
/// printing why.
pub(super) fn verify_run(wt: &str, b: &str, step: &str) -> Result<String, String> {
    verify_to(wt, b, step, &mut Report::Print)
}

/// The verdict is recorded against <wt>'s HEAD (src/verify_red.rs); a run sd
/// stopped on a signal is no verdict.
fn verify_to(wt: &str, b: &str, step: &str, r: &mut Report) -> Result<String, String> {
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
        r.say(&tap_missing(&o.stdout).unwrap_or_default());
    } else {
        // A pass on uncommitted edits didn't test HEAD.
        if before.is_empty() {
            verify_red::clear(wt, b);
        }
        return Ok(cmd);
    }
    if !o.stopped {
        verify_red::record(wt, b, step, &cmd);
    }
    Err(cmd)
}

pub fn verify(b: &str) {
    match verify_run(&worktree(b), b, "verify") {
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
    if let Err(cmd) = autofix_run(&wt, b, None, &mut Report::Print) {
        die(format!("autofix failed ({cmd})"));
    }
}

/// settings.autofix in <wt>, from a clean tree, and a commit of what it
/// changed: Ok(Some(its short sha)) when it committed, else Ok(None), as when
/// none is configured.
/// Err(the command) when it failed, after printing its output's tail; its
/// changes are left uncommitted to look at, or inside agent step <step> (the
/// hook) put back, so the next step starts from the commit it knows. Inside a
/// step it rechecks the attach lock before it commits or reverts (#270).
fn autofix_run(wt: &str, b: &str, step: Option<&str>, r: &mut Report) -> Result<Option<String>, String> {
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
    let left = format!("autofix's changes are left in {wt}");
    if !o.ok {
        r.say(&tail(&o.stdout, VERIFY_TAIL));
        if let Some(step) = step.filter(|_| changed > 0) {
            // A revert would throw an attached session's edits away too.
            lock_recheck(b, step, "reverting", &left);
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
    if let Some(step) = step {
        lock_recheck(b, step, "committing", &left);
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
pub(super) fn verify_autofix(wt: &str, b: &str, step: &str) -> Result<(), String> {
    verify_autofix_to(wt, b, step, &mut Report::Print).0
}

/// verify_autofix, saying so to <r>, and autofix's commit if it made one.
pub(super) fn verify_autofix_to(wt: &str, b: &str, step: &str, r: &mut Report) -> (Result<(), String>, Option<String>) {
    let Err(cmd) = verify_to(wt, b, step, r) else { return (Ok(()), None) };
    if !status(wt).is_empty() {
        return (Err(cmd), None);
    }
    let Ok(Some(sha)) = autofix_run(wt, b, Some(step), r) else { return (Err(cmd), None) };
    let verified = verify_to(wt, b, step, r);
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

    #[test]
    fn tap_missing_names_the_plan_numbers_with_no_line() {
        let out = "1..5\nok 1 a\nnot ok 2 b\n# (in test file x.bats)\nok 3 c # skip\n# bats warning: Executed 3 instead of expected 5 tests\n";
        assert_eq!(
            tap_missing(out).as_deref(),
            Some("TAP plan 1..5 but 3 reported; no line for 4-5 (after: ok 3 c # skip). TAP doesn't name the missing tests.\n"),
        );
    }

    // bats 1.13's output for a test whose EXIT trap ran "exit 0": the gap is
    // in the middle, not at the end.
    #[test]
    fn tap_missing_places_a_gap_in_the_middle() {
        let out = "1..4\nok 1 first\nok 3 third\nok 4 fourth\n# bats warning: Executed 3 instead of expected 4 tests\n";
        assert_eq!(
            tap_missing(out).as_deref(),
            Some("TAP plan 1..4 but 3 reported; no line for 2 (after: ok 1 first). TAP doesn't name the missing tests.\n"),
        );
    }

    #[test]
    fn tap_missing_joins_runs_and_singles() {
        let out = "1..7\nok 6 f\nok 2 b\nok 3 c\n";
        assert_eq!(
            tap_missing(out).as_deref(),
            Some("TAP plan 1..7 but 3 reported; no line for 1 (at the start), 4-5 (after: ok 3 c), 7 (after: ok 6 f). TAP doesn't name the missing tests.\n"),
        );
    }

    #[test]
    fn tap_missing_checks_each_plan_on_its_own() {
        let out = "1..2\nok 1 a\nok 2 b\n1..2\nok 1 c\n";
        assert_eq!(
            tap_missing(out).as_deref(),
            Some("TAP plan 1..2 but 1 reported; no line for 2 (after: ok 1 c). TAP doesn't name the missing tests.\n"),
        );
    }

    #[test]
    fn tap_missing_is_none_when_every_test_reported_or_there_is_no_plan() {
        assert_eq!(tap_missing("1..2\nok 1 a\nnot ok 2 b\n"), None);
        assert_eq!(tap_missing("ok 1 a\nFAIL\n"), None);
        assert_eq!(tap_missing("1..0 # skipped\n"), None);
        assert_eq!(tap_missing(""), None);
    }

    #[test]
    fn tap_missing_takes_a_plan_at_the_end_for_the_results_above_it() {
        assert_eq!(tap_missing("ok 1 a\nok 2 b\n1..2\n"), None);
        assert_eq!(
            tap_missing("ok 1 a\nok 3 c\n1..3\n").as_deref(),
            Some("TAP plan 1..3 but 2 reported; no line for 2 (after: ok 1 a). TAP doesn't name the missing tests.\n"),
        );
    }

    // A "1..3" with no result under it may not be TAP at all.
    #[test]
    fn tap_missing_is_none_for_a_plan_with_no_results() {
        assert_eq!(tap_missing("1..3\nboom\n"), None);
        assert_eq!(tap_missing("1..3\nok 1 a\nok 2 b\nok 3 c\n1..2\n"), None);
    }
}
