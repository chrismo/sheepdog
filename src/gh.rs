// The gh runner (#336, arch review A5): every gh sheepdog runs goes through
// call, under a time limit, its failure put in a class; and every write
// sheepdog makes to GitHub is a named function here, so this file answers
// "what does sheepdog write to GitHub". gh_runs_only_from_gh_rs and
// github_writes_live_in_gh_rs hold both.

use crate::config::one_value;
use crate::git;
use crate::proc;
use crate::val::t;
use regex::Regex;
use std::process::Command;
use std::sync::OnceLock;

/// A read: GitHub answers in seconds. sd state makes a few, inside the
/// watcher's 120s for its whole run (SHEEPDOG_WATCH_TIMEOUT).
pub const READ: u64 = 30;
/// A write: longer than a read, as one given up on may land anyway, and the
/// next run's retry then posts it twice.
pub const WRITE: u64 = 60;
/// A failed CI job's log, downloaded whole.
pub const LOG: u64 = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Ok,
    /// GitHub has no such thing: HTTP 404 from REST, or a PR number GraphQL
    /// can't resolve.
    Gone,
    /// GitHub refused for the rate limit, primary or secondary.
    RateLimited,
    /// Anything else, a timeout too: stderr says why.
    Failed,
}

#[derive(Debug)]
pub struct Call {
    pub class: Class,
    pub stdout: String,
    pub stderr: String,
}

impl Call {
    pub fn ok(&self) -> bool {
        self.class == Class::Ok
    }
}

/// gh <args> in the current directory, at the repo GH_REPO names (point_at).
pub fn call(args: &[&str], secs: u64) -> Call {
    run("gh", None, None, args, secs)
}

/// gh <args> in <dir>; with <remote>, at the GitHub repo its URL names.
pub fn call_in(dir: &str, remote: Option<&str>, args: &[&str], secs: u64) -> Call {
    let repo = remote.and_then(|r| git::gh_repo(dir, r));
    run("gh", Some(dir), repo.as_deref(), args, secs)
}

/// Every gh this process runs asks <remote>'s GitHub repo. Unset, gh picks
/// among the remotes, and in a fork's clone that's upstream.
pub fn point_at(dir: &str, remote: &str) {
    if let Some(r) = git::gh_repo(dir, remote) {
        std::env::set_var("GH_REPO", r);
    }
}

pub fn graphql(fields: &[(&str, &str)], secs: u64) -> Call {
    let fields: Vec<String> = fields.iter().map(|(k, v)| format!("{k}={v}")).collect();
    let mut args = vec!["api", "graphql"];
    for f in &fields {
        args.extend(["-f", f.as_str()]);
    }
    call(&args, secs)
}

fn run(prog: &str, dir: Option<&str>, repo: Option<&str>, args: &[&str], secs: u64) -> Call {
    let mut c = Command::new(prog);
    c.args(args);
    if let Some(d) = dir {
        c.current_dir(d);
    }
    if let Some(r) = repo {
        c.env("GH_REPO", r);
    }
    let o = proc::timeout(secs, c, false);
    // GraphQL answers a missing PR in its body; its 404 is the endpoint.
    let rest = args.get(1) != Some(&"graphql");
    let class = match &o.stderr {
        _ if o.ok => Class::Ok,
        _ if o.timed_out => Class::Failed,
        e if rate_limited(e) => Class::RateLimited,
        e if (rest && e.contains("HTTP 404")) || e.contains("Could not resolve to a PullRequest") => Class::Gone,
        _ => Class::Failed,
    };
    Call { class, stdout: o.stdout, stderr: o.stderr }
}

fn rate_limited(err: &str) -> bool {
    static RL: OnceLock<Regex> = OnceLock::new();
    RL.get_or_init(|| Regex::new(r"(?i)rate.?limit").unwrap()).is_match(err)
}

// ── The writes ───────────────────────────────────────────────────────────────

