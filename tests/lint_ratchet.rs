// clippy's too_many_lines and cognitive_complexity, held per function. Every
// function over clippy's cap (100 lines, complexity 25) is listed in
// tests/lint-ratchet.txt at its score and may not grow past it. Shrinking, or
// dropping under the cap, passes with the line left as is; #354 recalibrates
// the list. The clippy task runs the ignored test; the others check the
// comparison.

use regex::Regex;
use serde_json::Value;
use std::collections::BTreeMap;
use std::process::Command;

/// "<file> <fn> <lint>" → score.
type Scores = BTreeMap<String, u64>;

const BASELINE: &str = "tests/lint-ratchet.txt";

/// The two lints' scores in clippy's --message-format=json output, <json>.
fn scores(json: &str) -> Scores {
    let name = Regex::new(r"\bfn\s+(\w+)").unwrap();
    let score = Regex::new(r"\((\d+)/\d+\)").unwrap();
    let mut out = Scores::new();
    for m in json.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()) {
        let m = &m["message"];
        let Some(lint) = m["code"]["code"].as_str().and_then(|c| c.strip_prefix("clippy::")) else { continue };
        if lint != "too_many_lines" && lint != "cognitive_complexity" {
            continue;
        }
        let span = &m["spans"][0];
        let read = (span["file_name"].as_str(), span["text"][0]["text"].as_str().and_then(|l| name.captures(l)), m["message"].as_str().and_then(|s| score.captures(s)));
        let (Some(file), Some(f), Some(n)) = read else { panic!("can't read clippy::{lint} in {m}") };
        let (k, n) = (format!("{file} {} {lint}", &f[1]), n[1].parse().unwrap());
        // Each target reports a function again at the same score; another
        // score is a second function with the name, which the key can't hold.
        if let Some(had) = out.insert(k.clone(), n) {
            assert_eq!(had, n, "{k} scores both {had} and {n}: rename one");
        }
    }
    out
}

/// <text>'s "<file> <fn> <lint> <score>" lines; # starts a comment.
fn baseline(text: &str) -> Scores {
    let mut out = Scores::new();
    for (i, l) in text.lines().enumerate() {
        match l.split('#').next().unwrap().split_whitespace().collect::<Vec<_>>()[..] {
            [] => {}
            [file, f, lint, n] => {
                let k = format!("{file} {f} {lint}");
                let n = n.parse().unwrap_or_else(|_| panic!("{BASELINE}:{}: score {n:?}", i + 1));
                assert!(out.insert(k.clone(), n).is_none(), "{BASELINE}:{}: {k} is listed twice", i + 1);
            }
            _ => panic!("{BASELINE}:{}: want <file> <fn> <lint> <score>, got {l:?}", i + 1),
        }
    }
    out
}

/// What breaks the ratchet: <now> against <base>.
fn ratchet(now: &Scores, base: &Scores) -> Vec<String> {
    let mut problems = vec![];
    for (k, &n) in now {
        match base.get(k) {
            None => problems.push(format!("{k} is {n}, over clippy's cap: split it")),
            Some(&b) if n > b => problems.push(format!("{k} grew from {b} to {n}: split it, don't grow it")),
            Some(_) => {}
        }
    }
    problems
}

