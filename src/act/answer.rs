// answer: chrismo's answer to an agent step's standing question (#178), with
// no TUI. The step that asked resumes the branch's session with the answer as
// its whole prompt (step::answer_prompt): the session holds the step's own
// prompt, and its edits are still in the worktree, which the step would
// otherwise refuse. The rest of the step (commit, verify, push, replies) runs
// as usual, under the running marker. Only a question that still stands.

use super::{address, ci_fix, die, implement, Args};
use crate::git::rev;
use crate::state::question_state;
use crate::val::{on, t};
use serde_json::Value;

#[derive(Debug, PartialEq)]
enum Resume {
    Implement,
    Address,
    CiFix,
}

/// The verb that goes on from question <q> on <b>, or why none can.
/// review-fix is a round of implement.
fn resumes(b: &str, q: &Value) -> Result<Resume, String> {
    let step = t(q, "step");
    if !on(q, "at_head") {
        return Err(format!(
            "the {step} agent's question on {b} no longer stands: HEAD or the worktree changed since it asked (sd act ack --question drops it)"
        ));
    }
    match step.as_str() {
        "implement" | "review-fix" => Ok(Resume::Implement),
        "address" => Ok(Resume::Address),
        "ci-fix" => Ok(Resume::CiFix),
        _ => Err(format!("can't answer a {step} question")),
    }
}

/// <a>.answer is the answer; --force, --no-publish and --harness go to the
/// step as given.
pub fn answer(b: &str, remote: &str, a: &Args) {
    let q = question_state(b, &rev(".", &format!("refs/heads/{b}")))
        .unwrap_or_else(|()| die(format!("refs/sheepdog/{b}/question isn't a question record")))
        .unwrap_or_else(|| die(format!("no question to answer on {b}")));
    match resumes(b, &q).unwrap_or_else(|e| die(e)) {
        Resume::Implement => implement::implement(b, a),
        Resume::Address => address::address(b, remote, a),
        Resume::CiFix => ci_fix::ci_fix(b, remote, a),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn q(step: &str, at_head: bool) -> Value {
        json!({"step": step, "sha": "h1", "text": "which?", "at_head": at_head})
    }

    #[test]
    fn resumes_the_step_that_asked_a_review_fix_round_as_implement() {
        assert_eq!(resumes("feat", &q("implement", true)), Ok(Resume::Implement));
        assert_eq!(resumes("feat", &q("review-fix", true)), Ok(Resume::Implement));
        assert_eq!(resumes("feat", &q("address", true)), Ok(Resume::Address));
        assert_eq!(resumes("feat", &q("ci-fix", true)), Ok(Resume::CiFix));
    }

    #[test]
    fn resumes_nothing_once_the_question_no_longer_stands() {
        let stale = "the ci-fix agent's question on feat no longer stands: HEAD or the worktree changed since it asked (sd act ack --question drops it)";
        assert_eq!(resumes("feat", &q("ci-fix", false)), Err(stale.into()));
        assert!(resumes("feat", &json!({"step": "implement"})).is_err());
    }

    #[test]
    fn resumes_no_step_that_cant_ask() {
        assert_eq!(resumes("feat", &q("describe", true)), Err("can't answer a describe question".into()));
    }
}
