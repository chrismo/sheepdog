// Black-box tests of sd act enroll, unenroll, forget and unregister (#202):
// the built binary, run as sd-act through a link (tests/common). Moved from
// libexec/sd-act.bats; each test keeps the bats test's title above it. The
// dispatcher's tests (--help, no verb, an unknown verb, and "--remove is for
// sd act forget only") are in tests/act.rs.

mod common;

use common::{assert_has, assert_lacks, spawn, Sd};
use sheepdog::running::proc_start;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::process::Command;

/// The bats setup: repo (cwd) with one commit on main, feat checked out.
fn sd() -> Sd {
    let mut sd = Sd::for_verb("act");
    sd.git(&["init", "-q", &sd.path("repo")]);
    sd.cd("repo");
    sd.git(&["commit", "-q", "--allow-empty", "-m", "init"]);
    sd.git(&["switch", "-qc", "feat"]);
    sd
}

const NO_PRS: &str = r#"#!/usr/bin/env bash
echo '{"data":{"viewer":{"login":"me"},"repository":{"defaultBranchRef":{"name":"main"},"pullRequests":{"nodes":[]}}}}'
"#;

/// A gh that answers with <pr> for both of sd state's queries: discovery's
/// list and the fetch by number.
fn gh_answering(pr: &str) -> String {
    format!(
        "#!/usr/bin/env bash\ncat <<'JSON'\n\
         {{\"data\":{{\"viewer\":{{\"login\":\"me\"}},\"repository\":{{\"defaultBranchRef\":{{\"name\":\"main\"}},\"pullRequests\":{{\"nodes\":[{pr}]}},\"pullRequest\":{pr}}}}}}}\n\
         JSON\n"
    )
}

impl Sd {
    /// The watcher's list: git config --global --get-all sheepdog.repo.
    fn repos(&self) -> String {
        self.git_try(&["config", "--global", "--get-all", "sheepdog.repo"]).unwrap_or_default()
    }

    /// git config <key>'s value, or "" when unset.
    fn config(&self, key: &str) -> String {
        self.git_try(&["config", key]).unwrap_or_default()
    }

    fn rev(&self, r: &str) -> String {
        self.git(&["rev-parse", r])
    }

    fn has_ref(&self, r: &str) -> bool {
        self.git_status(&["rev-parse", "-q", "--verify", r]) == 0
    }

    fn pins(&self, b: &str) -> String {
        self.git(&["for-each-ref", &format!("refs/sheepdog/{b}/")])
    }

    /// no_prs: an origin with main on it and a gh with no PRs.
    fn no_prs(&self) {
        self.git(&["init", "-q", "--bare", &self.path("origin.git")]);
        self.git(&["remote", "add", "origin", &self.path("origin.git")]);
        self.git(&["push", "-q", "origin", "HEAD:main"]);
        self.git(&["fetch", "-q", "origin"]);
        self.git(&["remote", "set-head", "origin", "main"]);
        self.fake("gh", NO_PRS);
    }

    /// main, then par one commit past it, then kid one past par.
    fn par_and_kid(&self) {
        self.git(&["switch", "-q", "main"]);
        self.no_prs();
        self.git(&["switch", "-qc", "par"]);
        self.git(&["commit", "-q", "--allow-empty", "-m", "p1"]);
        self.git(&["switch", "-qc", "kid"]);
        self.git(&["commit", "-q", "--allow-empty", "-m", "k1"]);
    }

