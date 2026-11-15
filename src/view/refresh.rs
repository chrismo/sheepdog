// The Flock's branches, kept fresh while a page is open (#401): a read of
// /api/flock starts `sd watch refresh <branch>` in its repo on the stalest
// branch whose cache file is a minute old, or older than the refresh button,
// one branch at a time. A refresh counts as tried when it starts; one that
// fails, for any reason, waits until the watcher rewrites the file it left as
// it was. None starts while the rate limit the freshest file saw is under
// settings.rateLimitFloor, as the pass stops then; a pass that finds the quota
// back writes a file that lifts it. A branch whose step (its running marker)
// was seen to end, and whose file is from before the step or says it runs,
// is checked first, a minute old or not (#503).

use super::cache::{forced, now_ms, stale};
use super::flock::Cached;
use crate::config;
use crate::proc;
use crate::time::utc;
use crate::val::{on, t};
use crate::watch::{registered, repo_key, sd_verb};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, UNIX_EPOCH};

const REFRESH_SECS: u64 = 120;

struct Branches {
    running: Option<String>,
    tried: BTreeMap<String, u64>,
    /// Each failed refresh's file, by when it was written.
    failed: BTreeMap<String, Option<u64>>,
    /// The branches a live step held at the last read, with when it started, in ms.
    stepping: BTreeMap<String, u64>,
    /// When each branch's step that was seen to end had started, in ms.
    ended: BTreeMap<String, u64>,
}
static BRANCHES: Mutex<Branches> = Mutex::new(Branches {
    running: None,
    tried: BTreeMap::new(),
    failed: BTreeMap::new(),
    stepping: BTreeMap::new(),
    ended: BTreeMap::new(),
});

/// The branches a live step holds now, with when it started (its marker's
/// since, in ms): one held at the last read and not now has ended its step,
/// unless it's in <own>, the page's own step (act.rs), whose refresh::now
/// checks it after.
pub fn stepping(now: BTreeMap<String, u64>, own: &BTreeSet<String>) {
    let mut b = branches();
    let gone: Vec<_> = b.stepping.iter().filter(|(id, _)| !now.contains_key(*id) && !own.contains(*id)).map(|(id, &at)| (id.clone(), at)).collect();
    b.ended.extend(gone);
    // A step running again: its file is the new step's, not the last one's end.
    b.ended.retain(|id, _| !now.contains_key(id));
    b.stepping = now;
}

/// Whether a branch's file, tried at <tried> ms, is from before the end of
/// the step that started at <started> ms and has ended: the pass's from
/// before the launch (the marker's since is to the second), or one written
/// while the step ran, whose NEXT says running. When the page saw the step
/// end doesn't say: it may have been closed then.
fn before_end(started: Option<u64>, tried: u64, doc: Option<&Value>) -> bool {
    let Some(started) = started else { return false };
    tried < started + 1000 || doc.is_some_and(|d| d["state"]["next"][0]["key"] == "running")
}

fn branches() -> MutexGuard<'static, Branches> {
    BRANCHES.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The refresh slot while no refresh runs; none starts while it's held.
pub fn idle() -> Option<impl Sized> {
    let b = branches();
    b.running.is_none().then_some(b)
}

/// Frees the one refresh slot when it ends, even by a panic.
struct Running;

impl Drop for Running {
    fn drop(&mut self) {
        branches().running = None;
    }
}

/// When the watcher last wrote the cache file of the branch <id>, in ms.
fn written(state: &Path, id: &str) -> Option<u64> {
    let m = fs::metadata(state.join("branches").join(format!("{id}.json"))).and_then(|m| m.modified()).ok()?;
    m.duration_since(UNIX_EPOCH).ok().map(|d| d.as_millis() as u64)
}

pub fn checked(state: &Path, c: &Cached) -> Option<String> {
    written(state, &c.id).map(|ms| utc((ms / 1000) as i64))
}

/// Why no refresh may start for GitHub's rate limit, as the pass says it.
pub fn held(state: &Path, cached: &[Cached]) -> Option<String> {
    let now = now_ms();
    let freshest = cached.iter().filter_map(|c| Some((written(state, &c.id).filter(|&w| w <= now)?, c.doc.as_ref()?)));
    let s = &freshest.max_by_key(|(w, _)| *w)?.1["state"];
    let floor: f64 = config::get_global("settings.rateLimitFloor", false).ok().and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let left = t(s, "rate_limit.remaining");
    if on(s, "rate_limit.limited") {
        Some("GitHub refused a call for the rate limit: no branch is checked until the watcher's pass finds the quota back".into())
    } else if left.parse::<f64>().is_ok_and(|n| n < floor) {
        Some(format!("GitHub rate limit: {left} points left, under settings.rateLimitFloor ({floor}): no branch is checked until the watcher's pass finds the quota back"))
    } else {
        None
    }
}

