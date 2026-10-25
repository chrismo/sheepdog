// The harness adapter: the per-harness code (#79 Design record, rule 3): the
// headless steps, the TUI sd act attach opens, and the tool-rule wording a
// step's prompt needs. `sd harness` runs a step or tallies a session from the
// command line; sd --help doesn't list it: tests/harness.rs drives run_step
// through it (#203).

use crate::git::{git_out, main_worktree};
use crate::val::{at, n, on, t, text};
use crate::{config, failed, json, limit, proc, running, time, usage};
use regex::Regex;
use serde_json::{json, Value};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

// Denied whatever the caller's tool lists allow: the pipeline commits, pushes
// and talks to GitHub.
const CLAUDE_DENY: &str = "Bash(git commit *),Bash(git push *),Bash(gh *)";

/// A read-only step's tools (review, describe), and pi's "read".
pub const READONLY_ALLOW: &str = "Read,Grep,Glob,Bash(git diff *),Bash(git log *),Bash(git show *),Bash(git blame *)";
pub const READONLY_PI_TOOLS: &str = "read";

/// An editing step's tools. Claude may read and edit; pi has no per-command
/// bash rules, so no bash at all.
pub const CLAUDE_ALLOW: &str = "Read,Edit,Write,Grep,Glob,Bash(git diff *),Bash(git log *),Bash(git show *),Bash(git status *)";
pub const PI_TOOLS: &str = "read,edit,write";

/// An editing step's claude tools with <rules> (Claude Code permission
/// rules, agent_rules) added, and its prompt's word on what the agent may
/// run. pi gets none of them.
pub fn tool_rules(h: &str, rules: &[String]) -> (String, String) {
    if h == "claude" && !rules.is_empty() {
        let said = "Beyond reading files and read-only git, you may run only these (Claude Code permission rules); anything else is refused:";
        return (format!("{CLAUDE_ALLOW},{}", rules.join(",")), format!("{said}\n{}", rules.join("\n")));
    }
    (CLAUDE_ALLOW.to_string(), "You can't run commands here beyond read-only git.".to_string())
}

/// A new session's id, as `uuidgen | tr 'A-F' 'a-f'` made it: a random
/// (version 4) UUID, lowercase.
pub fn new_session() -> String {
    let mut b = [0u8; 16];
    fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).expect("/dev/urandom reads");
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

/// One headless agent step: run_step's arguments.
pub struct Step<'a> {
    pub harness: &'a str,
    pub session: &'a str,
    /// new, resume or fork.
    pub mode: &'a str,
    /// The session a fork starts from; "" otherwise.
    pub parent: &'a str,
    /// Seconds; 0 for none.
    pub timeout: u64,
    pub prompt: &'a str,
    pub schema: &'a str,
    /// sd config step's object: model, effort, provider, args, and for
    /// claude useApiKey.
    pub settings: &'a Value,
    /// usage.sup's step; "" for none.
    pub name: &'a str,
    /// The branch, "" for the worktree's: a step that runs mid-rebase
    /// (restack --resolve) has a detached HEAD, so callers name it.
    pub branch: &'a str,
    pub claude_allow: &'a str,
    pub pi_tools: &'a str,
}

impl Step<'_> {
    /// The running marker's and failure record's step.
    fn label(&self) -> &str {
        if self.name.is_empty() { "agent" } else { self.name }
    }
}

/// Runs the step in the current directory: its result objects, or its status
/// with what went wrong on stderr. claude runs with ANTHROPIC_API_KEY unset
/// unless useApiKey is true: an exported key overrides the claude.ai login
/// (#68), and the one in a shell rc is often not the one to bill. pi can't
/// use the login, so it keeps the key. settings.stepBudget (USD, read for
/// the branch) caps a claude step through --max-budget-usd; 0 is no cap. pi
/// has no such flag: settings.branchBudget (sd state) is its only guard.
/// Every step appends a record to usage.sup. The harness's stderr (claude
/// can put protocol JSON there) shows only when the step fails, and then all
/// at once: nothing streams while it runs. While it runs, the branch's
/// running marker says so, for sd state (#170). A step the harness's usage
/// limit stopped isn't a failure (#269): it says so, with the reset, and
/// leaves the failure record (#242) as it was, and it pauses sd watch's agent
/// steps till the reset (crate::limit). The verbs still take any Err as their
/// step failing: they say so after the limit's line, and exit LIMIT_STATUS.
pub fn run_step(s: &Step) -> Result<Ran, Stop> {
    let b = match s.branch {
        "" => git_out(".", &["branch", "--show-current"]).unwrap_or_default(),
        b => b.to_string(),
    };
    if b.is_empty() {
        return agent(s, &b);
    }
    running::during(&b, s.label(), || agent(s, &b)).unwrap_or(Err(Stop::Failed(1)))
}

pub enum Stop {
    Failed(i32),
    Limit(Limit),
}

pub struct Limit {
    pub said: String,
    /// As time::utc writes it.
    pub resets: Option<String>,
}

/// sysexits' EX_TEMPFAIL: sd harness run's exit status and usage.sup's, and
/// any verb's whose agent step stopped on a usage limit.
pub const LIMIT_STATUS: i32 = 75;

