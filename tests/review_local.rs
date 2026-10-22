// Black-box tests of sd review-local, the default local reviewer (A8)
// (#203): one fresh, read-only model session over the diff
// SHEEPDOG_BASE..SHEEPDOG_HEAD, findings as a JSON list on stdout. The built
// binary, run as sd-review-local through a link (tests/common). Moved from
// libexec/sd-review-local.bats; each test keeps the bats test's title above
// it. Its link test is in tests/links.rs with the other verbs'.
//
// claude and pi are stubs on PATH that record their arguments and print
// $FX/result.json.

mod common;

use common::{assert_has, chomp, jq, Sd};
use std::fs;

const CLAUDE: &str = r#"#!/usr/bin/env bash
printf '%s\n' "$@" > "$FX/claude.args"
args=("$@") prompt= i=0
while (( i < $# )); do
  case ${args[i]} in
    --allowedTools|--disallowedTools)
      i=$((i + 1))
      while (( i < $# )) && [[ ${args[i]} != -* ]]; do i=$((i + 1)); done
      continue ;;
    --session-id|--resume|--output-format|--json-schema|--permission-mode|--model|--effort|--max-turns|--max-budget-usd)
      i=$((i + 2)); continue ;;
    -*) ;;
    *) prompt=${args[i]} ;;
  esac
  i=$((i + 1))
done
printf '%s' "$prompt" > "$FX/claude.prompt"
echo "$PWD" > "$FX/claude.cwd"
[[ -e $FX/fail ]] && { echo "claude: boom" >&2; exit 1; }
[[ -e $FX/earlier.json ]] && jq -n --argjson r "$(cat "$FX/earlier.json")" '{type: "result", is_error: false, structured_output: $r}'
jq -n --argjson r "$(cat "$FX/result.json")" '{type: "result", is_error: false, structured_output: $r}'
"#;

const PI: &str = r#"#!/usr/bin/env bash
printf '%s\n' "$@" > "$FX/pi.args"
printf 'Done.\n\n```json\n%s\n```\n' "$(cat "$FX/result.json")"
"#;

/// The bats setup: repo (cwd) with file.txt at BASE and one commit past it
/// at HEAD, SHEEPDOG_BASE, SHEEPDOG_HEAD and SHEEPDOG_TASK set, the stubs
/// on PATH and an empty findings list in $FX/result.json.
fn sd() -> Sd {
    let mut sd = Sd::for_verb("review-local");
    sd.env = vec![
        ("SHEEPDOG_STATE_DIR".to_string(), sd.path("state")),
        ("FX".to_string(), sd.path("fx")),
    ];
    fs::create_dir(sd.tmp.join("fx")).unwrap();
    sd.fake("claude", CLAUDE);
    sd.fake("pi", PI);
    sd.fx("result.json", r#"{"findings": []}"#);
    sd.git(&["init", "-q", &sd.path("repo")]);
    sd.cd("repo");
    sd.write("file.txt", "one\ntwo\n");
    sd.git(&["add", "file.txt"]);
    sd.git(&["commit", "-qm", "init"]);
    let base = sd.head();
    sd.write("file.txt", "one\ntwo\nthree added\n");
    sd.git(&["commit", "-qam", "add three"]);
    let head = sd.head();
    sd.set_env("SHEEPDOG_BASE", &base);
    sd.set_env("SHEEPDOG_HEAD", &head);
    sd.set_env("SHEEPDOG_TASK", "add a third line");
    sd
}

impl Sd {
    fn set_env(&mut self, k: &str, v: &str) {
        self.env.retain(|(n, _)| n != k);
        self.env.push((k.to_string(), v.to_string()));
    }

    fn var(&self, k: &str) -> String {
        self.env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).unwrap_or_default()
    }

    fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"])
    }

    fn fx(&self, name: &str, line: &str) {
        fs::write(self.tmp.join("fx").join(name), format!("{line}\n")).unwrap();
    }

    fn fx_read(&self, name: &str) -> String {
        fs::read_to_string(self.tmp.join("fx").join(name)).unwrap_or_default()
    }

    fn fx_exists(&self, name: &str) -> bool {
        self.tmp.join("fx").join(name).exists()
    }

    fn set_global(&self, key: &str, value: &str) {
        self.verb_ok("config", &["set", "--global", key, value]);
    }

    /// `"$REVIEW" >/dev/null` in <dir>, relative to the temp dir.
    fn review_in(&mut self, dir: &str) {
        let cwd = self.cwd.clone();
        self.cd(dir);
        self.ok(&[]);
        self.cwd = cwd;
    }
}

