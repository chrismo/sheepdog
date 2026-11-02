// ci-fix (J2): CI is red on HEAD. verify and autofix first, at zero tokens;
// else the agent gets the failing checks, their logs and the local verify
// output in the branch's session, and the pipeline commits its fix, verifies
// and pushes. "flaky" is recorded against the red SHA (branch.<b>.sd-ci-fix),
// so the same red isn't handed over again.

use super::commands::{status, tail, verify_autofix_to, Report};
use super::editing::{guidance, run_editing_step, Outcome, Spec};
use super::hold::{hold_gate, pr_gate};
use super::implement::blank;
use super::push::push_branch;
use super::step::{answer_cmd, answer_store, answer_waits, commit_edits, guidance_base, enter_worktree, must, question_ask, question_gate,
                  question_say, going_on, spend_worded, step_session, with_stored_answer};
use super::gate::{enforce, enforce_leaving, judge, reported, Gate, Go, Rule};
use super::{cfg, die, gates, lock_recheck, state_json, Args};
use crate::config::one_value;
use crate::git::{git, git_out};
use crate::next::Derived;
use crate::{gh, proc};
use crate::val::{at, sj, t, text};
use regex::Regex;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

// The ci-fix step's prompt: guidance (settings.ciFixPrompt, else
// .sheepdog/ci-fix.md in the base commit, else lib/prompts/ci-fix.md), then
// this contract, which the schema and the pipeline rely on (#344).
const CI_FIX_CONTRACT: &str = r#"Edit files, but do not commit, push, or run gh: the pipeline commits your edits
with your commit_message, runs the repo's checks, and pushes so CI runs again.

When the failure is in CI's environment rather than in the change (a timeout, a
flaky dependency, an outage, a runner that died), change nothing and return
outcome "flaky", saying why in summary; the pipeline reruns CI instead of
spending another round here.

Return outcome "fixed" with a one-line commit_message when you changed
something; "flaky" as above; "needs-input" when you need a decision from the
task's author (put the question in summary and leave your edits in place); or
"gave-up" when you can't make sense of the failure (say why in summary)."#;

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

/// <sha>, CI's red, when it is red at HEAD: not when forced past a CI wait,
/// where a record would overwrite the one for a real red at HEAD.
fn red_at_head<'a>(sha: &'a str, forced: &[String]) -> Option<&'a str> {
    forced.is_empty().then_some(sha).filter(|s| !s.is_empty())
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
    one_value(&gh::call(&["api", path], gh::READ).stdout).unwrap_or_default()
}

/// The failed tasks, (key, id), in `rwx results --json`'s answer: from its
/// first line that opens an object, rwx's chatter before it skipped.
fn rwx_failed(out: &str) -> Vec<(String, String)> {
    let start = out.split_inclusive('\n').take_while(|l| !l.starts_with('{')).map(str::len).sum::<usize>();
    let results = one_value(&out[start..]).unwrap_or_default();
    let task = |tk: &&Value| (t(tk, "Key"), t(tk, "ID"));
    failed(&results, "Tasks", "Status.Result", &["failed"]).iter().map(task).filter(|(_, id)| !id.is_empty()).collect()
}

/// The failed tasks' logs of RWX run <run>.
fn rwx_logs(run: &str) -> String {
    let mut logs = String::new();
    let results = proc::run("rwx", &["results", run, "--json"], None, None);
    for (i, (key, id)) in rwx_failed(&results.stdout).iter().enumerate() {
        let Some(dir) = proc::private_dir(&format!("sd-ci-fix-{i}")) else { continue };
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
            let log = gh::call(&["run", "view", run, "--job", job, "--log-failed"], gh::LOG);
            logs += &tail(&log.stdout, CI_LOG_TAIL);
        }
    }
    logs
}

/// Push the fix in <wt>, unless held for chrismo, <state>'s PR is no longer
/// open, <b> was attached during the step, or sd act push's gate refuses on
/// the state after the fix; <kept> says what is left unpushed when the push
/// doesn't happen.
fn publish(b: &str, wt: &str, remote: &str, state: &Value, force: bool, kept: &str) {
    hold_gate(b, &format!("pushing {b}"), "");
    let left = format!("the commit is kept in {wt}");
    pr_gate(&t(state, "pr.number"), &t(state, "pr.url"), "ci-fix", "pushing", &left);
    lock_recheck(b, "ci-fix", "pushing", &left);
    let now = state_json(wt, b).unwrap_or_else(|e| {
        eprint!("{e}");
        die(format!("sd state failed; {kept}"))
    });
    if enforce_leaving(gates::push(&Derived::new(&now)), force, &format!("pushing {b}"), kept).is_none() {
        return;
    }
    if let Err(e) = push_branch(b, remote) {
        eprintln!("sd act: {e}");
        die(kept);
    }
}

