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
    let f = file(b)?;
    let Ok(text) = fs::read_to_string(&f) else { return Some(Value::Null) };
    if text.is_empty() {
        return Some(Value::Null);
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
        Some(json!({"step": step, "pid": pid, "since": since}))
    } else {
        let _ = fs::remove_file(&f);
        Some(Value::Null)
    }
}

/// Fails, saying so, while another live process's step holds <b>'s marker,
/// so a second pass doesn't start a second agent on the worktree, or take the
/// first one's marker. Not a lock: two steps starting at once can both pass.
fn free(b: &str) -> bool {
    let Some(s) = state(b) else { return false };
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
    if std::env::var("RUNNING_HELD").is_ok_and(|h| h == b) {
        return Some(f());
    }
    if !free(b) {
        return None;
    }
    let marker = file(b)?;
    let pid = std::process::id().to_string();
    let line = format!("{pid} {step} {} {}\n", time::now_utc().0, proc_start(&pid));
    proc::deferring_signals(|| {
        let dir = marker.rsplit_once('/').map_or("", |(d, _)| d);
        fs::create_dir_all(dir).and_then(|()| fs::write(&marker, &line)).ok()?;
        let out = f();
        if fs::read_to_string(&marker).is_ok_and(|l| l == line) {
            let _ = fs::remove_file(&marker);
        }
        Some(out)
    })
}
