// Black-box tests of sd act attach (A9, takeover) (#203): take
// branch.<b>.sd-lock, open the branch's session interactively in its
// worktree, release the lock when the session exits, however it exits. While
// the lock is held, agent steps refuse to run; a stale lock (its pid is gone
// from this host) is cleared. The built binary, run as sd-act through a link
// (tests/common). Moved from libexec/sd-act-attach.bats; each test keeps the
// bats test's title above it. "--help lists attach" is in tests/act.rs,
// beside the dispatcher's other --help tests.
//
// claude and pi are stubs standing in for the TUI: they record their args,
// cwd and the lock as it is while they run, then exit as $FX says
// ($FX/agent runs first, $FX/exit is the status, $FX/hang waits for a
// signal), and write the session's transcript where the real ones do
// ($CLAUDE_CONFIG_DIR, $PI_CODING_AGENT_SESSION_DIR), unless
// $FX/no-transcript. gh is a stub with no PRs, for the verbs that read sd
// state first; sd state is the real one, in-process.

mod common;

use common::{assert_has, assert_match, chomp, spawn, Sd};
use sheepdog::running::proc_start;
use std::fs;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const GH: &str = r#"#!/usr/bin/env bash
echo '{"data":{"viewer":{"login":"me"},"repository":{"defaultBranchRef":{"name":"main"},"pullRequests":{"nodes":[]}}}}'
"#;

/// The TUI stub, HARNESS being claude or pi.
const TUI: &str = r#"#!/usr/bin/env bash
printf '%s\n' "$@" > "$FX/HARNESS.args"
pwd > "$FX/HARNESS.cwd"
echo "${ANTHROPIC_API_KEY-unset}" > "$FX/HARNESS.key"
sid=$(grep -A1 -x -e --resume -e --session-id "$FX/HARNESS.args" | tail -1)
[[ -e $FX/no-transcript ]] || { [[ HARNESS == claude ]] && touch "$CLAUDE_CONFIG_DIR/projects/-repo/$sid.jsonl" || touch "$PI_CODING_AGENT_SESSION_DIR/--repo--/2026-10-16T00-00-00-000Z_$sid.jsonl"; }
git config --get branch.feat.sd-lock > "$FX/HARNESS.lock" || echo "(no lock)" > "$FX/HARNESS.lock"
echo $$ > "$FX/HARNESS.pid"
[[ -x $FX/agent ]] && "$FX/agent"
[[ -e $FX/hang ]] && exec sleep 30
[[ -e $FX/exit ]] && exit "$(cat "$FX/exit")"
exit 0
"#;

/// A claude for implement: it edits file.txt and answers $FX/result.json.
const IMPLEMENTER: &str = r#"#!/usr/bin/env bash
printf '%s\n' "$@" > "$FX/claude.args"
echo d >> file.txt
jq -n --argjson r "$(cat "$FX/result.json")" '{type: "result", is_error: false, structured_output: $r}'
"#;

const SID: &str = "11111111-2222-4333-8444-555555555555";

/// The bats setup: repo (cwd) with feat one commit past main, enrolled, with
/// a task and a claude session.
fn sd() -> Sd {
    let mut sd = Sd::for_verb("act");
    sd.env = vec![
        ("SHEEPDOG_STATE_DIR".to_string(), sd.path("state")),
        ("GH".to_string(), sd.path("gh")),
        ("FX".to_string(), sd.path("fx")),
        ("CLAUDE_CONFIG_DIR".to_string(), sd.path("claude")),
        ("PI_CODING_AGENT_SESSION_DIR".to_string(), sd.path("pi")),
    ];
    for d in ["gh", "fx", "claude/projects/-repo", "pi/--repo--"] {
        fs::create_dir_all(sd.tmp.join(d)).unwrap();
    }
    sd.fake("gh", GH);
    for h in ["claude", "pi"] {
        sd.fake(h, &TUI.replace("HARNESS", h));
    }
    sd.git(&["init", "-q", &sd.path("repo")]);
    sd.cd("repo");
    sd.write("file.txt", "a\nb\n");
    sd.git(&["add", "file.txt"]);
    sd.git(&["commit", "-qm", "init"]);
    sd.git(&["switch", "-qc", "feat"]);
    sd.write("file.txt", "a\nb\nc\n");
    sd.git(&["commit", "-qam", "add c"]);
    sd.git(&["config", "branch.feat.sheepdog", "true"]);
    sd.git(&["config", "branch.feat.sd-task", "add a third line"]);
    sd.git(&["config", "branch.feat.sd-session", SID]);
    sd
}

