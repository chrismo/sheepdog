// Black-box tests of sd act push (A3, #202): the built binary, run as sd-act
// through a link (tests/common). Moved from libexec/sd-act-push.bats; each
// test keeps the bats test's title above it.
//
// Real git: a bare origin, our clone, and other/ for other people's pushes.
// gh is a stub that fails: push never talks to GitHub.

mod common;

use common::{assert_has, dead_pid, running_marker, step_refusal, Holder, Sd, GH_FAILS};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

/// The bats setup: origin.git, its clones repo (cwd) and other, file.txt
/// on main, and feat one commit past it, pushed and tracking origin/feat.
fn sd() -> Sd {
    let mut sd = Sd::for_verb("act");
    sd.fake("gh", GH_FAILS);
    sd.origin(&["repo"]);
    sd.cd("repo");
    sd.write("file.txt", "a\nb\nc\n");
    sd.git(&["add", "file.txt"]);
    sd.git(&["commit", "-qm", "init"]);
    sd.git(&["push", "-q", "origin", "main"]);
    sd.git(&["clone", "-q", &sd.path("origin.git"), &sd.path("other")]);
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("feat");
    sd.git(&["push", "-q", "-u", "origin", "feat"]);
    sd
}

/// echo <line> >> <f>
fn append(f: &Path, line: &str) {
    writeln!(OpenOptions::new().create(true).append(true).open(f).unwrap(), "{line}").unwrap();
}

impl Sd {
    fn commit(&self, msg: &str) {
        append(&self.cwd.join("file.txt"), msg);
        self.git(&["commit", "-qam", msg]);
    }

    /// git -c user.email=them@example.com -c user.name=Them commit -qm <msg>, in <dir>.
    fn commit_as_them(&self, dir: &str, msg: &str) {
        self.git_in(dir, &["-c", "user.email=them@example.com", "-c", "user.name=Them", "commit", "-qm", msg]);
    }

    /// Someone else commits <msg> to feat on origin.
    fn upstream(&self, msg: &str) {
        self.git_in("other", &["fetch", "-q", "origin"]);
        self.git_in("other", &["checkout", "-q", "-B", "feat", "origin/feat"]);
        append(&self.tmp.join("other/other.txt"), msg);
        self.git_in("other", &["add", "other.txt"]);
        self.commit_as_them("other", msg);
        self.git_in("other", &["push", "-q", "origin", "feat"]);
    }

    /// other/ commits <line> appended to <file> on main and pushes it, after
    /// pulling main when <pull>; the author is Them when <them>.
    fn main_moves(&self, pull: bool, file: &str, line: &str, msg: &str, them: bool) {
        self.git_in("other", &["checkout", "-q", "main"]);
        if pull {
            self.git_in("other", &["pull", "-q", "origin", "main"]);
        }
        append(&self.tmp.join("other").join(file), line);
        self.git_in("other", &["add", file]);
        if them {
            self.commit_as_them("other", msg);
        } else {
            self.git_in("other", &["commit", "-qm", msg]);
        }
        self.git_in("other", &["push", "-q", "origin", "main"]);
    }

    /// The remote's sha for <b>: git ls-remote origin refs/heads/<b> | cut -f1.
    fn remote(&self, b: &str) -> String {
        self.remote_sha("origin", b)
    }

    fn remote_sha(&self, remote: &str, b: &str) -> String {
        let l = self.git(&["ls-remote", remote, &format!("refs/heads/{b}")]);
        l.split('\t').next().unwrap_or_default().to_string()
    }

    fn rev(&self, r: &str) -> String {
        self.git(&["rev-parse", r])
    }

    /// <b>'s upstream, or "" when it has none.
    fn tracks(&self, b: &str) -> String {
        self.git_try(&["rev-parse", "--abbrev-ref", &format!("{b}@{{upstream}}")]).unwrap_or_default()
    }

    fn inherit_main(&self) {
        self.git(&["branch", "-q", "--set-upstream-to=origin/main", "feat"]);
    }
}

