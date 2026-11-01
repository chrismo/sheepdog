// The run every editing step shares (#344, the first step of #133): implement
// and review-fix, address, ci-fix and restack --resolve. Each verb's gate,
// payload and what it does with the Outcome stay the verb's.

use super::commands::{status, verify_autofix};
use super::die;
use super::hold::hold_since;
use super::step::{agent_rules, answer_prompt, give_up, question_answered, step_store, Session};
use crate::git::{git, rev};
use crate::guidance::{lib_dir, step_guidance};
use crate::harness::{self, Ran, Step, PI_TOOLS};
use crate::val::t;
use serde_json::Value;

/// What an editing step's agent returned, parsed once. NeedsInput carries
/// its question, GaveUp its reason; Other is a word only some verbs have
/// (ci-fix's flaky, resolve's already-on-target), or one none has.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    Done,
    NeedsInput(String),
    GaveUp(String),
    Other(String),
}

impl Outcome {
    /// <result>'s outcome, where <done> is the word the verb's contract
    /// gives for done ("fixed" for ci-fix). pi has no schema enforcement,
    /// so a word outside the verb's schema can come back: it's Other.
    pub fn of(result: &Value, done: &str) -> Outcome {
        match t(result, "outcome").as_str() {
            o if o == done => Outcome::Done,
            "needs-input" => Outcome::NeedsInput(t(result, "summary")),
            "gave-up" => Outcome::GaveUp(t(result, "summary")),
            o => Outcome::Other(o.into()),
        }
    }
}

/// An outcome its verb has no word for, given up on: the question's text,
/// then the line that says so.
pub fn unknown(word: &str, summary: &str) -> (String, String) {
    (format!("returned outcome \"{word}\": {summary}"), format!("the agent returned outcome \"{word}\""))
}

/// <b>'s guidance for a step: <setting>, else .sheepdog/<file> in <base>,
/// else lib/prompts/<file>. Dies, naming <what>, when it can't be read.
pub fn guidance(b: &str, base: &str, setting: &str, file: &str, what: &str) -> String {
    step_guidance(setting, b, base, &lib_dir().join("prompts").join(file)).unwrap_or_else(|e| {
        eprintln!("{e}");
        die(format!("can't read the {what} guidance"))
    })
}

/// One editing step, as its verb sets it up past its gate.
pub struct Spec<'a> {
    pub b: &'a str,
    pub wt: &'a str,
    /// The step: usage.sup's, and the failure's word.
    pub step: &'a str,
    /// The step a question or give-up is recorded as: review-fix for a fix
    /// round, else the step.
    pub asks_as: &'a str,
    pub session: &'a Session,
    /// The commit the guidance was read from: hold_since's.
    pub base: &'a str,
    /// Guidance, contract and payload; an answer replaces it.
    pub prompt: String,
    pub schema: &'a str,
    /// The contract's word for done.
    pub done: &'a str,
    /// What the agent is doing, for the line that says so: "implementing feat".
    pub doing: String,
    /// settings.verify, named to the agent as what runs after it.
    pub verify: &'a str,
    /// chrismo's answer to the step's question; "" for none.
    pub answer: &'a str,
    /// What an answered step owes now (address's items); "" for none.
    pub items: &'a str,
}

impl Spec<'_> {
    /// Runs the agent on the prompt, with what it may run and the verify line
    /// after it: its last result and the session it ran in, or None when the
    /// step failed or returned nothing usable.
    pub fn agent(&self) -> Option<(Value, String)> {
        let s = self.session;
        let mut prompt = self.prompt.clone();
        if !self.verify.is_empty() {
            prompt += &format!("\n\nThe repo's checks, which the pipeline runs after you: {}", self.verify);
        }
        let (allow, said) = harness::tool_rules(&s.harness, &agent_rules(self.b));
        prompt += &format!("\n\n{said}");
        eprintln!("sd act: {} ({} session {}) is {}…", s.harness, s.mode, s.id, self.doing);
        let step = Step {
            harness: &s.harness,
            session: &s.id,
            mode: s.mode,
            parent: &s.parent,
            timeout: s.timeout,
            prompt: &prompt,
            schema: self.schema,
            settings: &s.cfg,
            name: self.step,
            branch: self.b,
            claude_allow: &allow,
            pi_tools: PI_TOOLS,
        };
        harness::run_step(&step).ok().and_then(Ran::last)
    }
}

