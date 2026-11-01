// sd state: where a branch's PR stands (src/state_help.txt). sd act and sd
// watch read its --json as JSON, not as bytes (src/json.rs, #379); NEXT is
// src/next.rs.
//
// Its one git config write is a cache (#339): lookup_pr's branch.<b>.sd-pr,
// an authored branch's discovered PR number. A followed branch's is its
// record, only read.

use crate::git::{
    commits_not_ours, count, git, git_ok, git_out, is_ancestor, made_here, main_worktree, patch_id, rev, show_ref, stacked_on,
    stacked_unclear, strip_nl, worktree_for, worktree_mark,
};
use crate::time::{epoch_ns, now_utc};
use crate::usage;
use crate::val::{at, n, on, sj, t, text, NULL};
use crate::{config, git, json, next, proc, running};
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::process::ExitCode;
use std::sync::OnceLock;

const HELP: &str = include_str!("state_help.txt");

// By number: GitHub multiplies each connection's cost by its parents' page
// sizes, so under pullRequests(first: 10) this cost 21 points, not 2 (#122).
const QUERY: &str = r#"query($owner: String!, $repo: String!, $number: Int!, $commits: Boolean!) {
  viewer { login }
  rateLimit { cost remaining resetAt }
  repository(owner: $owner, name: $repo) {
    defaultBranchRef { name }
    pullRequest(number: $number) {
      number url state isDraft author { login } isCrossRepository headRepositoryOwner { login } headRefName baseRefName headRefOid mergeable mergedAt
      mergeStateStatus reviewDecision
      reviewRequests(first: 20) { nodes { requestedReviewer {
        ... on User { login } ... on Bot { login } ... on Mannequin { login } ... on Team { slug } } } }
      # GitHub takes up to 250 on a PR's commits, not the usual 100.
      allCommits: commits(last: 250) @include(if: $commits) { nodes { commit { oid } } }
      commits(last: 1) { nodes { commit { oid statusCheckRollup { state contexts(first: 100) { nodes {
        __typename
        ... on CheckRun { databaseId name status conclusion
          checkSuite { app { slug } workflowRun { event workflow { name } } } }
        ... on StatusContext { context state }
      } } } } } }
      reviewThreads(first: 100) { nodes {
        id isResolved isOutdated path line
        first: comments(first: 1) { nodes { author { __typename login } url body } }
        last: comments(last: 1) { nodes { id author { __typename login } } }
      } }
      reviews(last: 50) { nodes { id url state body createdAt submittedAt commit { oid } author { __typename login } } }
      comments(last: 50) { nodes { id url body createdAt author { __typename login } } }
    }
  }
}"#;

// A branch's PRs, with only what pick_pr needs: ten, since a common branch
// name also matches strangers' fork PRs.
const DISCOVER: &str = r#"query($owner: String!, $repo: String!, $branch: String!) {
  viewer { login }
  rateLimit { cost remaining resetAt }
  repository(owner: $owner, name: $repo) {
    defaultBranchRef { name }
    pullRequests(headRefName: $branch, first: 10, orderBy: {field: CREATED_AT, direction: DESC}) {
      nodes { number state isCrossRepository headRepositoryOwner { login } }
    }
  }
}"#;

fn str_or<'a>(v: &'a Value, d: &'a str) -> &'a str {
    v.as_str().unwrap_or(d)
}

fn nn(s: &str) -> Value {
    if s.is_empty() {
        Value::Null
    } else {
        json!(s)
    }
}

fn arr(v: &Value) -> &[Value] {
    v.as_array().map_or(&[], Vec::as_slice)
}

fn re(p: &str) -> Regex {
    Regex::new(p).expect("a fixed pattern compiles")
}

fn die(msg: impl std::fmt::Display) -> ! {
    exit_with(&fail(msg))
}

fn exit_with(stderr: &str) -> ! {
    eprint!("{stderr}");
    std::process::exit(1)
}

fn fail(msg: impl std::fmt::Display) -> String {
    format!("sd state: {msg}\n")
}

// ── usage.sup: what the branch has spent (#121) ──────────────────────────────

/// This repo's <branch> spend, usd, unknown and stray (usage::Spend),
/// warning of lines that aren't records; None when usage.sup can't be read.
fn branch_spend(dir: &str, b: &str) -> Option<(Value, Value, Value)> {
    let s = usage::spend(&main_worktree(dir), b)?;
    if s.skipped > 0 {
        eprintln!("sd state: warning: {} line(s) of {} aren't usage records; their cost isn't counted", s.skipped, usage::file());
    }
    Some((json!(s.usd), Value::Array(s.unknown), Value::Array(s.stray)))
}

// ── records sd act keeps under refs/sheepdog/<b>/ ────────────────────────────

fn blob_record(dir: &str, r: &str) -> Option<Result<Value, ()>> {
    let o = git(dir, &["cat-file", "blob", r]);
    if !o.ok {
        return None;
    }
    let mut it = serde_json::Deserializer::from_str(&o.stdout).into_iter::<Value>();
    Some(match (it.next(), it.next()) {
        (Some(Ok(v)), None) if v.is_object() => Ok(v),
        _ => Err(()),
    })
}

/// An agent step's question for chrismo (#167): {step, kind, sha, text,
/// at_head, answer}, kind "ask", or "gave-up" for a give-up (#254);
/// None with no record, Err for one that isn't one. answer: chrismo's
/// answer stored for this question while its step couldn't run (#326),
/// else null.
pub(crate) fn question_state(dir: &str, b: &str, head: &str) -> Result<Option<Value>, ()> {
    let qref = format!("refs/sheepdog/{b}/question");
    let Some(rec) = blob_record(dir, &qref) else { return Ok(None) };
    let rec = rec?;
    let s = |k: &str| rec.get(k).and_then(Value::as_str);
    let (Some(h), Some(step), Some(m), Some(text)) = (s("head"), s("step"), s("mark"), s("text")) else { return Err(()) };
    let at_head = h == head
        && {
            let wt = worktree_for(dir, b);
            !wt.is_empty() && worktree_mark(&wt).as_deref() == Some(m)
        }
        && match (s("onto_ref"), s("onto")) {
            (Some(r), Some(sha)) => rev(dir, r) == sha,
            _ => true,
        };
    let kind = s("kind").unwrap_or("ask");
    let answer = stored_answer(dir, b, &git::oid(dir, &qref));
    Ok(Some(json!({"step": step, "kind": kind, "sha": h, "text": text, "at_head": at_head, "answer": answer})))
}

/// refs/sheepdog/<b>/answer's text when it answers the question whose record
/// is the blob <question> (#326); one for another question, or not a record,
/// is none.
fn stored_answer(dir: &str, b: &str, question: &str) -> Value {
    let Some(Ok(rec)) = blob_record(dir, &format!("refs/sheepdog/{b}/answer")) else { return Value::Null };
    match (rec.get("question").and_then(Value::as_str), rec.get("text").and_then(Value::as_str)) {
        (Some(q), Some(text)) if !question.is_empty() && q == question => json!(text),
        _ => Value::Null,
    }
}

/// The agent steps that failed in a row (#242, src/failed.rs): {step,
/// harness, count, since, at, error}; None with no record, Err for one that
/// isn't one.
fn failed_state(dir: &str, b: &str) -> Result<Option<Value>, ()> {
    let Some(rec) = blob_record(dir, &format!("refs/sheepdog/{b}/failed")) else { return Ok(None) };
    let rec = rec?;
    let s = |k: &str| rec.get(k).and_then(Value::as_str);
    let (Some(step), Some(h), Some(since), Some(at), Some(error)) = (s("step"), s("harness"), s("since"), s("at"), s("error")) else {
        return Err(());
    };
    let Some(count) = rec.get("count").and_then(Value::as_i64) else { return Err(()) };
    Ok(Some(json!({"step": step, "harness": h, "count": count, "since": since, "at": at, "error": error})))
}

/// A failed verify (#325, src/verify_red.rs): {sha, step, cmd, at,
/// at_head}, at_head while HEAD is the commit it failed on; None with no
/// record, Err for one that isn't one.
fn verify_state(dir: &str, b: &str, head: &str) -> Result<Option<Value>, ()> {
    let Some(rec) = blob_record(dir, &crate::verify_red::name(b)) else { return Ok(None) };
    let rec = rec?;
    let s = |k: &str| rec.get(k).and_then(Value::as_str);
    let (Some(h), Some(step), Some(cmd), Some(at)) = (s("head"), s("step"), s("cmd"), s("at")) else { return Err(()) };
    Ok(Some(json!({"sha": h, "step": step, "cmd": cmd, "at": at, "at_head": h == head})))
}

// ── GitHub ───────────────────────────────────────────────────────────────────

struct Gh<'a> {
    dir: &'a str,
    resp: Value,
    errors: Vec<String>,
    rl_cost: Option<i64>,
    rl_remaining: Value,
    rl_reset: String,
    rl_limited: bool,
}

enum Call {
    Ok,
    Failed,
    Gone,
}

fn rate_limited(err: &str) -> bool {
    static RL: OnceLock<Regex> = OnceLock::new();
    RL.get_or_init(|| re(r"(?i)rate.?limit")).is_match(err)
}