static LIMITED: AtomicBool = AtomicBool::new(false);

/// The status a verb fails with: LIMIT_STATUS once one of its steps stopped
/// on a usage limit, else 1.
pub fn failed_status() -> i32 {
    if LIMITED.load(Ordering::Relaxed) {
        LIMIT_STATUS
    } else {
        1
    }
}

/// A step's result objects, and the session it ran in.
pub struct Ran {
    pub results: Vec<Value>,
    pub session: String,
}

impl Ran {
    /// The last result object, and the session.
    pub fn last(mut self) -> Option<(Value, String)> {
        self.results.pop().map(|r| (r, self.session))
    }
}

fn agent(s: &Step, b: &str) -> Result<Ran, Stop> {
    let (h, minted) = (s.harness, s.session);
    // Only the first line, as bash's read -ra took it.
    let args = t(s.settings, "args");
    let extra: Vec<&str> = args.lines().next().unwrap_or("").split_whitespace().collect();
    let mut typed = typed_flags(h, s.settings);
    let (mut cap, mut cap_note) = (0.0, String::new());
    let mut c = match h {
        "claude" => {
            let budget = config::get_text("settings.stepBudget", b, false).map_err(|lines| {
                for l in lines {
                    eprintln!("sd config: {l}");
                }
                eprintln!("sd config failed");
                Stop::Failed(1)
            })?;
            let (flags, c, note) = step_cap(&budget, &extra).map_err(|e| {
                eprintln!("{e}");
                Stop::Failed(1)
            })?;
            typed.extend(flags);
            (cap, cap_note) = (c, note);
            claude_command(s)
        }
        "pi" => {
            let mut c = Command::new("pi");
            c.arg("-p");
            if s.mode == "fork" {
                c.args(["--fork", s.parent]);
            }
            c.args(["--session-id", minted, "--tools", s.pi_tools]);
            c
        }
        _ => {
            eprintln!("unknown harness: {h} (claude or pi)");
            return Err(Stop::Failed(1));
        }
    };
    c.args(&typed).args(&extra);
    if h == "pi" {
        // pi has no schema flag: the prompt asks for a final ```json block.
        c.arg(format!(
            "{}\n\nEnd your reply with the result as one ```json fenced block matching this JSON Schema:\n{}",
            s.prompt, s.schema
        ));
    }

    // A fork's transcript, and claude's total_cost_usd, carry its parent's.
    let from = if s.mode == "fork" { s.parent } else { minted };
    let before = tally(h, from);
    let spent = usage::session_total(h, from);
    let t0 = Instant::now();
    // No timeout: as long as a u32 of seconds runs, as perl's alarm 0 did.
    // A longer one is cut to that too: a later deadline overflows Instant.
    let secs = if s.timeout == 0 { u64::from(u32::MAX) } else { s.timeout.min(u64::from(u32::MAX)) };
    let out = proc::timeout(secs, c, false);
    let result = match (out.ok, h) {
        (false, _) => Err(out.code),
        (true, "claude") => claude_result(&out.stdout).ok_or(1),
        (true, _) => objects(&fenced(&out.stdout)).ok_or(1),
    };
    let o = match objects(&out.stdout) {
        Some(mut os) if h == "claude" && os.len() == 1 => os.remove(0),
        _ => Value::Null,
    };
    let sid = ran_in(h, minted, &o);
    let limit = usage_limit(h, &o).map(|said| Limit {
        said,
        resets: transcript(h, &sid).and_then(|f| fs::read(f).ok()).and_then(|t| limit_resets(&String::from_utf8_lossy(&t))),
    });
    let status = match (&limit, &result) {
        (Some(_), _) => LIMIT_STATUS,
        (_, Err(c)) => *c,
        _ => 0,
    };
    let (now, repo, after) = (time::now_utc().0, main_worktree("."), tally(h, &sid));
    usage::append(&usage::record(&usage::Step {
        time: &now,
        repo: &repo,
        branch: b,
        step: s.name,
        harness: h,
        session: &sid,
        mode: s.mode,
        status,
        ms: t0.elapsed().as_millis() as i64,
        cap,
        prev: spent,
        stopped: out.stopped,
        o: &o,
        a: &after,
        b: &before,
    }));

    if let Some(l) = limit {
        let resets = l.resets.as_ref().map(|r| format!(" (resets {r})")).unwrap_or_default();
        let said = l.said.lines().next().unwrap_or_default();
        eprintln!("sd: {} stopped on {h}'s usage limit, not counted as a failure: {said}{resets}", s.label());
        limit::write(h, said, l.resets.as_deref());
        LIMITED.store(true, Ordering::Relaxed);
        return Err(Stop::Limit(l));
    }
    let said = if status != 0 { said(h, &out.stdout, &cap_note, spent) } else { String::new() };
    if status != 0 {
        eprint!("{}", tail(&out.stderr, 40));
        let said = said.trim_end_matches('\n');
        if !said.is_empty() {
            eprintln!("{}", tail(said, 20));
        }
    }
    if !b.is_empty() && result.is_ok() {
        failed::clear(b);
    } else if !b.is_empty() {
        failed::record(b, s.label(), h, &failed::why(out.timed_out, &said, &out.stderr, status));
    }
    result.map(|results| Ran { results, session: sid }).map_err(Stop::Failed)
}

