// Black-box tests of sd watch (#201): the built binary, run as sd-watch
// through a link (tests/common), making its passes over the registered
// repos. Moved from libexec/sd-watch.bats; each test keeps the bats test's
// title above it. The libexec/sd-watch link's test is in lib/config.bats,
// beside the sd-config and sd-state links'.
//
// Real git for repos, enrollment and fetch. sd state is faked: it serves
// $PS_FIXTURES/<branch>.json, so each glyph/notification rule can be driven
// directly. Notifications go to a stub configured as settings.notify.

mod common;

use common::{assert_has, assert_lacks, assert_match, chomp, jq, output, Run, Sd};
use regex::Regex;
use serde_json::Value;
use sheepdog::next::steps;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

const SD_STATE: &str = r#"#!/usr/bin/env bash
b=${@: -1}
echo "$PWD $b" >> "$PS_FIXTURES/calls"
f="$PS_FIXTURES/${b//\//__}.json"
[[ -f $f ]] || { echo "sd state: no fixture for $b" >&2; exit 1; }
cat "$f"
"#;

const NOTIFY: &str = r#"#!/usr/bin/env bash
printf '%s|%s\n' "$1" "$2" >> "$PS_FIXTURES/notified"
"#;

/// A notifier that logs each message to $PS_FIXTURES/attempts and fails.
const NOTIFY_FAILS: &str = r#"#!/usr/bin/env bash
echo "$2" >> "$PS_FIXTURES/attempts"
exit 1
"#;

/// sd state's JSON, ready to merge; $b is the branch.
const READY: &str = r#"{
    branch: $b, pr: {number: 7, state: "OPEN", draft: false, merge_state: "CLEAN", review_decision: null, review_requests: []},
    parent: {name: "main", merged: false},
    sync: {target: "main", default: "main", behind: 0, up_to_date: true, conflict: false},
    push: {remote_sha: "abc", ahead: 0, behind: 0, diverged: false, remote_ours: true},
    fresh: true, stale: [],
    ci: {verdict: "green", failing: [], pending: [], for_head: true},
    review: {local: {status: "covered"}, bot: {status: "none"}, accepted: {status: "none"}},
    threads: {owed_bot: 0, owed_human: 0, outdated: 0, blocking: 0, info: 0, items: []},
    session: {enrolled: true, locked: false}, errors: []}"#;

/// The bats setup: origin.git and its clone repo (cwd), main pushed, feat
/// checked out and enrolled, and the fake sd state and notifier first on PATH.
fn sd() -> Sd {
    let mut sd = Sd::for_verb("watch");
    let env = vec![
        ("SHEEPDOG_STATE_DIR".to_string(), sd.path("state")),
        ("STATUS_DIR".to_string(), sd.path("status")),
        ("PS_FIXTURES".to_string(), sd.path("ps")),
    ];
    sd.env = env;
    fs::create_dir(sd.tmp.join("ps")).unwrap();
    sd.fake("sd-state", SD_STATE);
    sd.fake("notify", NOTIFY);
    // Never the real macOS notifier: every config a test writes keeps this one.
    sd.verb_ok("config", &["set", "--global", "settings.notify", &sd.path("fakebin/notify")]);
    sd.origin(&["repo"]);
    sd.cd("repo");
    sd.git(&["commit", "-q", "--allow-empty", "-m", "init"]);
    sd.git(&["push", "-q", "origin", "main"]);
    sd.git(&["switch", "-qc", "feat"]);
    sd.enroll("feat");
    sd
}

/// Every path under <dir>, at any depth: find <dir> -mindepth 1.
fn find(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        out.push(p.clone());
        if p.is_dir() {
            out.extend(find(&p));
        }
    }
    out
}

fn name_of(p: &Path) -> &str {
    p.file_name().and_then(|n| n.to_str()).unwrap_or("")
}

/// `$(cat <p> 2>/dev/null)`: its text, chomped, or "" when there's none.
fn cat(p: &Path) -> String {
    chomp(&fs::read_to_string(p).unwrap_or_default())
}