    /// merged_pr <state> <head>: gh answers that feat's PR is <state> at <head>.
    fn merged_pr(&self, state: &str, head: &str) {
        let pr = format!(
            r#"{{"number":3,"url":"u","state":"{state}","isDraft":false,"baseRefName":"main","headRefName":"feat","headRefOid":"{head}",
  "mergeable":"UNKNOWN","mergeStateStatus":"UNKNOWN","reviewDecision":null,"mergedAt":null}}"#
        );
        self.fake("gh", &gh_answering(&pr));
    }

    /// merged_wt: feat in its own worktree at wt/, one commit past main, its
    /// PR merged at that tip and the remote branch deleted (a squash merge).
    fn merged_wt(&mut self) {
        self.git(&["switch", "-q", "main"]);
        self.no_prs();
        self.git(&["worktree", "add", "-q", &self.path("wt"), "feat"]);
        self.git_in("wt", &["commit", "-q", "--allow-empty", "-m", "f1"]);
        self.merged_pr("MERGED", &self.rev("feat"));
        self.ok(&["enroll", "feat"]);
        self.git(&["update-ref", "refs/sheepdog/feat/local", "feat"]);
        self.cd("wt");
    }

    /// From wt/ back in repo/, wt/ removed and feat checked out there.
    fn feat_in_main_worktree(&mut self) {
        self.cd("repo");
        self.git(&["worktree", "remove", &self.path("wt")]);
        self.git(&["switch", "-q", "feat"]);
    }
}

// @test "enroll: marks the current branch and registers the repo"
#[test]
fn enroll_marks_the_current_branch_and_registers_the_repo() {
    let sd = sd();
    let r = sd.run(&["enroll"]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &["enrolled feat"]);
    assert_eq!(sd.config("branch.feat.sheepdog"), "true");
    assert_eq!(sd.repos(), sd.path("repo"));
}

// @test "enroll: a named branch"
#[test]
fn enroll_a_named_branch() {
    let sd = sd();
    sd.git(&["branch", "other"]);
    assert_eq!(sd.run(&["enroll", "other"]).status, 0);
    assert_eq!(sd.config("branch.other.sheepdog"), "true");
    assert_eq!(sd.config("branch.feat.sheepdog"), "");
}

// @test "enroll: a branch made from another's tip, with no PR, records it as the parent (#94)"
#[test]
fn enroll_a_branch_made_from_anothers_tip_with_no_pr_records_it_as_the_parent_94() {
    let sd = sd();
    sd.par_and_kid();
    let r = sd.run(&["enroll", "kid"]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &["parent: par"]);
    assert_eq!(sd.config("branch.kid.sd-parent"), "par");
    assert_eq!(sd.rev("refs/sheepdog/kid/base"), sd.rev("par"));
}

// @test "enroll: a lineage write git refuses is reported, and enroll still succeeds"
#[test]
fn enroll_a_lineage_write_git_refuses_is_reported_and_enroll_still_succeeds() {
    let sd = sd();
    sd.par_and_kid();
    sd.git(&["update-ref", "refs/sheepdog/kid", "HEAD"]); // refs/sheepdog/kid/base can't be made
    let r = sd.run(&["enroll", "kid"]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &["enrolled kid", "parent: par", "refs/sheepdog/kid"]);
    assert_eq!(sd.config("branch.kid.sd-parent"), "par");
}

// @test "enroll: two candidate parents record neither, and say so"
#[test]
fn enroll_two_candidate_parents_record_neither_and_say_so() {
    let sd = sd();
    sd.git(&["switch", "-q", "main"]);
    sd.no_prs();
    sd.git(&["switch", "-qc", "par"]);
    sd.git(&["commit", "-q", "--allow-empty", "-m", "p1"]);
    sd.git(&["branch", "twin"]);
    sd.git(&["switch", "-qc", "kid"]);
    sd.git(&["commit", "-q", "--allow-empty", "-m", "k1"]);
    let r = sd.run(&["enroll", "kid"]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &["kid looks stacked on par or twin", "git config branch.kid.sd-parent"]);
    assert_eq!(sd.config("branch.kid.sd-parent"), "");
}

