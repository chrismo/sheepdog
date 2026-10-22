// Black-box tests of sd act verify (A10) and autofix (A11) (#202): the built
// binary, run as sd-act through a link (tests/common). Moved from
// libexec/sd-act-verify.bats and the verb's tests in sd-act-autofix.bats;
// each test keeps the bats test's title above it. The autofix suite's
// implement tests and its --help test stay in bats while implement and
// --help are bash.
//
// Real git; the commands are shell one-liners. gh is a stub that fails.

mod common;

use common::{assert_has, assert_lacks, spawn, Sd, GH_FAILS};
use std::fs;
use std::process::Stdio;
use std::time::Duration;

/// The bats setup: repo (cwd) with file.txt committed on main, feat checked out.
fn sd() -> Sd {
    let mut sd = Sd::for_verb("act");
    sd.fake("gh", GH_FAILS);
    sd.git(&["init", "-q", &sd.path("repo")]);
    sd.cd("repo");
    sd.write("file.txt", "a\nb\nc\n");
    sd.git(&["add", "file.txt"]);
    sd.git(&["commit", "-qm", "init"]);
    sd.git(&["switch", "-qc", "feat"]);
    sd
}

/// grep -qx <line> <<<"$output"
fn has_line(s: &str, line: &str) -> bool {
    s.lines().any(|l| l == line)
}

impl Sd {
    fn set_global(&self, key: &str, value: &str) {
        self.verb_ok("config", &["set", "--global", key, value]);
    }

    fn head_sha(&self) -> String {
        self.git(&["rev-parse", "HEAD"])
    }

    fn exists(&self, name: &str) -> bool {
        self.cwd.join(name).exists()
    }
}

mod verify {
    use super::*;

    fn verify_cmd(sd: &Sd, c: &str) {
        sd.set_global("settings.verify", c);
    }

    // @test "no settings.verify: says so and passes"
    #[test]
    fn no_settings_verify_says_so_and_passes() {
        let sd = sd();
        let r = sd.run(&["verify"]);
        assert_eq!(r.status, 0);
        assert_has(&r.output, &["no settings.verify"]);
    }

    // @test "passing: one line, the command's output is not shown"
    #[test]
    fn passing_one_line_the_commands_output_is_not_shown() {
        let sd = sd();
        verify_cmd(&sd, "seq 1 3");
        let r = sd.run(&["verify"]);
        assert_eq!(r.status, 0);
        assert_has(&r.output, &["verify passed"]);
        assert!(!has_line(&r.output, "2"));
    }

    // @test "failing: nonzero, the output trimmed to its last 40 lines"
    #[test]
    fn failing_nonzero_the_output_trimmed_to_its_last_40_lines() {
        let sd = sd();
        verify_cmd(&sd, "seq 1 60; echo 'FAIL: the end' >&2; exit 2");
        let r = sd.run(&["verify"]);
        assert_ne!(r.status, 0);
        assert_has(&r.output, &["verify failed"]);
        assert_has(&r.output, &["FAIL: the end"]);
        assert_has(&r.output, &["last 40 of 61 lines"]);
        assert!(has_line(&r.output, "60"));
        assert!(!has_line(&r.output, "1"));
    }

    // @test "failing: short output is shown whole, untrimmed"
    #[test]
    fn failing_short_output_is_shown_whole_untrimmed() {
        let sd = sd();
        verify_cmd(&sd, "echo only line; false");
        let r = sd.run(&["verify"]);
        assert_ne!(r.status, 0);
        assert_has(&r.output, &["only line"]);
        assert_lacks(&r.output, "last 40");
    }

    // @test "runs in the branch's worktree, not the current one"
    #[test]
    fn runs_in_the_branchs_worktree_not_the_current_one() {
        let sd = sd();
        sd.git(&["worktree", "add", "-q", &sd.path("wt"), "-b", "other"]);
        fs::write(sd.tmp.join("wt/marker"), "").unwrap();
        verify_cmd(&sd, "test -f marker");
        assert_eq!(sd.run(&["verify", "other"]).status, 0);
        assert_ne!(sd.run(&["verify", "feat"]).status, 0);
    }

