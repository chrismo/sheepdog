// Black-box tests of one headless agent step, src/harness.rs's run_step
// (#203): the built binary, run as sd-harness through a link (tests/common),
// in a repo on feat one commit past main. `sd harness run` is run_step from
// the command line, its tool lists in HARNESS_CLAUDE_ALLOW and
// HARNESS_PI_TOOLS, the step's name and branch in HARNESS_STEP and
// HARNESS_BRANCH; bin/sd doesn't list it. Moved from lib/sheepdog.bats;
// each test keeps the bats test's title above it. harness_tally's tests are
// src/harness.rs's unit tests.
//
// claude and pi are stubs that put protocol noise on stderr and print
// $FX/out on stdout, exiting $FX/status.

mod common;

use common::{assert_has, assert_lacks, assert_match, output, spawn, Run, Sd};
use serde_json::{json, Value};
use sheepdog::harness::{CLAUDE_ALLOW, PI_TOOLS};
use sheepdog::running::proc_start;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

const STUB: &str = r#"#!/usr/bin/env bash
echo '{"type":"system","subtype":"init","session_id":"x"}' >&2
echo "${ANTHROPIC_API_KEY-unset}" > "$FX/key"
printf '%s\n' "$@" > "$FX/args"
for m in "$(git rev-parse --path-format=absolute --git-common-dir)"/sheepdog/*.running; do
  [ -e "$m" ] && { echo "${m##*/} $(cat "$m")" > "$FX/marker"; kill -0 "$(cut -d' ' -f1 "$m")" && echo alive >> "$FX/marker"; }
done
while [ -e "$FX/hold" ]; do sleep 0.1; done
cat "$FX/out"
exit "$(cat "$FX/status" 2>/dev/null || echo 0)"
"#;

const PI_OK: &str = "Done.\n\n```json\n{\"ok\": true}\n```\n";

