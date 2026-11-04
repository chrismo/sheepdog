// The Map's data (#378): sheepdog's own src/, each file sized by its lines,
// grouped by the stage it serves (STAGES), with the functions
// tests/lint-ratchet.txt holds over clippy's cap and the open issues that
// name it (through view/cache.rs); and the trend, one sample a day of src/
// and tests/ lines and ratchet functions from git log, cached by HEAD. Read
// from the checkout the binary was built in, whatever repo sd view runs from.

use super::cache;
use crate::act::verbs::VERBS;
use crate::gh;
use crate::git::git;
use crate::val::t;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

/// A stalled gh mustn't hold the next fetch up for long.
const GH_SECS: u64 = 20;
/// The most open issues one gh call lists; a list this long may be cut off.
const ISSUE_LIMIT: usize = 500;
/// The trend's first day: the split from agent-rig.
const SPLIT: &str = "2026-10-13";
/// Keyed by HEAD, a trend can't change; the hour only bounds a page left open.
const TREND_FRESH_SECS: u64 = 3600;

/// The stages a file serves, in the order the page draws them. "pages" is
/// sd view (N2); the issue's list has no stage for it.
const STAGES: [&str; 9] = ["watch loop", "observe", "decide", "act", "agent steps", "records", "settings", "pages", "plumbing"];

/// Where a file serves, by its path or its directory (ending in /). A verb's
/// file (src/act/<verb>.rs) is placed by its declaration in VERBS instead:
/// agent steps if it spends tokens, else act. A file neither places lands in plumbing, and
/// every_file_under_src_has_a_stage names it.
const PLACES: &[(&str, &str)] = &[
    ("src/watch.rs", "watch loop"), ("src/watch/", "watch loop"), ("src/watch_help.txt", "watch loop"), ("src/stamp.rs", "watch loop"),
    ("src/state.rs", "observe"), ("src/state_help.txt", "observe"),
    ("src/next.rs", "decide"), ("src/next/", "decide"), ("src/act/gate.rs", "decide"), ("src/act/gates.rs", "decide"),
    ("src/act/mod.rs", "act"), ("src/act_help.txt", "act"), ("src/act/verbs.rs", "act"), ("src/act/bookkeeping.rs", "act"),
    ("src/act/commands.rs", "act"), ("src/act/github.rs", "act"), ("src/act/hold.rs", "act"),
    ("src/act/resolve.rs", "agent steps"), ("src/act/editing.rs", "agent steps"), ("src/act/step.rs", "agent steps"),
    ("src/harness.rs", "agent steps"), ("src/harness/", "agent steps"), ("src/harness_help.txt", "agent steps"),
    ("src/guidance.rs", "agent steps"), ("src/rules.rs", "agent steps"), ("src/stray.rs", "agent steps"),
    ("src/review_local.rs", "agent steps"), ("src/review_local_help.txt", "agent steps"),
    ("src/pins.rs", "records"), ("src/decisions.rs", "records"), ("src/failed.rs", "records"), ("src/verify_red.rs", "records"),
    ("src/usage.rs", "records"), ("src/running.rs", "records"), ("src/lock.rs", "records"), ("src/limit.rs", "records"),
    ("src/config.rs", "settings"),
    ("src/view.rs", "pages"), ("src/view/", "pages"), ("src/view_help.txt", "pages"),
    ("src/main.rs", "plumbing"), ("src/lib.rs", "plumbing"), ("src/sd_help.txt", "plumbing"), ("src/gh.rs", "plumbing"),
    ("src/git.rs", "plumbing"), ("src/proc.rs", "plumbing"), ("src/json.rs", "plumbing"), ("src/val.rs", "plumbing"),
    ("src/time.rs", "plumbing"), ("src/test_support.rs", "plumbing"),
];

/// The checkout the binary was built in: lib/'s parent.
pub(super) fn root() -> PathBuf {
    crate::guidance::lib_dir().parent().map(Path::to_path_buf).unwrap_or_default()
}

/// Every regular file under <root>/src, as (path from <root>, text).
/// Links aren't followed.
fn sources(root: &Path) -> Vec<(String, String)> {
    let (mut out, mut dirs) = (Vec::new(), vec![root.join("src")]);
    while let Some(d) = dirs.pop() {
        for e in fs::read_dir(&d).into_iter().flatten().flatten() {
            let (p, ft) = (e.path(), e.file_type());
            if ft.as_ref().is_ok_and(fs::FileType::is_dir) {
                dirs.push(p);
            } else if let (true, Ok(rel), Ok(b)) = (ft.is_ok_and(|f| f.is_file()), p.strip_prefix(root), fs::read(&p)) {
                out.push((rel.display().to_string(), String::from_utf8_lossy(&b).into_owned()));
            }
        }
    }
    out
}