// @test "enroll: a parent that moved on since: the child records nothing and says so, the parent isn't asked"
#[test]
fn enroll_a_parent_that_moved_on_since_the_child_records_nothing_and_says_so_the_parent_isnt_asked() {
    let sd = sd();
    sd.par_and_kid();
    sd.git(&["switch", "-q", "par"]);
    sd.git(&["commit", "-q", "--allow-empty", "-m", "p2"]);
    let r = sd.run(&["enroll", "kid"]);
    assert_eq!(r.status, 0);
    assert_has(
        &r.output,
        &["kid shares commits with par", "git config branch.kid.sd-parent par", "git config branch.kid.sd-parent main"],
    );
    assert_eq!(sd.config("branch.kid.sd-parent"), "");
    let r = sd.run(&["enroll", "par"]);
    assert_eq!(r.status, 0);
    assert_lacks(&r.output, "shares commits");
    assert_eq!(sd.config("branch.par.sd-parent"), "");
}

// @test "enroll: a PR on the default branch, or gh failing, records no git-inferred parent"
#[test]
fn enroll_a_pr_on_the_default_branch_or_gh_failing_records_no_git_inferred_parent() {
    let sd = sd();
    sd.par_and_kid();
    sd.fake("gh", "#!/usr/bin/env bash\necho \"gh: API rate limit exceeded\" >&2; exit 1\n");
    assert_eq!(sd.run(&["enroll", "kid"]).status, 0);
    assert_eq!(sd.config("branch.kid.sd-parent"), "");
    // One answer for both of sd state's queries: discovery's list and the fetch by number.
    let pr = format!(
        r#"{{"number":3,"url":"u","state":"OPEN","isDraft":false,"baseRefName":"main","headRefName":"kid","headRefOid":"{}",
  "mergeable":"MERGEABLE","mergeStateStatus":"CLEAN","reviewDecision":null,"mergedAt":null}}"#,
        sd.rev("kid")
    );
    sd.fake("gh", &gh_answering(&pr));
    assert_eq!(sd.run(&["enroll", "kid"]).status, 0);
    assert_eq!(sd.config("branch.kid.sd-parent"), "");
}

// @test "enroll: a branch made from main alone records no parent"
#[test]
fn enroll_a_branch_made_from_main_alone_records_no_parent() {
    let sd = sd();
    sd.git(&["switch", "-q", "main"]);
    sd.no_prs();
    sd.git(&["switch", "-qc", "solo"]);
    sd.git(&["commit", "-q", "--allow-empty", "-m", "s1"]);
    assert_eq!(sd.run(&["enroll", "solo"]).status, 0);
    assert_eq!(sd.config("branch.solo.sd-parent"), "");
}

