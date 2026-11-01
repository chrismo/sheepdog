// The decisions record (#343): refs/sheepdog/<b>/decisions, JSON lines
// {kind, head, about, finding, text, at}, appended and never rewritten: a
// finding review-fix declined or review --accept left standing, chrismo's
// answer to a step's question, bot feedback address answered on the PR. The
// local reviewer and the review-fix round read it so a settled finding isn't
// raised again (#240); they get these facts, never the authoring session (#79).

use crate::git::{git, git_input, oid, strip_nl};
use crate::val::{at, t, text};
use crate::{json, time};
use serde_json::{json, Value};

fn name(b: &str) -> String {
    format!("refs/sheepdog/{b}/decisions")
}

/// One decision of <kind> (declined, accepted, answered, bot-<action>) made
/// at <head>: <about> is where (file:line, or the step that asked),
/// <finding> what was raised or asked, <text> why, or the answer or reply.
pub fn entry(kind: &str, head: &str, about: &str, finding: &str, text: &str) -> Value {
    json!({"kind": kind, "head": head, "about": about, "finding": finding, "text": text})
}

/// A review finding's decision: its file:line and body.
pub fn of_finding(kind: &str, head: &str, f: &Value, why: &str) -> Value {
    let part = |k: &str| match at(f, k) {
        Value::Null => "?".to_string(),
        v => text(v),
    };
    entry(kind, head, &format!("{}:{}", part("file"), part("line")), &t(f, "body"), why)
}

/// <entries> after <prev>, each stamped <now>, as the record's blob.
fn appended(prev: &str, entries: &[Value], now: &str) -> String {
    let mut blob: String = prev.lines().filter(|l| !l.trim().is_empty()).map(|l| format!("{l}\n")).collect();
    for e in entries {
        let mut e = e.clone();
        e["at"] = json!(now);
        blob += &(json::to_string(&e) + "\n");
    }
    blob
}

/// Append <entries> to <b>'s record. Another writer moving the ref between
/// the read and the write is retried; a failure only warns.
pub fn append(b: &str, entries: &[Value]) {
    if entries.is_empty() {
        return;
    }
    let (r, now) = (name(b), time::now_utc().0);
    for _ in 0..3 {
        let old = oid(".", &r);
        let prev = if old.is_empty() { String::new() } else { git(".", &["cat-file", "blob", &old]).stdout };
        let blob = appended(&prev, entries, &now);
        let obj = git_input(".", &["hash-object", "-w", "--stdin"], Some(blob.as_bytes()));
        if !obj.ok {
            break;
        }
        if git(".", &["update-ref", "-m", "sd: decisions", &r, &strip_nl(obj.stdout), &old]).ok {
            return;
        }
    }
    eprintln!("sd: warning: couldn't record the decisions on {b}, so later review rounds won't see them");
}

const LONGEST: usize = 600;

/// <s> on one bullet's lines: past the first, indented; past LONGEST
/// characters, cut.
fn indented(s: &str) -> String {
    let s = s.trim();
    let mut cut: String = s.chars().take(LONGEST).collect();
    if cut.len() < s.len() {
        cut += "…";
    }
    cut.lines().collect::<Vec<_>>().join("\n    ")
}

/// One decision as the prompts list it.
fn line(e: &Value) -> String {
    let head: String = t(e, "head").chars().take(7).collect();
    let (kind, about) = (t(e, "kind"), t(e, "about"));
    let (finding, said) = (indented(&t(e, "finding")), indented(&text(at(e, "text"))));
    let (what, label) = match kind.as_str() {
        "declined" => (format!("declined, {about}"), "why"),
        "accepted" => (format!("accepted by chrismo, {about}"), "why"),
        "answered" => (format!("chrismo answered the {about} step's question"), "answer"),
        k => match k.strip_prefix("bot-") {
            Some(action) => (format!("bot feedback, {about}, {action}"), "reply"),
            None => (format!("{k}, {about}"), "text"),
        },
    };
    let mut l = format!("- [{head}] {what}: {finding}");
    if !said.is_empty() {
        l += &format!("\n  {label}: {said}");
    }
    l
}