    // @test "a branch checked out nowhere fails"
    #[test]
    fn a_branch_checked_out_nowhere_fails() {
        let sd = sd();
        sd.git(&["branch", "parked"]);
        verify_cmd(&sd, "true");
        let r = sd.run(&["verify", "parked"]);
        assert_ne!(r.status, 0);
        assert_has(&r.output, &["not checked out"]);
    }

    // @test "a command that changes the tree fails, even when it exits 0"
    #[test]
    fn a_command_that_changes_the_tree_fails_even_when_it_exits_0() {
        let sd = sd();
        verify_cmd(&sd, "echo formatted >> file.txt");
        let r = sd.run(&["verify"]);
        assert_ne!(r.status, 0);
        assert_has(&r.output, &["changed files"]);
    }

    // @test "a git status that fails fails it, not a pass"
    #[test]
    fn a_git_status_that_fails_fails_it_not_a_pass() {
        let sd = sd();
        verify_cmd(&sd, "true");
        sd.write(".git/index", "not an index");
        let r = sd.run(&["verify"]);
        assert_ne!(r.status, 0);
        assert_has(&r.output, &["git status failed"]);
    }

    // @test "a list-valued verify is refused before anything runs"
    #[test]
    fn a_list_valued_verify_is_refused_before_anything_runs() {
        let sd = sd();
        let r = sd.exec(&sd.link("config"), &[], &["set", "--global", "--json", "settings.verify", r#"["true","false"]"#]);
        assert_eq!(r.status, 1);
        sd.write_global(r#"{"settings":{"verify":["true","false"]}}"#);
        let r = sd.run(&["verify"]);
        assert_ne!(r.status, 0);
        assert_has(&r.output, &[r#"settings.verify: expected a string, got ["true","false"]"#]);
    }

    // @test "the branch file's verify beats the global file's"
    #[test]
    fn the_branch_files_verify_beats_the_global_files() {
        let sd = sd();
        verify_cmd(&sd, "true");
        sd.verb_ok("config", &["set", "settings.verify", "false"]);
        assert_ne!(sd.run(&["verify"]).status, 0);
    }

    // @test "runs under settings.stepTimeout"
    #[test]
    fn runs_under_settings_steptimeout() {
        let sd = sd();
        verify_cmd(&sd, "sleep 5");
        sd.verb_ok("config", &["set", "--global", "--json", "settings.stepTimeout", "1"]);
        let r = sd.run(&["verify"]);
        assert_ne!(r.status, 0);
        assert_has(&r.output, &["timed out"]);
    }

    // @test "a TERM or INT to it ends the command, and it waits for that (#170)"
    #[test]
    fn a_term_or_int_to_it_ends_the_command_and_it_waits_for_that_170() {
        let sd = sd();
        let cmd = sd.tmp.join("cmd");
        for sig in [libc::SIGTERM, libc::SIGINT] {
            let _ = fs::remove_file(&cmd);
            let c = cmd.display();
            verify_cmd(&sd, &format!("echo $$ > '{c}.tmp' && mv '{c}.tmp' '{c}' && exec sleep 30"));
            let mut job = spawn(sd.command(&sd.bin).arg("verify").stdin(Stdio::null()));
            for _ in 0..100 {
                if fs::metadata(&cmd).is_ok_and(|m| m.len() > 0) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            unsafe { libc::kill(job.id() as i32, sig) };
            let _ = job.wait();
            let pid: i32 = fs::read_to_string(&cmd).unwrap().trim().parse().unwrap();
            assert_ne!(unsafe { libc::kill(pid, 0) }, 0, "signal {sig}: the command is still running");
        }
    }
}

mod autofix {
    use super::*;

    fn autofix_cmd(sd: &Sd, c: &str) {
        sd.set_global("settings.autofix", c);
    }

    // @test "no settings.autofix: says so and passes"
    #[test]
    fn no_settings_autofix_says_so_and_passes() {
        let sd = sd();
        let r = sd.run(&["autofix"]);
        assert_eq!(r.status, 0);
        assert_has(&r.output, &["no settings.autofix"]);
    }

    // @test "a formatter that changes the tree: committed, the message names the command"
    #[test]
    fn a_formatter_that_changes_the_tree_committed_the_message_names_the_command() {
        let sd = sd();
        autofix_cmd(&sd, r"printf '%s\n' a b c d > file.txt");
        let before = sd.head_sha();
        let r = sd.run(&["autofix"]);
        assert_eq!(r.status, 0);
        assert_has(&r.output, &["autofix committed"]);
        assert_ne!(sd.head_sha(), before);
        assert_eq!(sd.git(&["status", "--porcelain"]), "");
        assert_eq!(sd.git(&["log", "-1", "--format=%s"]), r"autofix: printf '%s\n' a b c d > file.txt");
        let stat = sd.git(&["show", "--stat", "--format=", "HEAD"]);
        assert_eq!(stat.lines().last().unwrap(), " 1 file changed, 1 insertion(+)");
    }

    // @test "new files the formatter makes are committed too"
    #[test]
    fn new_files_the_formatter_makes_are_committed_too() {
        let sd = sd();
        autofix_cmd(&sd, "echo gen > generated.txt");
        assert_eq!(sd.run(&["autofix"]).status, 0);
        assert_eq!(sd.git(&["status", "--porcelain"]), "");
        assert_eq!(sd.git_status(&["cat-file", "-e", "HEAD:generated.txt"]), 0);
    }

    // @test "a clean tree: nothing committed, says so"
    #[test]
    fn a_clean_tree_nothing_committed_says_so() {
        let sd = sd();
        autofix_cmd(&sd, "true");
        let before = sd.head_sha();
        let r = sd.run(&["autofix"]);
        assert_eq!(r.status, 0);
        assert_has(&r.output, &["autofix changed nothing"]);
        assert_eq!(sd.head_sha(), before);
    }

    // @test "a failing command: nonzero, the output trimmed to its last 40 lines, nothing committed"
    #[test]
    fn a_failing_command_nonzero_the_output_trimmed_to_its_last_40_lines_nothing_committed() {
        let sd = sd();
        autofix_cmd(&sd, "seq 1 60; echo 'lint: unfixable' >&2; exit 2");
        let before = sd.head_sha();
        let r = sd.run(&["autofix"]);
        assert_ne!(r.status, 0);
        assert_has(&r.output, &["autofix failed"]);
        assert_has(&r.output, &["lint: unfixable"]);
        assert_has(&r.output, &["last 40 of 61 lines"]);
        assert!(has_line(&r.output, "60"));
        assert_eq!(sd.head_sha(), before);
    }

    // @test "a failing command that changed files leaves them uncommitted, and says so"
    #[test]
    fn a_failing_command_that_changed_files_leaves_them_uncommitted_and_says_so() {
        let sd = sd();
        autofix_cmd(&sd, "echo half >> file.txt; exit 1");
        let before = sd.head_sha();
        let r = sd.run(&["autofix"]);
        assert_ne!(r.status, 0);
        assert_has(&r.output, &["left uncommitted"]);
        assert_eq!(sd.head_sha(), before);
        assert_ne!(sd.git(&["status", "--porcelain"]), "");
    }

    // @test "uncommitted changes are refused before anything runs"
    #[test]
    fn uncommitted_changes_are_refused_before_anything_runs() {
        let sd = sd();
        autofix_cmd(&sd, "echo x > ran");
        sd.write("file.txt", "a\nb\nc\ndirty\n");
        let r = sd.run(&["autofix"]);
        assert_ne!(r.status, 0);
        assert_has(&r.output, &["uncommitted changes"]);
        assert!(!sd.exists("ran"));
    }

    // @test "runs in the branch's worktree, not the current one"
    #[test]
    fn runs_in_the_branchs_worktree_not_the_current_one() {
        let sd = sd();
        sd.git(&["worktree", "add", "-q", &sd.path("wt"), "-b", "other"]);
        autofix_cmd(&sd, "echo fixed >> file.txt");
        assert_eq!(sd.run(&["autofix", "other"]).status, 0);
        assert_eq!(sd.git_in("wt", &["log", "-1", "--format=%s"]), "autofix: echo fixed >> file.txt");
        assert_eq!(sd.git(&["log", "-1", "--format=%s"]), "init");
    }

    // @test "a branch checked out nowhere fails"
    #[test]
    fn a_branch_checked_out_nowhere_fails() {
        let sd = sd();
        sd.git(&["branch", "parked"]);
        autofix_cmd(&sd, "true");
        let r = sd.run(&["autofix", "parked"]);
        assert_ne!(r.status, 0);
        assert_has(&r.output, &["not checked out"]);
    }

    // @test "a git status that fails stops it before the command runs"
    #[test]
    fn a_git_status_that_fails_stops_it_before_the_command_runs() {
        let sd = sd();
        autofix_cmd(&sd, "touch ran");
        sd.write(".git/index", "not an index");
        let r = sd.run(&["autofix"]);
        assert_ne!(r.status, 0);
        assert_has(&r.output, &["git status failed"]);
        assert!(!sd.exists("ran"));
    }

    // @test "a list-valued autofix is refused before anything runs"
    #[test]
    fn a_list_valued_autofix_is_refused_before_anything_runs() {
        let sd = sd();
        let r = sd.exec(&sd.link("config"), &[], &["set", "--global", "--json", "settings.autofix", r#"["true","false"]"#]);
        assert_eq!(r.status, 1);
        sd.write_global(r#"{"settings":{"autofix":["true","false"]}}"#);
        let r = sd.run(&["autofix"]);
        assert_ne!(r.status, 0);
        assert_has(&r.output, &[r#"settings.autofix: expected a string, got ["true","false"]"#]);
    }

    // @test "the branch file's autofix beats the global file's"
    #[test]
    fn the_branch_files_autofix_beats_the_global_files() {
        let sd = sd();
        autofix_cmd(&sd, "echo global >> file.txt");
        sd.verb_ok("config", &["set", "settings.autofix", "echo branch >> file.txt"]);
        assert_eq!(sd.run(&["autofix"]).status, 0);
        let file = fs::read_to_string(sd.cwd.join("file.txt")).unwrap();
        assert_eq!(file.lines().last(), Some("branch"));
    }

    // @test "runs under settings.stepTimeout"
    #[test]
    fn runs_under_settings_steptimeout() {
        let sd = sd();
        autofix_cmd(&sd, "sleep 5");
        sd.verb_ok("config", &["set", "--global", "--json", "settings.stepTimeout", "1"]);
        let r = sd.run(&["autofix"]);
        assert_ne!(r.status, 0);
        assert_has(&r.output, &["timed out"]);
    }

    // @test "the commit's hooks are heard: a pre-commit hook's output passes through"
    #[test]
    fn the_commits_hooks_are_heard_a_pre_commit_hooks_output_passes_through() {
        let sd = sd();
        autofix_cmd(&sd, "echo d >> file.txt");
        sd.script(&sd.cwd.join(".git/hooks/pre-commit"),"#!/bin/sh\necho hook-out\necho hook-err >&2\n");
        let r = sd.run(&["autofix"]);
        assert_eq!(r.status, 0);
        assert_has(&r.output, &["hook-out"]);
        assert_has(&r.output, &["hook-err"]);
        assert_has(&r.output, &["autofix committed"]);
    }
}
