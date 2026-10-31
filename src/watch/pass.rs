// The pass, refresh and vacuum (src/watch_help.txt): sd state every enrolled
// branch, glyph it, notify per the policy, and keep each branch's state for
// the next pass; with settings.autoRun, a pass also launches a branch's
// first step when it's one a pass runs, mechanical (#267) or an agent's
// (#269). sd state runs as `sd state`, this binary in a process of its own,
// so a hung one can be timed out: on purpose not state::observe in-process,
// as sd act calls it, since the watcher supervises its children with
// deadlines (#12).

use super::{die, env_or, home, HELP};
use crate::config::{self, state_dir};
use crate::git::{self, git, git_input, git_out};
use crate::json;
use crate::proc;
use crate::{failed, harness, limit, running, time};
use crate::val::{at, on, t, text};
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The NEXT steps that are ❌: chrismo's to deal with.
const RED: [&str; 20] = [
    "ci-red", "ci-blocked", "conflict", "human", "foreign", "parent-closed", "review-decide", "question",
    "restack-undone", "bot-silent", "budget", "failed", "ci-config", "nothing", "default", "dirty", "read",
    "submit", "yours", "verify-red",
];

/// chrismo's steps (who "you") that don't notify when first in NEXT (#326).
/// Notify when the branch is stuck until chrismo acts; stay quiet for end
/// states (closed, ready: their glyph shows them) and when sd isn't sure yet
/// (errors, stale, gh-wait). So bot or answer, handed to chrismo when
/// address's gate refuses, is never a silent stall.
const QUIET_YOURS: [&str; 5] = ["closed", "ready", "errors", "stale", "gh-wait"];

/// What sd state's next steps mean to the pass. carry: sd state is unsure,
/// so what was notified, what failed and the stale clock carry over. hold:
/// an agent step hides every other step, so what was notified and failed
/// carry over. keep: the notified steps a step that hides them keeps.
#[derive(Debug, PartialEq)]
struct Class {
    carry: bool,
    hold: bool,
    keep: Vec<&'static str>,
    glyph: &'static str,
    /// {key, msg}: the steps that notify.
    reasons: Vec<Value>,
    note: Option<&'static str>,
}

/// The steps a pass runs, by NEXT key, with the step each is: the run steps
/// (#267), and the agent steps (#269), whose step is the running marker's.
/// Not stale (the pass has just fetched), merged (forget --remove deletes the
/// worktree), draft, ci-rerun, or ship-parent and open-pr, which may spend
/// tokens describing.
const DISPATCH: [(&str, &str); 11] = [
    ("fetch", "fetch"),
    ("behind", "restack"),
    ("push", "push"),
    ("retarget", "retarget"),
    ("bot-trigger", "trigger"),
    ("implement", "implement"),
    ("review-fix", "implement"),
    ("review", "review"),
    ("bot", "address"),
    ("ci-red", "ci-fix"),
    ("conflict", "resolve"),
];

const AGENT_STEPS: [&str; 5] = ["implement", "review", "address", "ci-fix", "resolve"];

fn act_args(step: &str) -> Vec<&str> {
    if step == "resolve" {
        vec!["restack", "--resolve"]
    } else {
        vec![step]
    }
}

/// How long a launched step past its lease has to end on a TERM before it
/// gets a KILL: launchd's default exit timeout (comment on #268, 2026-10-23).
const KILL_AFTER: Duration = Duration::from_secs(5);

/// <sig> to <pid>'s process group, else to <pid> when it leads none.
fn end(pid: i32, sig: i32) {
    if pid <= 1 {
        return;
    }
    unsafe {
        if libc::kill(-pid, sig) != 0 {
            libc::kill(pid, sig);
        }
    }
}

/// End the launched step holding <l>, <b>'s lease: TERM to its process
/// group, KILL if it's still there KILL_AFTER later; then release the lease.
fn stop(repo: &str, b: &str, l: &running::Lease) {
    let pid: i32 = l.pid.parse().unwrap_or(0);
    end(pid, libc::SIGTERM);
    let t = Instant::now();
    while running::lease(repo, b).is_some_and(|l| l.live) && t.elapsed() < KILL_AFTER {
        std::thread::sleep(Duration::from_millis(50));
    }
    if running::lease(repo, b).is_some_and(|l| l.live) {
        end(pid, libc::SIGKILL);
    }
    release(repo, b, l);
}

/// Remove <b>'s running marker if <l> still holds it.
fn release(repo: &str, b: &str, l: &running::Lease) {
    let Some(marker) = running::file(repo, b) else { return };
    if running::lease(repo, b).is_some_and(|now| now.pid == l.pid) {
        let _ = fs::remove_file(&marker);
    }
}

/// How long a launched step's lease outlasts its own timeout, so the lease
/// ends only a step that's stuck.
const GRACE: u64 = 60;

/// The program that is sd <verb>, and the args before its own: sd itself and
/// <verb>. In a debug build (cargo test's) with <seam> set, that program
/// instead, run as sd-<verb> is: the suite fakes sd state and sd act with
/// it. A release build, the one installed, never reads it.
fn sd_verb(verb: &'static str, seam: &str) -> (String, Vec<&'static str>) {
    let fake = if cfg!(debug_assertions) { std::env::var(seam).ok() } else { None };
    match fake {
        Some(p) if !p.is_empty() => (p, vec![]),
        _ => (proc::sd_itself().map_or("sd".into(), |p| p.to_string_lossy().into_owned()), vec![verb]),
    }
}

/// The dispatch gate, the one place it's decided: a PR the viewer authored,
/// with no one else's commits on the remote. A stand-in for #266's facts (the
/// PR's author isn't the viewer; the remote has commits from someone else),
/// which replace it; not keyed on sd-mode, which #266 deletes. fetch passes
/// whoever wrote the PR: only a followed PR's NEXT has it, sd act fetch
/// refuses any other branch, and it takes only the author's pushes, refusing
/// when the branch has commits of its own (#250).
fn may_dispatch(s: &Value, key: &str) -> bool {
    if key == "fetch" {
        return true;
    }
    let viewer = t(s, "viewer");
    let mine = !viewer.is_empty() && t(s, "pr.author") == viewer;
    let no_one_elses = match at(s, "push.remote_ours") {
        Value::Bool(true) => at(s, "push.remote_made") != &Value::Bool(false),
        Value::Bool(false) => at(s, "push.theirs").as_f64() == Some(0.0),
        _ => at(s, "push.remote_sha").is_null(),
    };
    mine && no_one_elses
}

fn reason(step: &Value) -> Value {
    json!({"key": at(step, "key"), "msg": at(step, "say")})
}

