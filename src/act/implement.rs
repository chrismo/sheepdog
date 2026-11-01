// implement (J0) and its review-fix rounds: the agent works the recorded
// task, or the local review's open findings about HEAD, in the branch's
// session; the pipeline commits its edits, then runs verify. Whether it runs
// is gates::implement's, which NEXT asks too (#348): the task only before the
// branch's first commit (#241), a fix round only below settings.reviewRounds.

use super::commands::status;
use super::editing::{guidance, run_editing_step, Outcome, Spec};
use super::step::{agent_rules, answer_cmd, commit_edits, guidance_base, enter_worktree, question_ask, question_gate, question_say,
                  spend_worded, step_session};
use super::gates::{self, imp};
use super::{die, enforce, plural, state_json, Args};
use crate::git::{git, git_out};
use crate::next::Derived;
use crate::pins::{self, Pin};
use crate::val::{at, sj, t, text};
use crate::{decisions, json};
use serde_json::{Map, Value};

// The implement step's prompt: guidance (settings.implementPrompt, else
// .sheepdog/implement.md in the base commit, else lib/prompts/implement.md),
// then this contract, which the pipeline relies on. A fix round swaps in the
// review-fix guidance and contract.
const IMPLEMENT_CONTRACT: &str = r#"Edit files, but do not commit, push, or run gh: the pipeline commits your edits
with your commit_message, runs the repo's checks, and opens the PR later.

Return outcome "done" when the task is implemented, with a one-line
commit_message; "needs-input" when you need a decision from the task's author
(put the question in summary and leave your edits in place); or "gave-up" when
you can't make sense of it (say why in summary)."#;

const REVIEW_FIX_CONTRACT: &str = r#"For each finding below, pick exactly one action:
- fixed:    you changed code to address it
- declined: you judge it wrong, or it needs no change; say why
- ask:      it needs a decision from the branch's owner; say what the question is

Edit files, but do not commit, push, or run gh: the pipeline commits your edits
with your commit_message and runs the repo's checks. Return one item per
finding id (the number in brackets), with your reason or question as its
reply (for fixed, what you changed, or ""); a one-line commit_message if you changed anything (else null); and
outcome "done", "needs-input" (anything you marked ask: your edits are left
uncommitted for the owner) or "gave-up" (you can't make sense of them; say why
in summary). What you decline goes to the next review round, or to the owner
when you decline them all."#;

const DECIDED: &str = "A finding one of these settles you may decline, citing it, unless the change since that commit reopens it.";

const IMPLEMENT_SCHEMA: &str = r#"{"type": "object", "required": ["outcome", "commit_message", "summary"],
 "properties": {"outcome": {"enum": ["done", "needs-input", "gave-up"]},
                "commit_message": {"type": ["string", "null"]},
                "summary": {"type": "string"}}}"#;

// A fix round's schema: one item per finding id, in the vocabulary #133 gives
// every source's findings.
const REVIEW_FIX_SCHEMA: &str = r#"{"type": "object", "required": ["outcome", "commit_message", "summary", "items"],
 "properties": {
   "outcome": {"enum": ["done", "needs-input", "gave-up"]},
   "commit_message": {"type": ["string", "null"]},
   "summary": {"type": "string"},
   "items": {"type": "array", "items": {"type": "object", "required": ["id", "action", "reply"],
     "properties": {"id": {"type": "string"},
                    "action": {"enum": ["fixed", "declined", "ask"]},
                    "reply": {"type": "string"}}}}}}"#;

/// The findings record's findings, each with its id (its place in the list,
/// from 1) and open: it blocks and the agent hasn't declined it, so a fix
/// round hands it over. None for a record that isn't one.
fn numbered(rec: &Value) -> Option<Vec<Map<String, Value>>> {
    let findings = match at(rec, "findings") {
        Value::Null => return Some(vec![]),
        Value::Array(fs) => fs,
        _ => return None,
    };
    let number = |(i, f): (usize, &Value)| {
        let mut f = f.as_object()?.clone();
        let is = |k: &str, v: &str| f.get(k).and_then(Value::as_str) == Some(v);
        let open = !is("severity", "nit") && !is("action", "declined");
        f.insert("id".into(), Value::String((i + 1).to_string()));
        f.insert("open".into(), Value::Bool(open));
        Some(f)
    };
    findings.iter().enumerate().map(number).collect()
}

