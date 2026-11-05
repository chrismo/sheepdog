// The log store (#467): a command's whole output, one file per run, at
// $SHEEPDOG_STATE_DIR/logs/runs/<repo_key>/<branch>/<utc>-<sha12>-<kind>[-<step>].log
// (<repo_key> of the main worktree, as the watcher's branches/ has it; no
// step when it's the kind). Dirs 0700, files 0600: the output may hold
// secrets. Written as it comes. The first line says what ran and the last
// how it ended; a log without one is still running, or its sd was killed.
// Pruned by settings.log* (sd config --help) after each run, by sd act
// forget, and by the watcher's pass; never a log a verify-red record names.

use crate::{config, git, time, verify_red};
use std::collections::HashSet;
use std::fs;
use std::io::{self, BufRead, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keep {
    All,
    Failed,
    None,
}

#[derive(Clone, Copy, Debug)]
pub struct Policy {
    pub keep: Keep,
    pub runs: usize,
    pub file_bytes: u64,
    pub age_secs: u64,
    pub total_bytes: u64,
}

impl Policy {
    /// From settings.log*, each key's text read by <get>.
    pub fn from(get: impl Fn(&str) -> String) -> Policy {
        let n = |k: &str| get(&format!("settings.{k}")).parse::<f64>().ok().filter(|f| *f >= 0.0).map(|f| f as u64);
        let keep = match get("settings.logKeep").as_str() {
            "all" => Keep::All,
            "none" => Keep::None,
            _ => Keep::Failed,
        };
        Policy {
            keep,
            runs: n("logRuns").unwrap_or(5) as usize,
            file_bytes: n("logFileBytes").unwrap_or(8 << 20),
            age_secs: n("logAge").unwrap_or(14 * 86400),
            total_bytes: n("logBytes").unwrap_or(1 << 30),
        }
    }
}

pub fn root() -> PathBuf {
    Path::new(&config::state_dir()).join("logs/runs")
}

/// <repo>'s dir: <repo> is its main worktree's path.
fn repo_dir(root: &Path, repo: &str) -> PathBuf {
    root.join(crate::watch::repo_key(repo))
}

pub fn branch_dir(root: &Path, repo: &str, b: &str) -> PathBuf {
    repo_dir(root, repo).join(b)
}

/// The file in each repo's dir naming the repo, so a prune can read its
/// records.
const REPO_FILE: &str = ".repo";

fn private_dir(d: &Path) -> io::Result<()> {
    fs::DirBuilder::new().recursive(true).mode(0o700).create(d)
}

pub struct Run {
    pub path: PathBuf,
    file: fs::File,
    /// Bytes the head may still take; past it, output is held.
    room: Option<u64>,
    held: Vec<u8>,
    keep_end: usize,
    dropped: u64,
    last: u8,
    started: Instant,
}

/// "2026-11-05T14:03:00Z" and <ns> as "20261105T140300.123Z": a branch's
/// logs sort in the order they started.
fn compact(at: &str, ns: i128) -> String {
    let secs: String = at.trim_end_matches('Z').chars().filter(|c| *c != '-' && *c != ':').collect();
    format!("{secs}.{:03}Z", ns / 1_000_000 % 1000)
}

impl Run {
    /// A new log in <dir> for <kind> (verify) run by <step> on <b> at <sha>,
    /// its first line written: <file_bytes> caps it, 0 for none.
    pub fn start(dir: &Path, b: &str, sha: &str, kind: &str, step: &str, cmd: &str, file_bytes: u64) -> io::Result<Run> {
        private_dir(dir)?;
        let (at, ns) = time::now_utc_ns();
        let sha12 = &sha[..sha.len().min(12)];
        let what = if step == kind { kind.to_string() } else { format!("{kind}-{step}") };
        let stem = format!("{}-{sha12}-{what}", compact(&at, ns));
        let (path, mut file) = (0..100)
            .find_map(|n| {
                let name = if n == 0 { format!("{stem}.log") } else { format!("{stem}.{n}.log") };
                let p = dir.join(name);
                match fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&p) {
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => None,
                    o => Some(o.map(|f| (p, f))),
                }
            })
            .unwrap_or_else(|| Err(io::Error::other(format!("{stem}.log and 99 more are taken"))))?;
        writeln!(file, "==> {at} {b} {sha12} {kind} ({step}) pid {}: {cmd}", std::process::id())?;
        let keep_end = (file_bytes / 4) as usize;
        let room = (file_bytes > 0).then(|| file_bytes - keep_end as u64);
        Ok(Run { path, file, room, held: Vec::new(), keep_end, dropped: 0, last: b'\n', started: Instant::now() })
    }

    pub fn write(&mut self, chunk: &[u8]) {
        let n = self.room.map_or(chunk.len(), |r| chunk.len().min(r as usize));
        if n > 0 {
            let _ = self.file.write_all(&chunk[..n]);
            self.last = chunk[n - 1];
            if let Some(r) = &mut self.room {
                *r -= n as u64;
            }
        }
        self.held.extend_from_slice(&chunk[n..]);
        if self.held.len() > 2 * self.keep_end.max(4096) {
            self.drop_to_end();
        }
    }

    fn drop_to_end(&mut self) {
        let cut = self.held.len().saturating_sub(self.keep_end);
        self.dropped += cut as u64;
        self.held.drain(..cut);
    }

    /// The last line: "<== <how> after <time>"; <how> is "exit <n>" for a
    /// command that exited, as the prune reads it.
    pub fn finish(mut self, how: &str) -> PathBuf {
        self.drop_to_end();
        let mut tail = Vec::new();
        if self.dropped > 0 {
            if self.last != b'\n' {
                tail.push(b'\n');
            }
            tail.extend(format!("[… {} bytes not kept …]\n", self.dropped).bytes());
        }
        tail.extend_from_slice(&self.held);
        let last = tail.last().copied().unwrap_or(self.last);
        if last != b'\n' {
            tail.push(b'\n');
        }
        let secs = self.started.elapsed().as_secs();
        tail.extend(format!("<== {how} after {}\n", time::took(secs)).bytes());
        let _ = self.file.write_all(&tail);
        self.path
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum End {
    Running,
    Passed,
    Failed,
}

fn head(f: &Path) -> Option<(u32, String)> {
    let mut first = Vec::new();
    io::BufReader::new(fs::File::open(f).ok()?).read_until(b'\n', &mut first).ok()?;
    let first = String::from_utf8_lossy(&first);
    let since = first.split(' ').nth(1)?.to_string();
    let pid = first.split(" pid ").nth(1)?.split(':').next()?.parse().ok()?;
    Some((pid, since))
}

fn alive(pid: u32) -> bool {
    // SAFETY: kill with signal 0 only asks whether the pid exists.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

fn end_of(f: &Path) -> End {
    let Ok(mut file) = fs::File::open(f) else { return End::Running };
    let len = file.metadata().map_or(0, |m| m.len());
    let mut buf = Vec::new();
    if file.seek(SeekFrom::Start(len.saturating_sub(512))).is_err() || file.read_to_end(&mut buf).is_err() {
        return End::Running;
    }
    let text = String::from_utf8_lossy(&buf);
    match text.trim_end_matches('\n').rsplit('\n').next() {
        Some(l) if l.starts_with("<== exit 0 after ") => End::Passed,
        Some(l) if l.starts_with("<== ") => End::Failed,
        _ if head(f).is_some_and(|(pid, _)| alive(pid)) => End::Running,
        _ => End::Failed,
    }
}

/// The *.log files directly in <dir>, oldest first by name.
fn logs_in(dir: &Path) -> Vec<PathBuf> {
    let mut l: Vec<PathBuf> = fs::read_dir(dir)
        .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "log")).collect())
        .unwrap_or_default();
    l.sort();
    l
}