/// Fetch, read sd state, and ask gates::ci_fix (#348): Go, the state and
/// the CI waits --force ran past ("<name>: <clears>"; none when CI is red at
/// HEAD), or None once it's said why not. A wait stores <a>'s answer, to run
/// once it clears (#326); without one it refuses, and --force runs anyway,
/// with a warning.
fn gated(b: &str, wt: &str, remote: &str, a: &Args) -> Option<(Go, Value, Vec<String>)> {
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
    let rules = gates::ci_fix_rules(&Derived::new(&state));
    let mut forced = vec![];
    let gate = match spend_worded(judge(&rules), &state, "ci-fix", b) {
        Gate::Done(why) if answering => die(format!("{why}: the ci-fix agent's question is moot; sd act ack --question drops it")),
        Gate::Done(why) => Gate::Done(format!("{why}: nothing to fix on {b}")),
        Gate::Soft(why) if answering => {
            answer_store(b, "ci-fix", a);
            let yours: Vec<String> = reported(&rules).iter().filter(|r| r.yours).map(|r| format!("{} ({})", r.name, r.clears)).collect();
            println!("{}", answer_waits(b, "ci-fix", &why.join("; "), gates::ci_fix_once(&rules), &yours, a));
            return None;
        }
        Gate::Soft(_) => {
            let said = |rs: Vec<&Rule>| rs.iter().map(|r| format!("{}: {}", r.name, r.clears)).collect::<Vec<_>>();
            forced = said(gates::ci_fix_ci_waits(&rules));
            Gate::Soft(said(reported(&rules)))
        }
        g => g,
    };
    enforce(gate, a.force, &format!("fixing CI on {b} until {}", gates::ci_fix_once(&rules))).map(|go| (go, state, forced))
}

/// The agent calls CI flaky: recorded against <red>, the SHA of CI's red at
/// HEAD, or not at all with none (forced past a CI wait).
fn flaky(b: &str, wt: &str, red: Option<&str>, summary: &str, (before, agent_head): (&str, &str), autofixed: Option<String>) {
    if !status(wt).is_empty() {
        die(format!("the agent says flaky but edited files; they are left uncommitted in {wt}"));
    }
    if let Some(sha) = red {
        record(b, sha, "flaky");
        println!("{b}: the agent calls {}'s red flaky, not the change's: {summary}", short(sha));
    } else {
        println!("{b}: the agent calls CI flaky, not the change's: {summary}");
        println!("not recorded: there's no red at HEAD to record it against");
    }
    if agent_head != before {
        println!("the agent committed anyway; kept, unpushed: look (git log {before}..{b}), then sd act push");
    } else if let Some(c) = autofixed {
        println!("autofix's commit {c} is kept, unpushed: sd act push");
    } else {
        println!("nothing committed; rerun CI (gh run rerun, or rwx retry), or look: gh pr checks");
    }
}

/// The prompt's line on what CI reports failing on <sha>.
fn failing_line(sha: &str, failing: &str) -> String {
    let on = if sha.is_empty() { String::new() } else { format!(" on {}", short(sha)) };
    if failing.is_empty() {
        format!("No failing checks are reported{on} yet.")
    } else {
        format!("Failing checks{on}: {failing}")
    }
}

/// Zero tokens first: verify, autofix, verify. A fix there is the whole step,
/// and None. Else the agent's prompt, from guidance read at <base>, and
/// autofix's commit if it made one.
fn first_pass(b: &str, wt: &str, remote: &str, a: &Args, state: &Value, forced: &[String], base: &str) -> Option<(String, Option<String>)> {
    let sha = t(state, "ci.sha");
    let verify = cfg("settings.verify", b);
    let mut vout = String::new();
    let (verified, commit) = verify_autofix_to(wt, b, "ci-fix", &mut Report::Keep(&mut vout));
    if commit.is_some() {
        print!("{vout}");
        if verified.is_ok() {
            if let Some(red) = red_at_head(&sha, forced) {
                record(b, red, "autofix");
            }
            if a.no_publish {
                println!("autofix fixed what verify saw; not pushed (--no-publish): sd act push");
                return None;
            }
            publish(b, wt, remote, state, a.force, &format!("autofix's commit is kept in {wt}, not pushed"));
            println!("autofix fixed what verify saw; CI reruns on the push");
            return None;
        }
    }
    if !status(wt).is_empty() {
        die(format!("verify ({verify}) changed files in {wt} (a formatter?); commit them, then sd act ci-fix"));
    }
    let task = git_out(".", &["config", "--get", &format!("branch.{b}.sd-task")]).unwrap_or_default();
    let logs = if sha.is_empty() { String::new() } else { ci_logs(&sha) };
    let mut p = guidance(b, base, "ciFixPrompt", "ci-fix.md", "ci-fix");
    p += &format!("\n\n{CI_FIX_CONTRACT}\n\nTask: {}", if task.is_empty() { "(none recorded)" } else { &task });
    p += &format!("\n\n{}", failing_line(&sha, &sj(at(state, "ci.failing"), ", ")));
    if !forced.is_empty() {
        p += "\n\nsd act ci-fix was forced past its gate, so CI may not be red at HEAD:";
        p += &forced.iter().map(|w| format!("\n- {w}")).collect::<String>();
    }
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
    Some((p, commit))
}

