// Black-box tests of sd harness probe (#347, src/harness/probe.rs): the
// built binary, run as sd-harness through a link (tests/common). claude and
// pi are fakes that keep the internals the probe checks, as the installed
// ones did when harness::VERIFIED was last bumped: their --version and
// --help, a result object (claude) or fenced reply (pi), and a transcript
// that a fork copies. A knob file in $FX breaks one of them. PATH is the
// fakes and the system's bin dirs only: the real harnesses are never run.

mod common;

use common::{assert_has, assert_lacks, Run, Sd};
use sheepdog::harness::VERIFIED;
use std::fs;

const CLAUDE: &str = r#"#!/usr/bin/env bash
case "$1" in
  --version) echo "$(cat "$FX/claude-version") (Claude Code)"; exit;;
  --help) cat "$FX/claude-help"; exit;;
esac
printf '%s\n' "$@" >> "$FX/claude-args"
pwd > "$FX/claude-cwd"
sid= parent=
while [ $# -gt 0 ]; do
  case "$1" in
    --session-id) sid=$2; shift;;
    --resume) parent=$2; shift;;
  esac
  shift
done
d="$CLAUDE_CONFIG_DIR/projects/-tmp-probe"
mkdir -p "$d"
cents=1
if [ -n "$parent" ]; then
  cat "$d/$parent.jsonl" > "$d/$sid.jsonl"
  cents=$(( $(cat "$FX/cents-$parent") + 1 ))
fi
echo "$cents" > "$FX/cents-$sid"
[ -e "$FX/no-transcript" ] ||
  echo '{"type":"assistant","isSidechain":false,"message":{"role":"assistant","model":"claude-x","usage":{"input_tokens":3,"cache_creation_input_tokens":100,"cache_read_input_tokens":0,"output_tokens":5}}}' >> "$d/$sid.jsonl"
said=$(cat "$FX/claude-sid" 2>/dev/null || echo "$sid")
printf '{"type":"result","subtype":"success","is_error":false,"result":"","structured_output":{"ok":true},"session_id":"%s","total_cost_usd":0.%02d}\n' "$said" "$cents"
"#;

const PI: &str = r#"#!/usr/bin/env bash
case "$1" in
  --version) cat "$FX/pi-version"; exit;;
  --help) cat "$FX/pi-help"; exit;;
esac
printf '%s\n' "$@" >> "$FX/pi-args"
sid= parent=
while [ $# -gt 0 ]; do
  case "$1" in
    --session-id) sid=$2; shift;;
    --fork) parent=$2; shift;;
  esac
  shift
