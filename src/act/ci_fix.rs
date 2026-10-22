// ci-fix (J2): CI is red on HEAD. verify and autofix first, at zero tokens;
// else the agent gets the failing checks, their logs and the local verify
// output in the branch's session, and the pipeline commits its fix, verifies
// and pushes. "flaky" is recorded against the red SHA (branch.<b>.sd-ci-fix),
// so the same red isn't handed over again.

use super::commands::{status, tail, verify_autofix, verify_autofix_to, Report};
use super::hold::{hold_gate, hold_since};
use super::implement::blank;
use super::push::push_branch;
use super::step::{agent_rules, answer_cmd, answer_prompt, commit_edits, guidance_base, in_worktree, must, question_answered,
                  question_ask, question_gate, question_say, step_session, step_store};
use super::{cfg, die, state_json, Args};
use crate::config::one_value;
use crate::git::{git, git_out, rev};
use crate::guidance::{lib_dir, step_guidance};
use crate::harness::{self, Step, PI_TOOLS};
use crate::proc;
use crate::val::{at, n, on, sj, t, text};
use regex::Regex;
use serde_json::Value;
use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;

const CI_FIX_SCHEMA: &str = r#"{"type": "object", "required": ["outcome", "commit_message", "summary"],
 "properties": {"outcome": {"enum": ["fixed", "flaky", "needs-input", "gave-up"]},
                "commit_message": {"type": ["string", "null"]},
                "summary": {"type": "string"}}}"#;

const CI_LOG_TAIL: usize = 150;

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(7)]
}

/// branch.<b>.sd-ci-fix, "<sha> fixed|flaky|autofix": the last red ci-fix
/// handled, and how.
fn record(b: &str, sha: &str, how: &str) {
    must(&["config", &format!("branch.{b}.sd-ci-fix"), &format!("{sha} {how}")]);
}

/// The record says the agent already called <sha>'s red flaky.
fn judged_flaky(last: &str, sha: &str) -> bool {
    last.split_once(' ') == Some((sha, "flaky"))
}

/// The entries of <v>'s <list> whose <field> is one of <failed>.
fn failed<'a>(v: &'a Value, list: &str, field: &str, failed: &[&str]) -> Vec<&'a Value> {
    let all = at(v, list).as_array().map_or(&[][..], Vec::as_slice);
    all.iter().filter(|e| at(e, field).as_str().is_some_and(|s| failed.contains(&s))).collect()
}

fn gh_api(path: &str) -> Value {
    one_value(&proc::run("gh", &["api", path], None, None).stdout).unwrap_or_default()
}

/// The failed tasks, (key, id), in `rwx results --json`'s answer: from its
/// first line that opens an object, rwx's chatter before it skipped.
fn rwx_failed(out: &str) -> Vec<(String, String)> {
    let start = out.split_inclusive('\n').take_while(|l| !l.starts_with('{')).map(str::len).sum::<usize>();
    let results = one_value(&out[start..]).unwrap_or_default();
    let task = |tk: &&Value| (t(tk, "Key"), t(tk, "ID"));
    failed(&results, "Tasks", "Status.Result", &["failed"]).iter().map(task).filter(|(_, id)| !id.is_empty()).collect()
}

/// A fresh directory for task <i>'s logs: 0700 whatever the umask, as the
/// bash's mktemp -d made it. A name already taken is someone else's, so it's
/// skipped, never reused or removed.
fn log_dir(i: usize) -> Option<PathBuf> {
    for n in 0..100 {
        let dir = std::env::temp_dir().join(format!("sd-ci-fix-{}-{i}-{n}", std::process::id()));
        match fs::DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => return Some(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return None,
        }
    }
    None
}

/// The failed tasks' logs of RWX run <run>.
fn rwx_logs(run: &str) -> String {
    let mut logs = String::new();
    let results = proc::run("rwx", &["results", run, "--json"], None, None);
    for (i, (key, id)) in rwx_failed(&results.stdout).iter().enumerate() {
        let Some(dir) = log_dir(i) else { continue };
        if proc::run("rwx", &["logs", id, "--output", &dir.to_string_lossy()], None, None).ok {
            let mut files: Vec<PathBuf> = fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()).collect();
            files.retain(|f| f.is_file() && f.extension().is_some_and(|x| x == "log"));
            files.sort();
            for f in files {
                logs += &format!("### task {key} (rwx logs {id})\n");
                logs += &tail(&String::from_utf8_lossy(&fs::read(&f).unwrap_or_default()), CI_LOG_TAIL);
            }
        }
        let _ = fs::remove_dir_all(&dir);
    }
    logs
}