/// claude's words when its result is a usage limit's refusal: api_error
/// usage_limit_reached (2.1.309), or 2.1.301's wording. A 429 alone is a
/// throttle. pi's is unchecked (#269).
fn usage_limit(h: &str, o: &Value) -> Option<String> {
    if h != "claude" || !on(o, "is_error") {
        return None;
    }
    let said = t(o, "result");
    if t(o, "api_error") == "usage_limit_reached" {
        return Some(if said.is_empty() { "usage_limit_reached".to_string() } else { said });
    }
    (said.starts_with("You've hit your") && said.contains("limit")).then_some(said)
}

/// claude's result doesn't carry the reset as a time; its transcript does.
fn limit_resets(transcript: &str) -> Option<String> {
    let v: Value = transcript.lines().rev().filter(|l| l.contains("usage_limit_reached")).find_map(|l| serde_json::from_str(l).ok())?;
    ["apiErrorParams.rate_limit_info.resetsAt", "quotaLimits.resetsAt"]
        .iter()
        .find_map(|p| at(&v, p).as_i64())
        .map(time::utc)
}

/// The session the step ran in: <minted>, unless claude reports another,
/// which is logged and taken (#69).
fn ran_in(h: &str, minted: &str, o: &Value) -> String {
    match t(o, "session_id") {
        r if !r.is_empty() && r != minted => {
            eprintln!("sd: {h} ran session {r}, not {minted}: going on in {r}");
            r
        }
        _ => minted.to_string(),
    }
}

/// claude's cap on a step: the flags to add, the cap in USD (0 for none) and
/// a note naming stepBudget as its source. A cap in args is the more specific
/// one, so it replaces stepBudget's. A cap that is negative, or in args
/// missing or not a number, is an error: 0 would silently mean no cap.
/// So is stepBudget not a number.
fn step_cap(budget: &str, extra: &[&str]) -> Result<(Vec<String>, f64, String), String> {
    let mut in_args = None;
    for (i, a) in extra.iter().enumerate() {
        if *a == "--max-budget-usd" {
            in_args = Some(extra.get(i + 1).copied().unwrap_or(""));
        } else if let Some(v) = a.strip_prefix("--max-budget-usd=") {
            in_args = Some(v);
        }
    }
    if let Some(v) = in_args {
        return match usd(v) {
            Some(c) => Ok((vec![], c, String::new())),
            None => Err(format!("--max-budget-usd in args must be 0 or more, not \"{v}\"")),
        };
    }
    if Regex::new(r"^0*(\.0*)?$").expect("a fixed pattern compiles").is_match(budget) {
        return Ok((vec![], 0.0, String::new()));
    }
    let Some(cap) = usd(budget).filter(|_| !budget.starts_with('-')) else {
        return Err(format!("settings.stepBudget must be 0 or more, not \"{budget}\""));
    };
    Ok((
        vec!["--max-budget-usd".to_string(), budget.to_string()],
        cap,
        format!(" (settings.stepBudget {budget} USD)"),
    ))
}

/// <v> as a finite amount of 0 or more; None otherwise.
fn usd(v: &str) -> Option<f64> {
    v.parse::<f64>().ok().filter(|c| c.is_finite() && *c >= 0.0)
}

/// The TUI on <sid>, for sd act attach: resumed or new, with the harness
/// block's model, effort, provider and args as its flags, the typed ones as
/// run_step passes them, and args split on whitespace. claude runs without
/// ANTHROPIC_API_KEY unless useApiKey, as run_step does (#68).
pub fn tui(h: &str, sid: &str, resume: bool, block: &Value) -> Result<Command, String> {
    let mut c = match h {
        "claude" => {
            let mut c = Command::new("claude");
            c.args([if resume { "--resume" } else { "--session-id" }, sid]);
            login_only(&mut c, block);
            c
        }
        "pi" => {
            let mut c = Command::new("pi");
            c.args(["--session-id", sid]);
            c
        }
        _ => return Err(format!("unknown harness: {h} (claude or pi)")),
    };
    c.args(typed_flags(h, block)).args(t(block, "args").split_whitespace());
    Ok(c)
}

/// <h>'s flags for the block's provider, model and effort, those it sets.
fn typed_flags(h: &str, block: &Value) -> Vec<String> {
    let table: &[(&str, &str)] = match h {
        "claude" => &[("--model", "model"), ("--effort", "effort")],
        "pi" => &[("--provider", "provider"), ("--model", "model"), ("--thinking", "effort")],
        _ => &[],
    };
    table
        .iter()
        .map(|&(f, k)| (f, t(block, k)))
        .filter(|(_, v)| !v.is_empty())
        .flat_map(|(f, v)| [f.to_string(), v])
        .collect()
}

/// claude on the claude.ai login: ANTHROPIC_API_KEY unset unless useApiKey.
fn login_only(c: &mut Command, block: &Value) {
    if !on(block, "useApiKey") {
        c.env_remove("ANTHROPIC_API_KEY");
    }
}