impl Gh<'_> {
    /// One GraphQL call into resp, its rateLimit added to rl_*. A failure is
    /// recorded in errors, unless it's a number GitHub can't resolve (Gone:
    /// the caller re-discovers), and a rate-limit refusal sets rl_limited.
    fn call(&mut self, label: &str, fields: &[String]) -> Call {
        let mut args: Vec<&str> = vec!["api", "graphql", "-F", "owner={owner}", "-F", "repo={repo}"];
        args.extend(fields.iter().map(String::as_str));
        let o = proc::run("gh", &args, Some(self.dir), None);
        if !o.ok {
            self.resp = Value::Null;
            if o.stderr.contains("Could not resolve to a PullRequest") {
                return Call::Gone;
            }
            if rate_limited(&o.stderr) {
                self.rl_limited = true;
            }
            self.errors.push(format!("gh ({label}): {}", o.stderr.replace('\n', " ")));
            return Call::Failed;
        }
        self.resp = serde_json::from_str(&o.stdout).unwrap_or(Value::Null);
        let rl = at(&self.resp, "data.rateLimit");
        let cost = rl.get("cost").and_then(Value::as_i64).unwrap_or(0);
        self.rl_cost = Some(self.rl_cost.unwrap_or(0) + cost);
        self.rl_remaining = rl.get("remaining").filter(|v| !v.is_null()).cloned().unwrap_or(Value::Null);
        self.rl_reset = rl.get("resetAt").and_then(Value::as_str).unwrap_or("").to_string();
        Call::Ok
    }

    fn fetch(&mut self, label: &str, number: &str, commits: bool) -> Call {
        self.call(
            label,
            &[
                "-F".into(),
                format!("number={number}"),
                "-F".into(),
                format!("commits={commits}"),
                "-f".into(),
                format!("query={QUERY}"),
            ],
        )
    }

    fn pr(&self) -> Value {
        at(&self.resp, "data.repository.pullRequest").clone()
    }

    /// The branch's PR (null for none), resp holding the last answer;
    /// commits also fetches its commits. A followed branch's is followed_pr.
    /// Otherwise by the number in branch.<b>.sd-pr, a cache (#339), unless
    /// GitHub has no such PR, its head is another branch, or it was closed
    /// unmerged: then discovery picks one and stores its number (unset for
    /// none), for a local branch only. Err when gh failed.
    fn lookup_pr(&mut self, b: &str, commits: bool) -> Result<Value, ()> {
        if git_out(self.dir, &["config", "--get", &format!("branch.{b}.sd-mode")]).as_deref() == Some("reviewer") {
            return self.followed_pr(b, commits);
        }
        let n = git_out(self.dir, &["config", "--get", &format!("branch.{b}.sd-pr")]).unwrap_or_default();
        let is_local = show_ref(self.dir, &format!("refs/heads/{b}"));
        let mut fetched = String::new();
        let mut fetched_pr = Value::Null;
        if !n.is_empty() {
            match self.fetch(b, &n, commits) {
                Call::Failed => return Err(()),
                Call::Ok => {
                    let pr = self.pr();
                    let head = str_or(at(&pr, "headRefName"), "");
                    let st = str_or(at(&pr, "state"), "");
                    if head == b && st != "CLOSED" {
                        return Ok(pr);
                    }
                    if head == b {
                        fetched = n.clone();
                        fetched_pr = pr;
                    }
                }
                Call::Gone => {}
            }
        }
        // -f, not -F, for the branch: -F would send "1234" or "true" as a non-string.
        if let Call::Failed | Call::Gone = self.call(b, &["-f".into(), format!("branch={b}"), "-f".into(), format!("query={DISCOVER}")]) {
            return Err(());
        }
        let pick = pick_pr(&self.resp);
        let Some(pick) = pick else {
            if !n.is_empty() && is_local {
                let _ = git(self.dir, &["config", "--unset", &format!("branch.{b}.sd-pr")]);
            }
            return Ok(Value::Null);
        };
        // A lost race with another writer only costs a discovery next time.
        if pick != n && is_local {
            let _ = git(self.dir, &["config", &format!("branch.{b}.sd-pr"), &pick]);
        }
        if pick == fetched {
            return Ok(fetched_pr);
        }
        match self.fetch(b, &pick, commits) {
            Call::Ok => Ok(self.pr()),
            Call::Gone => {
                self.errors.push(format!("gh ({b}): GitHub has no PR #{pick}, which it just listed"));
                Err(())
            }
            Call::Failed => Err(()),
        }
    }

    /// A followed PR (#250): by its stored number alone, whatever its head is
    /// called; never discovered, never unset. Err when gh failed.
    fn followed_pr(&mut self, b: &str, commits: bool) -> Result<Value, ()> {
        let n = git_out(self.dir, &["config", "--get", &format!("branch.{b}.sd-pr")]).unwrap_or_default();
        if n.is_empty() {
            self.errors.push(format!("{b} is followed (reviewer mode) but has no branch.{b}.sd-pr"));
            return Ok(Value::Null);
        }
        match self.fetch(b, &n, commits) {
            Call::Ok => Ok(self.pr()),
            Call::Gone => {
                self.errors.push(format!("gh ({b}): GitHub has no PR #{n}"));
                Ok(Value::Null)
            }
            Call::Failed => Err(()),
        }
    }
}

/// headRefName also matches strangers' fork PRs from a same-named branch;
/// keep same-repo PRs and PRs from the viewer's own fork; an open one first.
fn pick_pr(resp: &Value) -> Option<String> {
    let me = at(resp, "data.viewer.login");
    let nodes: Vec<&Value> = arr(at(resp, "data.repository.pullRequests.nodes"))
        .iter()
        .filter(|x| at(x, "isCrossRepository") != &Value::Bool(true) || (!me.is_null() && at(x, "headRepositoryOwner.login") == me))
        .collect();
    let pick = nodes.iter().find(|x| at(x, "state").as_str() == Some("OPEN")).or(nodes.first())?;
    match at(pick, "number") {
        Value::Null => None,
        v => Some(text(v)),
    }
}

// ── flags: what chrismo should see in base..HEAD (#125) ──────────────────────

/// bash's [[ $p == $g ]]: * and ? match any characters, / included; [..] a class.
fn glob(p: &[char], s: &[char]) -> bool {
    match p.first() {
        None => s.is_empty(),
        Some('*') => (0..=s.len()).any(|i| glob(&p[1..], &s[i..])),
        Some('?') => !s.is_empty() && glob(&p[1..], &s[1..]),
        Some('[') => {
            let Some(end) = p.iter().skip(2).position(|c| *c == ']').map(|i| i + 2) else {
                return s.first() == Some(&'[') && glob(&p[1..], &s[1..]);
            };
            let Some(c) = s.first() else { return false };
            let mut set = &p[1..end];
            let neg = matches!(set.first(), Some('!' | '^'));
            if neg {
                set = &set[1..];
            }
            let mut hit = false;
            let mut i = 0;
            while i < set.len() {
                if i + 2 < set.len() && set[i + 1] == '-' {
                    hit |= set[i] <= *c && *c <= set[i + 2];
                    i += 3;
                } else {
                    hit |= set[i] == *c;
                    i += 1;
                }
            }
            hit != neg && glob(&p[end + 1..], &s[1..])
        }
        Some('\\') if p.len() > 1 => s.first() == Some(&p[1]) && glob(&p[2..], &s[1..]),
        Some(c) => s.first() == Some(c) && glob(&p[1..], &s[1..]),
    }
}

fn glob_match(p: &str, s: &str) -> bool {
    glob(&p.chars().collect::<Vec<_>>(), &s.chars().collect::<Vec<_>>())
}

/// A glob ending in / is a directory anywhere in the path; one with a /
/// matches the whole path; any other the file's name.
fn is_test(p: &str, globs: &[String]) -> bool {
    globs.iter().any(|g| {
        if g.ends_with('/') {
            format!("/{p}").contains(&format!("/{g}"))
        } else if g.contains('/') {
            glob_match(g, p)
        } else {
            glob_match(g, p.rsplit('/').next().unwrap_or(p))
        }
    })
}

/// Whether two paths' file names, up to the first dot, share a word, other
/// than one that test files' names all have (test_a.py, b_spec.rb).
fn shares_a_word(a: &str, b: &str) -> bool {
    let words = |p: &str| -> Vec<String> {
        let name = p.rsplit('/').next().unwrap_or(p);
        let stem = name.split('.').next().unwrap_or(name);
        stem.split(|c: char| !c.is_alphanumeric())
            .map(str::to_lowercase)
            .filter(|w| !w.is_empty() && !["test", "tests", "spec", "specs"].contains(&w.as_str()))
            .collect()
    };
    let wa = words(a);
    words(b).iter().any(|w| wa.contains(w))
}

/// The net diff's lines, tab-separated: +, line, 1, test (1|0), file, file,
/// text: an added line; -, first line, lines removed, 1, old file, new file,
/// first removed line: a hunk of a test file. -U0, so a hunk is only changed
/// lines; a removed line is always one the base had.
fn git_diff_lines(dir: &str, base: &str, hd: &str, tests: &HashSet<String>) -> Option<Vec<String>> {
    let d = git(dir, &[
        "-c", "core.quotePath=false", "diff", "-M", "-U0", "--no-color", "--no-ext-diff", "--no-textconv",
        "--src-prefix=a/", "--dst-prefix=b/", base, hd,
    ]);
    if !d.ok {
        return None;
    }
    let (mut old, mut new) = (String::new(), String::new());
    let (mut ol, mut nl, mut ro, mut rn, mut cnt, mut first) = (0i64, 0i64, 0i64, 0i64, 0i64, false);
    let mut out = Vec::new();
    let num = |s: &str| -> i64 {
        let t: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
        t.parse().unwrap_or(0)
    };
    for l in d.stdout.strip_suffix('\n').unwrap_or(&d.stdout).split('\n') {
        if ro > 0 || rn > 0 {
            let rest = l.get(1..).unwrap_or("");
            if l.starts_with('-') && ro > 0 {
                if first && tests.contains(&old) {
                    out.push(format!("-\t{ol}\t{cnt}\t1\t{old}\t{new}\t{rest}"));
                }
                first = false;
                ro -= 1;
            } else if l.starts_with('+') && rn > 0 {
                out.push(format!("+\t{nl}\t1\t{}\t{new}\t{new}\t{rest}", i32::from(tests.contains(&new))));
                nl += 1;
                rn -= 1;
            }
            continue;
        }
        if l.starts_with("diff --git ") {
            old.clear();
            new.clear();
        } else if let Some(o) = l.strip_prefix("--- ") {
            let o = o.strip_prefix("a/").unwrap_or(o);
            old = o.strip_suffix('\t').unwrap_or(o).to_string();
        } else if let Some(n) = l.strip_prefix("+++ ") {
            let n = n.strip_prefix("b/").unwrap_or(n);
            new = n.strip_suffix('\t').unwrap_or(n).to_string();
        } else if l.starts_with("@@ ") {
            let f: Vec<&str> = l.split_whitespace().collect();
            let (o, w) = (f.get(1).map_or("", |x| &x[1..]), f.get(2).map_or("", |x| x.get(1..).unwrap_or("")));
            let os: Vec<&str> = o.split(',').collect();
            let ws: Vec<&str> = w.split(',').collect();
            ol = num(os[0]);
            ro = if os.len() > 1 { num(os[1]) } else { 1 };
            nl = num(ws[0]);
            rn = if ws.len() > 1 { num(ws[1]) } else { 1 };
            cnt = ro;
            first = true;
        }
    }
    Some(out)
}

fn compile_all(v: &Value) -> Option<Vec<Regex>> {
    arr(v).iter().map(|p| p.as_str().and_then(|p| Regex::new(p).ok())).collect()
}