/// The failing checks on <sha> and what can be fetched of their logs, for the
/// prompt. All REST: the commit's statuses (RWX and other external CI; an
/// RWX run's logs through rwx results and rwx logs) and its check runs
/// (GitHub Actions; gh run view --log-failed). Each log is cut to its last
/// CI_LOG_TAIL lines; a source that can't be read is left out, never fatal.
fn ci_logs(sha: &str) -> String {
    let rwx_run = Regex::new(r"cloud\.rwx\.com/[^/]+/runs/([0-9a-z]+)").expect("a fixed pattern compiles");
    let gha_job = Regex::new(r"/actions/runs/([0-9]+)/job/([0-9]+)").expect("a fixed pattern compiles");
    let mut logs = String::new();
    let statuses = gh_api(&format!("repos/{{owner}}/{{repo}}/commits/{sha}/status"));
    for s in failed(&statuses, "statuses", "state", &["failure", "error"]) {
        let (ctx, url) = (t(s, "context"), t(s, "target_url"));
        if ctx.is_empty() {
            continue;
        }
        logs += &format!("## {ctx}: {}\n{url}\n", t(s, "description"));
        if let Some(run) = rwx_run.captures(&url) {
            logs += &rwx_logs(&run[1]);
        }
    }
    let checks = gh_api(&format!("repos/{{owner}}/{{repo}}/commits/{sha}/check-runs"));
    for c in failed(&checks, "check_runs", "conclusion", &["failure", "timed_out"]) {
        let (name, url) = (t(c, "name"), t(c, "details_url"));
        if name.is_empty() {
            continue;
        }
        logs += &format!("## {name}: {}\n{}\n{url}\n", t(c, "output.title"), t(c, "output.summary"));
        if let Some(job) = gha_job.captures(&url) {
            let (run, job) = (&job[1], &job[2]);
            logs += &format!("### failed steps (gh run view {run} --job {job} --log-failed)\n");
            let log = proc::run("gh", &["run", "view", run, "--job", job, "--log-failed"], None, None);
            logs += &tail(&log.stdout, CI_LOG_TAIL);
        }
    }
    logs
}

/// Push the fix, unless held for chrismo; <kept> says what is left unpushed
/// when the push fails.
fn publish(b: &str, remote: &str, kept: &str) {
    hold_gate(b, &format!("pushing {b}"), "");
    if let Err(e) = push_branch(b, remote) {
        eprintln!("sd act: {e}");
        die(kept);
    }
}