fn logs_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(d) = fs::read_dir(dir) else { return };
    for e in d.flatten() {
        let p = e.path();
        if p.is_dir() {
            logs_under(&p, out);
        } else if p.extension().is_some_and(|x| x == "log") {
            out.push(p);
        }
    }
}

/// <dir>'s branch logs pruned by <p>'s keep and runs, newest first: those
/// in <named> and those running stay, and count.
fn prune_branch(dir: &Path, p: &Policy, named: &HashSet<PathBuf>) {
    let (mut kept, mut a_pass) = (0, false);
    for f in logs_in(dir).into_iter().rev() {
        let end = end_of(&f);
        let extra_pass = end == End::Passed && p.keep == Keep::Failed && a_pass;
        a_pass |= end == End::Passed;
        let stays = named.contains(&canonical(&f)) || end == End::Running;
        if !stays && (p.keep == Keep::None || extra_pass || (p.runs > 0 && kept >= p.runs)) {
            let _ = fs::remove_file(&f);
        } else {
            kept += 1;
        }
    }
}

/// Every repo's logs pruned by <p>'s age and total, oldest first; those in
/// <named> stay, and a running one stays under the total. Then a repo dir
/// with no logs left goes, and every empty dir.
fn prune_all(root: &Path, p: &Policy, named: &HashSet<PathBuf>, now: SystemTime) {
    let mut all = Vec::new();
    logs_under(root, &mut all);
    let mut files: Vec<(PathBuf, u64, SystemTime)> = all
        .into_iter()
        .filter_map(|f| fs::metadata(&f).ok().map(|m| (f, m.len(), m.modified().unwrap_or(now))))
        .collect();
    files.sort_by(|a, b| a.2.cmp(&b.2).then_with(|| a.0.cmp(&b.0)));
    let old = |t: SystemTime| p.age_secs > 0 && now.duration_since(t).is_ok_and(|d| d.as_secs() >= p.age_secs);
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    for (f, len, at) in files {
        let over = p.total_bytes > 0 && total > p.total_bytes && end_of(&f) != End::Running;
        if !named.contains(&canonical(&f)) && (old(at) || over) && fs::remove_file(&f).is_ok() {
            total -= len;
        }
    }
    let Ok(repos) = fs::read_dir(root) else { return };
    for r in repos.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
        let mut left = Vec::new();
        logs_under(&r, &mut left);
        if left.is_empty() {
            let _ = fs::remove_dir_all(&r);
        } else {
            remove_empty(&r);
        }
    }
}