/// The bats setup and harness_stubs: repo (cwd) on feat, one commit past
/// main; usage.sup and the harnesses' transcripts in the temp dir; the stubs
/// on PATH, printing a claude result of {"ok": true}.
fn sd() -> Sd {
    let mut sd = Sd::for_verb("harness");
    sd.env = [
        ("SHEEPDOG_STATE_DIR", sd.path("state")),
        ("CLAUDE_CONFIG_DIR", sd.path("claude")),
        ("PI_CODING_AGENT_SESSION_DIR", sd.path("pi")),
        ("FX", sd.path("fx")),
        ("HARNESS_CLAUDE_ALLOW", CLAUDE_ALLOW.to_string()),
        ("HARNESS_PI_TOOLS", PI_TOOLS.to_string()),
        // Set when sheepdog runs these tests from its own step.
        ("HARNESS_STEP", String::new()),
        ("HARNESS_BRANCH", String::new()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    fs::create_dir(sd.tmp.join("fx")).unwrap();
    sd.fake("claude", STUB);
    sd.fake("pi", STUB);
    sd.fx("out", r#"{"type": "result", "is_error": false, "structured_output": {"ok": true}}"#);
    sd.git(&["init", "-q", "repo"]);
    sd.cd("repo");
    sd.git(&["commit", "-q", "--allow-empty", "-m", "init"]);
    sd.git(&["switch", "-qc", "feat"]);
    sd.git(&["commit", "-q", "--allow-empty", "-m", "one"]);
    sd
}

impl Sd {
    /// `<env> run --separate-stderr harness_run <args>`
    fn step(&self, env: &[(&str, &str)], args: &[&str]) -> Run {
        let mut c = self.command(&self.bin);
        c.arg("run").args(args).envs(env.iter().copied());
        sep(&mut c)
    }

    fn fx(&self, name: &str, text: &str) {
        fs::write(self.tmp.join("fx").join(name), format!("{text}\n")).unwrap();
    }

    fn fx_read(&self, name: &str) -> String {
        fs::read_to_string(self.tmp.join("fx").join(name)).unwrap_or_default()
    }

    fn fx_exists(&self, name: &str) -> bool {
        self.tmp.join("fx").join(name).exists()
    }

    fn fx_rm(&self, name: &str) {
        fs::remove_file(self.tmp.join("fx").join(name)).unwrap();
    }

    /// $FX/args' lines.
    fn args(&self) -> Vec<String> {
        self.fx_read("args").lines().map(str::to_string).collect()
    }

    /// `grep -A1 -x -- <flag> "$FX/args" | tail -1`
    fn after(&self, flag: &str) -> String {
        let a = self.args();
        let i = a.iter().rposition(|l| l == flag).unwrap_or_else(|| panic!("no {flag} in {a:?}"));
        a.get(i + 1).cloned().unwrap_or_default()
    }

    /// claude_out <total_cost_usd>: a claude result with that session total.
    fn claude_out(&self, total: &str) {
        self.fx("out", &format!(r#"{{"type": "result", "is_error": false, "structured_output": {{"ok": true}}, "total_cost_usd": {total}}}"#));
    }

    fn usage_sup(&self) -> PathBuf {
        self.tmp.join("state/usage.sup")
    }

    /// usage.sup's records.
    fn usage(&self) -> Vec<Value> {
        let text = fs::read_to_string(self.usage_sup()).unwrap_or_default();
        text.lines()
            .map(|l| {
                let r = sheepdog::usage::sup_record(l).unwrap_or_else(|| panic!("not a usage record: {l}"));
                Value::Object(r.into_iter().collect())
            })
            .collect()
    }

    fn usage_count(&self) -> usize {
        self.usage().len()
    }

    /// usage_last: <keys> of usage.sup's last record, joined by |; null for
    /// a null.
    fn usage_last(&self, keys: &[&str]) -> String {
        let u = self.usage().pop().expect("a usage record");
        keys.iter()
            .map(|k| match &u[k] {
                Value::String(s) => s.clone(),
                v => v.to_string(),
            })
            .collect::<Vec<_>>()
            .join("|")
    }

    fn last(&self) -> Value {
        self.usage().pop().expect("a usage record")
    }

    fn marker_file(&self) -> PathBuf {
        self.cwd.join(".git/sheepdog/feat.running")
    }

    /// The .running markers left in the repo.
    fn markers(&self) -> Vec<String> {
        let Ok(d) = fs::read_dir(self.cwd.join(".git/sheepdog")) else { return vec![] };
        d.flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".running"))
            .collect()
    }

    /// claude_stub_appends <session-id> <transcript-line|"">: a claude stub
    /// that adds the line to the session's transcript, then fails without a
    /// result, as a step killed by stepTimeout does.
    fn claude_stub_appends(&self, sid: &str, line: &str) {
        let t = self.tmp.join(format!("claude/projects/-repo/{sid}.jsonl"));
        fs::create_dir_all(t.parent().unwrap()).unwrap();
        fs::write(&t, "{\"message\":{\"role\":\"assistant\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1}},\"type\":\"assistant\"}\n")
            .unwrap();
        self.fx("append", line);
        self.fake(
            "claude",
            &format!(
                "#!/usr/bin/env bash\nprintf '%s\\n' \"$@\" > \"$FX/args\"\ngrep . \"$FX/append\" >> '{}'\necho \"timed out after 30s\" >&2\nexit 124\n",
                t.display()
            ),
        );
    }
}

/// `run --separate-stderr` of <c>.
fn sep(c: &mut Command) -> Run {
    let out = output(c, None);
    Run {
        status: out.status.code().unwrap_or(-1),
        output: common::chomp(&String::from_utf8_lossy(&out.stdout)),
        stderr: common::chomp(&String::from_utf8_lossy(&out.stderr)),
    }
}

const STEP: [&str; 6] = ["claude", "sid1", "new", "30", "do it", "{}"];

// @test "harness_run: claude runs without ANTHROPIC_API_KEY, so the claude.ai login is used"
#[test]
fn harness_run_claude_runs_without_anthropic_api_key_so_the_claude_ai_login_is_used() {
    let sd = sd();
    let r = sd.step(&[("ANTHROPIC_API_KEY", "sk-no-credit")], &STEP);
    assert_eq!(r.status, 0);
    assert_eq!(sd.fx_read("key"), "unset\n");
}

// @test "harness_run: harnesses.claude.useApiKey keeps the key"
#[test]
fn harness_run_harnesses_claude_useapikey_keeps_the_key() {
    let sd = sd();
    let key = [("ANTHROPIC_API_KEY", "sk-paid")];
    let r = sd.step(&key, &[&STEP[..], &[r#"{"harness": "claude", "useApiKey": true}"#]].concat());
    assert_eq!(r.status, 0);
    assert_eq!(sd.fx_read("key"), "sk-paid\n");
    sd.step(&key, &[&STEP[..], &[r#"{"harness": "claude", "useApiKey": false}"#]].concat());
    assert_eq!(sd.fx_read("key"), "unset\n");
}

// @test "harness_run: pi keeps the key; it can't use the claude.ai login"
#[test]
fn harness_run_pi_keeps_the_key_it_cant_use_the_claude_ai_login() {
    let sd = sd();
    sd.fx("out", PI_OK);
    let r = sd.step(&[("ANTHROPIC_API_KEY", "sk-paid")], &["pi", "sid1", "new", "30", "do it", "{}"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.fx_read("key"), "sk-paid\n");
}

// @test "harness_run: settings.stepBudget caps a claude step's spend, 5 USD by default; 0 is no cap"
#[test]
fn harness_run_settings_stepbudget_caps_a_claude_steps_spend_5_usd_by_default_0_is_no_cap() {
    let sd = sd();
    let r = sd.step(&[], &STEP);
    assert_eq!(r.status, 0);
    assert_eq!(sd.after("--max-budget-usd"), "5");
    sd.write_global(r#"{"settings": {"stepBudget": 2.5}}"#);
    let r = sd.step(&[], &STEP);
    assert_eq!(r.status, 0);
    assert_eq!(sd.after("--max-budget-usd"), "2.5");
    sd.write_global(r#"{"settings": {"stepBudget": 0}}"#);
    let r = sd.step(&[], &STEP);
    assert_eq!(r.status, 0);
    assert_lacks(&sd.fx_read("args"), "--max-budget-usd");
}

// @test "harness_run: a --max-budget-usd in harnesses.claude.args replaces stepBudget's"
#[test]
fn harness_run_a_max_budget_usd_in_harnesses_claude_args_replaces_stepbudgets() {
    let sd = sd();
    let r = sd.step(&[], &[&STEP[..], &[r#"{"harness": "claude", "args": "--verbose --max-budget-usd 10"}"#]].concat());
    assert_eq!(r.status, 0);
    assert_eq!(sd.args().iter().filter(|l| *l == "--max-budget-usd").count(), 1);
    assert_eq!(sd.after("--max-budget-usd"), "10");
    let r = sd.step(&[], &[&STEP[..], &[r#"{"harness": "claude", "args": "--max-budget-usd=10"}"#]].concat());
    assert_eq!(r.status, 0);
    assert!(!sd.args().iter().any(|l| l == "--max-budget-usd"));
    assert!(sd.args().iter().any(|l| l == "--max-budget-usd=10"));
}

// @test "harness_run: pi has no budget flag"
#[test]
fn harness_run_pi_has_no_budget_flag() {
    let sd = sd();
    sd.write_global(r#"{"settings": {"stepBudget": 2.5}}"#);
    sd.fx("out", PI_OK);
    let r = sd.step(&[], &["pi", "sid1", "new", "30", "do it", "{}"]);
    assert_eq!(r.status, 0);
    assert_lacks(&sd.fx_read("args"), "budget");
}

// @test "harness_run: a step that hits the budget fails and says so"
#[test]
fn harness_run_a_step_that_hits_the_budget_fails_and_says_so() {
    let sd = sd();
    sd.write_global(r#"{"settings": {"stepBudget": 0.1}}"#);
    sd.fx(
        "out",
        r#"{"type": "result", "is_error": true, "subtype": "error_max_budget_usd", "result": null, "total_cost_usd": 0.1353}"#,
    );
    let r = sd.step(&[], &STEP);
    assert_ne!(r.status, 0);
    assert_has(&r.stderr, &["hit the step budget (settings.stepBudget 0.1 USD)"]);
    assert_has(&r.stderr, &["0.1353"]);
    // A cap from harnesses.claude.args instead: no stepBudget to blame.
    fs::remove_file(sd.cfg()).unwrap();
    let r = sd.step(&[], &[&STEP[..], &[r#"{"harness": "claude", "args": "--max-budget-usd 0.1"}"#]].concat());
    assert_ne!(r.status, 0);
    assert_has(&r.stderr, &["hit the step budget"]);
    assert_lacks(&r.stderr, "settings.stepBudget");
}

// @test "harness_run: a negative stepBudget, or an unreadable config, fails the step before it runs"
#[test]
fn harness_run_a_negative_stepbudget_or_an_unreadable_config_fails_the_step_before_it_runs() {
    let sd = sd();
    sd.write_global(r#"{"settings": {"stepBudget": -1}}"#);
    let r = sd.step(&[], &STEP);
    assert_ne!(r.status, 0);
    assert_has(&r.stderr, &["settings.stepBudget: expected a number of 0 or more, got -1"]);
    assert!(!sd.fx_exists("args"));
    sd.write_global("not json");
    let r = sd.step(&[], &STEP);
    assert_ne!(r.status, 0);
    assert!(!sd.fx_exists("args"));
}

// @test "harness_run: the tool lists are the caller's; sd harness run without them doesn't run the step"
// Retitled: the bats title named the bash lib, whose lists went with it.
#[test]
fn harness_run_the_tool_lists_are_the_callers_sd_harness_run_without_them_doesnt_run_the_step() {
    let sd = sd();
    let r = sd.step(&[("HARNESS_CLAUDE_ALLOW", "Read,Grep")], &STEP);
    assert_eq!(r.status, 0);
    assert!(sd.args().iter().any(|l| l == "Read,Grep"));
    sd.fx_rm("args");
    let mut c = sd.command(&sd.bin);
    c.env_remove("HARNESS_CLAUDE_ALLOW").env("HARNESS_PI_TOOLS", "read").arg("run").args(STEP);
    let r = sep(&mut c);
    assert_ne!(r.status, 0);
    assert!(!sd.fx_exists("args"));
    assert_has(&r.stderr, &["HARNESS_CLAUDE_ALLOW"]);
}

// @test "harness_run: every claude step appends a typed record to usage.sup, one line each"
#[test]
fn harness_run_every_claude_step_appends_a_typed_record_to_usage_sup_one_line_each() {
    let sd = sd();
    sd.fx(
        "out",
        r#"{"type": "result", "is_error": false, "structured_output": {"ok": true}, "total_cost_usd": 0.25, "duration_ms": 4321, "num_turns": 3,
         "usage": {"input_tokens": 10, "cache_creation_input_tokens": 200, "cache_read_input_tokens": 3000, "output_tokens": 40}}"#,
    );
    let r = sd.step(&[("HARNESS_STEP", "implement")], &["claude", "sid1", "resume", "30", "do it", "{}"]);
    assert_eq!(r.status, 0);
    let text = fs::read_to_string(sd.usage_sup()).unwrap();
    assert_eq!(text.lines().count(), 1);
    assert_match(&text, r"^\{time:\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?Z,");
    let u = sd.last();
    assert!(u["cost_usd"].is_f64() && u["input"].is_i64() && u["estimated"] == false, "{u}");
    assert_eq!(
        sd.usage_last(&[
            "repo", "branch", "step", "harness", "session", "mode", "status", "cost_usd", "input", "cache_create", "cache_read",
            "output", "duration_ms", "turns"
        ]),
        format!("{}|feat|implement|claude|sid1|resume|0|0.25|10|200|3000|40|4321|3", sd.path("repo"))
    );
    sd.step(&[], &["claude", "sid1", "resume", "30", "again", "{}"]);
    assert_eq!(fs::read_to_string(sd.usage_sup()).unwrap().lines().count(), 2);
    assert_eq!(sd.usage_last(&["branch", "step", "session"]), "feat|null|sid1");
}

// @test "harness_run: a resumed claude step's cost is what its session's total gained, the total kept beside it (#150)"
#[test]
fn harness_run_a_resumed_claude_steps_cost_is_what_its_sessions_total_gained_the_total_kept_beside_it_150() {
    let sd = sd();
    let cost = ["cost_usd", "total_cost_usd"];
    // claude's total_cost_usd on a resume is the session's running total.
    sd.claude_out("0.25");
    sd.step(&[], &["claude", "sid1", "new", "30", "do it", "{}"]);
    assert_eq!(sd.usage_last(&cost), "0.25|0.25");
    sd.claude_out("0.75");
    sd.step(&[], &["claude", "sid1", "resume", "30", "again", "{}"]);
    assert_eq!(sd.usage_last(&cost), "0.5|0.75");
    // Another session's total isn't this one's.
    sd.claude_out("0.125");
    sd.step(&[], &["claude", "sid2", "new", "30", "other", "{}"]);
    assert_eq!(sd.usage_last(&cost), "0.125|0.125");
    // A step with no result knows no total; the next one counts from the last known.
    sd.fx("status", "1");
    sd.fx("out", "not json");
    sd.step(&[], &["claude", "sid1", "resume", "30", "crash", "{}"]);
    assert_eq!(sd.usage_last(&cost), "null|null");
    sd.fx_rm("status");
    sd.claude_out("1.5");
    sd.step(&[], &["claude", "sid1", "resume", "30", "more", "{}"]);
    assert_eq!(sd.usage_last(&cost), "0.75|1.5");
    // The branch's spend is the steps', not the totals' (2.625): sd state sums cost_usd (src/state.rs).
    let spend: f64 = sd.usage().iter().filter(|u| u["branch"] == "feat").filter_map(|u| u["cost_usd"].as_f64()).sum();
    assert_eq!(spend, 1.625);
}

// @test "harness_run: a session's total that went down isn't a running total; the step is charged all of it (#150)"
#[test]
fn harness_run_a_sessions_total_that_went_down_isnt_a_running_total_the_step_is_charged_all_of_it_150() {
    let sd = sd();
    sd.claude_out("0.5");
    sd.step(&[], &["claude", "sid1", "new", "30", "do it", "{}"]);
    sd.claude_out("0.2");
    sd.step(&[], &["claude", "sid1", "resume", "30", "again", "{}"]);
    assert_eq!(sd.usage_last(&["cost_usd", "total_cost_usd"]), "0.2|0.2");
}

// @test "harness_run: a usage.sup row from before #150 counts as its session's total"
#[test]
fn harness_run_a_usage_sup_row_from_before_150_counts_as_its_sessions_total() {
    let sd = sd();
    fs::create_dir_all(sd.tmp.join("state")).unwrap();
    fs::write(
        sd.usage_sup(),
        format!(
            "{{time:2026-10-16T20:33:06Z,repo:\"{}\",branch:\"feat\",step:\"implement\",harness:\"claude\",session:\"sid1\",mode:\"new\",status:0,cost_usd:0.34,estimated:false}}\n",
            sd.path("repo")
        ),
    )
    .unwrap();
    sd.claude_out("0.58");
    sd.step(&[], &["claude", "sid1", "resume", "30", "again", "{}"]);
    assert_eq!(sd.usage_last(&["cost_usd", "total_cost_usd"]), "0.24|0.58");
}

// @test "harness_run: a resumed step that hits the budget says what the step cost, and the session's total (#150)"
#[test]
fn harness_run_a_resumed_step_that_hits_the_budget_says_what_the_step_cost_and_the_sessions_total_150() {
    let sd = sd();
    sd.claude_out("4.5");
    sd.step(&[], &["claude", "sid1", "new", "30", "do it", "{}"]);
    sd.fx(
        "out",
        r#"{"type": "result", "is_error": true, "subtype": "error_max_budget_usd", "result": null, "total_cost_usd": 5.25}"#,
    );
    let r = sd.step(&[], &["claude", "sid1", "resume", "30", "again", "{}"]);
    assert_ne!(r.status, 0);
    assert_has(
        &r.stderr,
        &["hit the step budget (settings.stepBudget 5 USD): the step cost 0.75 USD (session total 5.25 USD)"],
    );
    assert_eq!(sd.usage_last(&["cost_usd", "total_cost_usd"]), "0.75|5.25");
}

// @test "harness_run: HARNESS_BRANCH names the branch when the worktree is detached (restack --resolve)"
#[test]
fn harness_run_harness_branch_names_the_branch_when_the_worktree_is_detached_restack_resolve() {
    let sd = sd();
    sd.git(&["switch", "-q", "--detach"]);
    let r = sd.step(&[("HARNESS_BRANCH", "feat")], &["claude", "sid1", "resume", "30", "do it", "{}"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.usage_last(&["branch"]), "feat");
    sd.step(&[], &["claude", "sid1", "resume", "30", "do it", "{}"]);
    assert_eq!(sd.usage_count(), 2);
    assert_eq!(sd.usage_last(&["branch"]), "null");
}

// @test "harness_run: a failed step is logged with its status, and pi's record has no cost without a transcript"
#[test]
fn harness_run_a_failed_step_is_logged_with_its_status_and_pis_record_has_no_cost_without_a_transcript() {
    let sd = sd();
    sd.fx("status", "1");
    let r = sd.step(&[], &STEP);
    assert_ne!(r.status, 0);
    assert_eq!(sd.usage_last(&["harness", "session", "mode", "status"]), "claude|sid1|new|1");
    sd.fx_rm("status");
    sd.fx("out", PI_OK);
    let r = sd.step(&[], &["pi", "sid2", "new", "30", "do it", "{}"]);
    assert_eq!(r.status, 0);
    assert_eq!(
        sd.usage_last(&["harness", "session", "status", "cost_usd", "input", "context"]),
        "pi|sid2|0|null|null|null"
    );
}

// @test "harness_run: while the step runs, the branch's marker names the step, a live pid and the start time; gone after (#170)"
#[test]
fn harness_run_while_the_step_runs_the_branchs_marker_names_the_step_a_live_pid_and_the_start_time_gone_after_170() {
    let sd = sd();
    let r = sd.step(&[("HARNESS_STEP", "implement")], &STEP);
    assert_eq!(r.status, 0);
    let marker = sd.fx_read("marker");
    assert_match(marker.lines().next().unwrap_or_default(), r"^feat\.running [0-9]+ implement 20[0-9-]+T[0-9:]+Z [A-Z].* [0-9]{4}$");
    assert_eq!(marker.lines().nth(1).unwrap_or_default(), "alive");
    assert!(sd.markers().is_empty());
}

// @test "harness_run: the marker is per branch, HARNESS_BRANCH's when set, / made __ (#170)"
#[test]
fn harness_run_the_marker_is_per_branch_harness_branchs_when_set_made_170() {
    let sd = sd();
    sd.step(&[("HARNESS_BRANCH", "team/x")], &["pi", "sid1", "new", "30", "do it", "{}"]);
    let head = sd.fx_read("marker").lines().next().unwrap_or_default().to_string();
    let f: Vec<&str> = head.split(' ').collect();
    assert_eq!(format!("{} {}", f[0], f[2]), "team__x.running agent");
    assert!(sd.markers().is_empty());
}

// @test "harness_run: a failed step removes its marker too (#170)"
#[test]
fn harness_run_a_failed_step_removes_its_marker_too_170() {
    let sd = sd();
    sd.fx("status", "1");
    let r = sd.step(&[], &STEP);
    assert_ne!(r.status, 0);
    assert!(!sd.fx_read("marker").is_empty());
    assert!(sd.markers().is_empty());
}

// @test "harness_run: an interrupted step removes its marker (#170)"
#[test]
fn harness_run_an_interrupted_step_removes_its_marker_170() {
    let sd = sd();
    sd.fx("hold", "");
    let mut c = sd.command(&sd.bin);
    c.arg("run").args(STEP).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    let mut job = spawn(&mut c);
    for _ in 0..100 {
        if !sd.fx_read("marker").is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let pid: i32 = sd.fx_read("marker").split(' ').nth(1).unwrap_or_default().parse().unwrap();
    assert!(!sd.markers().is_empty());
    unsafe { libc::kill(pid, libc::SIGTERM) };
    sd.fx_rm("hold");
    let _ = job.wait();
    assert!(sd.markers().is_empty());
}

// @test "running_hold: the marker lasts until running_release; harness_run in between leaves it alone (#170)"
// The harness_run half; running::holding's is in tests/act_start.rs's and
// tests/act_ci_fix.rs's "running marker lasts" tests.
#[test]
fn running_hold_the_marker_lasts_until_running_release_harness_run_in_between_leaves_it_alone_170() {
    let sd = sd();
    let me = std::process::id().to_string();
    let held = format!("{me} implement 2026-10-18T14:03:00Z {}", proc_start(&me));
    fs::create_dir_all(sd.cwd.join(".git/sheepdog")).unwrap();
    fs::write(sd.marker_file(), format!("{held}\n")).unwrap();
    let r = sd.step(&[("RUNNING_HELD", "feat"), ("HARNESS_STEP", "resolve")], &STEP);
    assert_eq!(r.status, 0);
    let head = sd.fx_read("marker").lines().next().unwrap_or_default().to_string();
    assert_eq!(head.split_once(' ').map(|(_, rest)| rest), Some(held.as_str()));
    assert_eq!(fs::read_to_string(sd.marker_file()).unwrap(), format!("{held}\n"));
}

// @test "running_hold and harness_run: another live step's marker is refused and left alone; a dead one's is replaced (#170)"
// The harness_run half; src/running.rs's unit tests have the rest.
#[test]
fn running_hold_and_harness_run_another_live_steps_marker_is_refused_and_left_alone_a_dead_ones_is_replaced_170() {
    let sd = sd();
    fs::create_dir_all(sd.cwd.join(".git/sheepdog")).unwrap();
    let mut holder = spawn(Command::new("sleep").arg("30"));
    let pid = holder.id().to_string();
    let theirs = format!("{pid} implement 2026-10-18T14:03:00Z {}\n", proc_start(&pid));
    fs::write(sd.marker_file(), &theirs).unwrap();
    let r = sd.step(&[("HARNESS_STEP", "review")], &STEP);
    assert_ne!(r.status, 0);
    assert_has(&r.stderr, &[&format!("implement is running on feat (pid {pid}, since 2026-10-18T14:03:00Z)")]);
    assert!(!sd.fx_exists("args"));
    assert_eq!(fs::read_to_string(sd.marker_file()).unwrap(), theirs);
    holder.kill().unwrap();
    holder.wait().unwrap();
    let r = sd.step(&[("HARNESS_STEP", "address")], &STEP);
    assert_eq!(r.status, 0);
    assert_match(&sd.fx_read("marker"), r"^feat\.running [0-9]+ address ");
    assert!(sd.markers().is_empty());
}

// @test "harness_run: a claude step that got only an API error isn't charged its cap"
#[test]
fn harness_run_a_claude_step_that_got_only_an_api_error_isnt_charged_its_cap() {
    let sd = sd();
    sd.claude_stub_appends(
        "sid1",
        r#"{"message":{"role":"assistant","model":"<synthetic>","usage":{"input_tokens":0,"output_tokens":0}},"type":"assistant"}"#,
    );
    let r = sd.step(&[], &["claude", "sid1", "resume", "30", "do it", "{}"]);
    assert_ne!(r.status, 0);
    assert_eq!(sd.usage_last(&["cost_usd", "estimated"]), "null|false");
}

// @test "harness_run: a pi step's usage is what its transcript gained during the step"
#[test]
fn harness_run_a_pi_steps_usage_is_what_its_transcript_gained_during_the_step() {
    let sd = sd();
    let t = sd.tmp.join("pi/--repo--/2026-10-17T00-00-00-000Z_sid2.jsonl");
    fs::create_dir_all(t.parent().unwrap()).unwrap();
    fs::write(
        &t,
        "{\"type\":\"message\",\"message\":{\"role\":\"assistant\",\"usage\":{\"input\":100,\"output\":10,\"cacheRead\":0,\"cacheWrite\":0,\"totalTokens\":110,\"cost\":{\"total\":0.5}}}}\n",
    )
    .unwrap();
    // The stub's step adds a turn to the transcript.
    sd.fx(
        "append",
        r#"{"type":"message","message":{"role":"assistant","usage":{"input":20,"output":5,"cacheRead":100,"cacheWrite":0,"totalTokens":125,"cost":{"total":0.25}}}}"#,
    );
    sd.fake("pi", &format!("#!/usr/bin/env bash\ncat \"$FX/append\" >> '{}'\ncat \"$FX/out\"\n", t.display()));
    sd.fx("out", PI_OK);
    let r = sd.step(&[], &["pi", "sid2", "resume", "30", "do it", "{}"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.usage_last(&["cost_usd", "input", "cache_read", "output", "turns", "context"]), "0.25|20|100|5|1|125");
    assert!(sd.last()["duration_ms"].as_i64().is_some_and(|ms| ms >= 0));
}

// @test "harness_run: a claude record carries its session's context, from the transcript"
#[test]
fn harness_run_a_claude_record_carries_its_sessions_context_from_the_transcript() {
    let sd = sd();
    fs::create_dir_all(sd.tmp.join("claude/projects/-repo")).unwrap();
    fs::write(
        sd.tmp.join("claude/projects/-repo/sid1.jsonl"),
        "{\"message\":{\"role\":\"assistant\",\"usage\":{\"input_tokens\":1,\"cache_creation_input_tokens\":2,\"cache_read_input_tokens\":3,\"output_tokens\":4}},\"type\":\"assistant\"}\n",
    )
    .unwrap();
    let r = sd.step(&[], &STEP);
    assert_eq!(r.status, 0);
    assert_eq!(sd.usage_last(&["context"]), "10");
}

const TURN: &str = r#"{"message":{"role":"assistant","usage":{"input_tokens":9,"output_tokens":9}},"type":"assistant"}"#;

// @test "harness_run: a claude step that ran turns but printed no result (timed out) is charged its cap, marked estimated"
#[test]
fn harness_run_a_claude_step_that_ran_turns_but_printed_no_result_timed_out_is_charged_its_cap_marked_estimated() {
    let sd = sd();
    sd.claude_stub_appends("sid1", TURN);
    let r = sd.step(&[], &["claude", "sid1", "resume", "30", "do it", "{}"]);
    assert_ne!(r.status, 0);
    let u = sd.last();
    assert!(u["cost_usd"].is_f64(), "{u}");
    assert_eq!((u["cost_usd"].as_f64(), &u["estimated"], &u["input"]), (Some(5.0), &json!(true), &Value::Null));
    // With no cap there's nothing to charge: unknown, but still marked.
    sd.write_global(r#"{"settings": {"stepBudget": 0}}"#);
    sd.step(&[], &["claude", "sid1", "resume", "30", "do it", "{}"]);
    assert_eq!(sd.usage_last(&["cost_usd", "estimated"]), "null|true");
}

// @test "harness_run: a timed-out claude step whose cap came from harnesses.claude.args is charged that cap"
#[test]
fn harness_run_a_timed_out_claude_step_whose_cap_came_from_harnesses_claude_args_is_charged_that_cap() {
    let sd = sd();
    sd.claude_stub_appends("sid1", TURN);
    let resume = ["claude", "sid1", "resume", "30", "do it", "{}"];
    sd.step(&[], &[&resume[..], &[r#"{"harness": "claude", "args": "--verbose --max-budget-usd 7.5"}"#]].concat());
    let u = sd.last();
    assert_eq!((u["cost_usd"].as_f64(), &u["estimated"]), (Some(7.5), &json!(true)));
    sd.step(&[], &[&resume[..], &[r#"{"harness": "claude", "args": "--max-budget-usd=2"}"#]].concat());
    let u = sd.last();
    assert_eq!((u["cost_usd"].as_f64(), &u["estimated"]), (Some(2.0), &json!(true)));
}

// @test "harness_run: a claude step that failed before any turn costs nothing, and isn't an estimate"
#[test]
fn harness_run_a_claude_step_that_failed_before_any_turn_costs_nothing_and_isnt_an_estimate() {
    let sd = sd();
    sd.claude_stub_appends("sid1", "");
    let r = sd.step(&[], &["claude", "sid1", "resume", "30", "do it", "{}"]);
    assert_ne!(r.status, 0);
    assert_eq!(sd.usage_last(&["cost_usd", "estimated"]), "null|false");
}

// @test "harness_run: a successful run keeps the harness's stderr off the terminal"
#[test]
fn harness_run_a_successful_run_keeps_the_harnesss_stderr_off_the_terminal() {
    let sd = sd();
    let r = sd.step(&[], &STEP);
    assert_eq!(r.status, 0);
    assert_eq!(serde_json::from_str::<Value>(&r.output).unwrap(), json!({"ok": true}));
    assert_eq!(r.stderr, "");
}

// @test "harness_run: a failed run shows the harness's stderr"
#[test]
fn harness_run_a_failed_run_shows_the_harnesss_stderr() {
    let sd = sd();
    sd.fx("status", "1");
    let r = sd.step(&[], &STEP);
    assert_ne!(r.status, 0);
    assert_has(&r.stderr, &[r#""subtype":"init""#]);
}

// @test "harness_run: an error result is a failure, so the stderr shows"
#[test]
fn harness_run_an_error_result_is_a_failure_so_the_stderr_shows() {
    let sd = sd();
    sd.fx("out", r#"{"type": "result", "is_error": true, "result": "out of turns"}"#);
    let r = sd.step(&[], &STEP);
    assert_ne!(r.status, 0);
    assert_eq!(r.output, "");
    assert_has(&r.stderr, &[r#""subtype":"init""#]);
    assert_has(&r.stderr, &["out of turns"]);
}

// @test "harness_run: an error result without result text names its subtype"
#[test]
fn harness_run_an_error_result_without_result_text_names_its_subtype() {
    let sd = sd();
    sd.fx("out", r#"{"type": "result", "subtype": "error_max_turns", "is_error": true}"#);
    let r = sd.step(&[], &STEP);
    assert_ne!(r.status, 0);
    assert_has(&r.stderr, &["error_max_turns"]);
}

// @test "harness_run: a success result with nothing usable doesn't print its subtype"
#[test]
fn harness_run_a_success_result_with_nothing_usable_doesnt_print_its_subtype() {
    let sd = sd();
    sd.fx("out", r#"{"type": "result", "subtype": "success", "is_error": false, "result": ""}"#);
    let r = sd.step(&[], &STEP);
    assert_ne!(r.status, 0);
    assert_lacks(&r.stderr, "success");
}

// @test "harness_run: pi replying without a json block shows the end of the reply"
#[test]
fn harness_run_pi_replying_without_a_json_block_shows_the_end_of_the_reply() {
    let sd = sd();
    sd.fx("out", "I need to know which file you mean.");
    let r = sd.step(&[], &["pi", "sid1", "new", "30", "do it", "{}"]);
    assert_ne!(r.status, 0);
    assert_has(&r.stderr, &["which file you mean"]);
}

// @test "harness_run: claude failing with stdout that isn't JSON shows its end"
#[test]
fn harness_run_claude_failing_with_stdout_that_isnt_json_shows_its_end() {
    let sd = sd();
    sd.fx("out", "Error: unknown option --bogus");
    sd.fx("status", "1");
    let r = sd.step(&[], &STEP);
    assert_ne!(r.status, 0);
    assert_has(&r.stderr, &["unknown option --bogus"]);
}

// @test "harness_run: claude's error text is cut to its last 20 lines"
#[test]
fn harness_run_claudes_error_text_is_cut_to_its_last_20_lines() {
    let sd = sd();
    let text: Vec<String> = (1..31).map(|i| format!("line {i}")).collect();
    sd.fx("out", &json!({"type": "result", "is_error": true, "result": text.join("\n")}).to_string());
    let r = sd.step(&[], &STEP);
    assert_ne!(r.status, 0);
    assert_has(&r.stderr, &["line 11\n", "line 30"]);
    assert_lacks(&r.stderr, "line 10\n");
}

// @test "harness_run: pi's stderr follows the same rule"
#[test]
fn harness_run_pis_stderr_follows_the_same_rule() {
    let sd = sd();
    sd.fx("out", PI_OK);
    let pi = ["pi", "sid1", "new", "30", "do it", "{}"];
    let r = sd.step(&[], &pi);
    assert_eq!(r.status, 0);
    assert_eq!(r.stderr, "");
    sd.fx("status", "1");
    let r = sd.step(&[], &pi);
    assert_ne!(r.status, 0);
    assert_has(&r.stderr, &[r#""subtype":"init""#]);
}
