// The job's log lines (#364): the time on every line a pass writes to
// launchd.log, the summary line that closes a pass, and the header and end
// lines around a launched step's output in steps.log. One clock: in a debug
// build SHEEPDOG_WATCH_CLOCK fixes it, read once; a release build never
// reads it.

use crate::time::took;
use std::sync::OnceLock;

/// date '+%Y-%m-%d %H:%M:%S', local time.
pub(super) fn now() -> String {
    static FIXED: OnceLock<Option<String>> = OnceLock::new();
    let fixed = FIXED.get_or_init(|| {
        let v = if cfg!(debug_assertions) { std::env::var("SHEEPDOG_WATCH_CLOCK").ok() } else { None };
        v.filter(|v| !v.is_empty())
    });
    fixed.clone().unwrap_or_else(local_time)
}

// localtime_r needn't read TZ, as localtime does. Not in the libc crate.
extern "C" {
    fn tzset();
}

fn local_time() -> String {
    let mut buf = [0u8; 64];
    // SAFETY: tm is plain data that localtime_r fills, and strftime writes
    // at most buf.len() bytes, returning how many.
    let n = unsafe {
        tzset();
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return String::new();
        }
        libc::strftime(buf.as_mut_ptr().cast(), buf.len(), c"%Y-%m-%d %H:%M:%S".as_ptr(), &tm)
    };
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

/// Each line of <msg>, led by the time, to stderr: the job's log.
pub(super) fn say(msg: &str) {
    eprint!("{}", stamped(&now(), msg));
}

fn stamped(at: &str, msg: &str) -> String {
    msg.lines().map(|l| format!("{at} {l}\n")).collect()
}

/// What a pass counted, for the summary line that closes it.
pub(super) struct Tally {
    pub branches: usize,
    /// settings.autoRun globally, and the branches whose own differs.
    pub auto_run: bool,
    pub overrides: usize,
    pub launched: usize,
    /// Agent steps running as the pass ends, against settings.agentSteps.
    pub agents: u64,
    pub cap: u64,
}

fn branches(n: usize) -> String {
    format!("{n} branch{}", if n == 1 { "" } else { "es" })
}

fn on(b: bool) -> &'static str {
    if b {
        "on"
    } else {
        "off"
    }
}

pub(super) fn summary(t: &Tally) -> String {
    let overrides = if t.overrides == 0 { String::new() } else { format!(" ({} {})", branches(t.overrides), on(!t.auto_run)) };
    format!(
        "pass: {}, autoRun {}{overrides}, {} launched, agent steps {}/{}",
        branches(t.branches),
        on(t.auto_run),
        t.launched,
        t.agents,
        t.cap
    )
}

/// The line before a launched step's output in steps.log.
pub(super) fn step_header(at: &str, repo: &str, b: &str, verb: &str) -> String {
    format!("==> {at} {repo} {b}: sd act {verb}")
}

/// The line after it: <outcome>, the step's outcome line, and how long it took.
pub(super) fn step_end(at: &str, repo: &str, b: &str, outcome: &str, secs: u64) -> String {
    format!("<== {at} {repo} {b}: {outcome} ({})", took(secs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proc;
    use regex::Regex;

    const AT: &str = "2026-11-02 09:27:10";

    #[test]
    fn stamped_leads_each_line_with_the_time() {
        assert_eq!(stamped(AT, "one\ntwo\n"), "2026-11-02 09:27:10 one\n2026-11-02 09:27:10 two\n");
        assert_eq!(stamped(AT, "bare"), "2026-11-02 09:27:10 bare\n");
        assert_eq!(stamped(AT, ""), "");
    }

    fn tally(branches: usize, auto_run: bool, overrides: usize, launched: usize, agents: u64, cap: u64) -> String {
        summary(&Tally { branches, auto_run, overrides, launched, agents, cap })
    }

    #[test]
    fn summary_counts_branches_autorun_its_overrides_launches_and_agent_steps() {
        assert_eq!(tally(4, false, 0, 0, 0, 2), "pass: 4 branches, autoRun off, 0 launched, agent steps 0/2");
        assert_eq!(tally(4, true, 2, 1, 1, 2), "pass: 4 branches, autoRun on (2 branches off), 1 launched, agent steps 1/2");
        assert_eq!(tally(1, false, 1, 1, 2, 2), "pass: 1 branch, autoRun off (1 branch on), 1 launched, agent steps 2/2");
        assert_eq!(tally(0, true, 0, 0, 0, 0), "pass: 0 branches, autoRun on, 0 launched, agent steps 0/0");
    }

    #[test]
    fn step_header_and_end_name_the_time_repo_branch_verb_outcome_and_duration() {
        assert_eq!(step_header(AT, "sheepdog", "feat", "restack --resolve"), "==> 2026-11-02 09:27:10 sheepdog feat: sd act restack --resolve");
        assert_eq!(
            step_end(AT, "sheepdog", "feat", "ran sd act push: ok", 3),
            "<== 2026-11-02 09:27:10 sheepdog feat: ran sd act push: ok (3s)"
        );
    }

    #[test]
    fn local_time_is_dates_format() {
        let s = local_time();
        assert!(Regex::new(r"^\d{4}-\d\d-\d\d \d\d:\d\d:\d\d$").unwrap().is_match(&s), "{s}");
        // Local, not UTC: date's hour, read either side of it so an hour
        // boundary between them doesn't fail the test.
        let hour = || proc::run("date", &["+%H"], None, None).stdout.trim_end().to_string();
        let before = hour();
        let s = local_time();
        let after = hour();
        assert!(s[11..13] == before || s[11..13] == after, "{s} vs {before}..{after}");
    }
}