/// <text>'s "<file> <fn> <lint> <score>" lines, as (file, {fn, lint, score}).
/// # starts a comment; a line that isn't four words is skipped.
fn ratchet(text: &str) -> Vec<(String, Value)> {
    text.lines()
        .filter_map(|l| match l.split('#').next()?.split_whitespace().collect::<Vec<_>>()[..] {
            [file, f, lint, n] => Some((file.to_string(), json!({"fn": f, "lint": lint, "score": n.parse::<u64>().ok()?}))),
            _ => None,
        })
        .collect()
}

/// The stage <path> serves, or None when neither VERBS nor PLACES places it.
/// Only a bare verb names a file: "restack --resolve" is resolve.rs's.
fn stage(path: &str) -> Option<&'static str> {
    let verb = VERBS.iter().filter(|v| !v.name.contains(' ')).find(|v| path == format!("src/act/{}.rs", v.name.replace('-', "_")));
    // spends, not agent (refused while attached): start runs an agent as ship does.
    verb.map(|v| if v.spends { "agent steps" } else { "act" })
        .or_else(|| PLACES.iter().find(|(k, _)| *k == path || (k.ends_with('/') && path.starts_with(k))).map(|(_, s)| *s))
}

/// The files that fall through to plumbing.
#[cfg(test)]
fn unplaced(files: &[(String, String)]) -> Vec<String> {
    files.iter().filter(|(p, _)| stage(p).is_none()).map(|(p, _)| p.clone()).collect()
}

fn header(text: &str) -> String {
    let lines: Vec<&str> = text.lines().map_while(|l| l.strip_prefix("//")).map(|l| l.trim_start_matches('!').trim()).collect();
    lines.join(" ")
}

/// The open issues whose title or body names <path>.
fn naming(issues: &[Value], path: &str) -> Vec<Value> {
    issues
        .iter()
        .filter(|i| t(i, "title").contains(path) || t(i, "body").contains(path))
        .map(|i| json!({"number": i["number"], "title": i["title"], "url": i["url"]}))
        .collect()
}

fn open_issues(root: &Path) -> Result<Vec<Value>, String> {
    let limit = ISSUE_LIMIT.to_string();
    let args = ["issue", "list", "--state", "open", "--limit", &limit, "--json", "number,title,body,url"];
    let out = gh::call_in(&root.to_string_lossy(), None, &args, GH_SECS);
    match serde_json::from_str::<Value>(&out.stdout) {
        Ok(Value::Array(list)) if out.ok() => Ok(list),
        _ => Err(cache::gh_error(&out.stderr, "issue list")),
    }
}

fn file(path: &str, text: &str, over: Vec<Value>, issues: &[Value]) -> Value {
    let over_lines: u64 = over.iter().filter(|o| o["lint"] == "too_many_lines").filter_map(|o| o["score"].as_u64()).sum();
    json!({"path": path, "lines": text.lines().count(), "head": header(text), "over": over, "over_lines": over_lines,
           "issues": naming(issues, path)})
}

/// A group per stage with files, in STAGES' order, each group's files
/// largest first. issues is None until gh has answered once.
fn build(files: &[(String, String)], ratchet_text: &str, issues: Option<&[Value]>) -> Value {
    let over = ratchet(ratchet_text);
    let list = issues.unwrap_or_default();
    let mut groups: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
    for (path, text) in files {
        let mine = over.iter().filter(|(f, _)| f == path).map(|(_, o)| o.clone()).collect();
        groups.entry(stage(path).unwrap_or("plumbing")).or_default().push(file(path, text, mine, list));
    }
    let lines = |f: &Value| f["lines"].as_u64().unwrap_or(0);
    let groups: Vec<Value> = STAGES
        .iter()
        .filter_map(|name| {
            let mut fs = groups.remove(name)?;
            fs.sort_by_key(|f| std::cmp::Reverse(lines(f)));
            Some(json!({"name": name, "lines": fs.iter().map(lines).sum::<u64>(), "files": fs}))
        })
        .collect();
    // Functions, not lines: one over both caps is listed twice.
    let fns: BTreeSet<_> = over.iter().filter(|(f, _)| files.iter().any(|(p, _)| p == f)).map(|(f, o)| (f, t(o, "fn"))).collect();
    let cut = issues.filter(|l| l.len() >= ISSUE_LIMIT).map(|l| format!("gh lists {ISSUE_LIMIT} at most, and gave {}", l.len()));
    json!({"groups": groups, "ratchet": fns.len(), "issues": issues.map(<[Value]>::len), "cut": cut})
}

