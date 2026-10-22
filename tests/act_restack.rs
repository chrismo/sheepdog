// Black-box tests of sd act restack (A2) without --resolve, and retarget
// (A4) (#202): the built binary, run as sd-act through a link
// (tests/common). Moved from libexec/sd-act-restack.bats, with its enroll
// tests; each test keeps the bats test's title above it. The --resolve tests
// and the --help test stay in bats while restack --resolve and --help are
// bash (#203).
//
// Real git (a bare origin, our clone, and other/ for other people's pushes)
// and the real sd state. Fake gh: answers from $GH/<branch>.json, else "no
// PR", and a fetch by number with that node of whichever file has it;
// GH_FAIL_FOR=<branch> makes that branch's lookups fail.

mod common;

use common::{assert_has, assert_lacks, jq, Sd};
use std::fs;
use std::path::Path;

const GH: &str = r#"#!/usr/bin/env bash
if [[ $1 == pr && $2 == edit ]]; then echo "$*" >> "$GH/calls"; exit 0; fi
b="" n=""
for a; do [[ $a == branch=* ]] && b=${a#branch=}; [[ $a == number=* ]] && n=${a#number=}; done
# By number: the node with that number, from the one branch file that has it.
if [[ -n $n ]]; then
  f=$(jq -rn --argjson n "$n" 'inputs | select(any(.data.repository.pullRequests.nodes[]?; .number == $n)) | input_filename' "$GH"/*.json 2>/dev/null)
  [[ $f != *$'\n'* ]] || { echo "fake gh: #$n is in more than one fixture: $f" >&2; exit 2; }
  b=$(basename "${f:-none}" .json)
fi
[[ ${GH_FAIL_FOR:-} == "$b" ]] && { echo "gh: boom for $b" >&2; exit 1; }
if [[ -n $n ]]; then
  [[ -n $f ]] || { echo "gh: Could not resolve to a PullRequest with the number of $n." >&2; exit 1; }
  jq --argjson n "$n" '.data.repository |= {defaultBranchRef, pullRequest: (.pullRequests.nodes[] | select(.number == $n))}' "$f"
elif [[ -f $GH/$b.json ]]; then cat "$GH/$b.json"
else echo '{"data":{"viewer":{"login":"me"},"repository":{"defaultBranchRef":{"name":"main"},"pullRequests":{"nodes":[]}}}}'
fi
"#;

/// The bats setup: origin.git, its clones repo (cwd) and other, file.txt
/// (a..h) on main, and feat one commit past it (g→G), pushed and tracking
/// origin/feat; the fake gh and the real sd state on PATH.
fn sd() -> Sd {
    let mut sd = Sd::for_verb("act");
    sd.env = vec![("GH".to_string(), sd.path("gh"))];
    fs::create_dir(sd.tmp.join("gh")).unwrap();
    sd.fake("gh", GH);
    sd.on_path("state");
    sd.origin(&["repo"]);
    sd.cd("repo");
    sd.write("file.txt", "a\nb\nc\nd\ne\nf\ng\nh\n");
    sd.git(&["add", "file.txt"]);
    sd.git(&["commit", "-qm", "init"]);
    sd.git(&["push", "-q", "origin", "main"]);
    sd.git(&["clone", "-q", &sd.path("origin.git"), &sd.path("other")]);
    sd.git(&["switch", "-qc", "feat"]);
    sd.edit("g", "G", "feat");
    sd.git(&["push", "-q", "-u", "origin", "feat"]);
    sd
}

/// perl -pi -e 's/^<from>$/<to>/' <f>
fn subst(f: &Path, from: &str, to: &str) {
    let s = fs::read_to_string(f).unwrap();
    let out: String = s.lines().map(|l| format!("{}\n", if l == from { to } else { l })).collect();
    fs::write(f, out).unwrap();
}

/// grep -qx <line> <f>
fn has_line(f: &Path, line: &str) -> bool {
    fs::read_to_string(f).unwrap().lines().any(|l| l == line)
}

/// gh's answer for <b>: one PR <node>, with jq <args> before the filter and
/// <positional> after it.
fn pr_json(b: &str, node: &str, args: &[&str], positional: &[&str]) -> String {
    let filter = format!(
        r#"{{data: {{viewer: {{login: "me"}},
    repository: {{defaultBranchRef: {{name: "main"}},
      pullRequests: {{nodes: [{node}]}}}}}}}}"#
    );
    jq(&[&["-n", "--arg", "b", b], args, &[&filter], positional].concat(), "")
}

impl Sd {
    /// edit <from> <to> <message>: file.txt's line <from> becomes <to> here,
    /// committed.
    fn edit(&self, from: &str, to: &str, msg: &str) {
        subst(&self.cwd.join("file.txt"), from, to);
        self.git(&["commit", "-qam", msg]);
    }

    /// Someone else commits <from>→<to> in file.txt to <b> on origin.
    fn upstream(&self, b: &str, from: &str, to: &str, msg: &str) {
        self.git_in("other", &["fetch", "-q", "origin"]);
        self.git_in("other", &["checkout", "-q", "-B", b, &format!("origin/{b}")]);
        subst(&self.tmp.join("other/file.txt"), from, to);
        self.git_in("other", &["commit", "-qam", msg]);
        self.git_in("other", &["push", "-q", "origin", b]);
    }

    /// other/ squash-merges origin/A into main as "A (#1)" and pushes it,
    /// deleting A on origin too when <delete>.
    fn squash_a(&self, delete: bool) {
        self.git_in("other", &["fetch", "-q", "origin"]);
        self.git_in("other", &["checkout", "-q", "main"]);
        self.git_in("other", &["merge", "-q", "--squash", "origin/A"]);
        self.git_in("other", &["commit", "-qm", "A (#1)"]);
        let push: &[&str] = if delete { &["push", "-q", "origin", "main", ":A"] } else { &["push", "-q", "origin", "main"] };
        self.git_in("other", push);
    }

    fn gh_fixture(&self, b: &str, j: String) {
        fs::write(self.tmp.join(format!("gh/{b}.json")), j + "\n").unwrap();
    }

    /// merged_pr <branch> <head-sha> [<commit>...]: gh says <branch>'s PR
    /// merged with that head, and those commits.
    fn merged_pr(&self, b: &str, h: &str, commits: &[&str]) {
        let node = r#"{number: 1, state: "MERGED", baseRefName: "main", headRefName: $b,
                              headRefOid: $h, isCrossRepository: false,
                              allCommits: {nodes: [$ARGS.positional[] | {commit: {oid: .}}]}}"#;
        self.gh_fixture(b, pr_json(b, node, &["--arg", "h", h, "--args"], commits));
    }

    /// open_pr <branch> <base> <number>: gh says <branch> has an open PR into <base>.
    fn open_pr(&self, b: &str, base: &str, n: u32) {
        let h = self.rev(b);
        let node = r#"{number: $n, state: "OPEN", baseRefName: $base, headRefName: $b,
                              headRefOid: $h, isCrossRepository: false}"#;
        let j = pr_json(b, node, &["--arg", "base", base, "--argjson", "n", &n.to_string(), "--arg", "h", &h], &[]);
        self.gh_fixture(b, j);
    }

    /// closed_pr <branch> <head-sha>: gh says <branch>'s PR was closed without merging.
    fn closed_pr(&self, b: &str, h: &str) {
        let node = r#"{number: 1, state: "CLOSED", baseRefName: "main", headRefName: $b,
                              headRefOid: $h, isCrossRepository: false}"#;
        self.gh_fixture(b, pr_json(b, node, &["--arg", "h", h], &[]));
    }

    /// stacked_child: A off main, child off A with sd-parent A; both pushed.
    /// A's head.
    fn stacked_child(&self) -> String {
        self.git(&["switch", "-q", "main"]);
        self.git(&["switch", "-qc", "A"]);
        self.edit("c", "C", "a1");
        self.git(&["push", "-q", "origin", "A"]);
        let a_head = self.rev("A");
        self.git(&["switch", "-qc", "child"]);
        self.edit("f", "F", "child");
        self.git(&["push", "-q", "-u", "origin", "child"]);
        self.git(&["config", "branch.child.sd-parent", "A"]);
        a_head
    }

    fn rev(&self, r: &str) -> String {
        self.git(&["rev-parse", r])
    }

    fn short12(&self, r: &str) -> String {
        self.git(&["rev-parse", "--short=12", r])
    }

    fn remote_feat(&self) -> String {
        let l = self.git(&["ls-remote", "origin", "refs/heads/feat"]);
        l.split('\t').next().unwrap_or_default().to_string()
    }

    /// refs/sheepdog/feat/base's sha, or "".
    fn base_pin(&self) -> String {
        self.git_try(&["rev-parse", "-q", "--verify", "refs/sheepdog/feat/base"]).unwrap_or_default()
    }

    /// git config <key>'s value, or "" when unset.
    fn config(&self, key: &str) -> String {
        self.git_try(&["config", key]).unwrap_or_default()
    }

    fn file_has(&self, line: &str) -> bool {
        has_line(&self.cwd.join("file.txt"), line)
    }

    fn gh_calls(&self) -> Option<String> {
        fs::read_to_string(self.tmp.join("gh/calls")).ok().map(|s| s.trim_end_matches('\n').to_string())
    }

    /// [ -e "$(git rev-parse --git-path <name>)" ]
    fn git_path_exists(&self, name: &str) -> bool {
        self.cwd.join(self.git(&["rev-parse", "--git-path", name])).exists()
    }
}

// @test "onto the moved default branch: rebased, base pinned, not pushed"
#[test]
fn onto_the_moved_default_branch_rebased_base_pinned_not_pushed() {
    let sd = sd();
    sd.upstream("main", "b", "B", "upstream");
    let before_remote = sd.remote_feat();
    let r = sd.run(&["restack"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.rev("feat^"), sd.rev("origin/main"));
    assert_eq!(sd.git(&["log", "--format=%s", "origin/main..feat"]), "feat");
    assert!(sd.file_has("B") && sd.file_has("G"));
    assert_eq!(sd.base_pin(), sd.rev("origin/main"));
    assert_eq!(sd.remote_feat(), before_remote);
    assert_has(&r.output, &["restacked feat onto origin/main"]);
    assert_has(&r.output, &["not pushed: a plain git push is rejected", "sd act push"]);
}

// @test "onto the moved default branch, branch never pushed: no plain-push warning, just sd act push (#47)"
#[test]
fn onto_the_moved_default_branch_branch_never_pushed_no_plain_push_warning_just_sd_act_push_47() {
    let sd = sd();
    sd.git(&["push", "-q", "origin", ":feat"]);
    sd.git(&["fetch", "-q", "--prune"]);
    sd.upstream("main", "b", "B", "upstream");
    let r = sd.run(&["restack"]);
    assert_eq!(r.status, 0);
    assert!(r.output.ends_with("; not pushed: sd act push"), "{}", r.output);
    assert_lacks(&r.output, "rejected");
}

// @test "already on the target: nothing to do"
#[test]
fn already_on_the_target_nothing_to_do() {
    let sd = sd();
    let before = sd.rev("HEAD");
    let r = sd.run(&["restack"]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &["already on origin/main"]);
    assert_eq!(sd.rev("HEAD"), before);
    assert_eq!(sd.base_pin(), "");
}

// @test "fetches first: a move it hasn't seen yet still counts"
#[test]
fn fetches_first_a_move_it_hasnt_seen_yet_still_counts() {
    let sd = sd();
    sd.upstream("main", "b", "B", "upstream");
    assert_ne!(sd.rev("origin/main"), sd.git_in("other", &["rev-parse", "main"]));
    assert_eq!(sd.run(&["restack"]).status, 0);
    assert_eq!(sd.rev("feat^"), sd.git_in("other", &["rev-parse", "main"]));
}

// @test "parent squash-merged: lands on main with only its own commits"
#[test]
fn parent_squash_merged_lands_on_main_with_only_its_own_commits() {
    let sd = sd();
    sd.git(&["switch", "-q", "main"]);
    sd.git(&["switch", "-qc", "A"]);
    sd.edit("c", "C", "a1");
    sd.edit("d", "D", "a2");
    sd.git(&["push", "-q", "origin", "A"]);
    let a_head = sd.rev("A");
    sd.git(&["switch", "-qc", "child"]);
    sd.edit("f", "F", "child");
    sd.git(&["config", "branch.child.sd-parent", "A"]);
    sd.squash_a(false);
    sd.merged_pr("A", &a_head, &[&sd.rev("A~1"), &a_head]);
    assert_eq!(sd.run(&["restack"]).status, 0);
    assert_eq!(sd.rev("child^"), sd.rev("origin/main"));
    assert_eq!(sd.git(&["log", "--format=%s", "origin/main..child"]), "child");
    assert!(sd.file_has("C") && sd.file_has("D") && sd.file_has("F"));
    assert_eq!(sd.rev("refs/sheepdog/child/base"), sd.rev("origin/main"));
    assert_eq!(sd.config("branch.child.sd-parent"), "main");
}

// @test "parent merged but the PR still targets it: sd-parent kept, so retarget is still owed"
#[test]
fn parent_merged_but_the_pr_still_targets_it_sd_parent_kept_so_retarget_is_still_owed() {
    let sd = sd();
    let a_head = sd.stacked_child();
    sd.squash_a(false);
    sd.merged_pr("A", &a_head, &[]);
    sd.open_pr("child", "A", 2);
    let r = sd.run(&["restack"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.rev("child^"), sd.rev("origin/main"));
    assert_eq!(sd.config("branch.child.sd-parent"), "A");
    assert_has(&r.output, &["PR #2 still targets A: sd act retarget"]);
    let r = sd.exec(&sd.tmp.join("fakebin/sd-state"), &[], &["--json", "child"]);
    assert_eq!(jq(&["-r", r#"[.next[].key] | index("retarget") != null"#], &r.output), "true");
    assert_eq!(sd.run(&["retarget"]).status, 0);
    assert_eq!(sd.gh_calls().as_deref(), Some("pr edit 2 --base main"));
    assert_eq!(sd.config("branch.child.sd-parent"), "main");
}

// @test "parent merged, the branch's own PR lookup fails: sd-parent kept"
#[test]
fn parent_merged_the_branchs_own_pr_lookup_fails_sd_parent_kept() {
    let sd = sd();
    let a_head = sd.stacked_child();
    sd.squash_a(false);
    sd.merged_pr("A", &a_head, &[]);
    let r = sd.run_env(&[("GH_FAIL_FOR", "child")], &["restack"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.rev("child^"), sd.rev("origin/main"));
    assert_eq!(sd.config("branch.child.sd-parent"), "A");
    assert_has(&r.output, &["sd-parent of child stays A: can't read its PR"]);
}

// @test "restack onto a parent closed without merging is refused"
#[test]
fn restack_onto_a_parent_closed_without_merging_is_refused() {
    let sd = sd();
    let a_head = sd.stacked_child();
    sd.closed_pr("A", &a_head);
    sd.upstream("A", "d", "D", "a2");
    let r = sd.run(&["restack"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["closed without merging"]);
    assert_eq!(sd.rev("child^"), a_head);
}

// ── retarget ─────────────────────────────────────────────────────────────────

// @test "retarget: parent merged, PR still on it: base edited to main, sd-parent rewritten"
#[test]
fn retarget_parent_merged_pr_still_on_it_base_edited_to_main_sd_parent_rewritten() {
    let sd = sd();
    let a_head = sd.stacked_child();
    sd.merged_pr("A", &a_head, &[]);
    sd.open_pr("child", "A", 2);
    assert_eq!(sd.run(&["retarget"]).status, 0);
    assert_eq!(sd.gh_calls().as_deref(), Some("pr edit 2 --base main"));
    assert_eq!(sd.config("branch.child.sd-parent"), "main");
}

// @test "retarget: the PR already targets main (GitHub did it): no edit, sd-parent still rewritten"
#[test]
fn retarget_the_pr_already_targets_main_github_did_it_no_edit_sd_parent_still_rewritten() {
    let sd = sd();
    let a_head = sd.stacked_child();
    sd.merged_pr("A", &a_head, &[]);
    sd.open_pr("child", "main", 2);
    assert_eq!(sd.run(&["retarget"]).status, 0);
    assert_eq!(sd.gh_calls(), None);
    assert_eq!(sd.config("branch.child.sd-parent"), "main");
}

// @test "retarget: parent still open: refused"
#[test]
fn retarget_parent_still_open_refused() {
    let sd = sd();
    sd.stacked_child();
    sd.open_pr("A", "main", 1);
    sd.open_pr("child", "A", 2);
    let r = sd.run(&["retarget"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["hasn't merged"]);
    assert_eq!(sd.gh_calls(), None);
    assert_eq!(sd.config("branch.child.sd-parent"), "A");
}

// @test "retarget: parent closed without merging: refused, says so"
#[test]
fn retarget_parent_closed_without_merging_refused_says_so() {
    let sd = sd();
    let a_head = sd.stacked_child();
    sd.closed_pr("A", &a_head);
    sd.open_pr("child", "A", 2);
    let r = sd.run(&["retarget"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["closed without merging"]);
    assert_eq!(sd.gh_calls(), None);
}

// @test "retarget: a branch with no PR is refused"
#[test]
fn retarget_a_branch_with_no_pr_is_refused() {
    let sd = sd();
    let a_head = sd.stacked_child();
    sd.merged_pr("A", &a_head, &[]);
    let r = sd.run(&["retarget"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["no PR"]);
}

// @test "stacked on an open parent: onto the parent, not main"
#[test]
fn stacked_on_an_open_parent_onto_the_parent_not_main() {
    let sd = sd();
    sd.git(&["switch", "-q", "main"]);
    sd.git(&["switch", "-qc", "A"]);
    sd.edit("c", "C", "a1");
    sd.git(&["push", "-q", "origin", "A"]);
    sd.git(&["switch", "-qc", "child"]);
    sd.edit("f", "F", "child");
    sd.git(&["config", "branch.child.sd-parent", "A"]);
    sd.upstream("A", "d", "D", "a2");
    sd.upstream("main", "b", "B", "upstream");
    sd.open_pr("A", "main", 1);
    sd.open_pr("child", "A", 2);
    let r = sd.run(&["restack"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.rev("child^"), sd.rev("origin/A"));
    assert!(!sd.file_has("B"));
    assert_lacks(&r.output, "sd act retarget");
    assert_eq!(sd.config("branch.child.sd-parent"), "A");
}

// @test "a restack that would conflict is refused; nothing moves"
#[test]
fn a_restack_that_would_conflict_is_refused_nothing_moves() {
    let sd = sd();
    sd.upstream("main", "g", "X", "upstream");
    let before = sd.rev("HEAD");
    let r = sd.run(&["restack"]);
    assert_eq!(r.status, 1);
    assert_has(&r.output, &["would conflict"]);
    let by_hand = format!(
        "git rebase --onto origin/main {} feat, then git update-ref refs/sheepdog/feat/base $(git merge-base origin/main feat)",
        sd.short12("main")
    );
    assert_has(&r.output, &[&by_hand]);
    assert_has(&r.output, &["sd act restack --resolve"]);
    assert_eq!(sd.rev("HEAD"), before);
    assert_eq!(sd.base_pin(), "");
}

// @test "a conflict partway through is rolled back"
#[test]
fn a_conflict_partway_through_is_rolled_back() {
    let sd = sd();
    // Net, feat leaves line e alone, so the merge sd state predicts is clean;
    // replaying e→E commit by commit still hits main's e→X.
    sd.edit("e", "E", "there");
    sd.edit("E", "e", "back");
    sd.upstream("main", "e", "X", "upstream");
    let before = sd.rev("HEAD");
    let r = sd.run(&["restack"]);
    assert_eq!(r.status, 1);
    assert_has(&r.output, &["rolled back"]);
    let onto = sd.short12("origin/main");
    let by_hand = format!(
        "git rebase --onto {onto} {} feat, then git update-ref refs/sheepdog/feat/base {onto}",
        sd.short12("main")
    );
    assert_has(&r.output, &[&by_hand]);
    assert_eq!(sd.rev("HEAD"), before);
    assert_eq!(sd.git(&["branch", "--show-current"]), "feat");
    assert_eq!(sd.git(&["status", "--porcelain"]), "");
    assert!(!sd.git_path_exists("rebase-merge"));
    assert!(!sd.git_path_exists("rebase-apply"));
    assert_eq!(sd.base_pin(), "");
}

// @test "a change that comes out different is rolled back"
#[test]
fn a_change_that_comes_out_different_is_rolled_back() {
    let sd = sd();
    // main picks up the same edit as feat's first commit, so the rebase drops it
    // and the branch's diff shrinks.
    sd.edit("b", "B", "second");
    sd.upstream("main", "g", "G", "upstream");
    let before = sd.rev("HEAD");
    let r = sd.run(&["restack"]);
    assert_eq!(r.status, 1);
    assert_has(&r.output, &["came out different"]);
    assert_has(&r.output, &["rolled back"]);
    assert_eq!(sd.rev("HEAD"), before);
    assert_eq!(sd.git(&["status", "--porcelain"]), "");
    assert_eq!(sd.base_pin(), "");
}

// @test "uncommitted changes are refused"
#[test]
fn uncommitted_changes_are_refused() {
    let sd = sd();
    sd.upstream("main", "b", "B", "upstream");
    let f = sd.cwd.join("file.txt");
    fs::write(&f, fs::read_to_string(&f).unwrap() + "dirty\n").unwrap();
    let r = sd.run(&["restack"]);
    assert_eq!(r.status, 1);
    assert_has(&r.output, &["uncommitted changes"]);
}

// @test "taken over: refused unless --force"
#[test]
fn taken_over_refused_unless_force() {
    let sd = sd();
    sd.upstream("main", "b", "B", "upstream");
    sd.git(&["config", "branch.feat.sd-lock", "chrismo"]);
    let r = sd.run(&["restack"]);
    assert_eq!(r.status, 1);
    assert_has(&r.output, &["taken over"]);
    assert_has(&r.output, &["--force"]);
    assert_eq!(sd.run(&["restack", "--force"]).status, 0);
    assert_eq!(sd.rev("feat^"), sd.rev("origin/main"));
}

// @test "a parent that isn't on the remote is refused with sd state's reason"
#[test]
fn a_parent_that_isnt_on_the_remote_is_refused_with_sd_states_reason() {
    let sd = sd();
    sd.git(&["config", "branch.feat.sd-parent", "nosuch"]);
    let r = sd.run(&["restack"]);
    assert_eq!(r.status, 1);
    assert_has(&r.output, &["no origin/nosuch ref"]);
}

// @test "a parent whose merged state is unknown is refused"
#[test]
fn a_parent_whose_merged_state_is_unknown_is_refused() {
    let sd = sd();
    sd.git(&["switch", "-q", "main"]);
    sd.git(&["switch", "-qc", "A"]);
    sd.edit("c", "C", "a1");
    sd.git(&["push", "-q", "origin", "A"]);
    sd.git(&["switch", "-qc", "child"]);
    sd.edit("f", "F", "child");
    sd.git(&["config", "branch.child.sd-parent", "A"]);
    sd.upstream("A", "d", "D", "a2");
    let r = sd.run_env(&[("GH_FAIL_FOR", "A")], &["restack"]);
    assert_eq!(r.status, 1);
    assert_has(&r.output, &["can't tell whether A merged"]);
}

// @test "a parent only here is refused with the step: ship it first (#105)"
#[test]
fn a_parent_only_here_is_refused_with_the_step_ship_it_first_105() {
    let sd = sd();
    sd.git(&["switch", "-q", "main"]);
    sd.git(&["switch", "-qc", "A"]);
    sd.edit("c", "C", "a1");
    sd.git(&["switch", "-qc", "child"]);
    sd.edit("f", "F", "child");
    sd.git(&["config", "branch.child.sd-parent", "A"]);
    let r = sd.run(&["restack"]);
    assert_eq!(r.status, 1);
    assert_has(&r.output, &["can't restack child: parent A isn't on origin yet: ship it first (sd act ship A)"]);
}

// @test "a branch not checked out anywhere is refused"
#[test]
fn a_branch_not_checked_out_anywhere_is_refused() {
    let sd = sd();
    sd.git(&["switch", "-q", "main"]);
    let r = sd.run(&["restack", "feat"]);
    assert_eq!(r.status, 1);
    assert_has(&r.output, &["isn't checked out in any worktree"]);
}

// @test "a named branch is restacked in its own worktree"
#[test]
fn a_named_branch_is_restacked_in_its_own_worktree() {
    let sd = sd();
    sd.upstream("main", "b", "B", "upstream");
    sd.git(&["switch", "-q", "main"]);
    sd.git(&["worktree", "add", "-q", &sd.path("wt"), "feat"]);
    assert_eq!(sd.run(&["restack", "feat"]).status, 0);
    assert_eq!(sd.rev("feat^"), sd.rev("origin/main"));
    assert!(has_line(&sd.tmp.join("wt/file.txt"), "B"));
    assert_eq!(sd.git(&["branch", "--show-current"]), "main");
}

// @test "enroll records a stacked branch's parent and base, so restack survives GitHub's retarget"
#[test]
fn enroll_records_a_stacked_branchs_parent_and_base_so_restack_survives_githubs_retarget() {
    let sd = sd();
    sd.git(&["switch", "-q", "main"]);
    sd.git(&["switch", "-qc", "A"]);
    sd.edit("c", "C", "a1");
    sd.git(&["push", "-q", "origin", "A"]);
    let a_head = sd.rev("A");
    sd.git(&["switch", "-qc", "child"]);
    sd.edit("f", "F", "child");
    sd.git(&["push", "-q", "-u", "origin", "child"]);
    sd.open_pr("A", "main", 1);
    sd.open_pr("child", "A", 2);
    assert_eq!(sd.run(&["enroll", "child"]).status, 0);
    assert_eq!(sd.config("branch.child.sd-parent"), "A");
    assert_eq!(sd.rev("refs/sheepdog/child/base"), a_head);
    sd.squash_a(false);
    sd.merged_pr("A", &a_head, &[]);
    sd.open_pr("child", "main", 2);
    assert_eq!(sd.run(&["restack"]).status, 0);
    assert_eq!(sd.rev("child^"), sd.rev("origin/main"));
    assert_eq!(sd.git(&["log", "--format=%s", "origin/main..child"]), "child");
}

// As on a second machine: the parent moved on before this clone saw where the
// branch left it, so there's no fork-point and nothing to pin (#97).
// @test "enroll of a stacked branch whose base git can't find: records the parent, pins nothing, says how to pin it"
#[test]
fn enroll_of_a_stacked_branch_whose_base_git_cant_find_records_the_parent_pins_nothing_says_how_to_pin_it() {
    let sd = sd();
    sd.git(&["switch", "-q", "main"]);
    sd.git(&["switch", "-qc", "A"]);
    sd.edit("c", "C", "a1");
    sd.git(&["push", "-q", "origin", "A"]);
    sd.git(&["switch", "-qc", "child"]);
    sd.edit("f", "F", "child");
    sd.git(&["push", "-q", "-u", "origin", "child"]);
    sd.upstream("A", "d", "D", "a2");
    sd.git(&["fetch", "-q"]);
    sd.git(&["reflog", "expire", "--expire=now", "--expire-unreachable=now", "refs/remotes/origin/A"]);
    sd.open_pr("A", "main", 1);
    sd.open_pr("child", "A", 2);
    let r = sd.run(&["enroll", "child"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.config("branch.child.sd-parent"), "A");
    assert_eq!(sd.git_try(&["rev-parse", "-q", "--verify", "refs/sheepdog/child/base"]), None);
    assert_has(&r.output, &["warning: no usable base for child: no base for child on A", "git update-ref refs/sheepdog/child/base"]);
}

// @test "enroll records nothing for a branch whose PR targets the default branch"
#[test]
fn enroll_records_nothing_for_a_branch_whose_pr_targets_the_default_branch() {
    let sd = sd();
    sd.open_pr("feat", "main", 1);
    assert_eq!(sd.run(&["enroll"]).status, 0);
    assert_eq!(sd.config("branch.feat.sd-parent"), "");
    assert_eq!(sd.base_pin(), "");
}
