// sd view's GitHub data (#401): one file per answer under <state>/github/,
// served at once. Once it's FRESH_SECS old, or the page's refresh button made
// everything stale, the request that sees it starts a fetch on a thread and
// answers with what's there; the page asks again while "refreshing".
// A failed fetch keeps the last data beside its error. Every try rewrites its
// file, so one PRUNE_DAYS unwritten is one no page has read since: prune drops it.

use crate::time::utc;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

const FRESH_SECS: u64 = 60;
const PRUNE_DAYS: u64 = 7;
/// Each key's fetch running, one at a time, and when each was last tried: a
/// try whose file couldn't be written still counts, or every read would
/// start another.
struct Keys {
    running: BTreeSet<String>,
    tried: BTreeMap<String, u64>,
}
static KEYS: Mutex<Keys> = Mutex::new(Keys { running: BTreeSet::new(), tried: BTreeMap::new() });
/// When the refresh button was last pressed, in ms: a fetch tried before then is stale.
static FORCED: AtomicU64 = AtomicU64::new(0);

pub(super) fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn keys() -> MutexGuard<'static, Keys> {
    KEYS.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Frees its key when the fetch ends, even by a panic, which would
/// otherwise leave the key "refreshing" for the life of the server.
struct Running(String);

impl Drop for Running {
    fn drop(&mut self) {
        keys().running.remove(&self.0);
    }
}

pub fn force() {
    FORCED.store(now_ms(), Ordering::Relaxed);
}

pub(super) fn forced() -> u64 {
    FORCED.load(Ordering::Relaxed)
}

/// gh's stderr as "gh: …", with one "gh:"; <what> names the answer that held nothing.
pub fn gh_error(stderr: &str, what: &str) -> String {
    match stderr.trim() {
        "" => format!("gh: no {what} in its answer"),
        e if e.starts_with("gh:") => e.to_string(),
        e => format!("gh: {e}"),
    }
}

/// Removes <state>/github/'s files PRUNE_DAYS unwritten, a dead write's temp file included.
pub fn prune(state: &Path) {
    let horizon = std::time::Duration::from_secs(PRUNE_DAYS * 86400);
    for e in fs::read_dir(state.join("github")).into_iter().flatten().flatten() {
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|m| m.elapsed().ok()).is_some_and(|age| age >= horizon);
        if old {
            if let Err(err) = fs::remove_file(e.path()) {
                eprintln!("sd view: {}: {err}", e.path().display());
            }
        }
    }
}

/// <key> as a file name, one per key: bytes but [A-Za-z0-9._-] as %XX.
fn file(state: &Path, key: &str) -> PathBuf {
    let name: String = key
        .bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"._-".contains(&b) { char::from(b).to_string() } else { format!("%{b:02X}") })
        .collect();
    state.join("github").join(format!("{name}.json"))
}

fn write(f: &Path, doc: &Value) {
    let tmp = f.with_extension(format!("tmp{}", std::process::id()));
    let wrote = f.parent().map_or(Ok(()), fs::create_dir_all).and_then(|()| fs::write(&tmp, doc.to_string())).and_then(|()| fs::rename(&tmp, f));
    if let Err(e) = wrote {
        eprintln!("sd view: {}: {e}", f.display());
        let _ = fs::remove_file(&tmp);
    }
}

/// Starts <fetch> for <key> unless one is running.
fn start<F>(key: &str, f: PathBuf, old: Value, fetch: F)
where
    F: FnOnce() -> Result<Value, String> + Send + 'static,
{
    let tried = now_ms();
    {
        let mut k = keys();
        if !k.running.insert(key.to_string()) {
            return;
        }
        k.tried.insert(key.to_string(), tried);
    }
    let running = Running(key.to_string());
    std::thread::spawn(move || {
        let _running = running;
        let doc = match fetch() {
            Ok(data) => json!({"tried": tried, "at": tried, "data": data, "error": null}),
            Err(e) => json!({"tried": tried, "at": old["at"], "data": old["data"], "error": e}),
        };
        write(&f, &doc);
    });
}