/// {kind, file, line, n, text, commits} for each flagged line or test-file
/// hunk: commits is the one that added the line (blame), or for a test-edit
/// every commit in base..head that touched the file. Err with a message on
/// an invalid pattern or a failed step. One blame per file with hits.
fn flags_json(dir: &str, base: &str, hd: &str, cfg: &Value, cfg_text: &str) -> Result<Value, String> {
    let (Some(sup), Some(skips)) = (compile_all(at(cfg, "suppressions")), compile_all(at(cfg, "skips"))) else {
        return Err(format!("settings.flags has an invalid regex: {cfg_text}"));
    };
    let globs: Vec<String> = arr(at(cfg, "testFiles")).iter().map(text).collect();
    let changed = git(dir, &["-c", "core.quotePath=false", "diff", "-M", "--name-status", base, hd]);
    if !changed.ok {
        return Err(format!("couldn't list the files {}..{} changes", &base[..7.min(base.len())], &hd[..7.min(hd.len())]));
    }
    // A test file renamed to a path that isn't one stops running, and with no
    // change (100% similar) the diff has no hunk to show it: one test-edit
    // record for the whole file, as git_diff_lines writes them.
    let mut tests = HashSet::new();
    let mut moved = Vec::new();
    let (mut deleted, mut added) = (Vec::new(), Vec::new());
    for l in changed.stdout.lines() {
        let mut f = l.split('\t');
        let (st, a, b) = (f.next().unwrap_or(""), f.next().unwrap_or(""), f.next().unwrap_or(""));
        if a.is_empty() {
            continue;
        }
        for p in [a, b] {
            if !p.is_empty() && is_test(p, &globs) {
                tests.insert(p.to_string());
            }
        }
        if st.starts_with('R') && is_test(a, &globs) && !is_test(b, &globs) {
            let n = git(dir, &["cat-file", "blob", &format!("{base}:{a}")]).stdout.matches('\n').count();
            moved.push(format!("-\t1\t{n}\t1\t{a}\t{b}\tmoved to {b}, out of the test files"));
        }
        if is_test(a, &globs) && (st == "D" || st == "A") {
            (if st == "D" { &mut deleted } else { &mut added }).push(a.to_string());
        }
    }
    // A test file renamed and edited past -M's 50% similarity is a delete and
    // an add, and reads as the whole file deleted (#327). A deleted and an
    // added test file that -M20% pairs, with a word of their names in common,
    // is that rename: one record of its lines changed, not the deletion's.
    if !deleted.is_empty() && !added.is_empty() {
        let mut args = vec!["--literal-pathspecs", "-c", "core.quotePath=false", "diff", "-M20%", "--name-status", base, hd, "--"];
        args.extend(deleted.iter().chain(&added).map(String::as_str));
        for l in git(dir, &args).stdout.lines() {
            let f: Vec<&str> = l.split('\t').collect();
            let [st, a, b] = f[..] else { continue };
            if !st.starts_with('R') || !shares_a_word(a, b) {
                continue;
            }
            let ns = git(dir, &["diff", "--numstat", &format!("{base}:{a}"), &format!("{hd}:{b}")]).stdout;
            let Some(n) = ns.split('\t').nth(1).and_then(|n| n.parse::<usize>().ok()) else { continue };
            let all = git(dir, &["cat-file", "blob", &format!("{base}:{a}")]).stdout.matches('\n').count();
            tests.remove(a);
            if n == 0 {
                continue;
            }
            moved.push(format!("-\t1\t{n}\t1\t{a}\t{b}\trenamed to {b}, {n} of its {all} lines changed"));
        }
    }
    let mut lines = git_diff_lines(dir, base, hd, &tests).ok_or("couldn't read the diff")?;
    lines.extend(moved);
    let hit = |t: &str, res: &[Regex]| res.iter().any(|r| r.is_match(t));
    let mut hits: Vec<[String; 6]> = Vec::new();
    for l in &lines {
        let f: Vec<&str> = l.split('\t').collect();
        let t = f.get(6..).map_or(String::new(), |r| r.join("\t"));
        let k = if f[0] == "-" {
            "test-edit"
        } else if f.get(3) == Some(&"1") && hit(&t, &skips) {
            "skip"
        } else if hit(&t, &sup) {
            "suppression"
        } else {
            continue;
        };
        let g = |i: usize| f.get(i).unwrap_or(&"").to_string();
        // read -r with IFS=tab: the last field loses its outer tabs
        hits.push([k.to_string(), g(1), g(2), g(4), g(5), t.trim_matches('\t').to_string()]);
    }
    // One blame per file, every hit's line at once; its porcelain headers are
    // "<sha> <orig-line> <final-line> [<count>]".
    let mut ranges: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for h in hits.iter().filter(|h| h[0] != "test-edit") {
        ranges.entry(h[3].clone()).or_default().push(h[1].clone());
    }
    let mut blamed: HashMap<(String, String), String> = HashMap::new();
    for (f, ls) in &ranges {
        let mut args: Vec<String> = vec!["blame".into(), "--porcelain".into()];
        for ln in ls {
            args.push("-L".into());
            args.push(format!("{ln},{ln}"));
        }
        args.extend([hd.to_string(), "--".into(), f.clone()]);
        let o = git(dir, &args.iter().map(String::as_str).collect::<Vec<_>>());
        for l in o.stdout.lines() {
            let w: Vec<&str> = l.split_whitespace().collect();
            if w.len() >= 3
                && w[0].len() == 40
                && w[0].chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
                && !w[2].is_empty()
                && w[2].chars().all(|c| c.is_ascii_digit())
            {
                blamed.insert((f.clone(), w[2].to_string()), w[0].to_string());
            }
        }
    }
    let mut touched: HashMap<String, Value> = HashMap::new();
    let mut out = Vec::new();
    for [kind, line, n, file, new, text] in hits {
        let commits = if kind == "test-edit" {
            if !touched.contains_key(&file) {
                let range = format!("{base}..{hd}");
                let mut args = vec!["log", "--format=%H", &range, "--", &file];
                if new != "/dev/null" && new != file {
                    args.push(&new);
                }
                let shas = git_out(dir, &args).ok_or(format!("couldn't list the commits that touched {file}"))?;
                touched.insert(file.clone(), shas.lines().filter(|s| !s.is_empty()).map(|s| json!(s)).collect());
            }
            touched[&file].clone()
        } else {
            blamed.get(&(file.clone(), line.clone())).map_or(json!([]), |s| json!([s]))
        };
        let text: String = text.replace('\t', " ").chars().filter(|c| !c.is_control()).take(200).collect();
        let raw = |s: &str| serde_json::from_str::<Value>(s).unwrap_or(Value::Null);
        out.push(json!({"kind": kind, "file": file, "line": raw(&line), "n": raw(&n), "text": text, "commits": commits}));
    }
    Ok(Value::Array(out))
}

// ── assemble ─────────────────────────────────────────────────────────────────

pub(crate) fn unbot(s: &str) -> &str {
    s.strip_suffix("[bot]").unwrap_or(s)
}

fn login(a: &Value) -> String {
    unbot(str_or(at(a, "login"), "")).to_string()
}

fn is_bot(a: &Value, bots: &[String]) -> bool {
    at(a, "__typename").as_str() == Some("Bot") || {
        let l = login(a);
        !l.is_empty() && bots.contains(&l)
    }
}

fn ignored(a: &Value, ign: &[String]) -> bool {
    ign.contains(&login(a))
}

/// First line with text, HTML comments, tags and heading / quote markers
/// removed. SuperDB's \s was RE2's, ASCII only: hence the spelled-out classes.
fn excerpt(b: &Value) -> String {
    static R: OnceLock<[Regex; 4]> = OnceLock::new();
    let [comment, tag, lead, trail] = R.get_or_init(|| {
        [re(r"(?s)<!--.*?-->"), re(r"<[^>]*>"), re(r"^[#>\t\n\x0C\r ]+"), re(r"[\t\n\x0C\r ]+$")]
    });
    let body = comment.replace_all(str_or(b, ""), "").into_owned();
    body.split('\n')
        .map(|l| {
            let l = tag.replace_all(l, "");
            let l = lead.replace_all(&l, "");
            trail.replace_all(&l, "").into_owned()
        })
        .find(|l| !l.is_empty())
        .unwrap_or_default()
        .chars()
        .take(100)
        .collect()
}

fn detail(x: &Value) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("author".into(), at(x, "author.login").clone());
    m.insert("url".into(), at(x, "url").clone());
    m.insert("excerpt".into(), json!(excerpt(at(x, "body"))));
    m
}

fn ci_class(x: &Value) -> &'static str {
    let v = if at(x, "__typename").as_str() == Some("CheckRun") {
        if at(x, "status").as_str() != Some("COMPLETED") {
            "pending"
        } else {
            str_or(at(x, "conclusion"), "")
        }
    } else {
        str_or(at(x, "state"), "")
    };
    match v {
        "pending" | "PENDING" | "EXPECTED" => "pending",
        "SUCCESS" | "NEUTRAL" | "SKIPPED" => "ok",
        "CANCELLED" | "STALE" => "cancelled",
        "STARTUP_FAILURE" | "ACTION_REQUIRED" => "blocked",
        _ => "fail",
    }
}

fn first_of(v: &Value, a: &str, b: &str) -> Value {
    let x = at(v, a);
    if x.is_null() {
        at(v, b).clone()
    } else {
        x.clone()
    }
}

#[allow(clippy::too_many_arguments)]
fn owed_item(id: &Value, ty: &str, path: Value, line: Value, outdated: Value, kind: &str, last: Value, mark: &str, d: Map<String, Value>) -> Value {
    let mut m = Map::new();
    m.insert("id".into(), id.clone());
    m.insert("type".into(), json!(ty));
    m.insert("path".into(), path);
    m.insert("line".into(), line);
    m.insert("outdated".into(), outdated);
    m.insert("kind".into(), json!(kind));
    m.insert("last".into(), last);
    m.insert("mark".into(), json!(mark));
    m.extend(d);
    Value::Object(m)
}

/// Review thread <t>'s owed item, opened by <first>. It carries its last
/// comment's id: a bot comment since an unposted round makes it new (#357).
fn thread_item(t: &Value, first: &Value, kind: &str) -> Value {
    let last = at(t, "last.nodes").get(0).unwrap_or(&NULL);
    let mut d = detail(first);
    d.insert("last_id".into(), at(last, "id").clone());
    let field = |k| at(t, k).clone();
    owed_item(at(t, "id"), "thread", field("path"), field("line"), field("isOutdated"), kind, at(last, "author.login").clone(), "!", d)
}

struct Facts {
    reviewer: bool,
    branch: String,
    head: String,
    task: String,
    pr: Value,
    parent: String,
    parent_source: &'static str,
    parent_pr: Value,
    parent_merged: Value,
    parent_local: Value,
    parent_remote: Value,
    parent_unpushed: bool,
    target: String,
    default_branch: String,
    sync_ahead: Value,
    behind: Value,
    up_to_date: Value,
    old_base: String,
    by_tree: bool,
    conflict: Value,
    replays_parent: Value,
    undone: i64,
    remote_sha: String,
    ahead: i64,
    push_behind: i64,
    diverged: bool,
    author_pushed: Value,
    remote_ours: Value,
    theirs: Value,
    remote_made: Value,
    dirty: Value,
    held: Value,
    stale: Vec<String>,
    lag: Vec<String>,
    r_local: Value,
    r_bot: Value,
    r_accepted: Value,
    r_findings: Value,
    r_findings_at_head: bool,
    review_rounds: Value,
    question: Value,
    failed: Value,
    verify: Value,
    flags: Value,
    flags_error: String,
    gh_ok: bool,
    parent_ok: bool,
    viewer: String,
    bots: Value,
    bot_command: String,
    bot_triggered: String,
    accepted_after: String,
    bot_timeout: Value,
    now_ns: i128,
    required: Value,
    ignore: Value,
    ignore_body: Vec<Regex>,
    handled: Vec<String>,
    unposted: Vec<String>,
    refs: Vec<(String, String)>,
    enrolled: String,
    harness: String,
    profile: Value,
    session: String,
    lock: String,
    running: Value,
    rl_cost: Option<i64>,
    rl_remaining: Value,
    rl_reset: String,
    rl_limited: bool,
    /// usd, unknown and stray.
    spend: (Value, Value, Value),
    branch_budget: Value,
    errors: Vec<String>,
}

