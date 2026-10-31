// Black-box tests of sd's own dispatch (src/main.rs, #204): `sd <verb>` runs
// the verb, the way git runs git-<verb>. The built binary, run as sd (its
// own name) from tests/common's temp dir. Moved from bin/sd.bats, the bash
// dispatcher's suite; each test keeps the bats test's title above it. The
// bash dispatcher's rebuild-on-stale is gone with it: a git hook that
// install.sh writes rebuilds sd (tests/install.rs).

mod common;

use common::{assert_has, Sd};
use std::fs;
use std::os::unix::fs::symlink;

/// tests/common's temp dir, running the binary itself as sd.
fn sd() -> Sd {
    let mut sd = Sd::for_verb("cli");
    sd.bin = env!("CARGO_BIN_EXE_sd").into();
    sd
}

/// The verbs src/main.rs's `match verb.as_str()` dispatches.
fn dispatched_verbs() -> Vec<String> {
    let src = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs")).unwrap();
    let arm = regex::Regex::new(r#"^        "([a-z-]*)" =>"#).unwrap();
    src.lines()
        .skip_while(|l| *l != "    match verb.as_str() {")
        .take_while(|l| *l != "    }")
        .filter_map(|l| arm.captures(l).map(|c| c[1].to_string()))
        .collect()
}

// @test "no verb: usage on stderr, nonzero"
#[test]
fn no_verb_usage_on_stderr_nonzero() {
    let sd = sd();
    let r = sd.run_sep(&[]);
    assert_ne!(r.status, 0);
    assert_eq!(r.output, "");
    assert_has(&r.stderr, &["Usage: sd <verb>"]);
}

#[test]
fn an_empty_verb_is_no_verb_usage_on_stderr_nonzero() {
    let sd = sd();
    let r = sd.run_sep(&[""]);
    assert_ne!(r.status, 0);
    assert_eq!(r.output, "");
    assert_has(&r.stderr, &["Usage: sd <verb>"]);
}

// @test "help, -h and --help: usage on stdout, zero"
#[test]
fn help_h_and_help_usage_on_stdout_zero() {
    let sd = sd();
    for h in ["help", "-h", "--help"] {
        let r = sd.run_sep(&[h]);
        assert_eq!(r.status, 0, "{h}");
        assert_has(&r.output, &["Usage: sd <verb>"]);
    }
}

// @test "usage names every verb in libexec"
#[test]
fn usage_names_every_verb() {
    let sd = sd();
    let r = sd.run(&["--help"]);
    let verbs = dispatched_verbs();
    assert!(!verbs.is_empty());
    for v in verbs {
        assert_has(&r.output, &[&format!("  {v} ")]);
    }
}

// @test "unknown verb fails"
#[test]
fn unknown_verb_fails() {
    let sd = sd();
    let r = sd.run(&["frobnicate"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["sd: unknown verb: frobnicate"]);
}

// @test "a verb that is a path is refused"
#[test]
fn a_verb_that_is_a_path_is_refused() {
    let sd = sd();
    let r = sd.run(&["../libexec/sd-config"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["unknown verb"]);
}

// @test "a test file is not a verb"
#[test]
fn a_test_file_is_not_a_verb() {
    let sd = sd();
    let r = sd.run(&["config.bats"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["unknown verb"]);
}

// @test "dispatches to libexec/sd-<verb> with the remaining args"
#[test]
fn dispatches_to_the_verb_with_the_remaining_args() {
    let sd = sd();
    let r = sd.run(&["config", "set", "--global", "settings.replyPrefix", "two words"]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_eq!(sd.out(&["config", "get", "--no-branch", "settings.replyPrefix"]), "two words");
}

// @test "passes the verb's exit status through"
#[test]
fn passes_the_verbs_exit_status_through() {
    let sd = sd();
    // sd config get exits 3 for a path that isn't set.
    let r = sd.run(&["config", "get", "--no-branch", "harnesses.claude.nope"]);
    assert_eq!(r.status, 3, "{}", r.output);
}

// @test "run through a symlink, it still finds its libexec"
#[test]
fn run_through_a_symlink_it_still_finds_its_verbs() {
    let sd = sd();
    fs::create_dir(sd.tmp.join("local-bin")).unwrap();
    let link = sd.tmp.join("local-bin/sd");
    symlink(&sd.bin, &link).unwrap();
    let r = sd.exec(&link, &[], &["config", "get", "--no-branch", "settings.remote"]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_eq!(r.output, "origin");
}

// @test "the real verbs are reachable"
#[test]
fn the_real_verbs_are_reachable() {
    let sd = sd();
    for v in dispatched_verbs() {
        let r = sd.run(&[&v, "--help"]);
        assert_eq!(r.status, 0, "{v}: {}", r.output);
        assert_has(&r.output, &["Usage"]);
    }
}

#[test]
fn run_as_sd_verb_it_is_that_verb() {
    let sd = sd();
    let r = sd.exec(&sd.link("config"), &[], &["get", "--no-branch", "settings.remote"]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_eq!(r.output, "origin");
}
