// sd view's GitHub data (#401): one file per answer under <state>/github/,
// served at once. Once it's FRESH_SECS old, or the page's refresh button made
// everything stale, the request that sees it starts a fetch on a thread and
// answers with what's there; the page asks again while "refreshing".
// A failed fetch keeps the last data beside its error. Nothing prunes these
// files yet: slice 2 of #401 has the watcher drop them with a branch.

use crate::time::utc;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

const FRESH_SECS: u64 = 60;
/// Keys with a fetch running: one at a time each.
static FETCHING: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
/// When the refresh button was last pressed, in ms: a fetch tried before then is stale.
static FORCED: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn fetching() -> MutexGuard<'static, BTreeSet<String>> {
    FETCHING.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Frees its key when the fetch ends, even by a panic, which would
/// otherwise leave the key "refreshing" for the life of the server.
struct Running(String);

impl Drop for Running {
    fn drop(&mut self) {
        fetching().remove(&self.0);
    }
}

pub fn force() {
    FORCED.store(now_ms(), Ordering::Relaxed);
}

/// gh's stderr as "gh: …", with one "gh:"; <what> names the answer that held nothing.
pub fn gh_error(stderr: &str, what: &str) -> String {
    match stderr.trim() {
        "" => format!("gh: no {what} in its answer"),
        e if e.starts_with("gh:") => e.to_string(),
        e => format!("gh: {e}"),
    }
}

/// <key> as a file name: anything but [A-Za-z0-9._-] becomes _.
fn file(state: &Path, key: &str) -> PathBuf {
    let name: String = key.chars().map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '_' }).collect();
    state.join("github").join(format!("{name}.json"))
}

fn write(f: &Path, doc: &Value) {
    let tmp = f.with_extension(format!("tmp{}", std::process::id()));
    let _ = f.parent().map(fs::create_dir_all);
    if fs::write(&tmp, doc.to_string()).and_then(|()| fs::rename(&tmp, f)).is_err() {
        let _ = fs::remove_file(&tmp);
    }
}

/// Starts <fetch> for <key> unless one is running.
fn start<F>(key: &str, f: PathBuf, old: Value, fetch: F)
where
    F: FnOnce() -> Result<Value, String> + Send + 'static,
{
    if !fetching().insert(key.to_string()) {
        return;
    }
    let running = Running(key.to_string());
    std::thread::spawn(move || {
        let _running = running;
        let tried = now_ms();
        let doc = match fetch() {
            Ok(data) => json!({"tried": tried, "at": tried, "data": data, "error": null}),
            Err(e) => json!({"tried": tried, "at": old["at"], "data": old["data"], "error": e}),
        };
        write(&f, &doc);
    });
}

fn stale(tried: u64, now: u64, forced: u64) -> bool {
    now.saturating_sub(tried) >= FRESH_SECS * 1000 || tried < forced
}

/// <key>'s cached answer: {data, error, fetched (UTC, or null), refreshing}.
pub fn get<F>(state: &Path, key: &str, fetch: F) -> Value
where
    F: FnOnce() -> Result<Value, String> + Send + 'static,
{
    let f = file(state, key);
    let doc = fs::read_to_string(&f).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()).unwrap_or(Value::Null);
    let tried = doc["tried"].as_u64().unwrap_or(0);
    let refreshing = stale(tried, now_ms(), FORCED.load(Ordering::Relaxed));
    if refreshing {
        start(key, f, doc.clone(), fetch);
    }
    let fetched = doc["at"].as_u64().map(|ms| utc((ms / 1000) as i64));
    json!({"data": doc["data"], "error": doc["error"], "fetched": fetched, "refreshing": refreshing})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gh_s_error_says_gh_once() {
        assert_eq!(gh_error("gh: rate limit\n", "PR"), "gh: rate limit");
        assert_eq!(gh_error("HTTP 502", "PR"), "gh: HTTP 502");
        assert_eq!(gh_error(" \n", "issue list"), "gh: no issue list in its answer");
    }

    #[test]
    fn an_answer_is_stale_a_minute_after_it_was_tried_or_once_forced_after_it() {
        assert!(!stale(1_000, 60_999, 0));
        assert!(stale(1_000, 61_000, 0));
        assert!(stale(999, 2_000, 1_000));
        assert!(!stale(1_000, 2_000, 1_000), "a refresh in the fetch's own ms");
        assert!(stale(0, 60_000, 0), "never tried");
    }

    #[test]
    fn a_fetch_that_panics_lets_the_next_one_start() {
        let dir = std::env::temp_dir().join(format!("sd-cache-test-{}", std::process::id()));
        let key = "panics";
        get(&dir, key, || panic!("a bug in the fetch"));
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while fetching().contains(key) && std::time::Instant::now() < end {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!fetching().contains(key));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_key_is_one_file_name_under_github() {
        assert_eq!(file(Path::new("/s"), "pr-github.com-o-r-7"), Path::new("/s/github/pr-github.com-o-r-7.json"));
        assert_eq!(file(Path::new("/s"), "../x/y"), Path::new("/s/github/.._x_y.json"));
    }
}