fn ci_fix_run(_: Go, b: &str, wt: &str, remote: &str, a: &Args, state: &Value, forced: &[String]) {
    let answering = !a.answer.is_empty();
    let sha = t(state, "ci.sha");
    let last = git_out(".", &["config", "--get", &format!("branch.{b}.sd-ci-fix")]).unwrap_or_default();
    if judged_flaky(&last, &sha) {
        if !a.force {
            die(format!(
                "ci-fix already judged {}'s red flaky and changed nothing; rerun CI (gh run rerun, or rwx retry), or look: gh pr checks. --force hands it over again",
                short(&sha)
            ));
        }
        eprintln!("sd act: warning: ci-fix already judged {}'s red flaky; running anyway", short(&sha));
    }
    question_gate(b, "ci-fix", a.force, answering);
    going_on("ci-fix", a);
    let sess = step_session(b, "ci-fix", &a.harness, answering);
    // With no base from sd state, the trusted one guidance_base finds (maybe none).
    let mut base = t(state, "sync.old_base");
    if base.is_empty() {
        base = guidance_base(b);
    }
    let verify = cfg("settings.verify", b);
    // An answer goes to the session that has the logs, with the agent's edits
    // in the worktree, so neither verify nor autofix runs first.
    let (prompt, autofixed) = match answering {
        true => (String::new(), None),
        false => match first_pass(b, wt, remote, a, state, forced, &base) {
            Some(p) => p,
            None => return,
        },
    };
    let r = run_editing_step(Spec {
        b,
        wt,
        step: "ci-fix",
        asks_as: "ci-fix",
        session: &sess,
        base: &base,
        prompt,
        schema: CI_FIX_SCHEMA,
        done: "fixed",
        doing: format!("fixing CI on {b} ({})", sj(at(state, "ci.failing"), ", ")),
        verify: &verify,
        answer: &a.answer,
        items: "",
    });
    match &r.outcome {
        Outcome::Done => {}
        Outcome::Other(w) if w == "flaky" => {
            return flaky(b, wt, red_at_head(&sha, forced), &r.summary, (&r.before, &r.agent_head), autofixed);
        }
        Outcome::NeedsInput(q) => {
            question_ask(b, wt, "ci-fix", q);
            die(format!("the agent needs input on {b}: {q}\n{}: answer with {}, which goes on from its edits (if any) in {wt}; or in its session (sd act attach {b}), then commit or discard the edits, or with none, sd act ack --question; then sd act ci-fix", question_say(b), answer_cmd(b)));
        }
        Outcome::GaveUp(why) => r.give_up(why),
        Outcome::Other(w) => r.unknown(w),
    }
    if !r.dirty() {
        r.changed_nothing("fixed");
    }
    let msg = match at(&r.result, "commit_message").as_str() {
        Some(m) if !blank(m) => m.to_string(),
        _ => match at(state, "ci.failing").get(0).map(text) {
            Some(check) if !check.is_empty() => format!("Fix CI: {check}"),
            _ => "Fix CI".into(),
        },
    };
    commit_edits(b, "ci-fix", wt, &msg);
    println!("{b}: {}", r.summary);
    r.print_log();
    if let Err(cmd) = r.verify() {
        die(format!("verify failed ({cmd}); the commit is kept in {wt}, nothing pushed"));
    }
    if let Some(red) = red_at_head(&sha, forced) {
        record(b, red, "fixed");
    }
    if a.no_publish {
        println!("not pushed (--no-publish): sd act push when ready");
        return;
    }
    publish(b, wt, remote, state, a.force, &format!("the fix is committed in {wt}, not pushed"));
    println!("CI reruns on the push");
}

pub fn ci_fix(b: &str, remote: &str, a: &Args) {
    let stored = with_stored_answer(b, "ci-fix", a);
    let a = stored.as_ref().unwrap_or(a);
    let wt = enter_worktree(b);
    if let Some((go, state, forced)) = gated(b, &wt, remote, a) {
        ci_fix_run(go, b, &wt, remote, a, &state, &forced);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