fn classify(s: &Value) -> Class {
    let next: &[Value] = at(s, "next").as_array().map_or(&[], Vec::as_slice);
    let k: Vec<&str> = next.iter().map(|x| at(x, "key").as_str().unwrap_or("")).collect();
    let notifies = |i: usize, x: &Value| {
        let key = at(x, "key").as_str().unwrap_or("");
        RED.contains(&key) || (i == 0 && at(x, "who") == "you" && !QUIET_YOURS.contains(&key))
    };
    let red: Vec<Value> = next.iter().enumerate().filter(|(i, x)| notifies(*i, x)).map(|(_, x)| reason(x)).collect();
    let waiting = !next.iter().any(|x| at(x, "who").as_str().is_some_and(|w| w != "wait"));
    let unsure = k.contains(&"errors") || k.contains(&"stale") || k.contains(&"gh-wait");
    let (glyph, reasons, note) = match k.as_slice() {
        [] => ("⚠️", vec![], Some("sd state gave no next steps")),
        ["running"] => ("🤖", vec![], None),
        ["merged"] => ("🏁", vec![reason(&next[0])], None),
        ["closed"] => ("🚫", vec![], None),
        _ if !red.is_empty() => ("❌", red, None),
        _ if unsure => ("⚠️", vec![], None),
        ["ready"] => ("✅", vec![reason(&next[0])], None),
        _ if waiting => ("⏳", vec![], None),
        _ => ("🔧", vec![], None),
    };
    Class {
        carry: next.is_empty() || unsure,
        hold: k == ["running"],
        keep: if k == ["merge-wait"] || k == ["attached"] { vec!["ready"] } else { vec![] },
        glyph,
        reasons,
        note,
    }
}

impl Class {
    fn record(&self) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("carry".into(), self.carry.into());
        m.insert("hold".into(), self.hold.into());
        m.insert("keep".into(), json!(self.keep));
        m.insert("glyph".into(), self.glyph.into());
        m.insert("reasons".into(), Value::Array(self.reasons.clone()));
        if let Some(n) = self.note {
            m.insert("note".into(), n.into());
        }
        m
    }
}

/// Whole days, else hours, minutes or seconds; a negative span (a clock
/// running behind) reads 0s.
fn dur(s: i64) -> String {
    let s = s.max(0);
    if s >= 86400 {
        format!("{}d", s / 86400)
    } else if s >= 3600 {
        format!("{}h", s / 3600)
    } else if s >= 60 {
        format!("{}m", s / 60)
    } else {
        format!("{s}s")
    }
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

// localtime_r needn't read TZ, as localtime does. Not in the libc crate.
extern "C" {
    fn tzset();
}

/// date '+%Y-%m-%d %H:%M:%S'.
fn local_time() -> String {
    let mut buf = [0u8; 64];
    // SAFETY: tm is plain data that localtime_r fills, and strftime writes
    // at most buf.len() bytes, returning how many.
    let n = unsafe {
        tzset();
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return String::new();
        }
        libc::strftime(buf.as_mut_ptr().cast(), buf.len(), c"%Y-%m-%d %H:%M:%S".as_ptr(), &tm)
    };
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

fn basename(p: &str) -> &str {
    Path::new(p).file_name().and_then(|n| n.to_str()).unwrap_or(p)
}

/// <state>/branches/<repo's name>-<first 8 hex of sha1 of its path>, as
/// shasum named it.
fn repo_state_dir(state: &str, repo: &str) -> String {
    let hex = sha1_smol::Sha1::from(repo).digest().to_string();
    format!("{state}/branches/{}-{}", basename(repo), &hex[..8])
}

fn show(lines: &str) -> String {
    lines.replace('\t', "  ")
}

fn config_lines(lines: Vec<String>) {
    for l in lines {
        eprintln!("sd config: {l}");
    }
}

fn global_num(path: &str) -> f64 {
    config::get_global(path, false).ok().and_then(|v| v.parse().ok()).unwrap_or(0.0)
}

/// settings.remote as inside <repo>, where its repo file can set it.
fn remote_of(repo: &str) -> Option<String> {
    let here = std::env::current_dir().ok();
    std::env::set_current_dir(repo).ok()?;
    let r = config::get_global("settings.remote", false);
    if let Some(h) = here {
        let _ = std::env::set_current_dir(h);
    }
    r.map_err(config_lines).ok()
}

fn registered() -> Vec<String> {
    git(".", &["config", "--global", "--get-all", "sheepdog.repo"]).stdout.lines().map(str::to_string).collect()
}

fn enrolled(repo: &str) -> Vec<String> {
    let o = git(repo, &["config", "--get-regexp", r"^branch\..*\.sheepdog$"]);
    o.stdout
        .lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let k = f.next()?;
            if f.next() != Some("true") {
                return None;
            }
            k.strip_prefix("branch.")?.strip_suffix(".sheepdog").map(str::to_string)
        })
        .collect()
}

/// The worktree each branch is checked out in, counting one paused
/// mid-rebase of it, and not one whose directory is gone (rm -rf leaves git
/// listing it).
fn worktree_of(repo: &str) -> HashMap<String, String> {
    git::worktrees(repo)
        .into_iter()
        .filter_map(|w| {
            let b = w.branch?;
            (w.rebasing || Path::new(&w.path).is_dir()).then_some((b, w.path))
        })
        .collect()
}

/// Delete refs/sheepdog/<b>/* whose branch no longer exists. Refs outlive
/// git branch -D (config doesn't); the pin's last path component is its
/// kind (base, local, bot, accepted), the rest is the branch name.
fn sweep_pins(repo: &str) {
    let refs = git(repo, &["for-each-ref", "--format=%(refname)", "refs/sheepdog/"]).stdout;
    let gone: String = refs
        .lines()
        .filter(|r| {
            let rest = r.strip_prefix("refs/sheepdog/").unwrap_or(r);
            !git::branch_in(repo, rest.rsplit_once('/').map_or(rest, |(b, _)| b))
        })
        .map(|r| format!("delete {r}\n"))
        .collect();
    if !gone.is_empty() {
        git_input(repo, &["update-ref", "--stdin"], Some(gone.as_bytes()));
    }
}

/// settings.notify with <title> <message> appended, else a macOS
/// notification, given <secs>: one that hangs has failed (#350). Its stdout
/// goes to stderr, out of the pass record.
fn notify(secs: u64, title: &str, msg: &str) -> bool {
    let cmd = config::get_global("settings.notify", false).unwrap_or_else(|lines| {
        config_lines(lines);
        eprintln!("sd watch: sd config failed; using the default notifier");
        String::new()
    });
    let (c, what) = if cmd.is_empty() {
        let mut c = Command::new("osascript");
        c.args(["-e", "on run argv", "-e", "display notification (item 2 of argv) with title (item 1 of argv)"]);
        c.args(["-e", "end run", title, msg]);
        (c, "osascript".to_string())
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", &format!("{cmd} \"$@\""), "_", title, msg]);
        (c, cmd)
    };
    let o = proc::timeout_passing_stderr(secs, c);
    eprint!("{}", o.stdout);
    if o.ok {
        return true;
    }
    let how = if o.timed_out { format!("timed out after {secs}s") } else { "failed".into() };
    eprintln!("sd watch: notify {how}: {what}");
    false
}

/// What a state file says was notified: notified, else (a file from before
/// it was recorded) its reasons' keys.
fn notified_of(doc: &Value) -> Vec<Value> {
    match at(doc, "notified") {
        Value::Null => at(doc, "reasons").as_array().map_or(vec![], |rs| rs.iter().map(|r| at(r, "key").clone()).collect()),
        v => v.as_array().cloned().unwrap_or_default(),
    }
}

fn list(doc: &Value, k: &str) -> Vec<Value> {
    at(doc, k).as_array().cloned().unwrap_or_default()
}

fn read_json(f: impl AsRef<Path>) -> Option<Value> {
    config::one_value(&fs::read_to_string(f).ok()?)
}