pub(super) fn stale(tried: u64, now: u64, forced: u64) -> bool {
    tried > now || now - tried >= FRESH_SECS * 1000 || tried < forced
}

/// <key>'s cached answer: {data, error, fetched (UTC, or null), refreshing}.
pub fn get<F>(state: &Path, key: &str, fetch: F) -> Value
where
    F: FnOnce() -> Result<Value, String> + Send + 'static,
{
    // Before the read: a fetch that ended by now has written its file.
    let (last, running) = {
        let k = keys();
        (k.tried.get(key).copied().unwrap_or(0), k.running.contains(key))
    };
    let f = file(state, key);
    let doc = fs::read_to_string(&f).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()).unwrap_or(Value::Null);
    let now = now_ms();
    // A file's try ahead of the clock that can't be rewritten would start a fetch every read.
    let tried = doc["tried"].as_u64().filter(|&t| t <= now).unwrap_or(0).max(last);
    let refreshing = if stale(tried, now, forced()) {
        start(key, f, doc.clone(), fetch);
        true
    } else {
        running
    };
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
        assert!(stale(5_000, 2_000, 0), "tried in the future: a clock moved, or the state dir was copied");
    }

    #[test]
    fn a_fetch_that_panics_frees_its_key() {
        let dir = std::env::temp_dir().join(format!("sd-cache-test-{}", std::process::id()));
        let key = "panics";
        get(&dir, key, || panic!("a bug in the fetch"));
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while keys().running.contains(key) && std::time::Instant::now() < end {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!keys().running.contains(key));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_key_is_one_file_name_under_github() {
        assert_eq!(file(Path::new("/s"), "issues-r-1a2b"), Path::new("/s/github/issues-r-1a2b.json"));
        assert_eq!(file(Path::new("/s"), "pr/h/a-b/c/7"), Path::new("/s/github/pr%2Fh%2Fa-b%2Fc%2F7.json"));
        assert_ne!(file(Path::new("/s"), "pr/h/a-b/c/7"), file(Path::new("/s"), "pr/h/a/b-c/7"));
        assert_ne!(file(Path::new("/s"), "a/b"), file(Path::new("/s"), "a%2Fb"));
        assert_eq!(file(Path::new("/s"), "../x"), Path::new("/s/github/..%2Fx.json"));
    }

    #[test]
    fn a_try_whose_file_can_t_be_written_still_counts_as_tried() {
        let state = std::env::temp_dir().join(format!("sd-cache-test-unwritable-{}", std::process::id()));
        fs::write(&state, "a file where the state dir should be").unwrap();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let ask = || {
            let c = calls.clone();
            get(&state, "unwritable", move || Ok(json!(c.fetch_add(1, Ordering::SeqCst))))
        };
        assert_eq!(ask()["refreshing"], true);
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while keys().running.contains("unwritable") && std::time::Instant::now() < end {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(ask()["refreshing"], false);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let _ = fs::remove_file(state);
    }

    #[test]
    fn a_future_try_in_a_file_that_can_t_be_rewritten_is_fetched_once() {
        use std::os::unix::fs::PermissionsExt;
        let state = std::env::temp_dir().join(format!("sd-cache-test-future-{}", std::process::id()));
        let f = file(&state, "future");
        fs::create_dir_all(f.parent().unwrap()).unwrap();
        fs::write(&f, json!({"tried": now_ms() + 3_600_000, "at": 1, "data": "old", "error": null}).to_string()).unwrap();
        fs::set_permissions(f.parent().unwrap(), fs::Permissions::from_mode(0o555)).unwrap();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let ask = || {
            let c = calls.clone();
            get(&state, "future", move || Ok(json!(c.fetch_add(1, Ordering::SeqCst))))
        };
        assert_eq!(ask()["refreshing"], true);
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while keys().running.contains("future") && std::time::Instant::now() < end {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let again = ask();
        fs::set_permissions(f.parent().unwrap(), fs::Permissions::from_mode(0o755)).unwrap();
        let _ = fs::remove_dir_all(&state);
        assert_eq!(again["refreshing"], false);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
