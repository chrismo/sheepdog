// The NEXT rules (src/next.rs): sd state JSON in, the ordered next steps out.
// Pure policy, so no git or gh: each test edits a "ready to merge" state with
// jq assignments. Moved from lib/next.bats (#200); each test keeps the bats
// test's title above it.

use serde_json::{json, Value};
use sheepdog::next::{lines, steps};
use std::io::Write;
use std::process::{Command, Stdio};

const READY: &str = r#"{"branch": "feat", "head": "h1",
    "pr": {"number": 7, "url": "u", "state": "OPEN", "draft": false, "base": "main", "head_sha": "h1", "mergeable": "MERGEABLE",
           "merge_state": "CLEAN", "review_decision": null, "review_requests": []},
    "parent": {"name": "main", "source": "pr", "merged": false, "pr": null},
    "sync": {"target": "main", "default": "main", "ahead": 1, "behind": 0, "up_to_date": true, "old_base": "b0b0b0b0b0b0b0b0", "conflict": false},
    "push": {"remote_sha": "h1", "ahead": 0, "behind": 0, "diverged": false, "remote_ours": true},
    "fresh": true, "stale": [],
    "ci": {"sha": "h1", "for_head": true, "verdict": "green", "failing": [], "pending": []},
    "review": {"local": {"sha": "h1", "status": "covered"}, "bot": {"sha": null, "status": "none"},
               "accepted": {"sha": null, "status": "none"}},
    "threads": {"owed_bot": 0, "owed_human": 0, "outdated": 0, "blocking": 0, "info": 0, "items": []},
    "session": {"enrolled": true, "harness": null, "id": null, "locked": false},
    "errors": []}"#;

// A stacked branch: its parent is another branch, not the default.
const STACKED: &[&str] = &[r#".parent.name = "base""#, r#".sync.target = "base""#, ".sync.up_to_date = false", ".sync.behind = 3"];

const FRESH: &[&str] = &[
    ".pr = null",
    ".sync.ahead = 0",
    r#".push = {"remote_sha": null, "ahead": 0, "behind": 0, "diverged": false, "remote_ours": null}"#,
];

const UNREVIEWED: &str = r#".review.local = {sha: null, status: "none"}"#;
const ON_OPEN_PARENT: &[&str] = &[
    r#".pr.base = "p""#,
    r#".sync.target = "p""#,
    r#".parent = {"name": "p", "source": "pr", "merged": false, "pr": {"number": 25, "state": "OPEN", "head_sha": "x"}}"#,
];

/// One edit or a list of them, for e![].
trait Ed {
    fn add(self, v: &mut Vec<String>);
}
impl Ed for &str {
    fn add(self, v: &mut Vec<String>) {
        v.push(self.to_string());
    }
}
impl Ed for &String {
    fn add(self, v: &mut Vec<String>) {
        v.push(self.clone());
    }
}
impl Ed for &[&str] {
    fn add(self, v: &mut Vec<String>) {
        v.extend(self.iter().map(|s| s.to_string()));
    }
}
impl Ed for &Vec<String> {
    fn add(self, v: &mut Vec<String>) {
        v.extend(self.iter().cloned());
    }
}

/// The edits, in order: strings, and lists spliced in (bash's "${a[@]}").
macro_rules! e {
    ($($x:expr),* $(,)?) => {{
        #[allow(unused_mut)]
        let mut v: Vec<String> = Vec::new();
        $( Ed::add($x, &mut v); )*
        v
    }};
}

fn jq(filter: &str, input: &str) -> String {
    let mut child = Command::new("jq")
        .args(["-c", filter])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("jq runs");
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "jq failed on {filter}");
    String::from_utf8(out.stdout).unwrap()
}

/// READY with the edits applied.
fn state(edits: &[String]) -> Value {
    let s = edits.iter().fold(READY.to_string(), |s, a| jq(a, &s));
    serde_json::from_str(&s).unwrap()
}

/// The next steps for READY with the edits applied.
fn next(edits: &[String]) -> Value {
    steps(&state(edits), "origin").unwrap()
}

fn keys(edits: &[String]) -> String {
    next(edits).as_array().unwrap().iter().map(|s| s["key"].as_str().unwrap().to_string()).collect::<Vec<_>>().join(" ")
}

/// jq -r's text of a value: strings raw, null as "null".
fn raw(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        _ => v.to_string(),
    }
}