// @test "nothing to push: says so"
#[test]
fn nothing_to_push_says_so() {
    let sd = sd();
    let r = sd.run(&["push"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, "nothing to push on feat");
}

// @test "the default branch: refused, nothing pushed (#159)"
#[test]
fn the_default_branch_refused_nothing_pushed_159() {
    let sd = sd();
    sd.git(&["switch", "-q", "main"]);
    sd.commit("local");
    let r = sd.run(&["push"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["not pushing main: it's the default branch"]);
    assert_ne!(sd.remote("main"), sd.rev("main"));
}

// @test "the default branch is the remote's HEAD, not a guess from local refs (#159)"
#[test]
fn the_default_branch_is_the_remotes_head_not_a_guess_from_local_refs_159() {
    let sd = sd();
    sd.git(&["branch", "-q", "develop", "main"]);
    sd.git(&["push", "-q", "origin", "develop"]);
    sd.git(&["-C", &sd.path("origin.git"), "symbolic-ref", "HEAD", "refs/heads/develop"]);
    sd.git(&["remote", "set-head", "origin", "-d"]);
    sd.git(&["switch", "-q", "develop"]);
    sd.commit("dev");
    let r = sd.run(&["push"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["not pushing develop: it's the default branch"]);
    assert_ne!(sd.remote("develop"), sd.rev("develop"));
    sd.git(&["switch", "-q", "main"]);
    sd.commit("local");
    assert_eq!(sd.run(&["push"]).status, 0);
    assert_eq!(sd.remote("main"), sd.rev("main"));
}

// @test "new commits: a plain push"
#[test]
fn new_commits_a_plain_push() {
    let sd = sd();
    sd.commit("two");
    let r = sd.run(&["push"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.remote("feat"), sd.rev("feat"));
    assert_eq!(r.output, "pushed feat to origin (1 commit)");
}

// @test "not on the remote yet: pushed, with the upstream set"
#[test]
fn not_on_the_remote_yet_pushed_with_the_upstream_set() {
    let sd = sd();
    sd.git(&["switch", "-qc", "fresh"]);
    sd.commit("fresh");
    let r = sd.run(&["push"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.remote("fresh"), sd.rev("fresh"));
    assert_eq!(sd.git(&["config", "branch.fresh.remote"]), "origin");
    assert_eq!(r.output, "pushed fresh to origin (new branch)");
}

// @test "a fork's clone: pushed to the repo file's settings.remote, never to origin"
#[test]
fn a_forks_clone_pushed_to_the_repo_files_settings_remote_never_to_origin() {
    let sd = sd();
    sd.git(&["init", "-q", "--bare", &sd.path("fork.git")]);
    sd.git(&["remote", "add", "fork", &sd.path("fork.git")]);
    sd.verb_ok("config", &["set", "--repo", "settings.remote", "fork"]);
    sd.git(&["push", "-q", "fork", "main"]);
    sd.commit("more");
    let before = sd.remote("feat");
    let r = sd.run(&["push"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, "pushed feat to fork (new branch)");
    assert_eq!(sd.remote_sha("fork", "feat"), sd.rev("feat"));
    assert_eq!(sd.remote("feat"), before);
}

// @test "rewritten, and the remote head is ours: force-pushed with a lease"
#[test]
fn rewritten_and_the_remote_head_is_ours_force_pushed_with_a_lease() {
    let sd = sd();
    sd.git(&["commit", "-q", "--amend", "-m", "feat, reworded"]);
    let r = sd.run(&["push"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.remote("feat"), sd.rev("feat"));
    assert!(r.output.starts_with("force-pushed feat to origin (rewritten; the remote head "), "{}", r.output);
    assert!(r.output.ends_with(" was ours)"), "{}", r.output);
}

// @test "rewritten, with someone else's commit on the remote: refused, even with --force"
#[test]
fn rewritten_with_someone_elses_commit_on_the_remote_refused_even_with_force() {
    let sd = sd();
    sd.inherit_main();
    sd.upstream("theirs");
    let theirs = sd.remote("feat");
    sd.git(&["commit", "-q", "--amend", "-m", "feat, reworded"]);
    let r = sd.run(&["push"]);
    assert_eq!(r.status, 1);
    assert_has(&r.output, &["origin/feat has 1 commit this copy has never had; not pushing over it"]);
    assert_has(&r.output, &["git log feat..origin/feat"]);
    assert_has(&r.output, &["then rebase onto or merge origin/feat and push again"]);
    assert_eq!(sd.remote("feat"), theirs);
    assert_eq!(sd.run(&["push", "--force"]).status, 1);
    assert_eq!(sd.remote("feat"), theirs);
    assert_eq!(sd.tracks("feat"), "origin/main");
}

// @test "a pushed commit reset away locally: the remote head is ours, so it's force-pushed with a lease (#86)"
#[test]
fn a_pushed_commit_reset_away_locally_the_remote_head_is_ours_so_its_force_pushed_with_a_lease_86() {
    let sd = sd();
    sd.commit("oops");
    sd.ok(&["push"]);
    sd.git(&["reset", "-q", "--hard", "HEAD~1"]);
    let r = sd.run(&["push"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.remote("feat"), sd.rev("feat"));
    assert!(r.output.starts_with("force-pushed feat to origin (dropped 1 commit; the remote head "), "{}", r.output);
    assert!(r.output.ends_with(" was ours)"), "{}", r.output);
}

// @test "a soft reset (changes still in the worktree) is not a drop: refused, nothing pushed (#86)"
#[test]
fn a_soft_reset_changes_still_in_the_worktree_is_not_a_drop_refused_nothing_pushed_86() {
    let sd = sd();
    sd.commit("oops");
    sd.ok(&["push"]);
    let pushed = sd.remote("feat");
    sd.git(&["reset", "-q", "--soft", "HEAD~1"]);
    let r = sd.run(&["push"]);
    assert_eq!(r.status, 1);
    assert_has(&r.output, &["uncommitted or untracked changes", "commit them", "git reset --hard"]);
    assert_eq!(sd.remote("feat"), pushed);
    sd.git(&["commit", "-qm", "oops, redone"]);
    assert_eq!(sd.run(&["push"]).status, 0);
    assert_eq!(sd.remote("feat"), sd.rev("feat"));
}

// @test "a mixed reset of a commit that added a file leaves it untracked: still not a drop (#86)"
#[test]
fn a_mixed_reset_of_a_commit_that_added_a_file_leaves_it_untracked_still_not_a_drop_86() {
    let sd = sd();
    sd.write("new.txt", "new\n");
    sd.git(&["add", "new.txt"]);
    sd.git(&["commit", "-qm", "add new"]);
    sd.ok(&["push"]);
    let pushed = sd.remote("feat");
    sd.git(&["reset", "-q", "HEAD~1"]);
    assert_eq!(sd.git(&["status", "--porcelain"]), "?? new.txt");
    let r = sd.run(&["push"]);
    assert_eq!(r.status, 1);
    assert_has(&r.output, &["git clean"]);
    assert_eq!(sd.remote("feat"), pushed);
    fs::remove_file(sd.cwd.join("new.txt")).unwrap();
    let r = sd.run(&["push"]);
    assert_eq!(r.status, 0);
    assert!(r.output.starts_with("force-pushed feat to origin (dropped 1 commit;"), "{}", r.output);
}

// @test "someone else's fast-forward taken, then reset away: not ours to drop, refused (#86)"
#[test]
fn someone_elses_fast_forward_taken_then_reset_away_not_ours_to_drop_refused_86() {
    let sd = sd();
    sd.inherit_main();
    sd.upstream("theirs");
    let theirs = sd.remote("feat");
    sd.git(&["fetch", "-q", "origin"]);
    sd.git(&["merge", "-q", "--ff-only", "origin/feat"]);
    assert_eq!(sd.rev("feat"), theirs);
    sd.git(&["reset", "-q", "--hard", "ORIG_HEAD"]);
    let r = sd.run(&["push"]);
    assert_eq!(r.status, 1);
    assert_has(&r.output, &["didn't make them all", "git merge --ff-only origin/feat"]);
    assert_eq!(sd.remote("feat"), theirs);
}

// @test "someone's fast-forward taken, a commit on top, pushed, then both reset away: not ours to drop (#86)"
#[test]
fn someones_fast_forward_taken_a_commit_on_top_pushed_then_both_reset_away_not_ours_to_drop_86() {
    let sd = sd();
    sd.inherit_main();
    sd.upstream("theirs");
    sd.git(&["fetch", "-q", "origin"]);
    sd.git(&["merge", "-q", "--ff-only", "origin/feat"]);
    sd.commit("mine");
    sd.ok(&["push"]);
    let pushed = sd.remote("feat");
    sd.git(&["reset", "-q", "--hard", "HEAD~2"]);
    let r = sd.run(&["push"]);
    assert_eq!(r.status, 1);
    assert_has(&r.output, &["didn't make them all"]);
    assert_eq!(sd.remote("feat"), pushed);
}

// @test "someone's commit taken with a fast-forward rebase, then reset away: not ours to drop (#86)"
#[test]
fn someones_commit_taken_with_a_fast_forward_rebase_then_reset_away_not_ours_to_drop_86() {
    let sd = sd();
    sd.inherit_main();
    sd.upstream("theirs");
    let theirs = sd.remote("feat");
    sd.git(&["fetch", "-q", "origin"]);
    sd.git(&["rebase", "-q", "origin/feat"]);
    assert_eq!(sd.rev("feat"), theirs);
    sd.git(&["reset", "-q", "--hard", "HEAD~1"]);
    let r = sd.run(&["push"]);
    assert_eq!(r.status, 1);
    assert_has(&r.output, &["didn't make them all"]);
    assert_eq!(sd.remote("feat"), theirs);
}

// @test "someone's commit taken, then replayed by a restack and pushed, then reset away: still theirs, refused (#86)"
#[test]
fn someones_commit_taken_then_replayed_by_a_restack_and_pushed_then_reset_away_still_theirs_refused_86() {
    let sd = sd();
    sd.inherit_main();
    sd.upstream("theirs");
    sd.git(&["fetch", "-q", "origin"]);
    sd.git(&["merge", "-q", "--ff-only", "origin/feat"]);
    sd.main_moves(true, "main.txt", "m", "m", false);
    sd.git(&["fetch", "-q", "origin"]);
    sd.git(&["rebase", "-q", "origin/main"]);
    assert_eq!(sd.git(&["log", "-1", "--format=%ae"]), "them@example.com");
    assert_eq!(sd.git(&["log", "-1", "--format=%ce"]), "t@example.com");
    sd.ok(&["push"]);
    let pushed = sd.remote("feat");
    assert_eq!(pushed, sd.rev("feat"));
    sd.git(&["reset", "-q", "--hard", "HEAD~1"]);
    let r = sd.run(&["push"]);
    assert_eq!(r.status, 1);
    assert_has(&r.output, &["didn't make them all"]);
    assert_eq!(sd.remote("feat"), pushed);
}

// @test "a branch checked out nowhere has no worktree to be dirty: its own pushed commit is dropped (#86)"
#[test]
fn a_branch_checked_out_nowhere_has_no_worktree_to_be_dirty_its_own_pushed_commit_is_dropped_86() {
    let sd = sd();
    sd.commit("oops");
    sd.ok(&["push"]);
    sd.git(&["switch", "-q", "main"]);
    sd.git(&["branch", "-qf", "feat", "feat~1"]);
    let r = sd.run(&["push", "feat"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.remote("feat"), sd.rev("feat"));
    assert!(r.output.starts_with("force-pushed feat to origin (dropped 1 commit;"), "{}", r.output);
}

// @test "a restacked branch's own commit, pushed then reset away, was made here: dropped (#86)"
#[test]
fn a_restacked_branchs_own_commit_pushed_then_reset_away_was_made_here_dropped_86() {
    let sd = sd();
    sd.main_moves(false, "main.txt", "m", "m", false);
    sd.git(&["fetch", "-q", "origin"]);
    sd.git(&["rebase", "-q", "origin/main"]);
    sd.commit("oops");
    sd.ok(&["push"]);
    sd.git(&["reset", "-q", "--hard", "HEAD~1"]);
    let r = sd.run(&["push"]);
    assert_eq!(r.status, 0);
    assert!(r.output.starts_with("force-pushed feat to origin (dropped 1 commit;"), "{}", r.output);
}

// @test "a restack past a colleague's squash-merged commit drops it from the branch: pushed with a lease (#86)"
#[test]
fn a_restack_past_a_colleagues_squash_merged_commit_drops_it_from_the_branch_pushed_with_a_lease_86() {
    let sd = sd();
    sd.inherit_main();
    sd.upstream("theirs");
    sd.git(&["fetch", "-q", "origin"]);
    sd.git(&["merge", "-q", "--ff-only", "origin/feat"]);
    sd.commit("mine");
    sd.ok(&["push"]);
    sd.main_moves(true, "other.txt", "theirs", "theirs (#1)", true);
    sd.git(&["fetch", "-q", "origin"]);
    sd.git(&["rebase", "-q", "origin/main"]);
    assert_eq!(sd.git(&["log", "--format=%s", "origin/main..feat"]), "mine\nfeat");
    let r = sd.run(&["push"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.remote("feat"), sd.rev("feat"));
    assert!(r.output.starts_with("force-pushed feat to origin (rewritten;"), "{}", r.output);
}

// @test "taken, reset away, then a new commit: the rewrite path leases over the taken commit, as before #86 (only a pure drop checks who made it)"
#[test]
fn taken_reset_away_then_a_new_commit_the_rewrite_path_leases_over_the_taken_commit_as_before_86_only_a_pure_drop_checks_who_made_it() {
    let sd = sd();
    sd.inherit_main();
    sd.upstream("theirs");
    sd.git(&["fetch", "-q", "origin"]);
    sd.git(&["merge", "-q", "--ff-only", "origin/feat"]);
    sd.git(&["reset", "-q", "--hard", "HEAD~1"]);
    sd.commit("mine");
    let r = sd.run(&["push"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.remote("feat"), sd.rev("feat"));
    assert!(r.output.starts_with("force-pushed feat to origin (rewritten;"), "{}", r.output);
}

// @test "the remote is only ahead: refused, says how to take its commits"
#[test]
fn the_remote_is_only_ahead_refused_says_how_to_take_its_commits() {
    let sd = sd();
    sd.inherit_main();
    sd.upstream("theirs");
    let r = sd.run(&["push"]);
    assert_eq!(r.status, 1);
    assert_has(&r.output, &["origin/feat has 1 commit you don't"]);
    assert_has(&r.output, &["git merge --ff-only origin/feat"]);
    assert_eq!(sd.tracks("feat"), "origin/main");
}

// @test "the lease holds when the remote moves after the fetch"
#[test]
fn the_lease_holds_when_the_remote_moves_after_the_fetch() {
    let sd = sd();
    // The hook runs after sd act has fetched and chosen its lease, before the
    // push lands: someone else's push sneaks in between.
    let hook = format!(
        "#!/usr/bin/env bash\nunset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE\n\
         cd '{}' && git fetch -q origin && git checkout -q -B feat origin/feat \\\n  \
         && echo sneaky > sneaky.txt && git add sneaky.txt && git commit -qm sneaky && git push -q origin feat 2>/dev/null\n",
        sd.path("other")
    );
    let hooks = sd.git(&["rev-parse", "--git-path", "hooks"]);
    sd.script(&sd.cwd.join(hooks).join("pre-push"), &hook);
    sd.inherit_main();
    sd.git(&["commit", "-q", "--amend", "-m", "feat, reworded"]);
    let r = sd.run(&["push"]);
    assert_eq!(r.status, 1);
    assert_has(&r.output, &["origin/feat moved while pushing; nothing pushed"]);
    assert_eq!(sd.git_in("other", &["log", "-1", "--format=%s", "origin/feat"]), "sneaky");
    assert_eq!(sd.tracks("feat"), "origin/main");
}

// A restack, then git's advice after a rejected plain push (pull --rebase onto
// the old remote head) replays main's commit as a copy: pushing would publish
// it (#47, #348). --force, given for the lock or a red verify, doesn't.
#[test]
fn a_restack_undone_by_a_plain_pull_refused_even_with_force_nothing_pushed_47() {
    let sd = sd();
    let pre = sd.rev("feat");
    sd.main_moves(true, "m.txt", "m", "main moves", false);
    sd.git(&["fetch", "-q"]);
    sd.git(&["rebase", "-q", "--onto", "origin/main", "main", "feat"]);
    sd.git(&["update-ref", "refs/sheepdog/feat/base", "origin/main"]);
    sd.git(&["reset", "-q", "--hard", &pre]);
    sd.git(&["cherry-pick", "origin/main"]);
    for args in [&["push"][..], &["push", "--force"]] {
        let r = sd.run(args);
        assert_eq!(r.status, 1);
        assert_has(&r.output, &["not pushing: the restack was undone", "git push by hand"]);
        assert_eq!(sd.remote("feat"), pre);
    }
}

// sd act verify's record of a failed verify at HEAD (#325).
#[test]
fn verify_red_at_head_refused_unless_force_nothing_pushed_348() {
    let sd = sd();
    sd.commit("local");
    let head = sd.rev("HEAD");
    let rec = format!(r#"{{"head": "{head}", "step": "implement", "cmd": "make test", "at": "2026-10-29T12:00:00Z"}}"#);
    let blob = sd.git_with(&[], Some(&rec), &["hash-object", "-w", "--stdin"]);
    sd.git(&["update-ref", "refs/sheepdog/feat/verify-red", blob.trim()]);
    let r = sd.run(&["push"]);
    assert_eq!(r.status, 1);
    assert_has(&r.output, &["not pushing feat:", "verify is red at HEAD", "--force"]);
    assert_ne!(sd.remote("feat"), head);
    assert_eq!(sd.run(&["push", "--force"]).status, 0);
    assert_eq!(sd.remote("feat"), head);
}

// gh fails here, so sd state answers from a fixture with the PR closed.
#[test]
fn a_closed_or_merged_pr_refused_unless_force_nothing_pushed_348() {
    for state in ["CLOSED", "MERGED"] {
        let mut sd = sd();
        sd.env = vec![("SHEEPDOG_STATE_FIXTURES".to_string(), sd.path("states"))];
        fs::create_dir_all(sd.tmp.join("states")).unwrap();
        fs::write(sd.tmp.join("states/feat.json"), format!(r#"{{"pr": {{"number": 7, "state": "{state}"}}}}"#)).unwrap();
        sd.commit("local");
        let r = sd.run(&["push"]);
        assert_eq!(r.status, 1, "{state}");
        assert_has(&r.output, &["not pushing feat:", "the PR is merged or closed", "--force"]);
        assert_ne!(sd.remote("feat"), sd.rev("feat"));
        assert_eq!(sd.run(&["push", "--force"]).status, 0, "{state}");
        assert_eq!(sd.remote("feat"), sd.rev("feat"));
    }
}

// @test "taken over: refused unless --force"
#[test]
fn taken_over_refused_unless_force() {
    let sd = sd();
    sd.commit("two");
    sd.git(&["config", "branch.feat.sd-lock", "chrismo"]);
    let r = sd.run(&["push"]);
    assert_eq!(r.status, 1);
    assert_has(&r.output, &["taken over"]);
    assert_eq!(sd.run(&["push", "--force"]).status, 0);
    assert_eq!(sd.remote("feat"), sd.rev("feat"));
}

// @test "a named branch is pushed from wherever you are"
#[test]
fn a_named_branch_is_pushed_from_wherever_you_are() {
    let sd = sd();
    sd.inherit_main();
    sd.commit("two");
    sd.git(&["switch", "-q", "main"]);
    assert_eq!(sd.run(&["push", "feat"]).status, 0);
    assert_eq!(sd.remote("feat"), sd.rev("feat"));
    assert_eq!(sd.tracks("feat"), "origin/feat");
    assert_eq!(sd.tracks("main"), "origin/main");
}

// @test "the remote comes from settings.remote"
#[test]
fn the_remote_comes_from_settings_remote() {
    let sd = sd();
    sd.git(&["remote", "rename", "origin", "up"]);
    sd.commit("two");
    sd.write_global(r#"{"settings":{"remote":"up"}}"#);
    sd.git(&["branch", "-q", "--set-upstream-to=up/main", "feat"]);
    let r = sd.run(&["push"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, "pushed feat to up (1 commit)");
    assert_eq!(sd.tracks("feat"), "up/feat");
}

// ── the upstream (#39) ───────────────────────────────────────────────────────
// A branch cut from origin/main tracks it; a successful push makes it track origin/<branch>.

// @test "upstream: a fast-forward onto a branch tracking origin/main: now tracks origin/feat"
#[test]
fn upstream_a_fast_forward_onto_a_branch_tracking_origin_main_now_tracks_origin_feat() {
    let sd = sd();
    sd.inherit_main();
    sd.commit("two");
    let r = sd.run(&["push"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, "pushed feat to origin (1 commit)");
    assert_eq!(sd.tracks("feat"), "origin/feat");
}

// @test "upstream: rewritten with a lease, tracking origin/main: now tracks origin/feat"
#[test]
fn upstream_rewritten_with_a_lease_tracking_origin_main_now_tracks_origin_feat() {
    let sd = sd();
    sd.inherit_main();
    sd.git(&["commit", "-q", "--amend", "-m", "feat, reworded"]);
    assert_eq!(sd.run(&["push"]).status, 0);
    assert_eq!(sd.remote("feat"), sd.rev("feat"));
    assert_eq!(sd.tracks("feat"), "origin/feat");
}

// @test "upstream: nothing to push, tracking origin/main: now tracks origin/feat, HEAD untouched"
#[test]
fn upstream_nothing_to_push_tracking_origin_main_now_tracks_origin_feat_head_untouched() {
    let sd = sd();
    sd.inherit_main();
    let head = sd.rev("feat");
    let r = sd.run(&["push"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, "nothing to push on feat");
    assert_eq!(sd.tracks("feat"), "origin/feat");
    assert_eq!(sd.rev("feat"), head);
    assert_eq!(sd.remote("feat"), head);
}

// @test "upstream: on the remote with no upstream at all: now tracks origin/feat"
#[test]
fn upstream_on_the_remote_with_no_upstream_at_all_now_tracks_origin_feat() {
    let sd = sd();
    sd.git(&["branch", "-q", "--unset-upstream", "feat"]);
    sd.commit("two");
    assert_eq!(sd.run(&["push"]).status, 0);
    assert_eq!(sd.tracks("feat"), "origin/feat");
}

// @test "upstream: nothing to push and no upstream: now tracks origin/feat"
#[test]
fn upstream_nothing_to_push_and_no_upstream_now_tracks_origin_feat() {
    let sd = sd();
    sd.git(&["branch", "-q", "--unset-upstream", "feat"]);
    assert_eq!(sd.run(&["push"]).status, 0);
    assert_eq!(sd.tracks("feat"), "origin/feat");
}

// @test "upstream: can't be set: pushed anyway, with git's error and how to set it"
#[test]
fn upstream_cant_be_set_pushed_anyway_with_gits_error_and_how_to_set_it() {
    let sd = sd();
    sd.inherit_main();
    sd.commit("two");
    sd.write(".git/config.lock", "");
    let r = sd.run_sep(&["push"]);
    fs::remove_file(sd.cwd.join(".git/config.lock")).unwrap();
    assert_eq!(r.status, 0);
    assert_eq!(r.output, "pushed feat to origin (1 commit)");
    assert_eq!(sd.remote("feat"), sd.rev("feat"));
    assert_has(&r.stderr, &["could not lock config file"]);
    let warning = "warning: feat is pushed, but its upstream isn't origin/feat: git branch --set-upstream-to=origin/feat feat";
    assert!(r.stderr.ends_with(warning), "{}", r.stderr);
}

// @test "upstream: pushing a named branch from another worktree leaves that worktree's branch alone"
#[test]
fn upstream_pushing_a_named_branch_from_another_worktree_leaves_that_worktrees_branch_alone() {
    let mut sd = sd();
    sd.inherit_main();
    sd.commit("two");
    sd.git(&["worktree", "add", "-q", "-b", "side", &sd.path("side"), "origin/main"]);
    assert_eq!(sd.tracks("side"), "origin/main");
    sd.cd("side");
    assert_eq!(sd.run(&["push", "feat"]).status, 0);
    assert_eq!(sd.tracks("feat"), "origin/feat");
    assert_eq!(sd.tracks("side"), "origin/main");
}

// @test "upstream: the sd parent is not the upstream"
#[test]
fn upstream_the_sd_parent_is_not_the_upstream() {
    let sd = sd();
    sd.inherit_main();
    sd.git(&["config", "branch.feat.sd-parent", "main"]);
    sd.commit("two");
    assert_eq!(sd.run(&["push"]).status, 0);
    assert_eq!(sd.tracks("feat"), "origin/feat");
    assert_eq!(sd.git(&["config", "branch.feat.sd-parent"]), "main");
}

// @test "a live step holds the running marker: refused, --force or not, nothing pushed (#270)"
#[test]
fn a_live_step_holds_the_running_marker_refused_force_or_not_nothing_pushed_270() {
    let sd = sd();
    sd.commit("two");
    let before = sd.remote("feat");
    let holder = Holder::new();
    let marker = running_marker(&sd.tmp.join("repo/.git"), "feat", "address", holder.pid());
    for args in [&["push"][..], &["push", "--force"]] {
        let r = sd.run(args);
        assert_ne!(r.status, 0, "{args:?}");
        assert_has(&r.output, &[&step_refusal("address", "feat", holder.pid(), "pushing")]);
    }
    // Another branch's step holding its own marker is no pass here.
    assert_ne!(sd.run_env(&[("RUNNING_HELD", "main")], &["push"]).status, 0);
    assert_eq!(sd.remote("feat"), before);
    assert!(marker.exists());
}

// @test "the step's own child (RUNNING_HELD=<b>) pushes under its marker, and a dead holder's marker doesn't refuse (#270)"
#[test]
fn the_steps_own_child_pushes_under_its_marker_and_a_dead_holders_marker_doesnt_refuse_270() {
    let sd = sd();
    let holder = Holder::new();
    running_marker(&sd.tmp.join("repo/.git"), "feat", "push", holder.pid());
    sd.commit("two");
    let r = sd.run_env(&[("RUNNING_HELD", "feat")], &["push"]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_eq!(sd.remote("feat"), sd.rev("feat"));

    running_marker(&sd.tmp.join("repo/.git"), "feat", "address", dead_pid());
    sd.commit("three");
    let r = sd.run(&["push"]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_eq!(sd.remote("feat"), sd.rev("feat"));
}

// @test "push by hand holds the running marker, naming push, while git pushes; removed after (#270)"
#[test]
fn push_by_hand_holds_the_running_marker_naming_push_while_git_pushes_removed_after_270() {
    let sd = sd();
    sd.commit("more");
    let hook = sd.tmp.join("repo/.git/hooks/pre-push");
    fs::create_dir_all(hook.parent().unwrap()).unwrap();
    fs::write(&hook, "#!/bin/sh\ncp \"$(git rev-parse --git-common-dir)/sheepdog/feat.running\" ../push-saw\n").unwrap();
    std::process::Command::new("chmod").args(["+x", hook.to_str().unwrap()]).status().unwrap();
    let r = sd.run(&["push"]);
    assert_eq!(r.status, 0, "{}", r.output);
    let line = fs::read_to_string(sd.tmp.join("push-saw")).unwrap_or_default();
    assert_eq!(line.split_whitespace().nth(1), Some("push"), "{line}");
    assert!(!sd.tmp.join("repo/.git/sheepdog/feat.running").exists());
}