/// The harness wrote a transcript for <sid>: the session began.
pub fn session_exists(h: &str, sid: &str) -> bool {
    transcript(h, sid).is_some()
}

/// claude's command for the step, before the typed flags and args.
fn claude_command(s: &Step) -> Command {
    let mut c = Command::new("claude");
    // The prompt goes first: --allowedTools and --disallowedTools are
    // variadic and would swallow a trailing one.
    c.args(["-p", s.prompt]);
    match s.mode {
        "resume" => c.args(["--resume", s.session]),
        "fork" => c.args(["--resume", s.parent, "--fork-session", "--session-id", s.session]),
        _ => c.args(["--session-id", s.session]),
    };
    c.args(["--output-format", "json", "--json-schema", s.schema])
        .args(["--permission-mode", "acceptEdits", "--allowedTools", s.claude_allow, "--disallowedTools", CLAUDE_DENY]);
    login_only(&mut c, s.settings);
    c
}

/// The JSON values in <text>; None when it isn't JSON.
fn values(text: &str) -> Option<Vec<Value>> {
    serde_json::Deserializer::from_str(text).into_iter::<Value>().collect::<Result<_, _>>().ok()
}

/// The objects in <text>, a stream of JSON; None with none, or for text
/// that isn't JSON.
fn objects(text: &str) -> Option<Vec<Value>> {
    let os: Vec<Value> = values(text)?.into_iter().filter(Value::is_object).collect();
    (!os.is_empty()).then_some(os)
}

/// claude --output-format json's result object: structured_output, else the
/// result text parsed as JSON; None on an error result.
fn claude_result(out: &str) -> Option<Vec<Value>> {
    let rs: Vec<Value> = values(out)?
        .iter()
        .filter(|v| !on(v, "is_error"))
        .filter_map(|v| match at(v, "structured_output") {
            Value::Null => at(v, "result").as_str().and_then(|r| serde_json::from_str(r).ok()),
            so => Some(so.clone()),
        })
        .filter(Value::is_object)
        .collect();
    (!rs.is_empty()).then_some(rs)
}

/// The last closed ```json block in pi's reply.
fn fenced(out: &str) -> String {
    let (mut buf, mut last, mut inside) = (String::new(), String::new(), false);
    for l in out.trim_end_matches('\n').split('\n') {
        if l.starts_with("```json") {
            buf.clear();
            inside = true;
        } else if l.starts_with("```") {
            if inside {
                last = buf.clone();
            }
            inside = false;
        } else if inside {
            buf.push_str(l);
            buf.push('\n');
        }
    }
    last
}

/// The end of the harness's own account of a failure: the budget hit, else
/// claude's result text, else an error result's subtype (e.g.
/// error_max_turns), else stdout that isn't JSON; pi's reply.
fn said(h: &str, out: &str, cap_note: &str, spent: f64) -> String {
    let vs = match values(out) {
        Some(vs) if h == "claude" => vs,
        _ => return out.to_string(),
    };
    let one = |v: &Value| {
        let total = n(v, "total_cost_usd");
        if t(v, "subtype") == "error_max_budget_usd" {
            let session = if spent > 0.0 { format!(" (session total {} USD)", json::go_g(total)) } else { String::new() };
            return format!("hit the step budget{cap_note}: the step cost {} USD{session}", json::go_g(usage::step_cost(total, spent)));
        }
        match text(at(v, "result")) {
            r if !r.is_empty() => r,
            _ if on(v, "is_error") => t(v, "subtype"),
            _ => String::new(),
        }
    };
    let lines: Vec<String> = vs.iter().filter(|v| v.is_object()).map(one).filter(|s| !s.is_empty()).collect();
    lines.join("\n")
}

/// The last <n> lines of <s>, as tail -n prints them.
fn tail(s: &str, n: usize) -> &str {
    let body = s.strip_suffix('\n').unwrap_or(s);
    match body.rmatch_indices('\n').nth(n.saturating_sub(1)) {
        Some((i, _)) if n > 0 => &s[i + 1..],
        _ => s,
    }
}

/// The file the harness keeps <sid>'s transcript in, if it wrote one: the
/// first match two levels down, as find -maxdepth 2 looked.
fn transcript(h: &str, sid: &str) -> Option<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let dir = |var: &str, default: &str| {
        std::env::var(var).ok().filter(|d| !d.is_empty()).unwrap_or_else(|| format!("{home}/{default}"))
    };
    let (root, name) = match h {
        "claude" => (format!("{}/projects", dir("CLAUDE_CONFIG_DIR", ".claude")), format!("{sid}.jsonl")),
        "pi" => (dir("PI_CODING_AGENT_SESSION_DIR", ".pi/agent/sessions"), format!("_{sid}.jsonl")),
        _ => return None,
    };
    let hit = |p: &Path| {
        let f = p.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        if h == "pi" { f.ends_with(&name) } else { f == name }
    };
    find(Path::new(&root), 2, &hit)
}

