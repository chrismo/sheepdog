// sd watch status (#364): the last pass, then what usually explains a
// silent job, read without writing anything (src/watch_help.txt, Logs).
// A branch's row in last-pass is row's (#317).

use super::pass::{auto_run, basename, dur, enrolled, registered};
use super::{uid, LABEL};
use crate::config::{self, state_dir};
use crate::next::FAIL_LIMIT;
use crate::pins::{self, Pin};
use crate::val::{at, t, text};
use crate::git::git_out;
use crate::{lock, running, time};
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::SystemTime;

pub(super) fn print() {
    let state = state_dir();
    match fs::read_to_string(format!("{state}/last-pass")) {
        Ok(text) => print!("{}", text.replace('\t', "  ")),
        Err(_) => println!("no pass yet"),
    }
    println!();
    print!("{}", block(&state));
}

/// (name, path) of each registered repo that exists.
fn repos() -> Vec<(String, String)> {
    registered().into_iter().filter(|r| Path::new(r).is_dir()).map(|r| (basename(&r).to_string(), r)).collect()
}

fn block(state: &str) -> String {
    let repos = repos();
    let on = config::get_global("settings.autoRun", false).is_ok_and(|v| v == "true");
    let overrides: Vec<String> =
        repos.iter().flat_map(|(name, repo)| enrolled(repo).into_iter().filter(|b| auto_run(repo, b) != on).map(move |b| format!("{name} {b}"))).collect();
    let mut lines = vec![age_line(age_of(&format!("{state}/job-pass")), interval()), auto_run_line(on, &overrides)];
    lines.extend(running_lines(&repos));
    lines.extend(attached_lines(&repos));
    lines.extend(limit_line(state));
    lines.extend(failure_lines(&repos));
    lines.push(launchd_line(launchctl_print()));
    lines.push(crate::stamp::status_line());
    lines.push(format!("logs: {state}/launchd.log, {state}/steps.log"));
    lines.iter().map(|l| format!("{l}\n")).collect()
}

/// Seconds since <f> was written, by its mtime; None for no file.
fn age_of(f: &str) -> Option<i64> {
    let written = fs::metadata(f).and_then(|m| m.modified()).ok()?;
    Some(SystemTime::now().duration_since(written).map_or(0, |d| d.as_secs() as i64))
}

fn interval() -> Option<i64> {
    config::get_global("settings.interval", false).ok()?.parse().ok()
}

/// Overdue once two intervals passed with no pass: one can run long.
fn age_line(age: Option<i64>, interval: Option<i64>) -> String {
    let iv = interval.map_or("unreadable".to_string(), |i| format!("{i}s"));
    match age {
        None => format!("job's last pass: none yet (settings.interval {iv})"),
        Some(a) if interval.is_some_and(|i| a > 2 * i) => format!("job's last pass: {} ago, overdue (settings.interval {iv}, two of them with no pass)", dur(a)),
        Some(a) => format!("job's last pass: {} ago (settings.interval {iv})", dur(a)),
    }
}

fn auto_run_line(on: bool, overrides: &[String]) -> String {
    let (global, other) = if on { ("on", "off") } else { ("off", "on") };
    if overrides.is_empty() {
        return format!("autoRun {global}; no branch turns it {other}");
    }
    let n = overrides.len();
    format!("autoRun {global}; {other} on {n} branch{}: {}", if n == 1 { "" } else { "es" }, overrides.join(", "))
}

/// The branch a marker file names: <b>.running, <b> encoded.
fn branch_of(marker: &str) -> String {
    config::decode_branch(basename(marker).strip_suffix(".running").unwrap_or(marker))
}

fn running_lines(repos: &[(String, String)]) -> Vec<String> {
    let mut out = vec![];
    for (name, repo) in repos {
        for l in running::leases(repo).into_iter().filter(|l| l.live) {
            let end = l.deadline.map_or("run by hand".to_string(), |d| format!("deadline {}", time::utc((d / 1_000_000_000) as i64)));
            out.push(format!("running: {name} {} {} (pid {}, since {}, {end})", branch_of(&l.file), l.step, l.pid, l.since));
        }
    }
    if out.is_empty() {
        out.push("running: none".into());
    }
    out
}

