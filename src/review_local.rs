// sd review-local (src/review_local_help.txt, which holds the contract any
// script named by settings.localReview must keep).
//
// Harness, model and effort come from `sd config step review`. Every run is a
// new session (fresh context, never the authoring conversation) with read-only
// tools: no edits, no gh. The prompt is the guidance (settings.reviewPrompt: a
// path, relative ones read from SHEEPDOG_BASE so the branch can't rewrite its
// own review; empty means .sheepdog/review.md in the base commit, else
// lib/prompts/review.md), then the fixed reporting rules (lib/review-rules.md),
// the task, the decisions on record (SHEEPDOG_DECISIONS, #343), the commits
// (each one's full message, trailers dropped, #280) and the diff. Why a local
// review before the bot: #79 (Design record).

use crate::git::{self, git, strip_nl};
use crate::guidance::{self, step_guidance};
use crate::harness::{self, Ran, Step, READONLY_ALLOW, READONLY_PI_TOOLS};
use crate::val::t;
use crate::{config, json};
use regex::Regex;
use std::process::ExitCode;

const HELP: &str = include_str!("review_local_help.txt");

const SCHEMA: &str = r#"{"type": "object", "required": ["findings"],
 "properties": {"findings": {"type": "array", "items": {
   "type": "object", "required": ["file", "line", "severity", "body"],
   "properties": {"file": {"type": "string"}, "line": {"type": "integer"},
                  "severity": {"type": "string", "enum": ["error", "warning", "nit"]},
                  "body": {"type": "string"}}}}}}"#;

fn die(msg: impl std::fmt::Display) -> ! {
    eprintln!("sd review-local: {msg}");
    std::process::exit(harness::failed_status())
}

fn or_die<T>(r: Result<T, Vec<String>>) -> T {
    r.unwrap_or_else(|lines| {
        for l in lines {
            eprintln!("sd config: {l}");
        }
        die("sd config failed")
    })
}

/// git <args>' stdout, as $( ) gives it; on a failure, git's stderr shown and
/// <what> failed.
fn git_or_die(args: &[&str], what: &str) -> String {
    let o = git(".", args);
    if !o.ok {
        eprint!("{}", o.stderr);
        die(format!("{what} failed"));
    }
    strip_nl(o.stdout)
}

/// The commits <base>..<head>, newest first, each with its full message (#280).
fn commit_list(base: &str, head: &str) -> String {
    let range = format!("{base}..{head}");
    let log = git_or_die(&["log", "--format=%h%x1f%B%x1e", &range], &format!("git log {range}"));
    let commits: Vec<String> = log.split('\x1e').filter_map(|c| c.trim_start().split_once('\x1f')).map(|(sha, msg)| listed(sha, msg)).collect();
    commits.join("\n")
}

/// One commit as the prompt lists it: "- <sha> <subject>", then its body
/// indented, without its trailers.
fn listed(sha: &str, message: &str) -> String {
    let message = message.trim();
    let (subject, body) = message.split_once("\n\n").unwrap_or((message, ""));
    let mut out = format!("- {sha} {subject}");
    for l in without_trailers(body).lines() {
        out += &if l.is_empty() { "\n".to_string() } else { format!("\n    {l}") };
    }
    out
}

/// <body> without its last paragraph when that is all trailers
/// (Co-Authored-By: and the like).
fn without_trailers(body: &str) -> &str {
    let (rest, last) = body.rsplit_once("\n\n").unwrap_or(("", body));
    let trailer = Regex::new(r"^[A-Za-z0-9-]+: ").expect("a fixed pattern compiles");
    if last.lines().all(|l| trailer.is_match(l)) {
        rest
    } else {
        body
    }
}

