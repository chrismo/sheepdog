// The running marker (#170): while an agent step runs on a branch, the
// branch's file says so, one line: "<pid> <step> <start time> <the pid's
// proc_start>". sd state reads it. A step the watcher launched holds it as a
// lease (#268), with a second line, "deadline <time>", past which the
// watcher's reconcile ends it. A KILL, or a signal sd has no handler for,
// leaves the marker behind: the watcher's reconcile and the next sd state
// clean it.

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

/// Where a step the watcher launched writes how it ended (#268).
pub fn outcome_file(dir: &str, b: &str) -> Option<String> {
    file(dir, b)?.strip_suffix(".running").map(|f| format!("{f}.outcome"))
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

/// state, of the marker at <f>. A dead holder's lease is left for the
/// watcher's reconcile to report; unreconciled, the next step overwrites it.
fn state_at(f: &str) -> Value {
    let Some(l) = lease_at(f) else { return Value::Null };
    if l.live {
        let pid: Value = serde_json::from_str(&l.pid).unwrap_or(Value::Null);
        json!({"step": l.step, "pid": pid, "since": l.since})
    } else {
        if l.deadline.is_none() {
            let _ = fs::remove_file(f);
        }
        Value::Null
    }
}

/// A marker as it stands. live: its pid is still the process that wrote it.
pub struct Lease {
    pub pid: String,
    pub step: String,
    pub since: String,
    /// In epoch ns; None for a step not launched by the watcher.
    pub deadline: Option<i128>,
    pub live: bool,
}

/// <b>'s marker, read and left in place.
pub fn lease(dir: &str, b: &str) -> Option<Lease> {
    lease_at(&file(dir, b)?)
}

/// Whether <doing> may go ahead on <b> in the repo at <dir>: not while
/// another process's live step holds its marker (#270). Ours isn't another's:
/// a step's children run with RUNNING_HELD=<b> (holding, leasing), and a
/// marker this process wrote is its own. attach, forget --remove and the
/// verbs that write the worktree or the remote ask.
pub fn idle(dir: &str, b: &str, doing: &str) -> Result<(), String> {
    file(dir, b).map_or(Ok(()), |f| idle_at(&f, b, doing))
}

fn idle_at(f: &str, b: &str, doing: &str) -> Result<(), String> {
    if std::env::var("RUNNING_HELD").is_ok_and(|h| h == b) {
        return Ok(());
    }
    let marker = lease_at(f).filter(|l| l.pid != std::process::id().to_string());
    refusal_for(b, marker.as_ref(), doing)
}

fn refusal_for(b: &str, marker: Option<&Lease>, doing: &str) -> Result<(), String> {
    match marker {
        Some(l) if l.live => Err(format!(
            "{} is running on {b} (pid {}, since {}): not {doing}. Wait for it to end (sd state shows it), or kill {}",
            l.step, l.pid, l.since, l.pid
        )),
        _ => Ok(()),
    }
}

/// Every branch's marker in the repo at <dir>.
pub fn leases(dir: &str) -> Vec<Lease> {
    let Some(common) = git::common_dir(dir) else { return vec![] };
    let files = fs::read_dir(format!("{common}/sheepdog")).into_iter().flatten().flatten();
    files
        .filter(|e| e.file_name().to_string_lossy().ends_with(".running"))
        .filter_map(|e| lease_at(&e.path().to_string_lossy()))
        .collect()
}

fn lease_at(f: &str) -> Option<Lease> {
    let text = fs::read_to_string(f).ok()?;
    let mut lines = text.lines();
    let mut it = lines.next()?.split_whitespace();
    let (pid, step, since) = (it.next().unwrap_or(""), it.next().unwrap_or(""), it.next().unwrap_or(""));
    let start = it.collect::<Vec<_>>().join(" ");
    let live = !pid.is_empty()
        && pid.chars().all(|c| c.is_ascii_digit())
        && Command::new("kill").args(["-0", pid]).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
        && proc_start(pid) == start;
    let deadline = lines.find_map(|l| l.strip_prefix("deadline ")).and_then(time::epoch_ns);
    Some(Lease { pid: pid.into(), step: step.into(), since: since.into(), deadline, live })
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
/// short of a KILL or a signal sd doesn't handle.
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
    during(b, step, || with_held(b, f))
}

/// <f> with RUNNING_HELD=<b>, then RUNNING_HELD as it was, so a hold nested
/// in another leaves the outer one's set (#270).
fn with_held<T>(b: &str, f: impl FnOnce() -> T) -> T {
    let before = std::env::var_os("RUNNING_HELD");
    std::env::set_var("RUNNING_HELD", b);
    let out = f();
    match before {
        Some(v) => std::env::set_var("RUNNING_HELD", v),
        None => std::env::remove_var("RUNNING_HELD"),
    }
    out
}

/// holding, for a step the watcher launched (#268): the marker is a lease
/// that ends <secs> from now.
pub fn leasing<T>(b: &str, step: &str, secs: u64, f: impl FnOnce() -> T) -> Option<T> {
    let Some(marker) = file(".", b) else {
        eprintln!("sd: no running marker for {b}: not in a git repository");
        return None;
    };
    leasing_at(&marker, b, step, secs, f)
}

fn leasing_at<T>(marker: &str, b: &str, step: &str, secs: u64, f: impl FnOnce() -> T) -> Option<T> {
    let end = time::utc((time::now_utc().1 / 1_000_000_000) as i64 + secs as i64);
    marked(marker, b, step, &format!("deadline {end}\n"), || with_held(b, f))
}

/// during, with <b>'s marker at <marker>.
fn during_at<T>(marker: &str, b: &str, step: &str, f: impl FnOnce() -> T) -> Option<T> {
    if std::env::var("RUNNING_HELD").is_ok_and(|h| h == b) {
        return Some(f());
    }
    marked(marker, b, step, "", f)
}

/// <f> under the marker at <marker>, its lines after the first <rest>.
fn marked<T>(marker: &str, b: &str, step: &str, rest: &str, f: impl FnOnce() -> T) -> Option<T> {
    if !free(marker, b) {
        return None;
    }
    let pid = std::process::id().to_string();
    let line = format!("{pid} {step} {} {}\n{rest}", time::now_utc().0, proc_start(&pid));
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
        let _hold = Hold { marker: marker.to_string(), line };
        Some(f())
    })
}