/// Each enrolled branch's sd-lock: who holds it and since when (#427).
fn attached_lines(repos: &[(String, String)]) -> Vec<String> {
    let now = time::now_utc().1;
    let mut out = vec![];
    for (name, repo) in repos {
        for b in enrolled(repo) {
            let lock = git_out(repo, &["config", "--get", &format!("branch.{b}.sd-lock")]).unwrap_or_default();
            if lock.is_empty() {
                continue;
            }
            let (who, since) = lock::who_since(&lock);
            let ago = time::epoch_ns(since).map_or(String::new(), |t| format!(", {} ago", dur(((now - t) / 1_000_000_000) as i64)));
            out.push(format!("attached: {name} {b}, {who} since {since}{ago}"));
        }
    }
    out
}

/// The usage limit while it holds (crate::limit's file, read and left).
fn limit_line(state: &str) -> Option<String> {
    let line = fs::read_to_string(format!("{state}/limit")).ok()?;
    let (at, said) = line.trim_end().split_once(' ')?;
    (time::epoch_ns(at)? > time::now_utc().1).then(|| format!("usage limit: till {at}, {said}"))
}

/// Each branch's failure record (#242), holding or not.
fn failure_lines(repos: &[(String, String)]) -> Vec<String> {
    let limit = FAIL_LIMIT as i64;
    let mut out = vec![];
    for (name, repo) in repos {
        for b in enrolled(repo) {
            let Some(rec) = pins::read(repo, &b, Pin::Failed).and_then(|r| config::one_value(&r)) else { continue };
            let count = rec["count"].as_i64().unwrap_or(0);
            let holds = if count >= limit { format!("holding, at {limit}") } else { format!("holds at {limit}") };
            out.push(format!("failures: {name} {b} {} failed {count} in a row ({holds}): {}", t(&rec, "step"), t(&rec, "error")));
        }
    }
    out
}

/// launchctl print's answer for the job: whether it succeeded, and its
/// output; None with no launchctl to run. In a debug build
/// SHEEPDOG_WATCH_LAUNCHCTL names the program, so tests never read the
/// host's job.
fn launchctl_print() -> Option<(bool, String)> {
    let seam = if cfg!(debug_assertions) { std::env::var("SHEEPDOG_WATCH_LAUNCHCTL").ok() } else { None };
    let prog = seam.filter(|p| !p.is_empty()).unwrap_or_else(|| "launchctl".into());
    let o = Command::new(prog).args(["print", &format!("gui/{}/{LABEL}", uid())]).output().ok()?;
    Some((o.status.success(), String::from_utf8_lossy(&o.stdout).into_owned() + &String::from_utf8_lossy(&o.stderr)))
}

/// "<key> = <value>" in launchctl print's output, else "?".
fn field<'a>(text: &'a str, key: &str) -> &'a str {
    text.lines().find_map(|l| l.trim().strip_prefix(key)?.strip_prefix(" = ")).unwrap_or("?")
}

fn launchd_line(answer: Option<(bool, String)>) -> String {
    match answer {
        None => "launchd: not here (no launchctl)".into(),
        Some((false, text)) => {
            let why = text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("");
            format!("launchd: {LABEL} not loaded: {why} (sd watch install)")
        }
        Some((true, text)) => format!(
            "launchd: {LABEL} loaded, state {}, {} runs, last exit {}, every {}; program {}",
            field(&text, "state"),
            field(&text, "runs"),
            field(&text, "last exit code"),
            field(&text, "run interval"),
            field(&text, "program")
        ),
    }
}