/// "<sha> <date>" lines, newest first, to the newest commit of each day
/// from SPLIT on, oldest first.
fn samples(log: &str) -> Vec<(&str, &str)> {
    let mut seen = BTreeSet::new();
    let mut out: Vec<(&str, &str)> = log
        .lines()
        .filter_map(|l| l.split_once(' '))
        .filter(|(_, date)| *date >= SPLIT && seen.insert(*date))
        .collect();
    out.sort_by_key(|(_, date)| *date);
    out
}

/// git grep -c's "<sha>:<path>:<count>" lines summed as (src/, tests/) lines.
fn tree_lines(out: &str) -> (u64, u64) {
    out.lines().filter_map(|l| l.rsplit_once(':')).fold((0, 0), |(s, t), (path, n)| {
        let n: u64 = n.parse().unwrap_or(0);
        let path = path.split_once(':').map_or(path, |(_, p)| p);
        (s + if path.starts_with("src/") { n } else { 0 }, t + if path.starts_with("tests/") { n } else { 0 })
    })
}

/// The trend's points, oldest first: {sha, date, src, tests, ratchet} for
/// the newest first-parent commit of each day since SPLIT. Counted in git's
/// objects (git grep -c, git show), nothing checked out.
fn trend(root: &Path) -> Result<Value, String> {
    // Not --since: git reads a bare date at the current time of day.
    let dir = root.to_string_lossy();
    let log = git(&dir, &["log", "--first-parent", "--format=%H %cs", "HEAD"]);
    if !log.ok {
        return Err(format!("git: {}", log.stderr.trim()));
    }
    let points = samples(&log.stdout).into_iter().map(|(sha, date)| sample(&dir, sha, date)).collect::<Result<_, _>>()?;
    Ok(Value::Array(points))
}

/// One trend point. A git failure (an object missing from a partial or
/// shallow clone) is an error, not a day of zeros; no lint-ratchet.txt at
/// the commit is a real 0. GIT_NO_LAZY_FETCH keeps a partial clone from
/// fetching each past day's blobs from its remote instead (git 2.44+).
fn sample(dir: &str, sha: &str, date: &str) -> Result<Value, String> {
    let git = |args: &[&str]| crate::git::Git::new(dir, args).env("GIT_NO_LAZY_FETCH", "1").run();
    let fail = |o: &crate::proc::Out| format!("git: {}", o.stderr.trim());
    // git grep exits 1 for no match: no lines under src/ or tests/.
    let grep = git(&["grep", "-c", "", sha, "--", "src", "tests"]);
    let no_match = grep.code == 1 && grep.stderr.trim().is_empty();
    if !grep.ok && !no_match {
        return Err(fail(&grep));
    }
    let (src, tests) = tree_lines(&grep.stdout);
    let listed = git(&["ls-tree", "--name-only", sha, "--", "tests/lint-ratchet.txt"]);
    if !listed.ok {
        return Err(fail(&listed));
    }
    let text = if listed.stdout.trim().is_empty() {
        String::new()
    } else {
        let show = git(&["show", &format!("{sha}:tests/lint-ratchet.txt")]);
        if !show.ok {
            return Err(fail(&show));
        }
        show.stdout
    };
    // As build counts the chip: functions in src/ files present at the sample.
    let src_files: BTreeSet<&str> = grep.stdout.lines().filter_map(|l| l.split(':').nth(1)).filter(|p| p.starts_with("src/")).collect();
    let list = ratchet(&text);
    let fns: BTreeSet<_> = list.iter().filter(|(f, _)| src_files.contains(f.as_str())).map(|(f, o)| (f, t(o, "fn"))).collect();
    Ok(json!({"sha": sha, "date": date, "src": src, "tests": tests, "ratchet": fns.len()}))
}

