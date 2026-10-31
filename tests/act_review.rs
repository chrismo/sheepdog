// Black-box tests of sd act review (A8) (#203): verify first, then the local
// reviewer (settings.localReview, default sd review-local) over the branch's
// own change. Clean → pin local at HEAD with its base. Findings → printed as
// JSON, nothing pinned. Reviewer failure → nonzero, never clean. The built
// binary, run as sd-act through a link (tests/common). Moved from
// libexec/sd-act-review.bats; each test keeps the bats test's title above it.
//
// Real git, the real sd state (in-process) and sd review-local (gh is
// a stub that fails, so only git facts). The reviewer is a stub that records
// its env and prints $FX/findings.json.

mod common;

use common::{assert_has, assert_lacks, assert_match, jq, running_marker, spawn, Holder, Sd, GH_FAILS};
use std::fs;
use std::process::Stdio;
use std::time::Duration;

const REVIEWER: &str = r#"#!/usr/bin/env bash
{ echo "base=$SHEEPDOG_BASE"; echo "head=$SHEEPDOG_HEAD"; echo "task=$SHEEPDOG_TASK"; echo "cwd=$PWD"; } > "$FX/reviewer.env"
[[ -e $FX/reviewer.fail ]] && { echo "reviewer: boom" >&2; exit 1; }
cat "$FX/findings.json"
"#;

const BLOCKING: &str = r#"[{"file": "file.txt", "line": 3, "severity": "error", "body": "c?"}]"#;

/// The bats setup: origin.git and its clone repo (cwd), main pushed, feat
/// one commit past it; settings.localReview is the stub reviewer, with no
/// findings.
fn sd() -> Sd {
    let mut sd = Sd::for_verb("act");
    sd.env = vec![
        ("SHEEPDOG_STATE_DIR".to_string(), sd.path("state")),
        ("FX".to_string(), sd.path("fx")),
    ];
    fs::create_dir(sd.tmp.join("fx")).unwrap();
    sd.fake("gh", GH_FAILS);
    sd.script(&sd.tmp.join("reviewer"), REVIEWER);
    sd.findings("[]");
    sd.set_global("settings.localReview", &sd.path("reviewer"));
    sd.origin(&["repo"]);
    sd.cd("repo");
    sd.write("file.txt", "a\nb\n");
    sd.git(&["add", "file.txt"]);
    sd.git(&["commit", "-qm", "init"]);
    sd.git(&["push", "-q", "origin", "main"]);
    sd.git(&["switch", "-qc", "feat"]);
    sd.write("file.txt", "a\nb\nc\n");
    sd.git(&["commit", "-qam", "feat"]);
    sd
}

impl Sd {
    fn set_global(&self, key: &str, value: &str) {
        self.verb_ok("config", &["set", "--global", key, value]);
    }

    fn unset_global(&self, key: &str) {
        self.verb_ok("config", &["unset", "--global", key]);
    }

    fn findings(&self, json: &str) {
        fs::write(self.tmp.join("fx/findings.json"), format!("{json}\n")).unwrap();
    }

    fn fx_exists(&self, name: &str) -> bool {
        self.tmp.join("fx").join(name).exists()
    }

    fn rev(&self, r: &str) -> String {
        self.git(&["rev-parse", r])
    }

    /// env_field <name>: the reviewer's env as it recorded it.
    fn env_field(&self, name: &str) -> String {
        let env = fs::read_to_string(self.tmp.join("fx/reviewer.env")).unwrap();
        let prefix = format!("{name}=");
        env.lines().filter_map(|l| l.strip_prefix(&prefix)).collect::<Vec<_>>().join("\n")
    }

    /// pin <kind>: refs/sheepdog/feat/<kind>'s sha, or "".
    fn pin(&self, kind: &str) -> String {
        self.git_try(&["rev-parse", "-q", "--verify", &format!("refs/sheepdog/feat/{kind}")]).unwrap_or_default()
    }

