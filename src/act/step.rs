// What every agent step shares: the branch's session, the commit its repo
// guidance is read from, what the agent may run, and the step's question for
// chrismo (#167).

use super::commands::step_timeout;
use super::start::start_default;
use super::{attached_check, cfg, die, lock_recheck, or_die, repo_remote};
use crate::git::{git, git_out, main_worktree, rev, show_ref, worktree_for, worktree_mark};
use crate::pins::{self, Pin};
use crate::val::{at, on, t};
use crate::{config, decisions, harness, json, next, rules, running, state, usage};
use serde_json::{json, Value};

/// <run> in <b>'s worktree, given its path, marked running as <step> for
/// all of it, commit and verify included, so NEXT doesn't hand the worktree
/// on mid-step (#170).
pub fn in_worktree(b: &str, step: &str, run: impl FnOnce(&str)) {
    let wt = worktree_for(".", b);
    if wt.is_empty() {
        die(format!("{b} isn't checked out in any worktree"));
    }
    if let Err(e) = std::env::set_current_dir(&wt) {
        die(format!("can't work in {wt}: {e}"));
    }
    if running::holding(b, step, || run(&wt)).is_none() {
        std::process::exit(1);
    }
}

/// The harness and session a step runs in: step_session's STEP_*.
pub struct Session {
    /// sd config step's object for the step.
    pub cfg: Value,
    pub harness: String,
    /// Minted for the step.
    pub id: String,
    /// new or fork.
    pub mode: &'static str,
    /// The session a fork starts from; "" for a new one.
    pub parent: String,
    pub timeout: u64,
}

impl Session {
    /// The next step's session: a fork of <ran>, the session the last one
    /// ran in.
    pub fn fork_of(&mut self, ran: String) {
        (self.id, self.mode, self.parent) = (harness::new_session(), "fork", ran);
    }
}

fn branch_config(b: &str, key: &str) -> String {
    git_out(".", &["config", "--get", &format!("branch.{b}.{key}")]).unwrap_or_default()
}

/// A fork of the branch's session (branch.<b>.sd-session), else a new one
/// (#69): the branch's session isn't touched, so a failed step leaves
/// nothing in it for a rerun to inherit. Refuses while the branch is
/// attached: the session is open in a TUI, and a fork misses what it hasn't
/// written yet (#14). <harness> is --harness's, else the branch's own.
pub fn step_session(b: &str, step: &str, harness: &str, force: bool, answering: bool) -> Session {
    attached_check(b, &format!("running the {step} step on {b}"), force);
    spend_gate(b, step);
    let stored_h = branch_config(b, "sd-harness");
    let cfg = or_die(config::step(step, b, if harness.is_empty() { &stored_h } else { harness }));
    let h = t(&cfg, "harness");
    let sid = branch_config(b, "sd-session");
    let forking = forks(b, &stored_h, &h, &sid, answering).unwrap_or_else(|e| die(e));
    // An answer goes to the session that asked, however big it is.
    let (mode, parent) = if forking && (answering || !past_fork_limit(b, &h, &sid)) {
        ("fork", sid)
    } else {
        ("new", String::new())
    };
    Session { cfg, harness: h, id: harness::new_session(), mode, parent, timeout: step_timeout(b) }
}

/// Refuse, at zero tokens, an agent step on <b> while NEXT's budget step
/// holds it (#121, #349): past settings.branchBudget, or with spend it
/// can't tell. --force doesn't override it; raising the budget does.
pub fn spend_gate(b: &str, step: &str) {
    if let Some(why) = spend_held(b) {
        die(format!("not running {step} on {b}: {why}"));
    }
}

/// next::budget_hold of <b>'s spend as sd state reports it.
pub fn spend_held(b: &str) -> Option<String> {
    let budget = or_die(config::get_text("settings.branchBudget", b, true));
    let budget: Value = serde_json::from_str(&budget).unwrap_or(Value::Null);
    let spend = match usage::spend(&main_worktree("."), b) {
        Some(s) => json!({"usd": s.usd, "unknown": s.unknown, "budget": budget}),
        None => json!({"usd": null, "unknown": null, "budget": budget}),
    };
    next::budget_hold(&json!({"spend": spend}))
}

/// Whether a step on <h> forks the branch's session <sid>, which belongs
/// to <stored_h> (claude when unset); Err when it can't run at all.
fn forks(b: &str, stored_h: &str, h: &str, sid: &str, answering: bool) -> Result<bool, String> {
    if sid.is_empty() {
        return match answering {
            true => Err(format!("no session on {b} to answer in (branch.{b}.sd-session)")),
            false => Ok(false),
        };
    }
    let owner = if stored_h.is_empty() { "claude" } else { stored_h };
    if h != owner {
        return Err(format!("branch.{b}.sd-session belongs to {owner}; unset it to start a {h} session"));
    }
    Ok(true)
}