pub fn run(argv: &[String]) -> ExitCode {
    if matches!(argv.first().map(String::as_str), Some("-h" | "--help")) {
        print!("{HELP}");
        return ExitCode::SUCCESS;
    }
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    let (base, head) = (env("SHEEPDOG_BASE"), env("SHEEPDOG_HEAD"));
    if base.is_empty() {
        die("SHEEPDOG_BASE is not set (the base SHA)");
    }
    if head.is_empty() {
        die("SHEEPDOG_HEAD is not set (the head SHA)");
    }
    if !git::in_repo() {
        die("not a git repository");
    }
    let branch = git::git_out(".", &["branch", "--show-current"]).unwrap_or_default();

    let cfg = or_die(config::step("review", &branch, ""));
    let h = t(&cfg, "harness");
    let timeout = or_die(config::get_text("settings.stepTimeout", &branch, false));
    if !Regex::new("^[1-9][0-9]*$").expect("a fixed pattern compiles").is_match(&timeout) {
        die(format!("settings.stepTimeout must be whole seconds: {timeout}"));
    }

    let diff = git_or_die(&["diff", &base, &head], &format!("git diff {base} {head}"));
    let commits = commit_list(&base, &head);
    let short = |s: &str| s.chars().take(7).collect::<String>();
    let range = format!("{}..{}", short(&base), short(&head));
    let lib = guidance::lib_dir();
    let mut prompt = step_guidance("reviewPrompt", &branch, &base, &lib.join("prompts/review.md")).unwrap_or_else(|e| {
        eprintln!("{e}");
        die("can't read the review guidance")
    });
    let rules = guidance::read(&lib.join("review-rules.md")).unwrap_or_else(|e| die(e));
    let task = Some(env("SHEEPDOG_TASK")).filter(|s| !s.is_empty()).unwrap_or_else(|| "(none recorded)".into());
    let decided = crate::decisions::section(
        &env("SHEEPDOG_DECISIONS"),
        "Don't raise again a finding one of these settles, unless the change since that commit reopens it; then say which decision and what changed.",
    );
    prompt += &format!("\n\n{}\n\nTask: {task}{decided}\n\nCommits ({range}):\n{commits}\n\nDiff:\n{diff}", strip_nl(rules));

    let sid = harness::new_session();
    let what = if branch.is_empty() { range.clone() } else { format!("{branch} ({range})") };
    eprintln!("sd review-local: {h} (new session {sid}) is reviewing {what}…");
    let step = Step {
        harness: &h,
        session: &sid,
        mode: "new",
        parent: "",
        timeout: timeout.parse().unwrap_or(u64::MAX),
        prompt: &prompt,
        schema: SCHEMA,
        settings: &cfg,
        name: "review",
        branch: &branch,
        claude_allow: READONLY_ALLOW,
        pi_tools: READONLY_PI_TOOLS,
    };
    let Ok(Ran { results, .. }) = harness::run_step(&step) else {
        die(format!("the {h} review failed or returned no usable result"))
    };
    // One list, as the contract says: the last, if the harness gave several.
    let Some(findings) = results.iter().filter_map(|r| r.get("findings")).filter(|f| f.is_array()).next_back() else {
        die(format!("the {h} review returned no findings list"))
    };
    println!("{}", json::to_string(findings));
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::listed;

    #[test]
    fn a_subject_only_commit_is_one_line() {
        assert_eq!(listed("abc1234", "fix the thing\n"), "- abc1234 fix the thing");
    }

    #[test]
    fn a_commit_body_is_indented_under_its_subject() {
        let msg = "fix the thing\n\nWhy: it broke.\nSecond line.\n\nAnother paragraph.\n";
        assert_eq!(
            listed("abc1234", msg),
            "- abc1234 fix the thing\n    Why: it broke.\n    Second line.\n\n    Another paragraph."
        );
    }

    #[test]
    fn trailers_are_dropped() {
        let msg = "fix the thing\n\nBecause.\n\nCo-Authored-By: A <a@b.c>\nSigned-off-by: B <b@c.d>\n";
        assert_eq!(listed("abc1234", msg), "- abc1234 fix the thing\n    Because.");
        assert_eq!(listed("abc1234", "fix the thing\n\nCo-Authored-By: A <a@b.c>\n"), "- abc1234 fix the thing");
    }
}
