// sd review-local (src/review_local_help.txt, which holds the contract any
// script named by settings.localReview must keep).
//
// Harness, model and effort come from `sd config step review`. Every run is a
// new session (fresh context, never the authoring conversation) with read-only
// tools: no edits, no gh. The prompt is the guidance (settings.reviewPrompt: a
// path, relative ones read from SHEEPDOG_BASE so the branch can't rewrite its
// own review; empty means .sheepdog/review.md in the base commit, else
// lib/prompts/review.md), then the fixed reporting rules (lib/review-rules.md),
// the task, the commits and the diff. Why a local review before the bot: #79
// (Design record).

use crate::git::{self, git, strip_nl};
use crate::guidance::{self, step_guidance};
use crate::harness::{self, Step, READONLY_ALLOW, READONLY_PI_TOOLS};
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
    std::process::exit(1)
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
    let commits = git_or_die(&["log", "--format=- %s", &format!("{base}..{head}")], &format!("git log {base}..{head}"));
    let short = |s: &str| s.chars().take(7).collect::<String>();
    let range = format!("{}..{}", short(&base), short(&head));
    let lib = guidance::lib_dir();
    let mut prompt = step_guidance("reviewPrompt", &branch, &base, &lib.join("prompts/review.md")).unwrap_or_else(|e| {
        eprintln!("{e}");
        die("can't read the review guidance")
    });
    let rules = guidance::read(&lib.join("review-rules.md")).unwrap_or_else(|e| die(e));
    let task = Some(env("SHEEPDOG_TASK")).filter(|s| !s.is_empty()).unwrap_or_else(|| "(none recorded)".into());
    prompt += &format!("\n\n{}\n\nTask: {task}\n\nCommits ({range}):\n{commits}\n\nDiff:\n{diff}", strip_nl(rules));

    let sid = harness::new_session();
    let what = if branch.is_empty() { range.clone() } else { format!("{branch} ({range})") };
    eprintln!("sd review-local: {h} (new session {sid}) is reviewing {what}…");
    let step = Step {
        harness: &h,
        session: &sid,
        mode: "new",
        timeout: timeout.parse().unwrap_or(u64::MAX),
        prompt: &prompt,
        schema: SCHEMA,
        settings: &cfg,
        name: "review",
        branch: &branch,
        claude_allow: READONLY_ALLOW,
        pi_tools: READONLY_PI_TOOLS,
    };
    let Ok(results) = harness::run_step(&step) else {
        die(format!("the {h} review failed or returned no usable result"))
    };
    // One list, as the contract says: the last, if the harness gave several.
    let Some(findings) = results.iter().filter_map(|r| r.get("findings")).filter(|f| f.is_array()).next_back() else {
        die(format!("the {h} review returned no findings list"))
    };
    println!("{}", json::to_string(findings));
    ExitCode::SUCCESS
}