fn find(dir: &Path, depth: usize, hit: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    let mut entries: Vec<fs::DirEntry> = fs::read_dir(dir).ok()?.flatten().collect();
    entries.sort_by_key(fs::DirEntry::file_name);
    entries.iter().find_map(|e| {
        let p = e.path();
        if hit(&p) {
            return Some(p);
        }
        let sub = depth > 1 && e.file_type().is_ok_and(|t| t.is_dir());
        sub.then(|| find(&p, depth - 1, hit)).flatten()
    })
}

/// What the session's transcript says, as one JSON object; {} with no
/// transcript. context is its last turn's tokens, the whole context then:
/// what a resume or fork inherits (#121). For claude, also entries, its
/// assistant lines: more after a step than before means the step ran turns.
/// For pi, which prints no usage, also cost, input, cache_create,
/// cache_read, output and turns, summed over the session. A line is read
/// only if it names the role: a transcript runs to megabytes. claude's
/// top-level type is the record's; a nested one is a subagent's progress,
/// and a quoted one a tool result's text. A "<synthetic>" model's entry is
/// claude's note of an API error (prompt too long), all zeros: no turn, and
/// no context. A line that isn't JSON (one being written) is skipped.
pub fn tally(h: &str, sid: &str) -> Value {
    match transcript(h, sid).and_then(|f| fs::read(f).ok()) {
        Some(text) => tally_of(h, &String::from_utf8_lossy(&text)),
        None => json!({}),
    }
}

