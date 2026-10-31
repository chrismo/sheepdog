// sd harness probe (#347, C7 in docs/arch-review/findings.md): the installed
// harnesses against the internals sheepdog reads and no harness promises.
// Its steps aren't a branch's: no usage.sup record, running marker or
// failure record.

use super::{
    new_session, outcome, step_cap, step_command, tail, tally, transcript, typed_flags, Step, READONLY_ALLOW,
    READONLY_PI_TOOLS, VERIFIED,
};
use crate::val::{at, n, t};
use crate::{config, proc};
use regex::Regex;
use serde_json::{json, Value};
use std::process::ExitCode;

const HARNESSES: [&str; 2] = ["claude", "pi"];

/// The words each harness's --help must still have: every flag its step and
/// TUI commands pass (a unit test holds the list to them), and the
/// permission mode a claude step runs in.
const HELP_WORDS: [(&str, &[&str]); 2] = [
    (
        "claude",
        &[
            "--session-id",
            "--resume",
            "--fork-session",
            "--output-format",
            "--json-schema",
            "--permission-mode",
            "acceptEdits",
            "--allowedTools",
            "--disallowedTools",
            "--max-budget-usd",
            "--model",
            "--effort",
        ],
    ),
    ("pi", &["--session-id", "--fork", "--tools", "--provider", "--model", "--thinking"]),
];

const PROMPT: &str = "This is a probe of the harness. Do nothing else: reply with the result {\"ok\": true}.";
const SCHEMA: &str = r#"{"type":"object","properties":{"ok":{"type":"boolean"}},"required":["ok"]}"#;
/// claude's cap on each probe step, in USD.
const BUDGET: &str = "1";
const TIMEOUT: u64 = 300;

struct Report<'a> {
    h: &'a str,
    failed: bool,
}

impl Report<'_> {
    fn ok(&self, check: &str) {
        println!("{}: ok: {check}", self.h);
    }

    fn fail(&mut self, check: &str, why: &str) {
        println!("{}: FAIL: {check}: {why}", self.h);
        self.failed = true;
    }
}

/// sd harness probe [<harness>...]: those named, else every installed one.
pub fn run(names: &[&str]) -> ExitCode {
    if let Some(h) = names.iter().find(|h| !HARNESSES.contains(h)) {
        println!("unknown harness: {h} (claude or pi)");
        return ExitCode::from(1);
    }
    let named = !names.is_empty();
    let hs: Vec<&str> = if named { names.to_vec() } else { HARNESSES.to_vec() };
    let (mut failed, mut probed) = (false, 0);
    for h in hs {
        let mut r = Report { h, failed: false };
        match version(h) {
            None if named => r.fail("not installed", &format!("no {h} on PATH")),
            None => println!("{h}: not installed, skipped"),
            Some(v) => {
                probed += 1;
                probe(&mut r, &v);
            }
        }
        failed |= r.failed;
    }
    if probed == 0 && !named {
        println!("no harness installed (claude or pi)");
        failed = true;
    }
    ExitCode::from(u8::from(failed))
}