/// <doc> to <file> through a temp file of this process's, so a reader never
/// sees half of it.
fn save(file: &str, doc: &Value) -> bool {
    let tmp = format!("{file}.{}.tmp", std::process::id());
    if fs::write(&tmp, json::to_string(doc) + "\n").and_then(|_| fs::rename(&tmp, file)).is_ok() {
        return true;
    }
    let _ = fs::remove_file(&tmp);
    false
}

fn unsure_why(note: Option<&str>, s: &Value) -> String {
    if let Some(n) = note {
        return n.into();
    }
    let mut all: Vec<&Value> = vec![];
    for k in ["errors", "stale", "lag"] {
        match at(s, k) {
            Value::Null => {}
            Value::Array(a) => all.extend(a),
            _ => return "its reasons couldn't be read".into(),
        }
    }
    let words: Option<Vec<&str>> = all.iter().map(|v| v.as_str()).collect();
    words.map_or("its reasons couldn't be read".into(), |w| w.join("; "))
}

pub(crate) fn json_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            json_files(&e.path(), out);
        } else if ft.is_file() && e.file_name().to_string_lossy().ends_with(".json") {
            out.push(e.path());
        }
    }
}

struct Watch {
    state: String,
    status: String,
    timeout: u64,
    vacuum_after: f64,
    rate_floor: f64,
    /// A pass stops for GitHub's rate limit; a refresh checks its branch
    /// either way.
    rate_stop: bool,
    /// sd watch vacuum's lines, when one runs.
    vacuum: Option<String>,
    /// The job's pass, not refresh or vacuum: may run a branch's first step
    /// (#267).
    dispatch: bool,
    /// settings.agentSteps: how many agent steps may run at once (#269).
    agent_steps: u64,
    /// The .pr files and the glyph index rows written.
    written: Vec<String>,
    glyphs: String,
}

impl Watch {
    fn new() -> Watch {
        Watch {
            state: state_dir(),
            status: env_or("STATUS_DIR", || format!("{}/.claude/tab-status", home())),
            timeout: std::env::var("SHEEPDOG_WATCH_TIMEOUT").ok().and_then(|s| s.parse().ok()).unwrap_or(120),
            vacuum_after: 0.0,
            rate_floor: 0.0,
            rate_stop: false,
            vacuum: None,
            dispatch: false,
            agent_steps: 0,
            written: vec![],
            glyphs: String::new(),
        }
    }

    fn dirs(&self) {
        for d in [&self.state, &self.status] {
            if let Err(e) = fs::create_dir_all(d) {
                die(format!("can't create {d}: {e}"));
            }
        }
    }

    fn log(&mut self, name: &str, b: &str, msg: &str) {
        if let Some(l) = &mut self.vacuum {
            l.push_str(&format!("{name}\t{b}\t{msg}\n"));
        }
    }

    fn pr_file(&self, wt: &str) -> String {
        format!("{}/{}.pr", self.status, basename(wt))
    }

    /// The glyph tab-status reads (<branch> TAB <glyph> in <worktree>.pr),
    /// and its row in the index sd watch glyph reads.
    fn write_glyph(&mut self, wt: &str, b: &str, glyph: &str) {
        if wt.is_empty() {
            return;
        }
        let f = self.pr_file(wt);
        if let Err(e) = fs::write(&f, format!("{b}\t{glyph}\n")) {
            eprintln!("sd watch: can't write {f}: {e}");
        }
        self.written.push(f);
        self.glyphs.push_str(&format!("{wt}\t{b}\t{glyph}\n"));
    }

    /// Remove the <worktree>.pr file of a name two indexed worktrees share
    /// while on the same branch, when the file holds that branch:
    /// tab-status reads the file by name, and its branch check can't tell
    /// them apart (sd watch glyph can).
    fn drop_ambiguous(&self, index: &str) {
        let mut seen = HashSet::new();
        let mut count: HashMap<(&str, &str), usize> = HashMap::new();
        for row in index.lines() {
            let mut f = row.split('\t');
            let (wt, b) = (f.next().unwrap_or(""), f.next().unwrap_or(""));
            if !seen.insert(wt) {
                continue;
            }
            let n = count.entry((basename(wt), b)).or_default();
            *n += 1;
            if *n == 2 {
                let f = self.pr_file(wt);
                let held = fs::read_to_string(&f).unwrap_or_default();
                if held.trim_end_matches('\n').split('\t').next() == Some(b) {
                    let _ = fs::remove_file(&f);
                }
            }
        }
    }

    /// Fetch <repo>'s remote. Err is the pass line for a remote that can't
    /// be read, so nothing should be checked; Ok's is one for a failed fetch.
    fn fetch(&self, repo: &str, name: &str) -> Result<Option<String>, String> {
        let Some(remote) = remote_of(repo) else { return Err(format!("{name}\t-\t⚠️\tsd config failed")) };
        let o = git::git_timed(repo, self.timeout, &["fetch", "--prune", "--quiet", &remote]);
        if o.ok {
            return Ok(None);
        }
        let err = o.stdout + &o.stderr;
        Ok(Some(format!("{name}\t-\t⚠️\tfetch from {remote} failed: {}", err.lines().next().unwrap_or(""))))
    }

    /// Why the pass stops for GitHub's GraphQL quota, if it must. A refusal
    /// carries no reset time; GitHub's rate_limit endpoint, which costs
    /// nothing, has it.
    fn rate_stop(&self, s: &Value) -> Option<String> {
        let left = t(s, "rate_limit.remaining");
        let why = if on(s, "rate_limit.limited") {
            "GitHub refused a call for the rate limit".to_string()
        } else if !left.is_empty() && left.bytes().all(|c| c.is_ascii_digit()) && left.parse::<f64>().ok()? < self.rate_floor {
            format!("GitHub rate limit: {left} points left, under settings.rateLimitFloor ({})", self.rate_floor)
        } else {
            return None;
        };
        let mut reset = t(s, "rate_limit.reset_at");
        if reset.is_empty() {
            let o = proc::with_timeout(self.timeout, "gh", &["api", "rate_limit", "--jq", ".resources.graphql.reset | todate"], None);
            reset = if o.ok { git::strip_nl(o.stdout) } else { String::new() };
        }
        let resets = if reset.is_empty() { String::new() } else { format!(" (the quota resets at {reset})") };
        Some(format!("{why}: the branches below weren't checked{resets}"))
    }