fn remove_empty(dir: &Path) {
    let Ok(d) = fs::read_dir(dir) else { return };
    for p in d.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
        remove_empty(&p);
        let _ = fs::remove_dir(&p);
    }
}

/// The logs the verify-red records of every repo with logs name.
fn named(root: &Path) -> HashSet<PathBuf> {
    let Ok(repos) = fs::read_dir(root) else { return HashSet::new() };
    repos
        .flatten()
        .filter_map(|e| fs::read_to_string(e.path().join(REPO_FILE)).ok())
        .flat_map(|repo| verify_red::logs(repo.trim_end()))
        .map(|l| canonical(Path::new(&l)))
        .collect()
}

/// A record and a pass can reach the store by different spellings, e.g.
/// /tmp and /private/tmp.
fn canonical(p: &Path) -> PathBuf {
    fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// A run of <kind> by <step> on <b> at <sha> in <wt>'s repo starts its log,
/// under <p>: None when <p> keeps none, or the log can't be made (it warns).
pub fn start(wt: &str, b: &str, sha: &str, kind: &str, step: &str, cmd: &str, p: &Policy) -> Option<Run> {
    if p.keep == Keep::None {
        return None;
    }
    let repo = git::main_worktree(wt);
    let root = root();
    let made = Run::start(&branch_dir(&root, &repo, b), b, sha, kind, step, cmd, p.file_bytes).inspect(|_| {
        let f = repo_dir(&root, &repo).join(REPO_FILE);
        if !f.exists() {
            let _ = fs::write(f, format!("{repo}\n"));
        }
    });
    made.map_err(|e| eprintln!("sd: warning: can't keep {kind}'s output in {}: {e}", root.display())).ok()
}

/// After a run on <b> in <wt>'s repo: <b>'s logs, then every repo's, by <p>.
pub fn prune(wt: &str, b: &str, p: &Policy) {
    let root = root();
    let named = named(&root);
    prune_branch(&branch_dir(&root, &git::main_worktree(wt), b), p, &named);
    prune_all(&root, p, &named, SystemTime::now());
}

/// sd act forget: <b>'s logs in <wt>'s repo go, named or not.
pub fn forget(wt: &str, b: &str) {
    let root = root();
    let repo = git::main_worktree(wt);
    for f in logs_in(&branch_dir(&root, &repo, b)) {
        let _ = fs::remove_file(f);
    }
    remove_empty(&repo_dir(&root, &repo));
}

/// The pass: the logs of branches <repo> no longer has go, as pins::sweep
/// does their refs.
pub fn sweep(repo: &str) {
    let dir = repo_dir(&root(), &git::main_worktree(repo));
    let mut all = Vec::new();
    logs_under(&dir, &mut all);
    for f in all {
        let b = f.parent().and_then(|p| p.strip_prefix(&dir).ok()).map(|p| p.to_string_lossy().into_owned());
        if b.is_some_and(|b| !b.is_empty() && !git::branch_in(repo, &b)) {
            let _ = fs::remove_file(f);
        }
    }
    remove_empty(&dir);
}

/// The pass, after every repo's sweep: the limits over every repo's logs.
pub fn prune_every_repo(p: &Policy) {
    let root = root();
    prune_all(&root, p, &named(&root), SystemTime::now());
}

/// The newest of <b>'s logs in <wt>'s repo still being written, by a live
/// sd: (path, pid, since).
pub fn running(wt: &str, b: &str) -> Option<(PathBuf, u32, String)> {
    let dir = branch_dir(&root(), &git::main_worktree(wt), b);
    logs_in(&dir).into_iter().rev().filter(|f| end_of(f) == End::Running).find_map(|f| head(&f).map(|(pid, since)| (f, pid, since)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    struct Dir(PathBuf);
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn dir(name: &str) -> Dir {
        let d = std::env::temp_dir().join(format!("sd-logs-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        Dir(d)
    }

    fn policy(keep: Keep, runs: usize) -> Policy {
        Policy { keep, runs, file_bytes: 0, age_secs: 0, total_bytes: 0 }
    }

    const DEAD: u32 = 999_999_999;

    /// A log at <dir>/<name> by <pid> that ended <how> (none: no last line),
    /// <len> bytes of output.
    fn log_by(dir: &Path, name: &str, how: &str, len: usize, pid: u32) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let f = dir.join(name);
        let end = if how.is_empty() { String::new() } else { format!("<== {how} after 1s\n") };
        fs::write(&f, format!("==> 2026-11-05T00:00:00Z feat abc verify (verify) pid {pid}: x\n{}\n{end}", "o".repeat(len))).unwrap();
        f
    }

    /// A finished log by a gone sd, or with no <how> one this sd is writing.
    fn log(dir: &Path, name: &str, how: &str, len: usize) -> PathBuf {
        log_by(dir, name, how, len, if how.is_empty() { std::process::id() } else { DEAD })
    }

    fn names(dir: &Path) -> Vec<String> {
        logs_in(dir).iter().map(|f| f.file_name().unwrap().to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn end_of_reads_the_last_line() {
        let d = dir("end");
        assert_eq!(end_of(&log(&d.0, "a.log", "exit 0", 3)), End::Passed);
        assert_eq!(end_of(&log(&d.0, "b.log", "exit 2", 3)), End::Failed);
        assert_eq!(end_of(&log(&d.0, "c.log", "timed out after 5s", 3)), End::Failed);
        assert_eq!(end_of(&log(&d.0, "d.log", "", 3)), End::Running);
        assert_eq!(end_of(&log(&d.0, "e.log", "exit 0", 2000)), End::Passed);
    }

    #[test]
    fn a_log_with_no_last_line_whose_sd_is_gone_failed() {
        let d = dir("pid");
        let killed = log_by(&d.0, "killed.log", "", 3, DEAD);
        assert_eq!(end_of(&killed), End::Failed);
        prune_branch(&d.0, &policy(Keep::All, 0), &HashSet::new());
        prune_branch(&d.0, &policy(Keep::None, 0), &HashSet::new());
        assert!(!killed.exists(), "it doesn't stay as a running one would");
    }

    #[test]
    fn head_reads_the_pid_and_start_off_the_first_line_alone() {
        let d = dir("head");
        let f = log_by(&d.0, "a.log", "", 0, 42);
        fs::OpenOptions::new().append(true).open(&f).unwrap().write_all(b"\xff\xfe not utf-8\n").unwrap();
        assert_eq!(head(&f), Some((42, "2026-11-05T00:00:00Z".to_string())));
    }

    #[test]
    fn prune_branch_keeps_failures_and_the_newest_pass() {
        let d = dir("failed");
        for (n, how) in [("1.log", "exit 1"), ("2.log", "exit 0"), ("3.log", "exit 1"), ("4.log", "exit 0"), ("5.log", "")] {
            log(&d.0, n, how, 1);
        }
        prune_branch(&d.0, &policy(Keep::Failed, 0), &HashSet::new());
        assert_eq!(names(&d.0), ["1.log", "3.log", "4.log", "5.log"]);
    }

    #[test]
    fn prune_branch_keeps_the_newest_runs_and_those_named_or_running() {
        let d = dir("runs");
        for (n, how) in [("1.log", "exit 1"), ("2.log", "exit 1"), ("3.log", "exit 1"), ("4.log", ""), ("5.log", "exit 1")] {
            log(&d.0, n, how, 1);
        }
        let named: HashSet<PathBuf> = [canonical(&d.0.join("1.log"))].into();
        prune_branch(&d.0, &policy(Keep::All, 3), &named);
        assert_eq!(names(&d.0), ["1.log", "3.log", "4.log", "5.log"], "the newest 3, and the named one past them");
        prune_branch(&d.0, &policy(Keep::None, 0), &named);
        assert_eq!(names(&d.0), ["1.log", "4.log"]);
    }

    #[test]
    fn a_log_named_by_another_spelling_of_its_path_stays() {
        let d = dir("spelled");
        let link = d.0.with_file_name(format!("{}-link", d.0.file_name().unwrap().to_string_lossy()));
        std::os::unix::fs::symlink(&d.0, &link).unwrap();
        log(&d.0, "1.log", "exit 1", 1);
        let named: HashSet<PathBuf> = [canonical(&link.join("1.log"))].into();
        prune_branch(&d.0, &policy(Keep::None, 0), &named);
        prune_all(&d.0, &Policy { age_secs: 1, ..policy(Keep::All, 0) }, &named, SystemTime::now() + Duration::from_secs(9));
        let _ = fs::remove_file(&link);
        assert_eq!(names(&d.0), ["1.log"]);
    }

    #[test]
    fn prune_all_drops_the_old_then_the_oldest_past_the_total_and_empty_dirs() {
        let d = dir("all");
        let (a, b) = (d.0.join("repo-1/feat"), d.0.join("repo-2/x/y"));
        let at = |f: &Path, ago: u64| {
            let t = SystemTime::now() - Duration::from_secs(ago);
            fs::File::options().write(true).open(f).unwrap().set_modified(t).unwrap();
        };
        let old = log(&a, "old.log", "exit 1", 10);
        at(&old, 100);
        let named = log(&a, "named.log", "exit 1", 10);
        at(&named, 100);
        let older = log(&b, "older.log", "exit 1", 1000);
        at(&older, 50);
        let running = log(&b, "running.log", "", 1000);
        at(&running, 40);
        let newer = log(&b, "newer.log", "exit 1", 1000);
        at(&newer, 30);
        let p = Policy { age_secs: 90, total_bytes: 2500, ..policy(Keep::All, 0) };
        prune_all(&d.0, &p, &[canonical(&named)].into(), SystemTime::now());
        assert!(!old.exists() && named.exists(), "the old go, the named stay");
        assert!(!older.exists() && running.exists() && newer.exists(), "past the total, the oldest not running go");
        let p = Policy { age_secs: 1, ..p };
        prune_all(&d.0, &p, &HashSet::new(), SystemTime::now());
        assert!(!d.0.join("repo-1").exists() && !d.0.join("repo-2").exists());
    }

    #[test]
    fn a_run_past_its_cap_keeps_its_head_and_its_end() {
        let d = dir("cap");
        let mut r = Run::start(&d.0.join("b"), "b", "0123456789abcdef", "verify", "implement", "make", 400).unwrap();
        assert_eq!(r.path.file_name().unwrap().to_string_lossy().len(), "20261105T140300.123Z-0123456789ab-verify-implement.log".len());
        for i in 1..=1000 {
            r.write(format!("{i}\n").as_bytes());
        }
        let text = fs::read_to_string(r.finish("exit 1")).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].starts_with("==> ") && lines[0].ends_with(": make"));
        assert_eq!(lines[1], "1");
        assert!(lines.contains(&"1000") && !lines.contains(&"500"));
        assert!(lines.iter().any(|l| l.starts_with("[… ") && l.ends_with(" bytes not kept …]")));
        assert_eq!(lines[lines.len() - 1], "<== exit 1 after 0s");
        assert!(text.len() < 400 + 200, "{}", text.len());
    }

    #[test]
    fn a_run_whose_output_has_no_last_newline_still_ends_on_its_own_line() {
        let d = dir("nl");
        let mut r = Run::start(&d.0, "b", "abc", "verify", "verify", "x", 0).unwrap();
        r.write(b"partial");
        let text = fs::read_to_string(r.finish("exit 0")).unwrap();
        assert!(text.ends_with("partial\n<== exit 0 after 0s\n"), "{text}");
        assert_eq!(end_of(&logs_in(&d.0)[0]), End::Passed);
    }

    #[test]
    fn policy_reads_the_settings_and_falls_back_to_the_defaults() {
        let p = Policy::from(|k| match k {
            "settings.logKeep" => "all".into(),
            "settings.logRuns" => "2".into(),
            "settings.logAge" => "0.5".into(),
            _ => String::new(),
        });
        assert_eq!((p.keep, p.runs, p.file_bytes, p.age_secs, p.total_bytes), (Keep::All, 2, 8 << 20, 0, 1 << 30));
    }
}
