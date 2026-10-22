// Black-box tests of sd act's dispatcher (src/act/mod.rs): --help, no verb,
// an unknown verb, and a flag on a verb it isn't for (#204). The built
// binary, run as sd-act through a link (tests/common). Moved from
// libexec/sd-act.bats; each test keeps the bats test's title above it. The
// verbs are tested in tests/act_*.rs.

mod common;

use common::{assert_has, assert_match, Sd};
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

/// The verbs src/act/mod.rs's `match verb.as_str()` dispatches, as the bats
/// test's sed read them.
fn dispatched_verbs() -> Vec<String> {
    let src = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/act/mod.rs")).unwrap();
    let arm = regex::Regex::new(r#"^        "([a-z-]*)""#).unwrap();
    src.lines()
        .skip_while(|l| *l != "    match verb.as_str() {")
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
