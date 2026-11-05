// decision (#454, #462): chrismo's decision on a branch, or with --finding
// his decline of one review finding, appended to the decisions record at
// HEAD (decisions.rs) for the review rounds after it. Zero tokens: no agent,
// no worktree, no review pin or ack.

use super::review::short;
use super::{die, enforce, Args, Gate};
use crate::config::one_value;
use crate::git::rev;
use crate::pins::{self, Pin};
use crate::val::{at, t};
use crate::{decisions, running};
use serde_json::Value;

pub fn decision(b: &str, a: &Args) {
    let head = rev(".", &format!("refs/heads/{b}"));
    if head.is_empty() {
        die(format!("no branch {b}"));
    }
    let Some(_go) = enforce(in_flight(b), a.force, &format!("recording a decision on {b}")) else { return };
    let (e, what) = if a.finding.is_empty() {
        (decisions::entry("decided", &head, "", "", &a.text), "decision".to_string())
    } else {
        declined(b, &a.finding, &head, &a.text)
    };
    if !decisions::append(b, &[e]) {
        die("nothing recorded");
    }
    println!("recorded on {b} at {}: {what}: {}", short(&head), a.text);
}

/// Soft while a live step holds <b>'s running marker: it was handed the
/// record as it stood.
fn in_flight(b: &str) -> Gate {
    match running::lease(".", b).filter(|l| l.live) {
        Some(l) => Gate::Soft(vec![format!(
            "{} is running on {b} (pid {}, since {}), handed the decisions as they stood",
            l.step, l.pid, l.since
        )]),
        None => Gate::Go,
    }
}

/// The findings record's finding <id> (1-based, as review-fix numbers
/// them) declined at <head> for <why>, and the receipt's words for it.
fn declined(b: &str, id: &str, head: &str, why: &str) -> (Value, String) {
    let rec = pins::read(".", b, Pin::Findings).and_then(|r| one_value(&r));
    let fs = rec.and_then(|r| at(&r, "findings").as_array().cloned()).unwrap_or_default();
    if fs.is_empty() {
        die(format!("no review findings recorded on {b}: no finding {id} to decline"));
    }
    let n = id.parse::<usize>().ok().filter(|n| (1..=fs.len()).contains(n));
    let f = n.map_or_else(|| die(format!("no finding {id} on {b}: its findings are 1 to {}", fs.len())), |n| &fs[n - 1]);
    let e = decisions::of_finding("declined", head, f, why);
    let what = format!("declined finding {id} ({})", t(&e, "about"));
    (e, what)
}
