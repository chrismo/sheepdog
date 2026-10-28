// implement (J0) and its review-fix rounds: the agent works the recorded
// task, or the local review's open findings about HEAD, in the branch's
// session; the pipeline commits its edits, then runs verify. A branch already
// ahead of its target, with no findings to fix, is refused at zero tokens
// unless --force (#241).

use super::commands::{status, verify_autofix};
use super::hold::hold_since;
use super::step::{agent_rules, answer_cmd, answer_prompt, commit_edits, give_up, guidance_base, in_worktree,
                  question_answered, question_ask, question_gate, question_say, step_session, step_store};
use super::{die, plural, state_json, Args};
use crate::git::{git, git_input, git_out, rev, strip_nl};
use crate::guidance::{lib_dir, step_guidance};
use crate::harness::{self, Ran, Step, PI_TOOLS};
use crate::val::{at, on, t, text};
use crate::json;
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

/// The agent's commit message, else for a fix round its own, else the task's
/// first line.
fn commit_message(result: &Value, fixing: bool, task: &str) -> String {
    match at(result, "commit_message").as_str() {
        Some(m) if !blank(m) => m.to_string(),
        _ if fixing => "Fix the local review findings".into(),
        _ => task.lines().next().unwrap_or("").to_string(),
    }
}

fn implement_run(b: &str, wt: &str, a: &Args) {
    let answering = !a.answer.is_empty();
    if !answering && !status(wt).is_empty() {
        die(format!("{wt} has uncommitted changes"));
    }
    let task = git_out(".", &["config", "--get", &format!("branch.{b}.sd-task")]).unwrap_or_default();
    let head = format!("refs/heads/{b}");
    let before = rev(".", &head);
    // A fix round: sd act review's open findings, when they're about HEAD.
    // The task goes along as context. sd state decides whether they're about
    // HEAD, so NEXT and this agree.
    let findings_ref = format!("refs/sheepdog/{b}/findings");
    let rec = git(".", &["cat-file", "blob", &findings_ref]);
    let rec = if rec.ok { rec.stdout } else { String::new() };
    let mut fs: Vec<Map<String, Value>> = vec![];
    let mut fix: Option<Value> = None;
    let s = state_json(wt, b).unwrap_or_else(|e| {
        eprint!("{e}");
        die("sd state failed")
    });
    if !rec.is_empty() && on(&s, "review.findings.at_head") {
        let r = crate::config::one_value(&rec).filter(Value::is_object);
        fs = r.as_ref().and_then(numbered).unwrap_or_else(|| die(format!("couldn't read the review findings on {b}")));
        fix = r;
    }
    let list = listed(&fs);
    let fixing = !list.is_empty();
    let fix = fix.filter(|_| fixing);
    if task.is_empty() && !fixing {
        die(format!("no task recorded for {b}: git config branch.{b}.sd-task \"<task>\""));
    }
    let qstep = if fixing { "review-fix" } else { "implement" };
    question_gate(b, qstep, a.force, answering);
    // NEXT offers implement only with nothing ahead of the target (#241).
    let ahead = at(&s, "sync.ahead").as_u64().unwrap_or(0);
    if ahead > 0 && !fixing && !answering && !a.force {
        die(format!(
            "nothing to implement: {b} is already {} ahead of {}; --force to implement its task anyway",
            plural(ahead as usize, "commit"),
            t(&s, "sync.target")
        ));
    }
    let verify = super::cfg("settings.verify", b);
    let rules = agent_rules(b);
    if !verify.is_empty() && rules.is_empty() {
        eprintln!("sd act: warning: verify ({verify}) isn't plain words, so the agent may not run it; allow what it may run in settings.implementAllow");
    }
    let s = step_session(b, "implement", &a.harness, a.force, answering);
    let gbase = guidance_base(b);
    let lib = lib_dir();
    let guidance = |setting: &str, file: &str, what: &str| {
        step_guidance(setting, b, &gbase, &lib.join("prompts").join(file)).unwrap_or_else(|e| {
            eprintln!("{e}");
            die(format!("can't read the {what} guidance"))
        })
    };
    let (mut prompt, schema) = if fixing {
        let g = guidance("reviewFixPrompt", "review-fix.md", "review-fix");
        let task = if task.is_empty() { "(none recorded)" } else { &task };
        let p = format!("{g}\n\n{REVIEW_FIX_CONTRACT}\n\nFindings:\n{list}\n\nThe task this branch was made for, for context:\n{task}");
        (p, REVIEW_FIX_SCHEMA)
    } else {
        let g = guidance("implementPrompt", "implement.md", "implement");
        (format!("{g}\n\n{IMPLEMENT_CONTRACT}\n\nTask:\n{task}"), IMPLEMENT_SCHEMA)
    };
    if answering {
        prompt = answer_prompt(&a.answer, "");
    }
    if !verify.is_empty() {
        prompt += &format!("\n\nThe repo's checks, which the pipeline runs after you: {verify}");
    }
    let (allow, said) = harness::tool_rules(&s.harness, &rules);
    prompt += &format!("\n\n{said}");

    let (h, sid) = (s.harness.as_str(), s.id.as_str());
    eprintln!("sd act: {h} ({} session {sid}) is implementing {b}…", s.mode);
    let step = Step {
        harness: h,
        session: sid,
        mode: s.mode,
        parent: &s.parent,
        timeout: s.timeout,
        prompt: &prompt,
        schema,
        settings: &s.cfg,
        name: "implement",
        branch: b,
        claude_allow: &allow,
        pi_tools: PI_TOOLS,
    };
    let Some((result, ran)) = harness::run_step(&step).ok().and_then(Ran::last) else {
        hold_since(b, &gbase, &before);
        die(format!("the {h} implement step failed or returned no usable result (session {sid})"))
    };
    step_store(b, &s, &ran);
    question_answered(b, answering);
    hold_since(b, &gbase, &before);
    let agent_head = rev(".", &head);

    let mut outcome = t(&result, "outcome");
    let mut summary = t(&result, "summary");
    match outcome.as_str() {
        "done" | "needs-input" => {}
        "gave-up" => give_up(b, wt, qstep, &summary, format!("the agent gave up on {b}: {summary}. Edits (if any) are left uncommitted in {wt}")),
        _ => give_up(b, wt, qstep, &format!("returned outcome \"{outcome}\": {summary}"), format!("the agent returned outcome \"{outcome}\"; edits (if any) are left uncommitted in {wt}")),
    }
    let changed = !status(wt).is_empty() || rev(".", &head) != before;
    // A fix round's answers: any ask goes to chrismo, as needs-input does.
    let answers = answered(&fs, &result, changed);
    let asked = asks(&answers);
    if fixing && !asked.is_empty() {
        outcome = "needs-input".into();
        summary += &format!("\n{asked}");
    }
    if outcome == "needs-input" {
        question_ask(b, wt, qstep, &summary);
        // implement refuses a dirty worktree, so with edits left an answer in
        // the session itself is chrismo's to commit; sd act answer goes on
        // from them.
        if !status(wt).is_empty() {
            die(format!("the agent needs input on {b}: {summary}\n{}. Its edits are left uncommitted in {wt}: answer with {}, which goes on from them; or in its session (sd act attach {b}) and commit what you keep; or discard them and run sd act implement again", question_say(b), answer_cmd(b)));
        }
        die(format!("the agent needs input on {b}: {summary}\n{}: answer with {}; or answer in its session (sd act attach {b}), then sd act ack --question and sd act implement", question_say(b), answer_cmd(b)));
    }
    if !status(wt).is_empty() {
        commit_edits(b, "implement", wt, &commit_message(&result, fixing, &task));
    } else if !changed && !fixing {
        give_up(b, wt, qstep, &format!("said done but changed nothing: {summary}"), format!("the agent says done but changed nothing (session {sid})"));
    }
    if let Some(r) = &fix {
        let rec = answered_record(r, &answers);
        let obj = git_input(".", &["hash-object", "-w", "--stdin"], Some(rec.as_bytes()));
        if !obj.ok || !git(".", &["update-ref", "-m", "review findings answered", &findings_ref, &strip_nl(obj.stdout)]).ok {
            die("couldn't record the agent's answers to the findings");
        }
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
    let log = git(wt, &["log", "--format=  %h %s", &format!("{before}..{head}")]);
    print!("{}", log.stdout);
    hold_since(b, &gbase, &agent_head);
    if let Err(cmd) = verify_autofix(wt, b) {
        die(format!("verify failed ({cmd}); the commit is kept in {wt}"));
    }
}

pub fn implement(b: &str, a: &Args) {
    in_worktree(b, "implement", |wt| implement_run(b, wt, a));
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