/// <blob>'s decisions as the prompts list them, one bullet each, skipping
/// lines that aren't one; "" for none.
fn listed(blob: &str) -> String {
    let decisions = blob.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(Value::is_object);
    decisions.map(|e| line(&e)).collect::<Vec<_>>().join("\n")
}

/// <b>'s decisions on record, as the prompts list them; "" for none.
pub fn on_record(b: &str) -> String {
    let o = git(".", &["cat-file", "blob", &name(b)]);
    if o.ok {
        listed(&o.stdout)
    } else {
        String::new()
    }
}

/// <listed> decisions under the prompts' heading, then <ask>, led by a
/// blank line; "" for none.
pub fn section(listed: &str, ask: &str) -> String {
    let listed = listed.trim_end();
    if listed.trim().is_empty() {
        return String::new();
    }
    format!("\n\nDecisions on record, made on this branch since its task (each at the commit in brackets):\n{listed}\n\n{ask}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appended_keeps_every_earlier_line_and_stamps_only_the_new_ones() {
        let one = appended("", &[entry("declined", "h1", "f:1", "x?", "on purpose")], "T0");
        assert_eq!(one, "{\"kind\":\"declined\",\"head\":\"h1\",\"about\":\"f:1\",\"finding\":\"x?\",\"text\":\"on purpose\",\"at\":\"T0\"}\n");
        let two = appended(one.trim_end(), &[entry("answered", "h2", "implement", "A or B?", "B")], "T1");
        assert_eq!(two.lines().count(), 2);
        assert!(two.starts_with(&one));
        assert!(two.ends_with("\"at\":\"T1\"}\n"));
    }

    #[test]
    fn of_finding_is_its_file_line_and_body_missing_ones_as_question_marks() {
        let e = of_finding("accepted", "h", &json!({"file": "a.rs", "line": 3, "severity": "error", "body": "c?"}), "");
        assert_eq!(e, entry("accepted", "h", "a.rs:3", "c?", ""));
        assert_eq!(t(&of_finding("declined", "h", &json!({"body": "b"}), "no"), "about"), "?:?");
    }

    #[test]
    fn listed_is_one_bullet_per_decision_with_its_short_head_and_what_was_said() {
        let blob = appended(
            "",
            &[
                entry("declined", "1234567890", "a.rs:3", "c should be C", "c is lowercase on purpose"),
                entry("accepted", "abcdef0123", "a.rs:9", "unused?", ""),
                entry("answered", "abcdef0123", "review-fix", "A or a?", "a, as the task says"),
                entry("bot-pushback", "fedcba9876", "b.rs:1", "rename it", "the name is the issue's"),
            ],
            "T",
        );
        assert_eq!(
            listed(&blob),
            "- [1234567] declined, a.rs:3: c should be C\n  why: c is lowercase on purpose\n\
             - [abcdef0] accepted by chrismo, a.rs:9: unused?\n\
             - [abcdef0] chrismo answered the review-fix step's question: A or a?\n  answer: a, as the task says\n\
             - [fedcba9] bot feedback, b.rs:1, pushback: rename it\n  reply: the name is the issue's"
        );
    }

    #[test]
    fn listed_skips_lines_that_arent_decisions_and_indents_long_text() {
        let blob = format!("nope\n[1]\n{}", appended("", &[entry("bot-fixed", "h", "PR comment", "first\nsecond", "")], "T"));
        assert_eq!(listed(&blob), "- [h] bot feedback, PR comment, fixed: first\n    second");
        assert_eq!(listed(""), "");
        assert_eq!(section(" \n", "ask"), "");
        assert_eq!(section("- [h] x\n", "ask"), "\n\nDecisions on record, made on this branch since its task (each at the commit in brackets):\n- [h] x\n\nask");
        let long = listed(&appended("", &[entry("declined", "h", "f:1", &"x".repeat(700), "y")], "T"));
        assert!(long.contains(&format!("{}…\n  why: y", "x".repeat(LONGEST))));
    }
}