done
d="$PI_CODING_AGENT_SESSION_DIR/--tmp-probe--"
mkdir -p "$d"
f="$d/2026-10-31T00-00-00-000Z_$sid.jsonl"
if [ -n "$parent" ]; then cat "$d"/*_"$parent".jsonl > "$f"; else echo "{\"type\":\"session\",\"id\":\"$sid\"}" > "$f"; fi
echo '{"type":"message","message":{"role":"assistant","usage":{"input":10,"output":2,"cacheRead":0,"cacheWrite":0,"totalTokens":12,"cost":{"total":0.001}}}}' >> "$f"
if [ -e "$FX/pi-fails" ]; then cat "$FX/pi-fails" >&2; exit 1; fi
if [ -e "$FX/pi-reply" ]; then cat "$FX/pi-reply"; else printf 'Done.\n\n```json\n{"ok": true}\n```\n'; fi
"#;

const CLAUDE_HELP: &str = "Usage: claude [options] [command] [prompt]
  --allowedTools, --allowed-tools <tools...>  Allowed tools
  --disallowedTools, --disallowed-tools <tools...>  Denied tools
  --effort <level>               Effort level
  --fork-session                 When resuming, create a new session ID
  --json-schema <schema>         JSON Schema for structured output
  --max-budget-usd <amount>      Maximum dollar amount to spend
  --model <model>                Model for the current session
  --output-format <format>       (choices: \"text\", \"json\", \"stream-json\")
  --permission-mode <mode>       (choices: \"acceptEdits\", \"auto\", \"plan\")
  -p, --print                    Print response and exit
  -r, --resume [value]           Resume a conversation by session ID
  --session-id <uuid>            Use a specific session ID
";

const PI_HELP: &str = "Usage: pi [options] [messages...]
  --provider <name>              Provider name
  --model <pattern>              Model pattern or ID
  --print, -p                    Non-interactive mode
  --session-id <id>              Use exact project session ID
  --fork <path|id>               Fork specific session file into a new session
  --tools, -t <tools>            Comma-separated allowlist of tool names
  --thinking <level>             Set thinking level
";

fn verified(h: &str) -> &'static str {
    VERIFIED.iter().find(|(name, _)| *name == h).map(|(_, v)| *v).unwrap()
}

/// The fakes on a PATH without the real harnesses, at their verified
/// versions; their transcripts in the temp dir; a global config whose
/// harness blocks set a model.
fn sd() -> Sd {
    let mut sd = Sd::for_verb("harness");
    sd.env = [
        ("PATH", format!("{}:/usr/bin:/bin", sd.path("fakebin"))),
        ("SHEEPDOG_STATE_DIR", sd.path("state")),
        ("CLAUDE_CONFIG_DIR", sd.path("claude")),
        ("PI_CODING_AGENT_SESSION_DIR", sd.path("pi")),
        ("FX", sd.path("fx")),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    fs::create_dir(sd.tmp.join("fx")).unwrap();
    sd.fake("claude", CLAUDE);
    sd.fake("pi", PI);
    sd.fx("claude-version", verified("claude"));
    sd.fx("pi-version", verified("pi"));
    sd.fx("claude-help", CLAUDE_HELP);
    sd.fx("pi-help", PI_HELP);
    sd.write_global(r#"{"harnesses": {"claude": {"model": "opus"}, "pi": {"provider": "cf", "model": "kimi"}}}"#);
    sd
}

trait Fx {
    fn fx(&self, name: &str, text: &str);
    fn fx_read(&self, name: &str) -> String;
    fn probe(&self, args: &[&str]) -> Run;
}

impl Fx for Sd {
    fn fx(&self, name: &str, text: &str) {
        fs::write(self.tmp.join("fx").join(name), format!("{text}\n")).unwrap();
    }

    fn fx_read(&self, name: &str) -> String {
        fs::read_to_string(self.tmp.join("fx").join(name)).unwrap_or_default()
    }

    fn probe(&self, args: &[&str]) -> Run {
        self.run(&[&["probe"], args].concat())
    }
}

#[test]
fn probe_passes_harnesses_that_keep_the_internals_sheepdog_depends_on() {
    let sd = sd();
    let r = sd.probe(&[]);
    assert_eq!(r.status, 0, "{}", r.output);
    for h in ["claude", "pi"] {
        assert_has(
            &r.output,
            &[&format!("{h} {}: the version last verified", verified(h)), &format!("{h}: ok: flags"), &format!("{h}: ok: step"), &format!("{h}: ok: fork")],
        );
    }
    assert_lacks(&r.output, "FAIL");
    // The flags run_step passes, the harness block's typed ones among them.
    let args = sd.fx_read("claude-args");
    for f in ["--json-schema", "--permission-mode\nacceptEdits", "--fork-session", "--max-budget-usd", "--model\nopus"] {
        assert_has(&args, &[f]);
    }
    assert_has(&sd.fx_read("pi-args"), &["--fork", "--tools\nread", "--provider\ncf", "--model\nkimi"]);
    // Not in the repo it was run from: a temp dir, gone after.
    let cwd = sd.fx_read("claude-cwd");
    assert_lacks(&cwd, &sd.tmp.display().to_string());
    assert!(!std::path::Path::new(cwd.trim_end()).exists(), "{cwd}");
}

#[test]
fn probe_names_the_harnesses_given_and_only_those() {
    let sd = sd();
    let r = sd.probe(&["pi"]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_has(&r.output, &["pi: ok: fork"]);
    assert_lacks(&r.output, "claude");
    assert_eq!(sd.fx_read("claude-args"), "");

    let r = sd.probe(&["codex"]);
    assert_eq!(r.status, 1);
    assert_has(&r.output, &["unknown harness: codex (claude or pi)"]);
}

#[test]
fn probe_skips_a_harness_not_installed_but_fails_one_named() {
    let sd = sd();
    fs::remove_file(sd.tmp.join("fakebin/pi")).unwrap();
    let r = sd.probe(&[]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_has(&r.output, &["claude: ok: fork", "pi: not installed, skipped"]);

    let r = sd.probe(&["pi"]);
    assert_eq!(r.status, 1, "{}", r.output);
    assert_has(&r.output, &["pi: FAIL: not installed"]);

    fs::remove_file(sd.tmp.join("fakebin/claude")).unwrap();
    let r = sd.probe(&[]);
    assert_eq!(r.status, 1, "{}", r.output);
    assert_has(&r.output, &["no harness installed (claude or pi)"]);
}

#[test]
fn probe_says_when_the_version_isnt_the_one_last_verified() {
    let sd = sd();
    sd.fx("claude-version", "9.9.9");
    let r = sd.probe(&["claude"]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_has(
        &r.output,
        &[&format!("claude 9.9.9: verified against {}; once the checks pass, bump harness::VERIFIED", verified("claude")), "claude: ok: fork"],
    );
}

#[test]
fn probe_fails_a_harness_whose_help_lacks_a_flag_sheepdog_passes() {
    let sd = sd();
    sd.fx("claude-help", &CLAUDE_HELP.replace("--fork-session", "--fork").replace("acceptEdits", "accept"));
    let r = sd.probe(&["claude"]);
    assert_eq!(r.status, 1, "{}", r.output);
    assert_has(&r.output, &["claude: FAIL: flags: claude --help lacks --fork-session acceptEdits"]);
}

#[test]
fn probe_fails_a_claude_result_that_names_another_session() {
    let sd = sd();
    sd.fx("claude-sid", "someone-else");
    let r = sd.probe(&["claude"]);
    assert_eq!(r.status, 1, "{}", r.output);
    assert_has(&r.output, &["claude: FAIL: step: the result's session_id is someone-else, not "]);
    assert_lacks(&r.output, "claude: ok: fork");
}

#[test]
fn probe_fails_a_step_that_leaves_no_transcript() {
    let sd = sd();
    sd.fx("no-transcript", "");
    let r = sd.probe(&["claude"]);
    assert_eq!(r.status, 1, "{}", r.output);
    assert_has(&r.output, &["claude: FAIL: step: no transcript for "]);
}

#[test]
fn probe_fails_a_pi_reply_without_the_fenced_result() {
    let sd = sd();
    sd.fx("pi-reply", "Done, no block.");
    let r = sd.probe(&["pi"]);
    assert_eq!(r.status, 1, "{}", r.output);
    assert_has(&r.output, &["pi: FAIL: step: no {\"ok\":true} result in its output: Done, no block."]);
}

#[test]
fn probe_shows_the_start_of_each_of_a_failed_steps_last_lines() {
    let sd = sd();
    sd.fx("pi-fails", &format!("Warning: no session\n{{\"error\":\"quota exceeded{}\"}}", " and more".repeat(100)));
    let r = sd.probe(&["pi"]);
    assert_eq!(r.status, 1, "{}", r.output);
    assert_has(&r.output, &["pi: FAIL: step: exit 1: Warning: no session\n{\"error\":\"quota exceeded and more", "…"]);
    assert!(r.output.lines().all(|l| l.chars().count() < 300), "{}", r.output);
}
