// The running marker (#170): while an agent step runs on a branch, the
// branch's file says so, one line: "<pid> <step> <start time> <the pid's
// proc_start>". sd state reads it. lib/sheepdog.sh's running_hold writes the
// same marker for bash sd act's steps, with its own $BASHPID, and exports
// RUNNING_HELD=<branch> so the steps it runs leave it alone.

use crate::val::t;
use crate::{git, proc, time};
use serde_json::{json, Value};
use std::fs;
use std::process::{Command, Stdio};

pub fn file(b: &str) -> Option<String> {
    let dir = git::common_dir()?;
    Some(format!("{dir}/sheepdog/{}.running", b.replace('/', "__")))
}

/// When <pid> started, by ps, else nothing. It tells a marker's process from
/// a later one given the same pid.
pub fn proc_start(pid: &str) -> String {
    let o = Command::new("ps")
        .args(["-o", "lstart=", "-p", pid])
        .env("LC_ALL", "C")
        .env("TZ", "UTC")
        .stderr(Stdio::null())
        .output();
    o.map(|o| String::from_utf8_lossy(&o.stdout).split_whitespace().collect::<Vec<_>>().join(" "))
        .unwrap_or_default()
}

/// The agent step running on <b>, {step, pid, since}, else null. A marker
/// whose pid is gone, or now another process's, is removed.
pub fn state(b: &str) -> Option<Value> {
    Some(state_at(&file(b)?))
}

/// state, of the marker at <f>.
fn state_at(f: &str) -> Value {
    let Ok(text) = fs::read_to_string(f) else { return Value::Null };
    if text.is_empty() {
        return Value::Null;
    }
    let line = text.lines().next().unwrap_or("");
    let mut it = line.split_whitespace();
    let (pid, step, since) = (it.next().unwrap_or(""), it.next().unwrap_or(""), it.next().unwrap_or(""));
    let start = it.collect::<Vec<_>>().join(" ");
    let live = !pid.is_empty()
        && pid.chars().all(|c| c.is_ascii_digit())
        && Command::new("kill").args(["-0", pid]).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
        && proc_start(pid) == start;
    if live {
        let pid: Value = serde_json::from_str(pid).unwrap_or(Value::Null);
        json!({"step": step, "pid": pid, "since": since})
    } else {
        let _ = fs::remove_file(f);
        Value::Null
    }
}

/// Fails, saying so, while another live process's step holds <b>'s marker,
/// so a second pass doesn't start a second agent on the worktree, or take the
/// first one's marker. Not a lock: two steps starting at once can both pass.
fn free(marker: &str, b: &str) -> bool {
    let s = state_at(marker);
    if s.is_null() || s["pid"] == json!(std::process::id()) {
        return true;
    }
    let (step, pid, since) = (t(&s, "step"), t(&s, "pid"), t(&s, "since"));
    eprintln!("{step} is running on {b} (pid {pid}, since {since}): not starting another agent step");
    false
}

/// <f> under <b>'s marker, naming <step>; None, and <f> not run, when another
/// live step holds it. A marker running_hold keeps (RUNNING_HELD) is left
/// alone. A HUP, INT or TERM ends <f>'s agent at once (proc::timeout), but
/// the marker is removed before sd dies of it.
pub fn during<T>(b: &str, step: &str, f: impl FnOnce() -> T) -> Option<T> {
    let Some(marker) = file(b) else {
        eprintln!("sd: no running marker for {b}: not in a git repository");
        return None;
    };
    during_at(&marker, b, step, f)
}

/// during, for a step whose commit and verify come after its harness step,
/// as running_hold was: RUNNING_HELD=<b> is exported while <f> runs, so the
/// step's harness step and children (verify) run under this marker. A step
/// that dies takes its pid with it, and sd state clears the marker.
pub fn holding<T>(b: &str, step: &str, f: impl FnOnce() -> T) -> Option<T> {
    during(b, step, || {
        std::env::set_var("RUNNING_HELD", b);
        let out = f();
        std::env::remove_var("RUNNING_HELD");
        out
    })
}

/// during, with <b>'s marker at <marker>.
fn during_at<T>(marker: &str, b: &str, step: &str, f: impl FnOnce() -> T) -> Option<T> {
    if std::env::var("RUNNING_HELD").is_ok_and(|h| h == b) {
        return Some(f());
    }
    if !free(marker, b) {
        return None;
    }
    let pid = std::process::id().to_string();
    let line = format!("{pid} {step} {} {}\n", time::now_utc().0, proc_start(&pid));
    proc::deferring_signals(|| {
        let dir = marker.rsplit_once('/').map_or("", |(d, _)| d);
        if let Err(e) = fs::create_dir_all(dir).and_then(|()| fs::write(marker, &line)) {
            eprintln!("sd: can't write the running marker {marker}: {e}");
            return None;
        }
        let out = f();
        if fs::read_to_string(marker).is_ok_and(|l| l == line) {
            let _ = fs::remove_file(marker);
        }
        Some(out)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn marker(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sd-running-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let f = dir.join(format!("{name}.running"));
        let _ = fs::remove_file(&f);
        f
    }

    fn path(f: &Path) -> &str {
        f.to_str().unwrap()
    }

    #[test]
    fn during_holds_the_marker_while_the_step_runs_and_removes_it_after() {
        let f = marker("held");
        let seen = during_at(path(&f), "b-held", "implement", || fs::read_to_string(&f).unwrap());
        let line = seen.expect("the step ran");
        let me = std::process::id().to_string();
        assert!(line.starts_with(&format!("{me} implement ")), "{line}");
        assert!(line.ends_with(&format!(" {}\n", proc_start(&me))), "{line}");
        assert!(!f.exists());
    }

    #[test]
    fn during_leaves_a_marker_another_step_wrote_meanwhile() {
        let f = marker("replaced");
        during_at(path(&f), "b-replaced", "review", || fs::write(&f, "1 other x y\n").unwrap()).unwrap();
        assert_eq!(fs::read_to_string(&f).unwrap(), "1 other x y\n");
    }

    #[test]
    fn a_live_holder_refuses_a_second_step_and_a_dead_ones_marker_is_cleared() {
        let f = marker("live");
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id().to_string();
        fs::write(&f, format!("{pid} address 2026-10-22T00:00:00Z {}\n", proc_start(&pid))).unwrap();
        assert_eq!(state_at(path(&f))["step"], "address");
        assert!(during_at(path(&f), "b-live", "implement", || ()).is_none());
        assert!(f.exists());

        child.kill().unwrap();
        child.wait().unwrap();
        assert!(state_at(path(&f)).is_null());
        assert!(!f.exists());
        assert!(during_at(path(&f), "b-live", "implement", || ()).is_some());
    }

    #[test]
    fn a_marker_running_hold_keeps_is_left_alone() {
        let f = marker("kept");
        std::env::set_var("RUNNING_HELD", "b-kept");
        let ran = during_at(path(&f), "b-kept", "implement", || f.exists());
        std::env::remove_var("RUNNING_HELD");
        assert_eq!(ran, Some(false));
    }
}