fn field(f: &Map<String, Value>, k: &str, none: &str) -> String {
    match f.get(k) {
        None | Some(Value::Null) => none.to_string(),
        Some(v) => text(v),
    }
}

/// No \S in it, as RE2 has \s: tab, newline, form feed, CR and space.
pub(super) fn blank(s: &str) -> bool {
    s.chars().all(|c| matches!(c, '\t' | '\n' | '\x0c' | '\r' | ' '))
}

fn is_open(f: &Map<String, Value>) -> bool {
    f.get("open") == Some(&Value::Bool(true))
}

/// The open findings, as the prompt lists them.
fn listed(fs: &[Map<String, Value>]) -> String {
    let line = |f: &Map<String, Value>| {
        format!(
            "- [{}] {}:{} ({}) {}",
            field(f, "id", "?"),
            field(f, "file", "?"),
            field(f, "line", "?"),
            field(f, "severity", "?"),
            field(f, "body", "")
        )
    };
    fs.iter().filter(|f| is_open(f)).map(line).collect::<Vec<_>>().join("\n")
}

/// The agent's answers on the numbered findings. An open finding takes its
/// item's action and reply; no answer, a decline or ask with no reply text,
/// or fixed with nothing <changed> is ask, so chrismo sees it. A fix needs
/// no reply text: the commit shows it.
fn answered(fs: &[Map<String, Value>], result: &Value, changed: bool) -> Vec<Map<String, Value>> {
    let no_items = vec![];
    let items = at(result, "items").as_array().unwrap_or(&no_items);
    let answer = |f: &Map<String, Value>| {
        let mut f = f.clone();
        if !is_open(&f) {
            return f;
        }
        let a = items.iter().find(|i| at(i, "id") == &f["id"]).unwrap_or(&Value::Null);
        let act = at(a, "action").as_str().unwrap_or("");
        let rep = at(a, "reply").as_str().unwrap_or("");
        let (action, reply) = match act {
            "fixed" if !changed => ("ask", format!("(said fixed but changed nothing): {rep}")),
            "fixed" => ("fixed", rep.to_string()),
            "declined" | "ask" if blank(rep) => ("ask", "(the agent gave no reply text)".to_string()),
            "declined" | "ask" => (act, rep.to_string()),
            _ => ("ask", "(the agent did not answer this one)".to_string()),
        };
        f.insert("action".into(), Value::String(action.into()));
        f.insert("reply".into(), Value::String(reply));
        f
    };
    fs.iter().map(answer).collect()
}

/// The findings the agent marked ask, or that count as ask, for chrismo.
fn asks(fs: &[Map<String, Value>]) -> String {
    let ask = |f: &&Map<String, Value>| is_open(f) && f.get("action").and_then(Value::as_str) == Some("ask");
    let line = |f: &Map<String, Value>| {
        format!("  [{}] {}:{} {}", field(f, "id", "?"), field(f, "file", "?"), field(f, "line", "?"), field(f, "reply", ""))
    };
    fs.iter().filter(ask).map(line).collect::<Vec<_>>().join("\n")
}

/// The answers, as printed: one line per finding the round handed over.
fn report(fs: &[Map<String, Value>]) -> String {
    let line = |f: &Map<String, Value>| {
        let a = field(f, "action", "");
        format!("  {a:<9} [{}] {}:{} {}", field(f, "id", "?"), field(f, "file", "?"), field(f, "line", "?"), field(f, "reply", ""))
    };
    fs.iter().filter(|f| is_open(f)).map(line).collect::<Vec<_>>().join("\n")
}

/// The record with the answered findings in place of its own, open dropped
/// (each keeps its id, action and reply), as one line.
fn answered_record(rec: &Value, fs: &[Map<String, Value>]) -> String {
    let mut rec = rec.as_object().cloned().unwrap_or_default();
    let findings = fs.iter().map(|f| {
        let mut f = f.clone();
        f.shift_remove("open");
        Value::Object(f)
    });
    rec.insert("findings".into(), Value::Array(findings.collect()));
    json::to_string(&Value::Object(rec)) + "\n"
}

/// The fix round's answers <fs> to the findings record <rec>, recorded in
/// it; each decline also goes on the decisions record at <head>, the HEAD
/// the round judged (#343).
fn record_answers(b: &str, rec: &Value, fs: &[Map<String, Value>], head: &str) {
    let blob = answered_record(rec, fs);
    if !pins::store(".", b, Pin::Findings, &blob, None).ok {
        die("couldn't record the agent's answers to the findings");
    }
    decisions::append(b, &declines(fs, head));
}