// @test "enroll: a missing branch fails"
#[test]
fn enroll_a_missing_branch_fails() {
    let sd = sd();
    let r = sd.run(&["enroll", "nope"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["no such branch"]);
}

// @test "enroll: registers the repo only once"
#[test]
fn enroll_registers_the_repo_only_once() {
    let sd = sd();
    sd.ok(&["enroll"]);
    sd.git(&["branch", "other"]);
    sd.ok(&["enroll", "other"]);
    assert_eq!(sd.repos().lines().count(), 1);
}

// @test "enroll: from a linked worktree, registers the main worktree"
#[test]
fn enroll_from_a_linked_worktree_registers_the_main_worktree() {
    let mut sd = sd();
    sd.git(&["switch", "-q", "main"]);
    sd.git(&["worktree", "add", "-q", &sd.path("wt"), "feat"]);
    sd.cd("wt");
    assert_eq!(sd.run(&["enroll"]).status, 0);
    assert_eq!(sd.repos(), sd.path("repo"));
}

// @test "unenroll: clears the mark"
#[test]
fn unenroll_clears_the_mark() {
    let sd = sd();
    sd.ok(&["enroll"]);
    assert_eq!(sd.run(&["unenroll"]).status, 0);
    assert_eq!(sd.config("branch.feat.sheepdog"), "");
}

// @test "unenroll: a branch that was never enrolled is fine"
#[test]
fn unenroll_a_branch_that_was_never_enrolled_is_fine() {
    let sd = sd();
    assert_eq!(sd.run(&["unenroll"]).status, 0);
}

// @test "forget: unenrolls and deletes refs/sheepdog/<branch>/*"
#[test]
fn forget_unenrolls_and_deletes_refs_sheepdog_branch() {
    let sd = sd();
    sd.ok(&["enroll"]);
    sd.git(&["update-ref", "refs/sheepdog/feat/base", "HEAD"]);
    sd.git(&["update-ref", "refs/sheepdog/feat/local", "HEAD"]);
    assert_eq!(sd.run(&["forget"]).status, 0);
    assert_eq!(sd.config("branch.feat.sheepdog"), "");
    assert_eq!(sd.pins("feat"), "");
}

// @test "forget: a named branch; other branches' pins stay"
#[test]
fn forget_a_named_branch_other_branches_pins_stay() {
    let sd = sd();
    sd.git(&["switch", "-qc", "other"]);
    sd.git(&["update-ref", "refs/sheepdog/feat/base", "HEAD"]);
    sd.git(&["update-ref", "refs/sheepdog/other/base", "HEAD"]);
    assert_eq!(sd.run(&["forget", "feat"]).status, 0);
    assert_eq!(sd.pins("feat"), "");
    assert_ne!(sd.pins("other"), "");
}

// @test "forget: deletes the branch's stored description too (#153)"
#[test]
fn forget_deletes_the_branchs_stored_description_too_153() {
    let sd = sd();
    fs::create_dir_all(sd.cwd.join(".git/sheepdog")).unwrap();
    sd.write(".git/sheepdog/feat.describe.json", "{}\n");
    sd.write(".git/sheepdog/other.describe.json", "{}\n");
    assert_eq!(sd.run(&["forget"]).status, 0);
    assert!(!sd.cwd.join(".git/sheepdog/feat.describe.json").exists());
    assert!(sd.cwd.join(".git/sheepdog/other.describe.json").exists());
}

// @test "forget --remove: a merged branch's worktree is removed and the branch deleted (#153)"
#[test]
fn forget_remove_a_merged_branchs_worktree_is_removed_and_the_branch_deleted_153() {
    let mut sd = sd();
    sd.merged_wt();
    let wt = sd.path("wt");
    let r = sd.run(&["forget", "--remove"]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &["forgot feat"]);
    assert_has(&r.output, &[&format!("removed {wt}")]);
    assert_has(&r.output, &["deleted feat"]);
    sd.cd("repo");
    assert!(!sd.tmp.join("wt").is_dir());
    assert_lacks(&sd.git(&["worktree", "list", "--porcelain"]), &wt);
    assert!(!sd.has_ref("refs/heads/feat"));
    assert_eq!(sd.pins("feat"), "");
    assert_eq!(sd.config("branch.feat.sheepdog"), "");
}

// @test "forget --remove: the deleted branch's config file goes too (#283)"
#[test]
fn forget_remove_the_deleted_branchs_config_file_goes_too_283() {
    let mut sd = sd();
    sd.merged_wt();
    let dir = sd.tmp.join("repo/.git/sheepdog");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("feat.json"), "{}\n").unwrap();
    assert_eq!(sd.run(&["forget", "--remove"]).status, 0);
    assert!(!dir.join("feat.json").exists());
}

// @test "forget: a branch that still exists keeps its config file (#283)"
#[test]
fn forget_a_branch_that_still_exists_keeps_its_config_file_283() {
    let sd = sd();
    fs::create_dir_all(sd.cwd.join(".git/sheepdog")).unwrap();
    sd.write(".git/sheepdog/feat.json", "{}\n");
    assert_eq!(sd.run(&["forget"]).status, 0);
    assert!(sd.cwd.join(".git/sheepdog/feat.json").exists());
}

