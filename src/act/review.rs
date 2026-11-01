// review (A8), and review --accept: verify, then the local reviewer over the
// branch's own change, base..HEAD. Clean pins local; findings that block are
// recorded for the review-fix round and nothing is pinned. Whether it runs is
// gates::review's, which NEXT asks too (#348): a HEAD a review already covers
// (#241), or one its findings or a failed verify stand at, is refused at
// zero tokens unless --force. --accept asks gates::accept: a base to pin
// against, whatever the worktree holds. verify and the reviewer run under the
// branch's running marker, a custom settings.localReview too, so another live
// step's marker refuses review before its verify runs (#270).

use super::commands::{step_timeout, verify_run};
use super::gates::{self, rvw};
use super::step::spend_worded;
use super::{cfg, die, enforce, no_base_why, plural, state_json};
use crate::config::one_value;
use crate::git::{git, git_out, rev, worktree_for};
use crate::pins::{self, Pin};
use crate::json;
use crate::next::Derived;
use crate::{decisions, harness, limit, proc};
use crate::state::flag_lines;
use crate::val::{at, on, t};
use serde_json::{json, Value};
use std::process::Command;

fn must(o: proc::Out) {
    if !o.ok {
        eprint!("{}", o.stderr);
        std::process::exit(o.code);
    }
}

pub(super) fn short(s: &str) -> &str {
    s.get(..7).unwrap_or(s)
}

fn blocking(findings: &[Value]) -> usize {
    findings.iter().filter(|f| at(f, "severity").as_str() != Some("nit")).count()
}

/// <prev>'s round plus one, counting from 0 when <prev> has no usable round.
/// A review of the same HEAD (or the same change rebased, as sd state judged
/// it) isn't a new round.
fn round(prev: &str, at_head: bool) -> u64 {
    let last = one_value(prev).and_then(|r| at(&r, "round").as_u64()).unwrap_or(0);
    if at_head {
        last
    } else {
        last + 1
    }
}

/// refs/sheepdog/<b>/findings' blob. It drops the earlier findings' answers,
/// which were about a list this one needn't repeat.
fn record(head: &str, base: &str, round: u64, findings: &[Value]) -> String {
    json::to_string(&json!({"head": head, "base": base, "round": round, "findings": findings})) + "\n"
}

/// The decisions an accept at <head> makes of the findings record <rec>:
/// each finding that blocks and wasn't fixed stands, accepted, with the
/// reason when the agent declined it.
fn accepted(rec: &str, head: &str) -> Vec<Value> {
    let Some(rec) = one_value(rec) else { return vec![] };
    let standing = |f: &&Value| at(f, "severity").as_str() != Some("nit") && at(f, "action").as_str() != Some("fixed");
    let why = |f: &Value| if at(f, "action").as_str() == Some("declined") { t(f, "reply") } else { String::new() };
    let fs = at(&rec, "findings").as_array().cloned().unwrap_or_default();
    fs.iter().filter(standing).map(|f| decisions::of_finding("accepted", head, f, &why(f))).collect()
}

fn accept(b: &str, head: &str, base: &str) {
    pins::review(".", b, Pin::Accepted, head, base).unwrap_or_else(|e| die(e));
    if let Some(findings) = pins::read(".", b, Pin::Findings) {
        decisions::append(b, &accepted(&findings, head));
    }
    let after = format!("branch.{b}.sd-accepted-after");
    match git_out(".", &["config", "--get", &format!("branch.{b}.sd-bot-triggered")]) {
        Some(when) if !when.is_empty() => must(git(".", &["config", &after, &when])),
        _ => {
            git(".", &["config", "--unset", &after]);
        }
    }
    must(pins::delete(".", b, Pin::Findings));
    println!("accepted: {b} at {} against {}; pinned accepted", short(head), short(base));
}

/// <why>, one of gates::review's or gates::accept's reasons, in record <s>'s
/// words, about <head>.
fn worded(why: &str, s: &Value, d: &Derived, head: &str, remote: &str) -> String {
    let b = t(s, "branch");
    let covers = |p: &str| matches!(t(s, &format!("review.{p}.status")).as_str(), "covered" | "rewritten-same");
    match why {
        rvw::UNSEEN => format!("{b} isn't checked out in any worktree, or git status failed there"),
        rvw::DIRTY => format!("{b}'s worktree has uncommitted changes"),
        rvw::NO_BASE => format!("can't find {b}'s base: {}", no_base_why(s, remote)),
        rvw::CLOSED => format!("PR #{} is {}", t(s, "pr.number"), t(s, "pr.state").to_lowercase()),
        rvw::COVERED => {
            let by = if covers("local") { "local review" } else if covers("accepted") { "an accept" } else { "the bot's review" };
            format!("{by} already covers {}", short(head))
        }
        rvw::FINDINGS => {
            let n = at(s, "review.findings.n").as_u64().unwrap_or(0) as usize;
            let stand = if n == 1 { "stands" } else { "stand" };
            format!("{} {stand} at {} (sd state --long): {}", plural(n, "review finding"), short(head), gates::findings_clear(d))
        }
        rvw::VERIFY_RED => format!("verify failed at {}: fix it, then sd act verify", short(head)),
        rvw::ANSWER => format!("your answer to the {} agent's question goes on first", t(s, "question.step")),
        _ => why.into(),
    }
}

