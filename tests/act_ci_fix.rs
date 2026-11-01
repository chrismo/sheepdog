// Black-box tests of sd act ci-fix (J2, #203): the built binary, run as
// sd-act through a link (tests/common). Moved from libexec/sd-act-ci-fix.bats;
// each test keeps the bats test's title above it. On red CI, run
// settings.autofix and verify first (zero tokens), then hand the failing
// checks, their logs and the local verify output to the agent in the branch's
// session; the pipeline commits, verifies and pushes its fix. A "flaky"
// verdict is recorded against the red SHA so the same red doesn't burn
// another round.
//
// Real git (a bare origin and a clone). sd state answers from
// $FX/states/feat.json (SHEEPDOG_STATE_FIXTURES). Fakes on PATH: gh (REST:
// commit status from $FX/status.json, check runs from $FX/checks.json, a job's
// failed log from $FX/gha.log; the PR's state from $FX/pr.state, OPEN when
// it's missing, and a failure when $FX/pr-view.fail exists; every call logged
// to $FX/gh.calls), rwx
// (results from $FX/rwx-results.json, logs from $FX/rwx.log), claude (runs
// $FX/agent to edit, prints $FX/result.json, keeps its prompt in
// $FX/claude.prompt).

mod common;

use common::{assert_has, assert_lacks, assert_match, chomp, dead_pid, jq, lock_by, spawn, Holder, Sd};
use sheepdog::running::proc_start;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

const GH: &str = r#"#!/usr/bin/env bash
printf '%s\n' "$*" >> "$FX/gh.calls"
filter=""; args=("$@"); for ((i = 0; i < $#; i++)); do [[ ${args[i]} == --jq ]] && filter=${args[i+1]}; done
case "$1 $2" in
  "api repos/{owner}/{repo}/commits/"*"/status") src=$FX/status.json ;;
  "api repos/{owner}/{repo}/commits/"*"/check-runs") src=$FX/checks.json ;;
  "run view") cat "$FX/gha.log"; exit ;;
  "pr view") [[ -f $FX/pr-view.fail ]] && { echo "gh: HTTP 502" >&2; exit 1; }
             cat "$FX/pr.state" 2>/dev/null || echo OPEN; exit ;;
  *) echo "gh stub: unexpected call: $*" >&2; exit 1 ;;
esac
if [[ -n $filter ]]; then jq -r "$filter" "$src"; else cat "$src"; fi   # gh's --jq is jq
"#;

const RWX: &str = r#"#!/usr/bin/env bash
printf '%s\n' "$*" >> "$FX/rwx.calls"
case $1 in
  results) cat "$FX/rwx-results.json" ;;
  logs) out=""; while (( $# )); do [[ $1 == --output ]] && out=$2; shift; done
        mkdir -p "$out" && cp "$FX/rwx.log" "$out/test.log" && echo "Logs downloaded to $out/" ;;
  *) exit 1 ;;
esac
"#;

const CLAUDE: &str = r#"#!/usr/bin/env bash
printf '%s\n' "$@" > "$FX/claude.args"
args=("$@") prompt= i=0
while (( i < $# )); do
  case ${args[i]} in
    --allowedTools|--disallowedTools) i=$((i + 1)); while (( i < $# )) && [[ ${args[i]} != -* ]]; do i=$((i + 1)); done; continue ;;
    --session-id|--resume|--output-format|--json-schema|--permission-mode|--model|--effort|--max-turns|--max-budget-usd) i=$((i + 2)); continue ;;
    -*) ;;
    *) prompt=${args[i]} ;;
  esac
  i=$((i + 1))
done
printf '%s' "$prompt" > "$FX/claude.prompt"
[[ -x $FX/agent ]] && "$FX/agent"
jq -n --argjson r "$(cat "$FX/result.json")" '{type: "result", is_error: false, structured_output: $r}'
"#;

/// sd state's JSON: open PR, in sync, CI red on HEAD.
const STATE: &str = r#"{
    branch: "feat", head: $head,
    pr: {number: 7, url: "https://example/pr/7", state: "OPEN", draft: false, base: "main"},
    parent: {name: "main", merged: false, pr: null},
    sync: {target: "main", default: "main", ahead: 1, behind: 0, up_to_date: true, old_base: $main, conflict: false},
    push: {remote_sha: $head, ahead: 0, behind: 0, diverged: false, remote_ours: true},
    fresh: true,
    ci: {sha: $head, for_head: true, verdict: "red", failing: ["RWX: tests.yml (pull request)", "test"], pending: []},
    threads: {owed_bot: 0, owed_human: 0, outdated: 0, blocking: 0, info: 0, items: []},
    session: {enrolled: true, harness: null, id: null, locked: false}, errors: []}"#;

const CHECKS: &str = r#"{"total_count":2,"check_runs":[
    {"name":"test","status":"completed","conclusion":"failure","details_url":"https://github.com/o/r/actions/runs/555/job/777","app":{"slug":"github-actions"},"output":{"title":"1 test failed","summary":"FAIL: TestThree wants THREE","text":null}},
    {"name":"lint","status":"completed","conclusion":"failure","details_url":"https://github.com/o/r/actions/runs/556/job/778","app":{"slug":"github-actions"},"output":{"title":null,"summary":null,"text":null}}]}"#;

/// Default agent: uppercases three, which is what the logs ask for.
const AGENT_FIXES: &str = "#!/usr/bin/env bash\nprintf \"one\\ntwo\\nTHREE\\n\" > file.txt\n";

/// The fix, plus a CI config file.
const AGENT_TOUCHES_CI: &str = "#!/usr/bin/env bash\nprintf \"one\\ntwo\\nTHREE\\n\" > file.txt\nmkdir -p .github/workflows && echo \"on: push\" > .github/workflows/test.yml\n";

/// A CI config file the agent commits itself.
const AGENT_COMMITS_CI: &str = "#!/usr/bin/env bash\nmkdir -p .github/workflows && echo x > .github/workflows/x.yml && git add -A && git commit -qm \"agent made it\"\n";