/// The session's context (its last turn's tokens, harness::tally) is past
/// settings.forkLimit, so a step starts a new session instead of forking
/// it (#121). Every step's prompt is self-contained (task, contract, findings
/// or logs), so the history is only an optimization, and past the limit it
/// stops paying for itself.
fn past_fork_limit(b: &str, h: &str, sid: &str) -> bool {
    let limit: i64 = cfg("settings.forkLimit", b).parse().unwrap_or(0);
    if limit <= 0 {
        return false;
    }
    let Some(ctx) = over(limit, &harness::tally(h, sid)) else { return false };
    eprintln!("sd act: session {sid} is {ctx} tokens, past settings.forkLimit ({limit}): starting a new one");
    true
}

/// The tally's context when it's past <limit>; 0 is no limit, and no
/// transcript, or none that says, is resumed.
fn over(limit: i64, tally: &Value) -> Option<i64> {
    at(tally, "context").as_i64().filter(|ctx| limit > 0 && *ctx > limit)
}

pub(super) fn must(args: &[&str]) {
    let o = git(".", args);
    if !o.ok {
        eprint!("{}", o.stderr);
        std::process::exit(o.code);
    }
}

/// The agent's edits in <wt>, committed with <msg>, unless <b> was attached
/// during <step> (#270).
pub(super) fn commit_edits(b: &str, step: &str, wt: &str, msg: &str) {
    lock_recheck(b, step, "committing", &format!("the edits are left in {wt}"));
    for args in [&["add", "-A"][..], &["commit", "-q", "-m", msg]] {
        let g = git(wt, args);
        print!("{}", g.stdout);
        eprint!("{}", g.stderr);
        if !g.ok {
            die(format!("commit failed; the edits are left in {wt}"));
        }
    }
}

/// After a step returned a usable result, the branch's session is <ran>,
/// the one it ran in, and its harness the step's. A failed step's session
/// stays on disk, and the branch's where it was (#69).
pub fn step_store(b: &str, s: &Session, ran: &str) {
    must(&["config", &format!("branch.{b}.sd-session"), ran]);
    must(&["config", &format!("branch.{b}.sd-harness"), &s.harness]);
}

/// The commit a step reads repo guidance (.sheepdog/, relative prompt paths)
/// from, which the branch's agent can't rewrite: the base pin, else where
/// the branch forked from the default branch, the remote's or else the local
/// one, unless the branch is the default branch: sheepdog pushes it, so
/// neither ref is clear of its commits. Empty when none qualifies: the
/// branch's own commits are never trusted for its guidance.
pub fn guidance_base(b: &str) -> String {
    if let Some(base) = git_out(".", &["rev-parse", "-q", "--verify", &format!("refs/sheepdog/{b}/base")]) {
        return base;
    }
    let remote = repo_remote();
    let Some(def) = start_default(&remote).filter(|d| d != b) else { return String::new() };
    let head = format!("refs/heads/{b}");
    [format!("refs/remotes/{remote}/{def}"), format!("refs/heads/{def}")]
        .iter()
        .filter(|r| show_ref(".", r))
        .find_map(|r| git_out(".", &["merge-base", &head, r]))
        .unwrap_or_default()
}

/// rules::agent_rules from <b>'s settings.verify and settings.implementAllow.
pub fn agent_rules(b: &str) -> Vec<String> {
    rules::agent_rules(&cfg("settings.verify", b), &cfg("settings.implementAllow", b))
}

// ── an agent step's question (#167) ──────────────────────────────────────────

/// refs/sheepdog/<b>/question's blob: what a step asked, at the HEAD and
/// worktree mark it left. Control characters other than tab, newline and CR
/// are dropped, as are trailing newlines. A give-up (#254) has kind
/// "gave-up"; a question has no kind. A resolve's give-up (#257) also keys
/// on the ref it rebased onto, at its SHA.
fn question_record(head: &str, step: &str, mark: &str, text: &str, kind: &str, onto: Option<(&str, &str)>) -> String {
    let keep = |c: &char| !c.is_ascii_control() || matches!(c, '\t' | '\n' | '\r');
    let text: String = text.chars().filter(keep).collect();
    let mut rec = json!({"head": head, "step": step, "mark": mark, "text": text.trim_end_matches('\n')});
    if !kind.is_empty() {
        rec["kind"] = json!(kind);
    }
    if let Some((r, sha)) = onto {
        rec["onto_ref"] = json!(r);
        rec["onto"] = json!(sha);
    }
    json::to_string(&rec) + "\n"
}

/// Record a step's needs-input on HEAD and the worktree as the step left
/// them; a failure only warns, as the question is printed anyway.
pub fn question_ask(b: &str, wt: &str, step: &str, text: &str) {
    record_question(b, wt, step, text, "", None);
}