fn strings(v: &Value) -> Vec<String> {
    arr(v).iter().map(|x| unbot(str_or(x, "")).to_string()).collect()
}

fn assemble(s: &Facts) -> Value {
    let bots = strings(&s.bots);
    let ign = strings(&s.ignore);
    let matches = |b: &str| s.ignore_body.iter().any(|r| r.is_match(b));
    let pr = &s.pr;
    let c = at(pr, "commits.nodes").get(0).map_or(&NULL, |n| at(n, "commit"));
    // StatusContexts arrive latest-only; CheckRuns don't (#195)
    let cr: Vec<(Value, &str, i64, String)> = arr(at(c, "statusCheckRollup.contexts.nodes"))
        .iter()
        .map(|x| {
            let name = first_of(x, "name", "context");
            let key = format!(
                "{}/{}/{}/{}/{}",
                text(at(x, "__typename")),
                str_or(at(x, "checkSuite.app.slug"), ""),
                str_or(at(x, "checkSuite.workflowRun.workflow.name"), ""),
                str_or(at(x, "checkSuite.workflowRun.event"), ""),
                text(&name)
            );
            (name, ci_class(x), at(x, "databaseId").as_i64().unwrap_or(0), key)
        })
        .collect();
    let ctx: Vec<(Value, &str)> = cr
        .iter()
        .filter(|e| !cr.iter().any(|x| x.3 == e.3 && x.2 > e.2))
        .map(|e| (e.0.clone(), e.1))
        .collect();
    let reviews = arr(at(pr, "reviews.nodes"));
    let comments = arr(at(pr, "comments.nodes"));
    let trig = if s.bot_triggered.is_empty() { "~" } else { &s.bot_triggered };
    let author = |x: &Value| at(x, "author").clone();
    let empty = json!({});
    let or_empty = |v: Value| if v.is_null() { empty.clone() } else { v };
    // posted strictly after the trigger, which keeps out the trigger comment
    // itself when the viewer is a Bot account
    let br = reviews
        .iter()
        .chain(comments)
        .filter(|x| {
            let a = or_empty(author(x));
            let when = at(x, "submittedAt").as_str().or(at(x, "createdAt").as_str()).unwrap_or("");
            is_bot(&a, &bots) && !ignored(&a, &ign) && when > trig && !matches(str_or(at(x, "body"), ""))
        })
        .count();
    let hv: Vec<(usize, &Value)> = reviews
        .iter()
        .filter(|r| {
            let a = or_empty(author(r));
            !(is_bot(&a, &bots) || ignored(&a, &ign))
                && matches!(str_or(at(r, "state"), ""), "APPROVED" | "CHANGES_REQUESTED" | "DISMISSED")
        })
        .enumerate()
        .collect();
    let required = arr(&s.required);
    let mi: Vec<Value> = required.iter().filter(|x| !ctx.iter().any(|(n, _)| n == *x)).cloned().collect();

    let mut owed: Vec<Value> = Vec::new();
    for t in arr(at(pr, "reviewThreads.nodes")) {
        let first = at(t, "first.nodes").get(0).cloned().unwrap_or(Value::Null);
        let last = at(t, "last.nodes").get(0).map_or(&NULL, |n| at(n, "author.login"));
        if at(t, "isResolved") == &Value::Bool(true)
            || str_or(last, "") == s.viewer
            || ignored(&or_empty(at(&first, "author").clone()), &ign)
        {
            continue;
        }
        let kind = if is_bot(&or_empty(at(&first, "author").clone()), &bots) { "bot" } else { "human" };
        owed.push(thread_item(t, &or_empty(first), kind));
    }
    let handled: HashSet<&str> = s.handled.iter().map(String::as_str).collect();
    let typed = reviews.iter().map(|n| (n, "review")).chain(comments.iter().map(|n| (n, "comment")));
    static NONBLANK: OnceLock<Regex> = OnceLock::new();
    let nonblank = NONBLANK.get_or_init(|| re(r"[^\t\n\x0C\r ]"));
    for (n, ty) in typed {
        let a = or_empty(author(n));
        let body = str_or(at(n, "body"), "");
        let id = at(n, "id");
        if is_bot(&a, &bots) && !ignored(&a, &ign) && nonblank.is_match(body) && !matches(body) && !id.as_str().is_some_and(|i| handled.contains(i)) {
            let mark = if at(n, "state").as_str() == Some("CHANGES_REQUESTED") { "!" } else { "i" };
            owed.push(owed_item(id, ty, Value::Null, Value::Null, Value::Null, "bot", at(n, "author.login").clone(), mark, detail(n)));
        }
    }
    // A reviewer verdict is their latest approving, requesting or dismissed review.
    let who = |r: &Value| str_or(at(r, "author.login"), "").to_string();
    let mut verdicts: Vec<&Value> = hv
        .iter()
        .filter(|(j, r)| {
            at(r, "state").as_str() == Some("CHANGES_REQUESTED") && !hv.iter().any(|(k, x)| k > j && who(x) == who(r))
        })
        .map(|(_, r)| *r)
        .collect();
    verdicts.sort_by_key(|r| who(r));
    for r in verdicts {
        owed.push(owed_item(at(r, "id"), "review", Value::Null, Value::Null, Value::Null, "human", at(r, "author.login").clone(), "!", detail(r)));
    }
    let ob: Vec<&Value> = owed.iter().filter(|o| o["kind"] == "bot").collect();
    let gh = |v: Value| if s.gh_ok { v } else { Value::Null };
    let cnt = |t: &str| ob.iter().filter(|o| o["type"] == t).count();

    let pr_out = if pr.is_null() {
        Value::Null
    } else {
        json!({"number": at(pr, "number"), "url": at(pr, "url"), "state": at(pr, "state"), "draft": at(pr, "isDraft"),
               "author": at(pr, "author.login"), "base": at(pr, "baseRefName"), "head_sha": at(pr, "headRefOid"), "mergeable": at(pr, "mergeable"),
               "merge_state": at(pr, "mergeStateStatus"), "review_decision": at(pr, "reviewDecision"),
               "review_requests": arr(at(pr, "reviewRequests.nodes")).iter()
                   .map(|n| first_of(n, "requestedReviewer.login", "requestedReviewer.slug"))
                   .filter(|v| !v.is_null()).collect::<Vec<_>>()})
    };
    let parent_pr = if s.parent_pr.is_null() {
        Value::Null
    } else {
        let p = &s.parent_pr;
        json!({"number": at(p, "number"), "state": at(p, "state"), "head_sha": at(p, "headRefOid"),
               "commits": arr(at(p, "allCommits.nodes")).iter().map(|n| at(n, "commit.oid").clone()).collect::<Vec<_>>()})
    };
    let has = |k: &str| ctx.iter().any(|(_, x)| *x == k);
    let names = |k: &str| ctx.iter().filter(|(_, x)| *x == k).map(|(n, _)| n.clone()).collect::<Vec<_>>();
    let verdict = if ctx.is_empty() && mi.is_empty() {
        "none"
    } else if has("fail") {
        "red"
    } else if has("blocked") {
        "blocked"
    } else if has("pending") || !mi.is_empty() {
        "pending"
    } else if has("cancelled") {
        "inconclusive"
    } else {
        "green"
    };
    let bot_pin = if s.r_bot.is_null() {
        Value::Null
    } else {
        let mut m = s.r_bot.as_object().cloned().unwrap_or_default();
        let unknown = at(&s.r_bot, "status").as_str() == Some("none") || s.bot_triggered.is_empty() || !s.gh_ok;
        m.insert("triggered".into(), nn(&s.bot_triggered));
        m.insert("replied".into(), if unknown { Value::Null } else { json!(br > 0) });
        m.insert(
            "waited".into(),
            if unknown {
                Value::Null
            } else {
                epoch_ns(&s.bot_triggered).map_or(Value::Null, |t| json!(((s.now_ns - t) / 1_000_000_000) as i64))
            },
        );
        Value::Object(m)
    };
    let accepted = if s.r_accepted.is_null() {
        Value::Null
    } else {
        let mut m = s.r_accepted.as_object().cloned().unwrap_or_default();
        m.insert("after".into(), nn(&s.accepted_after));
        Value::Object(m)
    };
    let findings = if s.r_findings.is_null() {
        Value::Null
    } else {
        let f = &s.r_findings;
        let items = arr(at(f, "findings"));
        let blocking: Vec<&Value> = items.iter().filter(|x| str_or(at(x, "severity"), "") != "nit").collect();
        let declined = !blocking.is_empty() && blocking.iter().all(|x| str_or(at(x, "action"), "") == "declined");
        json!({"sha": at(f, "head"), "round": at(f, "round"), "n": blocking.len(), "at_head": s.r_findings_at_head,
               "declined": declined, "cap": s.review_rounds, "items": at(f, "findings")})
    };
    let github = if !s.reviewer || !s.gh_ok {
        Value::Null
    } else {
        let mine: Vec<&Value> = reviews.iter().filter(|r| str_or(at(r, "author.login"), "") == s.viewer).collect();
        let pending = mine.iter().any(|r| at(r, "state").as_str() == Some("PENDING"));
        // A lone inline comment or thread reply is a COMMENTED review with no body.
        let verdict = |r: &Value| match at(r, "state").as_str() {
            Some("APPROVED" | "CHANGES_REQUESTED") => true,
            Some("COMMENTED") => !str_or(at(r, "body"), "").trim().is_empty(),
            _ => false,
        };
        match mine.iter().rev().copied().find(|r| verdict(r)) {
            None if !pending => Value::Null,
            None => json!({"sha": null, "state": null, "at_head": false, "pending": true}),
            Some(r) => json!({"sha": at(r, "commit.oid"), "state": at(r, "state"),
                              "at_head": at(r, "commit.oid").as_str() == Some(&s.head), "pending": pending}),
        }
    };
    let prefix = format!("refs/sheepdog/{}/", s.branch);
    let refs: Map<String, Value> = s
        .refs
        .iter()
        .map(|(r, sha)| (r.chars().skip(prefix.chars().count()).collect::<String>(), json!(sha)))
        .collect();
    let jn = |x: &str| nn(x);
    json!({
        "branch": s.branch, "head": s.head, "task": jn(&s.task), "viewer": jn(&s.viewer),
        "mode": if s.reviewer { json!("reviewer") } else { Value::Null },
        "pr": pr_out,
        "parent": {"name": s.parent, "source": s.parent_source, "merged": s.parent_merged, "local": s.parent_local,
                   "on_remote": s.parent_remote, "unpushed": s.parent_unpushed, "pr": parent_pr},
        "sync": {"target": s.target, "default": s.default_branch, "ahead": s.sync_ahead, "behind": s.behind,
                 "up_to_date": s.up_to_date, "old_base": jn(&s.old_base), "by_tree": s.by_tree, "conflict": s.conflict,
                 "undone": s.undone, "replays_parent": s.replays_parent},
        "push": {"remote_sha": jn(&s.remote_sha), "ahead": s.ahead, "behind": s.push_behind, "diverged": s.diverged, "author_pushed": s.author_pushed,
                 "remote_ours": s.remote_ours, "theirs": s.theirs, "remote_made": s.remote_made, "dirty": s.dirty,
                 "held": s.held},
        "fresh": if s.gh_ok && s.parent_ok { json!(s.stale.is_empty()) } else { Value::Null },
        "stale": s.stale,
        "lag": s.lag,
        "ci": {"sha": at(c, "oid"), "for_head": if c.is_null() { Value::Null } else { json!(at(c, "oid").as_str() == Some(&s.head)) },
               "verdict": verdict, "failing": names("fail"), "blocked": names("blocked"), "pending": names("pending"),
               "required": s.required, "missing": mi},
        "review": {"local": s.r_local, "bot": bot_pin, "accepted": accepted, "findings": findings, "github": github},
        "question": s.question,
        "failed": s.failed,
        "verify": s.verify,
        "flags": s.flags, "flags_error": jn(&s.flags_error),
        "threads": {"owed_bot": gh(json!(ob.len())),
                    "owed_human": gh(json!(owed.iter().filter(|o| o["kind"] == "human").count())),
                    "outdated": gh(json!(owed.iter().filter(|o| o["outdated"] == true).count())),
                    "by_type": gh(json!({"thread": cnt("thread"), "review": cnt("review"), "comment": cnt("comment")})),
                    "blocking": gh(json!(owed.iter().filter(|o| o["mark"] == "!").count())),
                    "info": gh(json!(owed.iter().filter(|o| o["mark"] == "i").count())),
                    "items": owed, "unposted": s.unposted},
        "bot": {"command": jn(&s.bot_command), "timeout": s.bot_timeout},
        "session": {"enrolled": s.enrolled == "true", "harness": jn(&s.harness), "profile": s.profile, "id": jn(&s.session),
                    "locked": !s.lock.is_empty(), "running": s.running},
        "spend": {"usd": s.spend.0, "unknown": s.spend.1, "budget": s.branch_budget, "stray": s.spend.2},
        "stored": {"handled": s.handled, "refs": refs},
        "rate_limit": {"cost": s.rl_cost, "remaining": s.rl_remaining, "reset_at": jn(&s.rl_reset), "limited": s.rl_limited},
        "errors": s.errors
    })
}

