// Black-box tests of sd act verify (A10) and autofix (A11) (#202): the built
// binary, run as sd-act through a link (tests/common). Moved from
// libexec/sd-act-verify.bats and sd-act-autofix.bats; each test keeps the
// bats test's title above it. autofix's implement tests run it as the agent
// steps do: implement runs autofix when verify fails, then verifies again
// (address hooks in the same way, through the same function).
//
// Real git; the commands are shell one-liners. gh is a stub that fails;
// claude is a stub whose edit $FX/agent makes, for the implement hook.

mod common;

use common::{assert_has, assert_lacks, assert_match, spawn, Sd, GH_FAILS};
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

    /// refs/sheepdog/feat/verify-red's record, if any (#325).
    fn verify_red(&self) -> Option<serde_json::Value> {
        let rec = self.git_try(&["cat-file", "blob", "refs/sheepdog/feat/verify-red"])?;
        Some(serde_json::from_str(&rec).unwrap())
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

    #[test]
    fn a_failure_is_recorded_against_head_and_a_pass_removes_it_325() {
        let sd = sd();
        verify_cmd(&sd, "false");
        assert_ne!(sd.run(&["verify"]).status, 0);
        let rec = sd.verify_red().expect("a verify-red record");
        assert_eq!(rec["head"], sd.head_sha().as_str());
        assert_eq!(rec["step"], "verify");
        assert_eq!(rec["cmd"], "false");
        verify_cmd(&sd, "true");
        assert_eq!(sd.run(&["verify"]).status, 0);
        assert!(sd.verify_red().is_none());
    }

    #[test]
    fn a_command_that_changes_the_tree_is_recorded_red_too_325() {
        let sd = sd();
        verify_cmd(&sd, "echo formatted >> file.txt");
        assert_ne!(sd.run(&["verify"]).status, 0);
        assert_eq!(sd.verify_red().expect("a verify-red record")["head"], sd.head_sha().as_str());
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

    // bats' "Executed N instead of expected M" exits non-zero with no
    // "not ok" line, and its plan is above the 40-line tail (#325).
    #[test]
    fn failing_tap_short_of_its_plan_names_the_missing_numbers_and_the_test_before_them() {
        let sd = sd();
        verify_cmd(
            &sd,
            "echo 1..45; for i in $(seq 1 43); do echo \"ok $i t$i\"; done; \
             echo '# bats warning: Executed 43 instead of expected 45 tests'; exit 1",
        );
        let r = sd.run(&["verify"]);
        assert_ne!(r.status, 0);
        assert_has(&r.output, &["last 40 of 45 lines", "Executed 43 instead of expected 45"]);
        assert_has(&r.output, &["plan 1..45", "no line for 44-45 (after: ok 43 t43)"]);
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

mod running {
    use super::*;
    use crate::common::{dead_pid, running_marker, step_refusal, Holder};

    // @test "verify and autofix: refused while a live step holds the running marker; neither command runs (#270)"
    #[test]
    fn verify_and_autofix_refused_while_a_live_step_holds_the_running_marker_neither_command_runs_270() {
        let sd = sd();
        sd.set_global("settings.verify", "touch ../verify-ran");
        sd.set_global("settings.autofix", "touch ../autofix-ran; echo d >> file.txt");
        let head = sd.head_sha();
        let holder = Holder::new();
        let marker = running_marker(&sd.tmp.join("repo/.git"), "feat", "address", holder.pid());
        for (verb, doing) in [("verify", "running verify"), ("autofix", "running autofix")] {
            let r = sd.run(&[verb]);
            assert_ne!(r.status, 0, "{verb}");
            assert_has(&r.output, &[&step_refusal("address", "feat", holder.pid(), doing)]);
            assert!(!sd.tmp.join(format!("{verb}-ran")).exists(), "{verb}");
        }
        assert_eq!(sd.head_sha(), head);
        assert_eq!(sd.git(&["status", "--porcelain"]), "");
        assert!(marker.exists());
    }

    // @test "verify and autofix: the step's own child (RUNNING_HELD=<b>) runs them, and a dead holder's marker doesn't refuse (#270)"
    #[test]
    fn verify_and_autofix_the_steps_own_child_runs_them_and_a_dead_holders_marker_doesnt_refuse_270() {
        let holder = Holder::new();
        for (pid, env) in [(holder.pid(), &[("RUNNING_HELD", "feat")][..]), (dead_pid(), &[])] {
            let sd = sd();
            sd.set_global("settings.verify", "touch ../verify-ran");
            sd.set_global("settings.autofix", "echo d >> file.txt");
            running_marker(&sd.tmp.join("repo/.git"), "feat", "ci-fix", pid);
            let r = sd.run_env(env, &["verify"]);
            assert_eq!(r.status, 0, "{env:?}: {}", r.output);
            assert!(sd.tmp.join("verify-ran").exists());
            let head = sd.head_sha();
            let r = sd.run_env(env, &["autofix"]);
            assert_eq!(r.status, 0, "{env:?}: {}", r.output);
            assert_ne!(sd.head_sha(), head);
        }
    }
}

mod marker {
    use super::*;
    use crate::common::{running_marker, Holder};

    // @test "verify by hand holds the running marker, naming verify, removed after (#270)"
    #[test]
    fn verify_by_hand_holds_the_running_marker_naming_verify_removed_after_270() {
        let sd = sd();
        sd.set_global("settings.verify", "cp .git/sheepdog/feat.running ../verify-saw");
        let r = sd.run(&["verify"]);
        assert_eq!(r.status, 0, "{}", r.output);
        let line = fs::read_to_string(sd.tmp.join("verify-saw")).unwrap();
        assert_eq!(line.split_whitespace().nth(1), Some("verify"), "{line}");
        assert!(!sd.tmp.join("repo/.git/sheepdog/feat.running").exists());
    }

    // @test "verify by hand failing leaves no running marker behind (#270)"
    #[test]
    fn verify_by_hand_failing_leaves_no_running_marker_behind_270() {
        let sd = sd();
        sd.set_global("settings.verify", "false");
        let r = sd.run(&["verify"]);
        assert_ne!(r.status, 0);
        assert_has(&r.output, &["verify failed"]);
        assert!(!sd.tmp.join("repo/.git/sheepdog/feat.running").exists());
    }

    // @test "verify as a step's child (RUNNING_HELD=<b>) runs under the step's marker and leaves it (#270)"
    #[test]
    fn verify_as_a_steps_child_runs_under_the_steps_marker_and_leaves_it_270() {
        let sd = sd();
        sd.set_global("settings.verify", "cp .git/sheepdog/feat.running ../verify-saw");
        let holder = Holder::new();
        let marker = running_marker(&sd.tmp.join("repo/.git"), "feat", "push", holder.pid());
        let before = fs::read_to_string(&marker).unwrap();
        let r = sd.run_env(&[("RUNNING_HELD", "feat")], &["verify"]);
        assert_eq!(r.status, 0, "{}", r.output);
        assert_eq!(fs::read_to_string(sd.tmp.join("verify-saw")).unwrap(), before);
        assert_eq!(fs::read_to_string(&marker).unwrap(), before);
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
        sd.script(&sd.cwd.join(".git/hooks/pre-commit"), "#!/bin/sh\necho hook-out\necho hook-err >&2\n");
        let r = sd.run(&["autofix"]);
        assert_eq!(r.status, 0);
        assert_has(&r.output, &["hook-out"]);
        assert_has(&r.output, &["hook-err"]);
        assert_has(&r.output, &["autofix committed"]);
    }

    // @test "--help lists autofix"
    #[test]
    fn help_lists_autofix() {
        let sd = sd();
        let r = sd.run(&["--help"]);
        assert_match(&r.output, "sd act autofix ");
    }
}

// ── the implement hook: autofix when verify fails, then verify again ─────────

mod implement {
    use super::*;

    const CLAUDE: &str = r#"#!/usr/bin/env bash
printf '%s\n' "$@" > "$FX/claude.args"
[[ -x $FX/agent ]] && "$FX/agent"
jq -n --argjson r "$(cat "$FX/result.json")" '{type: "result", is_error: false, structured_output: $r}'
"#;

    /// The bats setup's agent half (the state dir, $FX, the fake claude) and
    /// implement_setup: a task, an agent that adds "d  " (trailing spaces,
    /// so verify fails), its result, and a verify that rejects them.
    fn implement_setup() -> Sd {
        let mut sd = sd();
        sd.env = vec![("SHEEPDOG_STATE_DIR".into(), sd.path("state")), ("FX".into(), sd.path("fx"))];
        fs::create_dir(sd.tmp.join("fx")).unwrap();
        sd.fake("claude", CLAUDE);
        sd.git(&["config", "branch.feat.sd-task", "add a fourth line"]);
        fs::write(
            sd.tmp.join("fx/result.json"),
            "{\"outcome\": \"done\", \"summary\": \"added d\", \"commit_message\": \"add d\"}\n",
        )
        .unwrap();
        sd.agent("#!/usr/bin/env bash\nprintf \"%s\\n\" a b c \"d  \" > file.txt\n");
        sd.set_global("settings.verify", "! grep -q ' $' file.txt");
        sd
    }

    impl Sd {
        /// $FX/agent := <script>, what the agent does.
        fn agent(&self, script: &str) {
            self.script(&self.tmp.join("fx/agent"), script);
        }

        fn subject(&self) -> String {
            self.git(&["log", "-1", "--format=%s"])
        }
    }

    fn autofix_cmd(sd: &Sd, c: &str) {
        sd.set_global("settings.autofix", c);
    }

    // @test "implement: a failed verify runs autofix; when it changes the tree, verify runs again"
    #[test]
    fn implement_a_failed_verify_runs_autofix_when_it_changes_the_tree_verify_runs_again() {
        let sd = implement_setup();
        autofix_cmd(&sd, "sed -i.bak 's/ *$//' file.txt && rm file.txt.bak");
        let r = sd.run(&["implement"]);
        assert_eq!(r.status, 0);
        assert_has(&r.output, &["autofix committed"]);
        assert_has(&r.output, &["verify passed"]);
        assert_eq!(sd.subject(), "autofix: sed -i.bak 's/ *$//' file.txt && rm file.txt.bak");
        assert_eq!(sd.git(&["log", "-2", "--format=%s"]).lines().last(), Some("add d"));
        assert_eq!(sd.git(&["status", "--porcelain"]), "");
        assert!(sd.verify_red().is_none(), "autofix's commit passed verify: nothing red at HEAD");
    }

    // @test "implement: a failed verify with no autofix fails as before"
    #[test]
    fn implement_a_failed_verify_with_no_autofix_fails_as_before() {
        let sd = implement_setup();
        let r = sd.run(&["implement"]);
        assert_ne!(r.status, 0);
        assert_has(&r.output, &["verify failed"]);
        assert_lacks(&r.output, "autofix");
        assert_eq!(sd.subject(), "add d");
        let rec = sd.verify_red().expect("a verify-red record");
        assert_eq!(rec["head"], sd.head_sha().as_str());
        assert_eq!(rec["step"], "implement");
    }

    // @test "implement: autofix that changes nothing doesn't rerun verify; the failure stands"
    #[test]
    fn implement_autofix_that_changes_nothing_doesnt_rerun_verify_the_failure_stands() {
        let sd = implement_setup();
        autofix_cmd(&sd, "true");
        let r = sd.run(&["implement"]);
        assert_ne!(r.status, 0);
        assert_has(&r.output, &["autofix changed nothing"]);
        assert_has(&r.output, &["verify failed"]);
    }

    // @test "implement: autofix failing after it changed files: reverted, the verify failure stands"
    #[test]
    fn implement_autofix_failing_after_it_changed_files_reverted_the_verify_failure_stands() {
        let sd = implement_setup();
        autofix_cmd(&sd, "echo half >> file.txt; exit 1");
        let r = sd.run(&["implement"]);
        assert_ne!(r.status, 0);
        assert_has(&r.output, &["reverted"]);
        assert_has(&r.output, &["verify failed"]);
        assert_eq!(sd.git(&["status", "--porcelain"]), "");
        assert_eq!(sd.subject(), "add d");
    }

    // @test "implement: autofix commits but verify still fails: nonzero, the autofix commit stays"
    #[test]
    fn implement_autofix_commits_but_verify_still_fails_nonzero_the_autofix_commit_stays() {
        let sd = implement_setup();
        autofix_cmd(&sd, "echo more >> file.txt");
        let r = sd.run(&["implement"]);
        assert_ne!(r.status, 0);
        assert_has(&r.output, &["autofix committed"]);
        assert_has(&r.output, &["verify failed"]);
        assert_eq!(sd.subject(), "autofix: echo more >> file.txt");
        assert_eq!(sd.verify_red().expect("a verify-red record")["head"], sd.head_sha().as_str());
    }

    // @test "implement: verify dirtying the tree is verify's failure; autofix doesn't run"
    #[test]
    fn implement_verify_dirtying_the_tree_is_verifys_failure_autofix_doesnt_run() {
        let sd = implement_setup();
        sd.set_global("settings.verify", "echo touched >> file.txt");
        autofix_cmd(&sd, "echo ran > autofix-ran");
        let r = sd.run(&["implement"]);
        assert_ne!(r.status, 0);
        assert_has(&r.output, &["changed files"]);
        assert_lacks(&r.output, "clean tree");
        assert!(!sd.exists("autofix-ran"));
    }

    // @test "implement: verify passing first time never runs autofix"
    #[test]
    fn implement_verify_passing_first_time_never_runs_autofix() {
        let sd = implement_setup();
        sd.agent("#!/usr/bin/env bash\nprintf \"%s\\n\" a b c d > file.txt\n");
        autofix_cmd(&sd, "echo ran > autofix-ran");
        let r = sd.run(&["implement"]);
        assert_eq!(r.status, 0);
        assert!(!sd.exists("autofix-ran"));
    }
}
