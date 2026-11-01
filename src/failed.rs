// The failure record (#242): refs/sheepdog/<b>/failed, one line of JSON,
// {step, harness, count, since, at, error}. An agent step whose harness ran
// and failed, or returned nothing usable, counts one more; one that succeeds
// removes it (harness::run_step). One the harness's usage limit stopped does
// neither (#269), nor does one sd ended because sd got a HUP, INT or TERM
// (#270): that's the sender's to count. sd watch counts, harness sd, a lease
// reconcile finds past its deadline (and ends) or crashed, and sd watch
// step's timeout; ending one on a merged or closed PR isn't counted.

use crate::pins::{self, Pin};
use crate::{json, time};
use serde_json::{json, Value};

/// <prev>'s since is kept: when the run of failures began.
fn counted(prev: Option<&Value>, step: &str, harness: &str, now: &str, error: &str) -> Value {
    let count = prev.and_then(|p| p["count"].as_i64()).unwrap_or(0) + 1;
    let since = prev.and_then(|p| p["since"].as_str()).unwrap_or(now);
    json!({"step": step, "harness": harness, "count": count, "since": since, "at": now, "error": error})
}

/// The failure's one line: a timeout's own line; else the last line the
/// harness said (claude's result), else its stderr's; else its exit status.
pub fn why(timed_out: bool, said: &str, stderr: &str, status: i32) -> String {
    let last = |s: &str| s.lines().rev().map(str::trim).find(|l| !l.is_empty()).map(str::to_string);
    let line = if timed_out { last(stderr) } else { last(said).or_else(|| last(stderr)) };
    let line = line.unwrap_or_else(|| format!("exit {status}"));
    line.chars().take(200).collect()
}

/// Count a failure of <step> on <b>; a failure to record it only warns.
pub fn record(b: &str, step: &str, harness: &str, error: &str) {
    record_in(".", b, step, harness, error);
}

/// record, in the repo at <dir>.
pub fn record_in(dir: &str, b: &str, step: &str, harness: &str, error: &str) {
    let prev = pins::read(dir, b, Pin::Failed).and_then(|p| serde_json::from_str::<Value>(&p).ok()).filter(Value::is_object);
    let rec = json::to_string(&counted(prev.as_ref(), step, harness, &time::now_utc().0, error)) + "\n";
    if !pins::store(dir, b, Pin::Failed, &rec, None).ok {
        eprintln!("sd: warning: couldn't record the failure on {b}, so NEXT won't count it");
    }
}

pub fn clear(b: &str) {
    pins::delete(".", b, Pin::Failed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counted_adds_one_and_keeps_the_first_failures_time() {
        let first = counted(None, "implement", "pi", "T0", "no quota");
        assert_eq!(first, json!({"step": "implement", "harness": "pi", "count": 1, "since": "T0", "at": "T0", "error": "no quota"}));
        let second = counted(Some(&first), "review", "claude", "T1", "boom");
        assert_eq!(second, json!({"step": "review", "harness": "claude", "count": 2, "since": "T0", "at": "T1", "error": "boom"}));
    }

    #[test]
    fn why_is_the_timeout_else_what_the_harness_said_else_its_stderr_else_the_status() {
        assert_eq!(why(true, "partial", "noise\ntimed out after 30s\n", 124), "timed out after 30s");
        assert_eq!(why(false, "API Error: 529\n\n", "noise\n", 1), "API Error: 529");
        assert_eq!(why(false, " \n", "first\n  pi: no quota  \n\n", 1), "pi: no quota");
        assert_eq!(why(false, "", "", 2), "exit 2");
        assert_eq!(why(false, &"x".repeat(300), "", 1).len(), 200);
    }
}