/// repo TAB branch TAB glyph TAB why TAB PR URL. why is what was notified
/// and the note; a branch that only waits has neither, so its why is what
/// each NEXT step waits on, in sd state's own words ("CI running", "waiting
/// on review from alice", "waiting on 377-atlas-settings (#413) to merge
/// first"). The URL (empty with no PR) lets a row be pasted as it is.
pub fn row(name: &str, b: &str, glyph: &str, mut why: Vec<String>, s: &Value) -> String {
    let next: &[Value] = at(s, "next").as_array().map_or(&[], Vec::as_slice);
    if !next.is_empty() && next.iter().all(|x| at(x, "who") == "wait") {
        why.extend(next.iter().map(|x| text(at(x, "say"))).filter(|m| !m.is_empty()));
    }
    format!("{name}\t{b}\t{glyph}\t{}\t{}\n", why.join("; "), t(s, "pr.url"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn age_line_is_overdue_past_two_intervals() {
        assert_eq!(age_line(None, Some(300)), "job's last pass: none yet (settings.interval 300s)");
        assert_eq!(age_line(Some(0), Some(300)), "job's last pass: 0s ago (settings.interval 300s)");
        assert_eq!(age_line(Some(600), Some(300)), "job's last pass: 10m ago (settings.interval 300s)");
        assert_eq!(age_line(Some(601), Some(300)), "job's last pass: 10m ago, overdue (settings.interval 300s, two of them with no pass)");
        assert_eq!(age_line(Some(5), None), "job's last pass: 5s ago (settings.interval unreadable)");
    }

    #[test]
    fn auto_run_line_lists_the_branches_that_override_the_global() {
        assert_eq!(auto_run_line(true, &[]), "autoRun on; no branch turns it off");
        assert_eq!(auto_run_line(false, &[]), "autoRun off; no branch turns it on");
        assert_eq!(auto_run_line(true, &["sheepdog a".into()]), "autoRun on; off on 1 branch: sheepdog a");
        assert_eq!(auto_run_line(false, &["r a".into(), "r b".into()]), "autoRun off; on on 2 branches: r a, r b");
    }

    #[test]
    fn branch_of_undoes_the_markers_file_name() {
        assert_eq!(branch_of("/r/.git/sheepdog/feat.running"), "feat");
        assert_eq!(branch_of("/r/.git/sheepdog/cm%2Fx.running"), "cm/x");
        assert_eq!(branch_of("/r/.git/sheepdog/cm__x.running"), "cm__x");
    }

    const LOADED: &str = "gui/501/com.sheepdog.watch = {\n\tactive count = 0\n\tstate = not running\n\n\tprogram = /h/sd\n\truns = 510\n\tlast exit code = 0\n\trun interval = 300 seconds\n}\n";

    #[test]
    fn launchd_line_reads_launchctl_prints_fields_or_says_why_not() {
        assert_eq!(
            launchd_line(Some((true, LOADED.into()))),
            "launchd: com.sheepdog.watch loaded, state not running, 510 runs, last exit 0, every 300 seconds; program /h/sd"
        );
        assert_eq!(launchd_line(Some((true, "x = {\n}\n".into()))), "launchd: com.sheepdog.watch loaded, state ?, ? runs, last exit ?, every ?; program ?");
        assert_eq!(
            launchd_line(Some((false, "Bad request.\nCould not find service \"com.sheepdog.watch\" in domain for user gui: 501\n".into()))),
            "launchd: com.sheepdog.watch not loaded: Could not find service \"com.sheepdog.watch\" in domain for user gui: 501 (sd watch install)"
        );
        assert_eq!(launchd_line(None), "launchd: not here (no launchctl)");
    }

    use serde_json::json;

    fn wait(key: &str, say: &str) -> Value {
        json!({"key": key, "who": "wait", "say": say, "cmd": null})
    }

    fn state(next: Vec<Value>) -> Value {
        json!({"pr": {"number": 7, "url": "https://example/pr/7"}, "next": next})
    }

    #[test]
    fn row_of_a_waiting_branch_says_what_each_step_waits_on_then_the_prs_url() {
        let parent = state(vec![wait("parent-wait", "waiting on 377-atlas-settings (#413) to merge first")]);
        assert_eq!(row("sheepdog", "b", "⏳", vec![], &parent), "sheepdog\tb\t⏳\twaiting on 377-atlas-settings (#413) to merge first\thttps://example/pr/7\n");
        let ci = state(vec![wait("ci-wait", "CI running")]);
        assert_eq!(row("sheepdog", "b", "⏳", vec![], &ci), "sheepdog\tb\t⏳\tCI running\thttps://example/pr/7\n");
        let both = state(vec![wait("ci-wait", "CI running"), wait("bot-wait", "waiting on the review bot (/review-bot)")]);
        assert_eq!(row("sheepdog", "b", "⏳", vec![], &both), "sheepdog\tb\t⏳\tCI running; waiting on the review bot (/review-bot)\thttps://example/pr/7\n");
    }

    #[test]
    fn row_keeps_the_notified_reasons_when_a_step_isnt_a_wait_and_the_url_column_is_empty_with_no_pr() {
        let mixed = state(vec![json!({"key": "ci-red", "who": "you", "say": "CI red", "cmd": null}), wait("ci-wait", "CI running")]);
        assert_eq!(row("r", "b", "❌", vec!["CI red".into()], &mixed), "r\tb\t❌\tCI red\thttps://example/pr/7\n");
        let no_pr = json!({"pr": null, "next": [wait("ci-wait", "CI running")]});
        assert_eq!(row("r", "b", "⏳", vec![], &no_pr), "r\tb\t⏳\tCI running\t\n");
        let unsure = json!({"pr": null, "next": []});
        assert_eq!(row("r", "b", "⚠️", vec!["sd state gave no next steps".into()], &unsure), "r\tb\t⚠️\tsd state gave no next steps\t\n");
    }
}