pub fn review(b: &str, remote: &str, accepting: bool, force: bool) {
    let wt = worktree_for(".", b);
    let s = state_json(if wt.is_empty() { "." } else { &wt }, b).unwrap_or_else(|e| {
        eprint!("{e}");
        die("sd state failed")
    });
    let d = Derived::new(&s);
    let base = t(&s, "sync.old_base");
    let head = rev(".", &format!("refs/heads/{b}"));
    let word = |w: &str| worded(w, &s, &d, &head, remote);
    if accepting {
        let Some(_go) = enforce(gates::accept(&d).worded(word), force, &format!("accepting {b}")) else { return };
        return accept(b, &head, &base);
    }
    let gate = spend_worded(gates::review(&d).worded(word), &s, "review", b);
    let Some(_go) = enforce(gate, force, &format!("reviewing {b}")) else { return };
    reviewed(b, &wt, &s, &base, &head);
}

/// verify, the reviewer, and its findings pinned or recorded.
fn reviewed(b: &str, wt: &str, s: &Value, base: &str, head: &str) {
    if let Err(cmd) = verify_run(wt, b, "review") {
        die(format!("verify failed ({cmd}); not reviewing"));
    }
    let mut secs = step_timeout(b);
    // A child process on purpose, sd review-local too: by contract
    // (sd review-local --help) settings.localReview is any script.
    let mut reviewer = cfg("settings.localReview", b);
    let mut c = if reviewer.is_empty() {
        // Its harness step's own stepTimeout must fire first: this one can't
        // reach the harness, which would go on spending tokens.
        secs += 30;
        reviewer = "sd review-local".into();
        let mut c = Command::new(proc::sd_itself().unwrap_or_else(|e| die(format!("can't find sd itself: {e}"))));
        c.arg("review-local");
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", &reviewer]);
        c
    };
    let task = git_out(".", &["config", "--get", &format!("branch.{b}.sd-task")]).unwrap_or_default();
    c.current_dir(wt);
    c.env("SHEEPDOG_BASE", base).env("SHEEPDOG_HEAD", head).env("SHEEPDOG_TASK", &task);
    c.env("SHEEPDOG_DECISIONS", decisions::on_record(b));
    let o = proc::timeout_passing_stderr(secs, c);
    eprint!("{}", o.stderr);
    if !o.ok {
        eprintln!("sd act: the reviewer failed ({reviewer}); nothing pinned");
        if o.code != harness::LIMIT_STATUS {
            std::process::exit(1);
        }
        if limit::holding().is_none() {
            limit::write("reviewer", &format!("{reviewer} exited 75"), None);
        }
        std::process::exit(o.code);
    }
    let Some(Value::Array(findings)) = one_value(&o.stdout) else {
        die(format!("the reviewer printed no JSON list ({reviewer})"))
    };
    let shown = o.stdout.trim_end_matches('\n');
    let flagged = at(s, "flags").as_array().map_or(0, Vec::len);
    if flagged > 0 {
        eprintln!("sd act: {flagged} flagged for chrismo in {}..{} (not a gate):", short(base), short(head));
        eprint!("{}", flag_lines(s));
    }
    let (n, blocking) = (findings.len(), blocking(&findings));
    if blocking == 0 {
        pins::review(".", b, Pin::Local, head, base).unwrap_or_else(|e| die(e));
        must(pins::delete(".", b, Pin::Findings));
        let nits = if n == 0 { String::new() } else { format!(" ({})", plural(n, "nit")) };
        println!("clean: {b} at {} reviewed against {}; pinned local{nits}", short(head), short(base));
        if n > 0 {
            println!("{shown}");
        }
        return;
    }
    let prev = pins::read(".", b, Pin::Findings).unwrap_or_default();
    let round = round(&prev, on(s, "review.findings.at_head"));
    if !pins::store(".", b, Pin::Findings, &record(head, base, round, &findings), None).ok {
        die("couldn't record the findings");
    }
    eprintln!("sd act: {} on {b}, {blocking} blocking (review round {round}); nothing pinned", plural(n, "finding"));
    println!("{shown}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocking_counts_all_but_nits_and_one_with_no_severity_blocks() {
        let f = json!([
            {"severity": "nit"}, {"severity": "warning"}, {"severity": "error"}, {}, {"severity": 3}, "nit"
        ]);
        assert_eq!(blocking(f.as_array().unwrap()), 5);
        assert_eq!(blocking(json!([{"severity": "nit"}]).as_array().unwrap()), 0);
        assert_eq!(blocking(&[]), 0);
    }

    #[test]
    fn round_is_the_last_plus_one_unless_the_last_was_about_this_head() {
        assert_eq!(round(r#"{"round": 2, "findings": []}"#, false), 3);
        assert_eq!(round("{\"round\": 2}\n", true), 2);
        assert_eq!(round("", false), 1);
    }

    #[test]
    fn round_starts_over_from_a_record_with_no_usable_round() {
        for prev in [r#"{"round": "lots"}"#, r#"{"round": -1}"#, r#"{"round": 2.5}"#, "{}", "[]", "nope", "{} {}"] {
            assert_eq!(round(prev, false), 1, "{prev}");
            assert_eq!(round(prev, true), 0, "{prev}");
        }
    }

    #[test]
    fn record_is_head_base_round_and_the_findings_in_order_one_line() {
        let f = json!([{"file": "a", "line": 3, "severity": "warning", "body": "c?"}]);
        assert_eq!(
            record("h1", "b1", 2, f.as_array().unwrap()),
            "{\"head\":\"h1\",\"base\":\"b1\",\"round\":2,\"findings\":[{\"file\":\"a\",\"line\":3,\"severity\":\"warning\",\"body\":\"c?\"}]}\n"
        );
    }
}
