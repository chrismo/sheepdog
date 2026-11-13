// Black-box tests of sd act decision (#454, #462): chrismo's decision, or
// his decline of one review finding, appended to the decisions record
// (refs/sheepdog/<b>/decisions, #343) at HEAD, for later review rounds to
// read. Zero tokens: no agent, no worktree, nothing pushed.
//
// Real git and the real sd state; gh is a stub that fails. The local
// reviewer is a stub that records the decisions it was handed.

mod common;

use common::{assert_has, jq, running_marker, Holder, Sd, GH_FAILS};
use std::fs;

const REVIEWER: &str = r#"#!/usr/bin/env bash
printf '%s' "$SHEEPDOG_DECISIONS" > "$FX/reviewer.decisions"
echo '[]'
"#;

/// origin.git and its clone repo (cwd), main pushed, feat one commit past
/// it; settings.localReview is the stub reviewer.
fn sd() -> Sd {
    let mut sd = Sd::for_verb("act");
    sd.env = vec![
        ("SHEEPDOG_STATE_DIR".to_string(), sd.path("state")),
        ("FX".to_string(), sd.path("fx")),
    ];
    fs::create_dir(sd.tmp.join("fx")).unwrap();
    sd.fake("gh", GH_FAILS);
    sd.script(&sd.tmp.join("reviewer"), REVIEWER);
    sd.verb_ok("config", &["set", "--global", "settings.localReview", &sd.path("reviewer")]);
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
    /// The decisions record, one JSON line each, through jq -c <filter>.
    fn decisions(&self, filter: &str) -> String {
        jq(&["-c", filter], &self.git(&["cat-file", "blob", "refs/sheepdog/feat/decisions"]))
    }

    fn has_decisions(&self) -> bool {
        self.git_status(&["rev-parse", "-q", "--verify", "refs/sheepdog/feat/decisions"]) == 0
    }

    fn write_findings_ref(&self, json: &str) {
        let blob = self.git_with(&[], Some(&format!("{json}\n")), &["hash-object", "-w", "--stdin"]);
        self.git(&["update-ref", "refs/sheepdog/feat/findings", &blob]);
    }
}

const FINDINGS: &str = r#"{"head": "h0", "base": "b0", "round": 1, "findings": [
  {"file": "file.txt", "line": 3, "severity": "error", "body": "c?"},
  {"file": "file.txt", "line": 1, "severity": "warning", "body": "a should be A"}]}"#;

#[test]
fn a_decision_goes_on_the_record_at_head_and_prints_a_receipt_454() {
    let sd = sd();
    let r = sd.run(&["decision", "keep the local checks out: IAM authorizes"]);
    assert_eq!(r.status, 0, "{}", r.output);
    let head = sd.git(&["rev-parse", "HEAD"]);
    assert_has(&r.output, &["recorded on feat at", &head[..7], "keep the local checks out: IAM authorizes"]);
    assert_eq!(
        sd.decisions("[.kind, .head, .text]"),
        format!("[\"decided\",\"{head}\",\"keep the local checks out: IAM authorizes\"]")
    );
    assert!(!sd.decisions(".at").is_empty());
}

// Append-only: the second decision follows the first, which stays.
#[test]
fn a_second_decision_appends_and_the_first_stays_454() {
    let sd = sd();
    sd.ok(&["decision", "first"]);
    sd.ok(&["decision", "second", "feat"]);
    assert_eq!(sd.decisions(".text"), "\"first\"\n\"second\"");
}

