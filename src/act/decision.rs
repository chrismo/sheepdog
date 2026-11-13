// decision (#454, #462): chrismo's decision on a branch, or with --finding
// his decline of one review finding, appended to the decisions record at
// HEAD (decisions.rs) for the review rounds after it. A decline also settles
// the finding on the findings record (#501), as a review-fix round's decline
// does, so NEXT hands it to no review-fix round again. Zero tokens: no
// agent, no worktree, no review pin or ack.

use super::review::short;
use super::{die, enforce, Args, Gate};
use crate::config::one_value;
use crate::git::{git, rev};
use crate::json;
use crate::pins::{self, Pin};
use crate::val::{at, t};
use crate::{decisions, running};
use serde_json::{json, Value};

pub fn decision(b: &str, a: &Args) {
    let head = rev(".", &format!("refs/heads/{b}"));
    if head.is_empty() {
        die(format!("no branch {b}"));
    }
    let Some(_go) = enforce(in_flight(b), a.force, &format!("recording a decision on {b}")) else { return };
    let (e, what, settle) = if a.finding.is_empty() {
        (decisions::entry("decided", &head, "", "", &a.text), "decision".to_string(), None)
    } else {
        declined(b, &a.finding, &head, &a.text)
    };
    if !decisions::append(b, &[e]) {
        die("nothing recorded");
    }
    if let Some((old, blob)) = settle {
        if !pins::store(".", b, Pin::Findings, &blob, Some(&old)).ok {
            eprintln!("sd: warning: couldn't mark the finding declined on {b}'s findings record, so NEXT may hand it to review-fix");
        }
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

/// The findings record's object and blob as the decline leaves it, written
/// only while the record is still that object.
type Settle = Option<(String, String)>;

/// The findings record's finding <id> (1-based, as review-fix numbers
/// them) declined at <head> for <why>, the receipt's words for it, and the
/// record with that finding settled.
fn declined(b: &str, id: &str, head: &str, why: &str) -> (Value, String, Settle) {
    let old = pins::oid(".", b, Pin::Findings);
    let blob = if old.is_empty() { String::new() } else { git(".", &["cat-file", "blob", &old]).stdout };
    let mut rec = one_value(&blob).unwrap_or(Value::Null);
    let fs = at(&rec, "findings").as_array().cloned().unwrap_or_default();
    if fs.is_empty() {
        die(format!("no review findings recorded on {b}: no finding {id} to decline"));
    }
    let n = id.parse::<usize>().ok().filter(|n| (1..=fs.len()).contains(n));
    let n = n.unwrap_or_else(|| die(format!("no finding {id} on {b}: its findings are 1 to {}", fs.len())));
    if let Some(was) = settled_already(&fs[n - 1]) {
        die(format!("finding {id} on {b} is {was} already: nothing to decline"));
    }
    let e =decisions::of_finding("declined", head, &fs[n - 1], why);
    let what = format!("declined finding {id} ({})", t(&e, "about"));
    let settle = settled(&mut rec["findings"][n - 1], why).then(|| (old, json::to_string(&rec) + "\n"));
    (e, what, settle)
}

/// How finding <f> was settled, if a decline would undo or repeat it: fixed,
/// or declined by chrismo. One the agent declined, his decline takes over.
fn settled_already(f: &Value) -> Option<&'static str> {
    match (at(f, "action").as_str(), at(f, "by").as_str()) {
        (Some("fixed"), _) => Some("fixed"),
        (Some("declined"), Some("chrismo")) => Some("declined by you"),
        _ => None,
    }
}

/// Finding <f> declined by chrismo for <why>; false for one that isn't an
/// object.
fn settled(f: &mut Value, why: &str) -> bool {
    let Some(f) = f.as_object_mut() else { return false };
    f.insert("action".into(), json!("declined"));
    f.insert("reply".into(), json!(why));
    f.insert("by".into(), json!("chrismo"));
    true
}