/// The decisions the round's answers <fs> make at <head>: its declines.
fn declines(fs: &[Map<String, Value>], head: &str) -> Vec<Value> {
    let declined = |f: &&Map<String, Value>| is_open(f) && f.get("action").and_then(Value::as_str) == Some("declined");
    let decision = |f: &Map<String, Value>| decisions::of_finding("declined", head, &Value::Object(f.clone()), &field(f, "reply", ""));
    fs.iter().filter(declined).map(decision).collect()
}

/// The agent's commit message, else for a fix round its own, else the task's
/// first line.
fn commit_message(result: &Value, fixing: bool, task: &str) -> String {
    match at(result, "commit_message").as_str() {
        Some(m) if !blank(m) => m.to_string(),
        _ if fixing => "Fix the local review findings".into(),
        _ => task.lines().next().unwrap_or("").to_string(),
    }
}

/// <why>, one of gates::implement's reasons, in record <s>'s words.
fn worded(why: &str, s: &Value) -> String {
    let b = t(s, "branch");
    let target = t(s, "sync.target");
    match why {
        imp::NO_TASK => format!("no task recorded for {b}: git config branch.{b}.sd-task \"<task>\""),
        imp::CAPPED => format!(
            "the review findings on {b} had their {} rounds (settings.reviewRounds): fix them by hand, raise it, or sd act review --accept",
            t(s, "review.findings.cap")
        ),
        imp::CLOSED => format!("PR #{} is {}", t(s, "pr.number"), text(at(s, "pr.state")).to_lowercase()),
        imp::ERRORS => format!("{why}: {}", sj(at(s, "errors"), "; ")),
        imp::AHEAD_UNKNOWN => format!("how far {b} is ahead of {target} is unknown"),
        imp::AHEAD => {
            let ahead = at(s, "sync.ahead").as_u64().unwrap_or(0) as usize;
            format!("nothing to implement: {b} is already {} ahead of {target}", plural(ahead, "commit"))
        }
        imp::REMOTE_MORE => format!("the remote branch has {} commit(s) {b} doesn't", t(s, "push.behind")),
        _ => why.into(),
    }
}

/// The findings record at <findings_ref>, numbered, and the record itself.
fn findings(b: &str, findings_ref: &str) -> (Vec<Map<String, Value>>, Option<Value>) {
    let rec = git(".", &["cat-file", "blob", findings_ref]);
    let r = rec.ok.then(|| crate::config::one_value(&rec.stdout)).flatten().filter(Value::is_object);
    match r.as_ref().and_then(numbered) {
        Some(fs) if fs.iter().any(is_open) => (fs, r),
        _ => die(format!("couldn't read the review findings on {b}")),
    }
}