// #462: --finding <n> declines the findings record's nth finding (the id a
// review-fix round gives it) with <text> as why, recorded as the round's own
// declines are.
#[test]
fn finding_declines_that_finding_with_the_text_as_why_462() {
    let sd = sd();
    sd.write_findings_ref(FINDINGS);
    let r = sd.run(&["decision", "--finding", "2", "a is lowercase on purpose"]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_has(&r.output, &["recorded on feat at", "declined finding 2 (file.txt:1)", "a is lowercase on purpose"]);
    let head = sd.git(&["rev-parse", "HEAD"]);
    assert_eq!(
        sd.decisions("[.kind, .head, .about, .finding, .text]"),
        format!("[\"declined\",\"{head}\",\"file.txt:1\",\"a should be A\",\"a is lowercase on purpose\"]")
    );
}

// #501: the decline also settles the finding on the findings record, which
// NEXT reads, as a review-fix round's decline does: declined, chrismo's text
// its reply, by chrismo. The other findings, and the record's head and
// round, stay as they were.
#[test]
fn finding_settles_that_finding_on_the_findings_record_501() {
    let sd = sd();
    sd.write_findings_ref(FINDINGS);
    sd.ok(&["decision", "--finding", "2", "a is lowercase on purpose"]);
    let rec = sd.git(&["cat-file", "blob", "refs/sheepdog/feat/findings"]);
    assert_eq!(jq(&["-c", "[.head, .round]"], &rec), "[\"h0\",1]");
    assert_eq!(
        jq(&["-c", ".findings[] | [.body, .action, .reply, .by]"], &rec),
        "[\"c?\",null,null,null]\n[\"a should be A\",\"declined\",\"a is lowercase on purpose\",\"chrismo\"]"
    );
}

// A finding already settled can't be declined: one the agent fixed, or one
// chrismo declined already, is refused with nothing recorded. One the agent
// declined, chrismo's decline takes over.
#[test]
fn a_finding_fixed_or_declined_by_chrismo_is_refused_501() {
    let sd = sd();
    sd.write_findings_ref(
        r#"{"head": "h0", "round": 1, "findings": [
          {"file": "file.txt", "line": 3, "body": "c?", "action": "fixed"},
          {"file": "file.txt", "line": 1, "body": "a?", "action": "declined", "by": "chrismo"},
          {"file": "file.txt", "line": 2, "body": "b?", "action": "declined", "reply": "no"}]}"#,
    );
    for (id, was) in [("1", "fixed"), ("2", "declined by you")] {
        let r = sd.run(&["decision", "--finding", id, "why"]);
        assert_ne!(r.status, 0);
        assert_has(&r.output, &[&format!("finding {id} on feat is {was} already")]);
    }
    assert!(!sd.has_decisions());
    sd.ok(&["decision", "--finding", "3", "b stays"]);
    let rec = sd.git(&["cat-file", "blob", "refs/sheepdog/feat/findings"]);
    assert_eq!(
        jq(&["-c", ".findings[] | [.action, .by]"], &rec),
        "[\"fixed\",null]\n[\"declined\",\"chrismo\"]\n[\"declined\",\"chrismo\"]"
    );
}

#[test]
fn an_unknown_finding_is_refused_and_nothing_recorded_462() {
    let sd = sd();
    let r = sd.run(&["decision", "--finding", "1", "why"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["no review findings recorded on feat"]);
    sd.write_findings_ref(FINDINGS);
    for id in ["3", "0", "x"] {
        let r = sd.run(&["decision", "--finding", id, "why"]);
        assert_ne!(r.status, 0);
        assert_has(&r.output, &[&format!("no finding {id} on feat"), "1 to 2"]);
    }
    assert!(!sd.has_decisions());
}

// The next local review is handed the decision (#454's acceptance).
#[test]
fn the_next_local_review_gets_the_decision_454() {
    let sd = sd();
    sd.ok(&["decision", "trust the registered schedule"]);
    sd.ok(&["review"]);
    let handed = fs::read_to_string(sd.tmp.join("fx/reviewer.decisions")).unwrap();
    let head = sd.git(&["rev-parse", "--short=7", "HEAD"]);
    assert_eq!(handed, format!("- [{head}] chrismo decided: trust the registered schedule"));
}

// A step in flight was handed its context already: a decision recorded under
// it would change what later rounds see but not what it saw. Soft: --force
// records it anyway.
#[test]
fn a_step_holding_the_running_marker_refuses_unless_force_454() {
    let sd = sd();
    let holder = Holder::new();
    running_marker(&sd.tmp.join("repo/.git"), "feat", "implement", holder.pid());
    let r = sd.run(&["decision", "x"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &[&format!("implement is running on feat (pid {}", holder.pid()), "--force"]);
    assert!(!sd.has_decisions());
    sd.ok(&["decision", "--force", "x"]);
    assert_eq!(sd.decisions(".text"), "\"x\"");
}

#[test]
fn no_text_is_refused_with_the_usage_454() {
    let sd = sd();
    for args in [&["decision"][..], &["decision", " "], &["decision", "a", "feat", "c"]] {
        let r = sd.run(args);
        assert_ne!(r.status, 0);
        assert_has(&r.output, &["usage: sd act decision [--force] [--finding <n>] <text> [<branch>]"]);
    }
    let r = sd.run(&["push", "--finding", "1"]);
    assert_has(&r.output, &["--finding is for sd act decision only"]);
}

// #501's acceptance: with chrismo's decline of the only finding at HEAD,
// NEXT hands it to no review-fix round: it's chrismo's to accept.
#[test]
fn next_offers_no_review_fix_for_a_finding_chrismo_declined_501() {
    let sd = sd();
    let (head, base) = (sd.git(&["rev-parse", "HEAD"]), sd.git(&["rev-parse", "main"]));
    sd.write_findings_ref(&format!(
        r#"{{"head": "{head}", "base": "{base}", "round": 1, "findings": [{{"file": "file.txt", "line": 3, "severity": "error", "body": "c?"}}]}}"#
    ));
    let next = || jq(&["-r", r#".next[] | "\(.key): \(.say)""#], &sd.verb_ok("state", &["--json"]));
    assert_has(&next(), &["review-fix: fix 1 review finding (round 1/3)"]);
    sd.ok(&["decision", "--finding", "1", "c stays"]);
    let after = next();
    assert_has(&after, &["review-decide: you declined 1 review finding: accept HEAD as reviewed"]);
    assert!(!after.contains("review-fix"), "{after}");
}