/// The agent stopped without doing the step: it gave up, or changed nothing
/// it said it had. Recorded as a question of kind gave-up (#254); dies with
/// <msg>.
pub fn give_up(b: &str, wt: &str, step: &str, text: &str, msg: String) -> ! {
    if record_question(b, wt, step, text, "gave-up", None) {
        die(format!("{msg}\n{}", question_say(b)))
    }
    die(msg)
}

/// restack --resolve's give-up onto <target> (<remote>/<branch>) at <sha>
/// (#257).
pub fn resolve_gave_up(b: &str, wt: &str, text: &str, msg: String, target: &str, sha: &str) -> ! {
    if record_question(b, wt, "resolve", text, "gave-up", Some((&format!("refs/remotes/{target}"), sha))) {
        die(format!(
            "{msg}\nNEXT shows it until {b}'s HEAD or worktree, or {target}, changes, or sd act ack --question"
        ))
    }
    die(msg)
}

/// false, having warned, when it couldn't be recorded.
fn record_question(b: &str, wt: &str, step: &str, text: &str, kind: &str, onto: Option<(&str, &str)>) -> bool {
    let head = rev(".", &format!("refs/heads/{b}"));
    let recorded = worktree_mark(wt).filter(|_| !head.is_empty()).is_some_and(|mark| {
        let rec = question_record(&head, step, &mark, text, kind, onto);
        pins::store(".", b, Pin::Question, &rec, None).ok
    });
    if !recorded {
        eprintln!("sd act: warning: couldn't record the question on {b}, so NEXT won't show it");
    }
    recorded
}

pub fn question_say(b: &str) -> String {
    format!("NEXT shows the question until {b}'s HEAD or worktree changes, or sd act ack --question")
}

pub fn answer_cmd(b: &str) -> String {
    format!("sd act answer \"<your answer>\" {b}")
}

/// Refuse, at zero tokens, to rerun the step whose question stands; --force
/// runs it anyway, and sd act answer goes on from it.
pub fn question_gate(b: &str, step: &str, force: bool, answering: bool) {
    if answering {
        return;
    }
    let Ok(Some(q)) = state::question_state(".", b, &rev(".", &format!("refs/heads/{b}"))) else { return };
    if !on(&q, "at_head") || t(&q, "step") != step {
        return;
    }
    if force {
        eprintln!("sd act: warning: the {step} agent's question on {b} stands; running anyway");
        return;
    }
    let did = if t(&q, "kind") == "gave-up" { "gave up" } else { "asked" };
    let how = if step == "resolve" {
        "Resolve it by hand (sd act restack says how), or sd act ack --question to let it try again".to_string()
    } else {
        format!("Answer with {}; or answer in its session (sd act attach {b}), then sd act ack --question", answer_cmd(b))
    };
    die(format!("not running {step} on {b}: its agent {did}, and nothing has changed since:\n{}\n{how}", t(&q, "text")));
}

/// The prompt for a step resumed with chrismo's answer, then the items owed
/// now (address: bot items can come in while the question stands, and an
/// item left unanswered is asked again).
pub fn answer_prompt(answer: &str, items: &str) -> String {
    let mut p = format!(
        "chrismo answers the question you asked:\n\n{answer}\n\nGo on with the step you were on, from the worktree as you left it, and end with that step's JSON result, as before (every item again, for a step that answers items)."
    );
    if !items.is_empty() {
        p += &format!("\n\nThe items to answer now, any that came in since you asked among them:\n{items}");
    }
    p
}

/// Once chrismo's <answer> ("" for none) reached the agent, its question is
/// done, and a stored answer with it; a new one it asks is recorded anew.
/// The question and answer go on the decisions record (#343).
pub fn question_answered(b: &str, answer: &str) {
    if answer.is_empty() {
        return;
    }
    if let Some(rec) = pins::read(".", b, Pin::Question).and_then(|q| config::one_value(&q)) {
        decisions::append(b, &[decisions::entry("answered", &t(&rec, "head"), &t(&rec, "step"), &t(&rec, "text"), answer)]);
    }
    pins::delete(".", b, Pin::Question).or_exit();
    answer_drop(b);
}

// ── chrismo's answer, waiting on its step's gate (#326) ──────────────────────

/// refs/sheepdog/<b>/answer's blob: chrismo's answer, keyed to the
/// question record's blob <question>, so it never goes to a different question.
fn answer_record(question: &str, step: &str, text: &str) -> String {
    json::to_string(&json!({"question": question, "step": step, "text": text})) + "\n"
}

/// Store <a>'s answer to <b>'s standing <step> question, unless it's the
/// one stored already.
pub fn answer_store(b: &str, step: &str, a: &super::Args) {
    if a.stored {
        return;
    }
    let text = &a.answer;
    let q = pins::oid(".", b, Pin::Question);
    if q.is_empty() || !pins::store(".", b, Pin::Answer, &answer_record(&q, step, text), None).ok {
        die(format!("couldn't store the answer on {b}"));
    }
}

