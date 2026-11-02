// Black-box tests of sd act's dispatcher (src/act/mod.rs): --help, no verb,
// an unknown verb, and a flag on a verb it isn't for (#204). The built
// binary, run as sd-act through a link (tests/common). Moved from
// libexec/sd-act.bats; each test keeps the bats test's title above it. The
// verbs are tested in tests/act_*.rs.

mod common;

use common::{assert_has, assert_match, Sd};
use sheepdog::act::verbs::VERBS;
use std::fs;

/// The bats setup: repo (cwd) with an empty commit on main, feat checked out.
fn sd() -> Sd {
    let mut sd = Sd::for_verb("act");
    sd.git(&["init", "-q", &sd.path("repo")]);
    sd.cd("repo");
    sd.git(&["commit", "-q", "--allow-empty", "-m", "init"]);
    sd.git(&["switch", "-qc", "feat"]);
    sd
}

// @test "no verb: usage on stderr, nonzero"
#[test]
fn no_verb_usage_on_stderr_nonzero() {
    let sd = sd();
    let r = sd.run(&[]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["Usage"]);
}

// @test "unknown verb fails"
#[test]
fn unknown_verb_fails() {
    let sd = sd();
    let r = sd.run(&["frobnicate"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["unknown verb"]);
}

// @test "--remove is for sd act forget only"
#[test]
fn remove_is_for_sd_act_forget_only() {
    let sd = sd();
    let r = sd.run(&["unenroll", "--remove"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["--remove is for sd act forget only"]);
}

/// The verbs src/act/mod.rs's `match verb` dispatches, as the bats
/// test's sed read them.
fn dispatched_verbs() -> Vec<String> {
    let src = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/act/mod.rs")).unwrap();
    let arm = regex::Regex::new(r#"^        "([a-z-]*)""#).unwrap();
    src.lines()
        .skip_while(|l| *l != "    match verb {")
        .take_while(|l| *l != "    }")
        .filter_map(|l| arm.captures(l).map(|c| c[1].to_string()))
        .collect()
}

// @test "--help: every verb has a usage line"
#[test]
fn help_every_verb_has_a_usage_line() {
    let sd = sd();
    let r = sd.run(&["--help"]);
    assert_eq!(r.status, 0);
    let verbs = dispatched_verbs();
    assert!(!verbs.is_empty());
    for v in verbs {
        assert_match(&r.output, &format!("sd act {v} "));
    }
}

// @test "--help works outside a repo"
#[test]
fn help_works_outside_a_repo() {
    let mut sd = sd();
    sd.cd("");
    let r = sd.run(&["--help"]);
    assert_has(&r.output, &["Usage"]);
}

// @test "--help lists attach"
#[test]
fn help_lists_attach() {
    let sd = sd();
    let r = sd.run(&["--help"]);
    assert_match(&r.output, "sd act attach ");
}

// ── each verb's declaration (#341) ───────────────────────────────────────────

// act::run takes a verb's interlocks from its declaration alone, so a verb
// dispatched without one would run with none.
#[test]
fn every_dispatched_verb_has_a_declaration_341() {
    let declared: Vec<&str> = VERBS.iter().map(|v| v.name).collect();
    for v in dispatched_verbs() {
        assert!(declared.contains(&v.as_str()), "sd act {v} has no declaration in src/act/verbs.rs");
    }
    for v in &declared {
        let verb = v.split(' ').next().unwrap();
        assert!(dispatched_verbs().iter().any(|d| d == verb), "{v} is declared but not dispatched");
    }
}

/// Whether src/act_help.txt marks <name> "Spends tokens": a verb's entry
/// under Verbs:, or for "<verb> --<flag>", the flag's line under "<verb>
/// options:" (none there: unmarked).
fn help_marks_spending(help: &str, name: &str) -> bool {
    let words = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    if let Some((verb, flag)) = name.split_once(' ') {
        let mut lines = help.lines().skip_while(|l| *l != format!("{verb} options:")).skip(1);
        return lines.find(|l| l.trim_start().starts_with(flag)).is_some_and(|l| words(l).contains("Spends tokens"));
    }
    let section = help.lines().skip_while(|l| *l != "Verbs:").skip(1).take_while(|l| !l.is_empty());
    let mut entry = String::new();
    let mut inside = false;
    for l in section {
        if l.starts_with("  ") && !l.starts_with("   ") {
            inside = l.split_whitespace().next() == Some(name);
        }
        if inside {
            entry.push(' ');
            entry.push_str(l);
        }
    }
    assert!(!entry.is_empty(), "no entry for {name} under Verbs: in src/act_help.txt");
    words(&entry).contains("Spends tokens")
}

// AGENTS.md rule 1: a verb that runs an agent says so in its help, and its
// declaration's spends is what puts the budget rule in its gate (#349).
#[test]
fn spends_matches_the_helps_spends_tokens_marks_341() {
    let help = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/act_help.txt")).unwrap();
    for v in VERBS {
        assert_eq!(v.spends, help_marks_spending(&help, v.name), "{}: spends is {}, but the help says otherwise", v.name, v.spends);
    }
}