pub fn reply_to_thread(thread: &str, body: &str) -> Call {
    let q = "mutation($thread: ID!, $body: String!) {
          addPullRequestReviewThreadReply(input: {pullRequestReviewThreadId: $thread, body: $body}) { comment { id } } }";
    graphql(&[("query", q), ("thread", thread), ("body", body)], WRITE)
}

pub fn resolve_thread(thread: &str) -> Call {
    let q = "mutation($thread: ID!) { resolveReviewThread(input: {threadId: $thread}) { thread { id } } }";
    graphql(&[("query", q), ("thread", thread)], WRITE)
}

/// By URL, not {owner}/{repo}: gh's default repo may be a fork.
pub fn comment_on_pr(pr_url: &str, body: &str) -> Call {
    let q = "query($url: URI!) { resource(url: $url) { ... on PullRequest { id } } }";
    let found = graphql(&[("url", pr_url), ("query", q)], READ);
    let id = one_value(&found.stdout).map(|v| t(&v, "data.resource.id")).unwrap_or_default();
    if !found.ok() || id.is_empty() {
        return Call { class: Class::Failed, ..found };
    }
    let q = "mutation($subject: ID!, $body: String!) { addComment(input: {subjectId: $subject, body: $body}) { clientMutationId } }";
    graphql(&[("query", q), ("subject", &id), ("body", body)], WRITE)
}