fn tally_of(h: &str, text: &str) -> Value {
    let lines = |needle: &str| -> Vec<Value> {
        text.lines().filter(|l| l.contains(needle)).filter_map(|l| serde_json::from_str(l).ok()).collect()
    };
    let int = |x: f64| x as i64;
    if h == "claude" {
        let turns: Vec<Value> = lines(r#""type":"assistant""#)
            .into_iter()
            .filter(|v| {
                t(v, "type") == "assistant"
                    && !on(v, "isSidechain")
                    && !at(v, "message.usage").is_null()
                    && t(v, "message.model") != "<synthetic>"
            })
            .collect();
        let context = turns.last().map(|v| {
            ["input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens", "output_tokens"]
                .iter()
                .map(|k| int(n(v, &format!("message.usage.{k}"))))
                .sum::<i64>()
        });
        return json!({"entries": turns.len(), "context": context});
    }
    let turns: Vec<Value> = lines(r#""role":"assistant""#)
        .into_iter()
        .filter(|v| t(v, "message.role") == "assistant" && !at(v, "message.usage").is_null())
        .collect();
    let sum = |k: &str| (!turns.is_empty()).then(|| turns.iter().map(|v| n(v, &format!("message.usage.{k}"))).sum::<f64>());
    json!({"cost": sum("cost.total"), "input": sum("input").map(int), "cache_create": sum("cacheWrite").map(int),
           "cache_read": sum("cacheRead").map(int), "output": sum("output").map(int), "turns": turns.len(),
           "context": turns.last().map(|v| int(n(v, "message.usage.totalTokens")))})
}

fn usage_exit() -> ExitCode {
    eprintln!("usage: sd harness run <harness> <session-id> <new|resume|fork> <timeout> <prompt> <json-schema> [<settings-json>]");
    eprintln!("       sd harness tally <harness> <session-id>");
    ExitCode::from(1)
}

/// sd harness: run_step and tally. The step's name, its branch, a fork's
/// parent and the tool lists come in the environment: HARNESS_STEP,
/// HARNESS_BRANCH, HARNESS_PARENT, HARNESS_CLAUDE_ALLOW and HARNESS_PI_TOOLS,
/// the last two required.
pub fn run(argv: &[String]) -> ExitCode {
    let a: Vec<&str> = argv.iter().map(String::as_str).collect();
    match a.as_slice() {
        ["tally", h, sid] => {
            println!("{}", json::to_string(&tally(h, sid)));
            ExitCode::SUCCESS
        }
        ["run", h, sid, mode, timeout, prompt, schema, settings @ ..] if settings.len() <= 1 => {
            let settings = settings.first().copied().filter(|s| !s.is_empty()).unwrap_or("{}");
            let Some(settings) = serde_json::from_str::<Value>(settings).ok().filter(Value::is_object) else {
                eprintln!("sd harness: settings aren't a JSON object: {settings}");
                return ExitCode::from(1);
            };
            let Some(timeout) = timeout.parse::<f64>().ok().filter(|t| t.is_finite() && *t >= 0.0) else {
                eprintln!("sd harness: timeout isn't seconds: {timeout}");
                return ExitCode::from(1);
            };
            let env = |k: &str| std::env::var(k).unwrap_or_default();
            let (name, branch, parent) = (env("HARNESS_STEP"), env("HARNESS_BRANCH"), env("HARNESS_PARENT"));
            let (Ok(claude_allow), Ok(pi_tools)) = (std::env::var("HARNESS_CLAUDE_ALLOW"), std::env::var("HARNESS_PI_TOOLS")) else {
                eprintln!("sd harness: HARNESS_CLAUDE_ALLOW and HARNESS_PI_TOOLS must be set");
                return ExitCode::from(1);
            };
            let step = Step {
                harness: h,
                session: sid,
                mode,
                parent: &parent,
                timeout: timeout as u64,
                prompt,
                schema,
                settings: &settings,
                name: &name,
                branch: &branch,
                claude_allow: &claude_allow,
                pi_tools: &pi_tools,
            };
            match run_step(&step) {
                Ok(ran) => {
                    for r in ran.results {
                        println!("{}", json::to_string(&r));
                    }
                    ExitCode::SUCCESS
                }
                Err(stop) => {
                    let code = match stop {
                        Stop::Failed(code) => code,
                        Stop::Limit(_) => LIMIT_STATUS,
                    };
                    ExitCode::from(u8::try_from(code).unwrap_or(1))
                }
            }
        }
        _ => usage_exit(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Was lib/sheepdog.bats: "harness_tally: claude's context is its session's last turn, read from the transcript"
    #[test]
    fn tally_claudes_context_is_its_last_turn_not_a_subagents_or_a_tool_results() {
        let text = r#"{"type":"user","message":{"role":"user","content":"do it"}}
{"parentUuid":"a","isSidechain":false,"message":{"role":"assistant","usage":{"input_tokens":5,"cache_creation_input_tokens":1000,"cache_read_input_tokens":0,"output_tokens":50}},"type":"assistant"}
{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"{\"type\":\"assistant\"}"}]}}
{"parentUuid":"b","isSidechain":false,"message":{"role":"assistant","usage":{"input_tokens":2,"cache_creation_input_tokens":300,"cache_read_input_tokens":120000,"output_tokens":700}},"type":"assistant"}
{"type":"progress","data":{"message":{"type":"assistant","message":{"usage":{"input_tokens":999999}}}}}
{"isSidechain":true,"message":{"role":"assistant","usage":{"input_tokens":7}},"type":"assistant"}
{"type":"assistant","message":{"role":"assistant","usage":{"input_tokens":
"#;
        assert_eq!(json::to_string(&tally_of("claude", text)), r#"{"entries":2,"context":121002}"#);
        assert_eq!(json::to_string(&tally_of("claude", "")), r#"{"entries":0,"context":null}"#);
    }

    // Was lib/sheepdog.bats: "harness_tally: claude's synthetic entries (an API error, e.g. prompt too long) are neither the context nor a turn"
    #[test]
    fn tally_claudes_synthetic_entries_are_neither_the_context_nor_a_turn() {
        let text = r#"{"message":{"role":"assistant","model":"claude-opus","usage":{"input_tokens":2,"cache_read_input_tokens":190000,"output_tokens":8}},"type":"assistant"}
{"message":{"role":"assistant","model":"<synthetic>","usage":{"input_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":0}},"type":"assistant"}
"#;
        assert_eq!(json::to_string(&tally_of("claude", text)), r#"{"entries":1,"context":190010}"#);
    }

    // Was lib/sheepdog.bats: "harness_tally: pi's cost and tokens summed over its transcript, its context the last turn's"
    #[test]
    fn tally_pis_cost_and_tokens_summed_its_context_the_last_turns() {
        let text = r#"{"type":"session","id":"sid2"}
{"type":"message","message":{"role":"user","content":"do it"}}
{"type":"message","message":{"role":"assistant","usage":{"input":12937,"output":21,"cacheRead":0,"cacheWrite":0,"totalTokens":12958,"cost":{"total":0.002}}}}
{"type":"message","message":{"role":"assistant","usage":{"input":1473,"output":44,"cacheRead":11520,"cacheWrite":10,"totalTokens":13047,"cost":{"total":0.0005}}}}
"#;
        assert_eq!(
            json::to_string(&tally_of("pi", text)),
            r#"{"cost":0.0025,"input":14410,"cache_create":10,"cache_read":11520,"output":65,"turns":2,"context":13047}"#
        );
        // A transcript with no turns yet: what super's aggregate made of none.
        assert_eq!(
            json::to_string(&tally_of("pi", "{\"type\":\"session\",\"id\":\"sid2\"}\n")),
            r#"{"cost":null,"input":null,"cache_create":null,"cache_read":null,"output":null,"turns":0,"context":null}"#
        );
    }

    #[test]
    fn claude_result_is_structured_output_else_the_result_text_and_none_on_an_error() {
        let one = |out: &str| claude_result(out).map(|rs| rs.iter().map(json::to_string).collect::<Vec<_>>().join("\n"));
        assert_eq!(one(r#"{"type":"result","is_error":false,"structured_output":{"ok":true}}"#).as_deref(), Some(r#"{"ok":true}"#));
        assert_eq!(one(r#"{"type":"result","result":"{\"ok\": 1.0}"}"#).as_deref(), Some(r#"{"ok":1}"#));
        assert_eq!(one(r#"{"type":"result","is_error":true,"structured_output":{"ok":true}}"#), None);
        assert_eq!(one(r#"{"type":"result","subtype":"success","is_error":false,"result":""}"#), None);
        assert_eq!(one(r#"{"type":"result","result":"Sure, done."}"#), None);
        assert_eq!(one("not json"), None);
    }

    #[test]
    fn fenced_is_the_last_closed_json_block() {
        assert_eq!(fenced("Done.\n\n```json\n{\"ok\": true}\n```\n"), "{\"ok\": true}\n");
        assert_eq!(fenced("```json\n{\"a\": 1}\n```\nthen\n```json\n{\"b\": 2}\n```"), "{\"b\": 2}\n");
        assert_eq!(fenced("```json\n{\"a\": 1}\n```\n```json\n{\"unclosed\": 2}\n"), "{\"a\": 1}\n");
        assert_eq!(fenced("```\nplain\n```\n"), "");
    }

    #[test]
    fn said_names_the_budget_hit_else_the_result_else_the_subtype() {
        let budget = r#"{"type":"result","is_error":true,"subtype":"error_max_budget_usd","result":null,"total_cost_usd":5.25}"#;
        assert_eq!(
            said("claude", budget, " (settings.stepBudget 5 USD)", 4.5),
            "hit the step budget (settings.stepBudget 5 USD): the step cost 0.75 USD (session total 5.25 USD)"
        );
        assert_eq!(said("claude", budget, "", 0.0), "hit the step budget: the step cost 5.25 USD");
        assert_eq!(said("claude", r#"{"is_error":true,"result":"out of turns"}"#, "", 0.0), "out of turns");
        assert_eq!(said("claude", r#"{"is_error":true,"subtype":"error_max_turns"}"#, "", 0.0), "error_max_turns");
        assert_eq!(said("claude", r#"{"is_error":false,"subtype":"success","result":""}"#, "", 0.0), "");
        assert_eq!(said("claude", "Error: unknown option --bogus\n", "", 0.0), "Error: unknown option --bogus\n");
        assert_eq!(said("pi", "{\"a\": 1}", "", 0.0), "{\"a\": 1}");
    }

    #[test]
    fn usage_limit_is_claudes_error_result_typed_usage_limit_reached_or_in_the_words_it_gave_269() {
        let said = "You've hit your session limit · resets 12:20am (America/Chicago)";
        let observed = json!({"type": "result", "subtype": "success", "is_error": true, "result": said});
        assert_eq!(usage_limit("claude", &observed).as_deref(), Some(said));
        let weekly = json!({"is_error": true, "result": "You've hit your weekly limit · resets Mon 12:00am"});
        assert!(usage_limit("claude", &weekly).is_some());
        let typed = json!({"is_error": true, "api_error": "usage_limit_reached", "result": "Limit reached"});
        assert_eq!(usage_limit("claude", &typed).as_deref(), Some("Limit reached"));
        let untold = json!({"is_error": true, "api_error": "usage_limit_reached", "result": ""});
        assert_eq!(usage_limit("claude", &untold).as_deref(), Some("usage_limit_reached"));

        assert_eq!(usage_limit("claude", &json!({"is_error": true, "api_error_status": 429, "result": "Request rejected (429)"})), None);
        assert_eq!(usage_limit("claude", &json!({"is_error": true, "api_error_status": 400, "result": "Credit balance is too low"})), None);
        assert_eq!(usage_limit("claude", &json!({"is_error": false, "result": said})), None);
        assert_eq!(usage_limit("pi", &observed), None);
        assert_eq!(usage_limit("claude", &Value::Null), None);
    }

    #[test]
    fn limit_resets_is_the_last_usage_limit_entrys_reset_in_the_transcript_269() {
        let entry = |at: i64| {
            format!(r#"{{"type":"assistant","message":{{"model":"<synthetic>"}},"apiError":"usage_limit_reached","apiErrorParams":{{"rate_limit_info":{{"resetsAt":{at}}}}}}}"#)
        };
        let text = format!("{}\n{{\"type\":\"user\"}}\n{}\n", entry(1_792_110_000), entry(1_792_128_000));
        assert_eq!(limit_resets(&text).as_deref(), Some("2026-10-16T05:20:00Z"));
        let quota = r#"{"type":"assistant","apiError":"usage_limit_reached","quotaLimits":{"resetsAt":1792128000}}"#;
        assert_eq!(limit_resets(quota).as_deref(), Some("2026-10-16T05:20:00Z"));
        assert_eq!(limit_resets(r#"{"type":"assistant","apiError":"usage_limit_reached"}"#), None);
        assert_eq!(limit_resets("{\"type\":\"user\"}\n"), None);
        assert_eq!(limit_resets(""), None);
    }

    #[test]
    fn step_cap_is_args_else_step_budget_and_rejects_what_would_mean_no_cap_by_accident() {
        let flags = |b: &str, x: &[&str]| step_cap(b, x).map(|(f, c, _)| (f.join(" "), c));
        assert_eq!(flags("0", &[]), Ok((String::new(), 0.0)));
        assert_eq!(flags("", &[]), Ok((String::new(), 0.0)));
        assert_eq!(flags("2.5", &[]), Ok(("--max-budget-usd 2.5".to_string(), 2.5)));
        assert!(step_cap("2.5", &[]).unwrap().2.contains("settings.stepBudget 2.5 USD"));
        assert_eq!(flags("2.5", &["--max-budget-usd", "1"]), Ok((String::new(), 1.0)));
        assert_eq!(flags("2.5", &["--max-budget-usd=0"]), Ok((String::new(), 0.0)));
        assert!(flags("-1", &[]).unwrap_err().contains("stepBudget must be 0 or more"));
        assert!(flags("0", &["--max-budget-usd"]).is_err());
        assert!(flags("0", &["--max-budget-usd", "lots"]).is_err());
        assert!(flags("0", &["--max-budget-usd=-3"]).is_err());
        assert!(flags("0", &["--max-budget-usd", "inf"]).is_err());
        assert!(flags("abc", &[]).unwrap_err().contains("stepBudget must be 0 or more"));
        assert!(flags("1e", &[]).is_err());
        assert!(flags("inf", &[]).is_err());
    }

    #[test]
    fn claude_command_puts_the_prompt_before_the_variadic_tool_flags_and_unsets_the_api_key() {
        let settings = json!({});
        let step = |settings| Step {
            harness: "claude",
            session: "sid",
            mode: "resume",
            parent: "",
            timeout: 0,
            prompt: "do it",
            schema: "{}",
            settings,
            name: "",
            branch: "",
            claude_allow: "Read,Edit",
            pi_tools: "",
        };
        let c = claude_command(&step(&settings));
        let args: Vec<_> = c.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(&args[..4], ["-p", "do it", "--resume", "sid"]);
        let pos = |f: &str| args.iter().position(|a| a == f).unwrap();
        assert!(pos("-p") < pos("--allowedTools") && pos("-p") < pos("--disallowedTools"));
        assert_eq!(args[pos("--allowedTools") + 1], "Read,Edit");
        assert_eq!(args[pos("--disallowedTools") + 1], CLAUDE_DENY);
        assert!(c.get_envs().any(|(k, v)| k == "ANTHROPIC_API_KEY" && v.is_none()));

        let keep = json!({"useApiKey": true});
        assert!(!claude_command(&step(&keep)).get_envs().any(|(k, _)| k == "ANTHROPIC_API_KEY"));
    }

    fn argv_of(c: &Command) -> String {
        c.get_args().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>().join(" ")
    }

    // Was libexec/sd-act's harness_flags: the typed flags in provider, model,
    // effort order, then args split on whitespace.
    #[test]
    fn tui_flags_are_the_harness_blocks_typed_as_run_step_passes_them() {
        let block = json!({"provider": "cf", "model": "opus", "effort": "high", "args": "--verbose \t--fast\n--x"});
        let c = tui("claude", "sid", true, &block).unwrap();
        assert_eq!(c.get_program(), "claude");
        assert_eq!(argv_of(&c), "--resume sid --model opus --effort high --verbose --fast --x");
        let c = tui("pi", "sid", true, &block).unwrap();
        assert_eq!(c.get_program(), "pi");
        assert_eq!(argv_of(&c), "--session-id sid --provider cf --model opus --thinking high --verbose --fast --x");
    }

    #[test]
    fn tui_starts_a_new_session_by_id_and_leaves_out_what_the_block_doesnt_set() {
        assert_eq!(argv_of(&tui("claude", "sid", false, &json!({})).unwrap()), "--session-id sid");
        assert_eq!(argv_of(&tui("claude", "sid", false, &Value::Null).unwrap()), "--session-id sid");
        assert_eq!(argv_of(&tui("pi", "sid", false, &json!({"model": "", "args": " "})).unwrap()), "--session-id sid");
        assert_eq!(tui("codex", "sid", false, &json!({})).err().as_deref(), Some("unknown harness: codex (claude or pi)"));
    }

    #[test]
    fn tui_claude_drops_the_api_key_unless_use_api_key_and_pi_keeps_it() {
        let drops = |h: &str, block: &Value| {
            tui(h, "sid", true, block).unwrap().get_envs().any(|(k, v)| k == "ANTHROPIC_API_KEY" && v.is_none())
        };
        assert!(drops("claude", &json!({})));
        assert!(!drops("claude", &json!({"useApiKey": true})));
        assert!(!drops("pi", &json!({})));
    }

    #[test]
    fn tool_rules_add_claudes_rules_and_name_them_and_pi_gets_none() {
        let rules = vec!["Bash(test -f file.txt)".to_string(), "WebFetch".to_string()];
        let (allow, said) = tool_rules("claude", &rules);
        assert_eq!(allow, format!("{CLAUDE_ALLOW},Bash(test -f file.txt),WebFetch"));
        assert!(said.ends_with("anything else is refused:\nBash(test -f file.txt)\nWebFetch"), "{said}");
        let none = (CLAUDE_ALLOW.to_string(), "You can't run commands here beyond read-only git.".to_string());
        assert_eq!(tool_rules("pi", &rules), none);
        assert_eq!(tool_rules("claude", &[]), none);
    }

    #[test]
    fn new_session_is_a_lowercase_v4_uuid_and_new_each_time() {
        let re = Regex::new(r"^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$").unwrap();
        let (a, b) = (new_session(), new_session());
        assert!(re.is_match(&a), "{a}");
        assert_ne!(a, b);
    }

    #[test]
    fn tail_keeps_the_last_lines_as_tail_n_does() {
        assert_eq!(tail("a\nb\nc\n", 2), "b\nc\n");
        assert_eq!(tail("a\nb\nc", 2), "b\nc");
        assert_eq!(tail("a\nb\n", 5), "a\nb\n");
        assert_eq!(tail("", 5), "");
    }
}