fn implement_run(b: &str, wt: &str, a: &Args) {
    let answering = !a.answer.is_empty();
    if !answering && !status(wt).is_empty() {
        die(format!("{wt} has uncommitted changes"));
    }
    let task = git_out(".", &["config", "--get", &format!("branch.{b}.sd-task")]).unwrap_or_default();
    let s = state_json(wt, b).unwrap_or_else(|e| {
        eprint!("{e}");
        die("sd state failed")
    });
    let d = Derived::new(&s);
    let gate = spend_worded(gates::implement(&d).worded(|w| worded(w, &s)), &s, "implement", b);
    let Some(_go) = enforce(gate, a.force, &format!("implementing {b}")) else { return };
    let fixing = gates::implement_fixes(&d);
    let findings_ref = format!("refs/sheepdog/{b}/findings");
    let (fs, fix) = if fixing { findings(b, &findings_ref) } else { (vec![], None) };
    let qstep = if fixing { "review-fix" } else { "implement" };
    question_gate(b, qstep, a.force, answering);
    let verify = super::cfg("settings.verify", b);
    if !verify.is_empty() && agent_rules(b).is_empty() {
        eprintln!("sd act: warning: verify ({verify}) isn't plain words, so the agent may not run it; allow what it may run in settings.implementAllow");
    }
    let sess = step_session(b, "implement", &a.harness, answering);
    let base = guidance_base(b);
    let (prompt, schema) = if fixing {
        let g = guidance(b, &base, "reviewFixPrompt", "review-fix.md", "review-fix");
        let task = if task.is_empty() { "(none recorded)" } else { &task };
        let decided = decisions::section(&decisions::on_record(b), DECIDED);
        let list = listed(&fs);
        let p = format!("{g}\n\n{REVIEW_FIX_CONTRACT}\n\nFindings:\n{list}{decided}\n\nThe task this branch was made for, for context:\n{task}");
        (p, REVIEW_FIX_SCHEMA)
    } else {
        let g = guidance(b, &base, "implementPrompt", "implement.md", "implement");
        (format!("{g}\n\n{IMPLEMENT_CONTRACT}\n\nTask:\n{task}"), IMPLEMENT_SCHEMA)
    };
    let r = run_editing_step(Spec {
        b,
        wt,
        step: "implement",
        asks_as: qstep,
        session: &sess,
        base: &base,
        prompt,
        schema,
        done: "done",
        doing: format!("implementing {b}"),
        verify: &verify,
        answer: &a.answer,
        items: "",
    });
    let mut question = match &r.outcome {
        Outcome::Done => None,
        Outcome::NeedsInput(q) => Some(q.clone()),
        Outcome::GaveUp(why) => r.give_up(why),
        Outcome::Other(w) => r.unknown(w),
    };
    let changed = r.dirty() || r.moved();
    // A fix round's answers: any ask goes to chrismo, as needs-input does.
    let answers = answered(&fs, &r.result, changed);
    let asked = asks(&answers);
    if fixing && !asked.is_empty() {
        question = Some(format!("{}\n{asked}", question.as_deref().unwrap_or(&r.summary)));
    }
    if let Some(q) = question {
        needs_input(b, wt, qstep, &q, r.dirty());
    }
    let summary = &r.summary;
    if r.dirty() {
        commit_edits(b, "implement", wt, &commit_message(&r.result, fixing, &task));
    } else if !changed && !fixing {
        r.changed_nothing("done");
    }
    if let Some(rec) = &fix {
        record_answers(b, rec, &answers, &r.before);
    }
    if !changed {
        println!("{b}: the agent declined the review findings: {summary}");
        println!("{}", report(&answers));
        return;
    }
    println!("{b}: {summary}");
    if fixing {
        println!("{}", report(&answers));
    }
    r.print_log();
    if let Err(cmd) = r.verify() {
        die(format!("verify failed ({cmd}); the commit is kept in {wt}"));
    }
}

/// The agent needs input on <b>: <q>, recorded as <qstep>'s question.
/// implement refuses a dirty worktree, so with <edits> left an answer in the
/// session itself is chrismo's to commit; sd act answer goes on from them.
fn needs_input(b: &str, wt: &str, qstep: &str, q: &str, edits: bool) -> ! {
    question_ask(b, wt, qstep, q);
    if edits {
        die(format!("the agent needs input on {b}: {q}\n{}. Its edits are left uncommitted in {wt}: answer with {}, which goes on from them; or in its session (sd act attach {b}) and commit what you keep; or discard them and run sd act implement again", question_say(b), answer_cmd(b)));
    }
    die(format!("the agent needs input on {b}: {q}\n{}: answer with {}; or answer in its session (sd act attach {b}), then sd act ack --question and sd act implement", question_say(b), answer_cmd(b)))
}