    /// Check one branch: its pass lines, whether sd state answered, and why
    /// the pass must stop for the rate limit, if it must.
    fn check(&mut self, repo: &str, name: &str, b: &str, wt: &str) -> (String, bool, Option<String>) {
        let (prog, verb) = sd_verb("state", "SHEEPDOG_STATE_PROGRAM");
        let o = proc::with_timeout(self.timeout, &prog, &[&verb[..], &["--json", b]].concat(), Some(repo));
        if !o.ok {
            let why = format!("sd state failed: {}", o.stderr.lines().next().unwrap_or(""));
            self.write_glyph(wt, b, "⚠️");
            self.log(name, b, &format!("skipped, sd state is unsure: {why}"));
            return (format!("{name}\t{b}\t⚠️\t{why}\n"), false, None);
        }
        let Some(s) = config::one_value(&o.stdout) else {
            self.write_glyph(wt, b, "⚠️");
            self.log(name, b, "skipped, sd state is unsure: unparseable sd state output");
            return (format!("{name}\t{b}\t⚠️\tunparseable sd state output\n"), false, None);
        };
        let c = classify(&s);
        let title = format!("{name} {b}");
        let ended = self.end_on_a_dead_pr(repo, name, b, &s);

        let file = format!("{}/{b}.json", repo_state_dir(&self.state, repo));
        if let Some(d) = Path::new(&file).parent() {
            let _ = fs::create_dir_all(d);
        }
        // An unreadable previous state re-notifies rather than silently
        // dropping reasons.
        let prev_doc = read_json(&file).unwrap_or(Value::Null);
        let prev = notified_of(&prev_doc);
        let prev_failed = list(&prev_doc, "failed");
        let (mut sent, mut failed) = (vec![], vec![]);
        for r in &c.reasons {
            let key = at(r, "key");
            if prev.contains(key) {
                continue;
            }
            let msg = text(at(r, "msg"));
            if notify(self.timeout, &title, &msg) {
                sent.push(key.clone());
            } else if prev_failed.contains(key) {
                eprintln!("sd watch: giving up on: {msg}");
                sent.push(key.clone());
            } else {
                failed.push(key.clone());
            }
        }

        // The stale clock: merged, closed or checked out nowhere, since when.
        let prev_since = at(&prev_doc, "stale.since").as_f64().map(|f| f as i64);
        let prev_reason = t(&prev_doc, "stale.reason");
        let now = now();
        let mut why = match c.glyph {
            "🏁" => "merged",
            "🚫" => "closed",
            _ if wt.is_empty() => "checked out nowhere",
            _ => "",
        }
        .to_string();
        let since = if c.carry {
            why = prev_reason;
            prev_since
        } else if !why.is_empty() && why == prev_reason && prev_since.is_some() {
            prev_since
        } else {
            (!why.is_empty()).then_some(now)
        };
        let (mut stale, mut unenrolled) = (Value::Null, None);
        if let Some(since) = since {
            stale = json!({"reason": why, "since": since});
            let age = now - since;
            if !c.carry && (self.vacuum.is_some() || (self.vacuum_after > 0.0 && age as f64 >= self.vacuum_after)) {
                let o = git(repo, &["config", "--unset", &format!("branch.{b}.sheepdog")]);
                if o.ok {
                    unenrolled = Some(format!("unenrolled: {why} for {} (sd act enroll {b} to watch it again)", dur(age)));
                    stale = Value::Null;
                } else {
                    eprint!("{}", o.stderr);
                    self.log(name, b, &format!("couldn't unenroll: {why} for {}", dur(age)));
                }
            }
        }
        if c.carry && self.vacuum.is_some() {
            self.log(name, b, &format!("skipped, sd state is unsure: {}", unsure_why(c.note, &s)));
        }

        // A refresh can overlap a pass: unlocked, so both may notify a new
        // reason and the slower may write the older state. Only the temp
        // file is per-process.
        let kept = |l: &[Value]| -> Vec<Value> { l.iter().filter(|p| p.as_str().is_some_and(|p| c.keep.contains(&p))).cloned().collect() };
        let (failed, notified) = if c.carry || c.hold {
            ([prev_failed, failed].concat(), [prev, sent].concat())
        } else {
            let known = [prev.as_slice(), &sent].concat();
            let now_notified = c.reasons.iter().map(|r| at(r, "key")).filter(|k| known.contains(k)).cloned();
            ([failed, kept(&prev_failed)].concat(), kept(&prev).into_iter().chain(now_notified).collect())
        };
        let mut rec = c.record();
        rec.insert("state".into(), s.clone());
        rec.insert("stale".into(), stale);
        rec.insert("failed".into(), failed.into());
        rec.insert("notified".into(), notified.into());
        if !save(&file, &Value::Object(rec)) {
            eprintln!("sd watch: couldn't save {file}: its reasons will notify again");
        }

        self.write_glyph(wt, b, c.glyph);
        let msgs: Vec<String> = c.reasons.iter().map(|r| at(r, "msg")).filter(|m| !m.is_null()).map(text).chain(c.note.map(str::to_string)).collect();
        let mut out = format!("{name}\t{b}\t{}\t{}\n", c.glyph, msgs.join("; "));
        out.push_str(&ended);
        if let Some(u) = unenrolled {
            if self.vacuum.is_none() {
                notify(self.timeout, &title, &u);
            }
            out.push_str(&format!("{name}\t{b}\t-\t{u}\n"));
            self.log(name, b, &u);
        }
        let stop = if self.rate_stop { self.rate_stop(&s) } else { None };
        let errors = at(&s, "errors").as_array().is_some_and(|e| !e.is_empty());
        if self.dispatch && !c.carry && !errors && stop.is_none() {
            out.push_str(&self.dispatch(repo, name, b, &s).unwrap_or_default());
        }
        (out, true, stop)
    }

    /// End the step a pass launched on <b> once its PR has merged or closed
    /// (#270), as reconcile ends one past its deadline, but not counted: it
    /// didn't fail. Its edits stay in the worktree. Its pass line, else "".
    fn end_on_a_dead_pr(&mut self, repo: &str, name: &str, b: &str, s: &Value) -> String {
        let state = t(s, "pr.state");
        if state != "MERGED" && state != "CLOSED" {
            return String::new();
        }
        let Some(l) = running::lease(repo, b).filter(|l| l.live && l.deadline.is_some()) else { return String::new() };
        stop(repo, b, &l);
        let msg = format!("sd act {}: ended, #{} {}", l.step, t(s, "pr.number"), state.to_lowercase());
        self.log(name, b, &msg);
        let line = format!("{name}\t{b}\t-\t{msg}\n");
        eprint!("{}", show(&line));
        line
    }

    /// Launch the branch's first NEXT step when it's one a pass runs: one
    /// attempt, under the watch timeout, waiting only till its lease is
    /// taken. Its output goes to steps.log in the state dir; its pass line
    /// to stderr, the job's log.
    fn dispatch(&self, repo: &str, name: &str, b: &str, s: &Value) -> Option<String> {
        let first = at(s, "next").get(0)?;
        let key = at(first, "key").as_str()?;
        let step = DISPATCH.iter().find(|(k, _)| *k == key)?.1;
        let agent = AGENT_STEPS.contains(&step);
        if at(first, "who").as_str() != Some(if agent { "agent" } else { "run" }) {
            return None;
        }
        // A file that doesn't read is off.
        let enabled = config::get_text_in(repo, "settings.autoRun", b, false).is_ok_and(|v| v == "true");
        // A followed PR's NEXT has no attached step, so neither is left to NEXT.
        let busy = !at(s, "session.running").is_null()
            || on(s, "session.locked")
            || running::lease(repo, b).is_some_and(|l| l.live);
        if !enabled || busy || !may_dispatch(s, key) {
            return None;
        }
        let verb = act_args(step).join(" ");
        let msg = match agent.then(|| self.held()).flatten() {
            Some(why) => format!("not launching sd act {verb}: {why}"),
            None => match self.launch(repo, step, b) {
                Ok(()) => format!("launched sd act {verb}"),
                Err(e) => format!("couldn't launch sd act {verb}: {e}"),
            },
        };
        let line = format!("{name}\t{b}\t-\t{msg}\n");
        eprint!("{}", show(&line));
        Some(line)
    }