/// What the agent returned, with where the branch was before and after it.
pub struct Returned<'a> {
    spec: Spec<'a>,
    pub outcome: Outcome,
    pub summary: String,
    pub result: Value,
    pub before: String,
    /// The branch as the agent left it: its own commits, if any.
    pub agent_head: String,
}

/// Run <spec>'s agent in the branch's session. Whatever it returns, its own
/// commits are held for chrismo if they need it; a step that failed dies,
/// leaving the branch's session where it was (#69). One that returned makes
/// its session the branch's, and an answer it took is done.
pub fn run_editing_step(mut spec: Spec<'_>) -> Returned<'_> {
    let head = format!("refs/heads/{}", spec.b);
    let before = rev(".", &head);
    if !spec.answer.is_empty() {
        spec.prompt = answer_prompt(spec.answer, spec.items);
    }
    let Some((result, ran)) = spec.agent() else {
        hold_since(spec.b, spec.base, &before);
        let s = spec.session;
        die(format!("the {} {} step failed or returned no usable result (session {})", s.harness, spec.step, s.id))
    };
    step_store(spec.b, spec.session, &ran);
    question_answered(spec.b, spec.answer);
    hold_since(spec.b, spec.base, &before);
    let agent_head = rev(".", &head);
    let (outcome, summary) = (Outcome::of(&result, spec.done), t(&result, "summary"));
    Returned { spec, outcome, summary, result, before, agent_head }
}

impl Returned<'_> {
    /// The agent gave up (#254): dies with <why>, its edits left.
    pub fn give_up(&self, why: &str) -> ! {
        let Spec { b, wt, asks_as, .. } = self.spec;
        give_up(b, wt, asks_as, why, format!("the agent gave up on {b}: {why}. Edits (if any) are left uncommitted in {wt}"))
    }

    /// The agent returned <word>, which the verb has no use for: given up on.
    pub fn unknown(&self, word: &str) -> ! {
        let Spec { b, wt, asks_as, .. } = self.spec;
        let (text, line) = unknown(word, &self.summary);
        give_up(b, wt, asks_as, &text, format!("{line}; edits (if any) are left uncommitted in {wt}"))
    }

    /// The agent said <said> (done, fixed) but changed nothing: a give-up.
    pub fn changed_nothing(&self, said: &str) -> ! {
        let Spec { b, wt, asks_as, session, .. } = self.spec;
        let text = format!("said {said} but changed nothing: {}", self.summary);
        give_up(b, wt, asks_as, &text, format!("the agent says {said} but changed nothing (session {})", session.id))
    }

    pub fn dirty(&self) -> bool {
        !status(self.spec.wt).is_empty()
    }

    /// The branch moved since the agent started: its own commits count.
    pub fn moved(&self) -> bool {
        rev(".", &format!("refs/heads/{}", self.spec.b)) != self.before
    }

    /// The commits since the agent ran, its and the pipeline's, as printed.
    pub fn print_log(&self) {
        let range = format!("{}..refs/heads/{}", self.before, self.spec.b);
        print!("{}", git(self.spec.wt, &["log", "--format=  %h %s", &range]).stdout);
    }

    /// After the pipeline committed: its commit held for chrismo if it needs
    /// it, then verify, autofix and verify again; Err is the verify command.
    pub fn verify(&self) -> Result<(), String> {
        let Spec { b, wt, step, base, .. } = self.spec;
        hold_since(b, base, &self.agent_head);
        verify_autofix(wt, b, step)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn outcome_of_reads_the_verbs_done_and_the_shared_words_and_anything_else_is_other() {
        let r = |o: &str| json!({"outcome": o, "summary": "why"});
        assert_eq!(Outcome::of(&r("done"), "done"), Outcome::Done);
        assert_eq!(Outcome::of(&r("fixed"), "fixed"), Outcome::Done);
        assert_eq!(Outcome::of(&r("needs-input"), "done"), Outcome::NeedsInput("why".into()));
        assert_eq!(Outcome::of(&r("gave-up"), "fixed"), Outcome::GaveUp("why".into()));
        assert_eq!(Outcome::of(&r("flaky"), "fixed"), Outcome::Other("flaky".into()));
        assert_eq!(Outcome::of(&r("done"), "fixed"), Outcome::Other("done".into()));
        assert_eq!(Outcome::of(&json!({}), "done"), Outcome::Other(String::new()));
    }

    #[test]
    fn unknown_names_the_word_the_agent_returned() {
        assert_eq!(
            unknown("maybe", "not sure"),
            ("returned outcome \"maybe\": not sure".into(), "the agent returned outcome \"maybe\"".into())
        );
    }
}