/// grep -q <re> over <s>, a line at a time.
fn grep(s: &str, re: &str) -> bool {
    regex::Regex::new(&format!("(?m){re}")).unwrap().is_match(s)
}

/// grep -qx <line> over <s>.
fn has_line(s: &str, line: &str) -> bool {
    s.lines().any(|l| l == line)
}

/// grep -A1 -x -- <flag> <args> | tail -1: the argument after <flag>.
fn after(args: &str, flag: &str) -> String {
    let lines: Vec<&str> = args.lines().collect();
    let i = lines.iter().rposition(|l| *l == flag).unwrap_or_else(|| panic!("no {flag} in {args:?}"));
    lines.get(i + 1).unwrap_or(&"").to_string()
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("")
}

// @test "without SHEEPDOG_BASE and SHEEPDOG_HEAD: fails and says so"
#[test]
fn without_sheepdog_base_and_sheepdog_head_fails_and_says_so() {
    let mut sd = sd();
    sd.env.retain(|(n, _)| n != "SHEEPDOG_HEAD");
    let r = sd.run(&[]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["SHEEPDOG_HEAD"]);
}

// @test "prints the reviewer's findings as a JSON list"
#[test]
fn prints_the_reviewers_findings_as_a_json_list() {
    let sd = sd();
    sd.fx("result.json", r#"{"findings": [{"file": "file.txt", "line": 3, "severity": "warning", "body": "no newline"}]}"#);
    let r = sd.run_sep(&[]);
    assert_eq!(r.status, 0);
    assert_eq!(jq(&["-c", "."], &r.output), r#"[{"file":"file.txt","line":3,"severity":"warning","body":"no newline"}]"#);
}

// @test "no findings: an empty list, exit 0"
#[test]
fn no_findings_an_empty_list_exit_0() {
    let sd = sd();
    let r = sd.run_sep(&[]);
    assert_eq!(r.status, 0);
    assert_eq!(jq(&["-c", "."], &r.output), "[]");
}

// @test "the prompt carries the diff between base and head, and the task"
#[test]
fn the_prompt_carries_the_diff_between_base_and_head_and_the_task() {
    let sd = sd();
    sd.ok(&[]);
    let prompt = sd.fx_read("claude.prompt");
    assert!(grep(&prompt, r"^\+three added"));
    assert!(grep(&prompt, "add a third line"));
    assert_eq!(chomp(&sd.fx_read("claude.cwd")), sd.path("repo"));
}

// @test "a fresh session each run, read-only tools, a schema"
#[test]
fn a_fresh_session_each_run_read_only_tools_a_schema() {
    let sd = sd();
    sd.ok(&[]);
    let args = sd.fx_read("claude.args");
    assert!(has_line(&args, "--session-id"));
    assert!(!has_line(&args, "--resume"));
    assert!(has_line(&args, "--json-schema"));
    let allow = after(&args, "--allowedTools");
    assert!(!allow.contains("Edit") && !allow.contains("Write"), "{allow}");
    assert!(allow.contains("Read"), "{allow}");
}

// @test "the reviewer failing is non-zero, with nothing on stdout"
#[test]
fn the_reviewer_failing_is_non_zero_with_nothing_on_stdout() {
    let sd = sd();
    sd.fx("fail", "");
    let r = sd.run_sep(&[]);
    assert_ne!(r.status, 0);
    assert_eq!(r.output, "");
}

// @test "a result without a findings list is a failure, never clean"
#[test]
fn a_result_without_a_findings_list_is_a_failure_never_clean() {
    let sd = sd();
    sd.fx("result.json", r#"{"verdict": "fine"}"#);
    let r = sd.run_sep(&[]);
    assert_ne!(r.status, 0);
    assert_eq!(r.output, "");
}

// @test "steps.review picks the harness: pi, with read-only tools"
#[test]
fn steps_review_picks_the_harness_pi_with_read_only_tools() {
    let sd = sd();
    sd.set_global("steps.review.harness", "pi");
    sd.fx("result.json", r#"{"findings": [{"file": "file.txt", "line": 3, "severity": "nit", "body": "x"}]}"#);
    let r = sd.run_sep(&[]);
    assert_eq!(r.status, 0);
    assert_eq!(jq(&["length"], &r.output), "1");
    assert_eq!(after(&sd.fx_read("pi.args"), "--tools"), "read");
}

// @test "says when the reviewer starts: which harness, which session, what range"
#[test]
fn says_when_the_reviewer_starts_which_harness_which_session_what_range() {
    let sd = sd();
    let r = sd.run_sep(&[]);
    assert_eq!(r.status, 0);
    let sid = after(&sd.fx_read("claude.args"), "--session-id");
    let (base, head) = (sd.var("SHEEPDOG_BASE"), sd.var("SHEEPDOG_HEAD"));
    assert_has(&r.stderr, &[&format!("claude (new session {sid}) is reviewing main ({}..{})", &base[..7], &head[..7])]);
}

// @test "settings.reviewPrompt empty: the shipped guidance and the reporting rules"
#[test]
fn settings_reviewprompt_empty_the_shipped_guidance_and_the_reporting_rules() {
    let sd = sd();
    sd.ok(&[]);
    let prompt = sd.fx_read("claude.prompt");
    assert!(grep(&prompt, "Find what a careful reviewer would flag"));
    assert!(grep(&prompt, "No findings is a valid"));
}

// @test "settings.reviewPrompt: a custom file replaces the guidance, the reporting rules stay"
#[test]
fn settings_reviewprompt_a_custom_file_replaces_the_guidance_the_reporting_rules_stay() {
    let sd = sd();
    fs::write(sd.tmp.join("guide.md"), "Look only for off-by-one errors.\n").unwrap();
    sd.set_global("settings.reviewPrompt", &sd.path("guide.md"));
    sd.ok(&[]);
    let prompt = sd.fx_read("claude.prompt");
    assert!(grep(&prompt, r"Look only for off-by-one errors\."));
    assert!(!grep(&prompt, "Find what a careful reviewer would flag"));
    assert!(grep(&prompt, "No findings is a valid"));
    assert!(grep(&prompt, r"^\+three added"));
}

// @test "settings.reviewPrompt: a relative path is read from the base commit, not the worktree"
#[test]
fn settings_reviewprompt_a_relative_path_is_read_from_the_base_commit_not_the_worktree() {
    let mut sd = sd();
    sd.git(&["switch", "-q", "--detach", &sd.var("SHEEPDOG_BASE")]);
    fs::create_dir_all(sd.cwd.join(".sheepdog")).unwrap();
    fs::create_dir_all(sd.cwd.join("sub")).unwrap();
    sd.write(".sheepdog/review-prompt.md", "Base guidance.\n");
    sd.git(&["add", ".sheepdog"]);
    sd.git(&["commit", "-qm", "review guidance"]);
    let base = sd.head();
    sd.write(".sheepdog/review-prompt.md", "Weaker guidance.\n");
    sd.git(&["commit", "-qam", "the branch rewrites it"]);
    let head = sd.head();
    sd.set_env("SHEEPDOG_BASE", &base);
    sd.set_env("SHEEPDOG_HEAD", &head);
    sd.set_global("settings.reviewPrompt", ".sheepdog/review-prompt.md");
    sd.review_in("repo/sub");
    assert_eq!(first_line(&sd.fx_read("claude.prompt")), "Base guidance.");
}

// @test "settings.reviewPrompt: a missing file fails, nothing on stdout, no review run"
#[test]
fn settings_reviewprompt_a_missing_file_fails_nothing_on_stdout_no_review_run() {
    let sd = sd();
    sd.set_global("settings.reviewPrompt", "nope.md");
    let r = sd.run_sep(&[]);
    assert_ne!(r.status, 0);
    assert_eq!(r.output, "");
    assert_has(&r.stderr, &["nope.md"]);
    assert!(!sd.fx_exists("claude.args"));
}

// @test "settings.reviewPrompt: a relative path only the worktree has is missing"
#[test]
fn settings_reviewprompt_a_relative_path_only_the_worktree_has_is_missing() {
    let sd = sd();
    fs::create_dir_all(sd.cwd.join(".sheepdog")).unwrap();
    sd.write(".sheepdog/review-prompt.md", "Uncommitted.\n");
    sd.set_global("settings.reviewPrompt", ".sheepdog/review-prompt.md");
    let r = sd.run_sep(&[]);
    assert_ne!(r.status, 0);
    assert_eq!(r.output, "");
    assert_has(&r.stderr, &[".sheepdog/review-prompt.md"]);
    assert!(!sd.fx_exists("claude.args"));
}

// @test "settings.reviewPrompt: ./ is the worktree's top, wherever the reviewer runs"
#[test]
fn settings_reviewprompt_is_the_worktrees_top_wherever_the_reviewer_runs() {
    let mut sd = sd();
    fs::create_dir_all(sd.cwd.join(".sheepdog")).unwrap();
    fs::create_dir_all(sd.cwd.join("sub/.sheepdog")).unwrap();
    sd.write(".sheepdog/review-prompt.md", "Top guidance.\n");
    sd.write("sub/.sheepdog/review-prompt.md", "Sub guidance.\n");
    sd.git(&["add", "."]);
    sd.git(&["commit", "-qm", "guidance"]);
    let head = sd.head();
    sd.set_env("SHEEPDOG_BASE", &head);
    sd.set_env("SHEEPDOG_HEAD", &head);
    sd.set_global("settings.reviewPrompt", "./.sheepdog/review-prompt.md");
    sd.review_in("repo/sub");
    assert_eq!(first_line(&sd.fx_read("claude.prompt")), "Top guidance.");
}

// @test "settings.reviewPrompt: a directory is not a guidance file"
#[test]
fn settings_reviewprompt_a_directory_is_not_a_guidance_file() {
    let mut sd = sd();
    fs::create_dir_all(sd.cwd.join(".sheepdog")).unwrap();
    sd.write(".sheepdog/review-prompt.md", "x\n");
    sd.git(&["add", "."]);
    sd.git(&["commit", "-qm", "guidance"]);
    let head = sd.head();
    sd.set_env("SHEEPDOG_BASE", &head);
    sd.set_env("SHEEPDOG_HEAD", &head);
    sd.set_global("settings.reviewPrompt", ".sheepdog");
    let r = sd.run_sep(&[]);
    assert_ne!(r.status, 0);
    assert!(!sd.fx_exists("claude.args"));
}

// @test "settings.reviewPrompt: a missing absolute path fails, no review run"
#[test]
fn settings_reviewprompt_a_missing_absolute_path_fails_no_review_run() {
    let sd = sd();
    let nope = sd.path("nope.md");
    sd.set_global("settings.reviewPrompt", &nope);
    let r = sd.run_sep(&[]);
    assert_ne!(r.status, 0);
    assert_has(&r.stderr, &[&nope]);
    assert!(!sd.fx_exists("claude.args"));
}

// @test "settings.reviewPrompt empty: .sheepdog/review.md in the base commit replaces the shipped guidance"
#[test]
fn settings_reviewprompt_empty_sheepdog_review_md_in_the_base_commit_replaces_the_shipped_guidance() {
    let mut sd = sd();
    sd.git(&["switch", "-q", "--detach", &sd.var("SHEEPDOG_BASE")]);
    fs::create_dir_all(sd.cwd.join(".sheepdog")).unwrap();
    sd.write(".sheepdog/review.md", "Repo review guidance.\n");
    sd.git(&["add", ".sheepdog"]);
    sd.git(&["commit", "-qm", "guidance"]);
    let base = sd.head();
    sd.set_env("SHEEPDOG_BASE", &base);
    sd.write("file.txt", "one\ntwo\nx\n");
    sd.git(&["commit", "-qam", "change"]);
    let head = sd.head();
    sd.set_env("SHEEPDOG_HEAD", &head);
    sd.ok(&[]);
    let prompt = sd.fx_read("claude.prompt");
    assert_eq!(first_line(&prompt), "Repo review guidance.");
    assert!(grep(&prompt, "No findings is a valid"));
}

// @test "two results from the harness: stdout is still one findings list, the last"
#[test]
fn two_results_from_the_harness_stdout_is_still_one_findings_list_the_last() {
    let sd = sd();
    sd.fx("earlier.json", r#"{"findings": [{"file": "a", "line": 1, "severity": "nit", "body": "earlier"}]}"#);
    sd.fx("result.json", r#"{"findings": [{"file": "b", "line": 2, "severity": "nit", "body": "last"}]}"#);
    let r = sd.run_sep(&[]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output.lines().count(), 1);
    assert_eq!(jq(&["-r", ".[0].body"], &r.output), "last");
}

#[test]
fn help_prints_the_usage_and_the_contract_without_a_base() {
    let mut sd = sd();
    sd.env.retain(|(n, _)| n != "SHEEPDOG_BASE");
    let r = sd.run(&["--help"]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_has(&r.output, &["Usage: ", "sd review-local", "settings.localReview"]);
    assert!(!sd.fx_exists("claude.args"));
}