fn ci_fix_run(b: &str, wt: &str, remote: &str, a: &Args) {
    let answering = !a.answer.is_empty();
    if !answering && !status(wt).is_empty() {
        die(format!("{wt} has uncommitted changes"));
    }
    let f = git(wt, &["fetch", "--quiet", remote]);
    eprint!("{}", f.stderr);
    if !f.ok {
        die(format!("fetch from {remote} failed"));
    }
    let state = state_json(wt, b).unwrap_or_else(|e| {
        eprint!("{e}");
        die("sd state failed")
    });
    if at(&state, "pr").is_null() {
        die(format!("no PR for {b}"));
    }
    let pr = t(&state, "pr.state");
    if pr != "OPEN" {
        die(format!("PR #{} is {}; nothing to fix", t(&state, "pr.number"), pr.to_lowercase()));
    }
    let sha = t(&state, "ci.sha");
    match at(&state, "ci.verdict").as_str().unwrap_or("none") {
        "red" => {}
        "inconclusive" => die("CI was cancelled, not red: rerun it (gh run rerun, or rwx retry), then wait"),
        v => die(format!("CI is {v} on {b}; nothing to fix")),
    }
    if !on(&state, "ci.for_head") {
        die(format!(
            "CI's red is for {}, not HEAD: push if there is something to push, then wait for CI on HEAD",
            short(&sha)
        ));
    }
    if n(&state, "push.ahead") != 0.0 {
        die(format!("{b} has unpushed commits, which may be the fix already: sd act push, then wait for CI"));
    }
    let failing = sj(at(&state, "ci.failing"), ", ");
    let last = git_out(".", &["config", "--get", &format!("branch.{b}.sd-ci-fix")]).unwrap_or_default();
    if !a.force && judged_flaky(&last, &sha) {
        die(format!(
            "ci-fix already judged {}'s red flaky and changed nothing; rerun CI (gh run rerun, or rwx retry), or look: gh pr checks. --force hands it over again",
            short(&sha)
        ));
    }
    question_gate(b, "ci-fix", a.force, answering);
    let s = step_session(b, "ci-fix", &a.harness, a.force, answering);
    // With no base from sd state, the trusted one guidance_base finds (maybe none).
    let mut base = t(&state, "sync.old_base");
    if base.is_empty() {
        base = guidance_base(b);
    }
    let task = git_out(".", &["config", "--get", &format!("branch.{b}.sd-task")]).unwrap_or_default();
    let rules = agent_rules(b);
    let verify = cfg("settings.verify", b);

    let mut autofixed = None;
    let mut prompt = if answering {
        // The session has the logs and the agent's edits are in the worktree,
        // so neither verify nor autofix runs first.
        answer_prompt(&a.answer, "")
    } else {
        // Zero tokens first: verify, autofix, verify. A fix there is the whole step.
        let mut vout = String::new();
        let (verified, commit) = verify_autofix_to(wt, b, &mut Report::Keep(&mut vout));
        if commit.is_some() {
            print!("{vout}");
            if verified.is_ok() {
                record(b, &sha, "autofix");
                if a.no_publish {
                    println!("autofix fixed what verify saw; not pushed (--no-publish): sd act push");
                    return;
                }
                publish(b, remote, &format!("autofix's commit is kept in {wt}, not pushed"));
                println!("autofix fixed what verify saw; CI reruns on the push");
                return;
            }
        }
        autofixed = commit;
        if !status(wt).is_empty() {
            die(format!("verify ({verify}) changed files in {wt} (a formatter?); commit them, then sd act ci-fix"));
        }
        let logs = ci_logs(&sha);
        let mut p = step_guidance("ciFixPrompt", b, &base, &lib_dir().join("prompts/ci-fix.md")).unwrap_or_else(|e| {
            eprintln!("{e}");
            die("can't read the ci-fix guidance")
        });
        p += &format!("\n\nTask: {}", if task.is_empty() { "(none recorded)" } else { &task });
        p += &format!("\n\nFailing checks on {}: {failing}", short(&sha));
        if !logs.is_empty() {
            p += &format!("\n\n{}", logs.trim_end_matches('\n'));
        }
        p += &if verify.is_empty() {
            "\n\nNo local checks are configured (settings.verify), so only CI's logs say what failed.".to_string()
        } else if verified.is_ok() {
            format!("\n\nThe repo's checks ({verify}) pass locally, so the failure may be in CI's environment, or in what CI runs that the local checks don't.")
        } else {
            format!("\n\nThe repo's checks ({verify}) fail locally too:\n{}", vout.trim_end_matches('\n'))
        };
        p
    };
    if !verify.is_empty() {
        prompt += &format!("\n\nThe repo's checks, which the pipeline runs after you: {verify}");
    }
    let (allow, said) = harness::tool_rules(&s.harness, &rules);
    prompt += &format!("\n\n{said}");

    let (h, sid) = (s.harness.as_str(), s.id.as_str());
    eprintln!("sd act: {h} ({} session {sid}) is fixing CI on {b} ({failing})…", s.mode);
    let head = format!("refs/heads/{b}");
    let before = rev(".", &head);
    let step = Step {
        harness: h,
        session: sid,
        mode: s.mode,
        timeout: s.timeout,
        prompt: &prompt,
        schema: CI_FIX_SCHEMA,
        settings: &s.cfg,
        name: "ci-fix",
        branch: b,
        claude_allow: &allow,
        pi_tools: PI_TOOLS,
    };
    let Some(result) = harness::run_step(&step).ok().and_then(|rs| rs.into_iter().last()) else {
        hold_since(b, &base, &before);
        die(format!("the {h} ci-fix step failed or returned no usable result (session {sid})"))
    };
    step_store(b, &s);
    question_answered(b, answering);
    hold_since(b, &base, &before);
    let agent_head = rev(".", &head);

    let outcome = t(&result, "outcome");
    let summary = t(&result, "summary");
    match outcome.as_str() {
        "fixed" => {}
        "flaky" => {
            if !status(wt).is_empty() {
                die(format!("the agent says flaky but edited files; they are left uncommitted in {wt}"));
            }
            record(b, &sha, "flaky");
            println!("{b}: the agent calls {}'s red flaky, not the change's: {summary}", short(&sha));
            if agent_head != before {
                println!("the agent committed anyway; kept, unpushed: look (git log {before}..{b}), then sd act push");
            } else if let Some(c) = autofixed {
                println!("autofix's commit {c} is kept, unpushed: sd act push");
            } else {
                println!("nothing committed; rerun CI (gh run rerun, or rwx retry), or look: gh pr checks");
            }
            return;
        }
        "needs-input" => {
            question_ask(b, wt, "ci-fix", &summary);
            die(format!("the agent needs input on {b}: {summary}\n{}: answer with {}, which goes on from its edits (if any) in {wt}; or in its session (sd act attach {b}), then commit or discard the edits, or with none, sd act ack --question; then sd act ci-fix", question_say(b), answer_cmd(b)));
        }
        "gave-up" => die(format!("the agent gave up on {b}: {summary}. Edits (if any) are left uncommitted in {wt}")),
        _ => die(format!("the agent returned outcome \"{outcome}\"; edits (if any) are left uncommitted in {wt}")),
    }
    if status(wt).is_empty() {
        die(format!("the agent says fixed but changed nothing (session {sid})"));
    }
    let msg = match at(&result, "commit_message").as_str() {
        Some(m) if !blank(m) => m.to_string(),
        _ => format!("Fix CI: {}", at(&state, "ci.failing").get(0).map(text).unwrap_or_default()),
    };
    commit_edits(wt, &msg);
    println!("{b}: {summary}");
    print!("{}", git(wt, &["log", "--format=  %h %s", &format!("{before}..{head}")]).stdout);
    hold_since(b, &base, &agent_head);
    if let Err(cmd) = verify_autofix(wt, b) {
        die(format!("verify failed ({cmd}); the commit is kept in {wt}, nothing pushed"));
    }
    record(b, &sha, "fixed");
    if a.no_publish {
        println!("not pushed (--no-publish): sd act push when ready");
        return;
    }
    publish(b, remote, &format!("the fix is committed in {wt}, not pushed"));
    println!("CI reruns on the push");
}