fn executable(p: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

fn uid() -> String {
    chomp(&String::from_utf8_lossy(&output(std::process::Command::new("id").arg("-u"), None).stdout))
}

impl Sd {
    fn enroll(&self, b: &str) {
        self.git_in("repo", &["config", &format!("branch.{b}.sheepdog"), "true"]);
        self.register(&self.path("repo"));
    }

    fn register(&self, repo: &str) {
        let repos = self.git_try(&["config", "--global", "--get-all", "sheepdog.repo"]).unwrap_or_default();
        if !repos.lines().any(|l| l == repo) {
            self.git(&["config", "--global", "--add", "sheepdog.repo", repo]);
        }
    }

    fn enrolled(&self, b: &str) -> bool {
        self.git_try(&["config", &format!("branch.{b}.sheepdog")]).as_deref() == Some("true")
    }

    /// $PS_FIXTURES/<name>
    fn ps(&self, name: &str) -> PathBuf {
        self.tmp.join("ps").join(name)
    }

    fn fixture(&self, b: &str) -> PathBuf {
        self.ps(&format!("{}.json", b.replace('/', "__")))
    }

    /// ps_fixture <branch> [jq-assignments...] — sd state's JSON. Default: ready to merge.
    /// .next comes from the real NEXT rules (sheepdog::next), as in sd state, unless an assignment sets it.
    fn ps_fixture(&self, b: &str, assigns: &[&str]) {
        let mut j = jq(&["-n", "--arg", "b", b, READY], "");
        for a in assigns {
            j = jq(&[a], &j);
        }
        let mut v: Value = serde_json::from_str(&j).unwrap();
        if v.get("next").is_none() {
            v["next"] = steps(&v, "origin").unwrap();
        }
        fs::write(self.fixture(b), format!("{}\n", serde_json::to_string_pretty(&v).unwrap())).unwrap();
    }

    /// one_step <key>: sd state's next is that single step, said as "say <key>".
    fn one_step(&self, key: &str) {
        self.ps_fixture("feat", &[&format!(r#".next = [{{key: "{key}", who: "you", say: "say {key}", cmd: null}}]"#)]);
    }

    /// The calls sd state got, "<dir> <branch>" each.
    fn calls(&self) -> String {
        cat(&self.ps("calls"))
    }

    fn notified(&self) -> String {
        cat(&self.ps("notified"))
    }

    fn notified_lines(&self) -> Vec<String> {
        self.notified().lines().filter(|l| !l.is_empty()).map(String::from).collect()
    }

    fn attempts(&self) -> usize {
        cat(&self.ps("attempts")).lines().count()
    }

    fn last_pass(&self) -> String {
        cat(&self.tmp.join("state/last-pass"))
    }

    /// glyph <worktree>: the glyph in $STATUS_DIR/<worktree>.pr.
    fn glyph(&self, wt: &str) -> String {
        let pr = cat(&self.tmp.join(format!("status/{wt}.pr")));
        pr.lines().map(|l| l.split('\t').nth(1).unwrap_or(l)).collect::<Vec<_>>().join("\n")
    }

    fn pr_file(&self, wt: &str) -> PathBuf {
        self.tmp.join(format!("status/{wt}.pr"))
    }

    /// A pass, from the temp dir, with <env>: `run --separate-stderr pass`.
    fn pass_run(&self, env: &[(&str, &str)]) -> Run {
        let mut c = self.command(&self.bin);
        c.current_dir(&self.tmp).envs(env.iter().copied());
        let out = output(&mut c, None);
        Run {
            status: out.status.code().unwrap_or(-1),
            output: chomp(&String::from_utf8_lossy(&out.stdout)),
            stderr: chomp(&String::from_utf8_lossy(&out.stderr)),
        }
    }

    #[track_caller]
    fn pass_env(&self, env: &[(&str, &str)]) {
        let r = self.pass_run(env);
        assert_eq!(r.status, 0, "pass failed: {}\n{}", r.output, r.stderr);
    }

    #[track_caller]
    fn pass(&self) {
        self.pass_env(&[]);
    }

    fn branches(&self) -> PathBuf {
        self.tmp.join("state/branches")
    }

    /// The branch state files: find branches -name '*.json'.
    fn state_files(&self) -> Vec<PathBuf> {
        find(&self.branches()).into_iter().filter(|p| name_of(p).ends_with(".json")).collect()
    }

    /// <branch>'s state file: find branches -path "*/<branch>.json".
    fn state_file(&self, b: &str) -> PathBuf {
        let tail = format!("/{b}.json");
        find(&self.branches()).into_iter().find(|p| p.to_string_lossy().ends_with(&tail)).unwrap()
    }

    fn nested_dirs(&self) -> Vec<PathBuf> {
        let b = self.branches();
        find(&b).into_iter().filter(|p| p.is_dir() && p.strip_prefix(&b).unwrap().components().count() >= 2).collect()
    }

    fn jq_file(&self, f: &Path, args: &[&str]) {
        let s = jq(&[&["-c"], args].concat(), &fs::read_to_string(f).unwrap());
        fs::write(f, s + "\n").unwrap();
    }

    /// aged <branch> <seconds>: the branch's stale clock started that long ago.
    fn aged(&self, b: &str, secs: i64) {
        self.jq_file(&self.state_file(b), &["--argjson", "s", &secs.to_string(), ".stale.since -= $s"]);
    }

    /// write_cfg <json>: the global file, keeping the stub notifier unless <json> sets one.
    fn write_cfg(&self, c: &str) {
        let n = self.path("fakebin/notify");
        self.write_global(&jq(&["-n", "--arg", "n", &n, "--argjson", "c", c, "{settings: {notify: $n}} * $c"], ""));
    }

    fn refs(&self, prefix: &str) -> String {
        self.git(&["for-each-ref", prefix])
    }

    /// policy n|q: the step keys sd watch --help lists as notified (n) or not (q).
    fn policy(&self, want: &str) -> Vec<String> {
        let key = Regex::new(r"^  [a-z][a-z -]*:").unwrap();
        let mut m = "";
        let mut keys = Vec::new();
        for l in self.out(&["--help"]).lines() {
            if l.starts_with("Notified") {
                m = "n";
                continue;
            }
            if l.starts_with("Not notified") {
                m = "q";
                continue;
            }
            if !l.is_empty() && !l.starts_with(' ') {
                m = "";
            }
            if m == want && key.is_match(l) {
                let k = l.trim_start();
                keys.extend(k[..k.find(':').unwrap()].split(' ').filter(|s| !s.is_empty()).map(String::from));
            }
        }
        keys
    }
}

// ── basics ───────────────────────────────────────────────────────────────────

// @test "no registered repos: nothing to do, exit 0"
#[test]
fn no_registered_repos_nothing_to_do_exit_0() {
    let sd = sd();
    sd.git(&["config", "--global", "--unset-all", "sheepdog.repo"]);
    sd.pass();
    assert!(!sd.ps("calls").exists());
}

// @test "only enrolled branches are checked, from the repo directory"
#[test]
fn only_enrolled_branches_are_checked_from_the_repo_directory() {
    let sd = sd();
    sd.git(&["branch", "other"]);
    sd.ps_fixture("feat", &[]);
    sd.pass();
    assert_eq!(sd.calls(), format!("{} feat", sd.path("repo")));
}

// @test "a registered repo that no longer exists is skipped"
#[test]
fn a_registered_repo_that_no_longer_exists_is_skipped() {
    let sd = sd();
    sd.register(&sd.path("gone"));
    sd.ps_fixture("feat", &[]);
    sd.pass();
    assert_eq!(sd.calls(), format!("{} feat", sd.path("repo")));
}

// @test "fetches before checking"
#[test]
fn fetches_before_checking() {
    let sd = sd();
    sd.git(&["clone", "-q", &sd.path("origin.git"), &sd.path("other")]);
    sd.git_in("other", &["commit", "-q", "--allow-empty", "-m", "moved"]);
    sd.git_in("other", &["push", "-q", "origin", "main"]);
    sd.ps_fixture("feat", &[]);
    sd.pass();
    assert_eq!(sd.git(&["rev-parse", "origin/main"]), sd.git_in("other", &["rev-parse", "main"]));
}

// @test "one branch's sd state failing doesn't stop the others"
#[test]
fn one_branchs_sd_state_failing_doesnt_stop_the_others() {
    let sd = sd();
    sd.git(&["branch", "other"]);
    sd.enroll("other");
    sd.ps_fixture("other", &[]);
    sd.pass();
    assert!(sd.tmp.join("state/last-pass").is_file());
    assert_has(&sd.last_pass(), &["other"]);
    assert_match(&sd.last_pass(), "feat.*sd state failed");
}

// ── glyph ────────────────────────────────────────────────────────────────────

// @test "glyph is written for the worktree the branch is checked out in, with the branch"
#[test]
fn glyph_is_written_for_the_worktree_the_branch_is_checked_out_in_with_the_branch() {
    let sd = sd();
    sd.ps_fixture("feat", &[]);
    sd.pass();
    assert_eq!(cat(&sd.pr_file("repo")), "feat\t✅");
}

// @test "glyph follows a linked worktree"
#[test]
fn glyph_follows_a_linked_worktree() {
    let sd = sd();
    sd.git(&["switch", "-q", "main"]);
    sd.git(&["worktree", "add", "-q", &sd.path("feat-wt"), "feat"]);
    sd.ps_fixture("feat", &[]);
    sd.pass();
    assert_eq!(sd.glyph("feat-wt"), "✅");
    assert!(!sd.pr_file("repo").exists());
}

// @test "a branch checked out nowhere gets no glyph but is still checked"
#[test]
fn a_branch_checked_out_nowhere_gets_no_glyph_but_is_still_checked() {
    let sd = sd();
    sd.git(&["switch", "-q", "main"]);
    sd.ps_fixture("feat", &[]);
    sd.pass();
    assert!(!sd.pr_file("repo").exists());
    assert_has(&sd.last_pass(), &["feat"]);
}

// @test "a glyph left from a branch no longer enrolled is removed"
#[test]
fn a_glyph_left_from_a_branch_no_longer_enrolled_is_removed() {
    let sd = sd();
    sd.ps_fixture("feat", &[]);
    sd.pass();
    sd.git(&["config", "--unset", "branch.feat.sheepdog"]);
    sd.pass();
    assert!(!sd.pr_file("repo").exists());
}

// @test "a pass sweeps refs/sheepdog/<b>/* whose branch no longer exists"
#[test]
fn a_pass_sweeps_refs_sheepdog_b_whose_branch_no_longer_exists() {
    let sd = sd();
    sd.git(&["update-ref", "refs/sheepdog/feat/base", "HEAD"]);
    sd.git(&["update-ref", "refs/sheepdog/gone/base", "HEAD"]);
    sd.git(&["update-ref", "refs/sheepdog/gone/local", "HEAD"]);
    sd.git(&["update-ref", "refs/sheepdog/also/gone/accepted", "HEAD"]);
    sd.ps_fixture("feat", &[]);
    sd.pass();
    assert!(!sd.refs("refs/sheepdog/feat/").is_empty());
    assert!(sd.refs("refs/sheepdog/gone/").is_empty());
    assert!(sd.refs("refs/sheepdog/also/").is_empty());
}

// @test "the sweep keeps the pins of a branch that exists but isn't enrolled"
#[test]
fn the_sweep_keeps_the_pins_of_a_branch_that_exists_but_isnt_enrolled() {
    let sd = sd();
    sd.git(&["switch", "-qc", "parked"]);
    sd.git(&["switch", "-q", "feat"]);
    sd.git(&["update-ref", "refs/sheepdog/parked/base", "HEAD"]);
    sd.ps_fixture("feat", &[]);
    sd.pass();
    assert!(!sd.refs("refs/sheepdog/parked/").is_empty());
}

// @test "glyph: CI red or blocked, conflict, human items, foreign push are ❌"
#[test]
fn glyph_ci_red_or_blocked_conflict_human_items_foreign_push_are() {
    let sd = sd();
    for a in [
        r#".ci.verdict = "red""#,
        r#".ci.verdict = "blocked" | .ci.blocked = ["wf"]"#,
        ".sync.up_to_date = false | .sync.conflict = true",
        ".threads.owed_human = 1",
        ".push.behind = 1 | .push.theirs = 1 | .push.remote_ours = false",
    ] {
        sd.ps_fixture("feat", &[a]);
        sd.pass();
        assert_eq!(sd.glyph("repo"), "❌", "for: {a}");
    }
}

// @test "glyph: review findings are 🔧 while the agent fixes them, ❌ once chrismo decides"
#[test]
fn glyph_review_findings_are_while_the_agent_fixes_them_once_chrismo_decides() {
    let sd = sd();
    let f = r#".review.local.status = "uncovered" | .review.findings = {sha: "x", round: 1, n: 1, at_head: true, declined: false, cap: 3, items: []}"#;
    sd.ps_fixture("feat", &[f]);
    sd.pass();
    assert_eq!(sd.glyph("repo"), "🔧");
    sd.ps_fixture("feat", &[&format!("{f} | .review.findings.round = 3")]);
    sd.pass();
    assert_eq!(sd.glyph("repo"), "❌");
}

// @test "glyph: waiting on the review bot is ⏳ and quiet; the bot silent past its timeout is ❌ and notified"
#[test]
fn glyph_waiting_on_the_review_bot_is_and_quiet_the_bot_silent_past_its_timeout_is_and_notified() {
    let sd = sd();
    sd.ps_fixture("feat", &[r#".bot = {command: "/review-bot", timeout: 3600} | .review.bot = {status: "covered", replied: false, waited: 60}"#]);
    sd.pass();
    assert_eq!(sd.glyph("repo"), "⏳");
    assert_eq!(sd.notified(), "");
    sd.ps_fixture("feat", &[r#".bot = {command: "/review-bot", timeout: 3600} | .review.bot = {status: "covered", replied: false, waited: 9000}"#]);
    sd.pass();
    assert_eq!(sd.glyph("repo"), "❌");
    assert_has(&sd.notified(), &["hasn't answered"]);
}

// @test "glyph: work an agent will do later is 🔧"
#[test]
fn glyph_work_an_agent_will_do_later_is() {
    let sd = sd();
    for a in [
        r#".sync.up_to_date = false | .sync.behind = 2 | .sync.target = "base" | .sync.old_base = "abc""#,
        r#".sync.up_to_date = false | .parent.merged = true | .sync.old_base = "abc""#,
        ".threads.owed_bot = 1",
        ".push.ahead = 1",
        ".pr = null",
        r#".review.local.status = "uncovered""#,
    ] {
        sd.ps_fixture("feat", &[a]);
        sd.pass();
        assert_eq!(sd.glyph("repo"), "🔧", "for: {a}");
    }
}

// @test "glyph: CI pending is ⏳"
#[test]
fn glyph_ci_pending_is() {
    let sd = sd();
    sd.ps_fixture("feat", &[r#".ci.verdict = "pending""#]);
    sd.pass();
    assert_eq!(sd.glyph("repo"), "⏳");
}

// @test "glyph: unknown state (errors, not fresh) is ⚠️"
#[test]
fn glyph_unknown_state_errors_not_fresh_is() {
    let sd = sd();
    sd.ps_fixture("feat", &[r#".errors = ["gh: down"] | .fresh = null"#]);
    sd.pass();
    assert_eq!(sd.glyph("repo"), "⚠️");
}

// @test "glyph: merged 🏁, closed 🚫"
#[test]
fn glyph_merged_closed() {
    let sd = sd();
    sd.ps_fixture("feat", &[r#".pr.state = "MERGED""#]);
    sd.pass();
    assert_eq!(sd.glyph("repo"), "🏁");
    sd.ps_fixture("feat", &[r#".pr.state = "CLOSED""#]);
    sd.pass();
    assert_eq!(sd.glyph("repo"), "🚫");
}

// @test "glyph: only behind the default branch is still ✅"
#[test]
fn glyph_only_behind_the_default_branch_is_still() {
    let sd = sd();
    sd.ps_fixture("feat", &[".sync.up_to_date = false | .sync.behind = 40"]);
    sd.pass();
    assert_eq!(sd.glyph("repo"), "✅");
}

// @test "glyph: ❌ outranks 🔧"
#[test]
fn glyph_outranks() {
    let sd = sd();
    // Human items are ❌ and the push is 🔧. (ci-red would yield to the push while
    // commits are unpushed, so it can't carry this test.)
    sd.ps_fixture("feat", &[".threads.owed_human = 1 | .push.ahead = 3"]);
    sd.pass();
    assert_eq!(sd.glyph("repo"), "❌");
}

// @test "glyph: cancelled checks are 🔧, CI for an older commit is ⏳"
#[test]
fn glyph_cancelled_checks_are_ci_for_an_older_commit_is() {
    let sd = sd();
    sd.ps_fixture("feat", &[r#".ci.verdict = "inconclusive""#]);
    sd.pass();
    assert_eq!(sd.glyph("repo"), "🔧");
    sd.ps_fixture("feat", &[".ci.for_head = false"]);
    sd.pass();
    assert_eq!(sd.glyph("repo"), "⏳");
}

// @test "glyph: a repo with no checks at all can be ready"
#[test]
fn glyph_a_repo_with_no_checks_at_all_can_be_ready() {
    let sd = sd();
    sd.ps_fixture("feat", &[r#".ci.verdict = "none" | .ci.for_head = true"#]);
    sd.pass();
    assert_eq!(sd.glyph("repo"), "✅");
}

// @test "glyph: follows sd state's next steps, not its raw fields"
#[test]
fn glyph_follows_sd_states_next_steps_not_its_raw_fields() {
    let sd = sd();
    sd.ps_fixture("feat", &[r#".next = [{key: "ci-wait", who: "wait", say: "CI running", cmd: null}]"#]);
    sd.pass();
    assert_eq!(sd.glyph("repo"), "⏳");
    sd.ps_fixture("feat", &[r#".next = [{key: "bot", who: "agent", say: "address 1 bot item", cmd: null}]"#]);
    sd.pass();
    assert_eq!(sd.glyph("repo"), "🔧");
}

// @test "glyph: nothing to ship is ❌; nothing to ship with uncommitted changes is 🔧, since implement looks the same (#143)"
#[test]
fn glyph_nothing_to_ship_is_nothing_to_ship_with_uncommitted_changes_is_since_implement_looks_the_same_143() {
    let sd = sd();
    sd.ps_fixture("feat", &[r#".next = [{key: "nothing", who: "you", say: "nothing to ship: commit, or record a task", cmd: null}]"#]);
    sd.pass();
    assert_eq!(sd.glyph("repo"), "❌");
    sd.ps_fixture(
        "feat",
        &[r#".next = [{key: "nothing-dirty", who: "you", say: "nothing to ship, and the worktree has uncommitted changes: commit them", cmd: null}]"#],
    );
    sd.pass();
    assert_eq!(sd.glyph("repo"), "🔧");
}

// @test "glyph: uncommitted edits holding up an agent step are ❌ and notified: they may be an agent's that gave up (#178)"
#[test]
fn glyph_uncommitted_edits_holding_up_an_agent_step_are_and_notified_they_may_be_an_agents_that_gave_up_178() {
    let sd = sd();
    sd.ps_fixture("feat", &[r#".next = [{key: "dirty", who: "you", say: "the worktree has uncommitted changes", cmd: "git status"}]"#]);
    sd.pass();
    assert_eq!(sd.glyph("repo"), "❌");
    assert_eq!(sd.notified(), "repo feat|the worktree has uncommitted changes");
}

// @test "glyph: an agent step running is 🤖 and quiet (#170)"
#[test]
fn glyph_an_agent_step_running_is_and_quiet_170() {
    let sd = sd();
    sd.ps_fixture("feat", &[r#".next = [{key: "running", who: "wait", say: "agent: implement running since 2026-10-18T14:03:00Z", cmd: null}]"#]);
    sd.pass();
    assert_eq!(sd.glyph("repo"), "🤖");
    assert_eq!(sd.notified(), "");
}

// @test "glyph: info-only bot items, waiting on chrismo's read, are ❌ and notified once (#76)"
#[test]
fn glyph_info_only_bot_items_waiting_on_chrismos_read_are_and_notified_once_76() {
    let sd = sd();
    sd.ps_fixture("feat", &[".threads.owed_bot = 1 | .threads.info = 1"]);
    sd.pass();
    assert_eq!(sd.glyph("repo"), "❌");
    sd.pass();
    assert_eq!(sd.notified(), "repo feat|read 1 bot item in sd state --long, then ack");
}

// @test "glyph: sd state output with no next steps is ⚠️"
#[test]
fn glyph_sd_state_output_with_no_next_steps_is() {
    let sd = sd();
    sd.ps_fixture("feat", &[".next = null"]);
    sd.pass();
    assert_eq!(sd.glyph("repo"), "⚠️");
    assert_match(&sd.last_pass(), "feat.*no next steps");
}

// @test "glyph: waiting on a required review is ⏳, never ✅"
#[test]
fn glyph_waiting_on_a_required_review_is_never() {
    let sd = sd();
    sd.ps_fixture("feat", &[r#".pr.merge_state = "BLOCKED" | .pr.review_decision = "REVIEW_REQUIRED" | .pr.review_requests = ["alice"]"#]);
    sd.pass();
    assert_eq!(sd.glyph("repo"), "⏳");
    assert_eq!(sd.notified(), "");
}

// @test "glyph: GitHub blocking the merge for another reason is 🔧, never ✅"
#[test]
fn glyph_github_blocking_the_merge_for_another_reason_is_never() {
    let sd = sd();
    sd.ps_fixture("feat", &[r#".pr.merge_state = "BLOCKED""#]);
    sd.pass();
    assert_eq!(sd.glyph("repo"), "🔧");
    assert_eq!(sd.notified(), "");
}

// @test "glyph: a draft is never ready"
#[test]
fn glyph_a_draft_is_never_ready() {
    let sd = sd();
    sd.ps_fixture("feat", &[".pr.draft = true"]);
    sd.pass();
    assert_eq!(sd.glyph("repo"), "🔧");
    assert_eq!(sd.notified(), "");
}

// @test "glyph: sd state failing shows ⚠️ instead of dropping the glyph"
#[test]
fn glyph_sd_state_failing_shows_instead_of_dropping_the_glyph() {
    let sd = sd();
    sd.ps_fixture("feat", &[]);
    sd.pass();
    fs::remove_file(sd.fixture("feat")).unwrap();
    sd.pass();
    assert_eq!(sd.glyph("repo"), "⚠️");
}

// @test "unparseable sd state output is ⚠️, and the next good pass still notifies"
#[test]
fn unparseable_sd_state_output_is_and_the_next_good_pass_still_notifies() {
    let sd = sd();
    fs::write(sd.fixture("feat"), "warning: junk\n{\"branch\":").unwrap();
    sd.pass();
    assert_eq!(sd.glyph("repo"), "⚠️");
    assert_match(&sd.last_pass(), "feat.*unparseable");
    sd.ps_fixture("feat", &[r#".ci.verdict = "red" | .ci.failing = ["lint"]"#]);
    sd.pass();
    assert_eq!(sd.notified(), "repo feat|fix CI: lint");
}

// @test "sd state stderr on success doesn't corrupt its JSON"
#[test]
fn sd_state_stderr_on_success_doesnt_corrupt_its_json() {
    let sd = sd();
    sd.ps_fixture("feat", &[]);
    sd.fake("sd-state", "#!/usr/bin/env bash\necho \"warning: something\" >&2\ncat \"$PS_FIXTURES/${@: -1}.json\"\n");
    sd.pass();
    assert_eq!(sd.glyph("repo"), "✅");
}

// @test "a hung sd state times out instead of stalling the pass"
#[test]
fn a_hung_sd_state_times_out_instead_of_stalling_the_pass() {
    let sd = sd();
    sd.git(&["branch", "other"]);
    sd.enroll("other");
    sd.ps_fixture("other", &[]);
    sd.fake("sd-state", "#!/usr/bin/env bash\n[[ ${@: -1} == feat ]] && sleep 30\ncat \"$PS_FIXTURES/${@: -1}.json\"\n");
    let t = Instant::now();
    sd.pass_env(&[("SHEEPDOG_WATCH_TIMEOUT", "1")]);
    assert!(t.elapsed().as_secs() < 10);
    assert_match(&sd.last_pass(), "feat.*sd state failed");
    assert_match(&sd.last_pass(), "other.*✅");
}

// @test "a failed state write leaves no temp file behind"
#[test]
fn a_failed_state_write_leaves_no_temp_file_behind() {
    let sd = sd();
    sd.ps_fixture("feat", &[]);
    sd.pass();
    // A directory where the state file goes: the temp file is written, and
    // the rename onto it fails.
    let f = sd.state_file("feat");
    fs::remove_file(&f).unwrap();
    fs::create_dir(&f).unwrap();
    let r = sd.pass_run(&[]);
    assert_has(&r.stderr, &["couldn't save", "feat.json"]);
    assert!(!find(&sd.branches()).iter().any(|p| name_of(p).ends_with(".tmp")));
    assert!(!find(&sd.branches()).iter().any(|p| name_of(p) == "feat.json" && p.is_file()));
}

// @test "a failed fetch keeps git's reason"
#[test]
fn a_failed_fetch_keeps_gits_reason() {
    let sd = sd();
    sd.git(&["remote", "set-url", "origin", &sd.path("nowhere.git")]);
    sd.ps_fixture("feat", &[]);
    sd.pass();
    assert_match(&sd.last_pass(), "fetch from origin failed: .*nowhere");
}

// ── notifications ────────────────────────────────────────────────────────────

// @test "notify: ready to merge, once"
#[test]
fn notify_ready_to_merge_once() {
    let sd = sd();
    sd.ps_fixture("feat", &[]);
    sd.pass();
    sd.pass();
    assert_eq!(sd.notified(), "repo feat|ready to merge");
}

// @test "notify: CI red names the failing checks"
#[test]
fn notify_ci_red_names_the_failing_checks() {
    let sd = sd();
    sd.ps_fixture("feat", &[r#".ci.verdict = "red" | .ci.failing = ["lint", "test"]"#]);
    sd.pass();
    assert_eq!(sd.notified(), "repo feat|fix CI: lint, test");
}

// @test "notify: a reason that clears and comes back notifies again"
#[test]
fn notify_a_reason_that_clears_and_comes_back_notifies_again() {
    let sd = sd();
    sd.ps_fixture("feat", &[r#".ci.verdict = "red" | .ci.failing = ["lint"]"#]);
    sd.pass();
    sd.ps_fixture("feat", &[r#".ci.verdict = "pending""#]);
    sd.pass();
    sd.ps_fixture("feat", &[r#".ci.verdict = "red" | .ci.failing = ["lint"]"#]);
    sd.pass();
    assert_eq!(sd.notified().lines().filter(|l| l.contains("fix CI")).count(), 2);
}

// @test "notify: agent-later work and pending CI stay quiet"
#[test]
fn notify_agent_later_work_and_pending_ci_stay_quiet() {
    let sd = sd();
    sd.ps_fixture("feat", &[r#".ci.verdict = "pending" | .push.ahead = 1 | .threads.owed_bot = 2"#]);
    sd.pass();
    assert_eq!(sd.notified(), "");
}

// @test "notify: a restack undone from outside is ❌ and notifies (#47)"
#[test]
fn notify_a_restack_undone_from_outside_is_and_notifies_47() {
    let sd = sd();
    sd.ps_fixture("feat", &[".sync.undone = 1 | .sync.up_to_date = false"]);
    sd.pass();
    assert_eq!(
        sd.notified_lines()[0],
        "repo feat|the restack was undone (1 of main's commits is a copy on feat; a plain git push after a restack?): redo it, and don't sd act push before that, which would publish the copies"
    );
    let r = sd.run(&["status"]);
    assert_has(&r.output, &["feat", "❌"]);
}

// @test "notify: CI config an agent changed is ❌ and notifies; chrismo reviews and pushes (#124)"
#[test]
fn notify_ci_config_an_agent_changed_is_and_notifies_chrismo_reviews_and_pushes_124() {
    let sd = sd();
    sd.ps_fixture("feat", &[r#".push.ahead = 1 | .push.held = {"sha": "abc", "paths": [".github/workflows/test.yml"]}"#]);
    sd.pass();
    assert_eq!(
        sd.notified_lines()[0],
        "repo feat|the agent changed CI config (.github/workflows/test.yml): review, then sd act push"
    );
    let r = sd.run(&["status"]);
    assert_has(&r.output, &["feat", "❌"]);
}

// @test "notify: human items, conflict, foreign push, merged"
#[test]
fn notify_human_items_conflict_foreign_push_merged() {
    let sd = sd();
    sd.ps_fixture("feat", &[".threads.owed_human = 2"]);
    sd.pass();
    sd.ps_fixture("feat", &[".sync.up_to_date = false | .sync.conflict = true"]);
    sd.pass();
    sd.ps_fixture("feat", &[".push.behind = 1 | .push.theirs = 1 | .push.remote_ours = false"]);
    sd.pass();
    sd.ps_fixture("feat", &[r#".pr.state = "MERGED""#]);
    sd.pass();
    let lines = sd.notified_lines();
    assert_eq!(lines[0], "repo feat|reply to 2 human items");
    assert_eq!(lines[1], "repo feat|restack onto main conflicts: resolve it");
    assert_eq!(lines[2], "repo feat|origin/feat has 1 commit this copy has never had");
    assert_eq!(lines[3], "repo feat|merged: drop the branch's pins and enrollment, remove its worktree and delete it");
}

// ── a branch deleted with its state left behind (#64) ───────────────────────

impl Sd {
    /// gh_pull <json>: what the fake gh serves for GET repos/{owner}/{repo}/pulls/<n>.
    fn gh_pull(&self, json: &str) {
        fs::write(self.ps("gh-pull"), format!("{json}\n")).unwrap();
        self.fake(
            "gh",
            r#"#!/usr/bin/env bash
echo "$*" >> "$PS_FIXTURES/gh-calls"
echo "${GH_REPO-unset}" >> "$PS_FIXTURES/gh-repo"
[[ -f $PS_FIXTURES/gh-pull ]] || { echo "gh: API rate limit exceeded" >&2; exit 1; }
cat "$PS_FIXTURES/gh-pull"
"#,
        );
    }

    /// gone <branch>: what gh pr merge --delete-branch does from the branch's checkout.
    fn gone(&self, b: &str) {
        self.git(&["switch", "-q", "main"]);
        self.git(&["branch", "-qD", b]);
    }
}

// @test "gone: an enrolled branch deleted after its PR merged is notified merged once, and its state dropped"
#[test]
fn gone_an_enrolled_branch_deleted_after_its_pr_merged_is_notified_merged_once_and_its_state_dropped() {
    let sd = sd();
    sd.ps_fixture("feat", &[r#".ci.verdict = "pending""#]);
    sd.pass();
    sd.gone("feat");
    sd.gh_pull(r#"{"state": "closed", "merged": true}"#);
    sd.pass();
    assert_eq!(sd.notified(), "repo feat|merged (#7); the local branch is already gone");
    assert_match(&sd.last_pass(), r"feat.*🏁.*merged \(#7\)");
    assert!(sd.state_files().is_empty());
    sd.pass();
    assert_eq!(sd.notified().lines().count(), 1);
    assert_eq!(cat(&sd.ps("gh-calls")), "api repos/{owner}/{repo}/pulls/7");
}

// @test "gone: a branch with a slash in its name is found"
#[test]
fn gone_a_branch_with_a_slash_in_its_name_is_found() {
    let sd = sd();
    sd.git(&["switch", "-qc", "12/doc"]);
    sd.enroll("12/doc");
    sd.ps_fixture("12/doc", &[r#".ci.verdict = "pending""#]);
    sd.pass();
    sd.gone("12/doc");
    sd.gh_pull(r#"{"state": "closed", "merged": true}"#);
    sd.pass();
    assert_has(&sd.notified(), &["repo 12/doc|merged (#7)"]);
    assert!(!sd.state_files().iter().any(|p| p.to_string_lossy().contains("doc")));
    assert!(sd.nested_dirs().is_empty());
}

// @test "gone: GitHub not answering keeps the state, and the next pass tries again"
#[test]
fn gone_github_not_answering_keeps_the_state_and_the_next_pass_tries_again() {
    let sd = sd();
    sd.ps_fixture("feat", &[r#".ci.verdict = "pending""#]);
    sd.pass();
    sd.gone("feat");
    sd.gh_pull("{}");
    fs::remove_file(sd.ps("gh-pull")).unwrap();
    sd.pass();
    assert_match(&sd.last_pass(), "feat.*⚠️.*branch is gone; GitHub didn't say what became of #7");
    assert!(!sd.state_files().is_empty());
    assert_eq!(sd.notified(), "");
    sd.gh_pull(r#"{"state": "closed", "merged": true}"#);
    sd.pass();
    assert_eq!(sd.notified(), "repo feat|merged (#7); the local branch is already gone");
}

// @test "gone: a PR GitHub says doesn't exist drops the state"
#[test]
fn gone_a_pr_github_says_doesnt_exist_drops_the_state() {
    let sd = sd();
    sd.git(&["switch", "-qc", "12/doc"]);
    sd.enroll("12/doc");
    sd.ps_fixture("12/doc", &[r#".ci.verdict = "pending""#]);
    sd.pass();
    sd.gone("12/doc");
    sd.fake("gh", "#!/usr/bin/env bash\necho \"gh: Not Found (HTTP 404)\" >&2\nexit 1\n");
    sd.pass();
    assert_match(&sd.last_pass(), "12/doc.*⚠️.*branch is gone; GitHub has no #7");
    assert!(!sd.state_files().iter().any(|p| p.to_string_lossy().contains("doc")));
    assert!(sd.nested_dirs().is_empty());
    assert_eq!(sd.notified(), "");
}

// @test "gone: the PR is asked for by its URL, not gh's default repo"
#[test]
fn gone_the_pr_is_asked_for_by_its_url_not_ghs_default_repo() {
    let sd = sd();
    sd.ps_fixture("feat", &[r#".ci.verdict = "pending" | .pr.url = "https://github.com/up/stream/pull/7""#]);
    sd.pass();
    sd.gone("feat");
    sd.gh_pull(r#"{"state": "closed", "merged": true}"#);
    sd.pass();
    assert_eq!(cat(&sd.ps("gh-calls")), "api repos/up/stream/pulls/7");
}

// @test "gone: a fork's clone asks GitHub about the repo file's settings.remote, not gh's pick among the remotes"
#[test]
fn gone_a_forks_clone_asks_github_about_the_repo_files_settings_remote_not_ghs_pick_among_the_remotes() {
    let mut sd = sd();
    sd.git(&["remote", "add", "fork", "git@github.com:me/fork.git"]);
    sd.verb_ok("config", &["set", "--repo", "settings.remote", "fork"]);
    sd.env.push(("GIT_SSH_COMMAND".into(), "false".into())); // the fork is never reached: no network
    sd.ps_fixture("feat", &[r#".ci.verdict = "pending""#]);
    sd.pass();
    sd.gone("feat");
    sd.gh_pull(r#"{"state": "closed", "merged": true}"#);
    sd.pass();
    assert_eq!(cat(&sd.ps("gh-repo")), "github.com/me/fork");
    assert_eq!(sd.notified(), "repo feat|merged (#7); the local branch is already gone");
}

// @test "gone: a repo file that can't be read is said, not blamed on GitHub, and the state kept"
#[test]
fn gone_a_repo_file_that_cant_be_read_is_said_not_blamed_on_github_and_the_state_kept() {
    let sd = sd();
    sd.ps_fixture("feat", &[r#".ci.verdict = "pending""#]);
    sd.pass();
    sd.gone("feat");
    sd.gh_pull(r#"{"state": "closed", "merged": true}"#);
    let common_dir = sd.git(&["rev-parse", "--git-common-dir"]);
    fs::write(sd.cwd.join(common_dir).join("sheepdog.json"), "{not json\n").unwrap();
    sd.pass();
    assert_match(&sd.last_pass(), "feat.*⚠️.*branch is gone; sd config failed");
    assert_lacks(&sd.last_pass(), "didn't say");
    assert!(!sd.ps("gh-calls").exists());
    assert!(!sd.state_files().is_empty());
}

// @test "gone: a PR that wasn't merged drops the state quietly"
#[test]
fn gone_a_pr_that_wasnt_merged_drops_the_state_quietly() {
    let sd = sd();
    sd.ps_fixture("feat", &[r#".ci.verdict = "pending""#]);
    sd.pass();
    sd.gone("feat");
    sd.gh_pull(r#"{"state": "closed", "merged": false}"#);
    sd.pass();
    assert_eq!(sd.notified(), "");
    assert!(sd.state_files().is_empty());
    assert_match(&sd.last_pass(), "feat.*branch is gone; #7 is closed, not merged");
}

// @test "gone: a branch that never had a PR drops the state without asking GitHub"
#[test]
fn gone_a_branch_that_never_had_a_pr_drops_the_state_without_asking_github() {
    let sd = sd();
    sd.ps_fixture("feat", &[".pr = null"]);
    sd.pass();
    sd.gone("feat");
    sd.gh_pull(r#"{"state": "closed", "merged": true}"#);
    sd.pass();
    assert!(sd.state_files().is_empty());
    assert!(!sd.ps("gh-calls").exists());
}

// @test "gone: a branch that still exists but was unenrolled has its state dropped, quietly"
#[test]
fn gone_a_branch_that_still_exists_but_was_unenrolled_has_its_state_dropped_quietly() {
    let sd = sd();
    sd.ps_fixture("feat", &[r#".ci.verdict = "pending""#]);
    sd.pass();
    sd.git(&["config", "--unset", "branch.feat.sheepdog"]);
    sd.gh_pull(r#"{"state": "closed", "merged": true}"#);
    sd.pass();
    assert!(sd.state_files().is_empty());
    assert!(!sd.ps("gh-calls").exists());
    sd.gone("feat");
    sd.pass();
    assert_eq!(sd.notified(), "");
}

// @test "gone: a merge already notified doesn't ask GitHub or add a line when the branch goes"
#[test]
fn gone_a_merge_already_notified_doesnt_ask_github_or_add_a_line_when_the_branch_goes() {
    let sd = sd();
    sd.ps_fixture("feat", &[r#".pr.state = "MERGED""#]);
    sd.pass();
    sd.gone("feat");
    sd.gh_pull(r#"{"state": "closed", "merged": true}"#);
    sd.pass();
    assert!(!sd.ps("gh-calls").exists());
    assert_lacks(&sd.last_pass(), "already gone");
    assert!(sd.state_files().is_empty());
}

// @test "gone: a merge notification that fails is tried once more, then given up on"
#[test]
fn gone_a_merge_notification_that_fails_is_tried_once_more_then_given_up_on() {
    let sd = sd();
    sd.fake("notify", NOTIFY_FAILS);
    sd.ps_fixture("feat", &[r#".ci.verdict = "pending""#]);
    sd.pass();
    sd.gone("feat");
    sd.gh_pull(r#"{"state": "closed", "merged": true}"#);
    sd.pass();
    assert!(!sd.state_files().is_empty());
    let r = sd.pass_run(&[]);
    assert_has(&r.stderr, &["giving up on: merged (#7)"]);
    sd.pass();
    assert_eq!(cat(&sd.ps("attempts")).lines().filter(|l| l.contains("merged")).count(), 2);
    assert!(sd.state_files().is_empty());
}

// @test "gone: a PR still open is ⚠️, not 🚫"
#[test]
fn gone_a_pr_still_open_is_not() {
    let sd = sd();
    sd.ps_fixture("feat", &[r#".ci.verdict = "pending""#]);
    sd.pass();
    sd.gone("feat");
    sd.gh_pull(r#"{"state": "open", "merged": false}"#);
    sd.pass();
    assert_match(&sd.last_pass(), "feat.*⚠️.*branch is gone; #7 is open, not merged");
}

// ── notification policy (#52) ────────────────────────────────────────────────

// @test "policy: every NEXT step (src/next.rs) is in --help, notified or not, exactly once"
#[test]
fn policy_every_next_step_src_next_rs_is_in_help_notified_or_not_exactly_once() {
    let sd = sd();
    let src = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/next.rs")).unwrap();
    let mut steps: Vec<&str> =
        Regex::new(r#"(?:step\(|agent\(budget, )"([a-z-]+)""#).unwrap().captures_iter(&src).map(|c| c.get(1).unwrap().as_str()).collect();
    steps.sort();
    steps.dedup();
    let mut listed = [sd.policy("n"), sd.policy("q")].concat();
    listed.sort();
    assert!(!sd.policy("n").is_empty());
    assert_eq!(steps, listed);
}

// @test "policy: a notified step notifies once on entering it, not on later passes"
#[test]
fn policy_a_notified_step_notifies_once_on_entering_it_not_on_later_passes() {
    let sd = sd();
    for k in sd.policy("n") {
        let _ = fs::remove_dir_all(sd.tmp.join("state"));
        let _ = fs::remove_file(sd.ps("notified"));
        sd.one_step(&k);
        sd.pass();
        sd.pass();
        sd.pass();
        assert_eq!(sd.notified(), format!("repo feat|say {k}"), "for {k}");
    }
}

// @test "policy: a step that isn't notified never is"
#[test]
fn policy_a_step_that_isnt_notified_never_is() {
    let sd = sd();
    for k in sd.policy("q") {
        let _ = fs::remove_dir_all(sd.tmp.join("state"));
        sd.one_step(&k);
        sd.pass();
        assert_eq!(sd.notified(), "", "for {k}");
    }
}

// @test "policy: a parent closed without merging is ❌ and notifies"
#[test]
fn policy_a_parent_closed_without_merging_is_and_notifies() {
    let sd = sd();
    sd.ps_fixture("feat", &[r#".pr.base = "par" | .parent = {name: "par", merged: false, pr: {number: 5, state: "CLOSED"}}"#]);
    sd.pass();
    assert_eq!(sd.glyph("repo"), "❌");
    assert!(sd.notified().starts_with("repo feat|par (#5) was closed without merging"), "{}", sd.notified());
}

// @test "notify: a conflict after CI red was notified still notifies, once"
#[test]
fn notify_a_conflict_after_ci_red_was_notified_still_notifies_once() {
    let sd = sd();
    sd.ps_fixture("feat", &[r#".ci.verdict = "red" | .ci.failing = ["lint"]"#]);
    sd.pass();
    sd.ps_fixture("feat", &[r#".ci.verdict = "red" | .ci.failing = ["lint"] | .sync.up_to_date = false | .sync.conflict = true"#]);
    sd.pass();
    sd.pass();
    let lines = sd.notified_lines();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0], "repo feat|fix CI: lint");
    assert_eq!(lines[1], "repo feat|restack onto main conflicts: resolve it");
}

// @test "notify: a conflict after ready to merge was notified still notifies, once"
#[test]
fn notify_a_conflict_after_ready_to_merge_was_notified_still_notifies_once() {
    let sd = sd();
    sd.ps_fixture("feat", &[]);
    sd.pass();
    sd.ps_fixture("feat", &[".sync.up_to_date = false | .sync.conflict = true"]);
    sd.pass();
    sd.pass();
    let lines = sd.notified_lines();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[1], "repo feat|restack onto main conflicts: resolve it");
}

// @test "notify: an unknown pass (sd state errors) between two doesn't re-notify what's still true"
#[test]
fn notify_an_unknown_pass_sd_state_errors_between_two_doesnt_re_notify_whats_still_true() {
    let sd = sd();
    sd.ps_fixture("feat", &[".sync.up_to_date = false | .sync.conflict = true"]);
    sd.pass();
    sd.ps_fixture("feat", &[r#".errors = ["gh: down"]"#]);
    sd.pass();
    assert_eq!(sd.glyph("repo"), "⚠️");
    sd.ps_fixture("feat", &[".sync.up_to_date = false | .sync.conflict = true"]);
    sd.pass();
    assert_eq!(sd.notified(), "repo feat|restack onto main conflicts: resolve it");
}

// @test "notify: errors alongside a ❌ step still keep what was notified"
#[test]
fn notify_errors_alongside_a_step_still_keep_what_was_notified() {
    let sd = sd();
    sd.ps_fixture("feat", &[r#".ci.verdict = "red" | .ci.failing = ["lint"] | .threads.owed_human = 1"#]);
    sd.pass();
    sd.ps_fixture("feat", &[r#".ci.verdict = "red" | .ci.failing = ["lint"] | .errors = ["gh: down"]"#]);
    sd.pass();
    assert_eq!(sd.glyph("repo"), "❌");
    sd.ps_fixture("feat", &[r#".ci.verdict = "red" | .ci.failing = ["lint"] | .threads.owed_human = 1"#]);
    sd.pass();
    assert_eq!(sd.notified().lines().filter(|l| l.contains("human item")).count(), 1);
}

// @test "notify: a state file from before notified was recorded doesn't re-notify"
#[test]
fn notify_a_state_file_from_before_notified_was_recorded_doesnt_re_notify() {
    let sd = sd();
    sd.ps_fixture("feat", &[]);
    sd.pass();
    sd.jq_file(&sd.state_file("feat"), &["del(.notified)"]);
    sd.pass();
    assert_eq!(sd.notified(), "repo feat|ready to merge");
}

// @test "notify: sd state failing between two passes doesn't re-notify what's still true"
#[test]
fn notify_sd_state_failing_between_two_passes_doesnt_re_notify_whats_still_true() {
    let sd = sd();
    sd.ps_fixture("feat", &[".sync.up_to_date = false | .sync.conflict = true"]);
    sd.pass();
    fs::rename(sd.fixture("feat"), sd.tmp.join("feat.json")).unwrap();
    sd.pass();
    fs::rename(sd.tmp.join("feat.json"), sd.fixture("feat")).unwrap();
    sd.pass();
    assert_eq!(sd.notified(), "repo feat|restack onto main conflicts: resolve it");
}

// @test "notify: a pass with no next steps between two doesn't re-notify what's still true"
#[test]
fn notify_a_pass_with_no_next_steps_between_two_doesnt_re_notify_whats_still_true() {
    let sd = sd();
    sd.ps_fixture("feat", &[".sync.up_to_date = false | .sync.conflict = true"]);
    sd.pass();
    sd.ps_fixture("feat", &[".next = []"]);
    sd.pass();
    assert_eq!(sd.glyph("repo"), "⚠️");
    sd.ps_fixture("feat", &[".sync.up_to_date = false | .sync.conflict = true"]);
    sd.pass();
    assert_eq!(sd.notified(), "repo feat|restack onto main conflicts: resolve it");
}

// @test "notify: a notification that fails is tried again next pass, then not again"
#[test]
fn notify_a_notification_that_fails_is_tried_again_next_pass_then_not_again() {
    let sd = sd();
    sd.fake(
        "notify",
        "#!/usr/bin/env bash\n[[ -e $PS_FIXTURES/notify-works ]] || exit 1\nprintf '%s|%s\\n' \"$1\" \"$2\" >> \"$PS_FIXTURES/notified\"\n",
    );
    sd.ps_fixture("feat", &[".sync.up_to_date = false | .sync.conflict = true"]);
    let r = sd.pass_run(&[]);
    assert_has(&r.stderr, &["notify failed"]);
    assert_eq!(sd.notified(), "");
    fs::write(sd.ps("notify-works"), "").unwrap();
    sd.pass();
    sd.pass();
    assert_eq!(sd.notified(), "repo feat|restack onto main conflicts: resolve it");
}

// @test "notify: a notifier that keeps failing is tried twice, then given up on"
#[test]
fn notify_a_notifier_that_keeps_failing_is_tried_twice_then_given_up_on() {
    let sd = sd();
    sd.fake("notify", NOTIFY_FAILS);
    sd.ps_fixture("feat", &[".sync.up_to_date = false | .sync.conflict = true"]);
    sd.pass();
    let r = sd.pass_run(&[]);
    assert_has(&r.stderr, &["giving up on: restack onto main conflicts"]);
    sd.pass();
    sd.pass();
    assert_eq!(sd.attempts(), 2);
}

// @test "notify: GitHub recomputing mergeability doesn't make ready re-notify"
#[test]
fn notify_github_recomputing_mergeability_doesnt_make_ready_re_notify() {
    let sd = sd();
    sd.ps_fixture("feat", &[]);
    sd.pass();
    sd.one_step("merge-wait");
    sd.pass();
    sd.ps_fixture("feat", &[]);
    sd.pass();
    assert_eq!(sd.notified(), "repo feat|ready to merge");
}

// @test "notify: an attached session hiding ready doesn't make ready re-notify"
#[test]
fn notify_an_attached_session_hiding_ready_doesnt_make_ready_re_notify() {
    let sd = sd();
    sd.ps_fixture("feat", &[]);
    sd.pass();
    sd.one_step("attached");
    sd.pass();
    sd.ps_fixture("feat", &[]);
    sd.pass();
    assert_eq!(sd.notified(), "repo feat|ready to merge");
}

// @test "notify: an agent step hiding every other step doesn't make them re-notify (#170)"
#[test]
fn notify_an_agent_step_hiding_every_other_step_doesnt_make_them_re_notify_170() {
    let sd = sd();
    sd.ps_fixture("feat", &[r#".ci.verdict = "red" | .ci.failing = ["lint"] | .threads.owed_human = 1"#]);
    sd.pass();
    sd.one_step("running");
    sd.pass();
    sd.ps_fixture("feat", &[r#".ci.verdict = "red" | .ci.failing = ["lint"] | .threads.owed_human = 1"#]);
    sd.pass();
    assert_eq!(sd.notified().lines().count(), 2);
}

// @test "notify: merge-wait doesn't reset a failed ready's one retry"
#[test]
fn notify_merge_wait_doesnt_reset_a_failed_readys_one_retry() {
    let sd = sd();
    sd.fake("notify", NOTIFY_FAILS);
    for _ in 1..=3 {
        sd.ps_fixture("feat", &[]);
        sd.pass();
        sd.one_step("merge-wait");
        sd.pass();
    }
    assert_eq!(sd.attempts(), 2);
}

// @test "notify: merge-wait carries only ready: a conflict that comes back after it notifies"
#[test]
fn notify_merge_wait_carries_only_ready_a_conflict_that_comes_back_after_it_notifies() {
    let sd = sd();
    sd.ps_fixture("feat", &[".sync.up_to_date = false | .sync.conflict = true"]);
    sd.pass();
    sd.one_step("merge-wait");
    sd.pass();
    sd.ps_fixture("feat", &[".sync.up_to_date = false | .sync.conflict = true"]);
    sd.pass();
    assert_eq!(sd.notified().lines().filter(|l| l.contains("conflicts")).count(), 2);
}

// @test "notify: an unsure pass doesn't reset a failed notification's one retry"
#[test]
fn notify_an_unsure_pass_doesnt_reset_a_failed_notifications_one_retry() {
    let sd = sd();
    sd.fake("notify", NOTIFY_FAILS);
    sd.ps_fixture("feat", &[".sync.up_to_date = false | .sync.conflict = true"]);
    sd.pass();
    sd.ps_fixture("feat", &[r#".errors = ["gh: down"]"#]);
    sd.pass();
    sd.ps_fixture("feat", &[".sync.up_to_date = false | .sync.conflict = true"]);
    sd.pass();
    sd.pass();
    assert_eq!(sd.attempts(), 2);
}

// @test "notify: a notifier that reads stdin doesn't swallow later reasons"
#[test]
fn notify_a_notifier_that_reads_stdin_doesnt_swallow_later_reasons() {
    let sd = sd();
    sd.fake("notify", "#!/usr/bin/env bash\ncat >/dev/null\nprintf '%s|%s\\n' \"$1\" \"$2\" >> \"$PS_FIXTURES/notified\"\n");
    sd.ps_fixture("feat", &[r#".ci.verdict = "red" | .ci.failing = ["lint"] | .threads.owed_human = 1"#]);
    sd.pass();
    assert_eq!(sd.notified().lines().count(), 2);
}

// @test "notify: notifier output stays out of the pass record"
#[test]
fn notify_notifier_output_stays_out_of_the_pass_record() {
    let sd = sd();
    sd.fake("notify", "#!/usr/bin/env bash\necho \"NOISE\"\n");
    sd.ps_fixture("feat", &[]);
    sd.pass();
    assert_lacks(&sd.last_pass(), "NOISE");
}

// @test "notify: a configured command with a space in its path works"
#[test]
fn notify_a_configured_command_with_a_space_in_its_path_works() {
    let sd = sd();
    fs::create_dir_all(sd.tmp.join("my tools")).unwrap();
    // cp fakebin/notify, through fake() so no spawn inherits its open fd (ETXTBSY)
    sd.fake("../my tools/notify", NOTIFY);
    sd.verb_ok("config", &["set", "--global", "settings.notify", &format!("'{}'", sd.path("my tools/notify"))]);
    sd.ps_fixture("feat", &[]);
    sd.pass();
    assert_eq!(sd.notified(), "repo feat|ready to merge");
}

// ── stale enrollments (#51) ──────────────────────────────────────────────────

// @test "stale: a branch checked out nowhere is unenrolled after vacuumAfter, and only unenrolled"
#[test]
fn stale_a_branch_checked_out_nowhere_is_unenrolled_after_vacuumafter_and_only_unenrolled() {
    let sd = sd();
    sd.git(&["update-ref", "refs/sheepdog/feat/base", "HEAD"]);
    sd.git(&["switch", "-q", "main"]);
    sd.ps_fixture("feat", &[r#".ci.verdict = "pending""#]);
    sd.pass();
    assert!(sd.enrolled("feat"));
    sd.aged("feat", 604801);
    sd.pass();
    assert!(!sd.enrolled("feat"));
    assert_match(&sd.last_pass(), "feat.*unenrolled: checked out nowhere for 7d");
    assert_eq!(sd.notified(), "repo feat|unenrolled: checked out nowhere for 7d (sd act enroll feat to watch it again)");
    sd.git(&["rev-parse", "-q", "--verify", "refs/heads/feat"]);
    assert!(!sd.refs("refs/sheepdog/feat/").is_empty());
}

// @test "stale: re-enrolling after an unenroll starts a fresh clock"
#[test]
fn stale_re_enrolling_after_an_unenroll_starts_a_fresh_clock() {
    let sd = sd();
    sd.ps_fixture("feat", &[r#".pr.state = "MERGED""#]);
    sd.pass();
    sd.aged("feat", 604801);
    sd.pass();
    assert!(!sd.enrolled("feat"));
    sd.enroll("feat");
    sd.pass();
    assert!(sd.enrolled("feat"));
}

// @test "vacuum --dry-run: one past its threshold says the next pass takes it too"
#[test]
fn vacuum_dry_run_one_past_its_threshold_says_the_next_pass_takes_it_too() {
    let sd = sd();
    sd.git(&["switch", "-q", "main"]);
    sd.ps_fixture("feat", &[r#".ci.verdict = "pending""#]);
    sd.pass();
    sd.aged("feat", 700000);
    let r = sd.run(&["vacuum", "--dry-run"]);
    assert_has(&r.output, &["checked out nowhere for 8d: vacuum unenrolls it, as does the next pass"]);
}

// @test "stale: a worktree whose directory was deleted doesn't count"
#[test]
fn stale_a_worktree_whose_directory_was_deleted_doesnt_count() {
    let sd = sd();
    sd.git(&["switch", "-q", "main"]);
    sd.git(&["worktree", "add", "-q", &sd.path("feat-wt"), "feat"]);
    fs::remove_dir_all(sd.tmp.join("feat-wt")).unwrap();
    sd.ps_fixture("feat", &[r#".ci.verdict = "pending""#]);
    sd.pass();
    sd.aged("feat", 604801);
    sd.pass();
    assert!(!sd.enrolled("feat"));
}

// @test "stale: not before vacuumAfter"
#[test]
fn stale_not_before_vacuumafter() {
    let sd = sd();
    sd.git(&["switch", "-q", "main"]);
    sd.ps_fixture("feat", &[r#".ci.verdict = "pending""#]);
    sd.pass();
    sd.aged("feat", 604000);
    sd.pass();
    assert!(sd.enrolled("feat"));
}

// @test "stale: checking the branch out again stops the clock"
#[test]
fn stale_checking_the_branch_out_again_stops_the_clock() {
    let sd = sd();
    sd.git(&["switch", "-q", "main"]);
    sd.ps_fixture("feat", &[r#".ci.verdict = "pending""#]);
    sd.pass();
    sd.aged("feat", 604000);
    sd.git(&["switch", "-q", "feat"]);
    sd.pass();
    sd.git(&["switch", "-q", "main"]);
    sd.pass();
    sd.aged("feat", 1000);
    sd.pass();
    assert!(sd.enrolled("feat"));
}

// @test "stale: merged or closed is unenrolled after vacuumAfter, even while checked out"
#[test]
fn stale_merged_or_closed_is_unenrolled_after_vacuumafter_even_while_checked_out() {
    let sd = sd();
    for st in ["MERGED", "CLOSED"] {
        sd.enroll("feat");
        let _ = fs::remove_dir_all(sd.tmp.join("state"));
        sd.ps_fixture("feat", &[&format!(".pr.state = \"{st}\"")]);
        sd.pass();
        sd.aged("feat", 604801);
        sd.pass();
        assert!(!sd.enrolled("feat"), "for {st}");
        assert_match(&sd.last_pass(), &format!("feat.*unenrolled: {} for 7d", st.to_lowercase()));
    }
}

// @test "stale: an unsure pass neither starts, resets nor acts on the clock"
#[test]
fn stale_an_unsure_pass_neither_starts_resets_nor_acts_on_the_clock() {
    let sd = sd();
    sd.git(&["switch", "-q", "main"]);
    sd.ps_fixture("feat", &[r#".errors = ["gh: down"]"#]);
    sd.pass();
    assert_eq!(jq(&["-c", ".stale"], &fs::read_to_string(sd.state_file("feat")).unwrap()), "null");
    sd.ps_fixture("feat", &[r#".ci.verdict = "pending""#]);
    sd.pass();
    sd.aged("feat", 604801);
    sd.ps_fixture("feat", &[r#".errors = ["gh: down"]"#]);
    sd.pass();
    assert!(sd.enrolled("feat"));
    sd.ps_fixture("feat", &[r#".ci.verdict = "pending""#]);
    sd.pass();
    assert!(!sd.enrolled("feat"));
}

// @test "stale: an enrolled branch with no local branch is unenrolled at once"
#[test]
fn stale_an_enrolled_branch_with_no_local_branch_is_unenrolled_at_once() {
    let sd = sd();
    sd.git(&["switch", "-q", "main"]);
    sd.git(&["update-ref", "-d", "refs/heads/feat"]);
    assert!(sd.enrolled("feat"));
    sd.pass();
    assert!(!sd.enrolled("feat"));
    assert_match(&sd.last_pass(), "feat.*unenrolled: no local branch");
    assert!(!sd.ps("calls").exists());
}

// @test "stale: a new reason restarts the clock"
#[test]
fn stale_a_new_reason_restarts_the_clock() {
    let sd = sd();
    sd.git(&["switch", "-q", "main"]);
    sd.ps_fixture("feat", &[r#".ci.verdict = "pending""#]);
    sd.pass();
    sd.aged("feat", 604000);
    sd.ps_fixture("feat", &[r#".pr.state = "MERGED""#]);
    sd.pass();
    sd.aged("feat", 1000);
    sd.pass();
    assert!(sd.enrolled("feat"));
    assert_eq!(jq(&["-r", ".stale.reason"], &fs::read_to_string(sd.state_file("feat")).unwrap()), "merged");
}

// @test "stale: a refresh can unenroll too"
#[test]
fn stale_a_refresh_can_unenroll_too() {
    let sd = sd();
    sd.ps_fixture("feat", &[r#".pr.state = "MERGED""#]);
    sd.pass();
    sd.aged("feat", 604801);
    let r = sd.run(&["refresh"]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &["feat  -  unenrolled: merged for 7d"]);
    assert!(!sd.enrolled("feat"));
}

// @test "vacuum --dry-run: thresholds under a day read in hours or minutes, and a config error is said"
#[test]
fn vacuum_dry_run_thresholds_under_a_day_read_in_hours_or_minutes_and_a_config_error_is_said() {
    let sd = sd();
    sd.write_cfg(r#"{"settings":{"vacuumAfter":7200}}"#);
    sd.git(&["switch", "-q", "main"]);
    sd.ps_fixture("feat", &[r#".ci.verdict = "pending""#]);
    sd.pass();
    sd.aged("feat", 90);
    let r = sd.run(&["vacuum", "--dry-run"]);
    assert_has(&r.output, &["checked out nowhere for 1m: vacuum unenrolls it; a pass, after 2h"]);
    sd.write_global("{not json");
    let r = sd.run(&["vacuum", "--dry-run"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["sd config failed"]);
}

// @test "vacuum --dry-run: durations under a minute read in seconds, and a clock running behind reads 0s"
#[test]
fn vacuum_dry_run_durations_under_a_minute_read_in_seconds_and_a_clock_running_behind_reads_0s() {
    let sd = sd();
    sd.write_cfg(r#"{"settings":{"vacuumAfter":30}}"#);
    sd.git(&["switch", "-q", "main"]);
    sd.ps_fixture("feat", &[r#".ci.verdict = "pending""#]);
    sd.pass();
    sd.aged("feat", 5);
    let r = sd.run(&["vacuum", "--dry-run"]);
    assert_match(&r.output, "checked out nowhere for [5-9]s: vacuum unenrolls it; a pass, after 30s");
    sd.aged("feat", -3600);
    let r = sd.run(&["vacuum", "--dry-run"]);
    assert_has(&r.output, &["checked out nowhere for 0s: vacuum unenrolls it; a pass, after 30s"]);
}

// @test "stale: vacuumAfter 0 never unenrolls"
#[test]
fn stale_vacuumafter_0_never_unenrolls() {
    let sd = sd();
    sd.write_cfg(r#"{"settings":{"vacuumAfter":0}}"#);
    sd.git(&["switch", "-q", "main"]);
    sd.ps_fixture("feat", &[r#".ci.verdict = "pending""#]);
    sd.pass();
    sd.aged("feat", 99999999);
    sd.pass();
    assert!(sd.enrolled("feat"));
}

// @test "stale: vacuumAfter from the config file"
#[test]
fn stale_vacuumafter_from_the_config_file() {
    let sd = sd();
    sd.write_cfg(r#"{"settings":{"vacuumAfter":60}}"#);
    sd.git(&["switch", "-q", "main"]);
    sd.ps_fixture("feat", &[r#".ci.verdict = "pending""#]);
    sd.pass();
    sd.aged("feat", 61);
    sd.pass();
    assert!(!sd.enrolled("feat"));
}

// @test "vacuum --dry-run: says what vacuum would unenroll, and when a pass would, and changes nothing"
#[test]
fn vacuum_dry_run_says_what_vacuum_would_unenroll_and_when_a_pass_would_and_changes_nothing() {
    let sd = sd();
    sd.git(&["branch", "other"]);
    sd.enroll("other");
    sd.git(&["switch", "-q", "main"]);
    sd.ps_fixture("feat", &[r#".ci.verdict = "pending""#]);
    sd.ps_fixture("other", &[r#".pr.state = "MERGED""#]);
    sd.git(&["worktree", "add", "-q", &sd.path("other-wt"), "other"]);
    sd.pass();
    sd.aged("feat", 172800);
    sd.aged("other", 120);
    let before = sd.last_pass();
    let r = sd.run(&["vacuum", "--dry-run"]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &["repo  feat  checked out nowhere for 2d: vacuum unenrolls it; a pass, after 7d"]);
    assert_has(&r.output, &["repo  other  merged for 2m: vacuum unenrolls it; a pass, after 7d"]);
    assert!(sd.enrolled("feat") && sd.enrolled("other"));
    assert_eq!(sd.last_pass(), before);
}

// @test "vacuum --dry-run: a branch enrolled since the last pass is said to be unchecked, not an error"
#[test]
fn vacuum_dry_run_a_branch_enrolled_since_the_last_pass_is_said_to_be_unchecked_not_an_error() {
    let sd = sd();
    sd.ps_fixture("feat", &[]);
    sd.pass();
    sd.git(&["branch", "other"]);
    sd.enroll("other");
    let r = sd.run(&["vacuum", "--dry-run"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, "repo  other  not checked yet: vacuum checks it first");
}

// @test "vacuum --dry-run: vacuumAfter 0, an unsure last check, and a gone branch"
#[test]
fn vacuum_dry_run_vacuumafter_0_an_unsure_last_check_and_a_gone_branch() {
    let sd = sd();
    sd.write_cfg(r#"{"settings":{"vacuumAfter":0}}"#);
    sd.git(&["branch", "other"]);
    sd.enroll("other");
    sd.git(&["branch", "gone"]);
    sd.enroll("gone");
    sd.git(&["switch", "-q", "main"]);
    sd.ps_fixture("feat", &[r#".pr.state = "MERGED""#]);
    sd.ps_fixture("other", &[r#".errors = ["gh: down"]"#]);
    sd.ps_fixture("gone", &[]);
    sd.pass();
    sd.git(&["update-ref", "-d", "refs/heads/gone"]);
    sd.aged("feat", 120);
    let r = sd.run(&["vacuum", "--dry-run"]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &["repo  feat  merged for 2m: vacuum unenrolls it; a pass doesn't (vacuumAfter 0)"]);
    assert_has(&r.output, &["repo  other  unsure at the last saved check: vacuum skips it if sd state still is"]);
    assert_has(&r.output, &["repo  gone  no local branch: vacuum unenrolls it, as does the next pass"]);
    assert!(sd.enrolled("feat") && sd.enrolled("other") && sd.enrolled("gone"));
}

// @test "vacuum: -h prints the help; arguments are refused"
#[test]
fn vacuum_h_prints_the_help_arguments_are_refused() {
    let sd = sd();
    let r = sd.run(&["vacuum", "-h"]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &["Usage: sd watch"]);
    let r = sd.run(&["vacuum", "feat"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["usage: sd watch vacuum [--dry-run]"]);
    let r = sd.run(&["vacuum", "--dry-run", "feat"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["usage: sd watch vacuum [--dry-run]"]);
    sd.ps_fixture("feat", &[r#".pr.state = "MERGED""#]);
    let r = sd.run(&["vacuum", ""]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["usage: sd watch vacuum [--dry-run]"]);
    assert!(sd.enrolled("feat"));
}

// @test "stale: a worktree paused mid-rebase still has the branch checked out"
#[test]
fn stale_a_worktree_paused_mid_rebase_still_has_the_branch_checked_out() {
    let sd = sd();
    sd.paused_mid_rebase();
    sd.ps_fixture("feat", &[r#".ci.verdict = "pending""#]);
    sd.pass();
    sd.git(&["rebase", "--abort"]);
    assert_eq!(jq(&["-c", ".stale"], &fs::read_to_string(sd.state_file("feat")).unwrap()), "null");
    assert_eq!(sd.glyph("repo"), "⏳");
}

// @test "vacuum --dry-run: nothing stale says so"
#[test]
fn vacuum_dry_run_nothing_stale_says_so() {
    let sd = sd();
    sd.ps_fixture("feat", &[]);
    sd.pass();
    let r = sd.run(&["vacuum", "--dry-run"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, "nothing stale");
}

// @test "vacuum: unenrolls a merged branch inside the clock, and only unenrolls it"
#[test]
fn vacuum_unenrolls_a_merged_branch_inside_the_clock_and_only_unenrolls_it() {
    let sd = sd();
    sd.git(&["update-ref", "refs/sheepdog/feat/base", "HEAD"]);
    sd.ps_fixture("feat", &[r#".pr.state = "MERGED""#]);
    sd.pass();
    sd.aged("feat", 120);
    let r = sd.run(&["vacuum"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, "repo  feat  unenrolled: merged for 2m (sd act enroll feat to watch it again)");
    assert!(!sd.enrolled("feat"));
    sd.git(&["rev-parse", "-q", "--verify", "refs/heads/feat"]);
    assert!(!sd.refs("refs/sheepdog/feat/").is_empty());
    assert_lacks(&sd.notified(), "unenrolled");
    assert_match(&sd.last_pass(), "feat.*unenrolled: merged for 2m");
}

// @test "vacuum: observes before acting, so a branch no pass has seen is judged too"
#[test]
fn vacuum_observes_before_acting_so_a_branch_no_pass_has_seen_is_judged_too() {
    let sd = sd();
    sd.git(&["branch", "other"]);
    sd.enroll("other");
    sd.ps_fixture("feat", &[]);
    sd.ps_fixture("other", &[r#".pr.state = "CLOSED""#]);
    let r = sd.run(&["vacuum"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, "repo  other  unenrolled: closed for 0s (sd act enroll other to watch it again)");
    assert!(sd.enrolled("feat"));
    assert!(!sd.enrolled("other"));
}

// @test "vacuum: acts with vacuumAfter 0 too, and on a branch that's gone"
#[test]
fn vacuum_acts_with_vacuumafter_0_too_and_on_a_branch_thats_gone() {
    let sd = sd();
    sd.write_cfg(r#"{"settings":{"vacuumAfter":0}}"#);
    sd.git(&["branch", "other"]);
    sd.enroll("other");
    sd.git(&["update-ref", "-d", "refs/heads/other"]);
    sd.git(&["switch", "-q", "main"]);
    sd.ps_fixture("feat", &[r#".ci.verdict = "pending""#]);
    let r = sd.run(&["vacuum"]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &["repo  other  unenrolled: no local branch"]);
    assert_has(&r.output, &["repo  feat  unenrolled: checked out nowhere for 0s (sd act enroll feat to watch it again)"]);
    assert!(!sd.enrolled("feat") && !sd.enrolled("other"));
}

// @test "vacuum: VACUUM_LOG from the environment doesn't make a pass a vacuum"
#[test]
fn vacuum_vacuum_log_from_the_environment_doesnt_make_a_pass_a_vacuum() {
    let sd = sd();
    sd.git(&["switch", "-q", "main"]);
    sd.ps_fixture("feat", &[r#".ci.verdict = "pending""#]);
    fs::write(sd.tmp.join("keep"), "keep\n").unwrap();
    sd.pass_env(&[("VACUUM_LOG", &sd.path("keep"))]);
    assert!(sd.enrolled("feat"));
    assert_eq!(cat(&sd.tmp.join("keep")), "keep");
}

// @test "vacuum: an active branch is kept"
#[test]
fn vacuum_an_active_branch_is_kept() {
    let sd = sd();
    sd.ps_fixture("feat", &[]);
    let r = sd.run(&["vacuum"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, "nothing stale");
    assert!(sd.enrolled("feat"));
}

// @test "vacuum: an unsure branch is skipped, and says why"
#[test]
fn vacuum_an_unsure_branch_is_skipped_and_says_why() {
    let sd = sd();
    sd.git(&["branch", "other"]);
    sd.enroll("other");
    sd.git(&["switch", "-q", "main"]);
    sd.ps_fixture("feat", &[r#".ci.verdict = "pending""#]);
    sd.pass();
    sd.aged("feat", 120);
    sd.ps_fixture("feat", &[r#".errors = ["gh: down"]"#]);
    let r = sd.run(&["vacuum"]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &["repo  feat  skipped, sd state is unsure: gh: down"]);
    assert_has(&r.output, &["repo  other  skipped, sd state is unsure: sd state failed: sd state: no fixture for other"]);
    assert!(sd.enrolled("feat") && sd.enrolled("other"));
}

// @test "vacuum: a repo whose config can't be read is skipped"
#[test]
fn vacuum_a_repo_whose_config_cant_be_read_is_skipped() {
    let sd = sd();
    sd.ps_fixture("feat", &[r#".pr.state = "MERGED""#]);
    sd.write_global("{not json");
    let r = sd.run(&["vacuum"]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &["repo  feat  skipped: sd config failed"]);
    assert!(sd.enrolled("feat"));
}

// @test "vacuum: no next steps or unparseable sd state output is unsure too"
#[test]
fn vacuum_no_next_steps_or_unparseable_sd_state_output_is_unsure_too() {
    let sd = sd();
    sd.git(&["branch", "other"]);
    sd.enroll("other");
    sd.git(&["switch", "-q", "main"]);
    sd.ps_fixture("feat", &[".next = []"]);
    fs::write(sd.fixture("other"), "not json {\n").unwrap();
    let r = sd.run(&["vacuum"]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &["repo  feat  skipped, sd state is unsure: sd state gave no next steps"]);
    assert_has(&r.output, &["repo  other  skipped, sd state is unsure: unparseable sd state output"]);
    assert!(sd.enrolled("feat") && sd.enrolled("other"));
}

// @test "vacuum: unsure reasons that can't be read aren't called no next steps"
#[test]
fn vacuum_unsure_reasons_that_cant_be_read_arent_called_no_next_steps() {
    let sd = sd();
    sd.git(&["switch", "-q", "main"]);
    sd.ps_fixture("feat", &[r#".errors = ["gh: down"]"#]);
    sd.jq_file(&sd.fixture("feat"), &[r#".errors = [{"odd": 1}]"#]);
    let r = sd.run(&["vacuum"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, "repo  feat  skipped, sd state is unsure: its reasons couldn't be read");
    assert!(sd.enrolled("feat"));
}

// @test "vacuum: an unenroll that fails is said"
#[test]
fn vacuum_an_unenroll_that_fails_is_said() {
    use std::os::unix::fs::PermissionsExt;
    // skip: root writes through a read-only .git
    if uid() == "0" {
        return;
    }
    let sd = sd();
    sd.git(&["switch", "-q", "main"]);
    sd.ps_fixture("feat", &[r#".pr.state = "MERGED""#]);
    let git = sd.cwd.join(".git");
    let mode = fs::metadata(&git).unwrap().permissions().mode();
    fs::set_permissions(&git, fs::Permissions::from_mode(mode & !0o222)).unwrap();
    let r = sd.run(&["vacuum"]);
    fs::set_permissions(&git, fs::Permissions::from_mode(mode | 0o200)).unwrap();
    assert_eq!(r.status, 0);
    assert_has(&r.output, &["repo  feat  couldn't unenroll: merged for 0s"]);
    assert!(sd.enrolled("feat"));
}

// @test "vacuum: stale remote refs are unsure too"
#[test]
fn vacuum_stale_remote_refs_are_unsure_too() {
    let sd = sd();
    sd.git(&["switch", "-q", "main"]);
    sd.ps_fixture("feat", &[r#".fresh = false | .stale = ["origin/feat is not at GitHub's abc1234"]"#]);
    let r = sd.run(&["vacuum"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, "repo  feat  skipped, sd state is unsure: origin/feat is not at GitHub's abc1234");
    assert!(sd.enrolled("feat"));
}

// ── glyph lookup (#3) ────────────────────────────────────────────────────────

impl Sd {
    /// feat's rebase onto main stopped at a conflict, as a worktree paused mid-rebase is.
    fn paused_mid_rebase(&self) {
        self.write("f", "a\n");
        self.git(&["add", "f"]);
        self.git(&["commit", "-qm", "a"]);
        self.git(&["switch", "-q", "main"]);
        self.write("f", "b\n");
        self.git(&["add", "f"]);
        self.git(&["commit", "-qm", "b"]);
        self.git(&["switch", "-q", "feat"]);
        assert_ne!(self.git_status(&["rebase", "-q", "main"]), 0);
    }
}

// @test "glyph: prints the current worktree's glyph, from a subdirectory too"
#[test]
fn glyph_prints_the_current_worktrees_glyph_from_a_subdirectory_too() {
    let mut sd = sd();
    sd.ps_fixture("feat", &[]);
    sd.pass();
    let r = sd.run(&["glyph"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, "✅");
    fs::create_dir_all(sd.cwd.join("sub")).unwrap();
    sd.cd("repo/sub");
    assert_eq!(sd.out(&["glyph"]), "✅");
}

// @test "glyph: takes a path"
#[test]
fn glyph_takes_a_path() {
    let mut sd = sd();
    sd.git(&["switch", "-q", "main"]);
    sd.git(&["worktree", "add", "-q", &sd.path("feat-wt"), "feat"]);
    sd.ps_fixture("feat", &[r#".ci.verdict = "red" | .ci.failing = ["lint"]"#]);
    sd.pass();
    sd.cd("");
    assert_eq!(sd.out(&["glyph", &sd.path("feat-wt")]), "❌");
    sd.cd("repo");
    assert_eq!(sd.out(&["glyph"]), "");
}

// @test "glyph: nothing when the worktree has moved to another branch, or no pass saw it"
#[test]
fn glyph_nothing_when_the_worktree_has_moved_to_another_branch_or_no_pass_saw_it() {
    let sd = sd();
    sd.ps_fixture("feat", &[]);
    sd.pass();
    sd.git(&["switch", "-q", "main"]);
    let r = sd.run(&["glyph"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, "");
    let r = sd.run(&["glyph", &sd.tmp.display().to_string()]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, "");
}

// @test "glyph: a file's path means its worktree; -h prints the help"
#[test]
fn glyph_a_files_path_means_its_worktree_h_prints_the_help() {
    let mut sd = sd();
    sd.ps_fixture("feat", &[]);
    sd.pass();
    sd.write("notes.txt", "");
    sd.cd("");
    assert_eq!(sd.out(&["glyph", &sd.path("repo/notes.txt")]), "✅");
    sd.cd("repo");
    let r = sd.run(&["glyph", "-h"]);
    assert_has(&r.output, &["Usage: sd watch"]);
}

// @test "glyph: a worktree paused mid-rebase still shows its branch's glyph"
#[test]
fn glyph_a_worktree_paused_mid_rebase_still_shows_its_branchs_glyph() {
    let sd = sd();
    sd.paused_mid_rebase();
    assert!(sd.cwd.join(sd.git(&["rev-parse", "--git-path", "rebase-merge"])).is_dir());
    sd.ps_fixture("feat", &[".sync.up_to_date = false | .sync.conflict = true"]);
    sd.pass();
    assert_eq!(sd.out(&["glyph"]), "❌");
    sd.git(&["rebase", "--abort"]);
}

// @test "glyph: a tag with the branch's name doesn't hide the glyph"
#[test]
fn glyph_a_tag_with_the_branchs_name_doesnt_hide_the_glyph() {
    let sd = sd();
    sd.ps_fixture("feat", &[]);
    sd.pass();
    sd.git(&["tag", "feat"]);
    assert_eq!(sd.out(&["glyph"]), "✅");
}

// @test "glyph: an unknown option or a second path is refused"
#[test]
fn glyph_an_unknown_option_or_a_second_path_is_refused() {
    let sd = sd();
    let r = sd.run(&["glyph", "--bogus"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["unknown option: --bogus"]);
    let r = sd.run(&["glyph", "a", "b"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["usage: sd watch glyph [<path>]"]);
}

// @test "glyph: before any pass, nothing"
#[test]
fn glyph_before_any_pass_nothing() {
    let sd = sd();
    let r = sd.run(&["glyph"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, "");
}

// @test "glyph: a refresh updates it, keeping other worktrees'"
#[test]
fn glyph_a_refresh_updates_it_keeping_other_worktrees() {
    let sd = sd();
    sd.git(&["branch", "other"]);
    sd.enroll("other");
    sd.git(&["worktree", "add", "-q", &sd.path("other-wt"), "other"]);
    sd.ps_fixture("feat", &[]);
    sd.ps_fixture("other", &[]);
    sd.pass();
    sd.ps_fixture("feat", &[r#".ci.verdict = "pending""#]);
    sd.ok(&["refresh"]);
    assert_eq!(sd.out(&["glyph"]), "⏳");
    assert_eq!(sd.out(&["glyph", &sd.path("other-wt")]), "✅");
}

// @test "glyph: gone after a pass once the branch isn't enrolled"
#[test]
fn glyph_gone_after_a_pass_once_the_branch_isnt_enrolled() {
    let sd = sd();
    sd.ps_fixture("feat", &[]);
    sd.pass();
    sd.git(&["config", "--unset", "branch.feat.sheepdog"]);
    sd.pass();
    assert_eq!(sd.out(&["glyph"]), "");
}

// ── same-named worktrees (#4) ───────────────────────────────────────────────

impl Sd {
    /// twin <branch-in-repo2>: a second registered repo whose <branch> is checked
    /// out in $TMP/y/wt, while this repo's feat is in $TMP/x/wt. sd state for
    /// repo2 serves <branch>'s fixture made CI red.
    fn twin(&self, b: &str) {
        self.git(&["clone", "-q", &self.path("origin.git"), &self.path("repo2")]);
        self.git_in("repo2", &["branch", "-q", b]);
        self.git_in("repo2", &["config", &format!("branch.{b}.sheepdog"), "true"]);
        self.register(&self.path("repo2"));
        self.git(&["switch", "-q", "main"]);
        fs::create_dir_all(self.tmp.join("x")).unwrap();
        fs::create_dir_all(self.tmp.join("y")).unwrap();
        self.git(&["worktree", "add", "-q", &self.path("x/wt"), "feat"]);
        self.git_in("repo2", &["worktree", "add", "-q", &self.path("y/wt"), b]);
        self.ps_fixture("feat", &[]);
        self.ps_fixture(b, &[]);
        let red = jq(
            &[r#".ci.verdict = "red" | .ci.failing = ["lint"] | .next = [{key: "ci-red", who: "agent", say: "fix CI: lint", cmd: null}]"#],
            &fs::read_to_string(self.ps(&format!("{b}.json"))).unwrap(),
        );
        fs::write(self.ps(&format!("repo2-{b}.json")), red + "\n").unwrap();
        self.fake(
            "sd-state",
            r#"#!/usr/bin/env bash
b=${@: -1}
echo "$PWD $b" >> "$PS_FIXTURES/calls"
f="$PS_FIXTURES/$(basename "$PWD")-${b//\//__}.json"
[[ -f $f ]] || f="$PS_FIXTURES/${b//\//__}.json"
cat "$f"
"#,
        );
    }
}

// @test "same name: two worktrees on the same branch get no shared .pr file, and glyph tells them apart"
#[test]
fn same_name_two_worktrees_on_the_same_branch_get_no_shared_pr_file_and_glyph_tells_them_apart() {
    let sd = sd();
    sd.twin("feat");
    sd.pass();
    assert!(!sd.pr_file("wt").exists());
    assert_eq!(sd.out(&["glyph", &sd.path("x/wt")]), "✅");
    assert_eq!(sd.out(&["glyph", &sd.path("y/wt")]), "❌");
}

// @test "same name: on different branches the .pr file stays, its branch telling whose it is"
#[test]
fn same_name_on_different_branches_the_pr_file_stays_its_branch_telling_whose_it_is() {
    let sd = sd();
    sd.twin("other");
    sd.pass();
    let pr = cat(&sd.pr_file("wt"));
    assert!(pr == "feat\t✅" || pr == "other\t❌", "{pr:?}");
    assert_eq!(sd.out(&["glyph", &sd.path("x/wt")]), "✅");
    assert_eq!(sd.out(&["glyph", &sd.path("y/wt")]), "❌");
}

// @test "same name: a refresh doesn't put the shared .pr file back"
#[test]
fn same_name_a_refresh_doesnt_put_the_shared_pr_file_back() {
    let mut sd = sd();
    sd.twin("feat");
    sd.pass();
    sd.cd("x/wt");
    sd.ok(&["refresh"]);
    sd.cd("repo");
    assert!(!sd.pr_file("wt").exists());
    assert_eq!(sd.out(&["glyph", &sd.path("x/wt")]), "✅");
    assert_eq!(sd.out(&["glyph", &sd.path("y/wt")]), "❌");
}

// @test "same name: a third one on another branch keeps the .pr file it wrote last"
#[test]
fn same_name_a_third_one_on_another_branch_keeps_the_pr_file_it_wrote_last() {
    let sd = sd();
    sd.twin("feat");
    sd.git(&["clone", "-q", &sd.path("origin.git"), &sd.path("repo3")]);
    sd.git_in("repo3", &["branch", "-q", "other"]);
    sd.git_in("repo3", &["config", "branch.other.sheepdog", "true"]);
    sd.register(&sd.path("repo3"));
    fs::create_dir_all(sd.tmp.join("z")).unwrap();
    sd.git_in("repo3", &["worktree", "add", "-q", &sd.path("z/wt"), "other"]);
    sd.ps_fixture("other", &[]);
    sd.pass();
    assert_eq!(cat(&sd.pr_file("wt")), "other\t✅");
}

// @test "same name: a repo registered twice isn't its own twin"
#[test]
fn same_name_a_repo_registered_twice_isnt_its_own_twin() {
    let sd = sd();
    sd.ps_fixture("feat", &[]);
    sd.git(&["config", "--global", "--add", "sheepdog.repo", &sd.path("repo")]);
    sd.pass();
    assert_eq!(sd.glyph("repo"), "✅");
}

// ── GitHub's rate limit (#122) ───────────────────────────────────────────────

impl Sd {
    /// low <branch> [remaining]: sd state for <branch> leaves that much of the quota.
    fn low(&self, b: &str, remaining: u32) {
        self.ps_fixture(
            b,
            &[&format!(r#".rate_limit = {{cost: 2, remaining: {remaining}, reset_at: "2026-10-16T13:00:00Z", limited: false}}"#)],
        );
    }

    /// rate_gh: a fake gh whose REST rate_limit says GraphQL resets at 14:00.
    fn rate_gh(&self) {
        self.fake(
            "gh",
            r#"#!/usr/bin/env bash
echo "$*" >> "$PS_FIXTURES/gh-calls"
[[ $1 == api && $2 == rate_limit ]] || exit 1
echo 2026-10-16T14:00:00Z
"#,
        );
    }

    fn repo2_z(&self) {
        self.git(&["clone", "-q", &self.path("origin.git"), &self.path("repo2")]);
        self.git_in("repo2", &["branch", "-q", "z"]);
        self.git_in("repo2", &["config", "branch.z.sheepdog", "true"]);
    }
}

// @test "rate limit: remaining under settings.rateLimitFloor stops the pass after that branch, and status says until when"
#[test]
fn rate_limit_remaining_under_settings_ratelimitfloor_stops_the_pass_after_that_branch_and_status_says_until_when() {
    let sd = sd();
    sd.git(&["branch", "other"]);
    sd.enroll("other");
    sd.low("feat", 100);
    sd.ps_fixture("other", &[]);
    sd.pass();
    assert_eq!(sd.calls(), format!("{} feat", sd.path("repo")));
    let r = sd.run(&["status"]);
    assert_has(&r.output, &["repo  feat  ✅"]);
    assert_has(
        &r.output,
        &["GitHub rate limit: 100 points left, under settings.rateLimitFloor (250): the branches below weren't checked (the quota resets at 2026-10-16T13:00:00Z)"],
    );
    assert_has(&r.output, &["repo  other  -  not checked: rate limit"]);
}

// @test "rate limit: low after the pass's last branch stops nothing"
#[test]
fn rate_limit_low_after_the_passs_last_branch_stops_nothing() {
    let sd = sd();
    sd.git(&["branch", "other"]);
    sd.enroll("other");
    sd.ps_fixture("feat", &[]);
    sd.low("other", 100);
    sd.pass();
    assert_eq!(sd.calls().lines().count(), 2);
    assert_lacks(&sd.last_pass(), "rate limit");
}

// @test "rate limit: a stopped pass still does the later repos' local housekeeping"
#[test]
fn rate_limit_a_stopped_pass_still_does_the_later_repos_local_housekeeping() {
    let sd = sd();
    sd.low("feat", 100);
    sd.git(&["clone", "-q", &sd.path("origin.git"), &sd.path("repo2")]);
    sd.git_in("repo2", &["config", "branch.gone.sheepdog", "true"]);
    sd.git_in("repo2", &["branch", "-q", "z"]);
    sd.git_in("repo2", &["config", "branch.z.sheepdog", "true"]);
    sd.git_in("repo2", &["update-ref", "refs/sheepdog/old/base", "HEAD"]);
    sd.register(&sd.path("repo2"));
    sd.ps_fixture("z", &[]);
    sd.pass();
    assert_eq!(sd.calls(), format!("{} feat", sd.path("repo")));
    assert_eq!(sd.git_try(&["-C", &sd.path("repo2"), "config", "branch.gone.sheepdog"]).unwrap_or_default(), "");
    assert_eq!(sd.git_in("repo2", &["for-each-ref", "refs/sheepdog/"]), "");
    assert_has(&sd.last_pass(), &["repo2\tz\t-\tnot checked: rate limit"]);
}

// @test "rate limit: plenty left, or none reported, checks every branch"
#[test]
fn rate_limit_plenty_left_or_none_reported_checks_every_branch() {
    let sd = sd();
    sd.git(&["branch", "other"]);
    sd.enroll("other");
    sd.git(&["branch", "third"]);
    sd.enroll("third");
    sd.ps_fixture("feat", &[]);
    sd.low("other", 4000);
    sd.ps_fixture("third", &[]);
    sd.pass();
    let repo = sd.path("repo");
    assert_eq!(sd.calls(), format!("{repo} feat\n{repo} other\n{repo} third"));
    assert_lacks(&sd.last_pass(), "rate limit");
}

// @test "rate limit: settings.rateLimitFloor sets the floor"
#[test]
fn rate_limit_settings_ratelimitfloor_sets_the_floor() {
    let sd = sd();
    sd.git(&["branch", "other"]);
    sd.enroll("other");
    sd.verb_ok("config", &["set", "--global", "settings.rateLimitFloor", "50"]);
    sd.low("feat", 100);
    sd.ps_fixture("other", &[]);
    sd.pass();
    assert_eq!(sd.calls().lines().count(), 2);
}

// @test "rate limit: a call refused for the rate limit stops the pass, other repos too, with GitHub's reset time"
#[test]
fn rate_limit_a_call_refused_for_the_rate_limit_stops_the_pass_other_repos_too_with_githubs_reset_time() {
    let sd = sd();
    sd.rate_gh();
    sd.ps_fixture(
        "feat",
        &[r#".rate_limit = {cost: null, remaining: null, reset_at: null, limited: true}
                   | .errors = ["gh (feat): API rate limit exceeded for user ID 1."] | .fresh = null"#],
    );
    sd.repo2_z();
    sd.register(&sd.path("repo2"));
    sd.ps_fixture("z", &[]);
    sd.pass();
    assert_eq!(sd.calls(), format!("{} feat", sd.path("repo")));
    assert_has(
        &sd.last_pass(),
        &["GitHub refused a call for the rate limit: the branches below weren't checked (the quota resets at 2026-10-16T14:00:00Z)"],
    );
    assert_has(&sd.last_pass(), &["repo2\tz\t-\tnot checked: rate limit"]);
}

// @test "rate limit: a stopped pass leaves the unchecked branches' glyphs as they were"
#[test]
fn rate_limit_a_stopped_pass_leaves_the_unchecked_branches_glyphs_as_they_were() {
    let sd = sd();
    sd.git(&["branch", "other"]);
    sd.enroll("other");
    sd.git(&["worktree", "add", "-q", &sd.path("other-wt"), "other"]);
    sd.ps_fixture("feat", &[]);
    sd.ps_fixture("other", &[]);
    sd.pass();
    assert_eq!(sd.glyph("other-wt"), "✅");
    sd.low("feat", 100);
    sd.ps_fixture("other", &[r#".ci.verdict = "red""#]);
    sd.pass();
    assert_eq!(sd.glyph("other-wt"), "✅");
    assert_eq!(sd.out(&["glyph", &sd.path("other-wt")]), "✅");
    assert_eq!(sd.glyph("repo"), "✅");
}

// @test "rate limit: refresh checks its branch and notes nothing to stop"
#[test]
fn rate_limit_refresh_checks_its_branch_and_notes_nothing_to_stop() {
    let sd = sd();
    sd.low("feat", 100);
    let r = sd.run(&["refresh", "feat"]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &["feat  ✅"]);
    assert_lacks(&r.output, "rate limit");
}

// @test "rate limit: a stopped pass keeps only the skipped branches' glyphs"
#[test]
fn rate_limit_a_stopped_pass_keeps_only_the_skipped_branches_glyphs() {
    let sd = sd();
    sd.git(&["branch", "other"]);
    sd.enroll("other");
    sd.git(&["worktree", "add", "-q", &sd.path("other-wt"), "other"]);
    sd.repo2_z();
    sd.git_in("repo2", &["worktree", "add", "-q", &sd.path("z-wt"), "z"]);
    sd.register(&sd.path("repo2"));
    sd.ps_fixture("feat", &[]);
    sd.ps_fixture("other", &[]);
    sd.ps_fixture("z", &[]);
    sd.pass();
    assert_eq!(sd.glyph("z-wt"), "✅");
    sd.git(&["config", "--global", "--unset", "sheepdog.repo", &sd.path("repo2")]);
    sd.low("feat", 100);
    sd.pass();
    assert_eq!(sd.glyph("other-wt"), "✅");
    assert!(!sd.pr_file("z-wt").exists());
    assert_eq!(sd.out(&["glyph", &sd.path("z-wt")]), "");
}

// ── status ───────────────────────────────────────────────────────────────────

// @test "status: prints the last pass"
#[test]
fn status_prints_the_last_pass() {
    let sd = sd();
    sd.ps_fixture("feat", &[r#".ci.verdict = "red" | .ci.failing = ["lint"]"#]);
    sd.pass();
    let r = sd.run(&["status"]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &["repo", "feat", "❌", "fix CI: lint"]);
    assert_lacks(&r.output, "\t");
}

// @test "status: before any pass"
#[test]
fn status_before_any_pass() {
    let sd = sd();
    let r = sd.run(&["status"]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &["no pass yet"]);
}

// ── refresh ──────────────────────────────────────────────────────────────────

// @test "refresh: checks only the current branch, from the main worktree"
#[test]
fn refresh_checks_only_the_current_branch_from_the_main_worktree() {
    let sd = sd();
    sd.git(&["branch", "other"]);
    sd.enroll("other");
    sd.ps_fixture("feat", &[]);
    sd.ps_fixture("other", &[]);
    let r = sd.run(&["refresh"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.calls(), format!("{} feat", sd.path("repo")));
    assert_has(&r.output, &["repo  feat  ✅  ready to merge"]);
}

// @test "refresh <branch>: checks that branch"
#[test]
fn refresh_branch_checks_that_branch() {
    let sd = sd();
    sd.git(&["branch", "other"]);
    sd.enroll("other");
    sd.ps_fixture("other", &[r#".ci.verdict = "red" | .ci.failing = ["lint"]"#]);
    sd.ok(&["refresh", "other"]);
    assert_eq!(sd.calls(), format!("{} other", sd.path("repo")));
    assert_eq!(sd.notified(), "repo other|fix CI: lint");
}

// @test "refresh: writes the glyph and notifies, and a later pass doesn't notify again"
#[test]
fn refresh_writes_the_glyph_and_notifies_and_a_later_pass_doesnt_notify_again() {
    let sd = sd();
    sd.ps_fixture("feat", &[r#".ci.verdict = "red" | .ci.failing = ["lint"]"#]);
    sd.ok(&["refresh"]);
    assert_eq!(sd.glyph("repo"), "❌");
    sd.pass();
    sd.ok(&["refresh"]);
    assert_eq!(sd.notified(), "repo feat|fix CI: lint");
}

// @test "refresh: after a pass, doesn't notify what the pass did"
#[test]
fn refresh_after_a_pass_doesnt_notify_what_the_pass_did() {
    let sd = sd();
    sd.ps_fixture("feat", &[]);
    sd.pass();
    sd.ok(&["refresh"]);
    assert_eq!(sd.notified(), "repo feat|ready to merge");
}

// @test "refresh: from a linked worktree, its branch and the pass's state"
#[test]
fn refresh_from_a_linked_worktree_its_branch_and_the_passs_state() {
    let mut sd = sd();
    sd.git(&["switch", "-q", "main"]);
    sd.git(&["worktree", "add", "-q", &sd.path("feat-wt"), "feat"]);
    sd.ps_fixture("feat", &[]);
    sd.cd("feat-wt");
    sd.ok(&["refresh"]);
    sd.cd("repo");
    assert_eq!(sd.glyph("feat-wt"), "✅");
    assert_eq!(sd.calls(), format!("{} feat", sd.path("repo")));
    sd.pass();
    assert_eq!(sd.notified(), "repo feat|ready to merge");
}

// @test "refresh: fetches the configured remote"
#[test]
fn refresh_fetches_the_configured_remote() {
    let sd = sd();
    sd.git(&["clone", "-q", &sd.path("origin.git"), &sd.path("other")]);
    sd.git_in("other", &["commit", "-q", "--allow-empty", "-m", "moved"]);
    sd.git_in("other", &["push", "-q", "origin", "main"]);
    sd.git(&["remote", "add", "up", &sd.path("origin.git")]);
    sd.write_cfg(r#"{"settings":{"remote":"up"}}"#);
    sd.ps_fixture("feat", &[]);
    sd.ok(&["refresh"]);
    assert_eq!(sd.git(&["rev-parse", "up/main"]), sd.git_in("other", &["rev-parse", "main"]));
}

// @test "refresh: a failed fetch is reported and the branch still checked"
#[test]
fn refresh_a_failed_fetch_is_reported_and_the_branch_still_checked() {
    let sd = sd();
    sd.git(&["remote", "set-url", "origin", &sd.path("nowhere.git")]);
    sd.ps_fixture("feat", &[]);
    let r = sd.run(&["refresh"]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &["fetch from origin failed: ", "nowhere"]);
    assert_eq!(sd.glyph("repo"), "✅");
}

// @test "refresh: isn't recorded as a pass, and leaves other glyphs alone"
#[test]
fn refresh_isnt_recorded_as_a_pass_and_leaves_other_glyphs_alone() {
    let sd = sd();
    sd.git(&["branch", "other"]);
    sd.enroll("other");
    sd.git(&["worktree", "add", "-q", &sd.path("other-wt"), "other"]);
    sd.ps_fixture("feat", &[r#".ci.verdict = "red" | .ci.failing = ["lint"]"#]);
    sd.ps_fixture("other", &[]);
    sd.pass();
    let before = sd.last_pass();
    sd.ps_fixture("feat", &[]);
    sd.ok(&["refresh"]);
    assert_eq!(sd.last_pass(), before);
    assert_eq!(sd.glyph("other-wt"), "✅");
    assert_eq!(sd.glyph("repo"), "✅");
}

// @test "refresh: before any pass, status still says so"
#[test]
fn refresh_before_any_pass_status_still_says_so() {
    let sd = sd();
    sd.ps_fixture("feat", &[]);
    sd.ok(&["refresh"]);
    let r = sd.run(&["status"]);
    assert_has(&r.output, &["no pass yet"]);
}

// @test "refresh: sd state failing is ⚠️ and exits non-zero"
#[test]
fn refresh_sd_state_failing_is_and_exits_non_zero() {
    let sd = sd();
    let r = sd.run(&["refresh"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["feat  ⚠️  sd state failed"]);
    assert_eq!(sd.glyph("repo"), "⚠️");
}

// @test "refresh: unparseable sd state output is ⚠️ and exits non-zero"
#[test]
fn refresh_unparseable_sd_state_output_is_and_exits_non_zero() {
    let sd = sd();
    fs::write(sd.fixture("feat"), "warning: junk\n{\"branch\":").unwrap();
    let r = sd.run(&["refresh"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["feat  ⚠️  unparseable"]);
}

// @test "refresh: an unreadable config file exits non-zero without checking"
#[test]
fn refresh_an_unreadable_config_file_exits_non_zero_without_checking() {
    let sd = sd();
    sd.ps_fixture("feat", &[]);
    sd.write_global("{not json");
    let r = sd.run(&["refresh"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["sd config failed"]);
    assert!(!sd.ps("calls").exists());
}

// @test "refresh: refuses a branch that isn't enrolled, or doesn't exist"
#[test]
fn refresh_refuses_a_branch_that_isnt_enrolled_or_doesnt_exist() {
    let sd = sd();
    sd.git(&["branch", "other"]);
    let r = sd.run(&["refresh", "other"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["other isn't enrolled", "sd act enroll"]);
    let r = sd.run(&["refresh", "nope"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["no such branch: nope"]);
    assert!(!sd.ps("calls").exists());
}

// @test "refresh: -h prints the help; another option or a second branch is refused"
#[test]
fn refresh_h_prints_the_help_another_option_or_a_second_branch_is_refused() {
    let sd = sd();
    let r = sd.run(&["refresh", "-h"]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &["Usage: sd watch"]);
    let r = sd.run(&["refresh", "--nope"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["unknown option: --nope"]);
    sd.git(&["branch", "other"]);
    sd.enroll("other");
    let r = sd.run(&["refresh", "feat", "other"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["usage: sd watch refresh [<branch>]"]);
    assert!(!sd.ps("calls").exists());
}

// @test "refresh: refuses a repo the pass doesn't watch"
#[test]
fn refresh_refuses_a_repo_the_pass_doesnt_watch() {
    let sd = sd();
    sd.git(&["config", "--global", "--unset-all", "sheepdog.repo"]);
    sd.ps_fixture("feat", &[]);
    let r = sd.run(&["refresh"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &[&format!("{} isn't registered", sd.path("repo")), "sd act enroll"]);
    assert!(!sd.ps("calls").exists());
}

// @test "refresh: refuses a detached HEAD with no branch named"
#[test]
fn refresh_refuses_a_detached_head_with_no_branch_named() {
    let sd = sd();
    sd.git(&["switch", "-q", "--detach"]);
    let r = sd.run(&["refresh"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["not on a branch"]);
}

// @test "refresh: outside a git repo, refuses"
#[test]
fn refresh_outside_a_git_repo_refuses() {
    let mut sd = sd();
    sd.cd("");
    let r = sd.run(&["refresh"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["not in a git repository"]);
}

// ── launchd ──────────────────────────────────────────────────────────────────

// needs_plutil: the tests that validate the plist need plutil, so off macOS
// they return early, where bats skipped them: cargo test has no skip. Not in
// setup_launchd: tests refused before plutil runs should still run on Linux.
fn no_plutil() -> bool {
    !cfg!(target_os = "macos")
}

const OLD_LABELS: [&str; 2] = ["com.agent-rig.pr-watch", "com.pr-tend.pr-watch"];

impl Sd {
    /// setup_launchd: the plist's directory in the temp dir, a fake launchctl
    /// logging to $PS_FIXTURES/launchctl, and no LOCAL_BIN. Nor CARGO, which
    /// cargo test sets and bats never had: sd watch reads both empty as unset.
    fn setup_launchd(&mut self) -> PathBuf {
        let agents = self.path("LaunchAgents");
        self.env.extend([
            ("LOCAL_BIN".into(), String::new()),
            ("CARGO".into(), String::new()),
            ("SHEEPDOG_WATCH_LAUNCH_AGENTS".into(), agents.clone()),
        ]);
        self.fake("launchctl", "#!/usr/bin/env bash\necho \"$*\" >> \"$PS_FIXTURES/launchctl\"\n");
        PathBuf::from(agents).join("com.sheepdog.watch.plist")
    }

    fn launchctl(&self) -> String {
        cat(&self.ps("launchctl"))
    }

    fn old_plists(&self) {
        fs::create_dir_all(self.tmp.join("LaunchAgents")).unwrap();
        for l in OLD_LABELS {
            fs::write(self.tmp.join(format!("LaunchAgents/{l}.plist")), "old\n").unwrap();
        }
    }

    #[track_caller]
    fn old_labels_retired(&self) {
        for l in OLD_LABELS {
            assert_has(&self.launchctl(), &[&format!("bootout gui/{}/{l}", uid())]);
            assert!(!self.tmp.join(format!("LaunchAgents/{l}.plist")).exists());
        }
    }

    /// A dir in the temp dir holding a link to the repo's bin/sd, as LOCAL_BIN.
    fn local_bin(&self) -> String {
        fs::create_dir_all(self.tmp.join("local-bin")).unwrap();
        std::os::unix::fs::symlink(concat!(env!("CARGO_MANIFEST_DIR"), "/bin/sd"), self.tmp.join("local-bin/sd")).unwrap();
        self.path("local-bin")
    }
}

#[track_caller]
fn lint(plist: &Path) {
    let out = output(std::process::Command::new("plutil").arg("-lint").arg(plist), None);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
}

fn extract(plist: &Path, key: &str) -> String {
    let out = output(std::process::Command::new("plutil").args(["-extract", key, "raw"]).arg(plist), None);
    chomp(&String::from_utf8_lossy(&out.stdout))
}

// @test "install: writes a valid plist and bootstraps it"
#[test]
fn install_writes_a_valid_plist_and_bootstraps_it() {
    if no_plutil() {
        return;
    }
    let mut sd = sd();
    let plist = sd.setup_launchd();
    let r = sd.run(&["install"]);
    assert_eq!(r.status, 0, "{}", r.output);
    lint(&plist);
    assert_eq!(extract(&plist, "StartInterval"), "300");
    assert!(extract(&plist, "EnvironmentVariables.PATH").starts_with(&format!("{}/.local/bin:/", sd.path("home"))));
    assert_has(&sd.launchctl(), &[&format!("bootstrap gui/{} {}", uid(), plist.display())]);
}

// @test "install: the plist is readable by its owner only, whatever the umask"
#[test]
fn install_the_plist_is_readable_by_its_owner_only_whatever_the_umask() {
    use std::os::unix::fs::PermissionsExt;
    if no_plutil() {
        return;
    }
    let mut sd = sd();
    let plist = sd.setup_launchd();
    let bin = sd.bin.display().to_string();
    let r = sd.exec(Path::new("sh"), &[], &["-c", r#"umask 022 && exec "$0" install"#, &bin]);
    assert_eq!(r.status, 0, "{}", r.output);
    // ls -l: -rw-------
    let m = fs::symlink_metadata(&plist).unwrap();
    assert!(m.is_file() && m.permissions().mode() & 0o7777 == 0o600, "{:o}", m.permissions().mode());
}

// @test "install: the job abandons its process group, so launchd spares a step that hasn't reached setsid (#12)"
#[test]
fn install_the_job_abandons_its_process_group_so_launchd_spares_a_step_that_hasnt_reached_setsid_12() {
    if no_plutil() {
        return;
    }
    let mut sd = sd();
    let plist = sd.setup_launchd();
    let r = sd.run(&["install"]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_eq!(extract(&plist, "AbandonProcessGroup"), "true");
}

// @test "install: the job runs sd watch"
#[test]
fn install_the_job_runs_sd_watch() {
    if no_plutil() {
        return;
    }
    let mut sd = sd();
    let plist = sd.setup_launchd();
    let r = sd.run(&["install"]);
    assert_eq!(r.status, 0, "{}", r.output);
    let prog = extract(&plist, "ProgramArguments.0");
    assert!(prog.ends_with("/sd"), "{prog}");
    assert!(executable(&prog), "{prog}");
    assert_eq!(extract(&plist, "ProgramArguments.1"), "watch");
}

// @test "install: the job runs the sd in LOCAL_BIN, which leads its PATH ahead of ~/.local/bin"
#[test]
fn install_the_job_runs_the_sd_in_local_bin_which_leads_its_path_ahead_of_local_bin() {
    if no_plutil() {
        return;
    }
    let mut sd = sd();
    let plist = sd.setup_launchd();
    let lb = sd.local_bin();
    let r = sd.run_env(&[("LOCAL_BIN", &lb)], &["install"]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_eq!(extract(&plist, "ProgramArguments.0"), format!("{lb}/sd"));
    assert!(extract(&plist, "EnvironmentVariables.PATH").starts_with(&format!("{lb}:{}/.local/bin:/", sd.path("home"))));
}

// @test "install: cargo's directory ends the job's PATH, so sd can rebuild itself after a pull (#193)"
#[test]
fn install_cargos_directory_ends_the_jobs_path_so_sd_can_rebuild_itself_after_a_pull_193() {
    if no_plutil() {
        return;
    }
    let mut sd = sd();
    let plist = sd.setup_launchd();
    fs::create_dir_all(sd.tmp.join("cargo-bin")).unwrap();
    sd.fake("../cargo-bin/cargo", "#!/bin/sh\n");
    let path = format!("{}:{}:{}", sd.path("cargo-bin"), sd.path("fakebin"), std::env::var("PATH").unwrap_or_default());
    let r = sd.run_env(&[("PATH", &path)], &["install"]);
    assert_eq!(r.status, 0, "{}", r.output);
    let job = extract(&plist, "EnvironmentVariables.PATH");
    assert!(job.starts_with(&format!("{}/.local/bin:", sd.path("home"))), "{job}");
    assert!(job.ends_with(&format!(":{}", sd.path("cargo-bin"))), "{job}");
}

// @test "install: cargo in a directory already on the job's PATH isn't added again"
#[test]
fn install_cargo_in_a_directory_already_on_the_jobs_path_isnt_added_again() {
    if no_plutil() {
        return;
    }
    let mut sd = sd();
    let plist = sd.setup_launchd();
    let lb = sd.local_bin();
    sd.fake("../local-bin/cargo", "#!/bin/sh\n");
    let cargo = format!("{lb}/cargo");
    let r = sd.run_env(&[("CARGO", &cargo), ("LOCAL_BIN", &lb)], &["install"]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_eq!(extract(&plist, "EnvironmentVariables.PATH").split(':').filter(|d| *d == lb).count(), 1);
}

// @test "install: no cargo warns that the watcher can't rebuild sd, and still installs"
#[test]
fn install_no_cargo_warns_that_the_watcher_cant_rebuild_sd_and_still_installs() {
    if no_plutil() {
        return;
    }
    let mut sd = sd();
    let plist = sd.setup_launchd();
    let r = sd.run_env(&[("CARGO", &sd.path("no-such-cargo"))], &["install"]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_has(&r.output, &["no cargo", "can't rebuild"]);
    lint(&plist);
}

// @test "install: a relative LOCAL_BIN is made absolute, since launchd runs jobs from /"
#[test]
fn install_a_relative_local_bin_is_made_absolute_since_launchd_runs_jobs_from() {
    if no_plutil() {
        return;
    }
    let mut sd = sd();
    let plist = sd.setup_launchd();
    let lb = sd.local_bin();
    sd.cd("");
    let r = sd.run_env(&[("LOCAL_BIN", "local-bin")], &["install"]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_eq!(extract(&plist, "ProgramArguments.0"), format!("{lb}/sd"));
    assert!(extract(&plist, "EnvironmentVariables.PATH").starts_with(&format!("{lb}:")));
}

// @test "uninstall: boots out and removes the plist"
#[test]
fn uninstall_boots_out_and_removes_the_plist() {
    if no_plutil() {
        return;
    }
    let mut sd = sd();
    let plist = sd.setup_launchd();
    sd.ok(&["install"]);
    let r = sd.run(&["uninstall"]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert!(!plist.exists());
    assert_has(&sd.launchctl(), &[&format!("bootout gui/{}/com.sheepdog.watch", uid())]);
}

// @test "install: boots out the jobs under the old labels and removes their plists"
#[test]
fn install_boots_out_the_jobs_under_the_old_labels_and_removes_their_plists() {
    if no_plutil() {
        return;
    }
    let mut sd = sd();
    let plist = sd.setup_launchd();
    sd.old_plists();
    let r = sd.run(&["install"]);
    assert_eq!(r.status, 0, "{}", r.output);
    sd.old_labels_retired();
    assert!(plist.exists());
}

// @test "uninstall: also boots out the old labels"
#[test]
fn uninstall_also_boots_out_the_old_labels() {
    if no_plutil() {
        return;
    }
    let mut sd = sd();
    sd.setup_launchd();
    sd.old_plists();
    let r = sd.run(&["uninstall"]);
    assert_eq!(r.status, 0, "{}", r.output);
    sd.old_labels_retired();
}

// @test "install: a non-numeric interval is refused and the old job is kept"
#[test]
fn install_a_non_numeric_interval_is_refused_and_the_old_job_is_kept() {
    if no_plutil() {
        return;
    }
    let mut sd = sd();
    let plist = sd.setup_launchd();
    sd.ok(&["install"]);
    fs::write(sd.ps("launchctl"), "").unwrap();
    sd.write_cfg(r#"{"settings":{"interval":"5m"}}"#);
    let r = sd.run(&["install"]);
    assert_ne!(r.status, 0);
    assert_eq!(extract(&plist, "StartInterval"), "300");
    assert_eq!(fs::metadata(sd.ps("launchctl")).unwrap().len(), 0);
}

// @test "install: paths are XML-escaped"
#[test]
fn install_paths_are_xml_escaped() {
    if no_plutil() {
        return;
    }
    let mut sd = sd();
    let plist = sd.setup_launchd();
    let state = sd.path("a&b");
    sd.env.iter_mut().find(|(k, _)| k == "SHEEPDOG_STATE_DIR").unwrap().1 = state.clone();
    sd.ok(&["install"]);
    lint(&plist);
    assert_eq!(extract(&plist, "StandardOutPath"), format!("{state}/launchd.log"));
}

// ── settings via sd config: the global file over the defaults ───────────────

// @test "config: git config sheepdog.notify is not read"
#[test]
fn config_git_config_sheepdog_notify_is_not_read() {
    let sd = sd();
    sd.git(&["config", "--global", "sheepdog.notify", &sd.path("fakebin/missing")]);
    sd.ps_fixture("feat", &[]);
    sd.pass();
    assert_eq!(sd.notified(), "repo feat|ready to merge");
}

// @test "config: a branch file in the working directory does not steer the watcher"
#[test]
fn config_a_branch_file_in_the_working_directory_does_not_steer_the_watcher() {
    let sd = sd();
    sd.write_cfg(&format!(r#"{{"settings":{{"notify":"{}"}}}}"#, sd.path("fakebin/notify")));
    fs::create_dir_all(sd.tmp.join("repo/.git/sheepdog")).unwrap();
    fs::write(
        sd.tmp.join("repo/.git/sheepdog/feat.json"),
        format!("{{\"settings\":{{\"notify\":\"{}\"}}}}\n", sd.path("fakebin/missing")),
    )
    .unwrap();
    sd.ps_fixture("feat", &[]);
    sd.ok(&[]);
    assert_eq!(sd.notified(), "repo feat|ready to merge");
}

// @test "config: remote from the file"
#[test]
fn config_remote_from_the_file() {
    let sd = sd();
    sd.write_cfg(r#"{"settings":{"remote":"nowhere"}}"#);
    sd.ps_fixture("feat", &[]);
    sd.pass();
    assert_has(&sd.last_pass(), &["fetch from nowhere failed"]);
}

// @test "config: each repo's remote from its own repo file, never another's (#194)"
#[test]
fn config_each_repos_remote_from_its_own_repo_file_never_anothers_194() {
    let sd = sd();
    sd.verb_ok("config", &["set", "--repo", "settings.remote", "nowhere"]);
    sd.repo2_z();
    sd.register(&sd.path("repo2"));
    sd.ps_fixture("feat", &[]);
    sd.ps_fixture("z", &[]);
    sd.pass();
    assert_eq!(sd.last_pass().lines().filter(|l| l.contains("fetch from")).count(), 1);
    assert_match(&sd.last_pass(), "(?m)^repo\t.*fetch from nowhere failed");
}

// @test "config: install reads interval from the file"
#[test]
fn config_install_reads_interval_from_the_file() {
    if no_plutil() {
        return;
    }
    let mut sd = sd();
    let plist = sd.setup_launchd();
    sd.write_cfg(r#"{"settings":{"interval":120}}"#);
    sd.ok(&["install"]);
    assert_eq!(extract(&plist, "StartInterval"), "120");
}

// @test "config: a non-numeric interval in the file is refused"
#[test]
fn config_a_non_numeric_interval_in_the_file_is_refused() {
    let mut sd = sd();
    let plist = sd.setup_launchd();
    sd.write_cfg(r#"{"settings":{"interval":"5m"}}"#);
    let r = sd.run(&["install"]);
    assert_ne!(r.status, 0);
    assert!(!plist.exists());
}

// @test "config: an unreadable config file is reported for the repo, not silently defaulted"
#[test]
fn config_an_unreadable_config_file_is_reported_for_the_repo_not_silently_defaulted() {
    let sd = sd();
    sd.write_global("{not json");
    sd.ps_fixture("feat", &[]);
    sd.pass();
    assert_match(&sd.last_pass(), "repo.*sd config failed");
    assert!(!sd.ps("calls").is_file());
}

// @test "config: install refuses when the config file is unreadable"
#[test]
fn config_install_refuses_when_the_config_file_is_unreadable() {
    let mut sd = sd();
    let plist = sd.setup_launchd();
    sd.write_global("{not json");
    let r = sd.run(&["install"]);
    assert_ne!(r.status, 0);
    assert!(!plist.exists());
}

// @test "rate limit: vacuum skips the branches the pass didn't check, and says so"
#[test]
fn rate_limit_vacuum_skips_the_branches_the_pass_didnt_check_and_says_so() {
    let sd = sd();
    sd.git(&["branch", "other"]);
    sd.enroll("other");
    sd.low("feat", 100);
    sd.ps_fixture("other", &[r#".pr.state = "MERGED""#]);
    let r = sd.run(&["vacuum"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, "repo  other  skipped, not checked: GitHub rate limit");
    assert!(sd.enrolled("other"));
}