fn host() -> String {
    chomp(&String::from_utf8_lossy(&Command::new("hostname").output().unwrap().stdout))
}

/// A process of this host that is running, killed when dropped (teardown).
struct Sleeper(Child);

impl Drop for Sleeper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Sleeper {
    /// "$SLEEPER@$HOST 2026-10-15T00:00:00Z", the lock live_lock wrote.
    fn lock(&self) -> String {
        format!("{}@{} 2026-10-15T00:00:00Z", self.0.id(), host())
    }
}

impl Sd {
    fn lock(&self) -> Option<String> {
        self.git_try(&["config", "--get", "branch.feat.sd-lock"])
    }

    fn config(&self, key: &str) -> Option<String> {
        self.git_try(&["config", "--get", key])
    }

    /// live_lock: a lock held by a process of this host that is running.
    fn live_lock(&self) -> Sleeper {
        let s = Sleeper(spawn(Command::new("sleep").arg("30")));
        self.git(&["config", "branch.feat.sd-lock", &s.lock()]);
        s
    }

    /// dead_lock: a lock held by a process of this host that is gone.
    fn dead_lock(&self) {
        self.git(&["config", "branch.feat.sd-lock", &format!("2147483646@{} 2026-10-15T00:00:00Z", host())]);
    }

    fn fx(&self, name: &str, content: &str) {
        fs::write(self.tmp.join("fx").join(name), content).unwrap();
    }

    fn fx_read(&self, name: &str) -> String {
        chomp(&fs::read_to_string(self.tmp.join("fx").join(name)).unwrap_or_default())
    }

    fn fx_exists(&self, name: &str) -> bool {
        self.tmp.join("fx").join(name).exists()
    }