/// The bats setup: origin.git and its clone repo (cwd), main with file.txt,
/// feat one commit past it, pushed, with a task; sd state saying CI is red on
/// HEAD, with the fixtures each fake answers from, and the agent that fixes it.
fn sd() -> Sd {
    let mut sd = Sd::for_verb("act");
    sd.env = vec![
        ("SHEEPDOG_STATE_DIR".into(), sd.path("state")),
        ("FX".into(), sd.path("fx")),
        ("SHEEPDOG_STATE_FIXTURES".into(), sd.path("fx/states")),
    ];
    fs::create_dir_all(sd.tmp.join("fx/states")).unwrap();
    sd.fake("gh", GH);
    sd.fake("rwx", RWX);
    sd.fake("claude", CLAUDE);
    sd.origin(&["repo"]);
    sd.cd("repo");
    sd.write("file.txt", "one\ntwo\n");
    sd.git(&["add", "file.txt"]);
    sd.git(&["commit", "-qm", "init"]);
    sd.git(&["push", "-q", "origin", "main"]);
    sd.git(&["switch", "-qc", "feat"]);
    sd.write("file.txt", "one\ntwo\nthree\n");
    sd.git(&["commit", "-qam", "add three"]);
    sd.git(&["push", "-q", "-u", "origin", "feat"]);
    sd.git(&["config", "branch.feat.sd-task", "add a third line"]);

    sd.state(&[]);
    sd.put("status.json", r#"{"state":"failure","statuses":[{"context":"RWX: tests.yml (pull request)","state":"failure","description":"Failed in 24s","target_url":"https://cloud.rwx.com/clabs/runs/01a1run"}]}"#);
    sd.put("checks.json", CHECKS);
    sd.put("rwx-results.json", r#"{"ID":"01a1run","ResultStatus":"failed","Tasks":[{"Key":"lint","ID":"task-ok","Status":{"Result":"succeeded"}},{"Key":"test","ID":"task-bad","Status":{"Result":"failed"}}]}"#);
    fs::write(sd.tmp.join("fx/rwx.log"), "rwx line 1\nrwx: expected THREE, got three\n").unwrap();
    fs::write(sd.tmp.join("fx/gha.log"), "gha line 1\ngha: --- FAIL: TestThree\n").unwrap();
    sd.agent(AGENT_FIXES);
    sd.result(r#"{outcome: "fixed", commit_message: "Uppercase three", summary: "the test wants THREE"}"#);
    sd
}

/// The first line of the shipped lib/prompts/ci-fix.md.
fn shipped_prompt() -> String {
    let p = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/lib/prompts/ci-fix.md")).unwrap();
    p.lines().next().unwrap_or_default().to_string()
}

impl Sd {
    /// $FX/<f> := <s>, newline-terminated.
    fn put(&self, f: &str, s: &str) {
        fs::write(self.tmp.join("fx").join(f), format!("{s}\n")).unwrap();
    }

    /// `$(cat "$FX/<f>")`, or "" when it's missing.
    fn fx(&self, f: &str) -> String {
        chomp(&fs::read_to_string(self.tmp.join("fx").join(f)).unwrap_or_default())
    }

    fn ran(&self, f: &str) -> bool {
        self.tmp.join("fx").join(f).exists()
    }

    fn rm(&self, f: &str) {
        let _ = fs::remove_file(self.tmp.join("fx").join(f));
    }

    /// state_fixture [jq-assignments...]
    fn state(&self, edits: &[&str]) {
        let (head, main) = (self.rev("HEAD"), self.rev("main"));
        let mut j = jq(&["-n", "--arg", "head", &head, "--arg", "main", &main, STATE], "");
        for e in edits {
            j = jq(&[e], &j);
        }
        self.put("states/feat.json", &j);
    }

    /// agent_result <jq expression>: what the agent returns.
    fn result(&self, expr: &str) {
        self.put("result.json", &jq(&["-n", expr], ""));
    }

    /// $FX/agent := <script>, what the agent does.
    fn agent(&self, script: &str) {
        self.script(&self.tmp.join("fx/agent"), script);
    }

    fn agent_edits_nothing(&self) {
        self.rm("agent");
    }

    /// "$SD_CONFIG" set --global <args>
    fn set(&self, args: &[&str]) {
        self.verb_ok("config", &[&["set", "--global"], args].concat());
    }

    fn rev(&self, r: &str) -> String {
        self.git(&["rev-parse", r])
    }

    fn remote_feat(&self) -> String {
        self.git_in("origin.git", &["rev-parse", "feat"])
    }

    fn prompt(&self) -> String {
        self.fx("claude.prompt")
    }

    fn recorded(&self) -> Option<String> {
        self.git_try(&["config", "--get", "branch.feat.sd-ci-fix"])
    }

    fn held(&self) -> Option<String> {
        self.git_try(&["config", "--get", "branch.feat.sd-held"])
    }

    fn subject(&self) -> String {
        self.git(&["log", "-1", "--format=%s"])
    }

    /// `grep -A1 -x -- <flag> "$FX/claude.args" | tail -1`: the word after the last <flag>.
    fn after(&self, flag: &str) -> String {
        let args = self.fx("claude.args");
        let lines: Vec<&str> = args.lines().collect();
        let i = lines.iter().rposition(|l| *l == flag).unwrap_or_else(|| panic!("no {flag} in {args}"));
        lines.get(i + 1).unwrap_or(&"").to_string()
    }

    fn question(&self) -> Option<String> {
        self.git_try(&["rev-parse", "-q", "--verify", "refs/sheepdog/feat/question"])
    }
}

/// echo <line> >> <f>
fn append(f: &Path, line: &str) {
    writeln!(OpenOptions::new().append(true).open(f).unwrap(), "{line}").unwrap();
}

/// `grep -q '^<prefix>' <text>`
fn has_line(text: &str, prefix: &str) -> bool {
    text.lines().any(|l| l.starts_with(prefix))
}

// ── gates ────────────────────────────────────────────────────────────────────

// @test "CI green: nothing to fix, nothing runs"
// The gate's Done (#348): nothing to do, so it exits 0.
#[test]
fn ci_green_nothing_to_fix_nothing_runs() {
    let sd = sd();
    sd.state(&[r#".ci.verdict = "green" | .ci.failing = []"#]);
    let r = sd.run(&["ci-fix"]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_has(&r.output, &["CI is green at HEAD: nothing to fix on feat"]);
    assert!(!sd.ran("claude.args"));
}

// @test "CI inconclusive (cancelled): rerun it instead"
#[test]
fn ci_inconclusive_cancelled_rerun_it_instead() {
    let sd = sd();
    sd.state(&[r#".ci.verdict = "inconclusive" | .ci.failing = []"#]);
    let r = sd.run(&["ci-fix"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["rerun"]);
    assert!(!sd.ran("claude.args"));
}

// @test "red on an old head: push first; the fix may already be there"
#[test]
fn red_on_an_old_head_push_first_the_fix_may_already_be_there() {
    let sd = sd();
    sd.state(&[r#".ci.for_head = false | .ci.sha = "0000000000000000000000000000000000000000""#]);
    let r = sd.run(&["ci-fix"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["CI's red isn't for HEAD", "then wait for CI on HEAD"]);
    assert!(!sd.ran("claude.args"));
}

// @test "unpushed commits: push first"
#[test]
fn unpushed_commits_push_first() {
    let sd = sd();
    sd.state(&[".push.ahead = 1"]);
    let r = sd.run(&["ci-fix"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["unpushed"]);
    assert_has(&r.output, &["sd act push"]);
    assert!(!sd.ran("claude.args"));
}

// A CI wait is Soft like any other verb's (chrismo, 2026-10-31, reversing #333):
// the cost is tokens, which branchBudget already caps.
#[test]
fn a_ci_wait_refuses_unforced_and_force_runs_the_agent_with_a_warning() {
    for (edit, why) in [
        (r#".ci.verdict = "pending" | .ci.failing = []"#, "CI is pending"),
        (r#".ci.verdict = "inconclusive" | .ci.failing = []"#, "CI was cancelled"),
        (".push.ahead = 1", "there are unpushed commits"),
    ] {
        let sd = sd();
        sd.state(&[edit]);
        let r = sd.run(&["ci-fix"]);
        assert_ne!(r.status, 0, "{edit}: {}", r.output);
        assert_has(&r.output, &["not fixing CI on feat until CI is red at HEAD", why, "--force"]);
        assert!(!sd.ran("claude.args"), "{edit}");
        let r = sd.run(&["ci-fix", "--force"]);
        assert_has(&r.output, &["warning", why]);
        assert!(sd.ran("claude.args"), "{edit}: {}", r.output);
    }
}

// Forced past a wait, the agent is told CI may not be red at HEAD, and an
// empty failing list reads as none reported, not "Failing checks on h: ".
#[test]
fn forced_past_a_ci_wait_the_agent_is_told_why_and_no_failing_checks_reads_as_none() {
    let sd = sd();
    sd.state(&[r#".ci.verdict = "pending" | .ci.failing = []"#]);
    sd.result(r#"{outcome: "fixed", commit_message: null, summary: "the test wants THREE"}"#);
    let r = sd.run(&["ci-fix", "--force"]);
    assert_eq!(r.status, 0, "{}", r.output);
    let p = sd.prompt();
    assert_has(&p, &["sd act ci-fix was forced past its gate, so CI may not be red at HEAD:", "- CI is pending: wait for it"]);
    assert_has(&p, &["No failing checks are reported on"]);
    assert_lacks(&p, "Failing checks on");
    assert_eq!(sd.subject(), "Fix CI");
}

// @test "no PR, or a closed one: refuses"
#[test]
fn no_pr_or_a_closed_one_refuses() {
    let sd = sd();
    sd.state(&[".pr = null"]);
    let r = sd.run(&["ci-fix"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["no PR"]);
    sd.state(&[r#".pr.state = "MERGED""#]);
    let r = sd.run(&["ci-fix"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["merged"]);
}

// @test "a dirty worktree is refused before anything runs"
#[test]
fn a_dirty_worktree_is_refused_before_anything_runs() {
    let sd = sd();
    append(&sd.cwd.join("file.txt"), "dirty");
    let r = sd.run(&["ci-fix"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["uncommitted changes"]);
}

// ── the step ─────────────────────────────────────────────────────────────────

// @test "red: the agent gets the failing checks with their logs; the fix is committed, verified, pushed, recorded"
#[test]
fn red_the_agent_gets_the_failing_checks_with_their_logs_the_fix_is_committed_verified_pushed_recorded() {
    let sd = sd();
    sd.set(&["settings.verify", "grep -q THREE file.txt"]);
    let head = sd.rev("HEAD");
    let r = sd.run(&["ci-fix"]);
    assert_eq!(r.status, 0);
    let p = sd.prompt();
    assert_has(&p, &["RWX: tests.yml (pull request)"]);
    assert_has(&p, &["test"]);
    assert_has(&p, &["Failed in 24s"]); // the status's description
    assert_has(&p, &["rwx: expected THREE, got three"]); // rwx logs of the failed task
    assert_has(&p, &["1 test failed"]); // the check run's output title
    assert_has(&p, &["FAIL: TestThree wants THREE"]); // and summary
    assert_has(&p, &["gha: --- FAIL: TestThree"]); // gh run view --log-failed
    assert_has(&p, &["add a third line"]); // the task
    assert_has(&p, &["grep -q THREE file.txt"]); // the repo's checks
    assert_eq!(sd.subject(), "Uppercase three");
    assert_eq!(sd.remote_feat(), sd.rev("HEAD"));
    assert_eq!(sd.recorded().as_deref(), Some(format!("{head} fixed").as_str()));
    assert_has(&r.output, &["pushed"]);
    let rwx = sd.fx("rwx.calls");
    assert!(has_line(&rwx, "results 01a1run --json"), "{rwx}");
    assert!(has_line(&rwx, "logs task-bad --output"), "{rwx}");
    assert_lacks(&rwx, "task-ok");
    let gh = sd.fx("gh.calls");
    assert!(has_line(&gh, "run view 555 --job 777 --log-failed"), "{gh}");
    assert!(has_line(&gh, "run view 556 --job 778 --log-failed"), "{gh}"); // a check run with no output text still gets its log
    assert_lacks(&gh, "graphql");
}

// @test "no settings.verify: the agent is told no local checks exist, not that they pass"
#[test]
fn no_settings_verify_the_agent_is_told_no_local_checks_exist_not_that_they_pass() {
    let sd = sd();
    let r = sd.run(&["ci-fix"]);
    assert_eq!(r.status, 0);
    assert_has(&sd.prompt(), &["No local checks are configured"]);
    assert_lacks(&sd.prompt(), "pass locally");
}

// @test "verify changing the tree is refused before the agent runs"
#[test]
fn verify_changing_the_tree_is_refused_before_the_agent_runs() {
    let sd = sd();
    sd.set(&["settings.verify", "echo touched >> file.txt"]);
    let r = sd.run(&["ci-fix"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["changed files"]);
    assert!(!sd.ran("claude.args"));
}

// @test "the agent may run the verify command, and is told what it may run"
#[test]
fn the_agent_may_run_the_verify_command_and_is_told_what_it_may_run() {
    let sd = sd();
    sd.set(&["settings.verify", "grep -q THREE file.txt"]);
    let r = sd.run(&["ci-fix"]);
    assert_eq!(r.status, 0);
    assert_has(&sd.after("--allowedTools"), &["Bash(grep -q THREE file.txt)"]);
    assert_has(&sd.prompt(), &["Bash(grep -q THREE file.txt)"]);
}

// @test "the branch's running marker lasts the whole step, both verifies included, then is gone (#170)"
#[test]
fn the_branchs_running_marker_lasts_the_whole_step_both_verifies_included_then_is_gone_170() {
    let sd = sd();
    let marker = sd.path("repo/.git/sheepdog/feat.running");
    sd.set(&["settings.verify", &format!("cat {marker} >> {}", sd.path("fx/during"))]);
    let r = sd.run(&["ci-fix"]);
    assert_eq!(r.status, 0);
    let during = sd.fx("during");
    assert_eq!(during.lines().count(), 2);
    assert!(during.lines().all(|l| l.split(' ').nth(1) == Some("ci-fix")), "{during}");
    assert!(!Path::new(&marker).exists());
}

// @test "another live step's marker refuses the run: the way out is named, the holder's marker is untouched, no agent runs (#177)"
#[test]
fn another_live_steps_marker_refuses_the_run_and_is_left_alone_177() {
    let sd = sd();
    let marker = sd.path("repo/.git/sheepdog/feat.running");
    fs::create_dir_all(sd.tmp.join("repo/.git/sheepdog")).unwrap();
    let mut holder = spawn(Command::new("sleep").arg("30"));
    let pid = holder.id().to_string();
    let theirs = format!("{pid} implement 2026-10-18T14:03:00Z {}\n", proc_start(&pid));
    fs::write(&marker, &theirs).unwrap();
    let r = sd.run(&["ci-fix"]);
    let after = fs::read_to_string(&marker);
    holder.kill().unwrap();
    holder.wait().unwrap();
    assert_ne!(r.status, 0);
    assert_has(
        &r.output,
        &[
            &format!("implement is running on feat (pid {pid}, since 2026-10-18T14:03:00Z)"),
            &format!("kill {pid}, or remove {marker}"),
        ],
    );
    assert!(!sd.ran("claude.args"));
    assert_eq!(after.unwrap(), theirs);
}

// @test "a mechanical verb run by hand holds the marker: ci-fix started meanwhile refuses, no agent runs (#270)"
#[test]
fn a_mechanical_verb_run_by_hand_holds_the_marker_ci_fix_started_meanwhile_refuses_270() {
    let sd = sd();
    sd.set(&["settings.verify", "touch ../verify-started; while [ ! -e ../go ]; do sleep 0.1; done"]);
    let mut verify = spawn(sd.command(&sd.bin).arg("verify").stdout(Stdio::null()).stderr(Stdio::null()));
    for _ in 0..100 {
        if sd.tmp.join("verify-started").exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let r = sd.run(&["ci-fix"]);
    fs::write(sd.tmp.join("go"), "").unwrap();
    assert!(verify.wait().unwrap().success());
    assert_ne!(r.status, 0);
    assert_has(&r.output, &[&format!("verify is running on feat (pid {}", verify.id())]);
    assert!(!sd.ran("claude.args"));
    assert!(!sd.tmp.join("repo/.git/sheepdog/feat.running").exists());
}

// @test "the PR merged or closed while the step ran: nothing pushed, the commit kept, and said (#270)"
#[test]
fn the_pr_merged_or_closed_while_the_step_ran_nothing_pushed_the_commit_kept_and_said_270() {
    for state in ["MERGED", "CLOSED"] {
        let sd = sd();
        sd.agent(&format!("{AGENT_FIXES}echo {state} > \"$FX/pr.state\"\n"));
        let before = sd.remote_feat();
        let r = sd.run(&["ci-fix"]);
        assert_ne!(r.status, 0, "{state}");
        let wt = sd.path("repo");
        assert_has(
            &r.output,
            &[&format!("PR #7 {} during ci-fix: not pushing; the commit is kept in {wt}", state.to_lowercase())],
        );
        assert_eq!(sd.subject(), "Uppercase three");
        assert_eq!(sd.remote_feat(), before);
        assert!(has_line(&sd.fx("gh.calls"), "pr view https://example/pr/7"), "{}", sd.fx("gh.calls"));
    }
}

// @test "attached during ci-fix: nothing committed or pushed; the edits left, and said (#270)"
#[test]
fn attached_during_ci_fix_nothing_committed_or_pushed_the_edits_left_and_said_270() {
    let sd = sd();
    let attach = Holder::new();
    let lock = lock_by(attach.pid());
    sd.agent(&format!("{AGENT_FIXES}git config branch.feat.sd-lock '{lock}'\n"));
    let (before, remote) = (sd.rev("HEAD"), sd.remote_feat());
    let r = sd.run(&["ci-fix"]);
    assert_ne!(r.status, 0);
    let wt = sd.path("repo");
    assert_has(&r.output, &[&format!("not committing: feat was attached during ci-fix ({lock}); the edits are left in {wt}")]);
    assert_eq!(sd.rev("HEAD"), before);
    assert_has(&sd.git(&["status", "--porcelain"]), &["file.txt"]);
    assert_eq!(sd.remote_feat(), remote);
}

// @test "attached after ci-fix's commit (during verify): nothing pushed, the commit kept (#270)"
#[test]
fn attached_after_ci_fixs_commit_during_verify_nothing_pushed_the_commit_kept_270() {
    let sd = sd();
    let attach = Holder::new();
    let lock = lock_by(attach.pid());
    // Red before the agent's fix, so the lock comes only with the verify after it.
    sd.set(&["settings.verify", &format!("grep -q THREE file.txt && git config branch.feat.sd-lock '{lock}'")]);
    let remote = sd.remote_feat();
    let r = sd.run(&["ci-fix"]);
    assert_ne!(r.status, 0);
    assert!(sd.ran("claude.args"));
    let wt = sd.path("repo");
    assert_has(&r.output, &[&format!("not pushing: feat was attached during ci-fix ({lock}); the commit is kept in {wt}")]);
    assert_eq!(sd.subject(), "Uppercase three");
    assert_eq!(sd.remote_feat(), remote);
}

// @test "a stale lock set during ci-fix doesn't stop it (#270)"
#[test]
fn a_stale_lock_set_during_ci_fix_doesnt_stop_it_270() {
    let sd = sd();
    sd.agent(&format!("{AGENT_FIXES}git config branch.feat.sd-lock '{}'\n", lock_by(dead_pid())));
    let r = sd.run(&["ci-fix"]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_eq!(sd.subject(), "Uppercase three");
    assert_eq!(sd.remote_feat(), sd.rev("HEAD"));
}

// @test "ci-fix --force over a live lock, the same lock still there: committed and pushed as ever (#270)"
#[test]
fn ci_fix_force_over_a_live_lock_the_same_lock_still_there_committed_and_pushed_as_ever_270() {
    let sd = sd();
    let attach = Holder::new();
    sd.git(&["config", "branch.feat.sd-lock", &lock_by(attach.pid())]);
    let r = sd.run(&["ci-fix", "--force"]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_eq!(sd.subject(), "Uppercase three");
    assert_eq!(sd.remote_feat(), sd.rev("HEAD"));
}

// @test "autofix's commit isn't pushed onto a PR that merged meanwhile (#270)"
#[test]
fn autofixs_commit_isnt_pushed_onto_a_pr_that_merged_meanwhile_270() {
    let sd = sd();
    sd.set(&["settings.verify", "grep -q THREE file.txt"]);
    sd.set(&["settings.autofix", "sed -i.bak s/three/THREE/ file.txt && rm file.txt.bak"]);
    sd.put("pr.state", "MERGED");
    let before = sd.remote_feat();
    let r = sd.run(&["ci-fix"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["PR #7 merged during ci-fix: not pushing"]);
    assert_eq!(sd.subject(), "autofix: sed -i.bak s/three/THREE/ file.txt && rm file.txt.bak");
    assert_eq!(sd.remote_feat(), before);
}

// @test "attached during ci-fix's autofix: autofix commits nothing, its changes left, and said (#270)"
#[test]
fn attached_during_ci_fixs_autofix_autofix_commits_nothing_its_changes_left_and_said_270() {
    let sd = sd();
    let attach = Holder::new();
    let lock = lock_by(attach.pid());
    sd.set(&["settings.verify", "grep -q THREE file.txt"]);
    sd.set(&["settings.autofix", &format!("sed -i.bak s/three/THREE/ file.txt && rm file.txt.bak && git config branch.feat.sd-lock '{lock}'")]);
    let (before, remote) = (sd.rev("HEAD"), sd.remote_feat());
    let r = sd.run(&["ci-fix"]);
    assert_ne!(r.status, 0);
    assert!(!sd.ran("claude.args"));
    let wt = sd.path("repo");
    assert_has(&r.output, &[&format!("not committing: feat was attached during ci-fix ({lock}); autofix's changes are left in {wt}")]);
    assert_eq!(sd.rev("HEAD"), before);
    assert_has(&sd.git(&["status", "--porcelain"]), &["file.txt"]);
    assert_eq!(sd.remote_feat(), remote);
}

// @test "attached during ci-fix's failing autofix: nothing reverted, its changes left, and said (#270)"
#[test]
fn attached_during_ci_fixs_failing_autofix_nothing_reverted_its_changes_left_and_said_270() {
    let sd = sd();
    let attach = Holder::new();
    let lock = lock_by(attach.pid());
    sd.set(&["settings.verify", "grep -q THREE file.txt"]);
    sd.set(&["settings.autofix", &format!("sed -i.bak s/three/THREE/ file.txt && rm file.txt.bak && git config branch.feat.sd-lock '{lock}' && false")]);
    let (before, remote) = (sd.rev("HEAD"), sd.remote_feat());
    let r = sd.run(&["ci-fix"]);
    assert_ne!(r.status, 0);
    assert!(!sd.ran("claude.args"));
    let wt = sd.path("repo");
    assert_has(&r.output, &[&format!("not reverting: feat was attached during ci-fix ({lock}); autofix's changes are left in {wt}")]);
    assert_eq!(sd.rev("HEAD"), before);
    assert_has(&sd.git(&["status", "--porcelain"]), &["file.txt"]);
    assert_eq!(sd.remote_feat(), remote);
}

// @test "the PR's state unreadable before the push: nothing pushed, the commit kept, and why (#270)"
#[test]
fn the_prs_state_unreadable_before_the_push_nothing_pushed_the_commit_kept_and_why_270() {
    let sd = sd();
    sd.put("pr-view.fail", "");
    let before = sd.remote_feat();
    let r = sd.run(&["ci-fix"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["can't read PR #7's state", "gh: HTTP 502", "not pushing", &format!("the commit is kept in {}", sd.path("repo"))]);
    assert_eq!(sd.subject(), "Uppercase three");
    assert_eq!(sd.remote_feat(), before);
}

// @test "logs that can't be fetched are skipped, not fatal"
#[test]
fn logs_that_cant_be_fetched_are_skipped_not_fatal() {
    let sd = sd();
    sd.fake("rwx", "#!/usr/bin/env bash\nexit 1\n");
    sd.rm("gha.log");
    let r = sd.run(&["ci-fix"]);
    assert_eq!(r.status, 0);
    assert_has(&sd.prompt(), &["RWX: tests.yml (pull request)"]);
    assert_eq!(sd.remote_feat(), sd.rev("HEAD"));
}

// @test "logs are cut to their last lines"
#[test]
fn logs_are_cut_to_their_last_lines() {
    let sd = sd();
    let log: String = (1..=400).map(|i| format!("gha {i}\n")).collect();
    fs::write(sd.tmp.join("fx/gha.log"), log).unwrap();
    let r = sd.run(&["ci-fix"]);
    assert_eq!(r.status, 0);
    assert_has(&sd.prompt(), &["gha 400"]);
    assert_lacks(&sd.prompt(), "gha 1\n");
}

// @test "autofix fixes what verify saw: committed and pushed, no agent"
#[test]
fn autofix_fixes_what_verify_saw_committed_and_pushed_no_agent() {
    let sd = sd();
    sd.set(&["settings.verify", "grep -q THREE file.txt"]);
    sd.set(&["settings.autofix", "sed -i.bak s/three/THREE/ file.txt && rm file.txt.bak"]);
    let r = sd.run(&["ci-fix"]);
    assert_eq!(r.status, 0);
    assert!(!sd.ran("claude.args"));
    assert_has(&r.output, &["autofix committed"]);
    assert_eq!(sd.subject(), "autofix: sed -i.bak s/three/THREE/ file.txt && rm file.txt.bak");
    assert_eq!(sd.remote_feat(), sd.rev("HEAD"));
}

// @test "verify failing locally: its output goes to the agent"
#[test]
fn verify_failing_locally_its_output_goes_to_the_agent() {
    let sd = sd();
    sd.set(&["settings.verify", "echo 'local: three should be THREE'; grep -q THREE file.txt"]);
    let r = sd.run(&["ci-fix"]);
    assert_eq!(r.status, 0);
    assert_has(&sd.prompt(), &["local: three should be THREE"]);
}

// @test "verify passing locally: the agent is told so"
#[test]
fn verify_passing_locally_the_agent_is_told_so() {
    let sd = sd();
    sd.set(&["settings.verify", "true"]);
    let r = sd.run(&["ci-fix"]);
    assert_eq!(r.status, 0);
    assert_has(&sd.prompt(), &["pass locally"]);
}

// @test "flaky after editing files: not recorded, so no --force is needed once they are cleaned up"
#[test]
fn flaky_after_editing_files_not_recorded_so_no_force_is_needed_once_they_are_cleaned_up() {
    let sd = sd();
    sd.result(r#"{outcome: "flaky", commit_message: null, summary: "flaky"}"#);
    let r = sd.run(&["ci-fix"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["edited files"]);
    assert_eq!(sd.recorded(), None);
}

// @test "flaky: nothing committed or pushed; recorded; the same red SHA is refused next time, a new one runs"
#[test]
fn flaky_nothing_committed_or_pushed_recorded_the_same_red_sha_is_refused_next_time_a_new_one_runs() {
    let sd = sd();
    sd.agent_edits_nothing();
    sd.result(r#"{outcome: "flaky", commit_message: null, summary: "a timeout in the runner, not the change"}"#);
    let (head, before) = (sd.rev("HEAD"), sd.remote_feat());
    let r = sd.run(&["ci-fix"]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &["flaky"]);
    assert_has(&r.output, &["a timeout in the runner"]);
    assert_eq!(sd.rev("HEAD"), head);
    assert_eq!(sd.remote_feat(), before);
    assert_eq!(sd.recorded().as_deref(), Some(format!("{head} flaky").as_str()));
    sd.rm("claude.args");
    let r = sd.run(&["ci-fix"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["already"]);
    assert_has(&r.output, &["flaky"]);
    assert!(!sd.ran("claude.args"));
    sd.git(&["commit", "-q", "--allow-empty", "-m", "retry"]);
    sd.git(&["push", "-q", "origin", "feat"]);
    sd.state(&[]);
    let r = sd.run(&["ci-fix"]);
    assert_eq!(r.status, 0);
    assert!(sd.ran("claude.args"));
}

// @test "--force runs again on a SHA already judged flaky"
#[test]
fn force_runs_again_on_a_sha_already_judged_flaky() {
    let sd = sd();
    sd.agent_edits_nothing();
    sd.result(r#"{outcome: "flaky", commit_message: null, summary: "flaky"}"#);
    sd.run(&["ci-fix"]);
    sd.rm("claude.args");
    let r = sd.run(&["ci-fix", "--force"]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &["sd act: warning: ci-fix already judged", "red flaky; running anyway"]);
    assert!(sd.ran("claude.args"));
}

// A record would make a later unforced run (sd watch's) refuse a red no
// agent saw (#348).
#[test]
fn flaky_forced_past_a_ci_wait_isnt_recorded_so_a_later_red_at_head_still_runs_348() {
    for edit in [r#".ci.verdict = "pending" | .ci.failing = []"#, r#".ci.verdict = "pending" | .ci.failing = [] | .ci.sha = null"#] {
        let sd = sd();
        sd.agent_edits_nothing();
        sd.result(r#"{outcome: "flaky", commit_message: null, summary: "a timeout in the runner"}"#);
        sd.state(&[edit]);
        let r = sd.run(&["ci-fix", "--force"]);
        assert_eq!(r.status, 0, "{edit}: {}", r.output);
        assert_has(&r.output, &["a timeout in the runner", "not recorded"]);
        assert_eq!(sd.recorded(), None, "{edit}");
        sd.rm("claude.args");
        sd.state(&[]);
        let r = sd.run(&["ci-fix"]);
        assert_eq!(r.status, 0, "{edit}: {}", r.output);
        assert!(sd.ran("claude.args"), "{edit}");
    }
}

#[test]
fn a_fix_forced_past_a_ci_wait_leaves_the_record_of_a_red_at_head_alone_348() {
    for edit in [r#".ci.verdict = "pending" | .ci.failing = []"#, r#".ci.verdict = "pending" | .ci.failing = [] | .ci.sha = null"#] {
        let sd = sd();
        sd.git(&["config", "branch.feat.sd-ci-fix", "r0 flaky"]);
        sd.state(&[edit]);
        let r = sd.run(&["ci-fix", "--force"]);
        assert_eq!(r.status, 0, "{edit}: {}", r.output);
        assert_eq!(sd.recorded().as_deref(), Some("r0 flaky"), "{edit}");
    }
}

// @test "fixed but changed nothing: fails"
#[test]
fn fixed_but_changed_nothing_fails() {
    let sd = sd();
    sd.agent_edits_nothing();
    let r = sd.run(&["ci-fix"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["changed nothing"]);
}

// @test "needs-input and gave-up: edits left uncommitted, nothing pushed"
#[test]
fn needs_input_and_gave_up_edits_left_uncommitted_nothing_pushed() {
    let sd = sd();
    let before = sd.remote_feat();
    sd.result(r#"{outcome: "needs-input", commit_message: null, summary: "which test is right?"}"#);
    let r = sd.run(&["ci-fix"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["which test is right?"]);
    assert!(!sd.git(&["status", "--porcelain"]).is_empty());
    sd.git(&["checkout", "-q", "file.txt"]);
    sd.result(r#"{outcome: "gave-up", commit_message: null, summary: "no idea"}"#);
    let r = sd.run(&["ci-fix"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["no idea"]);
    assert_eq!(sd.remote_feat(), before);
}

#[test]
fn gave_up_or_fixed_with_nothing_changed_is_recorded_as_a_give_up_254() {
    let sd = sd();
    sd.agent_edits_nothing();
    sd.result(r#"{outcome: "gave-up", commit_message: null, summary: "no idea"}"#);
    let r = sd.run(&["ci-fix"]);
    assert_ne!(r.status, 0);
    let rec = sd.git(&["cat-file", "blob", "refs/sheepdog/feat/question"]);
    assert_eq!(jq(&["-r", "[.step, .kind, .text] | join(\"|\")"], &rec), "ci-fix|gave-up|no idea");
    sd.ok(&["ack", "--question"]);
    sd.result(r#"{outcome: "fixed", commit_message: "Fix it", summary: "fixed the test"}"#);
    let r = sd.run(&["ci-fix"]);
    assert_ne!(r.status, 0);
    let rec = sd.git(&["cat-file", "blob", "refs/sheepdog/feat/question"]);
    assert_eq!(jq(&["-r", "[.kind, .text] | join(\"|\")"], &rec), "gave-up|said fixed but changed nothing: fixed the test");
}

// @test "needs-input: the question is recorded on HEAD; a rerun refuses at zero tokens until an ack (#167)"
#[test]
fn needs_input_the_question_is_recorded_on_head_a_rerun_refuses_at_zero_tokens_until_an_ack_167() {
    let sd = sd();
    sd.agent_edits_nothing();
    sd.result(r#"{outcome: "needs-input", commit_message: null, summary: "I changed nothing: which test is right?\n1. revert it"}"#);
    let r = sd.run(&["ci-fix"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["NEXT shows the question until feat's HEAD or worktree changes, or sd act ack --question"]);
    let rec = sd.git(&["cat-file", "blob", "refs/sheepdog/feat/question"]);
    assert_eq!(jq(&["-r", ".head"], &rec), sd.rev("HEAD"));
    assert_eq!(jq(&["-r", ".step"], &rec), "ci-fix");
    assert_eq!(jq(&["-r", ".text"], &rec), "I changed nothing: which test is right?\n1. revert it");
    sd.rm("claude.args");
    let r = sd.run(&["ci-fix"]);
    assert_ne!(r.status, 0);
    let tail = r#"sd act answer "<your answer>" feat; or answer in its session (sd act attach feat), then sd act ack --question"#;
    assert_has(&r.output, &["not running ci-fix on feat: its agent asked", "which test is right?", tail]);
    assert!(r.output.ends_with(tail), "{}", r.output);
    assert_lacks(&r.output, "--force");
    assert!(!sd.ran("claude.args"));
    let r = sd.run(&["ack", "--question"]);
    assert_has(&r.output, &["acked the agent's question on feat (ci-fix)"]);
    assert_eq!(sd.question(), None);
    sd.run(&["ci-fix"]);
    assert!(sd.ran("claude.args"));
}

// @test "needs-input: the question stops standing once HEAD changes, or with --force (#167)"
#[test]
fn needs_input_the_question_stops_standing_once_head_changes_or_with_force_167() {
    let sd = sd();
    sd.agent_edits_nothing();
    sd.result(r#"{outcome: "needs-input", commit_message: null, summary: "which?"}"#);
    sd.run(&["ci-fix"]);
    sd.rm("claude.args");
    let r = sd.run(&["ci-fix", "--force"]);
    assert_has(&r.output, &["the ci-fix agent's question on feat stands; running anyway"]);
    assert!(sd.ran("claude.args"));
    sd.rm("claude.args");
    sd.git(&["commit", "-q", "--allow-empty", "-m", "chrismo's answer"]);
    sd.git(&["push", "-q", "origin", "feat"]);
    sd.state(&[]);
    sd.run(&["ci-fix"]);
    assert!(sd.ran("claude.args"));
}

// @test "answer: resumes the asking session with chrismo's answer, its edits kept; commits, verifies, pushes, drops the question (#178)"
#[test]
fn answer_resumes_the_asking_session_with_chrismos_answer_its_edits_kept_commits_verifies_pushes_drops_the_question_178() {
    let sd = sd();
    sd.set(&["settings.verify", "grep -q THREE file.txt"]);
    sd.result(r#"{outcome: "needs-input", commit_message: null, summary: "which test is right?"}"#);
    let r = sd.run(&["ci-fix"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &[r#"sd act answer "<your answer>""#]);
    let sid = sd.git(&["config", "branch.feat.sd-session"]);
    assert!(!sd.git(&["status", "--porcelain"]).is_empty());
    sd.rm("agent");
    sd.rm("gh.calls");
    sd.result(r#"{outcome: "fixed", commit_message: "Uppercase three", summary: "kept THREE, as chrismo said"}"#);
    let r = sd.run(&["answer", "keep THREE; the test is right"]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_has(&sd.prompt(), &["keep THREE; the test is right"]);
    assert_lacks(&sd.prompt(), "rwx: expected THREE"); // the session has the logs already
    assert_eq!(sd.after("--resume"), sid);
    assert_eq!(sd.subject(), "Uppercase three");
    assert!(sd.git(&["status", "--porcelain"]).is_empty());
    assert_eq!(sd.remote_feat(), sd.rev("HEAD"));
    assert_eq!(sd.question(), None);
}

// @test "answer: the agent asking again records the new question (#178)"
#[test]
fn answer_the_agent_asking_again_records_the_new_question_178() {
    let sd = sd();
    sd.agent_edits_nothing();
    sd.result(r#"{outcome: "needs-input", commit_message: null, summary: "which test is right?"}"#);
    sd.run(&["ci-fix"]);
    sd.result(r#"{outcome: "needs-input", commit_message: null, summary: "and the other one?"}"#);
    let r = sd.run(&["answer", "the first"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["and the other one?"]);
    let rec = sd.git(&["cat-file", "blob", "refs/sheepdog/feat/question"]);
    assert_eq!(jq(&["-r", ".text"], &rec), "and the other one?");
}

fn has_answer(sd: &Sd) -> bool {
    sd.git_try(&["rev-parse", "-q", "--verify", "refs/sheepdog/feat/answer"]).is_some()
}

#[test]
fn answer_is_stored_while_ci_fix_s_gate_waits_on_ci_and_ci_fix_goes_on_with_it_once_ci_is_red_at_head_326() {
    let sd = sd();
    sd.result(r#"{outcome: "needs-input", commit_message: null, summary: "which test is right?"}"#);
    sd.run(&["ci-fix"]);
    let sid = sd.git(&["config", "branch.feat.sd-session"]);
    sd.rm("claude.prompt");
    // unpushed, so CI's red isn't for HEAD: the push restarts CI
    sd.state(&[".push.ahead = 1 | .ci.for_head = false | .ci.sha = \"r0\""]);
    let r = sd.run(&["answer", "keep THREE"]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_has(&r.output, &["ci-fix refuses on feat while CI's red isn't for HEAD", "your answer is stored", "once CI is red at HEAD"]);
    assert!(!sd.ran("claude.prompt"));
    let rec = sd.git(&["cat-file", "blob", "refs/sheepdog/feat/answer"]);
    assert_eq!(jq(&["-r", "[.step, .text] | join(\"|\")"], &rec), "ci-fix|keep THREE");
    // CI running on HEAD: still waiting
    sd.state(&[r#".ci.verdict = "pending""#]);
    let r = sd.run(&["ci-fix"]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_has(&r.output, &["CI is pending", "your answer is still stored"]);
    assert_lacks(&r.output, "going on with your stored answer");
    assert!(!sd.ran("claude.prompt"));
    // red at HEAD: ci-fix goes on with it, and it's used up
    sd.state(&[]);
    sd.result(r#"{outcome: "fixed", commit_message: "Uppercase three", summary: "kept THREE, as chrismo said"}"#);
    let r = sd.run(&["ci-fix"]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_has(&r.output, &["going on with your stored answer to the ci-fix agent's question"]);
    assert_has(&sd.prompt(), &["keep THREE"]);
    assert_eq!(sd.after("--resume"), sid);
    assert_eq!(sd.question(), None);
    assert!(!has_answer(&sd));
}

#[test]
fn a_stored_ci_fix_answer_says_sd_watch_runs_it_only_when_a_push_or_ci_run_clears_the_gate_326() {
    let sd = sd();
    sd.result(r#"{outcome: "needs-input", commit_message: null, summary: "which test is right?"}"#);
    sd.run(&["ci-fix"]);
    sd.rm("claude.prompt");
    sd.state(&[r#".ci.verdict = "pending""#]);
    let r = sd.ok(&["answer", "keep THREE"]);
    assert_has(&r, &["(NEXT then offers ci-fix, so sd watch runs ci-fix)"]);
    for (edit, yours) in [
        (r#".ci.verdict = "inconclusive""#, "CI was cancelled (rerun it: gh run rerun, or rwx retry)"),
        (r#".push.ahead = 1 | .push.held = {paths: [".rwx/tests.yml"]}"#, "there are unpushed commits, held with the agent's CI config change (review it, then sd act push)"),
        (r#".push.ahead = 1 | .verify = {at_head: true, sha: "h1", step: "ci-fix", cmd: "make test"}"#, "there are unpushed commits, held by the verify red on HEAD (fix it, then sd act verify)"),
        (r#".push.ahead = 1 | .ci.for_head = false | .ci.sha = "r0" | .parent.unpushed = true"#, "the parent branch isn't on the remote yet (ship it first (sd act ship <parent>))"),
        (r#".push.ahead = 1 | .push.remote_ours = false | .push.theirs = 1"#, "there are unpushed commits NEXT won't push (sd state says why)"),
    ] {
        sd.state(&[edit]);
        let r = sd.ok(&["answer", "keep THREE"]);
        assert_has(&r, &["once CI is red at HEAD", &format!("nothing sd watch runs clears {yours}, so that's yours to clear")]);
        assert_lacks(&r, "so sd watch runs ci-fix");
    }
    // red at HEAD, held only by what would refuse the fix's push (#348)
    sd.state(&[".sync.undone = 1"]);
    let r = sd.ok(&["answer", "keep THREE"]);
    assert_has(&r, &["ci-fix refuses on feat while the restack was undone, so a fix can't be pushed", "once a fix can be pushed"]);
    assert_lacks(&r, "red at HEAD");
    assert!(!sd.ran("claude.prompt"));
}

#[test]
fn answer_to_a_ci_fix_question_is_moot_once_ci_passed_at_head_326() {
    let sd = sd();
    sd.result(r#"{outcome: "needs-input", commit_message: null, summary: "which test is right?"}"#);
    sd.run(&["ci-fix"]);
    sd.rm("claude.prompt");
    sd.state(&[r#".ci.verdict = "green""#]);
    let r = sd.run(&["answer", "keep THREE"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["CI is green at HEAD", "the ci-fix agent's question is moot", "sd act ack --question"]);
    assert!(!sd.ran("claude.prompt"));
    assert!(!has_answer(&sd));
    // green, but not on HEAD: CI on HEAD may yet be red
    sd.state(&[r#".ci.verdict = "green" | .ci.for_head = false | .ci.sha = "r0""#]);
    sd.ok(&["answer", "keep THREE"]);
    assert!(has_answer(&sd));
}

// #355 (round 7 of #333): blocked CI is chrismo's to start or approve, not
// something sd watch clears, so a stored answer mustn't promise sd watch runs it.
#[test]
fn a_ci_fix_answer_stored_while_ci_is_blocked_is_chrismos_to_clear_not_sd_watchs_355() {
    let sd = sd();
    sd.result(r#"{outcome: "needs-input", commit_message: null, summary: "which test is right?"}"#);
    sd.run(&["ci-fix"]);
    sd.rm("claude.prompt");
    sd.state(&[r#".ci.verdict = "blocked" | .ci.failing = [] | .ci.blocked = ["deploy"]"#]);
    let r = sd.ok(&["answer", "keep THREE"]);
    assert_lacks(&r, "so sd watch runs ci-fix");
    assert_has(&r, &["once CI is red at HEAD", "nothing sd watch runs clears CI is blocked (start or approve its checks: gh pr checks), so that's yours to clear"]);
    assert!(has_answer(&sd));
    assert!(!sd.ran("claude.prompt"));
    // on an old SHA, with a push NEXT offers: the push starts CI on HEAD
    sd.state(&[r#".ci.verdict = "blocked" | .ci.failing = [] | .ci.blocked = ["deploy"] | .ci.for_head = false | .push.ahead = 1"#]);
    let r = sd.ok(&["answer", "keep THREE"]);
    assert_has(&r, &["CI is blocked, but not on HEAD", "so sd watch runs ci-fix"]);
    assert_lacks(&r, "yours to clear");
}

// #355: with no CI at all (no CI commit, so for_head is null), the question is
// moot, not an answer waiting on CI that never comes.
#[test]
fn an_answer_on_a_commit_with_no_ci_is_moot_not_stored_to_wait_forever_355() {
    let sd = sd();
    sd.result(r#"{outcome: "needs-input", commit_message: null, summary: "which test is right?"}"#);
    sd.run(&["ci-fix"]);
    sd.rm("claude.prompt");
    sd.state(&[r#".ci = {sha: null, for_head: null, verdict: "none", failing: [], pending: []}"#]);
    let r = sd.run(&["answer", "keep THREE"]);
    assert_ne!(r.status, 0, "{}", r.output);
    assert_has(&r.output, &["no CI ran at HEAD", "the ci-fix agent's question is moot", "sd act ack --question"]);
    assert!(!has_answer(&sd));
    assert!(!sd.ran("claude.prompt"));
}

// @test "answer: refused at zero tokens with no text, no question, or one that no longer stands (#178)"
#[test]
fn answer_refused_at_zero_tokens_with_no_text_no_question_or_one_that_no_longer_stands_178() {
    let sd = sd();
    let r = sd.run(&["answer"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["usage: sd act answer"]);
    let r = sd.run(&["answer", "yes"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["no question to answer on feat"]);
    sd.result(r#"{outcome: "needs-input", commit_message: null, summary: "which test is right?"}"#);
    sd.run(&["ci-fix"]);
    sd.rm("claude.args");
    append(&sd.cwd.join("file.txt"), "chrismo's edit");
    let r = sd.run(&["answer", "yes"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["the ci-fix agent's question on feat no longer stands"]);
    assert!(!sd.ran("claude.args"));
}

// @test "--no-publish: committed, not pushed"
#[test]
fn no_publish_committed_not_pushed() {
    let sd = sd();
    let before = sd.remote_feat();
    let r = sd.run(&["ci-fix", "--no-publish"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.subject(), "Uppercase three");
    assert_eq!(sd.remote_feat(), before);
    assert_has(&r.output, &["not pushed"]);
}

// @test "verify failing after the fix: commit kept, nothing pushed"
#[test]
fn verify_failing_after_the_fix_commit_kept_nothing_pushed() {
    let sd = sd();
    sd.set(&["settings.verify", "grep -q FOUR file.txt"]);
    let before = sd.remote_feat();
    let r = sd.run(&["ci-fix"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["verify failed"]);
    assert_eq!(sd.subject(), "Uppercase three");
    assert_eq!(sd.remote_feat(), before);
}

// ci-fix's push asks push's own gate (#348): with ci-fix's --force, past
// push's Soft refusals with a warning; never past its Hard one.
#[test]
fn ci_fix_s_push_asks_push_s_gate_forced_past_its_soft_refusals_with_a_warning_348() {
    let sd = sd();
    sd.state(&[".push.ahead = 1 | .parent.unpushed = true"]);
    let r = sd.run(&["ci-fix", "--force"]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_has(&r.output, &["sd act: warning: the parent branch isn't on the remote yet"]);
    assert_eq!(sd.remote_feat(), sd.rev("HEAD"));
}

// An undone restack refuses before the agent runs, as a fix couldn't be
// pushed. Forced, the agent runs, and push's Hard rule still keeps the fix.
#[test]
fn ci_fix_refuses_an_undone_restack_before_the_agent_runs_and_forced_never_pushes_past_it_348() {
    let sd = sd();
    sd.state(&[".sync.undone = 1"]);
    let before = sd.remote_feat();
    let r = sd.run(&["ci-fix"]);
    assert_ne!(r.status, 0, "{}", r.output);
    assert_has(&r.output, &["not fixing CI on feat until a fix can be pushed", "the restack was undone, so a fix can't be pushed: redo the restack", "--force"]);
    assert_lacks(&r.output, "red at HEAD");
    assert!(!sd.ran("claude.args"));
    let r = sd.run(&["ci-fix", "--force"]);
    assert_ne!(r.status, 0, "{}", r.output);
    assert_has(&r.output, &["not pushing: the restack was undone", &format!("the fix is committed in {}, not pushed", sd.path("repo"))]);
    assert_eq!(sd.subject(), "Uppercase three");
    assert_eq!(sd.remote_feat(), before);
}

// The parent not on the remote refuses before the agent runs, with nothing
// unpushed here too.
#[test]
fn ci_fix_refuses_an_unpushed_parent_before_the_agent_runs_348() {
    let sd = sd();
    sd.state(&[".parent.unpushed = true"]);
    let r = sd.run(&["ci-fix"]);
    assert_ne!(r.status, 0, "{}", r.output);
    assert_has(&r.output, &["not fixing CI on feat until a fix can be pushed", "the parent branch isn't on the remote yet: ship it first", "--force"]);
    assert_lacks(&r.output, "red at HEAD");
    assert!(!sd.ran("claude.args"));
}

// Forced past only what would refuse the fix's push, CI is red at HEAD: the
// agent isn't told it may not be, and a flaky verdict is recorded (#348).
#[test]
fn forced_past_only_a_push_refusal_the_red_is_at_head_so_flaky_is_recorded_and_the_agent_not_told_otherwise_348() {
    let sd = sd();
    sd.agent_edits_nothing();
    sd.result(r#"{outcome: "flaky", commit_message: null, summary: "a timeout in the runner"}"#);
    sd.state(&[".sync.undone = 1"]);
    let head = sd.rev("HEAD");
    let r = sd.run(&["ci-fix", "--force"]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_has(&r.output, &["sd act: warning: the restack was undone, so a fix can't be pushed"]);
    assert_lacks(&sd.prompt(), "forced past its gate");
    assert_eq!(sd.recorded().as_deref(), Some(format!("{head} flaky").as_str()));
}

// ── CI config: held for chrismo (#124) ───────────────────────────────────────

// @test "a fix that changes CI config is committed, not pushed, and held; chrismo's sd act push releases it (#124)"
#[test]
fn a_fix_that_changes_ci_config_is_committed_not_pushed_and_held_chrismos_sd_act_push_releases_it_124() {
    let sd = sd();
    sd.agent(AGENT_TOUCHES_CI);
    let before = sd.remote_feat();
    let r = sd.run(&["ci-fix"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["the agent changed CI config (.github/workflows/test.yml): review, then sd act push"]);
    assert_eq!(sd.subject(), "Uppercase three");
    assert_eq!(sd.remote_feat(), before);
    assert_eq!(sd.held(), Some(format!("{}\n.github/workflows/test.yml", sd.rev("HEAD"))));
    let r = sd.run(&["push"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.remote_feat(), sd.rev("HEAD"));
    assert_eq!(sd.git_status(&["config", "--get", "branch.feat.sd-held"]), 1);
}

// @test "a CI config change the agent committed itself is held, whatever it answers (#124)"
#[test]
fn a_ci_config_change_the_agent_committed_itself_is_held_whatever_it_answers_124() {
    let sd = sd();
    sd.agent(AGENT_COMMITS_CI);
    for r in [
        r#"{outcome: "fixed", commit_message: null, summary: "s"}"#,
        r#"{outcome: "flaky", commit_message: null, summary: "s"}"#,
        r#"{outcome: "needs-input", commit_message: null, summary: "s"}"#,
    ] {
        sd.git(&["reset", "-q", "--hard", "origin/feat"]);
        sd.git_status(&["config", "--unset", "branch.feat.sd-held"]);
        sd.git_status(&["config", "--unset", "branch.feat.sd-ci-fix"]);
        sd.result(r);
        sd.run(&["ci-fix"]);
        assert_eq!(sd.held(), Some(format!("{}\n.github/workflows/x.yml", sd.rev("HEAD"))), "for: {r}");
    }
}

// @test "a CI config change the agent committed itself is held even when the step then fails (#124)"
#[test]
fn a_ci_config_change_the_agent_committed_itself_is_held_even_when_the_step_then_fails_124() {
    let sd = sd();
    sd.agent(AGENT_COMMITS_CI);
    sd.put("result.json", "not json");
    let r = sd.run(&["ci-fix"]);
    assert_ne!(r.status, 0);
    assert_eq!(sd.held(), Some(format!("{}\n.github/workflows/x.yml", sd.rev("HEAD"))));
}

// @test "--no-publish with a CI config change: held all the same (#124)"
#[test]
fn no_publish_with_a_ci_config_change_held_all_the_same_124() {
    let sd = sd();
    sd.agent(AGENT_TOUCHES_CI);
    let r = sd.run(&["ci-fix", "--no-publish"]);
    assert_has(&r.output, &["the agent changed CI config (.github/workflows/test.yml)"]);
    assert!(sd.held().is_some_and(|h| !h.is_empty()));
}

// @test "settings.protectedPaths [] holds nothing: the fix is pushed (#124)"
#[test]
fn settings_protectedpaths_holds_nothing_the_fix_is_pushed_124() {
    let sd = sd();
    sd.agent(AGENT_TOUCHES_CI);
    sd.set(&["--json", "settings.protectedPaths", "[]"]);
    let r = sd.run(&["ci-fix"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.remote_feat(), sd.rev("HEAD"));
    assert_eq!(sd.git_status(&["config", "--get", "branch.feat.sd-held"]), 1);
}

// @test "the fix is committed under the agent's message, else the first failing check"
#[test]
fn the_fix_is_committed_under_the_agents_message_else_the_first_failing_check() {
    let sd = sd();
    sd.result(r#"{outcome: "fixed", commit_message: "", summary: "done"}"#);
    let r = sd.run(&["ci-fix"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.subject(), "Fix CI: RWX: tests.yml (pull request)");
}

// ── the prompt ───────────────────────────────────────────────────────────────

// @test "the shipped prompt is used by default"
#[test]
fn the_shipped_prompt_is_used_by_default() {
    let sd = sd();
    let r = sd.run(&["ci-fix"]);
    assert_eq!(r.status, 0);
    assert_has(&sd.prompt(), &[&shipped_prompt()]);
}

// @test "settings.ciFixPrompt names a file in the repo, read from the base commit"
#[test]
fn settings_cifixprompt_names_a_file_in_the_repo_read_from_the_base_commit() {
    let sd = sd();
    sd.git(&["switch", "-q", "main"]);
    sd.write("ci-fix.md", "OUR CI RULES: run rwx run first\n");
    sd.git(&["add", "ci-fix.md"]);
    sd.git(&["commit", "-qm", "ci guidance"]);
    sd.git(&["push", "-q", "origin", "main"]);
    sd.git(&["switch", "-q", "feat"]);
    sd.state(&[]); // old_base follows main
    sd.set(&["settings.ciFixPrompt", "ci-fix.md"]);
    let r = sd.run(&["ci-fix"]);
    assert_eq!(r.status, 0);
    assert_has(&sd.prompt(), &["OUR CI RULES: run rwx run first"]);
    assert_lacks(&sd.prompt(), &shipped_prompt());
}

// @test "settings.ciFixPrompt: an absolute path is read from disk; a missing file is an error"
#[test]
fn settings_cifixprompt_an_absolute_path_is_read_from_disk_a_missing_file_is_an_error() {
    let sd = sd();
    fs::write(sd.tmp.join("rules.md"), "ABSOLUTE RULES\n").unwrap();
    sd.set(&["settings.ciFixPrompt", &sd.path("rules.md")]);
    let r = sd.run(&["ci-fix"]);
    assert_eq!(r.status, 0);
    assert_has(&sd.prompt(), &["ABSOLUTE RULES"]);
    sd.set(&["settings.ciFixPrompt", &sd.path("nope.md")]);
    let r = sd.run(&["ci-fix"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["no such file"]);
}

// @test "no old_base from sd state: the branch's own .sheepdog/ci-fix.md is ignored"
#[test]
fn no_old_base_from_sd_state_the_branchs_own_sheepdog_ci_fix_md_is_ignored() {
    let sd = sd();
    fs::create_dir(sd.cwd.join(".sheepdog")).unwrap();
    sd.write(".sheepdog/ci-fix.md", "BRANCH-WRITTEN CI RULES\n");
    sd.git(&["add", ".sheepdog"]);
    sd.git(&["commit", "-qm", "the agent's guidance"]);
    sd.git(&["push", "-q", "origin", "feat"]);
    sd.state(&[".sync.old_base = null"]);
    let r = sd.run(&["ci-fix"]);
    assert_eq!(r.status, 0);
    assert_lacks(&sd.prompt(), "BRANCH-WRITTEN CI RULES");
    assert_has(&sd.prompt(), &[&shipped_prompt()]);
}

// @test "no old_base and no default branch to trust: a relative ciFixPrompt is refused, not read from the tip"
#[test]
fn no_old_base_and_no_default_branch_to_trust_a_relative_cifixprompt_is_refused_not_read_from_the_tip() {
    let sd = sd();
    sd.write("ci-fix.md", "BRANCH-WRITTEN CI RULES\n");
    sd.git(&["add", "ci-fix.md"]);
    sd.git(&["commit", "-qm", "the agent's guidance"]);
    sd.git(&["push", "-q", "origin", "feat"]);
    sd.state(&[".sync.old_base = null"]);
    sd.git_in("origin.git", &["update-ref", "-d", "refs/heads/main"]); // ci-fix fetches
    sd.git(&["update-ref", "-d", "refs/remotes/origin/main"]);
    sd.git(&["branch", "-qD", "main"]);
    sd.set(&["settings.ciFixPrompt", "ci-fix.md"]);
    let r = sd.run(&["ci-fix"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["no base commit to read ci-fix.md from"]);
    assert!(!sd.ran("claude.prompt"));
}

// @test "--help lists ci-fix"
#[test]
fn help_lists_ci_fix() {
    let sd = sd();
    let r = sd.run(&["--help"]);
    assert_match(&r.output, r"sd act ci-fix ");
}