// ── observe ──────────────────────────────────────────────────────────────────

/// Every value of branch.<branch>.<key>.
fn config_all(dir: &str, branch: &str, key: &str) -> Vec<String> {
    git_out(dir, &["config", "--get-all", &format!("branch.{branch}.{key}")]).map(|s| s.lines().map(str::to_string).collect()).unwrap_or_default()
}

fn cfg_text(dir: &str, path: &str, b: &str, as_json: bool) -> Result<String, String> {
    config::get_text_in(dir, path, b, as_json).map_err(config_failed)
}

fn config_failed(lines: Vec<String>) -> String {
    let mut out: String = lines.iter().map(|l| format!("sd config: {l}\n")).collect();
    out.push_str(&fail("sd config failed"));
    out
}

/// A value `sd config get` printed.
fn raw_json(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|_| json!(text))
}

fn pin_status(dir: &str, branch: &str, kind: &str, head: &str, target_ref: &str, old_base: &str, stackless: bool) -> Value {
    let pin = rev(dir, &format!("refs/sheepdog/{branch}/{kind}"));
    if pin.is_empty() {
        return json!({"sha": null, "status": "none"});
    }
    if pin == head {
        return json!({"sha": pin, "status": "covered"});
    }
    if is_ancestor(dir, &pin, head) {
        return json!({"sha": pin, "status": "uncovered", "uncovered": count(dir, &[&format!("{pin}..{head}")])});
    }
    let against = if target_ref.is_empty() { head } else { target_ref };
    let mut pin_base = rev(dir, &format!("refs/sheepdog/{branch}/{kind}-base"));
    let mut head_base = old_base.to_string();
    // The merge-base only when not stacked, as in O3: with a stacked parent,
    // open or merged, it can lie below the parent's commits (#97).
    if stackless {
        if pin_base.is_empty() {
            pin_base = git_out(dir, &["merge-base", &pin, against]).unwrap_or_default();
        }
        if head_base.is_empty() {
            head_base = git_out(dir, &["merge-base", head, against]).unwrap_or_default();
        }
    }
    let mut status = "unknown";
    if !pin_base.is_empty() && !head_base.is_empty() {
        status = if patch_id(dir, &pin_base, &pin) == patch_id(dir, &head_base, head) { "rewritten-same" } else { "rewritten-changed" };
    }
    json!({"sha": pin, "status": status})
}

/// A draft's base branch's required checks, sorted: rulesets, plus classic
/// protection as the branch shows it. A 404 or 403 counts as none, unless
/// it's a rate limit; any other failure is Err with gh's stderr.
fn required_checks(dir: &str, base: &str) -> Result<Value, String> {
    let rest = |path: &str| -> Result<Value, String> {
        let o = proc::run("gh", &["api", path], Some(dir), None);
        if o.ok {
            return serde_json::from_str(&o.stdout).map_err(|_| String::new());
        }
        let http = o.stderr.contains("HTTP 404") || o.stderr.contains("HTTP 403");
        if http && !rate_limited(&o.stderr) {
            Ok(Value::Null)
        } else {
            Err(o.stderr)
        }
    };
    let rules = rest(&format!("repos/{{owner}}/{{repo}}/rules/branches/{base}?per_page=100"))?;
    let branch = rest(&format!("repos/{{owner}}/{{repo}}/branches/{base}"))?;
    let mut out: Vec<String> = Vec::new();
    for r in arr(&rules).iter().filter(|r| at(r, "type").as_str() == Some("required_status_checks")) {
        out.extend(arr(at(r, "parameters.required_status_checks")).iter().map(|c| text(at(c, "context"))));
    }
    out.extend(arr(at(&branch, "protection.required_status_checks.contexts")).iter().map(text));
    out.extend(arr(at(&branch, "protection.required_status_checks.checks")).iter().map(|c| text(at(c, "context"))));
    out.sort();
    out.dedup();
    Ok(json!(out))
}

fn parent_commits(parent_pr: &Value) -> HashSet<String> {
    arr(at(parent_pr, "allCommits.nodes")).iter().map(|n| text(at(n, "commit.oid"))).collect()
}

/// O3: the merged parent's PR commits a restack would replay (old_base..HEAD),
/// newest first: a wrong base. [] unless the parent merged and HEAD is behind
/// with a base; null when git can't list it.
fn replays_parent(dir: &str, head: &str, old_base: &str, parent_merged: &Value, up_to_date: &Value, parent_pr: &Value) -> Value {
    if old_base.is_empty() || *parent_merged != true || *up_to_date != false {
        return json!([]);
    }
    let Some(list) = git_out(dir, &["rev-list", "--topo-order", &format!("{old_base}..{head}")]) else { return Value::Null };
    let commits = parent_commits(parent_pr);
    json!(list.lines().filter(|c| commits.contains(*c)).collect::<Vec<_>>())
}