fn s(pairs: &[(&str, u64)]) -> Scores {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

fn diag(code: &str, message: &str, file: &str, line: &str) -> String {
    let m = serde_json::json!({"reason": "compiler-message", "message": {
        "code": {"code": code}, "message": message,
        "spans": [{"file_name": file, "text": [{"text": line}]}]}});
    m.to_string()
}

#[test]
fn a_listed_function_at_its_score_passes() {
    let now = s(&[("src/next.rs steps too_many_lines", 600)]);
    assert_eq!(ratchet(&now, &now.clone()), Vec::<String>::new());
}

#[test]
fn a_listed_function_that_grew_fails() {
    let p = ratchet(&s(&[("src/next.rs steps too_many_lines", 601)]), &s(&[("src/next.rs steps too_many_lines", 600)]));
    assert_eq!(p, ["src/next.rs steps too_many_lines grew from 600 to 601: split it, don't grow it"]);
}

#[test]
fn a_listed_function_that_shrank_passes_without_lowering_its_line() {
    let p = ratchet(&s(&[("src/next.rs steps too_many_lines", 590)]), &s(&[("src/next.rs steps too_many_lines", 600)]));
    assert_eq!(p, Vec::<String>::new());
}

#[test]
fn a_new_function_over_the_cap_fails() {
    let p = ratchet(&s(&[("src/git.rs fetch too_many_lines", 120)]), &Scores::new());
    assert_eq!(p, ["src/git.rs fetch too_many_lines is 120, over clippy's cap: split it"]);
}

#[test]
fn a_listed_function_now_under_the_cap_passes_with_its_line_left() {
    let p = ratchet(&Scores::new(), &s(&[("src/next.rs steps cognitive_complexity", 90)]));
    assert_eq!(p, Vec::<String>::new());
}

#[test]
fn scores_reads_the_two_lints_once_per_function_and_nothing_else() {
    let json = [
        diag("clippy::too_many_lines", "this function has too many lines (600/100)", "src/next.rs", "pub fn steps(s: &Value) -> Value {"),
        // The same function, reported again for another target.
        diag("clippy::too_many_lines", "this function has too many lines (600/100)", "src/next.rs", "pub fn steps(s: &Value) -> Value {"),
        diag("clippy::cognitive_complexity", "the function has a cognitive complexity of (90/25)", "src/next.rs", "pub fn steps(s: &Value) -> Value {"),
        diag("clippy::too_many_lines", "this function has too many lines (108/100)", "src/watch/pass.rs", "    fn check(&mut self) -> bool {"),
        diag("clippy::needless_borrow", "this expression creates a reference", "src/git.rs", "    run(&x);"),
        r#"{"reason":"compiler-artifact"}"#.to_string(),
    ]
    .join("\n");
    let want = s(&[
        ("src/next.rs steps cognitive_complexity", 90),
        ("src/next.rs steps too_many_lines", 600),
        ("src/watch/pass.rs check too_many_lines", 108),
    ]);
    assert_eq!(scores(&json), want);
}

#[test]
fn baseline_skips_comments_and_blank_lines() {
    let text = "# file fn lint score\n\nsrc/next.rs  steps  too_many_lines  600\n";
    assert_eq!(baseline(text), s(&[("src/next.rs steps too_many_lines", 600)]));
}

#[test]
#[should_panic(expected = "src/state.rs run too_many_lines scores both 120 and 140")]
fn scores_refuses_two_functions_with_one_name_in_one_file() {
    let json = [
        diag("clippy::too_many_lines", "this function has too many lines (120/100)", "src/state.rs", "fn run() {"),
        diag("clippy::too_many_lines", "this function has too many lines (140/100)", "src/state.rs", "    fn run(&self) {"),
    ]
    .join("\n");
    scores(&json);
}

#[test]
#[should_panic(expected = "can't read clippy::cognitive_complexity")]
fn scores_refuses_one_of_the_lints_on_a_line_with_no_fn() {
    let json = diag("clippy::cognitive_complexity", "the function has a cognitive complexity of (30/25)", "src/next.rs", "    let f = |x| {");
    scores(&json);
}

#[test]
#[should_panic(expected = "tests/lint-ratchet.txt:3: src/next.rs steps too_many_lines is listed twice")]
fn baseline_refuses_a_function_listed_twice() {
    baseline("# file fn lint score\nsrc/next.rs  steps  too_many_lines  600\nsrc/next.rs  steps  too_many_lines  590\n");
}

#[test]
#[should_panic(expected = "tests/lint-ratchet.txt:2: want <file> <fn> <lint> <score>")]
fn baseline_refuses_a_line_without_four_fields() {
    baseline("# file fn lint score\nsrc/next.rs  steps  too_many_lines\n");
}

#[test]
#[ignore = "runs clippy; the clippy task runs it"]
fn no_function_is_over_clippys_cap_beyond_its_line_in_the_baseline() {
    let root = env!("CARGO_MANIFEST_DIR");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = Command::new(cargo)
        .current_dir(root)
        .args(["clippy", "--all-targets", "--locked", "--quiet", "--message-format=json", "--"])
        .args(["-W", "clippy::too_many_lines", "-W", "clippy::cognitive_complexity"])
        .output()
        .unwrap();
    assert!(out.status.success(), "clippy failed: {}", String::from_utf8_lossy(&out.stderr));
    let base = baseline(&std::fs::read_to_string(format!("{root}/{BASELINE}")).unwrap());
    let problems = ratchet(&scores(&String::from_utf8_lossy(&out.stdout)), &base);
    assert!(problems.is_empty(), "\n{}\n", problems.join("\n"));
}