pub fn answer_drop(b: &str) {
    pins::delete(".", b, Pin::Answer);
}

/// <a> with the answer stored for <b>'s standing <step> question, when <a>
/// brings none: the step goes on with it once its gate clears.
pub fn with_stored_answer(b: &str, step: &str, a: &super::Args) -> Option<super::Args> {
    if !a.answer.is_empty() {
        return None;
    }
    let q = state::question_state(".", b, &rev(".", &format!("refs/heads/{b}"))).ok()??;
    let answer = at(&q, "answer").as_str().filter(|_| on(&q, "at_head") && t(&q, "step") == step)?.to_string();
    Some(super::Args { answer, stored: true, ..a.clone() })
}

/// Once <step>'s gate passed with the stored answer, say so.
pub fn going_on(step: &str, a: &super::Args) {
    if a.stored {
        eprintln!("sd act: going on with your stored answer to the {step} agent's question");
    }
}

/// What sd act answer says when <step>'s gate refuses <why> and the answer
/// waits till <once>. sd watch runs it only when a push or a CI run clears
/// the gate; <yours> is what neither does, chrismo's to clear.
pub fn answer_waits(b: &str, step: &str, why: &str, once: &str, yours: &[String], a: &super::Args) -> String {
    let still = if a.stored { "still " } else { "" };
    let then = if yours.is_empty() {
        format!(" (NEXT then offers {step}, so sd watch runs {step})")
    } else {
        format!("; nothing sd watch runs clears {}, so that's yours to clear", yours.join("; "))
    };
    format!("{step} refuses on {b} while {why}: your answer is {still}stored, and {step} goes on with it once {once}{then}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resumes_the_branchs_session_on_its_own_harness_else_a_new_one() {
        assert_eq!(forks("feat", "", "claude", "", false), Ok(false));
        assert_eq!(forks("feat", "", "claude", "sid", false), Ok(true));
        assert_eq!(forks("feat", "pi", "pi", "sid", true), Ok(true));
        assert_eq!(
            forks("feat", "", "pi", "sid", false),
            Err("branch.feat.sd-session belongs to claude; unset it to start a pi session".into())
        );
        assert_eq!(
            forks("feat", "pi", "claude", "sid", false),
            Err("branch.feat.sd-session belongs to pi; unset it to start a claude session".into())
        );
    }

    #[test]
    fn an_answer_needs_the_session_that_asked() {
        assert_eq!(forks("feat", "", "claude", "", true), Err("no session on feat to answer in (branch.feat.sd-session)".into()));
    }

    #[test]
    fn over_the_fork_limit_only_past_it_and_never_with_no_limit_or_no_context() {
        assert_eq!(over(100, &json!({"context": 101})), Some(101));
        assert_eq!(over(100, &json!({"context": 100})), None);
        assert_eq!(over(0, &json!({"context": 101})), None);
        assert_eq!(over(100, &json!({"context": null})), None);
        assert_eq!(over(100, &json!({})), None);
    }

    #[test]
    fn question_record_is_one_line_its_text_without_control_characters() {
        let rec = question_record("h1", "implement", "m1", "Which file?\u{1b}[31m\tA\r\n\u{0}or B?\n\n", "", None);
        assert_eq!(rec, "{\"head\":\"h1\",\"step\":\"implement\",\"mark\":\"m1\",\"text\":\"Which file?[31m\\tA\\r\\nor B?\"}\n");
        let v: Value = serde_json::from_str(&rec).unwrap();
        assert_eq!(t(&v, "text"), "Which file?[31m\tA\r\nor B?");
        let rec = question_record("h1", "address", "m1", "too hard", "gave-up", None);
        assert_eq!(rec, "{\"head\":\"h1\",\"step\":\"address\",\"mark\":\"m1\",\"text\":\"too hard\",\"kind\":\"gave-up\"}\n");
    }

    #[test]
    fn answer_prompt_is_the_answer_then_the_items_owed_now() {
        let p = answer_prompt("the one with two lines", "");
        assert!(p.starts_with("chrismo answers the question you asked:\n\nthe one with two lines\n\nGo on with the step"));
        assert!(p.ends_with("for a step that answers items)."));
        let p = answer_prompt("yes", "- [1] a");
        assert!(p.ends_with("items).\n\nThe items to answer now, any that came in since you asked among them:\n- [1] a"));
    }

    #[test]
    fn question_say_and_answer_cmd_name_the_branch() {
        assert_eq!(answer_cmd("feat"), "sd act answer \"<your answer>\" feat");
        assert!(question_say("feat").starts_with("NEXT shows the question until feat's HEAD"));
    }
}