/// sd state --json <branch>: its record, NEXT included, as seen from <dir>,
/// any worktree of the repo. gh asks the repo GH_REPO names, which the caller
/// sets from <remote> (git::gh_repo), else <dir>'s. Err is what sd state
/// prints on stderr before it exits 1; warnings go to stderr as they come.
pub fn observe(dir: &str, branch: &str, remote: &str) -> Result<Value, String> {
    let head = rev(dir, &format!("refs/heads/{branch}"));
    if head.is_empty() {
        return Err(fail(format!("no such branch: {branch}")));
    }
    let cfg = |k: &str| git_out(dir, &["config", "--get", &format!("branch.{branch}.{k}")]).unwrap_or_default();
    let setting = |k: &str, as_json: bool| cfg_text(dir, &format!("settings.{k}"), branch, as_json);
    let bots = raw_json(&setting("botLogin", true)?);
    let ignore = raw_json(&setting("botIgnore", true)?);
    let ignore_body_text = setting("botIgnoreBody", true)?;
    let bot_command = setting("botCommand", false)?;
    let bot_timeout = raw_json(&setting("botTimeout", false)?);
    let bot_triggered = cfg("sd-bot-triggered"); // when trigger posted its comment, by GitHub's clock (#63)
    let accepted_after = cfg("sd-accepted-after"); // the bot_triggered review --accept followed (#181)
    let review_rounds = raw_json(&setting("reviewRounds", false)?);
    let branch_budget = raw_json(&setting("branchBudget", true)?);
    let profile = match config::get_text_in(dir, "profile", branch, true) {
        Ok(text) => raw_json(&text),
        Err(lines) if lines.is_empty() => Value::Null,
        Err(lines) => return Err(config_failed(lines)),
    };
    let mut errors: Vec<String> = Vec::new();
    let spend = branch_spend(dir, branch).unwrap_or_else(|| {
        errors.push(format!("usage: can't read {}", usage::file()));
        (Value::Null, Value::Null, Value::Null)
    });
    let ignore_body = compile_all(&raw_json(&ignore_body_text))
        .ok_or_else(|| fail(format!("botIgnoreBody has an invalid regex: {ignore_body_text}")))?;

    let reviewer = cfg("sd-mode") == "reviewer";
    let mut gh = Gh { dir, resp: Value::Null, errors: Vec::new(), rl_cost: None, rl_remaining: Value::Null, rl_reset: String::new(), rl_limited: false };

    // ── O1 PR, O2 parent ──
    let (mut pr, mut viewer, mut default_branch, mut gh_ok) = (Value::Null, String::new(), String::new(), false);
    if let Ok(p) = gh.lookup_pr(branch, false) {
        gh_ok = true;
        pr = p;
        viewer = str_or(at(&gh.resp, "data.viewer.login"), "").to_string();
        default_branch = str_or(at(&gh.resp, "data.repository.defaultBranchRef.name"), "").to_string();
    }
    errors.append(&mut gh.errors);
    if default_branch.is_empty() {
        default_branch = git::remote_default(dir, remote);
    }
    let mut parent = cfg("sd-parent");
    let mut parent_source = "config";
    if parent.is_empty() {
        if !pr.is_null() {
            parent = text(at(&pr, "baseRefName"));
            parent_source = "pr";
        } else {
            parent = default_branch.clone();
            parent_source = "default";
            // A plain git branch from another's tip records no parent (#94): flag it,
            // unless gh failed and a PR we couldn't see may name the parent.
            if gh_ok {
                match stacked_on(dir, branch, remote, &default_branch) {
                    Ok((name, _)) => errors.push(format!("stacked on {name} without sheepdog knowing: sd act enroll records it")),
                    Err((2, names)) => errors.push(format!(
                        "stacked on {} without sheepdog knowing: record one with git config branch.{branch}.sd-parent <name>",
                        names.replace(' ', " or ")
                    )),
                    Err((3, names)) => errors.push(stacked_unclear(branch, &names, &default_branch)),
                    Err(_) => {}
                }
            }
        }
    }
    let mut parent_pr = Value::Null;
    let mut parent_ok = gh_ok || parent_source == "config";
    if parent != default_branch {
        match gh.lookup_pr(&parent, true) {
            Ok(p) => parent_pr = p,
            Err(()) => parent_ok = false,
        }
        errors.append(&mut gh.errors);
    }
    let parent_merged = if parent_ok { json!(at(&parent_pr, "state").as_str() == Some("MERGED")) } else { Value::Null };
    let target = if parent_merged == true { default_branch.clone() } else { parent.clone() };

    // ── O3a restack undone (#47) ──
    // After a restack, a plain git push is rejected and git's advice (pull
    // --rebase) replays the target's commits as copies, orphaning the base pin.
    // Evidence, all three: the pin isn't an ancestor; the remote head is; and
    // commits above the remote head repeat a target commit's author date and
    // subject.
    let mut undone = 0i64;
    let u_pin = rev(dir, &format!("refs/sheepdog/{branch}/base"));
    let u_tr = rev(dir, &format!("refs/remotes/{remote}/{target}"));
    let u_rs = rev(dir, &format!("refs/remotes/{remote}/{branch}"));
    if !u_pin.is_empty() && !u_tr.is_empty() && !u_rs.is_empty() && is_ancestor(dir, &u_rs, &head) && !is_ancestor(dir, &u_pin, &head) {
        if let Some(u_mb) = git_out(dir, &["merge-base", &u_tr, &head]) {
            let set = |range: String| -> HashSet<String> {
                git(dir, &["log", "--format=%at %s", &range]).stdout.lines().map(str::to_string).collect()
            };
            let ours = set(format!("{u_rs}..{head}"));
            undone = set(format!("{u_mb}..{u_tr}")).intersection(&ours).count() as i64;
        }
    }

    // ── O3 sync vs parent ──
    // A stacked branch's parent can exist only here (#105). parent.unpushed:
    // only a parent known to have no PR counts.
    let (mut parent_local, mut parent_remote, mut parent_unpushed, mut parent_landed) = (Value::Null, Value::Null, false, false);
    if parent != default_branch {
        let l = show_ref(dir, &format!("refs/heads/{parent}"));
        let r = !rev(dir, &format!("refs/remotes/{remote}/{parent}")).is_empty();
        parent_local = json!(l);
        parent_remote = json!(r);
        if l && !r && parent_merged == false && parent_pr.is_null() {
            if is_ancestor(dir, &format!("refs/heads/{parent}"), &format!("refs/remotes/{remote}/{default_branch}")) {
                parent_landed = true;
            } else {
                parent_unpushed = true;
            }
        }
    }
    let target_ref = rev(dir, &format!("refs/remotes/{remote}/{target}"));
    let (mut sync_ahead, mut behind, mut up_to_date, mut old_base, mut conflict, mut by_tree) =
        (Value::Null, Value::Null, Value::Null, String::new(), Value::Null, false);
    if target_ref.is_empty() {
        if target == parent && parent_landed {
            errors.push(format!("{parent} has no commits beyond {remote}/{default_branch} and isn't on {remote}: if it landed, point sd-parent at {default_branch}; else commit to it and ship it first"));
        } else if target != parent || !parent_unpushed {
            errors.push(format!("no {remote}/{target} ref (not fetched?)"));
        }
    } else {
        sync_ahead = json!(count(dir, &[&format!("{target_ref}..{head}")]));
        behind = json!(count(dir, &[&format!("{head}..{target_ref}")]));
        let base_pin = rev(dir, &format!("refs/sheepdog/{branch}/base"));
        let merged_head = str_or(at(&parent_pr, "headRefOid"), "").to_string();
        if !base_pin.is_empty() && !is_ancestor(dir, &base_pin, &head) && undone == 0 {
            errors.push(format!("base pin {} is no longer in {branch}'s history (rewritten outside sheepdog?)", &base_pin[..7]));
        }
        // parent_in_head: HEAD holds the merged parent's whole change (squash merges only).
        let parent_in_head = || -> bool {
            if merged_head.is_empty() || !git_ok(dir, &["cat-file", "-e", &format!("{merged_head}^{{commit}}")]) {
                return false;
            }
            if is_ancestor(dir, &merged_head, &target_ref) {
                return false;
            }
            let Some(base) = git_out(dir, &["merge-base", &merged_head, &target_ref]) else { return false };
            if git_out(dir, &["merge-base", &head, &target_ref]).as_deref() != Some(base.as_str()) {
                return false;
            }
            let o = git(dir, &["merge-tree", "--write-tree", &format!("--merge-base={base}"), &head, &merged_head]);
            o.ok && strip_nl(o.stdout) == git_out(dir, &["rev-parse", &format!("{head}^{{tree}}")]).unwrap_or_default()
        };
        if !base_pin.is_empty() && is_ancestor(dir, &base_pin, &head) {
            old_base = base_pin.clone();
            // A rebase or fast-forward outside sheepdog leaves the pin below the
            // default branch's commits HEAD now holds (#120).
            if target == default_branch && is_ancestor(dir, &base_pin, &target_ref) {
                old_base = git_out(dir, &["merge-base", &target_ref, &head]).unwrap_or_default();
            }
        } else if parent_merged == true && !merged_head.is_empty() && is_ancestor(dir, &merged_head, &head) {
            old_base = merged_head.clone();
        } else if let Some(fp) = (!rev(dir, &format!("refs/remotes/{remote}/{parent}")).is_empty())
            .then(|| git_out(dir, &["merge-base", "--fork-point", &format!("refs/remotes/{remote}/{parent}"), &head]))
            .flatten()
        {
            old_base = fp;
        // No rebase --onto without an ancestor, but a tree merge from the merged head.
        } else if !base_pin.is_empty() && parent_merged == true && parent_in_head() {
            old_base = merged_head.clone();
            by_tree = true;
        // Not with the default branch once a stacked parent merged, nor with an
        // open stacked parent (#97).
        } else if target == parent && parent == default_branch {
            old_base = git_out(dir, &["merge-base", &target_ref, &head]).unwrap_or_default();
        }

        if is_ancestor(dir, &target_ref, &head) {
            up_to_date = json!(true);
            conflict = json!(false);
            // A tree base is for a restack, and there's none to do.
            if by_tree {
                old_base.clear();
                by_tree = false;
            }
            // Past the target, the branch's own commits, unless a merge kept the
            // merged parent's.
            if old_base.is_empty() {
                let commits = parent_commits(&parent_pr);
                let holds = !commits.is_empty()
                    && git(dir, &["rev-list", &format!("{target_ref}..{head}")]).stdout.lines().any(|c| commits.contains(c));
                if holds {
                    errors.push(format!("no base for {branch} on {parent}: {branch} is on {remote}/{target} but still has the merged parent's commits"));
                } else {
                    old_base = target_ref.clone();
                }
            }
        } else if old_base.is_empty() {
            up_to_date = json!(false);
            if parent != default_branch && target == parent {
                // A parent closed without merging needs a new target, not a pin.
                if at(&parent_pr, "state").as_str() == Some("CLOSED") {
                    errors.push(format!(
                        "no base for {branch} on {parent}: {parent}'s PR #{} was closed without merging; decide what {branch} targets",
                        text(at(&parent_pr, "number"))
                    ));
                } else if base_pin.is_empty() {
                    errors.push(format!("no base for {branch} on {parent}: no base pin, and no fork-point in {remote}/{parent}'s reflog; if {parent} wasn't rewritten, pin where {branch} left it: git update-ref refs/sheepdog/{branch}/base $(git merge-base {remote}/{parent} {branch})"));
                }
            } else if target == parent {
                errors.push(format!("no merge base with {remote}/{target}"));
            } else if base_pin.is_empty() {
                errors.push(format!("no base for {branch} on {parent}: no base pin, and neither the merged parent's head nor its fork-point is in {branch}'s history"));
            }
        } else {
            up_to_date = json!(false);
            let o = git(dir, &["merge-tree", "--write-tree", &format!("--merge-base={old_base}"), &target_ref, &head]);
            match o.code {
                0 => conflict = json!(false),
                1 => conflict = json!(true),
                rc => errors.push(format!("merge-tree failed ({rc}): {}", o.stderr.split('\n').next().unwrap_or(""))),
            }
        }
    }

    // ── O4 local vs remote ──
    // A followed PR's remote is its head as fetched: the author's, so never
    // judged ours or theirs (#250).
    let remote_ref = if reviewer { crate::pins::name(branch, crate::pins::Pin::PrHead) } else { format!("refs/remotes/{remote}/{branch}") };
    let remote_sha = rev(dir, &remote_ref);
    let (mut remote_ours, mut diverged, mut push_behind, mut theirs) = (Value::Null, false, 0i64, Value::Null);
    let ahead;
    if remote_sha.is_empty() {
        ahead = count(dir, &[&head, "--not", &format!("--remotes={remote}")]);
    } else {
        ahead = count(dir, &[&format!("{remote_sha}..{head}")]);
        push_behind = count(dir, &[&format!("{head}..{remote_sha}")]);
        diverged = ahead > 0 && push_behind > 0;
    }
    if !remote_sha.is_empty() && !reviewer {
        let reflog = git_out(dir, &["reflog", "show", "--format=%H", &format!("refs/heads/{branch}")]).unwrap_or_default();
        if reflog.lines().any(|l| l == remote_sha) {
            remote_ours = json!(true);
        } else {
            remote_ours = json!(false);
            theirs = commits_not_ours(dir, &reflog, &remote_sha).map_or(Value::Null, |n| json!(n));
        }
    }
    // The remote ahead of a branch that once had its commits: made here and
    // reset away, or taken from someone and reset away (#86).
    let mut remote_made = Value::Null;
    if remote_ours == true && ahead == 0 && push_behind > 0 {
        remote_made = json!(made_here(dir, &format!("{head}..{remote_sha}")));
    }
    // dirty: the worktree has uncommitted changes (#143, #178); null when the
    // branch is checked out nowhere or git status fails.
    let mut dirty = Value::Null;
    let wt = worktree_for(dir, branch);
    if !wt.is_empty() {
        let o = git(&wt, &["--no-optional-locks", "status", "--porcelain"]);
        if o.ok {
            dirty = json!(!o.stdout.is_empty());
        }
    }
    // An agent step's commit that changed CI config (sd act's branch.<b>.sd-held:
    // the commit, then the paths): held until sd act push, or until the commit
    // is on the remote some other way (#124).
    let mut held = Value::Null;
    let held_v = cfg("sd-held");
    if !held_v.is_empty() {
        let mut ls = held_v.split('\n');
        let sha = ls.next().unwrap_or("").to_string();
        if remote_sha.is_empty() || !is_ancestor(dir, &sha, &remote_sha) {
            held = json!({"sha": sha, "paths": ls.collect::<Vec<_>>()});
        }
    }

    // ── freshness ──
    // GitHub's head an older value of the remote ref (its ancestor, or in its
    // reflog after a force-push) is GitHub behind a push, not the ref (#246).
    let (mut stale, mut lag) = (Vec::new(), Vec::new());
    let mut author_pushed = if reviewer { json!(false) } else { Value::Null };
    for (p, name) in [(&pr, branch), (&parent_pr, parent.as_str())] {
        // A followed PR's base isn't this branch's to restack onto.
        if reviewer && name != branch {
            continue;
        }
        if at(p, "state").as_str() == Some("OPEN") {
            if let Some(gh_sha) = at(p, "headRefOid").as_str().filter(|s| !s.is_empty()) {
                let short = &gh_sha[..7.min(gh_sha.len())];
                if reviewer {
                    if gh_sha != remote_sha {
                        author_pushed = json!(true);
                        stale.push(format!("PR #{}'s head is not at GitHub's {short}", text(at(p, "number"))));
                    }
                    continue;
                }
                let r = format!("refs/remotes/{remote}/{name}");
                let r_sha = rev(dir, &r);
                if gh_sha != r_sha {
                    let older = !r_sha.is_empty()
                        && (is_ancestor(dir, gh_sha, &r_sha)
                            || git_out(dir, &["reflog", "show", "--format=%H", &r]).unwrap_or_default().lines().any(|l| l == gh_sha));
                    if older {
                        lag.push(format!("GitHub's {name} is at {short}, behind {remote}/{name}"));
                    } else {
                        stale.push(format!("{remote}/{name} is not at GitHub's {short}"));
                    }
                }
            }
        }
    }

    // ── O6 review pins ──
    let stackless = target == parent && parent == default_branch;
    let mut r_findings = match blob_record(dir, &format!("refs/sheepdog/{branch}/findings")) {
        Some(Ok(f))
            if at(&f, "head").is_string()
                && at(&f, "round").is_i64()
                && at(&f, "findings").is_array() =>
        {
            f
        }
        _ => Value::Null,
    };
    if r_findings.is_null() && git_ok(dir, &["rev-parse", "-q", "--verify", &format!("refs/sheepdog/{branch}/findings")]) {
        errors.push(format!("review findings: refs/sheepdog/{branch}/findings isn't a findings record"));
        r_findings = Value::Null;
    }
    // Whether the record is about HEAD: the same commit, or the same change
    // rebased (base..head's patch-id unchanged, as the pins compare it).
    let mut r_findings_at_head = false;
    if !r_findings.is_null() {
        let f_head = str_or(at(&r_findings, "head"), "").to_string();
        let f_base = str_or(at(&r_findings, "base"), "").to_string();
        let exists = |c: &str| git_ok(dir, &["cat-file", "-e", &format!("{c}^{{commit}}")]);
        if f_head == head {
            r_findings_at_head = true;
        } else if !f_base.is_empty() && !old_base.is_empty() && exists(&f_head) && exists(&f_base) {
            let f_patch = patch_id(dir, &f_base, &f_head);
            r_findings_at_head = !f_patch.is_empty() && f_patch == patch_id(dir, &old_base, &head);
        }
    }
    let question = match question_state(dir, branch, &head) {
        Ok(q) => q.unwrap_or(Value::Null),
        Err(()) => {
            errors.push(format!("question: refs/sheepdog/{branch}/question isn't a question record"));
            Value::Null
        }
    };
    let failed = match failed_state(dir, branch) {
        Ok(f) => f.unwrap_or(Value::Null),
        Err(()) => {
            errors.push(format!("failed: refs/sheepdog/{branch}/failed isn't a failure record"));
            Value::Null
        }
    };
    let verify = match verify_state(dir, branch, &head) {
        Ok(v) => v.unwrap_or(Value::Null),
        Err(()) => {
            errors.push(format!("verify: {} isn't a verify record", crate::verify_red::name(branch)));
            Value::Null
        }
    };
    let pin = |k: &str| pin_status(dir, branch, k, &head, &target_ref, &old_base, stackless);
    let (r_local, r_bot, r_accepted) = (pin("local"), pin("bot"), pin("accepted"));

    // ── O5 required checks (#123) ──
    let mut required = Value::Null;
    if at(&pr, "state").as_str() == Some("OPEN") && at(&pr, "isDraft") == &Value::Bool(true) {
        let r_base = text(at(&pr, "baseRefName"));
        match required_checks(dir, &r_base) {
            Ok(r) => required = r,
            Err(e) => errors.push(format!("gh (required checks on {r_base}): {}", e.replace('\n', " "))),
        }
    }

    // ── flags (#125) ──
    // A failed check is flags_error, not an error: errors steer NEXT, and flags
    // are never a gate.
    let (mut flags, mut flags_error) = (Value::Null, String::new());
    if !old_base.is_empty() {
        let flag_text = setting("flags", true)?;
        // A tree base (sync.by_tree) isn't HEAD's ancestor: diffing against it
        // would show the parent's later lines as the branch's.
        if !is_ancestor(dir, &old_base, &head) {
            flags_error = format!("the base {} isn't in {branch}'s history (a tree restack is due); no check until the restack", &old_base[..7]);
        } else {
            match flags_json(dir, &old_base, &head, &raw_json(&flag_text), &flag_text) {
                Ok(f) => flags = f,
                Err(e) => flags_error = if e.is_empty() { "the check failed".into() } else { e },
            }
        }
    }

    let running = running::state(dir, branch).ok_or_else(|| fail("couldn't read the branch's running marker"))?;
    let (handled, unposted) = (config_all(dir, branch, "sd-handled"), config_all(dir, branch, "sd-unposted"));
    let refs: Vec<(String, String)> = git(dir, &["for-each-ref", "--format=%(refname) %(objectname)", &format!("refs/sheepdog/{branch}/")])
        .stdout
        .lines()
        .filter_map(|l| l.split_once(' ').map(|(a, b)| (a.to_string(), b.split(' ').next().unwrap_or("").to_string())))
        .collect();
    let (_, now_ns) = now_utc();
    let facts = Facts {
        branch: branch.into(),
        replays_parent: replays_parent(dir, &head, &old_base, &parent_merged, &up_to_date, &parent_pr),
        head: head.clone(),
        task: cfg("sd-task"),
        reviewer,
        pr,
        parent,
        parent_source,
        parent_pr,
        parent_merged,
        parent_local,
        parent_remote,
        parent_unpushed,
        target,
        default_branch,
        sync_ahead,
        behind,
        up_to_date,
        old_base,
        by_tree,
        conflict,
        undone,
        remote_sha,
        ahead,
        push_behind,
        diverged,
        author_pushed,
        remote_ours,
        theirs,
        remote_made,
        dirty,
        held,
        stale,
        lag,
        r_local,
        r_bot,
        r_accepted,
        r_findings,
        r_findings_at_head,
        review_rounds,
        question,
        failed,
        verify,
        flags,
        flags_error,
        gh_ok,
        parent_ok,
        viewer,
        bots,
        bot_command,
        bot_triggered,
        accepted_after,
        bot_timeout,
        now_ns,
        required,
        ignore,
        ignore_body,
        handled,
        unposted,
        refs,
        enrolled: cfg("sheepdog"),
        harness: cfg("sd-harness"),
        profile,
        session: cfg("sd-session"),
        lock: cfg("sd-lock"),
        running,
        rl_cost: gh.rl_cost,
        rl_remaining: gh.rl_remaining,
        rl_reset: gh.rl_reset,
        rl_limited: gh.rl_limited,
        spend,
        branch_budget,
        errors,
    };
    let mut state = assemble(&facts);
    let mut steps = next::steps(&state, remote).map_err(fail)?;
    // Before NEXT says ready, the PR's base on the remote: GitHub's CLEAN can
    // lag a base that just moved (#169). So too before a first step that spends
    // tokens or triggers the bot, on a head a restack would rewrite (#182); with
    // no PR, the base is the target. And before blocked on GitHub's DIRTY: with no
    // conflict here, the base likelier moved than GitHub is wrong (#246). One
    // ls-remote, only for those first steps; a failure or timeout leaves it
    // unchecked. A moved base is stale, and NEXT is asked again.
    let first = arr(&steps).first().cloned().unwrap_or(json!({"key": "", "who": ""}));
    let fk = str_or(&first["key"], "");
    let alone = arr(&steps).len() == 1;
    let due = first["who"] == "agent"
        || fk == "open-pr"
        || fk == "bot-trigger"
        || (alone && fk == "ready")
        || (alone && fk == "blocked" && at(&state, "pr.merge_state").as_str() == Some("DIRTY"));
    if due {
        let rb = first_of(&state, "pr.base", "sync.target");
        if let Some(rb) = rb.as_str().filter(|r| !r.is_empty()) {
            let want = format!("refs/heads/{rb}");
            let ls = git::git_timed(dir, 10, &["ls-remote", remote, &want]);
            let rb_sha = if ls.timed_out { "" } else { &ls.stdout }
                .lines()
                .find_map(|l| l.split_once('\t').filter(|(_, r)| *r == want).map(|(s, _)| s.to_string()))
                .unwrap_or_default();
            if !rb_sha.is_empty() && rb_sha != rev(dir, &format!("refs/remotes/{remote}/{rb}")) {
                state["fresh"] = json!(false);
                let m = format!("{remote}/{rb} is not at {remote}'s {}", &rb_sha[..7.min(rb_sha.len())]);
                state["stale"].as_array_mut().expect("stale is a list").push(json!(m));
                steps = next::steps(&state, remote).map_err(fail)?;
            }
        }
    }
    state["next"] = steps;
    Ok(state)
}

