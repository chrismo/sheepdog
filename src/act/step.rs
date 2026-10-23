// What every agent step shares: the branch's session, the commit its repo
// guidance is read from, what the agent may run, and the step's question for
// chrismo (#167).

use super::commands::step_timeout;
use super::start::start_default;
use super::{attached_check, cfg, die, or_die, repo_remote};
use crate::git::{git, git_input, git_out, rev, show_ref, strip_nl, worktree_for, worktree_mark};
use crate::val::{at, on, t};
use crate::{config, harness, json, running, state};
use regex::Regex;
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
    pub id: String,
    /// new or resume.
    pub mode: &'static str,
    pub timeout: u64,
}

fn branch_config(b: &str, key: &str) -> String {
    git_out(".", &["config", "--get", &format!("branch.{b}.{key}")]).unwrap_or_default()
}

/// The branch's session (branch.<b>.sd-session) resumed, else a new one.
/// Refuses while the branch is attached: the session is open in a TUI, and a
/// resume beside it forks the transcript (#14). <harness> is --harness's,
/// else the branch's own.
pub fn step_session(b: &str, step: &str, harness: &str, force: bool, answering: bool) -> Session {
    attached_check(b, &format!("running the {step} step on {b}"), force);
    let stored_h = branch_config(b, "sd-harness");
    let cfg = or_die(config::step(step, b, if harness.is_empty() { &stored_h } else { harness }));
    let h = t(&cfg, "harness");
    let sid = branch_config(b, "sd-session");
    let resuming = resumes(b, &stored_h, &h, &sid, answering).unwrap_or_else(|e| die(e));
    // An answer goes to the session that asked, however big it is.
    let (id, mode) = if resuming && (answering || !past_fork_limit(b, &h, &sid)) {
        (sid, "resume")
    } else {
        (harness::new_session(), "new")
    };
    Session { cfg, harness: h, id, mode, timeout: step_timeout(b) }
}

/// Whether a step on <h> resumes the branch's session <sid>, which belongs
/// to <stored_h> (claude when unset); Err when it can't run at all.
fn resumes(b: &str, stored_h: &str, h: &str, sid: &str, answering: bool) -> Result<bool, String> {
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
/// settings.forkLimit, so a step starts a new session instead of inheriting
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

/// The agent's edits in <wt>, committed with <msg>.
pub(super) fn commit_edits(wt: &str, msg: &str) {
    for args in [&["add", "-A"][..], &["commit", "-q", "-m", msg]] {
        let g = git(wt, args);
        print!("{}", g.stdout);
        eprint!("{}", g.stderr);
        if !g.ok {
            die(format!("commit failed; the edits are left in {wt}"));
        }
    }
}

/// After a step ran, remember its session and harness.
pub fn step_store(b: &str, s: &Session) {
    must(&["config", &format!("branch.{b}.sd-session"), &s.id]);
    must(&["config", &format!("branch.{b}.sd-harness"), &s.harness]);
}

/// step_store after a failure, only once the harness wrote the session's
/// transcript: resuming a session it never started fails every retry (#254).
pub fn step_store_failed(b: &str, s: &Session) {
    if harness::has_transcript(&s.harness, &s.id) {
        step_store(b, s);
    }
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

/// What the agent may run beyond reading and read-only git (Claude Code
/// permission rules): settings.verify when it's plain words (a rule's * is a
/// wildcard), and settings.implementAllow's.
pub fn agent_rules(b: &str) -> Vec<String> {
    let verify = cfg("settings.verify", b);
    let plain = Regex::new(r"^[A-Za-z0-9_./:=@+-]+( [A-Za-z0-9_./:=@+-]+)*$").expect("a fixed pattern compiles");
    let mut rules: Vec<String> = Vec::new();
    if plain.is_match(&verify) {
        rules.push(format!("Bash({verify})"));
    }
    rules.extend(cfg("settings.implementAllow", b).lines().filter(|r| !r.is_empty()).map(str::to_string));
    rules
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
        let obj = git_input(".", &["hash-object", "-w", "--stdin"], Some(rec.as_bytes()));
        let did = if kind == "gave-up" { "gave up" } else { "needs input" };
        let msg = format!("sd act {step}: {did}");
        obj.ok && git(".", &["update-ref", "-m", &msg, &format!("refs/sheepdog/{b}/question"), &strip_nl(obj.stdout)]).ok
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

/// Once the answer reached the agent, its question is done; a new one it
/// asks is recorded anew.
pub fn question_answered(b: &str, answering: bool) {
    if answering {
        must(&["update-ref", "-d", &format!("refs/sheepdog/{b}/question")]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resumes_the_branchs_session_on_its_own_harness_else_a_new_one() {
        assert_eq!(resumes("feat", "", "claude", "", false), Ok(false));
        assert_eq!(resumes("feat", "", "claude", "sid", false), Ok(true));
        assert_eq!(resumes("feat", "pi", "pi", "sid", true), Ok(true));
        assert_eq!(
            resumes("feat", "", "pi", "sid", false),
            Err("branch.feat.sd-session belongs to claude; unset it to start a pi session".into())
        );
        assert_eq!(
            resumes("feat", "pi", "claude", "sid", false),
            Err("branch.feat.sd-session belongs to pi; unset it to start a claude session".into())
        );
    }

    #[test]
    fn an_answer_needs_the_session_that_asked() {
        assert_eq!(resumes("feat", "", "claude", "", true), Err("no session on feat to answer in (branch.feat.sd-session)".into()));
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