// @test "unenroll: a branch that's gone loses its config and describe files (#283)"
#[test]
fn unenroll_a_branch_thats_gone_loses_its_config_and_describe_files_283() {
    let sd = sd();
    fs::create_dir_all(sd.cwd.join(".git/sheepdog")).unwrap();
    for f in ["gone.json", "gone.describe.json", "feat.json", "feat.describe.json"] {
        sd.write(&format!(".git/sheepdog/{f}"), "{}\n");
    }
    assert_eq!(sd.run(&["unenroll", "gone"]).status, 0);
    assert!(!sd.cwd.join(".git/sheepdog/gone.json").exists());
    assert!(!sd.cwd.join(".git/sheepdog/gone.describe.json").exists());
    assert_eq!(sd.run(&["unenroll"]).status, 0);
    assert!(sd.cwd.join(".git/sheepdog/feat.json").exists());
    assert!(sd.cwd.join(".git/sheepdog/feat.describe.json").exists());
}

// @test "forget --remove: a merged branch checked out nowhere is deleted"
#[test]
fn forget_remove_a_merged_branch_checked_out_nowhere_is_deleted() {
    let mut sd = sd();
    sd.merged_wt();
    sd.cd("repo");
    sd.git(&["worktree", "remove", &sd.path("wt")]);
    assert_eq!(sd.run(&["forget", "--remove", "feat"]).status, 0);
    assert!(!sd.has_ref("refs/heads/feat"));
}

// @test "forget --remove: names the ignored files that go with the worktree"
#[test]
fn forget_remove_names_the_ignored_files_that_go_with_the_worktree() {
    let mut sd = sd();
    sd.merged_wt();
    fs::create_dir_all(sd.tmp.join("repo/.git/info")).unwrap();
    let mut exclude = OpenOptions::new().create(true).append(true).open(sd.tmp.join("repo/.git/info/exclude")).unwrap();
    write!(exclude, ".env\nbuild/\n").unwrap();
    sd.write(".env", "secret\n");
    fs::create_dir(sd.cwd.join("build")).unwrap();
    sd.write("build/out", "o\n");
    let r = sd.run(&["forget", "--remove"]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &[&format!("removed {} with its ignored files: .env build/", sd.path("wt"))]);
    assert!(!sd.tmp.join("wt").is_dir());
}

// @test "forget --remove: a branch already deleted is just forgotten"
#[test]
fn forget_remove_a_branch_already_deleted_is_just_forgotten() {
    let sd = sd();
    sd.git(&["update-ref", "refs/sheepdog/gone/local", "HEAD"]);
    let r = sd.run(&["forget", "--remove", "gone"]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &["forgot gone"]);
    assert_eq!(sd.pins("gone"), "");
}

// @test "forget --remove: a worktree git won't remove changes nothing"
#[test]
fn forget_remove_a_worktree_git_wont_remove_changes_nothing() {
    let mut sd = sd();
    sd.merged_wt();
    sd.git_in("repo", &["worktree", "lock", &sd.path("wt")]);
    assert_ne!(sd.run(&["forget", "--remove"]).status, 0);
    assert!(sd.tmp.join("wt").is_dir());
    assert!(sd.has_ref("refs/heads/feat"));
    assert_ne!(sd.pins("feat"), "");
    assert_eq!(sd.config("branch.feat.sheepdog"), "true");
}