// ── output ───────────────────────────────────────────────────────────────────

fn summary(s: &Value) -> String {
    let pin = |k: &str, p: &Value| -> Option<String> {
        let st = str_or(at(p, "status"), "");
        if p.is_null() || st == "none" {
            return None;
        }
        let mark = match st {
            "covered" | "rewritten-same" => (if at(p, "replied") == &Value::Bool(false) { "⏳" } else { "✓" }).to_string(),
            "uncovered" => format!("+{}", t(p, "uncovered")),
            "rewritten-changed" => "✗".into(),
            _ => "?".into(),
        };
        Some(format!("{k} {mark}"))
    };
    let pr = at(s, "pr");
    let f = at(s, "review.findings");
    let flags = arr(at(s, "flags"));
    let reviewer = t(s, "mode") == "reviewer";
    let parts: Vec<Option<String>> = vec![
        Some(if pr.is_null() {
            "no PR".into()
        } else {
            format!("{} {}", t(pr, "url"), if on(pr, "draft") { "draft".into() } else { t(pr, "state").to_lowercase() })
        }),
        reviewer.then(|| "reviewing".to_string()),
        match at(s, "session.profile") {
            Value::Null => None,
            p @ Value::Array(_) => Some(format!("profile {}", sj(p, ", "))),
            p => Some(format!("profile {}", text(p))),
        },
        on(s, "parent.merged").then(|| format!("parent merged → {}", t(s, "sync.target"))),
        (at(s, "sync.up_to_date") == &Value::Bool(false)).then(|| {
            format!("↓{} {}{}", t(s, "sync.behind"), t(s, "sync.target"), if on(s, "sync.conflict") { " (conflict)" } else { "" })
        }),
        // merged: a squash merge's deleted remote branch would read as unpushed work (#153)
        if t(pr, "state") == "MERGED" {
            None
        } else if at(s, "push.remote_sha").is_null() {
            Some(if n(s, "push.ahead") > 0.0 { format!("{} unpushed", t(s, "push.ahead")) } else { "not pushed".into() })
        } else if on(s, "push.diverged") && reviewer {
            Some("diverged from the PR's head".into())
        } else if on(s, "push.diverged") {
            Some(format!("diverged from remote{}", if on(s, "push.remote_ours") { "" } else { " (not ours)" }))
        } else if n(s, "push.ahead") > 0.0 {
            Some(format!("{} unpushed", t(s, "push.ahead")))
        } else {
            None
        },
        (!at(s, "push.remote_sha").is_null() && !on(s, "push.diverged") && n(s, "push.behind") > 0.0).then(|| {
            if reviewer {
                format!("author +{}", t(s, "push.behind"))
            } else {
                format!("remote +{}{}", t(s, "push.behind"), if on(s, "push.remote_ours") { " (reset away here)" } else { "" })
            }
        }),
        (!pr.is_null()).then(|| {
            let failing = arr(at(s, "ci.failing"));
            format!(
                "CI {}{}{}",
                t(s, "ci.verdict"),
                if failing.is_empty() { String::new() } else { format!(" ({})", sj(at(s, "ci.failing"), ", ")) },
                if at(s, "ci.for_head") == &Value::Bool(false) { " on old head" } else { "" }
            )
        }),
        (!pr.is_null() && !at(s, "threads.blocking").is_null())
            .then(|| format!("!:{} i:{}", t(s, "threads.blocking"), t(s, "threads.info"))),
        pin("local", at(s, "review.local")),
        pin("bot", at(s, "review.bot")),
        pin("accepted", at(s, "review.accepted")),
        (!f.is_null()).then(|| {
            format!(
                "review: {} finding{} (round {}/{}{}){}",
                t(f, "n"),
                if n(f, "n") == 1.0 { "" } else { "s" },
                t(f, "round"),
                t(f, "cap"),
                if on(f, "declined") { ", declined" } else { "" },
                if on(f, "at_head") { "" } else { " on old head" }
            )
        }),
        on(s, "question.at_head").then(|| format!("? {} {}", t(s, "question.step"), asks_or_gave_up(s))),
        (!at(s, "failed").is_null()).then(|| format!("✗ {} failed", t(s, "failed.count"))),
        on(s, "verify.at_head").then(|| "✗ verify red".to_string()),
        if !flags.is_empty() {
            Some(format!("⚑ {} flagged", flags.len()))
        } else if at(s, "flags_error").is_null() {
            None
        } else {
            Some("⚑ check failed".into())
        },
        on(s, "session.locked").then(|| "locked".to_string()),
        (at(s, "fresh") == &Value::Bool(false)).then(|| "⚠ stale: fetch".to_string()),
        (!arr(at(s, "lag")).is_empty()).then(|| "⏳ GitHub lagging".to_string()),
        (!arr(at(s, "errors")).is_empty()).then(|| format!("⚠ {} error(s)", arr(at(s, "errors")).len())),
    ];
    parts.into_iter().flatten().collect::<Vec<_>>().join(" · ")
}

