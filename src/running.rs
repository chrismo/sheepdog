// The running marker (#170): while an agent step runs on a branch, the
// branch's file says so, one line: "<pid> <step> <start time> <the pid's
// proc_start>". sd state reads it.

use crate::val::t;
use crate::{git, proc, time};
use serde_json::{json, Value};
use std::fs;
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard, Once, PoisonError};

pub fn file(dir: &str, b: &str) -> Option<String> {
    let common = git::common_dir(dir)?;
    Some(format!("{common}/sheepdog/{}.running", b.replace('/', "__")))
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
pub fn state(dir: &str, b: &str) -> Option<Value> {
    Some(state_at(&file(dir, b)?))
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
    eprintln!("{}", refusal(&s, b, marker));
    false
}

fn refusal(s: &Value, b: &str, marker: &str) -> String {
    let (step, pid, since) = (t(s, "step"), t(s, "pid"), t(s, "since"));
    format!(
        "{step} is running on {b} (pid {pid}, since {since}): not starting another agent step. \
         If it's hung: kill {pid}, or remove {marker}"
    )
}

/// The markers this process holds, and the line it wrote to each.
static HELD: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

fn held() -> MutexGuard<'static, Vec<(String, String)>> {
    HELD.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Removes <marker> unless another step has written its own there since.
fn release(marker: &str, line: &str) {
    if fs::read_to_string(marker).is_ok_and(|l| l == line) {
        let _ = fs::remove_file(marker);
    }
}

extern "C" {
    fn atexit(f: extern "C" fn()) -> i32;
}

/// process::exit (act's die) runs no destructors and skips the end of
/// deferring_signals, so at exit the markers are released, then sd ends of
/// a signal it got during the step, not as an ordinary failure (#177).
extern "C" fn at_exit() {
    for (marker, line) in held().drain(..) {
        release(&marker, &line);
    }
    proc::die_of_caught();
}

/// <f> under <b>'s marker, naming <step>; None, and <f> not run, when another
/// live step holds it. A marker holding keeps (RUNNING_HELD) is left
/// alone. A HUP, INT or TERM ends <f>'s agent at once (proc::timeout), and
/// sd dies of it once <f> is done. The marker is removed however sd ends,
/// short of a KILL.
pub fn during<T>(b: &str, step: &str, f: impl FnOnce() -> T) -> Option<T> {
    let Some(marker) = file(".", b) else {
        eprintln!("sd: no running marker for {b}: not in a git repository");
        return None;
    };
    during_at(&marker, b, step, f)
}

/// during, for a step whose commit and verify come after its harness step:
/// RUNNING_HELD=<b> is exported while <f> runs, so the
/// step's harness step and children (verify) run under this marker.
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
    static AT_EXIT: Once = Once::new();
    AT_EXIT.call_once(|| unsafe {
        atexit(at_exit);
    });
    proc::deferring_signals(|| {
        let dir = marker.rsplit_once('/').map_or("", |(d, _)| d);
        if let Err(e) = fs::create_dir_all(dir).and_then(|()| fs::write(marker, &line)) {
            eprintln!("sd: can't write the running marker {marker}: {e}");
            return None;
        }
        held().push((marker.to_string(), line.clone()));
        let out = f();
        held().retain(|(m, l)| !(m == marker && *l == line));
        release(marker, &line);
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

    // Was lib/sheepdog.bats: "running_state: a live marker as JSON; none, or a dead pid's, is null, and the dead one is removed (#170)"
    #[test]
    fn state_a_live_marker_as_json_none_or_a_dead_pids_is_null_and_the_dead_one_is_removed_170() {
        let f = marker("state");
        assert!(state_at(path(&f)).is_null());
        let me = std::process::id().to_string();
        fs::write(&f, format!("{me} implement 2026-10-18T14:03:00Z {}\n", proc_start(&me))).unwrap();
        assert_eq!(
            state_at(path(&f)),
            json!({"step": "implement", "pid": std::process::id(), "since": "2026-10-18T14:03:00Z"})
        );
        let mut dead = Command::new("sh").args(["-c", "exit 0"]).spawn().unwrap();
        dead.wait().unwrap();
        fs::write(&f, format!("{} review 2026-10-18T14:03:00Z {}\n", dead.id(), proc_start(&me))).unwrap();
        assert!(state_at(path(&f)).is_null());
        assert!(!f.exists());
    }

    // Was lib/sheepdog.bats: "running_state: a live pid that started at another time is a reused one: null, and the marker removed (#170)"
    #[test]
    fn state_a_live_pid_that_started_at_another_time_is_a_reused_one_null_and_the_marker_removed_170() {
        let f = marker("reused");
        let me = std::process::id().to_string();
        assert!(!proc_start(&me).is_empty());
        fs::write(&f, format!("{me} implement 2026-10-18T14:03:00Z Thu Jan 1 00:00:00 1970\n")).unwrap();
        assert!(state_at(path(&f)).is_null());
        assert!(!f.exists());
    }

    #[test]
    fn the_refusal_names_the_way_out_177() {
        let s = json!({"step": "address", "pid": 42, "since": "2026-10-22T00:00:00Z"});
        assert_eq!(
            refusal(&s, "feat", "/r/.git/sheepdog/feat.running"),
            "address is running on feat (pid 42, since 2026-10-22T00:00:00Z): not starting another agent step. \
             If it's hung: kill 42, or remove /r/.git/sheepdog/feat.running"
        );
    }

    #[test]
    fn a_marker_holding_keeps_is_left_alone() {
        let f = marker("kept");
        std::env::set_var("RUNNING_HELD", "b-kept");
        let ran = during_at(path(&f), "b-kept", "implement", || f.exists());
        std::env::remove_var("RUNNING_HELD");
        assert_eq!(ran, Some(false));
    }
}