    /// Why an agent step isn't launched now: settings.agentSteps of them
    /// already running, in any registered repo, launched or run by hand
    /// (#350), or a usage limit till its reset (crate::limit).
    fn held(&self) -> Option<String> {
        if self.agent_steps == 0 {
            return Some("settings.agentSteps 0".into());
        }
        let running = registered()
            .iter()
            .flat_map(|r| running::leases(r))
            .filter(|l| l.live && AGENT_STEPS.contains(&l.step.as_str()))
            .count() as u64;
        if running >= self.agent_steps {
            let n = if running == 1 { "1 agent step".into() } else { format!("{running} agent steps") };
            return Some(format!("{n} running, settings.agentSteps {}", self.agent_steps));
        }
        let line = limit::holding()?;
        let (at, said) = line.split_once(' ').unwrap_or((&line, ""));
        let harness = said.split_once(':').map_or(said, |(h, _)| h);
        Some(format!("{harness}'s usage limit, till {at}"))
    }

    /// `sd watch step` for <step> on <b>, in a session of its own, so the
    /// pass's end and launchd's cleanup of its job leave it running.
    fn launch(&self, repo: &str, step: &str, b: &str) -> std::io::Result<()> {
        let log = fs::OpenOptions::new().create(true).append(true).open(format!("{}/steps.log", self.state))?;
        let mut c = Command::new(proc::sd_itself()?);
        c.args(["watch", "step", &self.timeout.to_string(), step, b])
            .current_dir(repo)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log);
        unsafe {
            c.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        if let Some(f) = running::outcome_file(repo, b) {
            let _ = fs::remove_file(f);
        }
        let mut child = c.spawn()?;
        let pid = child.id().to_string();
        let t = Instant::now();
        loop {
            if running::lease(repo, b).is_some_and(|l| l.pid == pid) {
                return Ok(());
            }
            if t.elapsed() >= Duration::from_secs(5) {
                end(child.id() as i32, libc::SIGKILL);
                return Err(std::io::Error::other("it hadn't taken its lease after 5s, so it was ended"));
            }
            if let Some(st) = child.try_wait()? {
                // A quick step can take its lease and end between two looks.
                let ran = running::outcome_file(repo, b).is_some_and(|f| Path::new(&f).exists());
                return if ran { Ok(()) } else { Err(std::io::Error::other(format!("it exited {} before taking its lease", proc::code(st)))) };
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The pass lines for the branch's launched step: how it ended, or that it
    /// ran past its lease or crashed, both counted in the failure record
    /// (#242). A marker with no deadline is a step chrismo ran: never ended
    /// or counted.
    fn reconcile(&self, repo: &str, name: &str, b: &str) -> String {
        let line = |msg: &str| {
            let l = format!("{name}\t{b}\t-\t{msg}\n");
            eprint!("{}", show(&l));
            l
        };
        // The lease before the outcome: a holder gone by the first read
        // wrote any outcome it was going to by the second.
        let lease = running::lease(repo, b);
        let mut out = String::new();
        if let Some(f) = running::outcome_file(repo, b) {
            if let Ok(text) = fs::read_to_string(&f) {
                let _ = fs::remove_file(&f);
                out.push_str(&line(text.trim_end()));
            }
        }
        let Some(l) = lease else { return out };
        let Some(deadline) = l.deadline else { return out };
        let why = if !l.live {
            release(repo, b, &l);
            out.is_empty().then(|| ("crashed", format!("sd act {} crashed: its process is gone", l.step)))
        } else if time::now_utc().1 > deadline {
            stop(repo, b, &l);
            Some(("past its deadline", format!("sd act {}: past its deadline, ended", l.step)))
        } else {
            return out;
        };
        if let Some((error, msg)) = why {
            failed::record_in(repo, b, &l.step, "sd", error);
            out.push_str(&line(&msg));
        }
        out
    }

    /// Ask GitHub what became of a gone branch's PR: its pass line, and
    /// whether to keep the branch's state for the next pass to ask again.
    fn ask_gone(&self, repo: &str, name: &str, b: &str, f: &str, cached: &Value) -> (String, bool) {
        static PR_URL: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
        let pr = t(cached, "state.pr.number");
        let Some(remote) = remote_of(repo) else { return (format!("{name}\t{b}\t⚠️\tbranch is gone; sd config failed\n"), true) };
        // By URL, not {owner}/{repo}: gh's default repo may be a fork.
        let url = t(cached, "state.pr.url");
        let re = PR_URL.get_or_init(|| Regex::new(r"^https://[^/]+/([^/]+/[^/]+)/pull/([0-9]+)$").unwrap());
        let api = re.captures(&url).map_or(format!("repos/{{owner}}/{{repo}}/pulls/{pr}"), |c| format!("repos/{}/pulls/{}", &c[1], &c[2]));
        let mut c = Command::new("gh");
        c.args(["api", &api]).current_dir(repo);
        if let Some(r) = git::gh_repo(repo, &remote) {
            c.env("GH_REPO", r);
        }
        let o = proc::timeout(self.timeout, c, false);
        if !o.ok {
            if !o.stderr.contains("HTTP 404") {
                return (format!("{name}\t{b}\t⚠️\tbranch is gone; GitHub didn't say what became of #{pr}\n"), true);
            }
            return (format!("{name}\t{b}\t⚠️\tbranch is gone; GitHub has no #{pr}\n"), false);
        }
        let pull = config::one_value(&o.stdout);
        if !pull.as_ref().is_some_and(|p| on(p, "merged")) {
            let state = match pull.as_ref().map(|p| at(p, "state")) {
                None => String::new(),
                Some(Value::Null) => "unknown".into(),
                Some(v) => text(v),
            };
            let glyph = if state == "closed" { "🚫" } else { "⚠️" };
            return (format!("{name}\t{b}\t{glyph}\tbranch is gone; #{pr} is {state}, not merged\n"), false);
        }
        let msg = format!("merged (#{pr}); the local branch is already gone");
        let line = format!("{name}\t{b}\t🏁\t{msg}\n");
        if notify(self.timeout, &format!("{name} {b}"), &msg) {
            return (line, false);
        }
        if list(cached, "failed").contains(&json!("merged")) {
            eprintln!("sd watch: giving up on: {msg}");
            return (line, false);
        }
        let mut doc = cached.clone();
        if let Some(m) = doc.as_object_mut() {
            m.insert("failed".into(), [list(cached, "failed"), vec![json!("merged")]].concat().into());
        }
        if !save(f, &doc) {
            eprintln!("sd watch: couldn't save {f}: its merge will notify again");
        }
        (line, true)
    }

    /// Drop the state of each branch no longer enrolled. One that is gone
    /// too (gh pr merge --delete-branch takes the branch's config,
    /// enrollment and all) has its PR asked about once, and a merge not yet
    /// notified is. GitHub not answering, or a failed notification's one
    /// retry, keeps the state for the next pass.
    fn report_gone(&self, repo: &str, name: &str) -> String {
        let dir = repo_state_dir(&self.state, repo);
        let mut files = vec![];
        json_files(Path::new(&dir), &mut files);
        files.sort();
        let mut out = String::new();
        for path in files {
            let f = path.to_string_lossy().into_owned();
            let b = f.strip_prefix(&format!("{dir}/")).and_then(|r| r.strip_suffix(".json")).unwrap_or_default();
            if git_out(repo, &["config", &format!("branch.{b}.sheepdog")]).as_deref() == Some("true") {
                continue;
            }
            let cached = read_json(&path).unwrap_or(Value::Null);
            if !t(&cached, "state.pr.number").is_empty()
                && !git::branch_in(repo, b)
                && !notified_of(&cached).contains(&json!("merged"))
            {
                let (line, keep) = self.ask_gone(repo, name, b, &f, &cached);
                out.push_str(&line);
                if keep {
                    continue;
                }
            }
            let _ = fs::remove_file(&path);
            let mut d = path.parent();
            while let Some(p) = d.filter(|p| *p != Path::new(&dir)) {
                if fs::remove_dir(p).is_err() {
                    break;
                }
                d = p.parent();
            }
        }
        out
    }

    fn pass(&mut self) {
        self.dirs();
        self.rate_stop = true;
        // A config failure is reported per repo by fetch, which then skips it.
        self.vacuum_after = global_num("settings.vacuumAfter");
        self.rate_floor = global_num("settings.rateLimitFloor");
        self.agent_steps = global_num("settings.agentSteps") as u64;
        let mut lines = String::new();
        let (mut stopped, mut stop_line) = (false, String::new());
        let mut skipped: Vec<(String, String)> = vec![];
        for repo in registered() {
            if !Path::new(&repo).is_dir() {
                continue;
            }
            let name = basename(&repo).to_string();
            sweep_pins(&repo);
            for f in config::sweep_branch_files(&repo, false) {
                self.log(&name, "-", &format!("deleted {f}: no local branch"));
            }
            let mut branches = vec![];
            for b in enrolled(&repo) {
                if git::branch_in(&repo, &b) {
                    branches.push(b);
                } else {
                    let out = git(&repo, &["config", "--unset", &format!("branch.{b}.sheepdog")]);
                    if out.ok {
                        lines.push_str(&format!("{name}\t{b}\t-\tunenrolled: no local branch\n"));
                        self.log(&name, &b, "unenrolled: no local branch");
                    } else {
                        eprint!("{}", out.stderr);
                    }
                }
            }
            lines.push_str(&self.report_gone(&repo, &name));
            if branches.is_empty() {
                continue;
            }
            if !stopped {
                match self.fetch(&repo, &name) {
                    Err(line) => {
                        lines.push_str(&format!("{line}\n"));
                        let why = line.rsplit('\t').next().unwrap_or_default();
                        for b in &branches {
                            self.log(&name, b, &format!("skipped: {why}"));
                        }
                        continue;
                    }
                    Ok(Some(line)) => lines.push_str(&format!("{line}\n")),
                    Ok(None) => {}
                }
            }
            let wts = worktree_of(&repo);
            for b in &branches {
                let wt = wts.get(b).map_or("", String::as_str);
                lines.push_str(&self.reconcile(&repo, &name, b));
                if stopped {
                    lines.push_str(&std::mem::take(&mut stop_line));
                    lines.push_str(&format!("{name}\t{b}\t-\tnot checked: rate limit\n"));
                    skipped.push((wt.to_string(), b.clone()));
                    self.log(&name, b, "skipped, not checked: GitHub rate limit");
                    continue;
                }
                let (out, _, stop) = self.check(&repo, &name, b, wt);
                lines.push_str(&out);
                if let Some(why) = stop {
                    stop_line = format!("{name}\t-\t⚠️\t{why}\n");
                    stopped = true;
                }
            }
        }

        // The branches not checked keep their glyphs, files and index rows both.
        let index = format!("{}/glyphs", self.state);
        for row in fs::read_to_string(&index).unwrap_or_default().lines() {
            let mut f = row.split('\t');
            let (wt, b) = (f.next().unwrap_or(""), f.next().unwrap_or(""));
            if skipped.iter().any(|(w, sb)| w == wt && sb == b) {
                let f = self.pr_file(wt);
                self.glyphs.push_str(&format!("{row}\n"));
                self.written.push(f);
            }
        }
        for e in fs::read_dir(&self.status).into_iter().flatten().flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            let f = format!("{}/{n}", self.status);
            if n.ends_with(".pr") && !n.starts_with('.') && !self.written.contains(&f) {
                let _ = fs::remove_file(&f);
            }
        }
        self.drop_ambiguous(&self.glyphs);
        self.write_index(&self.glyphs);
        let _ = fs::write(format!("{}/last-pass", self.state), format!("last pass: {}\n{lines}", local_time()));
    }

    fn write_index(&self, rows: &str) {
        let index = format!("{}/glyphs", self.state);
        let tmp = format!("{index}.{}.tmp", std::process::id());
        if fs::write(&tmp, rows).and_then(|_| fs::rename(&tmp, &index)).is_err() {
            let _ = fs::remove_file(&tmp);
            eprintln!("sd watch: couldn't save {index}");
        }
    }
}

pub fn pass() -> ExitCode {
    let mut w = Watch::new();
    w.dispatch = true;
    w.pass();
    ExitCode::SUCCESS
}

/// An agent step's timeout, from the branch's settings.stepTimeout: verify,
/// the agent and verify again each take up to that, and <secs> covers the
/// rest. stepTimeout 0 is none.
fn agent_secs(b: &str, secs: u64) -> u64 {
    match config::get_text("settings.stepTimeout", b, false).ok().and_then(|t| t.parse::<u64>().ok()) {
        Some(0) => u64::from(u32::MAX),
        Some(t) => 3 * t + secs,
        None => secs,
    }
}

/// sd watch step <secs> <step> <branch>, what a pass launches, run in the
/// branch's repo.
pub fn step(argv: &[String]) -> ExitCode {
    let [secs, step, b] = argv else { die("usage: sd watch step <secs> <step> <branch>") };
    let Ok(mut secs) = secs.parse::<u64>() else { die(format!("not whole seconds: {secs}")) };
    let Some(outcome) = running::outcome_file(".", b) else { die("not in a git repository") };
    if AGENT_STEPS.contains(&step.as_str()) {
        secs = agent_secs(b, secs);
    }
    let args = act_args(step);
    let verb = args.join(" ");
    let ran = running::leasing(b, step, secs + GRACE, || {
        let (prog, pre) = sd_verb("act", "SHEEPDOG_ACT_PROGRAM");
        let mut c = Command::new(prog);
        c.args(pre).args(&args).arg(b);
        let o = proc::timeout(secs, c, true);
        eprint!("{}", o.stdout);
        let how = if o.ok {
            "ok".to_string()
        } else if o.timed_out {
            let how = format!("timed out after {secs}s");
            failed::record(b, step, "sd", &how);
            how
        } else if o.code == harness::LIMIT_STATUS {
            limit::holding().map_or("stopped on a usage limit".into(), |l| format!("stopped on a usage limit: {l}"))
        } else if o.stopped {
            "stopped by a signal".to_string()
        } else {
            format!("failed (exit {}): {}", o.code, o.stdout.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or(""))
        };
        if let Err(e) = fs::write(&outcome, format!("ran sd act {verb}: {how}\n")) {
            eprintln!("sd watch: can't write {outcome}: {e}");
        }
    });
    if ran.is_some() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

pub fn refresh(argv: &[String]) -> ExitCode {
    if argv.len() > 1 {
        die("usage: sd watch refresh [<branch>]");
    }
    let mut b = argv.first().cloned().unwrap_or_default();
    match b.as_str() {
        "-h" | "--help" => {
            print!("{HELP}");
            return ExitCode::SUCCESS;
        }
        o if o.starts_with('-') => die(format!("unknown option: {o}")),
        _ => {}
    }
    if !git::in_repo() {
        die("not in a git repository");
    }
    if b.is_empty() {
        b = git_out(".", &["symbolic-ref", "-q", "--short", "HEAD"])
            .unwrap_or_else(|| die("not on a branch: name one (sd watch refresh <branch>)"));
    }
    if !git::branch_exists(&b) {
        die(format!("no such branch: {b}"));
    }
    if git_out(".", &["config", &format!("branch.{b}.sheepdog")]).as_deref() != Some("true") {
        die(format!("{b} isn't enrolled (sd act enroll {b})"));
    }
    // The repo as enroll registered it, so a pass and a refresh share the branch's state.
    let repo = git::main_worktree(".");
    if !registered().contains(&repo) {
        die(format!("{repo} isn't registered, so the pass doesn't watch it (sd act enroll {b})"));
    }
    let name = basename(&repo).to_string();
    let mut w = Watch::new();
    w.dirs();
    let fetched = match w.fetch(&repo, &name) {
        Err(line) => {
            println!("{}", show(&line));
            return ExitCode::from(1);
        }
        Ok(line) => line,
    };
    w.vacuum_after = global_num("settings.vacuumAfter");
    if let Some(line) = fetched {
        println!("{}", show(&line));
    }
    let wt = worktree_of(&repo).remove(&b).unwrap_or_default();
    let (out, ok, _) = w.check(&repo, &name, &b, &wt);
    // Unlocked like the state file: a pass that finishes between this read
    // and the rename loses to the older index until the next pass. Other
    // worktrees' rows are the last pass's, so drop_ambiguous can drop a .pr
    // file for a twin that has since gone, and check has already written
    // the shared file; the next pass settles both.
    if !w.glyphs.is_empty() {
        let old = fs::read_to_string(format!("{}/glyphs", w.state)).unwrap_or_default();
        let rows: String = old.lines().filter(|r| r.split('\t').next() != Some(&wt)).map(|r| format!("{r}\n")).collect();
        let rows = rows + &w.glyphs;
        w.write_index(&rows);
        w.drop_ambiguous(&rows);
    }
    print!("{}", show(&out));
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

pub fn vacuum() -> ExitCode {
    let mut w = Watch::new();
    w.vacuum = Some(String::new());
    w.pass();
    match w.vacuum.as_deref() {
        Some(l) if !l.is_empty() => print!("{}", show(l)),
        _ => println!("nothing stale"),
    }
    ExitCode::SUCCESS
}

/// vacuum --dry-run: what vacuum would do to each enrolled branch, from
/// cached state, and when a pass would.
pub fn vacuum_preview() -> ExitCode {
    let now = now();
    let after = config::get_global("settings.vacuumAfter", false).unwrap_or_else(|lines| {
        config_lines(lines);
        die("sd config failed")
    });
    let after = after.parse::<f64>().unwrap_or(0.0);
    let state = state_dir();
    let mut out = String::new();
    for repo in registered() {
        if !Path::new(&repo).is_dir() {
            continue;
        }
        let name = basename(&repo);
        for f in config::sweep_branch_files(&repo, true) {
            out.push_str(&format!("{name}\t-\t{f}: no local branch: vacuum deletes it, as does the next pass\n"));
        }
        for b in enrolled(&repo) {
            if !git::branch_in(&repo, &b) {
                out.push_str(&format!("{name}\t{b}\tno local branch: vacuum unenrolls it, as does the next pass\n"));
                continue;
            }
            let f = format!("{}/{b}.json", repo_state_dir(&state, &repo));
            if !Path::new(&f).is_file() {
                out.push_str(&format!("{name}\t{b}\tnot checked yet: vacuum checks it first\n"));
                continue;
            }
            let doc = read_json(&f).unwrap_or(Value::Null);
            if on(&doc, "carry") {
                out.push_str(&format!("{name}\t{b}\tunsure at the last saved check: vacuum skips it if sd state still is\n"));
                continue;
            }
            let Some(since) = at(&doc, "stale.since").as_f64() else { continue };
            let age = now - since as i64;
            let when = if after == 0.0 {
                "; a pass doesn't (vacuumAfter 0)".to_string()
            } else if age as f64 >= after {
                ", as does the next pass".to_string()
            } else {
                format!("; a pass, after {}", dur(after as i64))
            };
            out.push_str(&format!("{name}\t{b}\t{} for {}: vacuum unenrolls it{when}\n", t(&doc, "stale.reason"), dur(age)));
        }
    }
    if out.is_empty() {
        println!("nothing stale");
    } else {
        print!("{}", show(&out));
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(key: &str, who: &str) -> Value {
        json!({"key": key, "who": who, "say": format!("say {key}"), "cmd": null})
    }

    fn of(steps: &[Value]) -> Class {
        classify(&json!({"next": steps}))
    }

    fn glyph(steps: &[Value]) -> &'static str {
        of(steps).glyph
    }

    #[test]
    fn classify_no_next_steps_is_unsure_with_a_note() {
        for s in [json!({}), json!({"next": null}), json!({"next": []}), json!({"next": "x"}), json!(null)] {
            let c = classify(&s);
            assert_eq!(
                c,
                Class { carry: true, hold: false, keep: vec![], glyph: "⚠️", reasons: vec![], note: Some("sd state gave no next steps") },
                "{s}"
            );
        }
    }

    #[test]
    fn classify_running_alone_holds_quietly() {
        let c = of(&[step("running", "wait")]);
        assert_eq!((c.glyph, c.hold, c.carry, c.reasons.len()), ("🤖", true, false, 0));
        // Not alone, it holds nothing.
        assert!(!of(&[step("running", "wait"), step("ci-wait", "wait")]).hold);
    }

    #[test]
    fn classify_merged_and_ready_notify_their_say() {
        let c = of(&[step("merged", "you")]);
        assert_eq!((c.glyph, c.reasons.clone()), ("🏁", vec![json!({"key": "merged", "msg": "say merged"})]));
        let c = of(&[step("ready", "you")]);
        assert_eq!((c.glyph, c.reasons.clone()), ("✅", vec![json!({"key": "ready", "msg": "say ready"})]));
        assert_eq!(glyph(&[step("closed", "you")]), "🚫");
        assert!(of(&[step("closed", "you")]).reasons.is_empty());
    }

    #[test]
    fn classify_red_steps_notify_each_and_outrank_the_rest() {
        let c = of(&[step("human", "you"), step("push", "agent"), step("ci-red", "agent"), step("errors", "you")]);
        assert_eq!(c.glyph, "❌");
        assert_eq!(c.reasons, vec![json!({"key": "human", "msg": "say human"}), json!({"key": "ci-red", "msg": "say ci-red"})]);
        // errors alongside: unsure, so it carries, but still ❌.
        assert!(c.carry);
        for k in RED {
            assert_eq!(glyph(&[step(k, "you")]), "❌", "{k}");
        }
        assert!(RED.contains(&"failed"));
        assert!(RED.contains(&"verify-red"), "a failed verify is chrismo's until #325's part 2 hands it to the agent");
    }

    #[test]
    fn classify_a_first_step_thats_chrismos_notifies_whatever_its_key_326() {
        // Address refused with nothing ahead clearing it: NEXT hands bot or
        // answer to chrismo, and that's ❌, notified.
        for k in ["bot", "answer", "some-new-key"] {
            let c = of(&[step(k, "you"), step("ci-wait", "wait")]);
            assert_eq!((c.glyph, c.reasons.clone()), ("❌", vec![json!({"key": k, "msg": format!("say {k}")})]), "{k}");
        }
        // The agent's or waiting, it isn't; nor chrismo's behind another step.
        assert_eq!((glyph(&[step("bot", "agent")]), of(&[step("bot", "agent")]).reasons.len()), ("🔧", 0));
        assert_eq!((glyph(&[step("answer", "wait")]), of(&[step("answer", "wait")]).reasons.len()), ("⏳", 0));
        let behind = [step("push", "run"), step("bot", "you")];
        assert_eq!((glyph(&behind), of(&behind).reasons.len()), ("🔧", 0));
        // Alongside a red step, both notify, in NEXT's order.
        let c = of(&[step("bot", "you"), step("ci-red", "agent")]);
        assert_eq!(c.reasons, vec![json!({"key": "bot", "msg": "say bot"}), json!({"key": "ci-red", "msg": "say ci-red"})]);
        // Stuck until chrismo acts, they notify: mid-change on the branch,
        // GitHub's branch rules, a restack with no base to restack from.
        for k in ["take", "reset-dirty", "nothing-dirty", "blocked", "behind"] {
            let c = of(&[step(k, "you"), step("ci-wait", "wait")]);
            assert_eq!((c.glyph, c.reasons.clone()), ("❌", vec![json!({"key": k, "msg": format!("say {k}")})]), "{k}");
        }
        // End states and sd state unsure stay quiet and keep their glyph.
        for (k, g) in [("closed", "🔧"), ("ready", "🔧"), ("errors", "⚠️"), ("stale", "⚠️"), ("gh-wait", "⚠️")] {
            let c = of(&[step(k, "you"), step("ci-wait", "wait")]);
            assert_eq!((c.glyph, c.reasons.len()), (g, 0), "{k}");
        }
        assert_eq!(glyph(&[step("ready", "you")]), "✅");
        assert_eq!(glyph(&[step("closed", "you")]), "🚫");
    }

    #[test]
    fn classify_a_followed_prs_turns_are_chrismos_or_the_authors_250() {
        assert_eq!(glyph(&[step("submit", "you")]), "❌");
        assert_eq!(glyph(&[step("yours", "you")]), "❌");
        assert_eq!(glyph(&[step("author-wait", "wait")]), "⏳");
        assert_eq!(glyph(&[step("fetch", "run")]), "🔧");
    }

    #[test]
    fn classify_errors_or_stale_is_unsure() {
        for k in ["errors", "stale", "gh-wait"] {
            let c = of(&[step(k, "you"), step("ci-wait", "wait")]);
            assert_eq!((c.glyph, c.carry, c.reasons.len()), ("⚠️", true, 0), "{k}");
        }
    }

    #[test]
    fn classify_waiting_only_on_waits_else_work_for_later() {
        assert_eq!(glyph(&[step("ci-wait", "wait"), step("bot-wait", "wait")]), "⏳");
        assert_eq!(glyph(&[step("ci-wait", "wait"), step("push", "agent")]), "🔧");
        assert_eq!(glyph(&[step("bot", "agent")]), "🔧");
        // ready with anything else isn't ready.
        assert_eq!(glyph(&[step("ready", "you"), step("ci-wait", "wait")]), "🔧");
    }

    #[test]
    fn classify_merge_wait_or_attached_alone_keeps_ready() {
        for k in ["merge-wait", "attached"] {
            let c = of(&[step(k, "wait")]);
            assert_eq!((c.keep.clone(), c.carry, c.hold), (vec!["ready"], false, false), "{k}");
        }
        assert!(of(&[step("merge-wait", "wait"), step("ci-wait", "wait")]).keep.is_empty());
    }

    #[test]
    fn class_record_is_in_the_order_the_bash_wrote_it() {
        let rec = |c: &Class| json::to_string(&Value::Object(c.record()));
        assert_eq!(
            rec(&of(&[step("ready", "you")])),
            r#"{"carry":false,"hold":false,"keep":[],"glyph":"✅","reasons":[{"key":"ready","msg":"say ready"}]}"#
        );
        assert_eq!(rec(&of(&[])), r#"{"carry":true,"hold":false,"keep":[],"glyph":"⚠️","reasons":[],"note":"sd state gave no next steps"}"#);
    }

    #[test]
    fn dur_takes_the_largest_whole_unit() {
        for (s, want) in [(0, "0s"), (59, "59s"), (60, "1m"), (3599, "59m"), (3600, "1h"), (86399, "23h"), (86400, "1d"), (700000, "8d"), (-3600, "0s")] {
            assert_eq!(dur(s), want, "{s}");
        }
    }

    #[test]
    fn repo_state_dir_is_named_as_shasum_named_it() {
        // printf %s <path> | shasum
        assert_eq!(repo_state_dir("/s", "/Users/me/src/repo"), "/s/branches/repo-95369696");
        assert_eq!(repo_state_dir("/s", "/tmp/a b/é"), "/s/branches/é-c81eda9f");
        let o = proc::run("shasum", &[], None, Some(b"/x/y/sheepdog"));
        if o.ok {
            assert_eq!(repo_state_dir("/s", "/x/y/sheepdog"), format!("/s/branches/sheepdog-{}", &o.stdout[..8]));
        }
    }

    #[test]
    fn unsure_why_is_the_note_else_errors_and_stale_else_unreadable() {
        assert_eq!(unsure_why(Some("n"), &json!({"errors": ["e"]})), "n");
        assert_eq!(unsure_why(None, &json!({"errors": ["e1", "e2"], "stale": ["s"]})), "e1; e2; s");
        assert_eq!(unsure_why(None, &json!({"errors": [], "stale": [], "lag": ["l"]})), "l");
        assert_eq!(unsure_why(None, &json!({"errors": [{"odd": 1}]})), "its reasons couldn't be read");
        assert_eq!(unsure_why(None, &json!({"errors": "e"})), "its reasons couldn't be read");
    }

    #[test]
    fn notified_of_falls_back_to_the_reasons_of_an_older_file() {
        assert_eq!(notified_of(&json!({"notified": ["a"], "reasons": [{"key": "b"}]})), vec![json!("a")]);
        assert_eq!(notified_of(&json!({"reasons": [{"key": "b"}]})), vec![json!("b")]);
        assert!(notified_of(&json!(null)).is_empty());
    }

    #[test]
    fn local_time_is_dates_format() {
        let s = local_time();
        assert!(Regex::new(r"^\d{4}-\d\d-\d\d \d\d:\d\d:\d\d$").unwrap().is_match(&s), "{s}");
        // Local, not UTC: date's hour, read either side of it so an hour
        // boundary between them doesn't fail the test.
        let hour = || proc::run("date", &["+%H"], None, None).stdout.trim_end().to_string();
        let before = hour();
        let s = local_time();
        let after = hour();
        assert!(s[11..13] == before || s[11..13] == after, "{s} vs {before}..{after}");
    }
}
