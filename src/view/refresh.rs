// The Flock's branches, kept fresh while a page is open (#401): a read of
// /api/flock starts `sd watch refresh <branch>` in its repo on the stalest
// branch whose cache file is a minute old, or older than the refresh button,
// one branch at a time. A refresh counts as tried when it starts; one that
// fails, for any reason, waits until the watcher rewrites the file it left as
// it was. None starts while the rate limit the freshest file saw is under
// settings.rateLimitFloor, as the pass stops then; a pass that finds the quota
// back writes a file that lifts it.

use super::cache::{forced, now_ms, stale};
use super::flock::Cached;
use crate::config;
use crate::proc;
use crate::time::utc;
use crate::val::{on, t};
use crate::watch::{registered, repo_key, sd_verb};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::UNIX_EPOCH;

const REFRESH_SECS: u64 = 120;

struct Branches {
    running: Option<String>,
    tried: BTreeMap<String, u64>,
    /// Each failed refresh's file, by when it was written.
    failed: BTreeMap<String, Option<u64>>,
}
static BRANCHES: Mutex<Branches> =
    Mutex::new(Branches { running: None, tried: BTreeMap::new(), failed: BTreeMap::new() });

fn branches() -> MutexGuard<'static, Branches> {
    BRANCHES.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Frees the one refresh slot when it ends, even by a panic.
struct Running;

impl Drop for Running {
    fn drop(&mut self) {
        branches().running = None;
    }
}

/// When the watcher last wrote <c>'s cache file, in ms.
fn written(state: &Path, c: &Cached) -> Option<u64> {
    let m = fs::metadata(state.join("branches").join(format!("{}.json", c.id))).and_then(|m| m.modified()).ok()?;
    m.duration_since(UNIX_EPOCH).ok().map(|d| d.as_millis() as u64)
}

pub fn checked(state: &Path, c: &Cached) -> Option<String> {
    written(state, c).map(|ms| utc((ms / 1000) as i64))
}

/// Why no refresh may start for GitHub's rate limit, as the pass says it.
pub fn held(state: &Path, cached: &[Cached]) -> Option<String> {
    let now = now_ms();
    let freshest = cached.iter().filter_map(|c| Some((written(state, c).filter(|&w| w <= now)?, c.doc.as_ref()?)));
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
            let w = written(state, c);
            if b.failed.get(&c.id) == Some(&w) {
                return None;
            }
            // A file written ahead of the clock would be stale on every read.
            let tried = w.filter(|&w| w <= now).unwrap_or(0).max(b.tried.get(&c.id).copied().unwrap_or(0));
            stale(tried, now, forced).then_some((tried, c, repo, w))
        })
        .min_by_key(|(tried, ..)| *tried);
    let Some((_, c, repo, w)) = stalest else { return false };
    b.running = Some(c.id.clone());
    b.tried.insert(c.id.clone(), now);
    let (id, repo, branch) = (c.id.clone(), repo.clone(), c.branch.clone());
    std::thread::spawn(move || {
        let _running = Running;
        let (prog, verb) = sd_verb("watch", "SHEEPDOG_WATCH_PROGRAM");
        let out = proc::with_timeout(REFRESH_SECS, &prog, &[&verb[..], &["refresh", &branch]].concat(), Some(&repo));
        if out.ok {
            branches().failed.remove(&id);
        } else {
            // Its fetch failure is on stdout.
            let why = out.stderr.lines().chain(out.stdout.lines()).next().unwrap_or("failed").to_string();
            eprintln!("sd view: sd watch refresh {branch} in {repo}: {why}");
            branches().failed.insert(id, w);
        }
    });
    true
}