// @test "forget --remove: untracked files count even when status hides them"
#[test]
fn forget_remove_untracked_files_count_even_when_status_hides_them() {
    let mut sd = sd();
    sd.merged_wt();
    sd.git(&["config", "status.showUntrackedFiles", "no"]);
    sd.write("wip.txt", "wip\n");
    let r = sd.run(&["forget", "--remove"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["uncommitted changes"]);
    assert!(sd.tmp.join("wt").is_dir());
}

// @test "forget --remove: refuses with uncommitted changes, and changes nothing"
#[test]
fn forget_remove_refuses_with_uncommitted_changes_and_changes_nothing() {
    let mut sd = sd();
    sd.merged_wt();
    sd.write("wip.txt", "wip\n");
    let r = sd.run(&["forget", "--remove"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["uncommitted changes"]);
    assert!(sd.tmp.join("wt").is_dir());
    assert!(sd.has_ref("refs/heads/feat"));
    assert_ne!(sd.pins("feat"), "");
    assert_eq!(sd.config("branch.feat.sheepdog"), "true");
}

/// feat's running marker, naming the step pid <holder> runs; its path.
fn marker(sd: &Sd, holder: u32) -> String {
    let marker = sd.path("repo/.git/sheepdog/feat.running");
    fs::create_dir_all(sd.tmp.join("repo/.git/sheepdog")).unwrap();
    let pid = holder.to_string();
    fs::write(&marker, format!("{pid} address 2026-10-18T14:03:00Z {}\n", proc_start(&pid))).unwrap();
    marker
}

// @test "forget --remove: refuses while a live agent step holds the running marker, and changes nothing (#270)"
#[test]
fn forget_remove_refuses_while_a_live_step_holds_the_running_marker_and_changes_nothing_270() {
    let mut sd = sd();
    sd.merged_wt();
    let mut holder = spawn(Command::new("sleep").arg("30"));
    let pid = holder.id();
    let marker = marker(&sd, pid);
    let r = sd.run(&["forget", "--remove"]);
    holder.kill().unwrap();
    holder.wait().unwrap();
    assert_ne!(r.status, 0);
    assert_has(
        &r.output,
        &[&format!(
            "address is running on feat (pid {pid}, since 2026-10-18T14:03:00Z): not removing its worktree. \
             Wait for it to end (sd state shows it), or kill {pid}"
        )],
    );
    assert!(sd.tmp.join("wt").is_dir());
    assert!(sd.has_ref("refs/heads/feat"));
    assert_ne!(sd.pins("feat"), "");
    assert_eq!(sd.config("branch.feat.sheepdog"), "true");
    assert!(Path::new(&marker).exists());
}

// @test "forget --remove: a dead step's marker doesn't refuse (#270)"
#[test]
fn forget_remove_a_dead_steps_marker_doesnt_refuse_270() {
    let mut sd = sd();
    sd.merged_wt();
    let mut dead = spawn(&mut Command::new("true"));
    dead.wait().unwrap();
    marker(&sd, dead.id());
    let r = sd.run(&["forget", "--remove"]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_has(&r.output, &["deleted feat"]);
    assert!(!sd.tmp.join("wt").is_dir());
}

// @test "forget --remove: refuses when the branch has commits the merged PR doesn't"
#[test]
fn forget_remove_refuses_when_the_branch_has_commits_the_merged_pr_doesnt() {
    let mut sd = sd();
    sd.merged_wt();
    sd.git(&["commit", "-q", "--allow-empty", "-m", "after-merge"]);
    let r = sd.run(&["forget", "--remove"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["not in PR #3"]);
    assert!(sd.tmp.join("wt").is_dir());
    assert!(sd.has_ref("refs/heads/feat"));
}

// @test "forget --remove: refuses when the PR isn't merged"
#[test]
fn forget_remove_refuses_when_the_pr_isnt_merged() {
    let mut sd = sd();
    sd.merged_wt();
    sd.merged_pr("OPEN", &sd.rev("feat"));
    let r = sd.run(&["forget", "--remove"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["isn't merged"]);
    assert!(sd.tmp.join("wt").is_dir());
}

// @test "forget --remove: in the main worktree, switches it to the default branch and deletes the branch, never removing it"
#[test]
fn forget_remove_in_the_main_worktree_switches_it_to_the_default_branch_and_deletes_the_branch_never_removing_it() {
    let mut sd = sd();
    sd.merged_wt();
    sd.feat_in_main_worktree();
    let r = sd.run(&["forget", "--remove"]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &[&format!("switched {} to main", sd.path("repo"))]);
    assert!(sd.tmp.join("repo/.git").is_dir());
    assert_eq!(sd.git(&["branch", "--show-current"]), "main");
    assert!(!sd.has_ref("refs/heads/feat"));
    assert_eq!(sd.pins("feat"), "");
}

// @test "forget --remove: in the main worktree, refuses with uncommitted changes"
#[test]
fn forget_remove_in_the_main_worktree_refuses_with_uncommitted_changes() {
    let mut sd = sd();
    sd.merged_wt();
    sd.feat_in_main_worktree();
    sd.write("wip.txt", "wip\n");
    let r = sd.run(&["forget", "--remove"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["uncommitted changes"]);
    assert_eq!(sd.git(&["branch", "--show-current"]), "feat");
    assert!(sd.cwd.join("wip.txt").exists());
}

// @test "forget --remove: a merged head this clone doesn't have says fetch, not extra commits"
#[test]
fn forget_remove_a_merged_head_this_clone_doesnt_have_says_fetch_not_extra_commits() {
    let mut sd = sd();
    sd.merged_wt();
    sd.merged_pr("MERGED", "0123456789abcdef0123456789abcdef01234567");
    let r = sd.run(&["forget", "--remove"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["git fetch"]);
    assert_lacks(&r.output, "commits the PR doesn't");
    assert!(sd.tmp.join("wt").is_dir());
    assert!(sd.has_ref("refs/heads/feat"));
}

// @test "forget: a branch with nothing stored, even one already deleted, is fine"
#[test]
fn forget_a_branch_with_nothing_stored_even_one_already_deleted_is_fine() {
    let sd = sd();
    assert_eq!(sd.run(&["forget"]).status, 0);
    assert_eq!(sd.run(&["forget", "gone"]).status, 0);
}

// @test "unregister: removes this repo from the watcher's list"
#[test]
fn unregister_removes_this_repo_from_the_watchers_list() {
    let sd = sd();
    sd.ok(&["enroll"]);
    sd.ok(&["unenroll"]);
    assert_eq!(sd.repos(), sd.path("repo"));
    assert_eq!(sd.run(&["unregister"]).status, 0);
    assert_eq!(sd.repos(), "");
}

// @test "unregister: refuses while branches are still enrolled; --force overrides"
#[test]
fn unregister_refuses_while_branches_are_still_enrolled_force_overrides() {
    let sd = sd();
    sd.ok(&["enroll"]);
    let r = sd.run(&["unregister"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["feat"]);
    assert_eq!(sd.repos(), sd.path("repo"));
    assert_eq!(sd.run(&["unregister", "--force"]).status, 0);
    assert_eq!(sd.repos(), "");
}

// @test "unregister: a path, for a repo whose directory is gone"
#[test]
fn unregister_a_path_for_a_repo_whose_directory_is_gone() {
    let sd = sd();
    sd.ok(&["enroll"]);
    sd.git(&["config", "--global", "--add", "sheepdog.repo", &sd.path("gone")]);
    assert_eq!(sd.run(&["unregister", &sd.path("gone")]).status, 0);
    assert_eq!(sd.repos(), sd.path("repo"));
}

// @test "unregister: a repo that isn't registered is fine"
#[test]
fn unregister_a_repo_that_isnt_registered_is_fine() {
    let sd = sd();
    assert_eq!(sd.run(&["unregister"]).status, 0);
}

// @test "unenroll: a real config error is not swallowed"
#[test]
fn unenroll_a_real_config_error_is_not_swallowed() {
    let sd = sd();
    sd.ok(&["enroll"]);
    sd.write(".git/config.lock", "");
    let r = sd.run(&["unenroll"]);
    let _ = fs::remove_file(sd.cwd.join(".git/config.lock"));
    assert_eq!(r.status, 1);
}