    /// `"$SD_ACT" attach 2>/dev/null &`, then wait_for "$FX/claude.pid".
    fn attach_bg(&self) -> Child {
        let mut job = spawn(self.command(&self.bin).arg("attach").stdin(Stdio::null()).stderr(Stdio::null()));
        let pid = self.tmp.join("fx/claude.pid");
        for _ in 0..100 {
            if fs::metadata(&pid).is_ok_and(|m| m.len() > 0) {
                return job;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = job.kill();
        let _ = job.wait();
        panic!("the stub never started");
    }

    /// kill -<sig> <job> "$(cat "$FX/claude.pid")"; wait <job> || true
    fn kill_both(&self, mut job: Child, sig: i32) {
        let tui: i32 = self.fx_read("claude.pid").parse().unwrap();
        unsafe {
            libc::kill(job.id() as i32, sig);
            libc::kill(tui, sig);
        }
        let _ = job.wait();
    }
}

/// grep -qx <line> over <s>.
fn has_line(s: &str, line: &str) -> bool {
    s.lines().any(|l| l == line)
}

/// grep -A1 -x -- <flag> <args> | tail -1: the argument after <flag>.
fn after(args: &str, flag: &str) -> String {
    let lines: Vec<&str> = args.lines().collect();
    let i = lines.iter().rposition(|l| *l == flag).unwrap_or_else(|| panic!("no {flag} in {args:?}"));
    lines.get(i + 1).unwrap_or(&"").to_string()
}

const UUID: &str = "^[0-9a-f-]{36}$";

// ── attach ───────────────────────────────────────────────────────────────────

// @test "attach: holds the lock while the session is open, in the branch's worktree, resuming its session"
#[test]
fn attach_holds_the_lock_while_the_session_is_open_in_the_branchs_worktree_resuming_its_session() {
    let sd = sd();
    let r = sd.run(&["attach"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.fx_read("claude.cwd"), sd.path("repo"));
    let args = sd.fx_read("claude.args");
    assert!(has_line(&args, "--resume"));
    assert!(has_line(&args, SID));
    assert!(!has_line(&args, "-p"));
    let host = regex::escape(&host());
    assert_match(&sd.fx_read("claude.lock"), &format!(r"^[0-9]+@{host} 20[0-9][0-9]-[0-9][0-9]-[0-9][0-9]T"));
    assert_eq!(sd.lock(), None);
}

// @test "attach: says what it attached to, on stderr"
#[test]
fn attach_says_what_it_attached_to_on_stderr() {
    let sd = sd();
    let r = sd.run_sep(&["attach"]);
    assert_eq!(r.status, 0);
    assert_has(&r.stderr, &["attached to feat"]);
    assert_has(&r.stderr, &[SID]);
    assert_eq!(r.output, "");
}

// @test "attach: a named branch, from another worktree"
#[test]
fn attach_a_named_branch_from_another_worktree() {
    let mut sd = sd();
    sd.git(&["worktree", "add", "-q", &sd.path("wt-main"), "main"]);
    sd.cd("wt-main");
    let r = sd.run(&["attach", "feat"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.fx_read("claude.cwd"), sd.path("repo"));
}

// @test "attach: a branch checked out nowhere fails"
#[test]
fn attach_a_branch_checked_out_nowhere_fails() {
    let sd = sd();
    sd.git(&["switch", "-q", "main"]);
    sd.git(&["worktree", "prune"]);
    let r = sd.run(&["attach", "feat"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["isn't checked out"]);
    assert!(!sd.fx_exists("claude.args"));
}

// @test "attach: no session yet: starts one with --session-id and remembers it"
#[test]
fn attach_no_session_yet_starts_one_with_session_id_and_remembers_it() {
    let sd = sd();
    sd.git(&["config", "--unset", "branch.feat.sd-session"]);
    let r = sd.run(&["attach"]);
    assert_eq!(r.status, 0);
    let args = sd.fx_read("claude.args");
    assert!(has_line(&args, "--session-id"));
    let sid = after(&args, "--session-id");
    assert_match(&sid, UUID);
    assert_eq!(sd.config("branch.feat.sd-session"), Some(sid));
    assert_eq!(sd.config("branch.feat.sd-harness").as_deref(), Some("claude"));
}

// @test "attach: no session yet, and the harness wrote no transcript: nothing stored, and it says so"
#[test]
fn attach_no_session_yet_and_the_harness_wrote_no_transcript_nothing_stored_and_it_says_so() {
    let sd = sd();
    sd.git(&["config", "--unset", "branch.feat.sd-session"]);
    sd.fx("no-transcript", "");
    let r = sd.run_sep(&["attach"]);
    assert_eq!(r.status, 0);
    assert_has(&r.stderr, &["no transcript"]);
    assert_eq!(sd.config("branch.feat.sd-session"), None);
    assert_eq!(sd.lock(), None);
}

// @test "attach: a resumed session stays stored even when the harness wrote nothing"
#[test]
fn attach_a_resumed_session_stays_stored_even_when_the_harness_wrote_nothing() {
    let sd = sd();
    sd.fx("no-transcript", "");
    let r = sd.run(&["attach"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.config("branch.feat.sd-session").as_deref(), Some(SID));
}

// @test "attach: the harness block's model, effort and args become the TUI's flags"
#[test]
fn attach_the_harness_blocks_model_effort_and_args_become_the_tuis_flags() {
    let sd = sd();
    sd.write_global(r#"{"harnesses": {"claude": {"model": "opus", "effort": "high", "args": "--verbose --fast"}}}"#);
    let r = sd.run(&["attach"]);
    assert_eq!(r.status, 0);
    assert_eq!(
        fs::read_to_string(sd.tmp.join("fx/claude.args")).unwrap().replace('\n', " "),
        format!("--resume {SID} --model opus --effort high --verbose --fast ")
    );
}

// @test "attach: claude runs without ANTHROPIC_API_KEY unless harnesses.claude.useApiKey"
#[test]
fn attach_claude_runs_without_anthropic_api_key_unless_harnesses_claude_useapikey() {
    let sd = sd();
    let key = [("ANTHROPIC_API_KEY", "sk-no-credit")];
    let r = sd.run_env(&key, &["attach"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.fx_read("claude.key"), "unset");
    sd.write_global(r#"{"harnesses": {"claude": {"useApiKey": true}}}"#);
    let r = sd.run_env(&key, &["attach"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.fx_read("claude.key"), "sk-no-credit");
}

// @test "attach: pi keeps ANTHROPIC_API_KEY"
#[test]
fn attach_pi_keeps_anthropic_api_key() {
    let sd = sd();
    sd.git(&["config", "branch.feat.sd-harness", "pi"]);
    let r = sd.run_env(&[("ANTHROPIC_API_KEY", "sk-paid")], &["attach"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.fx_read("pi.key"), "sk-paid");
}

// @test "attach: pi's flags: provider, model, thinking"
#[test]
fn attach_pis_flags_provider_model_thinking() {
    let sd = sd();
    sd.write_global(r#"{"harnesses": {"pi": {"provider": "cf", "model": "haiku", "effort": "low"}}}"#);
    sd.git(&["config", "branch.feat.sd-harness", "pi"]);
    let r = sd.run(&["attach"]);
    assert_eq!(r.status, 0);
    assert_eq!(
        fs::read_to_string(sd.tmp.join("fx/pi.args")).unwrap().replace('\n', " "),
        format!("--session-id {SID} --provider cf --model haiku --thinking low ")
    );
}

// @test "attach: pi: pi --session-id, no -p"
#[test]
fn attach_pi_pi_session_id_no_p() {
    let sd = sd();
    sd.git(&["config", "branch.feat.sd-harness", "pi"]);
    let r = sd.run(&["attach"]);
    assert_eq!(r.status, 0);
    assert!(!sd.fx_exists("claude.args"));
    let args = sd.fx_read("pi.args");
    assert!(has_line(&args, "--session-id"));
    assert!(has_line(&args, SID));
    assert!(!has_line(&args, "-p"));
    assert_match(&sd.fx_read("pi.lock"), "^[0-9]+@");
}

// @test "attach: --harness picks the harness for a branch with no session"
#[test]
fn attach_harness_picks_the_harness_for_a_branch_with_no_session() {
    let sd = sd();
    sd.git(&["config", "--unset", "branch.feat.sd-session"]);
    let r = sd.run(&["attach", "--harness", "pi"]);
    assert_eq!(r.status, 0);
    assert!(!sd.fx_exists("claude.args"));
    assert!(has_line(&sd.fx_read("pi.args"), "--session-id"));
    assert_eq!(sd.config("branch.feat.sd-harness").as_deref(), Some("pi"));
}

// @test "attach: --harness can't switch a branch whose session belongs to the other harness"
#[test]
fn attach_harness_cant_switch_a_branch_whose_session_belongs_to_the_other_harness() {
    let sd = sd();
    let r = sd.run(&["attach", "--harness", "pi"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["belongs to claude"]);
    assert!(!sd.fx_exists("pi.args"));
    assert_eq!(sd.lock(), None);
}

// @test "attach: the session's exit status comes through, and the lock is released anyway"
#[test]
fn attach_the_sessions_exit_status_comes_through_and_the_lock_is_released_anyway() {
    let sd = sd();
    sd.fx("exit", "3\n");
    let r = sd.run(&["attach"]);
    assert_eq!(r.status, 3);
    assert_eq!(sd.lock(), None);
}

// @test "attach: Ctrl-C (SIGINT to the foreground group) releases the lock"
#[test]
fn attach_ctrl_c_sigint_to_the_foreground_group_releases_the_lock() {
    let sd = sd();
    sd.fx("hang", "");
    let job = sd.attach_bg();
    assert_match(&sd.lock().unwrap_or_default(), "^[0-9]+@");
    sd.kill_both(job, libc::SIGINT);
    assert_eq!(sd.lock(), None);
}

// @test "attach: a new session is still recorded when the TUI was interrupted"
#[test]
fn attach_a_new_session_is_still_recorded_when_the_tui_was_interrupted() {
    let sd = sd();
    sd.git(&["config", "--unset", "branch.feat.sd-session"]);
    sd.fx("hang", "");
    let job = sd.attach_bg();
    let sid = after(&sd.fx_read("claude.args"), "--session-id");
    fs::write(sd.tmp.join(format!("claude/projects/-repo/{sid}.jsonl")), "").unwrap();
    sd.kill_both(job, libc::SIGINT);
    assert_match(&sd.config("branch.feat.sd-session").unwrap_or_default(), UUID);
    assert_eq!(sd.lock(), None);
}

// @test "attach: SIGTERM releases the lock"
#[test]
fn attach_sigterm_releases_the_lock() {
    let sd = sd();
    sd.fx("hang", "");
    let job = sd.attach_bg();
    sd.kill_both(job, libc::SIGTERM);
    assert_eq!(sd.lock(), None);
}

// @test "attach: refuses while a live lock is held; --force takes it over"
#[test]
fn attach_refuses_while_a_live_lock_is_held_force_takes_it_over() {
    let sd = sd();
    let sleeper = sd.live_lock();
    let held = sleeper.lock();
    let r = sd.run(&["attach"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["already attached"]);
    assert_has(&r.output, &[&format!("{}@{}", sleeper.0.id(), host())]);
    assert_has(&r.output, &["--force"]);
    assert!(!sd.fx_exists("claude.args"));
    assert_eq!(sd.lock(), Some(held.clone()));

    let r = sd.run(&["attach", "--force"]);
    assert_eq!(r.status, 0);
    let seen = sd.fx_read("claude.lock");
    assert_match(&seen, "^[0-9]+@");
    assert_ne!(seen, held);
    assert_eq!(sd.lock(), None);
}

// @test "attach: a stale lock (its process is gone) is replaced, with a note"
#[test]
fn attach_a_stale_lock_its_process_is_gone_is_replaced_with_a_note() {
    let sd = sd();
    sd.dead_lock();
    let r = sd.run_sep(&["attach"]);
    assert_eq!(r.status, 0);
    assert_has(&r.stderr, &["stale lock"]);
    assert_match(&sd.fx_read("claude.lock"), "^[0-9]+@");
    assert_eq!(sd.lock(), None);
}

// @test "attach: a lock from another host is never stale"
#[test]
fn attach_a_lock_from_another_host_is_never_stale() {
    let sd = sd();
    sd.git(&["config", "branch.feat.sd-lock", "1@elsewhere.example 2026-10-15T00:00:00Z"]);
    let r = sd.run(&["attach"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["already attached"]);
    assert!(!sd.fx_exists("claude.args"));
}

// @test "attach: releases only its own lock, not one that replaced it"
#[test]
fn attach_releases_only_its_own_lock_not_one_that_replaced_it() {
    let sd = sd();
    sd.script(
        &sd.tmp.join("fx/agent"),
        "#!/usr/bin/env bash\ngit config branch.feat.sd-lock \"other@elsewhere.example 2026-10-15T00:00:00Z\"\n",
    );
    let r = sd.run(&["attach"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.lock().as_deref(), Some("other@elsewhere.example 2026-10-15T00:00:00Z"));
}

/// The branch's running marker, written by <holder>: an implement step.
fn marker(sd: &Sd, holder: u32) -> (String, String) {
    let marker = sd.path("repo/.git/sheepdog/feat.running");
    fs::create_dir_all(sd.tmp.join("repo/.git/sheepdog")).unwrap();
    let pid = holder.to_string();
    let line = format!("{pid} implement 2026-10-18T14:03:00Z {}\n", proc_start(&pid));
    fs::write(&marker, &line).unwrap();
    (marker, line)
}

// @test "attach: refuses while a live agent step holds the running marker; nothing opens, the marker is untouched (#270)"
#[test]
fn attach_refuses_while_a_live_step_holds_the_running_marker_270() {
    let sd = sd();
    let holder = Sleeper(spawn(Command::new("sleep").arg("30")));
    let pid = holder.0.id();
    let (marker, line) = marker(&sd, pid);
    let r = sd.run(&["attach"]);
    assert_ne!(r.status, 0);
    assert_has(
        &r.output,
        &[
            &format!("implement is running on feat (pid {pid}, since 2026-10-18T14:03:00Z)"),
            "sd state",
            &format!("kill {pid}"),
        ],
    );
    assert!(!sd.fx_exists("claude.args"));
    assert_eq!(sd.lock(), None);
    assert_eq!(fs::read_to_string(&marker).unwrap(), line);
}

// @test "attach --force: takes over a lock, not a running step; the lock goes back (#270)"
#[test]
fn attach_force_takes_over_a_lock_but_not_a_running_step_270() {
    let sd = sd();
    let attached = sd.live_lock();
    let holder = Sleeper(spawn(Command::new("sleep").arg("30")));
    let (marker, line) = marker(&sd, holder.0.id());
    let r = sd.run(&["attach", "--force"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["implement is running on feat"]);
    assert!(!sd.fx_exists("claude.args"));
    assert_eq!(sd.lock(), Some(attached.lock()));
    assert_eq!(fs::read_to_string(&marker).unwrap(), line);
}

// @test "attach: a dead step's marker doesn't refuse (#270)"
#[test]
fn attach_a_dead_steps_marker_doesnt_refuse_270() {
    let sd = sd();
    let mut dead = spawn(Command::new("sh").args(["-c", "exit 0"]));
    dead.wait().unwrap();
    marker(&sd, dead.id());
    let r = sd.run(&["attach"]);
    assert_eq!(r.status, 0);
    assert!(sd.fx_exists("claude.args"));
    assert_eq!(sd.lock(), None);
}

// @test "attach: a session pointer moved during the attach is kept (#270)"
#[test]
fn attach_keeps_a_session_pointer_moved_during_the_attach_270() {
    let sd = sd();
    let moved = "99999999-2222-4333-8444-555555555555";
    sd.script(
        &sd.tmp.join("fx/agent"),
        &format!("#!/usr/bin/env bash\ngit config branch.feat.sd-session {moved}\n"),
    );
    let r = sd.run_sep(&["attach"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.config("branch.feat.sd-session").as_deref(), Some(moved));
    assert_has(&r.stderr, &[&format!("feat's session moved to {moved} during the attach, so {SID} isn't stored")]);
    assert_eq!(sd.lock(), None);
}

// @test "attach: a new session's pointer, set during the attach, is kept (#270)"
#[test]
fn attach_a_new_sessions_pointer_set_during_the_attach_is_kept_270() {
    let sd = sd();
    sd.git(&["config", "--unset", "branch.feat.sd-session"]);
    let moved = "99999999-2222-4333-8444-555555555555";
    sd.script(
        &sd.tmp.join("fx/agent"),
        &format!("#!/usr/bin/env bash\ngit config branch.feat.sd-session {moved}\n"),
    );
    let r = sd.run_sep(&["attach"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.config("branch.feat.sd-session").as_deref(), Some(moved));
    assert_has(&r.stderr, &[&format!("feat's session moved to {moved} during the attach")]);
}

// @test "attach: a session pointer unset during the attach stays unset, and it says so (#270)"
#[test]
fn attach_a_session_pointer_unset_during_the_attach_stays_unset_270() {
    let sd = sd();
    sd.script(&sd.tmp.join("fx/agent"), "#!/usr/bin/env bash\ngit config --unset branch.feat.sd-session\n");
    let r = sd.run_sep(&["attach"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.config("branch.feat.sd-session"), None);
    assert_has(&r.stderr, &[&format!("feat's session was unset during the attach, so {SID} isn't stored")]);
}

// ── agent steps while attached ───────────────────────────────────────────────

// @test "implement: refused while attached; nothing runs"
#[test]
fn implement_refused_while_attached_nothing_runs() {
    let sd = sd();
    let sleeper = sd.live_lock();
    let r = sd.run(&["implement"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["attached"]);
    assert_has(&r.output, &[&format!("{}@{}", sleeper.0.id(), host())]);
    assert!(!sd.fx_exists("claude.args"));
}

// @test "describe: refused while attached"
#[test]
fn describe_refused_while_attached() {
    let sd = sd();
    let _sleeper = sd.live_lock();
    let r = sd.run(&["describe"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["attached"]);
    assert!(!sd.fx_exists("claude.args"));
}

// @test "review: refused while attached, before verify runs"
#[test]
fn review_refused_while_attached_before_verify_runs() {
    let sd = sd();
    let _sleeper = sd.live_lock();
    sd.write_global(r#"{"settings": {"verify": "touch verify-ran"}}"#);
    let r = sd.run(&["review"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["attached"]);
    assert!(!sd.cwd.join("verify-ran").exists());
}

// @test "verify is mechanical: it runs while attached"
#[test]
fn verify_is_mechanical_it_runs_while_attached() {
    let sd = sd();
    let _sleeper = sd.live_lock();
    sd.write_global(r#"{"settings": {"verify": "true"}}"#);
    let r = sd.run(&["verify"]);
    assert_eq!(r.status, 0);
}

// @test "every verb clears a stale lock first"
#[test]
fn every_verb_clears_a_stale_lock_first() {
    let sd = sd();
    sd.dead_lock();
    let r = sd.run_sep(&["enroll"]);
    assert_eq!(r.status, 0);
    assert_has(&r.stderr, &["stale lock"]);
    assert_eq!(sd.lock(), None);
}

// @test "a stale lock is cleared and the step runs"
#[test]
fn a_stale_lock_is_cleared_and_the_step_runs() {
    let sd = sd();
    sd.dead_lock();
    sd.fx("result.json", r#"{"outcome": "done", "summary": "did it", "commit_message": "add c"}"#);
    sd.fake("claude", IMPLEMENTER);
    let r = sd.run_sep(&["implement"]);
    assert_eq!(r.status, 0);
    assert_has(&r.stderr, &["stale lock"]);
    assert!(sd.fx_exists("claude.args"));
    assert_eq!(sd.lock(), None);
    // The step's usage record names the step and the branch.
    let usage = fs::read_to_string(sd.tmp.join("state/usage.sup")).unwrap();
    assert_has(&usage, &[r#"branch:"feat",step:"implement",harness:"claude""#]);
}

// @test "--force runs an agent step over a live lock"
#[test]
fn force_runs_an_agent_step_over_a_live_lock() {
    let sd = sd();
    let sleeper = sd.live_lock();
    sd.fx("result.json", r#"{"outcome": "done", "summary": "did it", "commit_message": "add c"}"#);
    sd.fake("claude", IMPLEMENTER);
    let r = sd.run(&["implement", "--force"]);
    assert_eq!(r.status, 0);
    assert!(sd.fx_exists("claude.args"));
    assert_eq!(sd.lock(), Some(sleeper.lock()));
}