/// The trend, cached by the checkout's HEAD: {points, error, fetched, refreshing}.
fn cached_trend(state: &Path, root: &Path) -> Value {
    let head = git(&root.to_string_lossy(), &["rev-parse", "HEAD"]);
    if !head.ok {
        return json!({"points": [], "error": format!("git: {}", head.stderr.trim()), "fetched": null, "refreshing": false});
    }
    let key = format!("trend-{}-{}", crate::watch::repo_key(&root.to_string_lossy()), head.stdout.trim());
    let from = root.to_path_buf();
    let got = cache::get_for(state, &key, TREND_FRESH_SECS, move || trend(&from));
    json!({"points": got["data"].as_array().cloned().unwrap_or_default(), "error": got["error"], "fetched": got["fetched"], "refreshing": got["refreshing"]})
}

pub fn map(state: &Path) -> Value {
    let root = root();
    let key = format!("issues-{}", crate::watch::repo_key(&root.to_string_lossy()));
    let from = root.clone();
    let got = cache::get(state, &key, move || open_issues(&from).map(Value::Array));
    let ratchet_text = fs::read_to_string(root.join("tests/lint-ratchet.txt")).unwrap_or_default();
    let mut m = build(&sources(&root), &ratchet_text, got["data"].as_array().map(Vec::as_slice));
    for k in ["error", "fetched", "refreshing"] {
        m[k] = got[k].clone();
    }
    m["trend"] = cached_trend(state, &root);
    m["root"] = json!(root.display().to_string());
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATCHET: &str = "# file  fn  lint  score\n\
        src/act/push.rs  run  too_many_lines  140\n\
        src/act/push.rs  run  cognitive_complexity  30  # #335 splits it\n\
        src/state.rs  big  too_many_lines  120\n\
        tests/t.rs  long  too_many_lines  110\n\
        src/gone.rs  old  too_many_lines  105\n\
        \n\
        torn line\n\
        src/b.rs  x  too_many_lines  NaN\n";

    fn fixture() -> Vec<(String, String)> {
        let lines = |n: usize| "x\n".repeat(n);
        [("src/act/mod.rs", format!("// The act verbs.\n// All of them.\n{}", lines(8))), ("src/act/push.rs", lines(200)),
         ("src/act_help.txt", lines(3)), ("src/state.rs", lines(150)), ("src/c.rs", lines(1)), ("src/act/implement.rs", lines(9))]
            .into_iter()
            .map(|(p, t)| (p.to_string(), t))
            .collect()
    }

    fn file<'a>(m: &'a Value, path: &str) -> &'a Value {
        m["groups"].as_array().unwrap().iter().flat_map(|g| g["files"].as_array().unwrap()).find(|f| f["path"] == path).unwrap()
    }

    #[test]
    fn ratchet_lines_are_read_and_comments_and_torn_lines_skipped() {
        let r = ratchet(RATCHET);
        assert_eq!(r, [
            ("src/act/push.rs".into(), json!({"fn": "run", "lint": "too_many_lines", "score": 140})),
            ("src/act/push.rs".into(), json!({"fn": "run", "lint": "cognitive_complexity", "score": 30})),
            ("src/state.rs".into(), json!({"fn": "big", "lint": "too_many_lines", "score": 120})),
            ("tests/t.rs".into(), json!({"fn": "long", "lint": "too_many_lines", "score": 110})),
            ("src/gone.rs".into(), json!({"fn": "old", "lint": "too_many_lines", "score": 105})),
        ]);
    }

    #[test]
    fn a_verb_s_file_is_act_or_agent_steps_by_its_declaration() {
        assert_eq!(stage("src/act/push.rs"), Some("act"));
        assert_eq!(stage("src/act/implement.rs"), Some("agent steps"));
        assert_eq!(stage("src/act/start.rs"), stage("src/act/ship.rs"), "both run implement or describe");
        assert_eq!(stage("src/act/start.rs"), Some("agent steps"));
        assert_eq!(stage("src/act/ci_fix.rs"), Some("agent steps"));
        assert_eq!(stage("src/act/restack.rs"), Some("act"), "restack --resolve's agent flag is resolve.rs's, not restack.rs's");
        assert_eq!(stage("src/act/resolve.rs"), Some("agent steps"));
    }

    #[test]
    fn a_file_s_stage_is_its_path_s_or_its_directory_s_table_entry() {
        assert_eq!([stage("src/watch/pass.rs"), stage("src/watch.rs"), stage("src/state.rs"), stage("src/next/steps.rs")],
                   [Some("watch loop"), Some("watch loop"), Some("observe"), Some("decide")]);
        assert_eq!([stage("src/pins.rs"), stage("src/config.rs"), stage("src/view/map.rs"), stage("src/gh.rs")],
                   [Some("records"), Some("settings"), Some("pages"), Some("plumbing")]);
        assert_eq!([stage("src/zzz.rs"), stage("src/act/zzz.rs"), stage("src/nextx.rs")], [None, None, None]);
    }

    #[test]
    fn the_map_groups_files_by_stage_in_the_issue_s_order_largest_file_first() {
        let m = build(&fixture(), "", Some(&[]));
        let names: Vec<_> = m["groups"].as_array().unwrap().iter().map(|g| (g["name"].as_str().unwrap(), g["lines"].as_u64().unwrap())).collect();
        assert_eq!(names, [("observe", 150), ("act", 213), ("agent steps", 9), ("plumbing", 1)]);
        let act: Vec<_> = m["groups"][1]["files"].as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap()).collect();
        assert_eq!(act, ["src/act/push.rs", "src/act/mod.rs", "src/act_help.txt"]);
        assert_eq!(file(&m, "src/act/mod.rs")["lines"], 10);
        assert_eq!(file(&m, "src/act/mod.rs")["head"], "The act verbs. All of them.");
    }

    #[test]
    fn a_file_the_table_doesn_t_place_lands_in_plumbing_and_is_listed() {
        let m = build(&fixture(), "", Some(&[]));
        assert_eq!(m["groups"][3]["files"][0]["path"], "src/c.rs");
        assert_eq!(unplaced(&fixture()), ["src/c.rs"]);
    }

    /// Fails naming the files PLACES and VERBS don't place: add each to PLACES.
    #[test]
    fn every_file_under_src_has_a_stage() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        assert_eq!(unplaced(&sources(root)), Vec::<String>::new());
    }

    #[test]
    fn one_sample_a_day_since_the_split_oldest_first() {
        let log = "c1 2026-10-20\nc2 2026-10-20\nc3 2026-10-19\nc4 2026-10-20\nc5 2026-10-13\nc6 2026-10-13\nc7 2026-10-12\n";
        assert_eq!(samples(log), [("c5", "2026-10-13"), ("c3", "2026-10-19"), ("c1", "2026-10-20")]);
        assert_eq!(samples("torn\n\n"), []);
    }

    #[test]
    fn a_sample_s_lines_are_summed_per_tree() {
        let out = "sha:src/a.rs:100\nsha:src/act/b.rs:20\nsha:tests/t.rs:7\nsha:tests/lint-ratchet.txt:3\nnot a count\n";
        assert_eq!(tree_lines(out), (120, 10));
        assert_eq!(tree_lines(""), (0, 0));
    }

    #[test]
    fn the_trend_is_sampled_from_a_repo_s_first_parent_history() {
        let r = crate::test_support::Repo::new();
        r.git(&["checkout", "-q", "--orphan", "t"]);
        let day = |d: &str, files: &[(&str, &str)]| {
            for (p, text) in files {
                let f = Path::new(&r.dir).join(p);
                fs::create_dir_all(f.parent().unwrap()).unwrap();
                fs::write(f, text).unwrap();
            }
            r.git(&["add", "-A"]);
            let o = crate::git::Git::new(&r.dir, &["commit", "-qm", d, "--allow-empty"]).env("GIT_COMMITTER_DATE", format!("{d}T12:00:00Z")).run();
            assert!(o.ok, "{}", o.stderr);
        };
        day("2026-10-12", &[("src/a.rs", "x\n")]);
        day("2026-10-13", &[("src/a.rs", "x\n".repeat(10).as_str()), ("tests/t.rs", "y\n".repeat(4).as_str())]);
        day("2026-10-15", &[("tests/lint-ratchet.txt", RATCHET), ("src/state.rs", "z\n".repeat(5).as_str()), ("src/act/push.rs", "p\n")]);
        day("2026-10-15", &[("src/state.rs", "z\n".repeat(6).as_str())]);
        let head = r.git(&["rev-parse", "HEAD"]);
        let t = trend(Path::new(&r.dir)).unwrap();
        let point = |i: usize| (t[i]["date"].as_str().unwrap().to_string(), t[i]["src"].as_u64().unwrap(), t[i]["tests"].as_u64().unwrap(), t[i]["ratchet"].as_u64().unwrap());
        assert_eq!(t.as_array().unwrap().len(), 2, "the day before the split isn't sampled: {t}");
        assert_eq!(point(0), ("2026-10-13".into(), 10, 4, 0));
        // As the Map's ratchet chip counts: functions in src/ files in the
        // tree: not tests/t.rs's (not src/), nor src/gone.rs's (not in the tree).
        assert_eq!(point(1), ("2026-10-15".into(), 17, 4 + RATCHET.lines().count() as u64, 2));
        assert_eq!(t[1]["sha"], head);
    }

    #[test]
    fn a_sample_whose_objects_are_missing_fails_the_trend_not_counts_zero() {
        let r = crate::test_support::Repo::new();
        r.git(&["checkout", "-q", "--orphan", "t"]);
        for (d, text) in [("2026-10-13", "only on the thirteenth\n"), ("2026-10-14", "x\n")] {
            fs::create_dir_all(Path::new(&r.dir).join("src")).unwrap();
            fs::write(Path::new(&r.dir).join("src/a.rs"), text).unwrap();
            r.git(&["add", "-A"]);
            let o = crate::git::Git::new(&r.dir, &["commit", "-qm", d]).env("GIT_COMMITTER_DATE", format!("{d}T12:00:00Z")).run();
            assert!(o.ok, "{}", o.stderr);
        }
        let blob = r.git(&["rev-parse", "HEAD~1:src/a.rs"]);
        let git_dir = r.git(&["rev-parse", "--absolute-git-dir"]);
        fs::remove_file(Path::new(&git_dir).join("objects").join(&blob[..2]).join(&blob[2..])).unwrap();
        let e = trend(Path::new(&r.dir)).unwrap_err();
        assert!(e.starts_with("git:"), "{e}");
    }

    #[test]
    fn a_checkout_that_isn_t_a_repo_has_no_trend() {
        assert!(trend(Path::new("/")).is_err());
    }

    #[test]
    fn a_file_s_lines_over_the_cap_are_its_too_many_lines_scores() {
        let m = build(&fixture(), RATCHET, Some(&[]));
        assert_eq!(file(&m, "src/act/push.rs")["over_lines"], 140);
        assert_eq!(file(&m, "src/act/push.rs")["over"].as_array().unwrap().len(), 2);
        assert_eq!(file(&m, "src/state.rs")["over_lines"], 120);
        assert_eq!((&file(&m, "src/c.rs")["over_lines"], &file(&m, "src/c.rs")["over"]), (&json!(0), &json!([])));
    }

    #[test]
    fn the_ratchet_count_is_functions_in_the_mapped_files_not_lines() {
        let m = build(&fixture(), RATCHET, Some(&[]));
        assert_eq!(m["ratchet"], 2);
    }

    #[test]
    fn an_issue_counts_for_each_file_its_title_or_body_names() {
        let issues = vec![
            json!({"number": 1, "title": "split src/state.rs", "url": "u1", "body": "and src/act/push.rs"}),
            json!({"number": 2, "title": "t", "url": "u2", "body": "src/act/push.rs again; mod.rs alone doesn't count"}),
        ];
        let m = build(&fixture(), "", Some(&issues));
        let nums = |p| file(&m, p)["issues"].as_array().unwrap().iter().map(|i| i["number"].as_u64().unwrap()).collect::<Vec<_>>();
        assert_eq!([nums("src/act/push.rs"), nums("src/state.rs"), nums("src/act/mod.rs")], [vec![1, 2], vec![1], vec![]]);
        assert_eq!(file(&m, "src/state.rs")["issues"][0], json!({"number": 1, "title": "split src/state.rs", "url": "u1"}));
        assert_eq!((&m["issues"], &m["cut"]), (&json!(2), &Value::Null));
    }

    #[test]
    fn a_full_issue_list_says_some_may_be_left_out() {
        let list = |n: u64| (0..n).map(|i| json!({"number": i, "title": "t", "url": "u", "body": ""})).collect::<Vec<_>>();
        let m = build(&fixture(), "", Some(&list(ISSUE_LIMIT as u64)));
        assert_eq!(m["issues"], ISSUE_LIMIT);
        assert_eq!(m["cut"], format!("gh lists {ISSUE_LIMIT} at most, and gave {ISSUE_LIMIT}"));
        assert_eq!(build(&fixture(), "", Some(&list(ISSUE_LIMIT as u64 - 1)))["cut"], Value::Null);
    }

    #[test]
    fn without_the_issue_list_files_name_none() {
        let m = build(&fixture(), "", None);
        assert_eq!(file(&m, "src/state.rs")["issues"], json!([]));
        assert_eq!((&m["issues"], &m["cut"]), (&Value::Null, &Value::Null));
    }
}
