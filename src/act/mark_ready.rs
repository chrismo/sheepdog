// mark-ready (#500): gh pr ready, behind NEXT's draft step's own gate, so
// the loop can take a draft out of draft. Zero tokens. Not "ready": that
// step is ready to merge.

use super::gates;
use super::{die, enforce, state_json, Gate};
use crate::next::Derived;
use crate::val::{at, on, sj, t};
use serde_json::Value;

fn gate(s: &Value) -> Gate {
    let number = t(s, "pr.number");
    if at(s, "pr").is_null() {
        let why = sj(at(s, "errors"), "; ");
        return Gate::Hard(format!("no PR for {}{}", t(s, "branch"), if why.is_empty() { why } else { format!(": {why}") }));
    }
    if !at(s, "pr.state").is_null() && t(s, "pr.state") != "OPEN" {
        return Gate::Hard(format!("PR #{number} is {}", t(s, "pr.state").to_lowercase()));
    }
    if !on(s, "pr.draft") {
        return Gate::Done(format!("PR #{number} is already ready for review"));
    }
    gates::mark_ready(&Derived::new(s))
}

pub fn mark_ready(b: &str, force: bool) {
    let s = state_json(".", b).unwrap_or_else(|e| {
        eprint!("{e}");
        die("sd state failed")
    });
    if enforce(gate(&s), force, &format!("marking {b}'s PR ready")).is_none() {
        return;
    }
    let number = t(&s, "pr.number");
    // GH_REPO, which act::run set, points gh at settings.remote's repo.
    let o = crate::gh::mark_pr_ready(&number);
    eprint!("{}", o.stderr);
    if !o.ok() {
        die(format!("gh pr ready failed for PR #{number}"));
    }
    println!("PR #{number} is ready for review");
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn gate_is_hard_with_no_pr_or_one_not_open_and_done_on_a_non_draft() {
        let s = |pr: Value| json!({"branch": "feat", "pr": pr, "errors": []});
        assert_eq!(gate(&s(Value::Null)), Gate::Hard("no PR for feat".into()));
        assert_eq!(gate(&s(json!({"number": 7, "state": "MERGED", "draft": true}))), Gate::Hard("PR #7 is merged".into()));
        assert_eq!(gate(&s(json!({"number": 7, "state": "OPEN", "draft": false}))), Gate::Done("PR #7 is already ready for review".into()));
        assert!(matches!(gate(&s(json!({"number": 7, "state": "OPEN", "draft": true}))), Gate::Soft(_)));
    }
}