pub fn ci_fix(b: &str, remote: &str, a: &Args) {
    in_worktree(b, "ci-fix", |wt| ci_fix_run(b, wt, remote, a));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn log_dir_is_private_whatever_the_umask() {
        use std::os::unix::fs::PermissionsExt;
        let dir = log_dir(999).unwrap();
        let mode = fs::metadata(&dir).unwrap().permissions().mode();
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn log_dir_never_reuses_or_removes_a_path_someone_else_made() {
        // Someone else's directory where log_dir's first choice would be.
        let theirs = log_dir(998).unwrap();
        fs::write(theirs.join("keep"), "x").unwrap();
        let dir = log_dir(998).unwrap();
        let kept = theirs.join("keep").exists();
        fs::remove_dir_all(&dir).unwrap();
        fs::remove_dir_all(&theirs).unwrap();
        assert_ne!(dir, theirs);
        assert!(kept);
    }

    #[test]
    fn judged_flaky_only_for_a_flaky_record_on_the_same_sha() {
        assert!(judged_flaky("abc123 flaky", "abc123"));
        assert!(!judged_flaky("abc123 flaky", "def456"));
        assert!(!judged_flaky("abc123 fixed", "abc123"));
        assert!(!judged_flaky("abc123 autofix", "abc123"));
        assert!(!judged_flaky("", "abc123"));
        assert!(!judged_flaky("abc123", "abc123"));
    }

    #[test]
    fn short_is_the_first_seven_or_what_there_is() {
        assert_eq!(short("0123456789abcdef"), "0123456");
        assert_eq!(short("abc"), "abc");
        assert_eq!(short(""), "");
    }

    #[test]
    fn failed_picks_the_entries_in_a_failed_state() {
        let v = json!({"statuses": [
            {"context": "a", "state": "failure"}, {"context": "b", "state": "success"},
            {"context": "c", "state": "error"}, {"context": "d"}
        ]});
        let got: Vec<String> = failed(&v, "statuses", "state", &["failure", "error"]).iter().map(|s| t(s, "context")).collect();
        assert_eq!(got, ["a", "c"]);
        assert!(failed(&json!({}), "statuses", "state", &["failure"]).is_empty());
        assert!(failed(&Value::Null, "check_runs", "conclusion", &["failure"]).is_empty());
    }

    #[test]
    fn rwx_failed_reads_the_failed_tasks_after_rwxs_chatter() {
        let out = "Fetching results…\n{\"Tasks\": [{\"Key\": \"lint\", \"ID\": \"t1\", \"Status\": {\"Result\": \"succeeded\"}},\n\
                   {\"Key\": \"test\", \"ID\": \"t2\", \"Status\": {\"Result\": \"failed\"}},\n\
                   {\"Key\": \"gone\", \"Status\": {\"Result\": \"failed\"}}]}\n";
        assert_eq!(rwx_failed(out), [("test".to_string(), "t2".to_string())]);
        assert!(rwx_failed("").is_empty());
        assert!(rwx_failed("error: not found\n").is_empty());
        assert!(rwx_failed("{\"Tasks\": null}").is_empty());
    }
}