/// stdout is the comment's created_at, or empty.
pub fn post_issue_comment(host: &str, repo: &str, number: &str, body: &str) -> Call {
    let path = format!("repos/{repo}/issues/{number}/comments");
    let body = format!("body={body}");
    call(&["api", &path, "--hostname", host, "-f", &body, "--jq", r#".created_at // """#], WRITE)
}

pub fn retarget_pr(number: &str, base: &str) -> Call {
    call(&["pr", "edit", number, "--base", base], WRITE)
}

/// stdout is the new PR's URL.
pub fn create_draft_pr(base: &str, head: &str, title: &str, body: &str) -> Call {
    call(&["pr", "create", "--draft", "--base", base, "--head", head, "--title", title, "--body", body], WRITE)
}

pub fn edit_pr(number: &str, title: &str, body: &str, base: Option<&str>) -> Call {
    let mut args = vec!["pr", "edit", number, "--title", title, "--body", body];
    if let Some(b) = base {
        args.extend(["--base", b]);
    }
    call(&args, WRITE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::production_lines;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    fn fake(script: &str) -> (PathBuf, String) {
        use std::os::unix::fs::PermissionsExt;
        let dir = proc::private_dir("sd-gh-test").unwrap();
        let gh = dir.join("gh");
        std::fs::write(&gh, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        let prog = gh.to_string_lossy().into_owned();
        (dir, prog)
    }

    fn fake_call(script: &str, secs: u64) -> Call {
        let (dir, prog) = fake(script);
        let c = run(&prog, None, None, &["api", "x"], secs);
        let _ = std::fs::remove_dir_all(dir);
        c
    }

    #[test]
    fn a_gh_that_answers_is_ok_with_its_stdout_336() {
        let c = fake_call("echo \"$@\"; echo warn >&2", 5);
        assert_eq!(c.class, Class::Ok);
        assert_eq!((c.stdout.as_str(), c.stderr.as_str()), ("api x\n", "warn\n"));
    }

    #[test]
    fn a_hung_gh_is_killed_at_the_deadline_and_failed_336() {
        let start = Instant::now();
        let c = fake_call("echo partial; sleep 30", 1);
        assert!(start.elapsed() < Duration::from_secs(10), "waited {:?}", start.elapsed());
        assert_eq!(c.class, Class::Failed);
        assert_eq!(c.stdout, "partial\n");
        assert_eq!(c.stderr, "timed out after 1s\n");
    }

    #[test]
    fn a_rate_limit_refusal_is_rate_limited_336() {
        for msg in [
            "gh: API rate limit exceeded for user ID 1. (HTTP 403)",
            "GraphQL: API rate limit already exceeded for user ID 1.",
            "gh: You have exceeded a secondary rate limit. (HTTP 403)",
        ] {
            let c = fake_call(&format!("echo '{msg}' >&2; exit 1"), 5);
            assert_eq!(c.class, Class::RateLimited, "{msg}");
            assert_eq!(c.stderr, format!("{msg}\n"));
        }
    }

    #[test]
    fn a_404_or_an_unresolvable_pr_is_gone_336() {
        for msg in ["gh: Not Found (HTTP 404)", "GraphQL: Could not resolve to a PullRequest with the number of 9. (repository.pullRequest)"]
        {
            let c = fake_call(&format!("echo '{msg}' >&2; exit 1"), 5);
            assert_eq!(c.class, Class::Gone, "{msg}");
        }
    }

    #[test]
    fn a_graphql_404_is_the_endpoint_failed_not_gone_336() {
        let (dir, prog) = fake("echo 'gh: Not Found (HTTP 404)' >&2; exit 1");
        let c = run(&prog, None, None, &["api", "graphql", "-f", "query=q"], 5);
        let _ = std::fs::remove_dir_all(dir);
        assert_eq!(c.class, Class::Failed);
    }

    #[test]
    fn any_other_failure_is_failed_336() {
        for msg in ["gh: Forbidden (HTTP 403)", "error connecting to api.github.com", ""] {
            let c = fake_call(&format!("echo '{msg}' >&2; exit 1"), 5);
            assert_eq!(c.class, Class::Failed, "{msg}");
        }
        let c = run("no-such-gh-sd", None, None, &[], 5);
        assert_eq!(c.class, Class::Failed);
    }

    #[test]
    fn a_repo_is_gh_repo_and_a_dir_the_cwd_336() {
        let (dir, prog) = fake("echo \"${GH_REPO-unset} $(pwd -P)\"");
        let c = run(&prog, Some("/"), Some("github.com/me/r"), &[], 5);
        let _ = std::fs::remove_dir_all(dir);
        assert_eq!(c.stdout, "github.com/me/r /\n");
    }

    fn outside(file: &str, hit: impl Fn(&str) -> bool) -> Vec<String> {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut lines = Vec::new();
        production_lines(&src, &mut lines);
        assert!(lines.iter().any(|(f, _, _)| f.ends_with("src/act/ship.rs")), "read no src/act/ship.rs");
        lines
            .iter()
            .filter(|(f, _, l)| !f.ends_with(file) && hit(l))
            .map(|(f, n, l)| format!("{}:{n}: {}", f.strip_prefix(&src).unwrap().display(), l.trim()))
            .collect()
    }

    /// Test modules may run their own.
    #[test]
    fn gh_runs_only_from_gh_rs() {
        let runs = outside("src/gh.rs", |l| l.contains("\"gh\""));
        assert!(runs.is_empty(), "gh run outside gh.rs; run it through gh::call:\n{}", runs.join("\n"));
    }

    /// The write forms sheepdog uses: a GraphQL mutation, gh pr create or
    /// edit, an api call with a method, a REST api call with fields (gh POSTs
    /// them). By the line: a call split over lines can slip past.
    fn writes(l: &str) -> bool {
        let rest_fields = l.contains("\"api\"") && !l.contains("graphql") && (l.contains("\"-f\"") || l.contains("\"-F\""));
        rest_fields || l.contains("mutation") || ["\"create\"", "\"edit\"", "\"-X\"", "\"--method\""].iter().any(|w| l.contains(w))
    }

    #[test]
    fn github_writes_live_in_gh_rs() {
        assert!(writes(r#"call(&["api", &path, "-f", &body], WRITE)"#));
        assert!(!writes(r#"vec!["api", "graphql", "-F", "owner={owner}"]"#));
        let writes = outside("src/gh.rs", writes);
        assert!(writes.is_empty(), "a GitHub write outside gh.rs; make it a named function there:\n{}", writes.join("\n"));
    }
}