pub fn implement(b: &str, a: &Args) {
    implement_run(b, &enter_worktree(b), a);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn findings(v: Value) -> Vec<Map<String, Value>> {
        numbered(&json!({"head": "h", "findings": v})).unwrap()
    }

    #[test]
    fn numbered_ids_each_finding_and_opens_what_blocks_and_isnt_declined() {
        let fs = findings(json!([
            {"file": "a", "line": 1, "severity": "warning", "body": "x"},
            {"file": "a", "line": 2, "severity": "nit", "body": "y"},
            {"file": "a", "line": 3, "severity": "error", "body": "z", "action": "declined", "reply": "no"},
            {"body": "w"}
        ]));
        let got: Vec<(String, bool)> = fs.iter().map(|f| (t(&Value::Object(f.clone()), "id"), is_open(f))).collect();
        assert_eq!(got, [("1".into(), true), ("2".into(), false), ("3".into(), false), ("4".into(), true)]);
        assert_eq!(numbered(&json!({"head": "h"})), Some(vec![]));
        assert_eq!(numbered(&json!({"findings": "lots"})), None);
        assert_eq!(numbered(&json!({"findings": ["a string"]})), None);
    }

    #[test]
    fn listed_shows_only_the_open_ones_missing_fields_as_question_marks() {
        let fs = findings(json!([
            {"file": "file.txt", "line": 2, "severity": "error", "body": "b should be B"},
            {"file": "file.txt", "line": 1, "severity": "nit", "body": "a could be A"},
            {"file": "file.txt", "body": "b?"}
        ]));
        assert_eq!(listed(&fs), "- [1] file.txt:2 (error) b should be B\n- [3] file.txt:? (?) b?");
    }

    fn actions(fs: &[Map<String, Value>]) -> Vec<(String, String)> {
        fs.iter().map(|f| (field(f, "action", "-"), field(f, "reply", "-"))).collect()
    }

    fn pairs(p: &[(&str, &str)]) -> Vec<(String, String)> {
        p.iter().map(|(a, r)| (a.to_string(), r.to_string())).collect()
    }

    #[test]
    fn answered_takes_each_items_action_and_reply_and_what_isnt_one_is_ask() {
        let fs = findings(json!([{}, {}, {}, {}, {}, {"severity": "nit"}]));
        let r = json!({"items": [
            {"id": "1", "action": "fixed", "reply": ""},
            {"id": "2", "action": "declined", "reply": "on purpose"},
            {"id": "3", "action": "declined", "reply": " "},
            {"id": "4", "action": "maybe", "reply": "hm"},
            {"id": 5, "action": "fixed", "reply": "a number isn't the id"}
        ]});
        assert_eq!(
            actions(&answered(&fs, &r, true)),
            pairs(&[
                ("fixed", ""),
                ("declined", "on purpose"),
                ("ask", "(the agent gave no reply text)"),
                ("ask", "(the agent did not answer this one)"),
                ("ask", "(the agent did not answer this one)"),
                ("-", "-"),
            ])
        );
    }

    #[test]
    fn answered_fixed_with_nothing_changed_is_ask() {
        let fs = findings(json!([{}]));
        let r = json!({"items": [{"id": "1", "action": "fixed", "reply": "B now"}]});
        assert_eq!(actions(&answered(&fs, &r, false)), pairs(&[("ask", "(said fixed but changed nothing): B now")]));
        assert_eq!(actions(&answered(&fs, &json!({"items": "none"}), true)), pairs(&[("ask", "(the agent did not answer this one)")]));
    }

    #[test]
    fn asks_and_report_list_the_open_answers() {
        let fs = findings(json!([
            {"file": "file.txt", "line": 1, "severity": "warning"},
            {"file": "file.txt", "line": 2, "severity": "error"},
            {"file": "file.txt", "line": 3, "severity": "nit"}
        ]));
        let r = json!({"items": [{"id": "1", "action": "ask", "reply": "A or a?"}, {"id": "2", "action": "declined", "reply": "b is right"}]});
        let a = answered(&fs, &r, true);
        assert_eq!(asks(&a), "  [1] file.txt:1 A or a?");
        assert_eq!(report(&a), "  ask       [1] file.txt:1 A or a?\n  declined  [2] file.txt:2 b is right");
    }

    #[test]
    fn answered_record_keeps_the_record_and_each_findings_id_and_answer() {
        let rec = json!({"head": "h", "base": "b", "round": 2, "findings": [{"file": "f", "severity": "error"}]});
        let fs = numbered(&rec).unwrap();
        let a = answered(&fs, &json!({"items": [{"id": "1", "action": "fixed", "reply": "done"}]}), true);
        assert_eq!(
            answered_record(&rec, &a),
            "{\"head\":\"h\",\"base\":\"b\",\"round\":2,\"findings\":[{\"file\":\"f\",\"severity\":\"error\",\"id\":\"1\",\"action\":\"fixed\",\"reply\":\"done\"}]}\n"
        );
    }

    #[test]
    fn commit_message_is_the_agents_else_the_rounds_else_the_tasks_first_line() {
        assert_eq!(commit_message(&json!({"commit_message": "Add c"}), true, "t"), "Add c");
        assert_eq!(commit_message(&json!({"commit_message": " "}), true, "t"), "Fix the local review findings");
        assert_eq!(commit_message(&json!({"commit_message": null}), false, "add a line\n\nmore"), "add a line");
        assert_eq!(commit_message(&json!({}), false, ""), "");
    }
}