/// <field> of the step <key>; nothing when there's no such step.
fn step(key: &str, field: &str, edits: &[String]) -> String {
    next(edits)
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["key"] == key)
        .map(|s| raw(&s[field]))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The question edit: sd state's field for an agent step's question (#167).
fn asked(step: &str, at_head: bool) -> String {
    let q = json!({"step": step, "sha": "h1", "text": "Which test is right?\n1. revert it\n2. keep it", "at_head": at_head});
    format!(".question = {q}")
}

/// The NEXT line(s) for READY with the edits, as bats's $output has them.
fn lines_for(all: bool, edits: &[String]) -> String {
    lines(&next(edits), all).trim_end_matches('\n').to_string()
}

// @test "ready: the only step is to merge, by chrismo"
#[test]
fn ready_the_only_step_is_to_merge_by_chrismo() {
    assert_eq!(keys(&e![]), "ready");
    assert_eq!(step("ready", "who", &e![]), "you");
    assert_eq!(step("ready", "cmd", &e![]), "gh pr merge");
}

// @test "ready: only when GitHub would merge it (clean, unstable or has hooks)"
#[test]
fn ready_only_when_github_would_merge_it_clean_unstable_or_has_hooks() {
    assert_eq!(keys(&e![r#".pr.merge_state = "UNSTABLE""#]), "ready");
    assert_eq!(keys(&e![r#".pr.merge_state = "HAS_HOOKS""#]), "ready");
    assert_eq!(keys(&e![r#".pr.merge_state = "BLOCKED""#]), "blocked");
    assert_eq!(keys(&e![r#".pr.merge_state = "UNKNOWN""#]), "merge-wait");
    assert_eq!(keys(&e!["del(.pr.merge_state)"]), "merge-wait");
}

// @test "review required: wait on the requested reviewers, never ready"
#[test]
fn review_required_wait_on_the_requested_reviewers_never_ready() {
    let req = e![r#".pr.merge_state = "BLOCKED""#, r#".pr.review_decision = "REVIEW_REQUIRED""#];
    assert_eq!(keys(&e![&req, r#".pr.review_requests = ["alice", "core-team"]"#]), "review-wait");
    assert_eq!(step("review-wait", "who", &e![&req]), "wait");
    assert_eq!(step("review-wait", "say", &e![&req, r#".pr.review_requests = ["alice", "core-team"]"#]), "waiting on review from alice, core-team");
    assert_eq!(step("review-wait", "say", &e![&req]), "waiting on a required review");
}

// @test "review required stays listed behind other steps; ready-like ends don't"
#[test]
fn review_required_stays_listed_behind_other_steps_ready_like_ends_don_t() {
    assert_eq!(keys(&e![r#".ci.verdict = "pending""#, r#".pr.merge_state = "BLOCKED""#, r#".pr.review_decision = "REVIEW_REQUIRED""#]), "ci-wait review-wait");
    assert_eq!(keys(&e![r#".ci.verdict = "pending""#, r#".pr.merge_state = "BLOCKED""#]), "ci-wait");
    assert_eq!(keys(&e![r#".ci.verdict = "pending""#, r#".pr.merge_state = "UNKNOWN""#]), "ci-wait");
}

// @test "stacked on an open parent PR: wait for the parent, never ready"
#[test]
fn stacked_on_an_open_parent_pr_wait_for_the_parent_never_ready() {
    let st = e![r#".pr.base = "p""#, r#".sync.target = "p""#, r#".parent = {"name": "p", "source": "pr", "merged": false, "pr": {"number": 25, "state": "OPEN", "head_sha": "x"}}"#];
    assert_eq!(keys(&e![&st]), "parent-wait");
    assert_eq!(step("parent-wait", "who", &e![&st]), "wait");
    assert_eq!(step("parent-wait", "say", &e![&st]), "waiting on p (#25) to merge first");
    assert_eq!(keys(&e![&st, ".push.ahead = 1"]), "push parent-wait");
}

// @test "the parent merged but the PR still targets it: retarget points it at the default branch"
#[test]
fn the_parent_merged_but_the_pr_still_targets_it_retarget_points_it_at_the_default_branch() {
    let st = e![r#".pr.base = "p""#, r#".parent = {"name": "p", "source": "pr", "merged": true, "pr": {"number": 25, "state": "MERGED", "head_sha": "x"}}"#];
    assert_eq!(keys(&e![&st]), "retarget");
    assert_eq!(step("retarget", "who", &e![&st]), "run");
    assert_eq!(step("retarget", "say", &e![&st]), "p already merged: point this PR at main");
    assert_eq!(step("retarget", "cmd", &e![&st]), "sd act retarget");
}

// @test "a parent that exists only here: ship it first; no push or open-pr until then (#105)"
#[test]
fn a_parent_that_exists_only_here_ship_it_first_no_push_or_open_pr_until_then_105() {
    let st = e![".pr = null", r#".push = {"remote_sha": null, "ahead": 2, "behind": 0, "diverged": false, "remote_ours": null}"#, r#".parent = {"name": "p", "source": "config", "merged": false, "local": true, "on_remote": false, "unpushed": true, "pr": null}"#, r#".sync = {"target": "p", "default": "main", "behind": null, "up_to_date": null, "conflict": null, "old_base": null}"#];
    assert_eq!(keys(&e![&st]), "ship-parent");
    assert_eq!(step("ship-parent", "who", &e![&st]), "run");
    assert_eq!(step("ship-parent", "say", &e![&st]), "ship the parent first: p isn't on origin yet");
    assert_eq!(step("ship-parent", "cmd", &e![&st]), "sd act ship p");
    assert_eq!(keys(&e![&st, ".parent.on_remote = true | .parent.unpushed = false"]), "push open-pr");
}

// @test "the parent closed without merging: chrismo decides; no restack, never ready"
#[test]
fn the_parent_closed_without_merging_chrismo_decides_no_restack_never_ready() {
    let st = e![r#".pr.base = "p""#, r#".sync.target = "p""#, ".sync.up_to_date = false", ".sync.behind = 2", r#".parent = {"name": "p", "source": "config", "merged": false, "pr": {"number": 25, "state": "CLOSED", "head_sha": "x"}}"#];
    assert_eq!(keys(&e![&st]), "parent-closed");
    assert_eq!(step("parent-closed", "who", &e![&st]), "you");
    assert_eq!(step("parent-closed", "say", &e![&st]), "p (#25) was closed without merging: decide what this PR targets");
    assert_eq!(keys(&e![&st, ".push.ahead = 1"]), "push parent-closed");
}

// @test "a base branch with no PR of its own (a release branch) can still be ready"
#[test]
fn a_base_branch_with_no_pr_of_its_own_a_release_branch_can_still_be_ready() {
    assert_eq!(keys(&e![r#".pr.base = "release""#, r#".sync.target = "release""#, r#".parent = {"name": "release", "source": "pr", "merged": false, "pr": null}"#]), "ready");
}

// @test "GitHub's BEHIND (required up-to-date branches): restack onto the default branch, a real step"
#[test]
fn github_s_behind_required_up_to_date_branches_restack_onto_the_default_branch_a_real_step() {
    // Observed in sheepdog-sandbox 2026-10-14 (findings/6-behind.md): BEHIND needs a
    // required status check plus "require branches to be up to date", and then
    // wins over a required review. So it is a restack to run, not a blocked state.
    assert_eq!(keys(&e![r#".pr.merge_state = "BEHIND""#, ".sync.up_to_date = false", ".sync.behind = 3"]), "behind");
    assert_eq!(step("behind", "who", &e![r#".pr.merge_state = "BEHIND""#, ".sync.up_to_date = false", ".sync.behind = 3"]), "run");
    assert_eq!(step("behind", "say", &e![r#".pr.merge_state = "BEHIND""#, ".sync.up_to_date = false", ".sync.behind = 3"]), "restack onto main (3 behind): GitHub requires branches to be up to date");
    assert_eq!(step("behind", "cmd", &e![r#".pr.merge_state = "BEHIND""#, ".sync.up_to_date = false", ".sync.behind = 3"]), "sd act restack");
    // a required review on top still waits behind the restack
    assert_eq!(keys(&e![r#".pr.merge_state = "BEHIND""#, ".sync.up_to_date = false", ".sync.behind = 3", r#".pr.review_decision = "REVIEW_REQUIRED""#]), "behind review-wait");
}

// @test "blocked for another reason: chrismo looks on GitHub"
#[test]
fn blocked_for_another_reason_chrismo_looks_on_github() {
    assert_eq!(step("blocked", "who", &e![r#".pr.merge_state = "BLOCKED""#, r#".pr.review_decision = "APPROVED""#]), "you");
    assert_eq!(step("blocked", "say", &e![r#".pr.merge_state = "BLOCKED""#]), "GitHub says the merge is blocked: see why");
    assert_eq!(keys(&e![r#".pr.merge_state = "BEHIND""#]), "behind");
    assert_eq!(step("blocked", "cmd", &e![r#".pr.merge_state = "BLOCKED""#]), "gh pr view --web");
}

// @test "every step has key, who, say and cmd"
#[test]
fn every_step_has_key_who_say_and_cmd() {
    let out = next(&e![".push.ahead = 2", ".threads.owed_bot = 1", r#".ci.verdict = "pending""#]);
    let out = out.as_array().unwrap();
    assert!(out.iter().all(|s| {
        let mut k: Vec<&String> = s.as_object().unwrap().keys().collect();
        k.sort();
        k == ["cmd", "key", "say", "who"]
    }));
    assert!(out.iter().all(|s| ["you", "agent", "run", "wait"].contains(&s["who"].as_str().unwrap_or(""))));
}

// @test "merged: only the cleanup step, whatever else is true"
#[test]
fn merged_only_the_cleanup_step_whatever_else_is_true() {
    assert_eq!(keys(&e![r#".pr.state = "MERGED""#, ".push.ahead = 3", r#".ci.verdict = "red""#, r#".errors = ["x"]"#]), "merged");
    assert_eq!(step("merged", "cmd", &e![r#".pr.state = "MERGED""#]), "sd act forget --remove");
    assert_eq!(step("merged", "say", &e![r#".pr.state = "MERGED""#]), "merged: drop the branch's pins and enrollment, remove its worktree and delete it");
}

// @test "closed: only the closed step"
#[test]
fn closed_only_the_closed_step() {
    assert_eq!(keys(&e![r#".pr.state = "CLOSED""#, ".threads.owed_bot = 2"]), "closed");
}

// @test "errors come first and point at --json"
#[test]
fn errors_come_first_and_point_at_json() {
    assert_eq!(keys(&e![r#".errors = ["gh: boom"]"#, ".push.ahead = 1"]), "errors push");
    assert_eq!(step("errors", "say", &e![r#".errors = ["a", "b"]"#]), "2 errors: the steps below may be wrong");
    assert_eq!(step("errors", "cmd", &e![r#".errors = ["a"]"#]), "sd state --json");
}

// @test "stale refs: fetch, mechanically"
#[test]
fn stale_refs_fetch_mechanically() {
    assert_eq!(step("stale", "who", &e![".fresh = false"]), "run");
    assert_eq!(step("stale", "cmd", &e![".fresh = false"]), "git fetch --prune origin");
    assert_eq!(step("stale", "say", &e![".fresh = false"]), "fetch: GitHub is ahead of your remote refs, so the steps below may be wrong");
}

// @test "GitHub behind the remote refs: wait for it, not fetch (#246)"
#[test]
fn github_behind_the_remote_refs_wait_for_it_not_fetch_246() {
    let lag = r#".lag = ["GitHub's feat is at 1111111, behind origin/feat"]"#;
    assert_eq!(keys(&e![lag, ".push.ahead = 1"]), "gh-wait push");
    assert_eq!(step("gh-wait", "who", &e![lag]), "wait");
    assert_eq!(step("gh-wait", "cmd", &e![lag]), "null");
    assert_eq!(step("gh-wait", "say", &e![lag]), "GitHub hasn't seen the push yet, so the steps below may be wrong");
    assert_eq!(keys(&e![".lag = []"]), "ready");
}

// @test "a push that isn't ours: chrismo looks, and no push is suggested"
#[test]
fn a_push_that_isn_t_ours_chrismo_looks_and_no_push_is_suggested() {
    assert_eq!(keys(&e![".push.remote_ours = false", ".push.behind = 2", ".push.theirs = 2", ".push.ahead = 1", ".push.diverged = true"]), "foreign");
    assert_eq!(step("foreign", "cmd", &e![".push.remote_ours = false", ".push.behind = 2", ".push.theirs = 2"]), "git log HEAD..origin/feat");
    assert_eq!(step("foreign", "say", &e![".push.remote_ours = false", ".push.behind = 2", ".push.theirs = 2"]), "origin/feat has 2 commits this copy has never had");
    assert_eq!(step("foreign", "say", &e![".push.remote_ours = false", ".push.behind = 1", ".push.theirs = 1"]), "origin/feat has 1 commit this copy has never had");
}

// @test "a push that isn't ours: our own pre-amend commits under it aren't counted"
#[test]
fn a_push_that_isn_t_ours_our_own_pre_amend_commits_under_it_aren_t_counted() {
    assert_eq!(step("foreign", "say", &e![".push.remote_ours = false", ".push.behind = 2", ".push.theirs = 1"]), "origin/feat has 1 commit this copy has never had");
}

// @test "attached: the session is open interactively; agent steps wait"
#[test]
fn attached_the_session_is_open_interactively_agent_steps_wait() {
    let out = next(&e![".session.locked = true", ".threads.owed_bot = 2"]);
    assert_eq!(raw(&out[0]["key"]), "attached");
    assert_eq!(raw(&out[0]["who"]), "wait");
    assert_eq!(raw(&out[0]["cmd"]), "null");
    assert_eq!(raw(&out[1]["key"]), "bot");
}

// @test "an agent step running: the only step is to wait for it, whatever else the state says (#170)"
#[test]
fn an_agent_step_running_the_only_step_is_to_wait_for_it_whatever_else_the_state_says_170() {
    let run = r#".session.running = {"step": "implement", "pid": 123, "since": "2026-10-18T14:03:00Z"}"#;
    let out = next(&e![run, ".threads.owed_bot = 2"]);
    let got: Vec<Value> = out.as_array().unwrap().iter().map(|s| json!([s["key"], s["who"], s["say"], s["cmd"]])).collect();
    assert_eq!(
        serde_json::to_string(&got).unwrap(),
        r#"[["running","wait","agent: implement running since 2026-10-18T14:03:00Z",null]]"#
    );
    // mid-implement: the agent's edits, not chrismo's to commit or remove
    assert_eq!(keys(&e![FRESH, r#".task = "add c""#, ".push.dirty = true", run]), "running");
    assert_eq!(keys(&e![".session.running = null"]), "ready");
}

// @test "human items: chrismo replies, via the --long list"
#[test]
fn human_items_chrismo_replies_via_the_long_list() {
    assert_eq!(step("human", "who", &e![".threads.owed_human = 2"]), "you");
    assert_eq!(step("human", "say", &e![".threads.owed_human = 2"]), "reply to 2 human items");
    assert_eq!(step("human", "say", &e![".threads.owed_human = 1"]), "reply to 1 human item");
    assert_eq!(step("human", "cmd", &e![".threads.owed_human = 1"]), "sd state --long");
}

// @test "behind a stacked parent: sd act restack"
#[test]
fn behind_a_stacked_parent_sd_act_restack() {
    assert_eq!(step("behind", "who", &e![STACKED]), "run");
    assert_eq!(step("behind", "say", &e![STACKED]), "restack onto base (3 behind)");
    assert_eq!(step("behind", "cmd", &e![STACKED]), "sd act restack");
}

// @test "a tree restack that conflicts is chrismo's: --resolve can't do it"
#[test]
fn a_tree_restack_that_conflicts_is_chrismo_s_resolve_can_t_do_it() {
    assert_eq!(step("conflict", "who", &e![STACKED, ".sync.conflict = true", ".sync.by_tree = true"]), "you");
    assert_eq!(step("conflict", "say", &e![STACKED, ".sync.conflict = true", ".sync.by_tree = true"]), "restack onto base by tree conflicts, which --resolve can't do: resolve it by hand (sd act restack says how)");
    assert_eq!(step("conflict", "cmd", &e![STACKED, ".sync.conflict = true", ".sync.by_tree = true"]), "sd act restack");
}

// @test "behind, by tree: the step says the commits become one"
#[test]
fn behind_by_tree_the_step_says_the_commits_become_one() {
    assert_eq!(step("behind", "say", &e![STACKED, ".sync.by_tree = true"]), "restack onto base by tree (3 behind): its commits become one");
    assert_eq!(step("behind", "cmd", &e![STACKED, ".sync.by_tree = true"]), "sd act restack");
}

// @test "a conflicting restack hands the agent the resolution"
#[test]
fn a_conflicting_restack_hands_the_agent_the_resolution() {
    assert_eq!(step("conflict", "cmd", &e![STACKED, ".sync.conflict = true"]), "sd act restack --resolve");
    assert_eq!(step("conflict", "who", &e![STACKED, ".sync.conflict = true"]), "agent");
}

// A plain rebase there would replay a rewritten parent's old commits (#97).
// @test "behind with no old base: chrismo's, pointing at sd state's errors, no rebase"
#[test]
fn behind_with_no_old_base_chrismo_s_pointing_at_sd_state_s_errors_no_rebase() {
    assert_eq!(step("behind", "who", &e![STACKED, ".sync.old_base = null"]), "you");
    assert_eq!(step("behind", "say", &e![STACKED, ".sync.old_base = null"]), "restack onto base (3 behind): no base to restack from; see sd state's errors");
    assert_eq!(step("behind", "cmd", &e![STACKED, ".sync.old_base = null"]), "sd state --json");
}

// @test "only behind the default branch: no step; main moves too often to chase"
#[test]
fn only_behind_the_default_branch_no_step_main_moves_too_often_to_chase() {
    assert_eq!(keys(&e![".sync.up_to_date = false", ".sync.behind = 40"]), "ready");
}

// @test "behind the default branch with a conflict: the agent resolves it"
#[test]
fn behind_the_default_branch_with_a_conflict_the_agent_resolves_it() {
    assert_eq!(keys(&e![".sync.up_to_date = false", ".sync.behind = 40", ".sync.conflict = true"]), "conflict");
    assert_eq!(step("conflict", "cmd", &e![".sync.up_to_date = false", ".sync.conflict = true"]), "sd act restack --resolve");
}

// @test "parent merged, retargeted to the default branch: restack"
#[test]
fn parent_merged_retargeted_to_the_default_branch_restack() {
    assert_eq!(keys(&e![r#".parent.name = "base""#, ".parent.merged = true", ".sync.up_to_date = false", ".sync.behind = 2"]), "behind");
}

// @test "a conflicting restack is the agent's, instead of a plain restack"
#[test]
fn a_conflicting_restack_is_the_agent_s_instead_of_a_plain_restack() {
    assert_eq!(keys(&e![STACKED, ".sync.conflict = true"]), "conflict");
    assert_eq!(step("conflict", "who", &e![STACKED, ".sync.conflict = true"]), "agent");
}

// @test "red CI: the agent fixes it; names the failing checks"
#[test]
fn red_ci_the_agent_fixes_it_names_the_failing_checks() {
    assert_eq!(step("ci-red", "who", &e![r#".ci.verdict = "red""#, r#".ci.failing = ["lint", "test"]"#]), "agent");
    assert_eq!(step("ci-red", "say", &e![r#".ci.verdict = "red""#, r#".ci.failing = ["lint", "test"]"#]), "fix CI: lint, test");
    assert_eq!(step("ci-red", "cmd", &e![r#".ci.verdict = "red""#, r#".ci.failing = ["lint", "test"]"#]), "sd act ci-fix");
}

// @test "red CI with unpushed commits, or on an old head: push first, or wait; no fix step"
#[test]
fn red_ci_with_unpushed_commits_or_on_an_old_head_push_first_or_wait_no_fix_step() {
    assert_eq!(keys(&e![r#".ci.verdict = "red""#, r#".ci.failing = ["test"]"#, ".push.ahead = 1"]), "push");
    assert_eq!(keys(&e![r#".ci.verdict = "red""#, r#".ci.failing = ["test"]"#, ".ci.for_head = false"]), "ci-wait");
}

// @test "blocked CI: chrismo's, since no agent fixes a check that didn't start or wants approval"
#[test]
fn blocked_ci_chrismo_s_since_no_agent_fixes_a_check_that_didn_t_start_or_wants_approval() {
    let b = e![r#".ci.verdict = "blocked""#, r#".ci.blocked = ["wf", "deploy"]"#];
    assert_eq!(step("ci-blocked", "who", &e![&b]), "you");
    assert_eq!(step("ci-blocked", "say", &e![&b]), "CI blocked: wf, deploy didn't start or want approval; an agent can't fix that");
    assert_eq!(step("ci-blocked", "cmd", &e![&b]), "gh pr checks");
    assert_eq!(keys(&e![&b, ".pr.draft = true", r#".pr.merge_state = "DRAFT""#]), "ci-blocked");
    assert_eq!(keys(&e![&b, ".push.ahead = 1"]), "push");
}

// @test "bot items: sd act address"
#[test]
fn bot_items_sd_act_address() {
    assert_eq!(step("bot", "who", &e![".threads.owed_bot = 3"]), "agent");
    assert_eq!(step("bot", "say", &e![".threads.owed_bot = 3"]), "address 3 bot items");
    assert_eq!(step("bot", "cmd", &e![".threads.owed_bot = 3"]), "sd act address");
}

// @test "bot items that are only info: chrismo reads them, then acks"
#[test]
fn bot_items_that_are_only_info_chrismo_reads_them_then_acks() {
    assert_eq!(keys(&e![".threads.owed_bot = 2", ".threads.info = 2"]), "read");
    assert_eq!(step("read", "who", &e![".threads.owed_bot = 2", ".threads.info = 2"]), "you");
    assert_eq!(step("read", "say", &e![".threads.owed_bot = 2", ".threads.info = 2"]), "read 2 bot items in sd state --long, then ack");
    assert_eq!(step("read", "cmd", &e![".threads.owed_bot = 2", ".threads.info = 2"]), "sd act ack");
}

// @test "info bot items wait until the pipeline can't move without chrismo (#168)"
#[test]
fn info_bot_items_wait_until_the_pipeline_can_t_move_without_chrismo_168() {
    let info = e![".threads.owed_bot = 1", ".threads.info = 1"];
    // soak 4: after restack --resolve, diverged, the review behind the rewrite
    assert_eq!(keys(&e![&info, ".push.ahead = 3", ".push.behind = 2", ".push.diverged = true", ".sync.ahead = 2", r#".review.local = {sha: "x", status: "rewritten-changed"}"#]), "push review");
    assert_eq!(keys(&e![&info, r#".review.local = {sha: "x", status: "uncovered", uncovered: 1}"#]), "review");
    assert_eq!(keys(&e![&info, r#".review.local = {sha: "x", status: "uncovered", uncovered: 2}"#, r#".review.findings = {sha: "h1", round: 1, n: 2, at_head: true, declined: false, cap: 3, items: []}"#]), "review-fix");
    assert_eq!(keys(&e![&info, r#".bot = {command: "/review-bot"}"#, r#".review.bot = {sha: null, status: "none"}"#]), "bot-trigger");
    assert_eq!(keys(&e![&info, r#".bot = {command: "/review-bot", timeout: 3600}"#, r#".review.bot = {sha: "h1", status: "covered", triggered: "t", replied: false, waited: 60}"#]), "bot-wait");
    assert_eq!(keys(&e![&info, r#".ci.verdict = "inconclusive""#]), "ci-rerun");
    assert_eq!(keys(&e![&info, r#".pr.base = "p""#, r#".parent = {"name": "p", "source": "pr", "merged": true, "pr": {"number": 25, "state": "MERGED", "head_sha": "x"}}"#]), "retarget");
    // still ahead of marking the draft ready
    assert_eq!(keys(&e![&info, ".pr.draft = true", r#".pr.merge_state = "DRAFT""#]), "read draft");
}

#[test]
fn info_bot_items_wait_behind_a_restack_too_247() {
    let info = e![".threads.owed_bot = 1", ".threads.info = 1"];
    // soak 6: main moved and the restack conflicts; the resolve supersedes the item
    assert_eq!(keys(&e![&info, ".sync.up_to_date = false", ".sync.behind = 2", ".sync.conflict = true"]), "conflict");
    assert_eq!(keys(&e![&info, STACKED]), "behind");
    assert_eq!(keys(&e![&info, r#".pr.merge_state = "BEHIND""#, ".sync.up_to_date = false", ".sync.behind = 3"]), "behind");
    // a by-tree conflict is chrismo's to resolve, so nothing moves without chrismo
    assert_eq!(keys(&e![&info, STACKED, ".sync.conflict = true", ".sync.by_tree = true"]), "conflict read");
}

// @test "past settings.branchBudget: one you step in place of the agent steps; run steps still run (#121)"
#[test]
fn past_settings_branchbudget_one_you_step_in_place_of_the_agent_steps_run_steps_still_run_121() {
    let over = r#".spend = {"usd": 25.456, "budget": 25}"#;
    assert_eq!(keys(&e![over, r#".ci.verdict = "red""#, r#".ci.failing = ["test"]"#, ".threads.owed_bot = 2"]), "budget");
    assert_eq!(step("budget", "who", &e![over, ".threads.owed_bot = 2"]), "you");
    assert_eq!(step("budget", "say", &e![over, ".threads.owed_bot = 2"]), "spent $25.46 on this branch, past settings.branchBudget ($25.00): raise it, or take over");
    assert_eq!(step("budget", "cmd", &e![over, ".threads.owed_bot = 2"]), "sd act attach");
    // a restack ahead of the agent's steps, and a push after them, stay
    assert_eq!(keys(&e![over, r#".pr.merge_state = "BEHIND""#, ".sync.up_to_date = false", ".sync.behind = 3", ".threads.owed_bot = 2"]), "behind budget");
    assert_eq!(keys(&e![over, ".threads.owed_bot = 1", ".push.ahead = 1", r#".review.local.status = "uncovered""#]), "budget push");
    // nothing that spends: no budget step
    assert_eq!(keys(&e![over]), "ready");
}

// @test "past settings.branchBudget: open-pr is chrismo's too, since ship may describe (#121)"
#[test]
fn past_settings_branchbudget_open_pr_is_chrismo_s_too_since_ship_may_describe_121() {
    let nopr = e![".pr = null", r#".push = {"remote_sha": null, "ahead": 2, "behind": 0, "diverged": false, "remote_ours": null}"#];
    assert_eq!(keys(&e![&nopr]), "push open-pr");
    assert_eq!(keys(&e![r#".spend = {"usd": 30, "budget": 25}"#, &nopr]), "push budget");
    // the parent's ship spends on the parent, against its own budget
    assert_eq!(keys(&e![r#".spend = {"usd": 30, "budget": 25}"#, &nopr, r#".parent = {"name": "p", "source": "config", "merged": false, "local": true, "on_remote": false, "unpushed": true, "pr": null}"#, r#".sync = {"target": "p", "default": "main", "behind": null, "up_to_date": null, "conflict": null, "old_base": null}"#]), "ship-parent");
}

#[test]
fn three_agent_steps_failed_in_a_row_one_you_step_in_place_of_the_agent_steps_run_steps_still_run_242() {
    let failed = |n: u32, s: &str| format!(r#".failed = {{"step": "{s}", "harness": "pi", "count": {n}, "since": "T0", "at": "T1", "error": "no quota"}}"#);
    let three = failed(3, "address");
    assert_eq!(keys(&e![&three, r#".ci.verdict = "red""#, r#".ci.failing = ["test"]"#, ".threads.owed_bot = 2"]), "failed");
    assert_eq!(step("failed", "who", &e![&three, ".threads.owed_bot = 2"]), "you");
    assert_eq!(
        step("failed", "say", &e![&three, ".threads.owed_bot = 2"]),
        "3 agent steps failed in a row, the last pi address at T1: no quota; fix what fails, then rerun it"
    );
    assert_eq!(step("failed", "cmd", &e![&three, ".threads.owed_bot = 2"]), "sd act address");
    assert_eq!(step("failed", "cmd", &e![&failed(3, "resolve"), ".threads.owed_bot = 2"]), "sd act restack --resolve");
    // a step nothing reruns: clearing the record is the way on
    assert_eq!(step("failed", "cmd", &e![&failed(3, "agent"), ".threads.owed_bot = 2"]), "git update-ref -d refs/sheepdog/feat/failed");
    // a restack ahead of the agent's steps, and a push after them, stay
    assert_eq!(keys(&e![&three, r#".pr.merge_state = "BEHIND""#, ".sync.up_to_date = false", ".sync.behind = 3", ".threads.owed_bot = 2"]), "behind failed");
    assert_eq!(keys(&e![&three, ".threads.owed_bot = 1", ".push.ahead = 1", r#".review.local.status = "uncovered""#]), "failed push");
    // nothing that spends: no failed step
    assert_eq!(keys(&e![&three]), "ready");
    // fewer in a row: the loop retries
    assert_eq!(keys(&e![&failed(2, "address"), ".threads.owed_bot = 2"]), "bot");
    // past the budget too: the budget step stands in
    assert_eq!(keys(&e![&three, r#".spend = {"usd": 30, "budget": 25}"#, ".threads.owed_bot = 2"]), "budget");
}

// @test "spend unknown (usage.sup unreadable) with a budget: held as if over it (#121)"
#[test]
fn spend_unknown_usage_sup_unreadable_with_a_budget_held_as_if_over_it_121() {
    let unknown = r#".spend = {"usd": null, "budget": 25}"#;
    assert_eq!(keys(&e![unknown, ".threads.owed_bot = 2"]), "budget");
    assert_eq!(step("budget", "say", &e![unknown, ".threads.owed_bot = 2"]), "can't tell what this branch has spent (sd state's errors say why), so nothing spends: fix the usage record, or take over");
    // no budget, nothing to hold
    assert_eq!(keys(&e![r#".spend = {"usd": null, "budget": 0}"#, ".threads.owed_bot = 2"]), "bot");
}

// @test "within settings.branchBudget, or with none (0): the agent steps as usual"
#[test]
fn within_settings_branchbudget_or_with_none_0_the_agent_steps_as_usual() {
    assert_eq!(keys(&e![r#".spend = {"usd": 24.9, "budget": 25}"#, ".threads.owed_bot = 2"]), "bot");
    assert_eq!(keys(&e![r#".spend = {"usd": 99, "budget": 0}"#, ".threads.owed_bot = 2"]), "bot");
    assert_eq!(keys(&e![".spend = null", ".threads.owed_bot = 2"]), "bot");
}

// @test "blocking and info bot items together: address takes them all"
#[test]
fn blocking_and_info_bot_items_together_address_takes_them_all() {
    assert_eq!(keys(&e![".threads.owed_bot = 3", ".threads.info = 1"]), "bot");
    assert_eq!(step("bot", "say", &e![".threads.owed_bot = 3", ".threads.info = 1"]), "address 3 bot items");
}

// @test "unpushed commits: sd act push"
#[test]
fn unpushed_commits_sd_act_push() {
    assert_eq!(step("push", "say", &e![".push.ahead = 2"]), "push 2 commits");
    assert_eq!(step("push", "cmd", &e![".push.ahead = 2"]), "sd act push");
}

// @test "an agent's commit changed CI config: chrismo reviews, then pushes; no push step, no ci-fix (#124)"
#[test]
fn an_agent_s_commit_changed_ci_config_chrismo_reviews_then_pushes_no_push_step_no_ci_fix_124() {
    let held = r#".push.held = {"sha": "h2", "paths": [".github/workflows/test.yml", ".rwx/tests.yml"]}"#;
    assert_eq!(keys(&e![".push.ahead = 1", held]), "ci-config");
    assert_eq!(step("ci-config", "who", &e![".push.ahead = 1", held]), "you");
    assert_eq!(step("ci-config", "say", &e![".push.ahead = 1", held]), "the agent changed CI config (.github/workflows/test.yml, .rwx/tests.yml): review, then sd act push");
    assert_eq!(step("ci-config", "cmd", &e![".push.ahead = 1", held]), "sd act push");
    assert_eq!(keys(&e![".push.ahead = 1", r#".ci.verdict = "red""#, r#".ci.failing = ["test"]"#, held]), "ci-config");
    assert_eq!(keys(&e![".pr = null", r#".push = {"remote_sha": null, "ahead": 1, "behind": 0}"#, held]), "ci-config"); // ship would refuse
    assert_eq!(keys(&e![".push.ahead = 1", ".push.held = null"]), "push");
}

// @test "CI config held where a push would be wrong (undone restack, foreign, parent unpushed): still shown, no sd act push offered (#124)"
#[test]
fn ci_config_held_where_a_push_would_be_wrong_undone_restack_foreign_parent_unpushed_still_shown_no_sd_act_push_offered_124() {
    let held = r#".push.held = {"sha": "h2", "paths": [".rwx/tests.yml"]}"#;
    assert_eq!(keys(&e![".sync.undone = 1", ".sync.up_to_date = false", ".push.ahead = 1", held]), "restack-undone ci-config");
    assert_eq!(step("ci-config", "cmd", &e![".sync.undone = 1", ".sync.up_to_date = false", ".push.ahead = 1", held]), "null");
    assert_eq!(step("ci-config", "cmd", &e![".push.remote_ours = false", ".push.behind = 1", ".push.theirs = 1", ".push.ahead = 1", held]), "null");
    assert_eq!(step("ci-config", "cmd", &e![".parent.unpushed = true", ".push.ahead = 1", held]), "null");
    assert_eq!(step("ci-config", "say", &e![".sync.undone = 1", held]), "the agent changed CI config (.rwx/tests.yml): review it; push once the steps above are done");
}

// @test "a restack undone from outside sheepdog: redo it by hand, then sd act push (#47)"
#[test]
fn a_restack_undone_from_outside_sheepdog_redo_it_by_hand_then_sd_act_push_47() {
    assert_eq!(keys(&e![".sync.undone = 1", ".sync.up_to_date = false", ".push.ahead = 1", ".push.behind = 0"]), "restack-undone");
    assert_eq!(step("restack-undone", "who", &e![".sync.undone = 1"]), "you");
    assert_eq!(step("restack-undone", "say", &e![".sync.undone = 1"]), "the restack was undone (1 of main's commits is a copy on feat; a plain git push after a restack?): redo it, and don't sd act push before that, which would publish the copies");
    assert_eq!(step("restack-undone", "say", &e![".sync.undone = 2"]), "the restack was undone (2 of main's commits are copies on feat; a plain git push after a restack?): redo it, and don't sd act push before that, which would publish the copies");
    assert_eq!(step("restack-undone", "cmd", &e![".sync.undone = 1", ".sync.old_base = null"]), "git rebase origin/main && sd act push");
    assert_eq!(step("restack-undone", "cmd", &e![".sync.undone = 1", r#".sync.old_base = "0123456789abcdef0123""#]), "git rebase --onto origin/main 0123456789ab && sd act push");
    assert_eq!(keys(&e![".sync.undone = 0"]), "ready");
    assert_eq!(keys(&e![".sync.undone = 1", r#".pr.merge_state = "BEHIND""#]), "restack-undone");
}

// On a stacked parent a plain rebase would replay a rewritten parent's old commits (#97).
// @test "a restack undone on a stacked branch with no old base: no rebase offered, pin by hand (#97)"
#[test]
fn a_restack_undone_on_a_stacked_branch_with_no_old_base_no_rebase_offered_pin_by_hand_97() {
    let s = e![".sync.undone = 1", r#".sync.target = "base""#, r#".parent.name = "base""#, ".sync.old_base = null"];
    assert_eq!(step("restack-undone", "who", &e![&s]), "you");
    assert!(step("restack-undone", "say", &e![&s]).ends_with("no base to redo it from on base: pin one by hand (git update-ref refs/sheepdog/feat/base <the commit feat left base at>), then sd act restack"));
    assert_eq!(step("restack-undone", "cmd", &e![&s]), "null");
    let m = e![".sync.undone = 1", r#".parent.name = "base""#, ".parent.merged = true", ".sync.old_base = null"];
    assert_eq!(step("restack-undone", "cmd", &e![&m]), "null");
    assert!(step("restack-undone", "say", &e![&m]).ends_with("no base to redo it from on main: pin one by hand (git update-ref refs/sheepdog/feat/base <the commit feat left base at>), then sd act restack"));
}

// @test "the remote has commits this copy reset away (ours): force-push, which leases (#86)"
#[test]
fn the_remote_has_commits_this_copy_reset_away_ours_force_push_which_leases_86() {
    assert_eq!(step("push", "say", &e![".push.ahead = 0", ".push.behind = 1", ".push.remote_ours = true", ".push.remote_made = true"]), "force-push: drop 1 commit the remote has, reset away here");
    assert_eq!(step("push", "cmd", &e![".push.ahead = 0", ".push.behind = 2", ".push.remote_ours = true", ".push.remote_made = true"]), "sd act push");
    assert_eq!(keys(&e![".push.ahead = 0", ".push.behind = 1", ".push.remote_ours = true", ".push.remote_made = null"]), "ready");
}

// @test "commits taken from the remote and reset away: take them back, no drop (#86)"
#[test]
fn commits_taken_from_the_remote_and_reset_away_take_them_back_no_drop_86() {
    let st = e![".push.ahead = 0", ".push.behind = 1", ".push.remote_ours = true", ".push.remote_made = false"];
    assert_eq!(keys(&e![&st]), "take");
    assert_eq!(step("take", "who", &e![&st]), "you");
    assert_eq!(step("take", "say", &e![&st]), "origin/feat has 1 commit this branch took but didn't make, reset away here: take it back (git merge --ff-only origin/feat), or drop it by hand");
    assert_eq!(step("take", "say", &e![&st, ".push.behind = 2"]), "origin/feat has 2 commits this branch took but didn't make, reset away here: take them back (git merge --ff-only origin/feat), or drop them by hand");
    assert_eq!(keys(&e![&st, ".pr.draft = true", r#".pr.merge_state = "DRAFT""#]), "take");
}

// @test "the same reset with changes still in the worktree: chrismo commits or drops them, no push step (#86)"
#[test]
fn the_same_reset_with_changes_still_in_the_worktree_chrismo_commits_or_drops_them_no_push_step_86() {
    assert_eq!(keys(&e![".push.ahead = 0", ".push.behind = 1", ".push.remote_ours = true", ".push.remote_made = true", ".push.dirty = true"]), "reset-dirty");
    assert_eq!(step("reset-dirty", "who", &e![".push.ahead = 0", ".push.behind = 1", ".push.remote_ours = true", ".push.remote_made = true", ".push.dirty = true"]), "you");
    assert_eq!(step("reset-dirty", "say", &e![".push.ahead = 0", ".push.behind = 1", ".push.remote_ours = true", ".push.remote_made = true", ".push.dirty = true"]), "origin/feat has 1 commit reset away here, and the worktree has uncommitted changes: commit them (then sd act push replaces the remote commit), or remove them (git reset --hard; git clean for untracked files)");
    assert!(step("reset-dirty", "say", &e![".push.ahead = 0", ".push.behind = 2", ".push.remote_ours = true", ".push.remote_made = true", ".push.dirty = true"]).starts_with("origin/feat has 2 commits reset away here, and the worktree has uncommitted changes: commit them (then sd act push replaces the remote commits),"));
    assert_eq!(keys(&e![".push.ahead = 0", ".push.behind = 1", ".push.remote_ours = true", ".push.remote_made = true", ".push.dirty = false"]), "push");
    assert_eq!(keys(&e![".push.ahead = 0", ".push.behind = 1", ".push.remote_ours = true", ".push.remote_made = true", ".push.dirty = null"]), "push");
}

// @test "take or reset-dirty holds the pipeline: no bot trigger or CI wait after it, since nothing reaches GitHub until chrismo acts (#86)"
#[test]
fn take_or_reset_dirty_holds_the_pipeline_no_bot_trigger_or_ci_wait_after_it_since_nothing_reaches_github_until_chrismo_acts_86() {
    let remote_ahead = e![".push.ahead = 0", ".push.behind = 1", ".push.remote_ours = true", r#".ci.sha = "r1""#, ".ci.for_head = false", r#".bot = {command: "/review-bot"}"#];
    assert_eq!(keys(&e![&remote_ahead, ".push.remote_made = false"]), "take");
    assert_eq!(keys(&e![&remote_ahead, ".push.remote_made = true", ".push.dirty = true"]), "reset-dirty");
    assert_eq!(keys(&e![&remote_ahead, ".push.remote_made = true", ".push.dirty = false"]), "push");
}

// @test "rewritten history of our own: sd act push, which leases"
#[test]
fn rewritten_history_of_our_own_sd_act_push_which_leases() {
    assert_eq!(step("push", "say", &e![".push.ahead = 2", ".push.behind = 2", ".push.diverged = true", ".sync.ahead = 2"]), "force-push 2 commits (rewritten); a plain git push is rejected, and pulling would undo the rewrite");
    assert_eq!(step("push", "cmd", &e![".push.ahead = 2", ".push.behind = 2", ".push.diverged = true"]), "sd act push");
}

// @test "rewritten after a restack: count the branch's own commits, not what came in from the target"
#[test]
fn rewritten_after_a_restack_count_the_branch_s_own_commits_not_what_came_in_from_the_target() {
    // After a restack onto a squash-merged parent, remote..HEAD also counts the
    // squash commit (#31). sync.ahead is <target>..HEAD: just the branch's own.
    assert_eq!(step("push", "say", &e![".push.ahead = 3", ".push.behind = 2", ".push.diverged = true", ".sync.ahead = 2"]), "force-push 2 commits (rewritten); a plain git push is rejected, and pulling would undo the rewrite");
    assert_eq!(step("push", "say", &e![".push.ahead = 3", ".push.behind = 2", ".push.diverged = true", ".sync.ahead = null"]), "force-push 3 commits (rewritten); a plain git push is rejected, and pulling would undo the rewrite");
}

// @test "never pushed: sd act push"
#[test]
fn never_pushed_sd_act_push() {
    assert_eq!(step("push", "say", &e![".push.remote_sha = null", ".push.ahead = 1", ".pr = null"]), "push: not on origin yet");
    assert_eq!(step("push", "cmd", &e![".push.remote_sha = null", ".push.ahead = 1", ".pr = null"]), "sd act push");
}

// @test "no PR: push first, then open one"
#[test]
fn no_pr_push_first_then_open_one() {
    assert_eq!(keys(&e![".pr = null", ".push.remote_sha = null", ".push.ahead = 1"]), "push open-pr");
    assert_eq!(step("open-pr", "cmd", &e![".pr = null"]), "sd act ship");
}

// @test "no PR because gh failed: don't suggest opening one"
#[test]
fn no_pr_because_gh_failed_don_t_suggest_opening_one() {
    assert_eq!(keys(&e![".pr = null", ".threads.blocking = null", r#".errors = ["gh: boom"]"#]), "errors");
}

// @test "draft: ready for review once CI is green and the review covers HEAD, and only then"
#[test]
fn draft_ready_for_review_once_ci_is_green_and_the_review_covers_head_and_only_then() {
    assert_eq!(keys(&e![".pr.draft = true", r#".pr.merge_state = "DRAFT""#]), "draft");
    assert_eq!(step("draft", "who", &e![".pr.draft = true"]), "run");
    assert_eq!(step("draft", "cmd", &e![".pr.draft = true"]), "gh pr ready");
    assert_eq!(step("draft", "say", &e![".pr.draft = true"]), "CI green and reviewed: mark the PR ready for review");
    assert_eq!(keys(&e![".pr.draft = true", r#".ci.verdict = "none""#]), "draft");
    assert_eq!(keys(&e![".pr.draft = true", r#".ci.verdict = "none""#, ".ci.required = null"]), "");
    assert_eq!(keys(&e![".pr.draft = true", r#".ci.verdict = "none""#, ".ci.required = []"]), "draft");
    assert_eq!(step("draft", "say", &e![".pr.draft = true", r#".ci.verdict = "none""#]), "reviewed, no CI checks: mark the PR ready for review");
    assert_eq!(keys(&e![".pr.draft = true", r#".pr.merge_state = "DRAFT""#, r#".ci.verdict = "pending""#]), "ci-wait");
    assert_eq!(keys(&e![".pr.draft = true", r#".pr.merge_state = "DRAFT""#, r#".review.local = {sha: "x", status: "uncovered", uncovered: 1}"#]), "review");
    assert_eq!(keys(&e![".pr.draft = true", r#".pr.merge_state = "DRAFT""#, ".push.ahead = 1"]), "push");
    assert_eq!(keys(&e![".pr.draft = true", r#".pr.merge_state = "DRAFT""#, ".push.behind = 1", ".push.remote_ours = true", ".push.remote_made = true"]), "push");
}

// @test "no PR: ship opens one"
#[test]
fn no_pr_ship_opens_one() {
    assert_eq!(step("open-pr", "cmd", &e![".pr = null", ".push.remote_sha = null"]), "sd act ship");
    assert_eq!(step("open-pr", "who", &e![".pr = null", ".push.remote_sha = null"]), "run");
}

// @test "nothing ahead of the base, with a task: implement, not push or ship (#143)"
#[test]
fn nothing_ahead_of_the_base_with_a_task_implement_not_push_or_ship_143() {
    assert_eq!(keys(&e![FRESH, r#".task = "add c""#]), "implement");
    assert_eq!(step("implement", "who", &e![FRESH, r#".task = "add c""#]), "agent");
    assert_eq!(step("implement", "say", &e![FRESH, r#".task = "add c""#]), "implement the task: nothing ahead of main yet");
    assert_eq!(step("implement", "cmd", &e![FRESH, r#".task = "add c""#]), "sd act implement");
    // already pushed, identical to its base (soak 3): still implement, not ship
    assert_eq!(keys(&e![".pr = null", ".sync.ahead = 0", r#".task = "add c""#]), "implement");
    // past settings.branchBudget, it's chrismo's like every agent step
    assert_eq!(keys(&e![FRESH, r#".task = "add c""#, r#".spend = {"usd": 30, "budget": 25}"#]), "budget");
}

// @test "nothing ahead of the base, no task: chrismo commits or records one (#143)"
#[test]
fn nothing_ahead_of_the_base_no_task_chrismo_commits_or_records_one_143() {
    assert_eq!(keys(&e![FRESH]), "nothing");
    assert_eq!(keys(&e![FRESH, ".task = null"]), "nothing");
    assert_eq!(step("nothing", "who", &e![FRESH]), "you");
    assert_eq!(step("nothing", "say", &e![FRESH]), "nothing to ship: commit, or record a task");
    assert_eq!(step("nothing", "cmd", &e![FRESH]), "null");
}

// @test "nothing ahead, uncommitted changes: chrismo commits them; implement would refuse the worktree (#143)"
#[test]
fn nothing_ahead_uncommitted_changes_chrismo_commits_them_implement_would_refuse_the_worktree_143() {
    // its own key: sd act start's implement looks the same while it edits, so sd watch doesn't notify it
    assert_eq!(keys(&e![FRESH, r#".task = "add c""#, ".push.dirty = true"]), "nothing-dirty");
    assert_eq!(step("nothing-dirty", "who", &e![FRESH, r#".task = "add c""#, ".push.dirty = true"]), "you");
    assert_eq!(step("nothing-dirty", "say", &e![FRESH, r#".task = "add c""#, ".push.dirty = true"]), "nothing to ship, and the worktree has uncommitted changes: commit them, or remove them and sd act implement");
    assert_eq!(step("nothing-dirty", "say", &e![FRESH, ".push.dirty = true"]), "nothing to ship, and the worktree has uncommitted changes: commit them");
}

// @test "nothing ahead, but the remote has commits reset away here: the #86 steps, not implement or nothing (#143)"
#[test]
fn nothing_ahead_but_the_remote_has_commits_reset_away_here_the_86_steps_not_implement_or_nothing_143() {
    let gone = e![FRESH, r#".task = "add c""#, r#".push.remote_sha = "r1""#, ".push.behind = 1", ".push.remote_ours = true"];
    assert_eq!(keys(&e![&gone, ".push.remote_made = true"]), "push open-pr");
    assert_eq!(step("push", "say", &e![&gone, ".push.remote_made = true"]), "force-push: drop 1 commit the remote has, reset away here");
    assert_eq!(keys(&e![&gone, ".push.remote_made = true", ".push.dirty = true"]), "reset-dirty open-pr");
    assert_eq!(keys(&e![&gone, ".push.remote_made = false"]), "take open-pr");
    assert_eq!(keys(&e![&gone, ".push.remote_ours = false", ".push.theirs = 1"]), "foreign open-pr");
}

// @test "nothing ahead, with errors: no implement, which spends; the errors step leads (#143)"
#[test]
fn nothing_ahead_with_errors_no_implement_which_spends_the_errors_step_leads_143() {
    assert_eq!(keys(&e![FRESH, r#".task = "add c""#, r#".errors = ["gh: boom"]"#]), "errors");
}

// @test "commits ahead, or ahead unknown: push and ship, task or not (#143)"
#[test]
fn commits_ahead_or_ahead_unknown_push_and_ship_task_or_not_143() {
    assert_eq!(keys(&e![".pr = null", ".push.remote_sha = null", ".push.ahead = 1", r#".task = "add c""#]), "push open-pr");
    assert_eq!(keys(&e![FRESH, ".sync.ahead = null", r#".task = "add c""#]), "push open-pr");
}

// @test "on the default branch: one step, chrismo starts a branch (#159)"
#[test]
fn on_the_default_branch_one_step_chrismo_starts_a_branch_159() {
    let main = e![r#".branch = "main""#, FRESH];
    assert_eq!(keys(&e![&main]), "default");
    assert_eq!(step("default", "who", &e![&main]), "you");
    assert_eq!(step("default", "say", &e![&main]), "on main: start a branch");
    assert_eq!(step("default", "cmd", &e![&main]), "sd act start");
    // commits on it, a task, or errors change nothing
    assert_eq!(keys(&e![&main, ".sync.ahead = 2", ".push.ahead = 2", r#".task = "add c""#]), "default");
    assert_eq!(keys(&e![&main, r#".errors = ["boom"]"#]), "default");
    // a branch that is its own base, too
    assert_eq!(keys(&e![FRESH, r#".sync.target = "feat""#]), "default");
    assert_eq!(step("default", "say", &e![FRESH, r#".sync.target = "feat""#]), "on feat: start a branch");
}

// @test "review wording: a first review covers the branch's change (#145)"
#[test]
fn review_wording_a_first_review_covers_the_branch_s_change_145() {
    assert_eq!(step("review", "say", &e![r#".review.local = {sha: null, status: "none"}"#]), "review the branch's change");
}

// @test "review wording: a later review counts the commits since the last one (#145)"
#[test]
fn review_wording_a_later_review_counts_the_commits_since_the_last_one_145() {
    assert_eq!(step("review", "say", &e![r#".review.local = {sha: "x", status: "uncovered", uncovered: 2}"#]), "review again: 2 commits since the last review");
    assert_eq!(step("review", "say", &e![r#".review.local = {sha: "x", status: "uncovered", uncovered: 1}"#]), "review again: 1 commit since the last review");
    // the latest of several reviews is the last one
    assert_eq!(step("review", "say", &e![r#".review.local = {sha: "x", status: "uncovered", uncovered: 3}"#, r#".review.bot = {sha: "y", status: "uncovered", uncovered: 1}"#]), "review again: 1 commit since the last review");
    assert_eq!(step("review", "say", &e![r#".review.local = {sha: "x", status: "rewritten-changed"}"#]), "review again: the branch was rewritten since the last review");
}

// @test "review behind HEAD: local review by the agent"
#[test]
fn review_behind_head_local_review_by_the_agent() {
    assert_eq!(step("review", "who", &e![r#".review.local = {sha: "x", status: "uncovered", uncovered: 2}"#]), "agent");
    assert_eq!(step("review", "cmd", &e![r#".review.local = {sha: "x", status: "uncovered", uncovered: 2}"#]), "sd act review");
    assert_eq!(keys(&e![r#".review.local = {sha: "x", status: "rewritten-changed"}"#]), "review");
    assert_eq!(keys(&e![r#".review.local = {sha: "x", status: "unknown"}"#]), "review");
}

// @test "review findings at HEAD: the agent fixes them, not another review"
#[test]
fn review_findings_at_head_the_agent_fixes_them_not_another_review() {
    let st = e![r#".review.local = {sha: "x", status: "uncovered", uncovered: 2}"#, r#".review.findings = {sha: "h1", round: 1, n: 2, at_head: true, declined: false, cap: 3, items: []}"#];
    assert_eq!(keys(&e![&st]), "review-fix");
    assert_eq!(step("review-fix", "who", &e![&st]), "agent");
    assert_eq!(step("review-fix", "cmd", &e![&st]), "sd act implement");
    assert_eq!(step("review-fix", "say", &e![&st]), "fix 2 review findings (round 1/3)");
    // findings on an older head: the new commits get reviewed
    assert_eq!(keys(&e![&st, ".review.findings.at_head = false"]), "review");
}

// @test "review findings at the cap, or declined by the agent: chrismo fixes or accepts"
#[test]
fn review_findings_at_the_cap_or_declined_by_the_agent_chrismo_fixes_or_accepts() {
    let st = e![r#".review.local = {sha: "x", status: "uncovered", uncovered: 2}"#, r#".review.findings = {sha: "h1", round: 3, n: 1, at_head: true, declined: false, cap: 3, items: []}"#];
    assert_eq!(keys(&e![&st]), "review-decide");
    assert_eq!(step("review-decide", "who", &e![&st]), "you");
    assert_eq!(step("review-decide", "cmd", &e![&st]), "sd act review --accept");
    assert_eq!(step("review-decide", "say", &e![&st]), "1 review finding after 3 rounds: fix it, or accept HEAD as reviewed (sd state --long lists them)");
    assert_eq!(keys(&e![&st, ".review.findings.round = 1", ".review.findings.declined = true"]), "review-decide");
    assert_eq!(step("review-decide", "say", &e![&st, ".review.findings.round = 1", ".review.findings.declined = true"]), "the agent declined 1 review finding: fix it, or accept HEAD as reviewed (sd state --long lists them)");
}

// @test "review findings on an older head, before any PR or pin: the next review is due"
#[test]
fn review_findings_on_an_older_head_before_any_pr_or_pin_the_next_review_is_due() {
    assert_eq!(keys(&e![".pr = null", r#".review.local = {sha: null, status: "none"}"#, r#".review.findings = {sha: "x", round: 1, n: 1, at_head: false, declined: false, cap: 3, items: []}"#]), "open-pr review");
}

// @test "review findings an accepted pin covers: no review step"
#[test]
fn review_findings_an_accepted_pin_covers_no_review_step() {
    assert_eq!(keys(&e![r#".review.local = {sha: "x", status: "uncovered", uncovered: 2}"#, r#".review.findings = {sha: "h1", round: 3, n: 1, at_head: true, declined: false, cap: 3, items: []}"#, r#".review.accepted = {sha: "h1", status: "covered"}"#]), "ready");
}

// @test "bot configured and HEAD pushed and locally reviewed but not bot-reviewed: trigger it"
#[test]
fn bot_configured_and_head_pushed_and_locally_reviewed_but_not_bot_reviewed_trigger_it() {
    let st = e![r#".bot = {command: "/review-bot"}"#, r#".review.bot = {sha: null, status: "none"}"#];
    assert_eq!(keys(&e![&st]), "bot-trigger");
    assert_eq!(step("bot-trigger", "who", &e![&st]), "run");
    assert_eq!(step("bot-trigger", "cmd", &e![&st]), "sd act trigger");
    assert_eq!(step("bot-trigger", "say", &e![&st]), "trigger the review bot (/review-bot)");
    // unpushed, or the local review behind: those first, no trigger yet
    assert_eq!(keys(&e![&st, ".push.ahead = 1"]), "push");
    assert_eq!(keys(&e![&st, r#".review.local = {sha: "x", status: "uncovered", uncovered: 1}"#]), "review");
    // the bot already covers HEAD, or no bot configured: no step
    assert_eq!(keys(&e![&st, r#".review.bot = {sha: "h1", status: "covered"}"#]), "ready");
    assert_eq!(keys(&e![r#".review.bot = {sha: null, status: "none"}"#]), "ready");
    // a draft waits for the bot too
    assert_eq!(keys(&e![&st, ".pr.draft = true", r#".pr.merge_state = "DRAFT""#]), "bot-trigger");
}

// @test "HEAD accepted, the bot's round a commit behind: trigger it; an accept isn't the bot's review (#181)"
#[test]
fn head_accepted_the_bot_s_round_a_commit_behind_trigger_it_an_accept_isn_t_the_bot_s_review_181() {
    let st = e![r#".bot = {command: "/review-bot", timeout: 3600}"#, r#".review.local = {sha: "x", status: "uncovered", uncovered: 1}"#, r#".review.bot = {sha: "x", status: "uncovered", uncovered: 1, triggered: "t", replied: true, waited: 60}"#, r#".review.accepted = {sha: "h1", status: "covered"}"#];
    assert_eq!(keys(&e![&st]), "bot-trigger");
    assert_eq!(keys(&e![&st, ".pr.draft = true", r#".pr.merge_state = "DRAFT""#]), "bot-trigger");
    // triggered on the accepted HEAD: wait on its reply, as without the accept
    assert_eq!(keys(&e![&st, r#".review.bot = {sha: "h1", status: "covered", triggered: "t", replied: false, waited: 60}"#]), "bot-wait");
    assert_eq!(keys(&e![&st, r#".review.bot = {sha: "h1", status: "covered", triggered: "t", replied: false, waited: 60}"#, ".pr.draft = true", r#".pr.merge_state = "DRAFT""#]), "bot-wait");
    assert_eq!(keys(&e![&st, r#".review.bot = {sha: "h1", status: "covered", triggered: "t", replied: true, waited: 60}"#]), "ready");
}

// @test "bot triggered, no reply yet: wait on the bot; not ready, not draft-ready"
#[test]
fn bot_triggered_no_reply_yet_wait_on_the_bot_not_ready_not_draft_ready() {
    let st = e![r#".bot = {command: "/review-bot", timeout: 3600}"#, r#".review.bot = {sha: "h1", status: "covered", triggered: "t", replied: false, waited: 60}"#];
    assert_eq!(keys(&e![&st]), "bot-wait");
    assert_eq!(step("bot-wait", "who", &e![&st]), "wait");
    assert_eq!(step("bot-wait", "say", &e![&st]), "waiting on the review bot (/review-bot)");
    assert_eq!(step("bot-wait", "cmd", &e![&st]), "null");
    assert_eq!(keys(&e![&st, ".pr.draft = true", r#".pr.merge_state = "DRAFT""#]), "bot-wait");
}

// @test "bot waiting and CI cancelled: the rerun (a run step) comes before the wait"
#[test]
fn bot_waiting_and_ci_cancelled_the_rerun_a_run_step_comes_before_the_wait() {
    assert_eq!(keys(&e![r#".bot = {command: "/review-bot", timeout: 3600}"#, r#".review.bot = {sha: "h1", status: "covered", triggered: "t", replied: false, waited: 60}"#, r#".ci.verdict = "inconclusive""#]), "ci-rerun bot-wait");
}

// @test "bot replied since the trigger: ready"
#[test]
fn bot_replied_since_the_trigger_ready() {
    assert_eq!(keys(&e![r#".bot = {command: "/review-bot", timeout: 3600}"#, r#".review.bot = {sha: "h1", status: "covered", triggered: "t", replied: true, waited: 60}"#]), "ready");
}

// @test "bot silent past settings.botTimeout: chrismo looks, or triggers again"
#[test]
fn bot_silent_past_settings_bottimeout_chrismo_looks_or_triggers_again() {
    let st = e![r#".bot = {command: "/review-bot", timeout: 3600}"#, r#".review.bot = {sha: "h1", status: "covered", triggered: "t", replied: false, waited: 4000}"#];
    assert_eq!(keys(&e![&st]), "bot-silent");
    assert_eq!(step("bot-silent", "who", &e![&st]), "you");
    assert_eq!(step("bot-silent", "say", &e![&st]), "the review bot hasn't answered in 66 min: check it, trigger it again, or accept HEAD as reviewed (sd act review --accept)");
    assert_eq!(step("bot-silent", "cmd", &e![&st]), "sd act trigger --force");
}

// @test "bot silent, but HEAD accepted after the trigger: no wait on the bot; ready"
#[test]
fn bot_silent_but_head_accepted_after_the_trigger_no_wait_on_the_bot_ready() {
    assert_eq!(keys(&e![r#".bot = {command: "/review-bot", timeout: 3600}"#, r#".review.bot = {sha: "h1", status: "covered", triggered: "2026-10-15T12:00:00Z", replied: false, waited: 4000}"#, r#".review.accepted = {sha: "h1", status: "covered", after: "2026-10-15T12:00:00Z"}"#]), "ready");
}

// @test "accepted, then triggered, then the bot silent: chrismo decides; the accept isn't for the bot (#181)"
#[test]
fn accepted_then_triggered_then_the_bot_silent_chrismo_decides_the_accept_isn_t_for_the_bot_181() {
    let st = e![r#".bot = {command: "/review-bot", timeout: 3600}"#, r#".review.bot = {sha: "h1", status: "covered", triggered: "2026-10-15T12:00:00Z", replied: false, waited: 4000}"#];
    // the accept followed an earlier trigger, or none
    assert_eq!(keys(&e![&st, r#".review.accepted = {sha: "h1", status: "covered", after: "2026-10-15T11:00:00Z"}"#]), "bot-silent");
    assert_eq!(keys(&e![&st, r#".review.accepted = {sha: "h1", status: "covered", after: null}"#]), "bot-silent");
    // trigger times are matched, never ordered: they come from different clocks
    assert_eq!(keys(&e![&st, r#".review.accepted = {sha: "h1", status: "covered", after: "2026-10-15T12:00:05Z"}"#]), "bot-silent");
    assert_eq!(keys(&e![&st, r#".review.accepted = {sha: "h1", status: "covered"}"#]), "bot-silent");
}

// @test "a bot pin without a trigger time counts as reviewed, as before"
#[test]
fn a_bot_pin_without_a_trigger_time_counts_as_reviewed_as_before() {
    assert_eq!(keys(&e![r#".bot = {command: "/review-bot", timeout: 3600}"#, r#".review.bot = {sha: "h1", status: "covered", triggered: null, replied: null, waited: null}"#]), "ready");
}

// @test "review: a pin that covers HEAD means no review step"
#[test]
fn review_a_pin_that_covers_head_means_no_review_step() {
    assert_eq!(keys(&e![r#".review.local = {sha: "x", status: "uncovered", uncovered: 2}"#, r#".review.accepted = {sha: "h1", status: "covered"}"#]), "ready");
    assert_eq!(keys(&e![r#".review.local = {sha: "x", status: "rewritten-same"}"#]), "ready");
    assert_eq!(keys(&e![r#".review.bot = {sha: "x", status: "uncovered", uncovered: 2}"#]), "ready");
}

// @test "never reviewed: an open PR gets the review step, drafts too"
#[test]
fn never_reviewed_an_open_pr_gets_the_review_step_drafts_too() {
    assert_eq!(keys(&e![UNREVIEWED]), "review");
    assert_eq!(step("review", "cmd", &e![UNREVIEWED]), "sd act review");
    assert_eq!(keys(&e![UNREVIEWED, ".pr.draft = true", r#".pr.merge_state = "DRAFT""#]), "review");
}

// @test "never reviewed with no PR yet: ship first, no review step"
#[test]
fn never_reviewed_with_no_pr_yet_ship_first_no_review_step() {
    assert_eq!(keys(&e![UNREVIEWED, ".pr = null"]), "open-pr");
}

// @test "never reviewed, but accepted or bot coverage counts"
#[test]
fn never_reviewed_but_accepted_or_bot_coverage_counts() {
    assert_eq!(keys(&e![UNREVIEWED, r#".review.accepted = {sha: "h1", status: "covered"}"#]), "ready");
    assert_eq!(keys(&e![UNREVIEWED, r#".review.bot = {sha: "h1", status: "covered"}"#]), "ready");
}

// @test "unreviewed stacked PR: review before waiting on the parent"
#[test]
fn unreviewed_stacked_pr_review_before_waiting_on_the_parent() {
    assert_eq!(keys(&e![UNREVIEWED, ON_OPEN_PARENT]), "review parent-wait");
    // no bot trigger until the local review covers HEAD
    assert_eq!(keys(&e![UNREVIEWED, ON_OPEN_PARENT, r#".bot = {command: "/review-bot"}"#]), "review parent-wait");
}

// @test "stacked, locally reviewed, bot configured but not run: trigger it before waiting on the parent"
#[test]
fn stacked_locally_reviewed_bot_configured_but_not_run_trigger_it_before_waiting_on_the_parent() {
    assert_eq!(keys(&e![ON_OPEN_PARENT, r#".bot = {command: "/review-bot"}"#]), "bot-trigger parent-wait");
    // no bot configured: none required
    assert_eq!(keys(&e![ON_OPEN_PARENT]), "parent-wait");
}

// @test "stacked with outstanding feedback: address it before waiting on the parent"
#[test]
fn stacked_with_outstanding_feedback_address_it_before_waiting_on_the_parent() {
    assert_eq!(keys(&e![ON_OPEN_PARENT, ".threads.owed_bot = 1"]), "bot parent-wait");
    assert_eq!(keys(&e![ON_OPEN_PARENT, ".threads.owed_bot = 1", ".threads.info = 1"]), "read parent-wait");
    assert_eq!(keys(&e![ON_OPEN_PARENT, ".threads.owed_human = 1"]), "human parent-wait");
}

// @test "stacked with a required human review: wait on the review before the parent"
#[test]
fn stacked_with_a_required_human_review_wait_on_the_review_before_the_parent() {
    assert_eq!(keys(&e![ON_OPEN_PARENT, r#".pr.merge_state = "BLOCKED""#, r#".pr.review_decision = "REVIEW_REQUIRED""#]), "review-wait parent-wait");
}

// @test "stacked and fully reviewed: the open parent is all that's left"
#[test]
fn stacked_and_fully_reviewed_the_open_parent_is_all_that_s_left() {
    assert_eq!(keys(&e![ON_OPEN_PARENT, r#".bot = {command: "/review-bot"}"#, r#".review.bot = {sha: "h1", status: "covered"}"#]), "parent-wait");
    assert_eq!(keys(&e![UNREVIEWED, ON_OPEN_PARENT, r#".review.accepted = {sha: "h1", status: "covered"}"#]), "parent-wait");
}

// @test "inconclusive CI: rerun it"
#[test]
fn inconclusive_ci_rerun_it() {
    assert_eq!(step("ci-rerun", "who", &e![r#".ci.verdict = "inconclusive""#]), "run");
}

// @test "CI pending or not yet on HEAD: wait"
#[test]
fn ci_pending_or_not_yet_on_head_wait() {
    assert_eq!(step("ci-wait", "who", &e![r#".ci.verdict = "pending""#]), "wait");
    assert_eq!(keys(&e![r#".ci.verdict = "pending""#]), "ci-wait");
    assert_eq!(keys(&e![".ci.for_head = false"]), "ci-wait");
}

// @test "CI waiting only on required checks to report: says which"
#[test]
fn ci_waiting_only_on_required_checks_to_report_says_which() {
    assert_eq!(step("ci-wait", "say", &e![r#".ci.verdict = "pending""#, r#".ci.missing = ["test"]"#]), "waiting on required check test to report");
    assert_eq!(step("ci-wait", "say", &e![r#".ci.verdict = "pending""#, r#".ci.missing = ["test", "ext"]"#]), "waiting on required checks test, ext to report");
    // A draft's missing check may never come (a workflow that skips drafts, or
    // filtered to other branches): the wait says how out, by hand.
    assert_eq!(step("ci-wait", "say", &e![".pr.draft = true", r#".ci.verdict = "pending""#, r#".ci.missing = ["test"]"#]), "waiting on required check test to report; if it never runs on a draft, undraft by hand (gh pr ready)");
    assert_eq!(step("ci-wait", "say", &e![r#".ci.verdict = "pending""#, r#".ci.missing = ["test"]"#, r#".ci.pending = ["lint"]"#]), "CI running");
}

// @test "unpushed commits: push, not wait for CI that can't run yet"
#[test]
fn unpushed_commits_push_not_wait_for_ci_that_can_t_run_yet() {
    assert_eq!(keys(&e![".push.ahead = 1", ".ci.for_head = false"]), "push");
    assert_eq!(keys(&e![".push.ahead = 1", r#".ci.verdict = "pending""#]), "push");
}

// @test "no PR: no CI wait"
#[test]
fn no_pr_no_ci_wait() {
    assert_eq!(keys(&e![".pr = null", ".ci.for_head = null", r#".ci.verdict = "none""#]), "open-pr");
}

// @test "order follows the loop's Ordering (#79 Design record): restack, agent fixes, push, then waiting"
#[test]
fn order_follows_the_loop_s_ordering_79_design_record_restack_agent_fixes_push_then_waiting() {
    // ci-red yields to push while commits are unpushed (they may be the fix), so
    // it shows up only once the push is done.
    assert_eq!(keys(&e![".fresh = false", ".threads.owed_human = 1", STACKED, r#".ci.verdict = "red""#, ".threads.owed_bot = 1", ".push.ahead = 1", ".pr.draft = true", r#".review.local = {sha: "x", status: "uncovered", uncovered: 1}"#]), "stale human behind bot push review");
    assert_eq!(keys(&e![".fresh = false", ".threads.owed_human = 1", STACKED, r#".ci.verdict = "red""#, ".threads.owed_bot = 1", ".pr.draft = true", r#".review.local = {sha: "x", status: "uncovered", uncovered: 1}"#]), "stale human behind ci-red bot review");
}

// @test "a state that isn't a record fails"
#[test]
fn a_state_that_isn_t_a_record_fails() {
    assert!(steps(&json!([]), "origin").is_err());
}

// @test "the remote and branch reach commands as data"
#[test]
fn the_remote_and_branch_reach_commands_as_data() {
    let s = jq(r#".push.remote_ours = false | .push.behind = 1 | .push.theirs = 1 | .branch = "a\"b""#, READY);
    let out = steps(&serde_json::from_str(&s).unwrap(), r#"up"x"#).unwrap();
    let cmd: Vec<String> = out.as_array().unwrap().iter().filter(|s| s["key"] == "foreign").map(|s| raw(&s["cmd"])).collect();
    assert_eq!(cmd.join("\n"), r#"git log HEAD..up"x/a"b"#);
}

// @test "lines: one NEXT line with who, say, command and a count of the rest"
#[test]
fn lines_one_next_line_with_who_say_command_and_a_count_of_the_rest() {
    let out = lines_for(false, &e![".push.ahead = 1", ".threads.owed_bot = 1"]);
    assert_eq!(out.lines().count(), 1);
    assert_eq!(out, "NEXT  agent  address 1 bot item  → sd act address  (+1 more)");
}

// @test "lines: a step with no command has no arrow"
#[test]
fn lines_a_step_with_no_command_has_no_arrow() {
    let out = lines_for(false, &e![r#".pr.state = "CLOSED""#]);
    assert_eq!(out, "NEXT  you    PR closed without merging");
}

// @test "lines: all lists every step, the rest as then"
#[test]
fn lines_all_lists_every_step_the_rest_as_then() {
    let out = lines_for(true, &e![".push.ahead = 1", ".threads.owed_bot = 1"]);
    let ls: Vec<&str> = out.lines().collect();
    assert_eq!(ls.len(), 2);
    assert_eq!(ls[0], "NEXT  agent  address 1 bot item  → sd act address");
    assert_eq!(ls[1], "then  run    push 1 commit  → sd act push");
}

// @test "question: a you step quoting its first line, in place of the agent step that asked (#167)"
#[test]
fn question_a_you_step_quoting_its_first_line_in_place_of_the_agent_step_that_asked_167() {
    let red = e![r#".ci.verdict = "red""#, r#".ci.failing = ["test"]"#];
    assert_eq!(keys(&e![&red]), "ci-red");
    assert_eq!(keys(&e![&red, &asked("ci-fix", true)]), "question");
    assert_eq!(step("question", "who", &e![&red, &asked("ci-fix", true)]), "you");
    assert_eq!(step("question", "say", &e![&red, &asked("ci-fix", true)]), "the agent asks (ci-fix): Which test is right?; answer it with sd act answer; or answer in its session (sd act attach), then sd act ack --question");
    assert_eq!(step("question", "cmd", &e![&red, &asked("ci-fix", true)]), r#"sd act answer "<your answer>""#);
    // about an older HEAD or worktree: it no longer stands
    assert_eq!(keys(&e![&red, &asked("ci-fix", false)]), "ci-red");
}

#[test]
fn a_give_up_is_a_question_step_that_says_so_254() {
    let red = e![r#".ci.verdict = "red""#, r#".ci.failing = ["test"]"#];
    let gave_up = format!(".question = {}", json!({"step": "ci-fix", "kind": "gave-up", "sha": "h1", "text": "no idea\nmore", "at_head": true}));
    assert_eq!(keys(&e![&red, &gave_up]), "question");
    assert_eq!(
        step("question", "say", &e![&red, &gave_up]),
        "the agent gave up (ci-fix): no idea; guide it with sd act answer, fix it by hand, or sd act ack --question to let it try again"
    );
    assert_eq!(step("question", "cmd", &e![&red, &gave_up]), r#"sd act answer "<your answer>""#);
}

#[test]
fn a_resolve_give_up_holds_the_conflict_step_and_says_resolve_by_hand_257() {
    let conflict = e![".sync.up_to_date = false", ".sync.behind = 3", ".sync.conflict = true"];
    let gave_up = |at_head: bool| format!(".question = {}", json!({"step": "resolve", "kind": "gave-up", "sha": "h1", "text": "no sensible merge", "at_head": at_head}));
    assert_eq!(keys(&e![&conflict, &gave_up(true)]), "question");
    assert_eq!(
        step("question", "say", &e![&conflict, &gave_up(true)]),
        "the agent gave up (resolve): no sensible merge; resolve it by hand (sd act restack says how), or sd act ack --question to let it try again"
    );
    assert_eq!(step("question", "cmd", &e![&conflict, &gave_up(true)]), "sd act restack");
    assert_eq!(keys(&e![&conflict, &gave_up(false)]), "conflict");
}

// @test "question: replaces only the step that asked, ahead of the loop's own steps (#167)"
#[test]
fn question_replaces_only_the_step_that_asked_ahead_of_the_loop_s_own_steps_167() {
    assert_eq!(keys(&e![r#".ci.verdict = "red""#, r#".ci.failing = ["test"]"#, ".threads.owed_bot = 2", &asked("address", true)]), "question ci-red");
    assert_eq!(keys(&e![".threads.owed_bot = 2", &asked("ci-fix", true)]), "question bot");
    assert_eq!(keys(&e![FRESH, r#".task = "add c""#, &asked("implement", true)]), "question");
    let st = e![r#".review.local = {sha: "x", status: "uncovered", uncovered: 2}"#, r#".review.findings = {sha: "h1", round: 1, n: 2, at_head: true, declined: false, cap: 3, items: []}"#];
    // not review-decide or another review either: the round waits on the answer
    assert_eq!(keys(&e![&st, &asked("review-fix", true)]), "question");
    assert_eq!(keys(&e![&st, &asked("review-fix", false)]), "review-fix");
    assert_eq!(keys(&e![r#".pr.merge_state = "BEHIND""#, ".sync.up_to_date = false", ".sync.behind = 3", &asked("ci-fix", true)]), "question behind");
}

// @test "dirty: uncommitted edits with no question standing are chrismo's, in place of the agent steps that refuse them (#178)"
#[test]
fn dirty_uncommitted_edits_with_no_question_standing_are_chrismo_s_in_place_of_the_agent_steps_that_refuse_them_178() {
    let red = e![r#".ci.verdict = "red""#, r#".ci.failing = ["test"]"#];
    assert_eq!(keys(&e![&red, ".push.dirty = true"]), "dirty");
    assert_eq!(step("dirty", "who", &e![&red, ".push.dirty = true"]), "you");
    assert_eq!(step("dirty", "say", &e![&red, ".push.dirty = true"]), "the worktree has uncommitted changes, which the agent steps refuse: they're yours: commit and push them (sd act push), or discard them (git reset --hard; git clean for untracked files)");
    assert_eq!(step("dirty", "cmd", &e![&red, ".push.dirty = true"]), "git status");
    assert_eq!(keys(&e![".threads.owed_bot = 2", ".push.dirty = true"]), "dirty");
    let st = e![r#".review.local = {sha: "x", status: "uncovered", uncovered: 2}"#, r#".review.findings = {sha: "h1", round: 1, n: 2, at_head: true, declined: false, cap: 3, items: []}"#];
    assert_eq!(keys(&e![&st, ".push.dirty = true"]), "dirty");
    // one step, however many agent steps it stands in for
    assert_eq!(keys(&e![&red, ".threads.owed_bot = 2", ".push.dirty = true"]), "dirty");
    // review and ship refuse it too
    assert_eq!(keys(&e![UNREVIEWED, ".push.dirty = true"]), "dirty");
    assert_eq!(keys(&e![".pr = null", ".push.dirty = true"]), "dirty");
    assert_eq!(keys(&e![".pr = null", ".push.remote_sha = null", ".push.ahead = 1", ".push.dirty = true"]), "dirty push");
    // with commits reset away, reset-dirty says it already
    let gone = e![".pr = null", ".push.behind = 1", ".push.remote_ours = true", ".push.remote_made = true", ".push.dirty = true"];
    assert_eq!(keys(&e![&gone]), "reset-dirty open-pr");
    // and on an open PR, in place of the agent steps that refuse the edits
    let reset = e![".push.ahead = 0", ".push.behind = 1", ".push.remote_ours = true", ".push.remote_made = true", ".push.dirty = true"];
    assert_eq!(keys(&e![UNREVIEWED, &reset]), "reset-dirty");
    assert_eq!(keys(&e![&red, &reset]), "reset-dirty");
    // restack refuses it too, behind or conflicting
    assert_eq!(keys(&e![STACKED, ".push.dirty = true"]), "dirty");
    assert_eq!(keys(&e![STACKED, ".sync.by_tree = true", ".push.dirty = true"]), "dirty");
    assert_eq!(keys(&e![STACKED, ".sync.conflict = true", ".push.dirty = true"]), "dirty");
    assert_eq!(keys(&e![r#".pr.merge_state = "BEHIND""#, ".sync.up_to_date = false", ".sync.behind = 3", ".push.dirty = true"]), "dirty");
    // the by-tree conflict is chrismo's already, and behind with no base names no restack
    assert_eq!(keys(&e![STACKED, ".sync.conflict = true", ".sync.by_tree = true", ".push.dirty = true"]), "conflict");
}

// @test "dirty: while a question stands with its edits uncommitted, the other steps that refuse them wait too (#178)"
#[test]
fn dirty_while_a_question_stands_with_its_edits_uncommitted_the_other_steps_that_refuse_them_wait_too_178() {
    assert_eq!(keys(&e![".threads.owed_bot = 1", ".push.dirty = true", &asked("ci-fix", true)]), "question");
    assert_eq!(keys(&e![".threads.owed_bot = 1", ".push.dirty = false", &asked("ci-fix", true)]), "question bot");
    assert_eq!(keys(&e![UNREVIEWED, ".push.dirty = true", &asked("address", true)]), "question");
    assert_eq!(keys(&e![r#".ci.verdict = "red""#, r#".ci.failing = ["test"]"#, ".push.dirty = true", &asked("address", true)]), "question");
    let st = e![r#".review.local = {sha: "x", status: "uncovered", uncovered: 2}"#, r#".review.findings = {sha: "h1", round: 1, n: 2, at_head: true, declined: false, cap: 3, items: []}"#];
    assert_eq!(keys(&e![&st, ".push.dirty = true", &asked("ci-fix", true)]), "question");
    assert_eq!(keys(&e![STACKED, ".push.dirty = true", &asked("ci-fix", true)]), "question");
}

// @test "dirty: no step while the question stands (the edits are the agent's), the worktree is clean, or no agent step is due (#178)"
#[test]
fn dirty_no_step_while_the_question_stands_the_edits_are_the_agent_s_the_worktree_is_clean_or_no_agent_step_is_due_178() {
    let red = e![r#".ci.verdict = "red""#, r#".ci.failing = ["test"]"#];
    assert_eq!(keys(&e![&red, ".push.dirty = true", &asked("ci-fix", true)]), "question");
    assert_eq!(keys(&e![&red, ".push.dirty = false"]), "ci-red");
    assert_eq!(keys(&e![&red, ".push.dirty = null"]), "ci-red");
    assert_eq!(keys(&e![".push.dirty = true"]), "ready");
    assert_eq!(keys(&e![r#".ci.verdict = "pending""#, ".push.dirty = true"]), "ci-wait");
}

// ── reviewer mode: a teammate's PR, followed (#250) ──────────────────────────

/// A followed PR at its head, reviewed by no one yet.
const REVIEWING: &[&str] = &[
    r#".mode = "reviewer""#,
    r#".branch = "review/7""#,
    ".push.remote_ours = null",
    r#".review.local = {sha: null, status: "none"}"#,
    ".review.github = null",
];

const REVIEWED_LOCALLY: &str = r#".review.local = {sha: "h1", status: "covered"}"#;

#[test]
fn reviewer_mode_unreviewed_the_agent_reviews_the_prs_change() {
    assert_eq!(keys(&e![REVIEWING]), "review");
    assert_eq!(step("review", "who", &e![REVIEWING]), "agent");
    assert_eq!(step("review", "say", &e![REVIEWING]), "review the PR's change");
    assert_eq!(step("review", "cmd", &e![REVIEWING]), "sd act review");
}

#[test]
fn reviewer_mode_reviewed_here_chrismo_submits_on_github() {
    assert_eq!(keys(&e![REVIEWING, REVIEWED_LOCALLY]), "submit");
    assert_eq!(step("submit", "who", &e![REVIEWING, REVIEWED_LOCALLY]), "you");
    assert_eq!(step("submit", "say", &e![REVIEWING, REVIEWED_LOCALLY]), "the reviewer found nothing at HEAD: approve or comment on GitHub");
    assert_eq!(step("submit", "cmd", &e![REVIEWING, REVIEWED_LOCALLY]), "gh pr view 7 --web");
}

#[test]
fn reviewer_mode_findings_at_head_go_in_chrismos_review_never_a_fix_round() {
    let f = r#".review.findings = {sha: "h1", round: 1, n: 2, at_head: true, declined: false, cap: 3, items: []}"#;
    assert_eq!(keys(&e![REVIEWING, f]), "submit");
    assert_eq!(step("submit", "say", &e![REVIEWING, f]), "2 review findings at HEAD (sd state --long): put them in your review on GitHub");
}

#[test]
fn reviewer_mode_a_pending_review_on_github_is_chrismos_to_finish() {
    let p = r#".review.github = {sha: null, state: null, at_head: false, pending: true}"#;
    assert_eq!(keys(&e![REVIEWING, REVIEWED_LOCALLY, p]), "submit");
    assert_eq!(step("submit", "say", &e![REVIEWING, REVIEWED_LOCALLY, p]), "finish your pending review on GitHub");
}

#[test]
fn reviewer_mode_submitted_on_head_waits_on_the_author() {
    let g = r#".review.github = {sha: "h1", state: "COMMENTED", at_head: true, pending: false}"#;
    assert_eq!(keys(&e![REVIEWING, REVIEWED_LOCALLY, g]), "author-wait");
    assert_eq!(step("author-wait", "who", &e![REVIEWING, REVIEWED_LOCALLY, g]), "wait");
    assert_eq!(step("author-wait", "say", &e![REVIEWING, REVIEWED_LOCALLY, g]), "you reviewed HEAD (commented): waiting on the author");
    // Submitted with no local review: chrismo's review stands; no agent step.
    assert_eq!(keys(&e![REVIEWING, g]), "author-wait");
}

#[test]
fn reviewer_mode_the_author_pushed_fetch_it_before_reviewing() {
    let stale = e![".fresh = false", ".push.author_pushed = true"];
    assert_eq!(keys(&e![REVIEWING, &stale]), "fetch");
    assert_eq!(step("fetch", "who", &e![REVIEWING, &stale]), "run");
    assert_eq!(step("fetch", "say", &e![REVIEWING, &stale]), "the author pushed: take it");
    assert_eq!(step("fetch", "cmd", &e![REVIEWING, &stale]), "sd act fetch");
    assert_eq!(keys(&e![REVIEWING, ".push.behind = 2"]), "fetch");
    assert_eq!(step("fetch", "say", &e![REVIEWING, ".fresh = false"]), "fetch: GitHub is ahead of your remote refs");
    assert_eq!(keys(&e![REVIEWING, ".push.behind = 1 | .push.ahead = 1 | .push.diverged = true"]), "fetch");
}

#[test]
fn reviewer_mode_after_the_author_pushes_review_again() {
    let again = e![r#".review.local = {sha: "h0", status: "uncovered", uncovered: 2}"#, r#".review.github = {sha: "h0", state: "CHANGES_REQUESTED", at_head: false, pending: false}"#];
    assert_eq!(keys(&e![REVIEWING, &again]), "review");
    assert_eq!(step("review", "say", &e![REVIEWING, &again]), "review again: 2 commits since the last review");
}

#[test]
fn reviewer_mode_uncommitted_changes_block_review_and_fetch() {
    let dirty = ".push.dirty = true";
    assert_eq!(keys(&e![REVIEWING, dirty]), "dirty");
    assert_eq!(step("dirty", "who", &e![REVIEWING, dirty]), "you");
    assert_eq!(keys(&e![REVIEWING, dirty, ".push.behind = 1"]), "dirty");
}

#[test]
fn reviewer_mode_commits_the_pr_doesnt_have_are_chrismos_to_move() {
    let mine = ".push.ahead = 1";
    assert_eq!(keys(&e![REVIEWING, mine]), "yours");
    assert_eq!(step("yours", "say", &e![REVIEWING, mine]), "review/7 has 1 commit the PR doesn't: move it to a branch of your own; review and fetch wait");
}

#[test]
fn reviewer_mode_merged_or_closed_forget_the_branch() {
    assert_eq!(keys(&e![REVIEWING, r#".pr.state = "MERGED""#]), "merged");
    assert_eq!(step("merged", "cmd", &e![REVIEWING, r#".pr.state = "MERGED""#]), "sd act forget --remove");
    assert_eq!(keys(&e![REVIEWING, r#".pr.state = "CLOSED""#]), "closed");
    assert_eq!(step("closed", "cmd", &e![REVIEWING, r#".pr.state = "CLOSED""#]), "sd act forget");
}

#[test]
fn reviewer_mode_with_no_pr_only_the_errors() {
    assert_eq!(keys(&e![REVIEWING, REVIEWED_LOCALLY, ".pr = null", r#".errors = ["gh (review/7): GitHub has no PR #7"]"#]), "errors");
}

#[test]
fn reviewer_mode_errors_are_listed_first() {
    assert_eq!(keys(&e![REVIEWING, r#".errors = ["gh (review/7): GitHub has no PR #7"]"#]), "errors review");
}

#[test]
fn reviewer_mode_failed_agent_steps_hand_the_review_to_chrismo() {
    let failed = r#".failed = {count: 3, step: "review", harness: "claude", at: "2026-10-23T10:00:00Z", error: "boom"}"#;
    assert_eq!(keys(&e![REVIEWING, failed]), "failed");
}

#[test]
fn reviewer_mode_never_offers_the_authors_steps() {
    let authors = e![
        r#".ci.verdict = "red" | .ci.failing = ["test"]"#,
        ".sync.up_to_date = false | .sync.behind = 3",
        r#".pr.merge_state = "BEHIND" | .pr.draft = true"#,
        ".threads.owed_bot = 2 | .threads.owed_human = 1",
        r#".bot.command = "@bot review""#,
        r#".task = "a task""#,
    ];
    assert_eq!(keys(&e![REVIEWING, &authors]), "review");
    assert_eq!(keys(&e![REVIEWING, REVIEWED_LOCALLY, &authors]), "submit");
}