/// Released on drop, so a step that panics lets go of its marker too.
struct Hold {
    marker: String,
    line: String,
}

impl Drop for Hold {
    fn drop(&mut self) {
        held().retain(|(m, l)| !(*m == self.marker && *l == self.line));
        release(&self.marker, &self.line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// Held by the tests that set RUNNING_HELD, which is process-wide.
    static ENV: Mutex<()> = Mutex::new(());

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
    fn a_step_that_panics_releases_its_marker_as_it_unwinds_350() {
        let f = marker("panicked");
        let unwound = std::panic::catch_unwind(|| during_at(path(&f), "b-panicked", "implement", || panic!("the step")));
        assert!(unwound.is_err());
        assert!(!f.exists());
        assert!(!held().iter().any(|(m, _)| m == path(&f)));
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
    fn idle_unless_a_live_step_holds_the_marker_in_the_verbs_own_words_270() {
        let lease = |live| Lease {
            pid: "42".into(),
            step: "address".into(),
            since: "2026-10-22T00:00:00Z".into(),
            deadline: None,
            live,
        };
        assert_eq!(refusal_for("feat", None, "attaching"), Ok(()));
        assert_eq!(refusal_for("feat", Some(&lease(false)), "attaching"), Ok(()));
        assert_eq!(
            refusal_for("feat", Some(&lease(true)), "attaching"),
            Err("address is running on feat (pid 42, since 2026-10-22T00:00:00Z): not attaching. \
                 Wait for it to end (sd state shows it), or kill 42"
                .into())
        );
        assert_eq!(
            refusal_for("feat", Some(&lease(true)), "removing its worktree"),
            Err("address is running on feat (pid 42, since 2026-10-22T00:00:00Z): not removing its worktree. \
                 Wait for it to end (sd state shows it), or kill 42"
                .into())
        );
    }

    #[test]
    fn idle_passes_a_marker_held_by_this_process_or_for_this_branch_by_running_held_270() {
        let _env = ENV.lock().unwrap_or_else(PoisonError::into_inner);
        let f = marker("ours");
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id().to_string();
        fs::write(&f, format!("{pid} address 2026-10-22T00:00:00Z {}\n", proc_start(&pid))).unwrap();
        assert!(idle_at(path(&f), "b-ours", "pushing").is_err());
        std::env::set_var("RUNNING_HELD", "b-other");
        assert!(idle_at(path(&f), "b-ours", "pushing").is_err());
        std::env::set_var("RUNNING_HELD", "b-ours");
        assert_eq!(idle_at(path(&f), "b-ours", "pushing"), Ok(()));
        std::env::remove_var("RUNNING_HELD");
        child.kill().unwrap();
        child.wait().unwrap();

        let me = std::process::id().to_string();
        fs::write(&f, format!("{me} implement 2026-10-22T00:00:00Z {}\n", proc_start(&me))).unwrap();
        assert_eq!(idle_at(path(&f), "b-ours", "pushing"), Ok(()));
    }

    #[test]
    fn leasing_writes_the_deadline_on_a_second_line_and_holds_the_marker_for_the_steps_inside_268() {
        let _env = ENV.lock().unwrap_or_else(PoisonError::into_inner);
        let f = marker("leased");
        let seen = leasing_at(path(&f), "b-leased", "push", 30, || {
            (fs::read_to_string(&f).unwrap(), std::env::var("RUNNING_HELD").ok())
        });
        let (text, held) = seen.expect("the step ran");
        let me = std::process::id().to_string();
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].starts_with(&format!("{me} push ")), "{text}");
        let deadline = lines[1].strip_prefix("deadline ").expect("a deadline line");
        let left = (time::epoch_ns(deadline).unwrap() - time::now_utc().1) / 1_000_000_000;
        assert!((28..=30).contains(&left), "{left}");
        assert_eq!(held.as_deref(), Some("b-leased"));
        assert!(std::env::var("RUNNING_HELD").is_err());
        assert!(!f.exists());
    }

    #[test]
    fn lease_reads_a_marker_and_leaves_it_and_sd_state_leaves_a_dead_holders_lease_for_reconcile_268() {
        let f = marker("lease");
        assert!(lease_at(path(&f)).is_none());
        let mut dead = Command::new("sh").args(["-c", "exit 0"]).spawn().unwrap();
        dead.wait().unwrap();
        fs::write(&f, format!("{} push 2026-10-23T00:00:00Z x\ndeadline 2026-10-23T00:03:00Z\n", dead.id())).unwrap();
        let l = lease_at(path(&f)).unwrap();
        assert_eq!((l.step.as_str(), l.live), ("push", false));
        assert_eq!(l.deadline, time::epoch_ns("2026-10-23T00:03:00Z"));
        assert!(state_at(path(&f)).is_null());
        assert!(f.exists());

        let me = std::process::id().to_string();
        fs::write(&f, format!("{me} implement 2026-10-23T00:00:00Z {}\n", proc_start(&me))).unwrap();
        let l = lease_at(path(&f)).unwrap();
        assert_eq!((l.pid.as_str(), l.live, l.deadline), (me.as_str(), true, None));
    }

    #[test]
    fn with_held_puts_running_held_back_as_it_was_so_a_nested_hold_leaves_the_outer_ones_270() {
        let _env = ENV.lock().unwrap_or_else(PoisonError::into_inner);
        std::env::set_var("RUNNING_HELD", "b-outer");
        let inside = with_held("b-outer", || std::env::var("RUNNING_HELD").ok());
        let after = std::env::var("RUNNING_HELD").ok();
        std::env::remove_var("RUNNING_HELD");
        assert_eq!(inside.as_deref(), Some("b-outer"));
        assert_eq!(after.as_deref(), Some("b-outer"));
        assert_eq!(with_held("b-new", || std::env::var("RUNNING_HELD").ok()).as_deref(), Some("b-new"));
        assert!(std::env::var("RUNNING_HELD").is_err());
    }

    #[test]
    fn a_marker_holding_keeps_is_left_alone() {
        let _env = ENV.lock().unwrap_or_else(PoisonError::into_inner);
        let f = marker("kept");
        std::env::set_var("RUNNING_HELD", "b-kept");
        let ran = during_at(path(&f), "b-kept", "implement", || f.exists());
        std::env::remove_var("RUNNING_HELD");
        assert_eq!(ran, Some(false));
    }
}