fn done(c: &Cached) -> bool {
    c.doc.as_ref().is_some_and(|d| matches!(t(&d["state"], "pr.state").as_str(), "MERGED" | "CLOSED"))
}

pub fn running() -> bool {
    branches().running.is_some()
}

/// Starts the stalest branch's refresh unless one is running; whether one is.
pub fn refreshing(state: &Path, cached: &[Cached]) -> bool {
    let repos: BTreeMap<String, String> = registered().into_iter().map(|r| (repo_key(&r), r)).collect();
    let (now, forced) = (now_ms(), forced());
    let mut b = branches();
    if b.running.is_some() {
        return true;
    }
    let stalest = cached
        .iter()
        .filter(|c| !done(c))
        .filter_map(|c| {
            let repo = repos.get(c.id.split_once('/')?.0)?;
            let w = written(state, &c.id);
            if b.failed.get(&c.id) == Some(&w) {
                return None;
            }
            // A file written ahead of the clock would be stale on every read.
            let tried = w.filter(|&w| w <= now).unwrap_or(0).max(b.tried.get(&c.id).copied().unwrap_or(0));
            let before_end = before_end(b.ended.get(&c.id).copied(), tried, c.doc.as_ref());
            (before_end || stale(tried, now, forced)).then_some(((!before_end, tried), c, repo, w))
        })
        .min_by_key(|(order, ..)| *order);
    let Some((_, c, repo, w)) = stalest else { return false };
    b.running = Some(c.id.clone());
    b.tried.insert(c.id.clone(), now);
    b.ended.remove(&c.id);
    let (id, repo, branch) = (c.id.clone(), repo.clone(), c.branch.clone());
    std::thread::spawn(move || run(&id, &repo, &branch, w));
    true
}

/// <branch>'s refresh in <repo>, holding the slot; <w> is its file's time
/// before, which a failure waits to see change.
fn run(id: &str, repo: &str, branch: &str, w: Option<u64>) {
    let _running = Running;
    let (prog, verb) = sd_verb("watch", "SHEEPDOG_WATCH_PROGRAM");
    let out = proc::with_timeout(REFRESH_SECS, &prog, &[&verb[..], &["refresh", branch]].concat(), Some(repo));
    if out.ok {
        branches().failed.remove(id);
    } else {
        // Its fetch failure is on stdout.
        let why = out.stderr.lines().chain(out.stdout.lines()).next().unwrap_or("failed").to_string();
        eprintln!("sd view: sd watch refresh {branch} in {repo}: {why}");
        branches().failed.insert(id.to_string(), w);
    }
}

/// <branch>'s refresh now, on this thread, once the slot is free: after a
/// step the page ran on it (act.rs, #396), so the page sees the result.
pub fn now(state: &Path, id: &str, repo: &str, branch: &str) {
    loop {
        let mut b = branches();
        if b.running.is_none() {
            b.running = Some(id.to_string());
            b.tried.insert(id.to_string(), now_ms());
            // This is the check after the step: no second one for its end.
            b.stepping.remove(id);
            b.ended.remove(id);
            break;
        }
        drop(b);
        std::thread::sleep(Duration::from_millis(100));
    }
    run(id, repo, branch, written(state, id));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_step_s_end_is_noted_with_its_start_unless_it_was_the_page_s_own_503() {
        let set = |ids: &[&str]| ids.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>();
        let started = |ids: &[&str]| ids.iter().map(|s| (s.to_string(), 5_000)).collect::<BTreeMap<_, _>>();
        stepping(started(&["r/loop-503", "r/own-503"]), &set(&[]));
        stepping(started(&[]), &set(&["r/own-503"]));
        let b = branches();
        assert_eq!(b.ended.get("r/loop-503"), Some(&5_000));
        assert!(!b.ended.contains_key("r/own-503"));
    }

    #[test]
    fn a_file_from_before_the_step_or_during_it_is_from_before_its_end_503() {
        let (lead, running) = (json!({"state": {"next": [{"key": "review"}]}}), json!({"state": {"next": [{"key": "running"}]}}));
        // The marker's start is to the second: a file in that second may be the pass's before it.
        assert!(before_end(Some(5_000), 5_999, Some(&lead)), "the pass's file before the launch");
        assert!(before_end(Some(5_000), 60_000, Some(&running)), "a pass's file while the step ran");
        assert!(!before_end(Some(5_000), 60_000, Some(&lead)), "a file from after the step");
        assert!(!before_end(None, 0, Some(&running)), "no step seen to end");
    }
}