fn probe(r: &mut Report, v: &str) {
    let h = r.h;
    match VERIFIED.iter().find(|(name, _)| *name == h) {
        Some((_, ok)) if *ok == v => println!("{h} {v}: the version last verified"),
        Some((_, ok)) => println!("{h} {v}: verified against {ok}; once the checks pass, bump harness::VERIFIED"),
        None => {}
    }
    flags(r);
    let block = match config::get_global(&format!("harnesses.{h}"), true) {
        Ok(text) => serde_json::from_str(&text).unwrap_or(Value::Null),
        Err(lines) if lines.is_empty() => Value::Null,
        Err(lines) => return r.fail("config", &lines.join("; ")),
    };
    let Ok(dir) = temp_dir() else { return r.fail("step", "no temp dir to run it in") };
    if let Some(parent) = step(r, &block, &dir, None) {
        step(r, &block, &dir, Some(&parent));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// <h> --version's x.y.z; None when <h> won't run.
fn version(h: &str) -> Option<String> {
    let out = proc::run(h, &["--version"], None, None);
    if out.code == 127 && out.stdout.is_empty() {
        return None;
    }
    let re = Regex::new(r"\d+\.\d+\.\d+").expect("a fixed pattern compiles");
    Some(re.find(&out.stdout).map_or_else(|| out.stdout.trim().to_string(), |m| m.as_str().to_string()))
}

fn flags(r: &mut Report) {
    let h = r.h;
    let out = proc::run(h, &["--help"], None, None);
    let help = format!("{}{}", out.stdout, out.stderr);
    let words = HELP_WORDS.iter().find(|(name, _)| *name == h).map_or(&[][..], |(_, w)| w);
    let lacks: Vec<&str> = words.iter().copied().filter(|w| !has_word(&help, w)).collect();
    if lacks.is_empty() {
        r.ok("flags");
    } else {
        r.fail("flags", &format!("{h} --help lacks {}", lacks.join(" ")));
    }
}

/// <w> in <text>, not as part of a longer flag or word.
fn has_word(text: &str, w: &str) -> bool {
    let re = Regex::new(&format!(r"(^|[^\w-]){}($|[^\w-])", regex::escape(w))).expect("an escaped pattern compiles");
    re.is_match(text)
}

fn temp_dir() -> std::io::Result<String> {
    let d = std::env::temp_dir().join(format!("sd-harness-probe-{}", new_session()));
    std::fs::create_dir(&d)?;
    Ok(d.display().to_string())
}

/// What a step left: the session, its result object (claude) and its tally.
struct Left {
    sid: String,
    o: Value,
    tally: Value,
}

/// One trivial step in <dir>, new or a fork of <parent>, checked: the
/// session it left, for a fork to start from, if it passed.
fn step(r: &mut Report, block: &Value, dir: &str, parent: Option<&Left>) -> Option<Left> {
    let check = if parent.is_some() { "fork" } else { "step" };
    let sid = new_session();
    let s = Step {
        harness: r.h,
        session: &sid,
        mode: if parent.is_some() { "fork" } else { "new" },
        parent: parent.map_or("", |p| p.sid.as_str()),
        timeout: TIMEOUT,
        prompt: PROMPT,
        schema: SCHEMA,
        settings: block,
        name: "probe",
        branch: "",
        claude_allow: READONLY_ALLOW,
        pi_tools: READONLY_PI_TOOLS,
    };
    let why = match ran(&s, dir) {
        Ok(left) => match parent.map_or(Ok(()), |p| carries(r.h, p, &left)) {
            Ok(()) => {
                r.ok(check);
                return Some(left);
            }
            Err(why) => why,
        },
        Err(why) => why,
    };
    r.fail(check, &why);
    None
}

/// Runs <s> in <dir> as run_step would, and checks what it left.
fn ran(s: &Step, dir: &str) -> Result<Left, String> {
    let h = s.harness;
    let args = t(s.settings, "args");
    let extra: Vec<&str> = args.lines().next().unwrap_or("").split_whitespace().collect();
    let mut typed = typed_flags(h, s.settings);
    if h == "claude" {
        typed.extend(step_cap(BUDGET, &extra)?.0);
    }
    let mut c = step_command(s, &typed, &extra)?;
    c.current_dir(dir);
    let out = proc::timeout(s.timeout, c, false);
    let (result, o) = outcome(h, &out);
    if !out.ok {
        return Err(format!("exit {}: {}", out.code, brief(&format!("{}{}", out.stderr, out.stdout))));
    }
    let ok = json!({"ok": true});
    if result.ok().and_then(|mut rs| rs.pop()) != Some(ok.clone()) {
        return Err(format!("no {ok} result in its output: {}", brief(&out.stdout)));
    }
    if h == "claude" {
        result_object(&o, s.session)?;
    }
    if transcript(h, s.session).is_none() {
        return Err(format!("no transcript for {} where {h} keeps them", s.session));
    }
    let tally = tally(h, s.session);
    tallied(h, &tally)?;
    Ok(Left { sid: s.session.to_string(), o, tally })
}

/// The last lines of a step's output, each cut short: a harness's error can
/// be a page of JSON on one line.
fn brief(out: &str) -> String {
    let cut = |l: &str| match l.char_indices().nth(200) {
        Some((i, _)) => format!("{}…", &l[..i]),
        None => l.to_string(),
    };
    tail(out, 5).trim_end().lines().map(cut).collect::<Vec<_>>().join("\n")
}

/// claude's result object has what run_step and usage.sup read.
fn result_object(o: &Value, sid: &str) -> Result<(), String> {
    if o.is_null() {
        return Err("stdout isn't one JSON result object".to_string());
    }
    let said = t(o, "session_id");
    if said != sid {
        return Err(format!("the result's session_id is {said}, not {sid}"));
    }
    if !at(o, "total_cost_usd").is_number() {
        return Err(format!("the result has no total_cost_usd: {o}"));
    }
    Ok(())
}

/// The tally's count of turns.
fn turns(h: &str) -> &'static str {
    if h == "claude" { "entries" } else { "turns" }
}

fn tallied(h: &str, tally: &Value) -> Result<(), String> {
    let mut need = vec![turns(h), "context"];
    if h == "pi" {
        need.push("cost");
    }
    let bad: Vec<&str> = need.iter().copied().filter(|k| n(tally, k) <= 0.0).collect();
    if bad.is_empty() {
        Ok(())
    } else {
        Err(format!("the transcript's tally lacks {}: {tally}", bad.join(" ")))
    }
}

/// A fork carries its parent's: its transcript has more turns, and claude's
/// total_cost_usd is the session's running total, not the step's.
fn carries(h: &str, parent: &Left, fork: &Left) -> Result<(), String> {
    let k = turns(h);
    let (p, f) = (n(&parent.tally, k), n(&fork.tally, k));
    if f <= p {
        return Err(format!("its transcript has {f} {k}, its parent's {p}: it doesn't carry the parent's"));
    }
    let (p, f) = (n(&parent.o, "total_cost_usd"), n(&fork.o, "total_cost_usd"));
    if h == "claude" && f < p {
        return Err(format!("its total_cost_usd {f} is under its parent's {p}: not a running total"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::tui;
    use super::*;

    fn flags_of(c: &std::process::Command) -> Vec<String> {
        c.get_args().map(|a| a.to_string_lossy().into_owned()).filter(|a| a.starts_with("--")).collect()
    }

    #[test]
    fn help_words_name_every_flag_the_step_and_tui_commands_pass() {
        let block = json!({"provider": "cf", "model": "m", "effort": "high"});
        for h in HARNESSES {
            let words = HELP_WORDS.iter().find(|(name, _)| *name == h).unwrap().1;
            let mut typed = typed_flags(h, &block);
            if h == "claude" {
                typed.extend(step_cap("1", &[]).unwrap().0);
            }
            for mode in ["new", "resume", "fork"] {
                let s = Step {
                    harness: h,
                    session: "sid",
                    mode,
                    parent: "p",
                    timeout: 0,
                    prompt: "",
                    schema: "{}",
                    settings: &block,
                    name: "",
                    branch: "",
                    claude_allow: "",
                    pi_tools: "",
                };
                let mut passed = flags_of(&step_command(&s, &typed, &[]).unwrap());
                passed.extend(flags_of(&tui(h, "sid", mode == "resume", &block).unwrap()));
                for f in passed {
                    assert!(words.contains(&f.as_str()), "{h} passes {f}, which HELP_WORDS doesn't check");
                }
            }
        }
    }

    #[test]
    fn has_word_wants_the_whole_flag() {
        assert!(has_word("  --fork <id>  Fork", "--fork"));
        assert!(has_word("--fork", "--fork"));
        assert!(!has_word("  --fork-session  ", "--fork"));
        assert!(has_word("(choices: \"acceptEdits\", \"auto\")", "acceptEdits"));
        assert!(!has_word("--model-x", "--model"));
        assert!(has_word("--allowedTools, --allowed-tools <tools...>", "--allowedTools"));
    }
}