fn asks_or_gave_up(s: &Value) -> &'static str {
    if t(s, "question.kind") == "gave-up" { "gave up" } else { "asks" }
}

/// --long's lines after NEXT: owed items, review findings, the question, flags.
fn long_lines(s: &Value) -> String {
    let mut out = String::new();
    let items: Vec<[String; 5]> = arr(at(s, "threads.items"))
        .iter()
        .map(|x| {
            let author = if at(x, "author").is_null() { t(x, "last") } else { t(x, "author") };
            let place = if at(x, "path").is_null() { String::new() } else { format!("{}:{}", t(x, "path"), t(x, "line")) };
            let text = [place, t(x, "excerpt")].into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join("  ");
            [t(x, "mark"), t(x, "type"), author, text, t(x, "url")]
        })
        .collect();
    let w = |i: usize| items.iter().map(|x| x[i].chars().count()).max().unwrap_or(0);
    let (tw, aw, xw) = (w(1), w(2), w(3));
    let pad = |s: &str, w: usize| format!("{s}{}", " ".repeat(w.saturating_sub(s.chars().count()).min(200)));
    static TRAIL: OnceLock<Regex> = OnceLock::new();
    let trail = TRAIL.get_or_init(|| re(r"[\t\n\x0C\r ]+$"));
    for [mark, ty, author, text, url] in &items {
        let l = format!("  {mark}  {}  {}  {}  {url}", pad(ty, tw), pad(author, aw), pad(text, xw));
        out.push_str(&trail.replace(&l, ""));
        out.push('\n');
    }
    // The local review's open findings, if any, each with the agent's answer.
    for x in arr(at(s, "review.findings.items")) {
        let or_q = |p: &str| if at(x, p).is_null() { "?".to_string() } else { t(x, p) };
        out.push_str(&format!("  r  {}  {}:{}  {}\n", or_q("severity"), or_q("file"), or_q("line"), t(x, "body")));
        if !at(x, "action").is_null() {
            out.push_str(&format!("       {}: {}\n", t(x, "action"), t(x, "reply")));
        }
    }
    // The agent's standing question, whole.
    if on(s, "question.at_head") {
        out.push_str(&format!("  ?  {} {}:\n", t(s, "question.step"), asks_or_gave_up(s)));
        for l in t(s, "question.text").split('\n') {
            out.push_str(&format!("     {l}\n"));
        }
    }
    if !at(s, "failed").is_null() {
        out.push_str(&format!(
            "  ✗  {} agent step{} failed in a row, the last {} {} at {}: {}\n",
            t(s, "failed.count"),
            if n(s, "failed.count") == 1.0 { "" } else { "s" },
            t(s, "failed.harness"),
            t(s, "failed.step"),
            t(s, "failed.at"),
            t(s, "failed.error")
        ));
    }
    if on(s, "verify.at_head") {
        let sha = t(s, "verify.sha");
        out.push_str(&format!(
            "  ✗  verify red at {} ({}: {})\n",
            &sha[..sha.len().min(7)],
            t(s, "verify.step"),
            t(s, "verify.cmd")
        ));
    }
    out.push_str(&flag_lines(s));
    if !at(s, "flags_error").is_null() {
        out.push_str(&format!("  ⚑  check failed: {}\n", t(s, "flags_error")));
    }
    out
}

/// One line per flag (#125), for --long and sd act review: kind, file:line
/// (and the lines removed, for a test-edit), the line, the commits.
pub fn flag_lines(s: &Value) -> String {
    static LEAD: OnceLock<Regex> = OnceLock::new();
    let lead = LEAD.get_or_init(|| re(r"^[\t\n\x0C\r ]+"));
    let mut out = String::new();
    for f in arr(at(s, "flags")) {
        let kind = t(f, "kind");
        let removed = if kind == "test-edit" { format!(" (-{})", t(f, "n")) } else { String::new() };
        let commits: Vec<String> = arr(at(f, "commits")).iter().map(|c| text(c).chars().take(7).collect()).collect();
        out.push_str(&format!(
            "  ⚑  {kind}  {}:{}{removed}  {}  {}\n",
            t(f, "file"),
            t(f, "line"),
            lead.replace(&t(f, "text"), ""),
            commits.join(",")
        ));
    }
    out
}

pub fn run(argv: &[String]) -> ExitCode {
    let (mut as_json, mut long, mut branch) = (false, false, String::new());
    for a in argv {
        match a.as_str() {
            "--json" => as_json = true,
            "--long" => long = true,
            "-h" | "--help" => {
                print!("{HELP}");
                return ExitCode::SUCCESS;
            }
            o if o.starts_with('-') => die(format!("unknown option: {o}")),
            b => branch = b.to_string(),
        }
    }
    if as_json && long {
        die("--json and --long don't combine");
    }
    if !git::in_repo() {
        die("not a git repository");
    }
    if branch.is_empty() {
        branch = git::current_branch(".").unwrap_or_else(|| die("detached HEAD; name a branch"));
    }
    if rev(".", &format!("refs/heads/{branch}")).is_empty() {
        die(format!("no such branch: {branch}"));
    }
    let remote = config::get_global("settings.remote", false).unwrap_or_else(|l| exit_with(&config_failed(l)));
    if let Some(r) = git::gh_repo(".", &remote) {
        std::env::set_var("GH_REPO", r);
    }
    let state = observe(".", &branch, &remote).unwrap_or_else(|e| exit_with(&e));
    if as_json {
        println!("{}", json::to_string(&state));
        return ExitCode::SUCCESS;
    }
    let mut out = format!("{}\n", summary(&state));
    out.push_str(&next::lines(&state["next"], long));
    if long {
        out.push_str(&long_lines(&state));
    }
    print!("{out}");
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_test_matches_names_paths_and_directories_as_shell_globs() {
        let g = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(is_test("a/b/x_test.go", &g(&["*_test.go"])));
        assert!(!is_test("a/b/x.go", &g(&["*_test.go"])));
        assert!(is_test("a/test/x.rb", &g(&["test/"])));
        assert!(!is_test("a/latest/x.rb", &g(&["test/"])));
        assert!(is_test("lib/a.bats", &g(&["lib/*.bats"])));
        assert!(is_test("lib/x/a.bats", &g(&["lib/*.bats"])));
        assert!(is_test("a/x1.spec", &g(&["x[0-9].spec"])));
        assert!(!is_test("a/xa.spec", &g(&["x[!a-z].spec"])));
    }

    #[test]
    fn shares_a_word_compares_the_names_words_before_the_first_dot() {
        assert!(shares_a_word("t/claude-usage-monitor.bats", "t/ai-usage-monitor.bats"));
        assert!(shares_a_word("a/Parser_test.go", "b/parser.test.js"));
        assert!(!shares_a_word("gone.bats", "new.bats"));
        assert!(!shares_a_word("test/a.test.js", "test/b.test.js"));
        assert!(!shares_a_word("test_foo.py", "test_bar.py"));
        assert!(!shares_a_word("foo_spec.rb", "bar_specs.rb"));
        assert!(!shares_a_word("foo_test.go", "bar_tests.go"));
    }

    #[test]
    fn excerpt_takes_the_first_line_with_text_without_markup() {
        assert_eq!(excerpt(&json!("<!-- x\ny -->\n\n## <b>Title</b>  \nmore")), "Title");
        assert_eq!(excerpt(&json!("\u{a0}keep")), "\u{a0}keep");
        assert_eq!(excerpt(&Value::Null), "");
    }

    #[test]
    fn flag_lines_one_per_flag_the_text_unindented_the_commits_short() {
        let s = json!({"flags": [
            {"kind": "suppression", "file": "a.sh", "line": 2, "n": 1, "text": "  # shellcheck disable=SC2086",
             "commits": ["0123456789abcdef", "fedcba9876543210"]},
            {"kind": "test-edit", "file": "a.bats", "line": 7, "n": 3, "text": "\t[ x ]", "commits": []},
        ]});
        assert_eq!(
            flag_lines(&s),
            "  ⚑  suppression  a.sh:2  # shellcheck disable=SC2086  0123456,fedcba9\n  ⚑  test-edit  a.bats:7 (-3)  [ x ]  \n"
        );
        assert_eq!(flag_lines(&json!({"flags_error": "boom"})), "");
    }

    #[test]
    fn observe_returns_what_sd_state_would_say_instead_of_exiting() {
        let r = crate::test_support::Repo::new();
        assert_eq!(observe(&r.dir, "nope", "origin"), Err("sd state: no such branch: nope\n".into()));
    }
}