    /// findings_rec <filter>: the findings record, through jq -c.
    fn findings_rec(&self, filter: &str) -> String {
        jq(&["-c", filter], &self.git(&["cat-file", "blob", "refs/sheepdog/feat/findings"]))
    }

    fn has_findings_ref(&self) -> bool {
        self.git_status(&["rev-parse", "-q", "--verify", "refs/sheepdog/feat/findings"]) == 0
    }

    /// `"$SD_ACT" review >/dev/null 2>&1`, which must succeed.
    fn review(&self) {
        self.ok(&["review"]);
    }

    fn write_findings_ref(&self, json: &str) {
        let blob = self.git_with(&[], Some(&format!("{json}\n")), &["hash-object", "-w", "--stdin"]);
        self.git(&["update-ref", "refs/sheepdog/feat/findings", &blob]);
    }
}

// The local review spends too: past settings.branchBudget it's refused, the
// reviewer never runs, --force or not (#349).
#[test]
fn past_settings_branchbudget_the_reviewer_never_runs_349() {
    let sd = sd();
    sd.verb_ok("config", &["set", "--global", "--json", "settings.branchBudget", "2"]);
    sd.spent("feat", "review", "2.5");
    let r = sd.run(&["review", "--force"]);
    assert_eq!(r.status, 1);
    assert_has(&r.output, &["not running review on feat: spent $2.50 on this branch, past settings.branchBudget ($2.00): raise it, or take over"]);
    assert!(!sd.tmp.join("fx/reviewer.env").exists());
}

// @test "clean: pins local at HEAD with the branch's base"
#[test]
fn clean_pins_local_at_head_with_the_branchs_base() {
    let sd = sd();
    let r = sd.run(&["review"]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &["clean"]);
    assert_eq!(sd.pin("local"), sd.rev("HEAD"));
    assert_eq!(sd.pin("local-base"), sd.rev("origin/main"));
}

// @test "the reviewer gets base, head, task and the worktree as cwd"
#[test]
fn the_reviewer_gets_base_head_task_and_the_worktree_as_cwd() {
    let sd = sd();
    sd.git(&["config", "branch.feat.sd-task", "add c"]);
    sd.review();
    assert_eq!(sd.env_field("base"), sd.rev("origin/main"));
    assert_eq!(sd.env_field("head"), sd.rev("HEAD"));
    assert_eq!(sd.env_field("task"), "add c");
    assert_eq!(sd.env_field("cwd"), sd.path("repo"));
}

