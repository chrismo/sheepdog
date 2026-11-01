// The Map's data (#378): sheepdog's own src/, each file sized by its lines,
// grouped by module, with the functions tests/lint-ratchet.txt holds over
// clippy's cap and the open issues that name it. Read from the checkout the
// binary was built in, whatever repo sd view runs from.

use crate::proc;
use crate::val::t;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

/// gh runs inside the request: a stalled one mustn't leave the page waiting.
const GH_SECS: u64 = 20;

/// The checkout the binary was built in: lib/'s parent.
fn root() -> PathBuf {
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

/// <path>'s module: src/<m>/… and src/<m>.rs (or <m>_help.txt) beside a
/// src/<m>/ are <m>; the rest are "src".
fn module(path: &str, dirs: &BTreeSet<&str>) -> String {
    let rest = path.strip_prefix("src/").unwrap_or(path);
    if let Some((m, _)) = rest.split_once('/') {
        return m.to_string();
    }
    let stem = rest.strip_suffix(".rs").or_else(|| rest.strip_suffix("_help.txt")).unwrap_or(rest);
    if dirs.contains(stem) { stem } else { "src" }.to_string()
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
    let args = ["issue", "list", "--state", "open", "--limit", "500", "--json", "number,title,body,url"];
    let out = proc::with_timeout(GH_SECS, "gh", &args, Some(&root.to_string_lossy()));
    match serde_json::from_str::<Value>(&out.stdout) {
        Ok(Value::Array(list)) if out.ok => Ok(list),
        _ if out.stderr.trim().is_empty() => Err("gh: no issue list in its answer".into()),
        _ => Err(format!("gh: {}", out.stderr.trim())),
    }
}

fn file(path: &str, text: &str, over: Vec<Value>, issues: &[Value]) -> Value {
    let over_lines: u64 = over.iter().filter(|o| o["lint"] == "too_many_lines").filter_map(|o| o["score"].as_u64()).sum();
    json!({"path": path, "lines": text.lines().count(), "head": header(text), "over": over, "over_lines": over_lines,
           "issues": naming(issues, path)})
}

/// Groups and files, largest first.
fn build(files: &[(String, String)], ratchet_text: &str, issues: &Result<Vec<Value>, String>) -> Value {
    let over = ratchet(ratchet_text);
    let dirs: BTreeSet<&str> = files.iter().filter_map(|(p, _)| Some(p.strip_prefix("src/")?.split_once('/')?.0)).collect();
    let list = issues.as_deref().unwrap_or_default();
    let mut groups: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for (path, text) in files {
        let mine = over.iter().filter(|(f, _)| f == path).map(|(_, o)| o.clone()).collect();
        groups.entry(module(path, &dirs)).or_default().push(file(path, text, mine, list));
    }
    let lines = |f: &Value| f["lines"].as_u64().unwrap_or(0);
    let mut groups: Vec<Value> = groups
        .into_iter()
        .map(|(name, mut fs)| {
            fs.sort_by_key(|f| std::cmp::Reverse(lines(f)));
            json!({"name": name, "lines": fs.iter().map(lines).sum::<u64>(), "files": fs})
        })
        .collect();
    groups.sort_by_key(|g| std::cmp::Reverse(lines(g)));
    // Functions, not lines: one over both caps is listed twice.
    let fns: BTreeSet<_> = over.iter().filter(|(f, _)| files.iter().any(|(p, _)| p == f)).map(|(f, o)| (f, t(o, "fn"))).collect();
    json!({"groups": groups, "ratchet": fns.len(), "issues": issues.as_ref().ok().map(Vec::len), "error": issues.as_ref().err()})
}

pub fn map() -> Value {
    let root = root();
    let ratchet_text = fs::read_to_string(root.join("tests/lint-ratchet.txt")).unwrap_or_default();
    let mut m = build(&sources(&root), &ratchet_text, &open_issues(&root));
    m["root"] = json!(root.display().to_string());
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATCHET: &str = "# file  fn  lint  score\n\
        src/act/a.rs  run  too_many_lines  140\n\
        src/act/a.rs  run  cognitive_complexity  30  # #335 splits it\n\
        src/b.rs  big  too_many_lines  120\n\
        tests/t.rs  long  too_many_lines  110\n\
        \n\
        torn line\n\
        src/b.rs  x  too_many_lines  NaN\n";

    fn fixture() -> Vec<(String, String)> {
        let lines = |n: usize| "x\n".repeat(n);
        [("src/act.rs", format!("// The act verbs.\n// All of them.\n{}", lines(8))), ("src/act/a.rs", lines(200)),
         ("src/act_help.txt", lines(3)), ("src/b.rs", lines(150)), ("src/c.rs", lines(1))]
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
            ("src/act/a.rs".into(), json!({"fn": "run", "lint": "too_many_lines", "score": 140})),
            ("src/act/a.rs".into(), json!({"fn": "run", "lint": "cognitive_complexity", "score": 30})),
            ("src/b.rs".into(), json!({"fn": "big", "lint": "too_many_lines", "score": 120})),
            ("tests/t.rs".into(), json!({"fn": "long", "lint": "too_many_lines", "score": 110})),
        ]);
    }

    #[test]
    fn a_file_s_module_is_its_directory_or_the_directory_beside_it() {
        let dirs = BTreeSet::from(["act", "view"]);
        let m = |p| module(p, &dirs);
        assert_eq!([m("src/act/a.rs"), m("src/act.rs"), m("src/act_help.txt"), m("src/view/x.html")], ["act", "act", "act", "view"]);
        assert_eq!([m("src/b.rs"), m("src/state_help.txt"), m("src/actx.rs")], ["src", "src", "src"]);
    }

    #[test]
    fn the_map_groups_files_by_module_largest_first() {
        let m = build(&fixture(), "", &Ok(vec![]));
        let names: Vec<_> = m["groups"].as_array().unwrap().iter().map(|g| (g["name"].as_str().unwrap(), g["lines"].as_u64().unwrap())).collect();
        assert_eq!(names, [("act", 213), ("src", 151)]);
        let act: Vec<_> = m["groups"][0]["files"].as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap()).collect();
        assert_eq!(act, ["src/act/a.rs", "src/act.rs", "src/act_help.txt"]);
        assert_eq!(file(&m, "src/act.rs")["lines"], 10);
        assert_eq!(file(&m, "src/act.rs")["head"], "The act verbs. All of them.");
    }

    #[test]
    fn a_file_s_lines_over_the_cap_are_its_too_many_lines_scores() {
        let m = build(&fixture(), RATCHET, &Ok(vec![]));
        assert_eq!(file(&m, "src/act/a.rs")["over_lines"], 140);
        assert_eq!(file(&m, "src/act/a.rs")["over"].as_array().unwrap().len(), 2);
        assert_eq!(file(&m, "src/b.rs")["over_lines"], 120);
        assert_eq!((&file(&m, "src/c.rs")["over_lines"], &file(&m, "src/c.rs")["over"]), (&json!(0), &json!([])));
    }

    #[test]
    fn the_ratchet_count_is_functions_in_the_mapped_files_not_lines() {
        let m = build(&fixture(), RATCHET, &Ok(vec![]));
        assert_eq!(m["ratchet"], 2);
    }

    #[test]
    fn an_issue_counts_for_each_file_its_title_or_body_names() {
        let issues = vec![
            json!({"number": 1, "title": "split src/b.rs", "url": "u1", "body": "and src/act/a.rs"}),
            json!({"number": 2, "title": "t", "url": "u2", "body": "src/act/a.rs again; act.rs alone doesn't count"}),
        ];
        let m = build(&fixture(), "", &Ok(issues));
        let nums = |p| file(&m, p)["issues"].as_array().unwrap().iter().map(|i| i["number"].as_u64().unwrap()).collect::<Vec<_>>();
        assert_eq!([nums("src/act/a.rs"), nums("src/b.rs"), nums("src/act.rs")], [vec![1, 2], vec![1], vec![]]);
        assert_eq!(file(&m, "src/b.rs")["issues"][0], json!({"number": 1, "title": "split src/b.rs", "url": "u1"}));
        assert_eq!((&m["issues"], &m["error"]), (&json!(2), &Value::Null));
    }

    #[test]
    fn without_the_issue_list_files_name_none_and_the_error_says_why() {
        let m = build(&fixture(), "", &Err("gh: down".into()));
        assert_eq!(file(&m, "src/b.rs")["issues"], json!([]));
        assert_eq!((&m["issues"], &m["error"]), (&Value::Null, &json!("gh: down")));
    }
}