// @test "findings: printed as JSON, nothing pinned, exit 0"
#[test]
fn findings_printed_as_json_nothing_pinned_exit_0() {
    let sd = sd();
    sd.findings(r#"[{"file": "file.txt", "line": 3, "severity": "warning", "body": "c?"}]"#);
    let r = sd.run_sep(&["review"]);
    assert_eq!(r.status, 0);
    assert_eq!(jq(&["-c", "."], &r.output), r#"[{"file":"file.txt","line":3,"severity":"warning","body":"c?"}]"#);
    assert_has(&r.stderr, &["1 finding"]);
    assert_eq!(sd.pin("local"), "");
}

// @test "verify failing: the reviewer never runs"
#[test]
fn verify_failing_the_reviewer_never_runs() {
    let sd = sd();
    sd.set_global("settings.verify", "false");
    let r = sd.run(&["review"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["verify failed"]);
    assert!(!sd.fx_exists("reviewer.env"));
    assert_eq!(sd.pin("local"), "");
}

// @test "the reviewer failing: nonzero, nothing pinned, never clean"
#[test]
fn the_reviewer_failing_nonzero_nothing_pinned_never_clean() {
    let sd = sd();
    fs::write(sd.tmp.join("fx/reviewer.fail"), "").unwrap();
    let r = sd.run(&["review"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["reviewer failed"]);
    assert_lacks(&r.output, "clean");
    assert_eq!(sd.pin("local"), "");
}

// @test "the reviewer's stderr shows while it runs, not after it exits"
#[test]
fn the_reviewers_stderr_shows_while_it_runs_not_after_it_exits() {
    let sd = sd();
    sd.script(
        &sd.tmp.join("reviewer"),
        r#"#!/usr/bin/env bash
echo "reviewer: started" >&2
for _ in {1..100}; do [[ -e $FX/go ]] && break; sleep 0.1; done
echo '[]'
"#,
    );
    let err = sd.tmp.join("err");
    let mut job = spawn(
        sd.command(&sd.bin)
            .arg("review")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(fs::File::create(&err).unwrap()),
    );
    let mut seen = false;
    for _ in 0..30 {
        if fs::read_to_string(&err).unwrap_or_default().contains("reviewer: started") {
            seen = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    fs::write(sd.tmp.join("fx/go"), "").unwrap();
    assert!(job.wait().unwrap().success());
    assert!(seen, "the reviewer's stderr didn't show while it ran");
}

// @test "reviewer output that isn't a JSON list is a failure"
#[test]
fn reviewer_output_that_isnt_a_json_list_is_a_failure() {
    let sd = sd();
    sd.findings("looks fine to me");
    let r = sd.run(&["review"]);
    assert_ne!(r.status, 0);
    assert_eq!(sd.pin("local"), "");
}

// @test "uncommitted changes are refused"
#[test]
fn uncommitted_changes_are_refused() {
    let sd = sd();
    sd.write("file.txt", "a\nb\nc\nd\n");
    let r = sd.run(&["review"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["uncommitted"]);
}

// @test "a branch checked out nowhere is refused"
#[test]
fn a_branch_checked_out_nowhere_is_refused() {
    let sd = sd();
    sd.git(&["branch", "parked"]);
    let r = sd.run(&["review", "parked"]);
    assert_ne!(r.status, 0);
}

// @test "the default reviewer is libexec/sd-review-local"
#[test]
fn the_default_reviewer_is_libexec_sd_review_local() {
    let sd = sd();
    sd.unset_global("settings.localReview");
    sd.fake(
        "claude",
        r#"#!/usr/bin/env bash
printf '%s\n' "$@" > "$FX/claude.args"
echo '{"type": "result", "is_error": false, "structured_output": {"findings": []}}'
"#,
    );
    let r = sd.run(&["review"]);
    assert_eq!(r.status, 0);
    assert!(sd.fx_exists("claude.args"));
    assert_eq!(sd.pin("local"), sd.rev("HEAD"));
}

// @test "a hung default reviewer times out inside, so the harness's stderr still shows"
#[test]
fn a_hung_default_reviewer_times_out_inside_so_the_harnesss_stderr_still_shows() {
    let sd = sd();
    sd.unset_global("settings.localReview");
    sd.set_global("settings.stepTimeout", "1");
    sd.fake(
        "claude",
        r#"#!/usr/bin/env bash
echo "claude: still thinking" >&2
sleep 5
"#,
    );
    let r = sd.run_sep(&["review"]);
    assert_ne!(r.status, 0);
    assert_has(&r.stderr, &["claude: still thinking"]);
    assert_has(&r.stderr, &["timed out after 1s"]);
    assert_eq!(sd.pin("local"), "");
}

// @test "a custom reviewer gets stepTimeout, with no margin"
#[test]
fn a_custom_reviewer_gets_steptimeout_with_no_margin() {
    let sd = sd();
    sd.set_global("settings.stepTimeout", "1");
    sd.script(&sd.tmp.join("reviewer"), "#!/usr/bin/env bash\nsleep 3\necho \"[]\"\n");
    let r = sd.run_sep(&["review"]);
    assert_ne!(r.status, 0);
    assert_has(&r.stderr, &["timed out after 1s"]);
    assert_eq!(sd.pin("local"), "");
}

// @test "only nits: clean, pins local, still prints them"
#[test]
fn only_nits_clean_pins_local_still_prints_them() {
    let sd = sd();
    sd.findings(r#"[{"file": "file.txt", "line": 3, "severity": "nit", "body": "tiny"}]"#);
    let r = sd.run_sep(&["review"]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &["clean", "1 nit"]);
    assert!(r.output.contains(r#""body":"tiny""#) || r.output.contains(r#""body": "tiny""#), "{}", r.output);
    assert_eq!(sd.pin("local"), sd.rev("HEAD"));
}

// @test "findings that block: recorded with head, base and round"
#[test]
fn findings_that_block_recorded_with_head_base_and_round() {
    let sd = sd();
    sd.findings(
        r#"[{"file": "file.txt", "line": 3, "severity": "warning", "body": "c?"}, {"file": "file.txt", "line": 1, "severity": "nit", "body": "a"}]"#,
    );
    sd.review();
    assert_eq!(sd.findings_rec(".head"), format!("\"{}\"", sd.rev("HEAD")));
    assert_eq!(sd.findings_rec(".base"), format!("\"{}\"", sd.rev("origin/main")));
    assert_eq!(sd.findings_rec(".round"), "1");
    assert_eq!(sd.findings_rec(".findings | length"), "2");
}

// @test "each review that still blocks is the next round"
#[test]
fn each_review_that_still_blocks_is_the_next_round() {
    let sd = sd();
    sd.findings(BLOCKING);
    sd.review();
    sd.write("file.txt", "a\nb\nc\nd\n");
    sd.git(&["commit", "-qam", "more"]);
    sd.review();
    assert_eq!(sd.findings_rec(".round"), "2");
    assert_eq!(sd.findings_rec(".head"), format!("\"{}\"", sd.rev("HEAD")));
}

// @test "a clean review clears the findings record, so rounds start over"
#[test]
fn a_clean_review_clears_the_findings_record_so_rounds_start_over() {
    let sd = sd();
    sd.findings(BLOCKING);
    sd.review();
    sd.findings("[]");
    sd.review();
    assert!(!sd.has_findings_ref());
}

// @test "--accept: pins accepted at HEAD with its base, clears findings, runs no reviewer"
#[test]
fn accept_pins_accepted_at_head_with_its_base_clears_findings_runs_no_reviewer() {
    let sd = sd();
    sd.findings(BLOCKING);
    sd.review();
    fs::remove_file(sd.tmp.join("fx/reviewer.env")).unwrap();
    let r = sd.run(&["review", "--accept"]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &["accepted"]);
    assert_eq!(sd.pin("accepted"), sd.rev("HEAD"));
    assert_eq!(sd.pin("accepted-base"), sd.rev("origin/main"));
    // no trigger yet: none to record
    assert_ne!(sd.git_status(&["config", "--get", "branch.feat.sd-accepted-after"]), 0);
    assert!(!sd.fx_exists("reviewer.env"));
    assert!(!sd.has_findings_ref());
}

// @test "--accept: records the bot trigger it followed, so NEXT can order them without comparing clocks (#181)"
#[test]
fn accept_records_the_bot_trigger_it_followed_so_next_can_order_them_without_comparing_clocks_181() {
    let sd = sd();
    sd.git(&["config", "branch.feat.sd-bot-triggered", "2026-10-15T12:00:00Z"]);
    sd.ok(&["review", "--accept"]);
    assert_eq!(sd.git(&["config", "branch.feat.sd-accepted-after"]), "2026-10-15T12:00:00Z");
    // a later accept with no trigger to follow drops the old one
    sd.git(&["config", "--unset", "branch.feat.sd-bot-triggered"]);
    sd.ok(&["review", "--accept"]);
    assert_ne!(sd.git_status(&["config", "--get", "branch.feat.sd-accepted-after"]), 0);
}

#[test]
fn a_head_the_local_review_covers_is_refused_at_zero_tokens_unless_force_241() {
    let sd = sd();
    sd.review();
    fs::remove_file(sd.tmp.join("fx/reviewer.env")).unwrap();
    let r = sd.run(&["review"]);
    assert_ne!(r.status, 0);
    let head = sd.rev("HEAD");
    assert_has(&r.output, &[&format!("local review already covers {}; --force to review again", &head[..7])]);
    assert!(!sd.fx_exists("reviewer.env"));
    sd.ok(&["review", "--force"]);
    assert!(sd.fx_exists("reviewer.env"));
}

#[test]
fn the_same_change_rebased_is_covered_too_241() {
    let sd = sd();
    sd.review();
    sd.git(&["commit", "-q", "--amend", "--no-edit", "--date", "2001-01-01T00:00:00Z"]);
    assert_ne!(sd.pin("local"), sd.rev("HEAD"));
    let r = sd.run(&["review"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["local review already covers"]);
}

#[test]
fn a_rewrite_that_changes_the_change_is_reviewed_241() {
    let sd = sd();
    sd.review();
    sd.write("file.txt", "a\nb\nC\n");
    sd.git(&["commit", "-q", "--amend", "-a", "--no-edit"]);
    sd.review();
    assert_eq!(sd.pin("local"), sd.rev("HEAD"));
}

#[test]
fn a_commit_past_the_local_review_is_reviewed_241() {
    let sd = sd();
    sd.review();
    sd.write("file.txt", "a\nb\nc\nd\n");
    sd.git(&["commit", "-qam", "more"]);
    sd.review();
    assert_eq!(sd.pin("local"), sd.rev("HEAD"));
}

// @test "a findings record with no usable round starts the count over"
#[test]
fn a_findings_record_with_no_usable_round_starts_the_count_over() {
    let sd = sd();
    sd.write_findings_ref(r#"{"round": "lots"}"#);
    sd.findings(BLOCKING);
    let r = sd.run_sep(&["review"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.findings_rec(".round"), "1");
}

// @test "flags (#125): printed on stderr for chrismo, clean or not; never a gate"
#[test]
fn flags_125_printed_on_stderr_for_chrismo_clean_or_not_never_a_gate() {
    let sd = sd();
    sd.write("a.bats", "@test \"a\" {\n  skip\n}\n");
    sd.git(&["add", "a.bats"]);
    sd.git(&["commit", "-qm", "skip"]);
    let r = sd.run_sep(&["review"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.pin("local"), sd.rev("HEAD"));
    assert_has(&r.stderr, &["1 flagged for chrismo"]);
    assert_match(&r.stderr, r"⚑ +skip +a\.bats:2");
    sd.findings(r#"[{"file": "file.txt", "line": 3, "severity": "warning", "body": "c?"}]"#);
    let r = sd.run_sep(&["review", "--force"]);
    assert_eq!(r.status, 0);
    assert_eq!(jq(&["length"], &r.output), "1");
    assert_match(&r.stderr, r"⚑ +skip +a\.bats:2");
}

// @test "no flags: nothing about them is printed"
#[test]
fn no_flags_nothing_about_them_is_printed() {
    let sd = sd();
    let r = sd.run_sep(&["review"]);
    assert_eq!(r.status, 0);
    assert_lacks(&format!("{}{}", r.stderr, r.output), "flagged");
}

// @test "a second review of the same HEAD keeps the round; its findings start unanswered"
#[test]
fn a_second_review_of_the_same_head_keeps_the_round_its_findings_start_unanswered() {
    let sd = sd();
    sd.findings(BLOCKING);
    sd.review();
    let rec = sd.findings_rec(r#".findings[0] += {action: "declined", reply: "c is right"}"#);
    sd.write_findings_ref(&rec);
    sd.review();
    assert_eq!(sd.findings_rec("[.round, .findings[0].action]"), "[1,null]");
}

#[test]
fn review_the_default_reviewers_agent_stopped_on_its_usage_limit_exits_75_269() {
    let sd = sd();
    sd.unset_global("settings.localReview");
    sd.fake(
        "claude",
        r#"#!/usr/bin/env bash
echo '{"type":"result","subtype":"success","is_error":true,"api_error_status":429,"result":"You'"'"'ve hit your session limit · resets 12:20am (America/Chicago)","total_cost_usd":0,"num_turns":1}'
exit 1
"#,
    );
    let r = sd.run(&["review"]);
    assert_eq!(r.status, 75, "{}", r.output);
    assert_has(&r.output, &["review stopped on claude's usage limit"]);
    assert_eq!(sd.pin("local"), "");
}

#[test]
fn review_a_custom_reviewer_exiting_75_pauses_agent_steps_too_269() {
    let sd = sd();
    sd.script(&sd.tmp.join("reviewer"), "#!/usr/bin/env bash\necho \"my reviewer: out of quota\" >&2\nexit 75\n");
    let r = sd.run(&["review"]);
    assert_eq!(r.status, 75, "{}", r.output);
    let limit = fs::read_to_string(sd.tmp.join("state/limit")).unwrap();
    assert_match(&limit, &format!(r"^\S+Z reviewer: {} exited 75\n$", regex::escape(&sd.path("reviewer"))));
}

#[test]
fn review_a_reviewer_exiting_75_leaves_a_limit_that_already_holds_269() {
    let sd = sd();
    let soon = sheepdog::time::utc((sheepdog::time::now_utc().1 / 1_000_000_000) as i64 + 600);
    fs::create_dir_all(sd.tmp.join("state")).unwrap();
    let line = format!("{soon} claude: You've hit your session limit\n");
    fs::write(sd.tmp.join("state/limit"), &line).unwrap();
    sd.script(&sd.tmp.join("reviewer"), "#!/usr/bin/env bash\nexit 75\n");
    assert_eq!(sd.run(&["review"]).status, 75);
    assert_eq!(fs::read_to_string(sd.tmp.join("state/limit")).unwrap(), line);
}

// @test "a live step's running marker refuses review before verify runs (#270)"
#[test]
fn a_live_steps_running_marker_refuses_review_before_verify_runs_270() {
    let sd = sd();
    sd.set_global("settings.verify", "touch ../verify-ran");
    let holder = Holder::new();
    let marker = running_marker(&sd.tmp.join("repo/.git"), "feat", "address", holder.pid());
    let r = sd.run(&["review"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &[&format!("address is running on feat (pid {}, since", holder.pid()), "not starting another agent step"]);
    assert!(!sd.tmp.join("verify-ran").exists());
    assert!(!sd.fx_exists("reviewer.env"));
    assert_eq!(sd.pin("local"), "");
    assert!(marker.exists());
}

// @test "verify and a custom reviewer run under review's running marker, removed after (#270)"
#[test]
fn verify_and_a_custom_reviewer_run_under_reviews_running_marker_removed_after_270() {
    let sd = sd();
    sd.set_global("settings.verify", "cp .git/sheepdog/feat.running ../verify-saw");
    sd.script(
        &sd.tmp.join("reviewer"),
        "#!/usr/bin/env bash\ncp .git/sheepdog/feat.running \"$FX/reviewer-saw\" && echo '[]'\n",
    );
    sd.review();
    for saw in [sd.tmp.join("verify-saw"), sd.tmp.join("fx/reviewer-saw")] {
        let line = fs::read_to_string(&saw).unwrap_or_default();
        assert_eq!(line.split_whitespace().nth(1), Some("review"), "{}: {line}", saw.display());
    }
    assert!(!sd.tmp.join("repo/.git/sheepdog/feat.running").exists());
}
