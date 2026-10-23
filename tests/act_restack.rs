// Black-box tests of sd act restack (A2), with and without --resolve (J1: an
// agent resolves the conflicts), and retarget (A4) (#202, #203): the built
// binary, run as sd-act through a link (tests/common). Moved from
// libexec/sd-act-restack.bats, with its enroll tests; each test keeps the
// bats test's title above it.
//
// Real git (a bare origin, our clone, and other/ for other people's pushes)
// and the real sd state. Fake gh: answers from $GH/<branch>.json, else "no
// PR", and a fetch by number with that node of whichever file has it;
// GH_FAIL_FOR=<branch> makes that branch's lookups fail. For --resolve,
// claude edits through $FX/agent and prints $FX/result.json as structured
// output.

mod common;

use common::{assert_has, assert_lacks, chomp, jq, Sd};
use std::collections::BTreeSet;
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
/// origin/feat; the fake gh on PATH.
fn sd() -> Sd {
    let mut sd = Sd::for_verb("act");
    sd.env = vec![("GH".to_string(), sd.path("gh"))];
    fs::create_dir(sd.tmp.join("gh")).unwrap();
    sd.fake("gh", GH);
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

/// perl -pi -e 's/^<from>$/<to>/; ...' <f>, for each (<from>, <to>) in turn.
fn subst(f: &Path, subs: &[(&str, &str)]) {
    let s = fs::read_to_string(f).unwrap();
    let out: String = s
        .lines()
        .map(|l| format!("{}\n", subs.iter().fold(l, |l, (from, to)| if l == *from { to } else { l })))
        .collect();
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
        subst(&self.cwd.join("file.txt"), &[(from, to)]);
        self.git(&["commit", "-qam", msg]);
    }

    /// Someone else commits <from>→<to> in file.txt to <b> on origin.
    fn upstream(&self, b: &str, from: &str, to: &str, msg: &str) {
        self.upstream_all(b, &[(from, to)], msg);
    }

    /// Someone else commits each <from>→<to> in file.txt to <b> on origin.
    fn upstream_all(&self, b: &str, subs: &[(&str, &str)], msg: &str) {
        self.git_in("other", &["fetch", "-q", "origin"]);
        self.git_in("other", &["checkout", "-q", "-B", b, &format!("origin/{b}")]);
        subst(&self.tmp.join("other/file.txt"), subs);
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
    let r = sd.exec(&sd.link("state"), &[], &["--json", "child"]);
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

// ── --resolve: the agent resolves conflicts (J1) ─────────────────────────────

const CLAUDE: &str = r#"#!/usr/bin/env bash
printf '%s\n' "$@" > "$FX/claude.args"
args=("$@") prompt= i=0
while (( i < $# )); do
  case ${args[i]} in
    --allowedTools|--disallowedTools)
      i=$((i + 1)); while (( i < $# )) && [[ ${args[i]} != -* ]]; do i=$((i + 1)); done; continue ;;
    --session-id|--resume|--output-format|--json-schema|--permission-mode|--model|--effort|--max-turns|--max-budget-usd) i=$((i + 2)); continue ;;
    -*) ;;
    *) prompt=${args[i]} ;;
  esac
  i=$((i + 1))
done
printf '%s' "$prompt" > "$FX/claude.prompt"
echo "$PWD" >> "$FX/claude.calls"
[[ -x $FX/agent ]] && "$FX/agent"
[[ -e $FX/fail ]] && { echo "claude: boom" >&2; exit 1; }
jq -n --argjson r "$(cat "$FX/result.json")" '{type: "result", is_error: false, structured_output: $r}'
"#;

/// The bats setup's agent half on top of sd(): the state dir (usage.sup),
/// $FX, the fake claude, and its result.
fn agent_sd() -> Sd {
    let mut sd = sd();
    sd.env.push(("SHEEPDOG_STATE_DIR".into(), sd.path("state")));
    sd.env.push(("FX".into(), sd.path("fx")));
    fs::create_dir(sd.tmp.join("fx")).unwrap();
    sd.fake("claude", CLAUDE);
    sd.result(r#"{"outcome": "done", "summary": "kept both"}"#);
    sd
}

/// The line after <flag>'s last line in <args>: `grep -A1 -x -- <flag> | tail -1`.
fn after(args: &str, flag: &str) -> String {
    let lines: Vec<&str> = args.lines().collect();
    let i = lines.iter().rposition(|l| *l == flag).unwrap_or_else(|| panic!("no {flag} in {args:?}"));
    lines.get(i + 1).unwrap_or(&"").to_string()
}

/// `sed -n '/^<start>$/,/^$/p'`, chomped as `$( )` does.
fn sed_range(s: &str, start: &str) -> String {
    let mut out = vec![];
    let mut on = false;
    for l in s.lines() {
        if on || l == start {
            out.push(l);
            on = !(on && l.is_empty());
        }
    }
    chomp(&out.join("\n"))
}

impl Sd {
    /// $FX/result.json := <j>
    fn result(&self, j: &str) {
        fs::write(self.tmp.join("fx/result.json"), format!("{j}\n")).unwrap();
    }

    /// `$(cat "$FX/<f>")`, or "" when it's missing.
    fn fx(&self, f: &str) -> String {
        chomp(&fs::read_to_string(self.tmp.join("fx").join(f)).unwrap_or_default())
    }

    fn fx_exists(&self, f: &str) -> bool {
        self.tmp.join("fx").join(f).exists()
    }

    /// $FX/agent := <script>, what the agent does.
    fn agent(&self, script: &str) {
        self.script(&self.tmp.join("fx/agent"), script);
    }

    /// `printf <line> >> "$FX/agent"`: the agent does more.
    fn agent_also(&self, line: &str) {
        let f = self.tmp.join("fx/agent");
        self.agent(&(fs::read_to_string(&f).unwrap() + line));
    }

    /// resolver <replacement>: an agent that replaces every conflict block in
    /// file.txt with one line.
    fn resolver(&self, rep: &str) {
        self.agent(&format!(
            "#!/usr/bin/env bash\nperl -0pi -e \"s/<<<<<<<.*?>>>>>>>[^\\n]*\\n/{rep}\\n/sg\" file.txt\n"
        ));
    }

    fn rebase_in_progress(&self) -> bool {
        self.git_path_exists("rebase-merge") || self.git_path_exists("rebase-apply")
    }

    /// orphaned_pin: A (a1, a2) squash-merged into main and deleted; child
    /// was restacked onto A, then rewritten outside sheepdog: a2 is now an
    /// exact copy, so the pin at A's head isn't an ancestor. A~1 and A's head.
    fn orphaned_pin(&self) -> (String, String) {
        self.git(&["switch", "-q", "main"]);
        self.git(&["switch", "-qc", "A"]);
        self.edit("c", "C", "a1");
        self.edit("d", "D", "a2");
        self.git(&["push", "-q", "origin", "A"]);
        let (a1, a_head) = (self.rev("A~1"), self.rev("A"));
        self.git(&["switch", "-qc", "child", "A~1"]);
        self.edit("d", "D", "a2 again");
        self.edit("f", "F", "child");
        self.git(&["config", "branch.child.sd-parent", "A"]);
        self.git(&["update-ref", "refs/sheepdog/child/base", &a_head]);
        self.squash_a(true);
        (a1, a_head)
    }
}

// @test "help lists restack and push"
#[test]
fn help_lists_restack_and_push() {
    let sd = agent_sd();
    let r = sd.run(&["--help"]);
    assert_has(&r.output, &["sd act restack"]);
    assert_has(&r.output, &["sd act push"]);
}

// @test "stacked, pin lost, parent squash-merged and deleted: refused, no rebase, no agent"
#[test]
fn stacked_pin_lost_parent_squash_merged_and_deleted_refused_no_rebase_no_agent() {
    let sd = agent_sd();
    sd.git(&["switch", "-q", "main"]);
    sd.git(&["switch", "-qc", "A"]);
    sd.edit("c", "C", "a1");
    sd.edit("d", "D", "a2");
    sd.git(&["push", "-q", "origin", "A"]);
    let a_head = sd.rev("A");
    sd.git(&["switch", "-qc", "child", "A~1"]);
    sd.edit("d", "DD", "a2, changed");
    sd.edit("f", "F", "child");
    sd.git(&["config", "branch.child.sd-parent", "A"]);
    sd.git(&["update-ref", "refs/sheepdog/child/base", &a_head]);
    sd.squash_a(true);
    sd.merged_pr("A", &a_head, &[]);
    let before = sd.rev("child");
    let r = sd.run(&["restack", "--resolve"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &[&format!("base pin {} is no longer in child's history", &a_head[..7])]);
    assert_eq!(sd.rev("child"), before);
    assert!(!sd.rebase_in_progress());
    assert!(!sd.fx_exists("claude.calls"));
}

// @test "orphaned pin, all of the parent in HEAD: one commit on main by tree, no agent"
#[test]
fn orphaned_pin_all_of_the_parent_in_head_one_commit_on_main_by_tree_no_agent() {
    let sd = agent_sd();
    let (a1, a_head) = sd.orphaned_pin();
    sd.merged_pr("A", &a_head, &[&a1, &a_head]);
    let old_head = sd.rev("child");
    let r = sd.run(&["restack", "--resolve"]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &["restacked child onto origin/main by tree (1 commit)"]);
    let old = format!("child's commits are now one, committed by you; the old ones are at {}", &old_head[..7]);
    assert_has(&r.output, &[&old]);
    assert_eq!(sd.rev("child^"), sd.rev("origin/main"));
    assert_eq!(sd.git(&["rev-list", "--count", "origin/main..child"]), "1");
    assert_eq!(sd.git(&["diff", "origin/main", "child"]), sd.git(&["diff", &a_head, &old_head]));
    assert_eq!(
        sd.git(&["log", "-1", "--format=%B", "child"]),
        "child (restacked by tree onto origin/main)\n\nReplaces:\nchild"
    );
    assert_eq!(sd.rev("refs/sheepdog/child/base"), sd.rev("origin/main"));
    assert!(sd.file_has("F"));
    assert!(!sd.fx_exists("claude.calls"));
}

// @test "a tree restack's commit is the branch's own in its reflog, not a reset (stacked_on, #94)"
#[test]
fn a_tree_restacks_commit_is_the_branchs_own_in_its_reflog_not_a_reset_stacked_on_94() {
    let sd = agent_sd();
    let (a1, a_head) = sd.orphaned_pin();
    sd.merged_pr("A", &a_head, &[&a1, &a_head]);
    let r = sd.run(&["restack", "--resolve"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.git(&["reflog", "-1", "--format=%H", "refs/heads/child"]), sd.rev("child"));
    assert!(!sd.git(&["reflog", "-1", "--format=%gs", "refs/heads/child"]).starts_with("reset: moving to "));
}

// @test "orphaned pin, a tree restack that would conflict: refused even with --resolve"
#[test]
fn orphaned_pin_a_tree_restack_that_would_conflict_refused_even_with_resolve() {
    let sd = agent_sd();
    let (a1, a_head) = sd.orphaned_pin();
    sd.upstream("main", "f", "X", "upstream");
    sd.merged_pr("A", &a_head, &[&a1, &a_head]);
    let before = sd.rev("child");
    let r = sd.run(&["restack", "--resolve"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["by tree would conflict"]);
    let (a12, b12) = (&a_head[..12], &before[..12]);
    let by_hand = format!(
        "By hand: git switch child && git reset --hard origin/main && git diff --binary {a12} {b12} | git apply -3, resolve and commit, then git update-ref refs/sheepdog/child/base origin/main (the old head stays at {b12})"
    );
    assert_has(&r.output, &[&by_hand]);
    assert_eq!(sd.rev("child"), before);
    assert!(!sd.fx_exists("claude.calls"));
}

// @test "parent squash-merged, base below the parent's commits: refused, no rebase, no agent"
#[test]
fn parent_squash_merged_base_below_the_parents_commits_refused_no_rebase_no_agent() {
    let sd = agent_sd();
    sd.git(&["switch", "-q", "main"]);
    sd.git(&["switch", "-qc", "A"]);
    sd.edit("c", "C", "a1");
    sd.edit("d", "D", "a2");
    sd.git(&["push", "-q", "origin", "A"]);
    let (a1, a_head) = (sd.rev("A~1"), sd.rev("A"));
    sd.git(&["switch", "-qc", "child"]);
    sd.edit("f", "F", "child");
    sd.git(&["config", "branch.child.sd-parent", "A"]);
    sd.git(&["update-ref", "refs/sheepdog/child/base", &sd.rev("main")]);
    sd.squash_a(false);
    sd.merged_pr("A", &a_head, &[&a1, &a_head]);
    let before = sd.rev("child");
    let r = sd.run(&["restack", "--resolve"]);
    assert_ne!(r.status, 0);
    let wrong = format!(
        "wrong base: this would replay 2 of #1's commits onto origin/main; if the stack is linear, rebase past them by hand: git rebase --onto origin/main {} child && git update-ref refs/sheepdog/child/base origin/main",
        &a_head[..12]
    );
    assert_has(&r.output, &[&wrong]);
    assert_eq!(sd.rev("child"), before);
    assert!(!sd.rebase_in_progress());
    assert!(!sd.fx_exists("claude.calls"));
    let by_hand = r.output.rsplit_once("by hand: ").unwrap().1;
    assert_eq!(sd.exec(Path::new("bash"), &[], &["-c", "eval \"$1\" 2>/dev/null", "eval", by_hand]).status, 0);
    assert_eq!(sd.git(&["log", "--format=%s", "origin/main..child"]), "child");
    assert_eq!(sd.rev("refs/sheepdog/child/base"), sd.rev("origin/main"));
    let r = sd.run(&["restack"]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &["already on origin/main"]);
}

// @test "--resolve: the agent resolves a predicted conflict; base pinned; its change is unreviewed"
#[test]
fn resolve_the_agent_resolves_a_predicted_conflict_base_pinned_its_change_is_unreviewed() {
    let sd = agent_sd();
    sd.upstream("main", "g", "X", "upstream");
    sd.resolver("GX");
    let r = sd.run(&["restack", "--resolve"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.rev("feat^"), sd.rev("origin/main"));
    assert!(sd.file_has("GX"));
    assert_lacks(&fs::read_to_string(sd.cwd.join("file.txt")).unwrap(), "<<<<<<<");
    assert_eq!(sd.base_pin(), sd.rev("origin/main"));
    assert_has(&r.output, &["resolved by the agent, so feat's change is unreviewed"]);
    assert!(!sd.rebase_in_progress());
    assert_eq!(sd.git(&["status", "--porcelain"]), "");
}

// @test "--resolve: usage.sup logs the agent's step as resolve, not implement (#150)"
#[test]
fn resolve_usage_sup_logs_the_agents_step_as_resolve_not_implement_150() {
    let sd = agent_sd();
    sd.upstream("main", "g", "X", "upstream");
    sd.resolver("GX");
    let r = sd.run(&["restack", "--resolve"]);
    assert_eq!(r.status, 0);
    let usage = fs::read_to_string(sd.tmp.join("state/usage.sup")).unwrap();
    assert_has(&usage, &[r#"branch:"feat",step:"resolve","#]);
    assert_lacks(&usage, r#"step:"implement""#);
}

// sd state's NEXT is the one place the order lives (#144).
// @test "--resolve: names no next command; sd state's NEXT does that (#144)"
#[test]
fn resolve_names_no_next_command_sd_states_next_does_that_144() {
    let sd = agent_sd();
    sd.upstream("main", "g", "X", "upstream");
    sd.resolver("GX");
    let r = sd.run(&["restack", "--resolve"]);
    assert_eq!(r.status, 0);
    assert_lacks(&r.output, "sd act review");
    assert_lacks(&r.output, "next");
}

// @test "--resolve: prints the agent's summary of each stop (#156)"
#[test]
fn resolve_prints_the_agents_summary_of_each_stop_156() {
    let sd = agent_sd();
    sd.upstream("main", "g", "X", "upstream");
    sd.resolver("GX");
    sd.result(r#"{"outcome": "done", "summary": "kept both; main's new case lacks the refactor"}"#);
    let r = sd.run(&["restack", "--resolve"]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &["feat (stop 1): kept both; main's new case lacks the refactor"]);
}

// @test "--resolve: an agent that changes CI config while resolving holds the restacked branch for chrismo (#124)"
#[test]
fn resolve_an_agent_that_changes_ci_config_while_resolving_holds_the_restacked_branch_for_chrismo_124() {
    let sd = agent_sd();
    sd.upstream("main", "g", "X", "upstream");
    sd.resolver("GX");
    sd.agent_also("mkdir -p .github/workflows && echo x > .github/workflows/new.yml\n");
    let r = sd.run(&["restack", "--resolve"]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &["held: the agent changed CI config (.github/workflows/new.yml): review, then sd act push"]);
    assert_eq!(sd.config("branch.feat.sd-held"), format!("{}\n.github/workflows/new.yml", sd.rev("feat")));
}

// @test "--resolve: a CI config edit the agent staged itself is held all the same (#124)"
#[test]
fn resolve_a_ci_config_edit_the_agent_staged_itself_is_held_all_the_same_124() {
    let sd = agent_sd();
    sd.upstream("main", "g", "X", "upstream");
    sd.resolver("GX");
    sd.agent_also(
        "mkdir -p .github/workflows && echo x > .github/workflows/new.yml && git add .github/workflows/new.yml\n",
    );
    let r = sd.run(&["restack", "--resolve"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.config("branch.feat.sd-held"), format!("{}\n.github/workflows/new.yml", sd.rev("feat")));
}

// @test "--resolve: a CI config file with a non-ASCII name is held under its own name (#124)"
#[test]
fn resolve_a_ci_config_file_with_a_non_ascii_name_is_held_under_its_own_name_124() {
    let sd = agent_sd();
    sd.upstream("main", "g", "X", "upstream");
    sd.resolver("GX");
    sd.agent_also("mkdir -p .github/workflows && echo x > .github/workflows/tëst.yml\n");
    let r = sd.run(&["restack", "--resolve"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.config("branch.feat.sd-held"), format!("{}\n.github/workflows/tëst.yml", sd.rev("feat")));
}

// @test "--resolve: a resolution that stays out of CI config holds nothing (#124)"
#[test]
fn resolve_a_resolution_that_stays_out_of_ci_config_holds_nothing_124() {
    let sd = agent_sd();
    sd.upstream("main", "g", "X", "upstream");
    sd.resolver("GX");
    let r = sd.run(&["restack", "--resolve"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.git_status(&["config", "--get", "branch.feat.sd-held"]), 1);
}

// @test "--resolve: settings.resolvePrompt replaces the guidance; the rebase boundaries stay"
#[test]
fn resolve_settings_resolveprompt_replaces_the_guidance_the_rebase_boundaries_stay() {
    let sd = agent_sd();
    sd.git(&["config", "branch.feat.sd-task", "capitalise g"]);
    sd.upstream("main", "g", "X", "upstream");
    sd.resolver("GX");
    fs::write(sd.tmp.join("resolve.md"), "Prefer the target.\n").unwrap();
    sd.write_global(&format!(r#"{{"settings": {{"resolvePrompt": "{}"}}}}"#, sd.path("resolve.md")));
    sd.ok(&["restack", "--resolve"]);
    let prompt = sd.fx("claude.prompt");
    assert_eq!(prompt.lines().next(), Some("Prefer the target."));
    assert!(!regex::Regex::new("keeps this branch.s intent").unwrap().is_match(&prompt));
    assert_has(&prompt, &["Do not run git add"]);
    assert_has(&prompt, &["already-on-target"]);
}

// @test "--resolve: the agent sees the conflicting files and hunks, in the branch's session, edit tools only"
#[test]
fn resolve_the_agent_sees_the_conflicting_files_and_hunks_in_the_branchs_session_edit_tools_only() {
    let sd = agent_sd();
    sd.git(&["config", "branch.feat.sd-task", "capitalise g"]);
    sd.upstream("main", "g", "X", "upstream");
    sd.resolver("GX");
    sd.ok(&["restack", "--resolve"]);
    let prompt = sd.fx("claude.prompt");
    assert_has(&prompt, &["file.txt"]);
    assert_has(&prompt, &["<<<<<<<"]);
    assert_has(&prompt, &["capitalise g"]);
    assert_eq!(sd.fx("claude.calls"), sd.path("repo"));
    let sid = sd.config("branch.feat.sd-session");
    assert!(!sid.is_empty());
    let args = sd.fx("claude.args");
    assert!(args.lines().any(|l| l == sid), "no {sid} in {args:?}");
    let allow = after(&args, "--allowedTools");
    assert!(allow.contains("Edit") && !allow.contains("git add") && !allow.contains("rebase"), "{allow}");
}

// @test "--resolve: a plain verify command and settings.implementAllow become rules, as for implement (#156)"
#[test]
fn resolve_a_plain_verify_command_and_settings_implementallow_become_rules_as_for_implement_156() {
    let sd = agent_sd();
    sd.write_global(r#"{"settings": {"verify": "grep -qx GX file.txt", "implementAllow": ["Bash(bats *)", "WebFetch"]}}"#);
    sd.upstream("main", "g", "X", "upstream");
    sd.resolver("GX");
    let r = sd.run(&["restack", "--resolve"]);
    assert_eq!(r.status, 0);
    let allow = after(&sd.fx("claude.args"), "--allowedTools");
    assert_has(&format!(",{allow},"), &[",Bash(grep -qx GX file.txt),Bash(bats *),WebFetch,"]);
    let prompt = sd.fx("claude.prompt");
    assert_has(&prompt, &["The repo's checks, which the pipeline runs after you: grep -qx GX file.txt"]);
    assert_has(&prompt, &["may run only these"]);
}

// @test "--resolve: with nothing allowed, the prompt says the agent can't run commands"
#[test]
fn resolve_with_nothing_allowed_the_prompt_says_the_agent_cant_run_commands() {
    let sd = agent_sd();
    sd.upstream("main", "g", "X", "upstream");
    sd.resolver("GX");
    let r = sd.run(&["restack", "--resolve"]);
    assert_eq!(r.status, 0);
    assert_has(&sd.fx("claude.prompt"), &["can't run commands"]);
}

// @test "--resolve: the agent is told which commit it is replaying, out of which"
#[test]
fn resolve_the_agent_is_told_which_commit_it_is_replaying_out_of_which() {
    let sd = agent_sd();
    sd.edit("b", "B", "feat two");
    sd.upstream("main", "g", "X", "upstream");
    sd.resolver("GX");
    let own1 = sd.git(&["rev-parse", "--short", "feat~1"]);
    let own2 = sd.git(&["rev-parse", "--short", "feat"]);
    sd.ok(&["restack", "--resolve"]);
    let prompt = sd.fx("claude.prompt");
    let replaying = format!("Replaying: {own1} feat");
    assert!(prompt.lines().any(|l| l == replaying), "{prompt}");
    assert!(prompt.lines().any(|l| l == " file.txt | 2 +-"), "{prompt}");
    assert_eq!(
        sed_range(&prompt, "Commits being replayed, oldest first:"),
        format!("Commits being replayed, oldest first:\n{own1} feat\n{own2} feat two")
    );
}

// @test "--resolve: the agent saying the change is already on the target rolls back as a wrong base"
#[test]
fn resolve_the_agent_saying_the_change_is_already_on_the_target_rolls_back_as_a_wrong_base() {
    let sd = agent_sd();
    sd.upstream("main", "g", "X", "upstream");
    sd.result(r#"{"outcome": "already-on-target", "summary": "main already has this"}"#);
    let before = sd.rev("HEAD");
    let replayed = sd.git(&["rev-parse", "--short", "HEAD"]);
    let r = sd.run(&["restack", "--resolve"]);
    assert_eq!(r.status, 1);
    let wrong = format!("wrong base: the agent says {replayed} feat is already on origin/main (main already has this)");
    assert_has(&r.output, &[&wrong]);
    assert_has(&r.output, &[&format!("rebase past it by hand: git rebase --onto origin/main {replayed} feat")]);
    assert_eq!(sd.rev("HEAD"), before);
    assert!(!sd.rebase_in_progress());
    assert_eq!(sd.base_pin(), "");
}

// @test "--resolve: a conflict partway through goes to the agent too"
#[test]
fn resolve_a_conflict_partway_through_goes_to_the_agent_too() {
    let sd = agent_sd();
    sd.edit("e", "E", "there");
    sd.edit("E", "e", "back");
    sd.upstream("main", "e", "X", "upstream");
    sd.resolver("X");
    let r = sd.run(&["restack", "--resolve"]);
    assert_eq!(r.status, 0);
    // both stops resolved to main's line, so git dropped the two now-empty commits
    assert_eq!(sd.fx("claude.calls").lines().count(), 2);
    assert_eq!(sd.rev("feat^"), sd.rev("origin/main"));
    assert!(sd.file_has("X") && sd.file_has("G"));
    assert!(!sd.rebase_in_progress());
}

// @test "--resolve: one running marker lasts across every stop, then is gone (#170)"
#[test]
fn resolve_one_running_marker_lasts_across_every_stop_then_is_gone_170() {
    let sd = agent_sd();
    sd.edit("e", "E", "there");
    sd.edit("E", "e", "back");
    sd.upstream("main", "e", "X", "upstream");
    sd.resolver("X");
    let marker = sd.tmp.join("repo/.git/sheepdog/feat.running");
    sd.agent_also(&format!("cat {} >> {}\n", marker.display(), sd.path("fx/during")));
    let r = sd.run(&["restack", "--resolve"]);
    assert_eq!(r.status, 0);
    let during = sd.fx("during");
    assert_eq!(during.lines().count(), 2);
    let ids: BTreeSet<Vec<&str>> = during.lines().map(|l| l.split(' ').take(2).collect()).collect();
    assert_eq!(ids.len(), 1);
    assert_eq!(during.lines().next().unwrap().split(' ').nth(1), Some("resolve"));
    assert!(!marker.exists());
}

// @test "--resolve: the agent giving up rolls back"
#[test]
fn resolve_the_agent_giving_up_rolls_back() {
    let sd = agent_sd();
    sd.upstream("main", "g", "X", "upstream");
    sd.result(r#"{"outcome": "gave-up", "summary": "no sensible merge"}"#);
    let before = sd.rev("HEAD");
    let r = sd.run(&["restack", "--resolve"]);
    assert_eq!(r.status, 1);
    assert_has(&r.output, &["gave up"]);
    assert_has(&r.output, &["no sensible merge"]);
    assert_eq!(sd.rev("HEAD"), before);
    assert!(!sd.rebase_in_progress());
    assert_eq!(sd.git(&["status", "--porcelain"]), "");
    assert_eq!(sd.base_pin(), "");
}

#[test]
fn resolve_a_give_up_stands_until_the_target_moves_257() {
    let sd = agent_sd();
    sd.upstream("main", "g", "X", "upstream");
    sd.result(r#"{"outcome": "gave-up", "summary": "no sensible merge"}"#);
    let r = sd.run(&["restack", "--resolve"]);
    assert_eq!(r.status, 1);
    assert_has(&r.output, &["until feat's HEAD or worktree, or origin/main, changes"]);
    let q: serde_json::Value = serde_json::from_str(&sd.git(&["cat-file", "blob", "refs/sheepdog/feat/question"])).unwrap();
    assert_eq!(q["step"], "resolve");
    assert_eq!(q["kind"], "gave-up");
    assert_eq!(q["text"], "no sensible merge");
    assert_eq!(q["onto"], sd.rev("origin/main"));

    let r = sd.run(&["restack", "--resolve"]);
    assert_eq!(r.status, 1);
    assert_has(&r.output, &["not running resolve on feat: its agent gave up", "Resolve it by hand (sd act restack says how)"]);
    assert_eq!(sd.fx("claude.calls").lines().count(), 1);

    sd.upstream("main", "a", "Z", "more upstream");
    sd.run(&["restack", "--resolve"]);
    assert_eq!(sd.fx("claude.calls").lines().count(), 2);
}

#[test]
fn resolve_force_runs_past_a_standing_give_up_257() {
    let sd = agent_sd();
    sd.upstream("main", "g", "X", "upstream");
    sd.result(r#"{"outcome": "gave-up", "summary": "no sensible merge"}"#);
    sd.run(&["restack", "--resolve"]);
    sd.resolver("GX");
    sd.result(r#"{"outcome": "done", "summary": "kept both"}"#);
    let r = sd.run(&["restack", "--resolve", "--force"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.fx("claude.calls").lines().count(), 2);
}

#[test]
fn resolve_every_stop_the_agent_caused_is_a_give_up_257() {
    let wrong_base = agent_sd();
    wrong_base.upstream("main", "g", "X", "upstream");
    wrong_base.result(r#"{"outcome": "already-on-target", "summary": "main already has this"}"#);
    let markers = agent_sd();
    markers.upstream("main", "g", "X", "upstream");
    markers.agent("#!/usr/bin/env bash\ntrue\n");
    for (sd, text) in [(&wrong_base, "wrong base: the agent says"), (&markers, "conflict markers left in: file.txt")] {
        assert_eq!(sd.run(&["restack", "--resolve"]).status, 1);
        let q = sd.git(&["cat-file", "blob", "refs/sheepdog/feat/question"]);
        assert_has(&q, &[r#""step":"resolve""#, text, r#""kind":"gave-up""#]);
    }
}

#[test]
fn resolve_a_failed_agent_step_is_no_give_up_257() {
    let sd = agent_sd();
    sd.upstream("main", "g", "X", "upstream");
    fs::write(sd.tmp.join("fx/fail"), "").unwrap();
    assert_eq!(sd.run(&["restack", "--resolve"]).status, 1);
    assert_eq!(sd.git(&["for-each-ref", "refs/sheepdog/feat/question"]), "");
}

// @test "--resolve: leftover conflict markers roll back"
#[test]
fn resolve_leftover_conflict_markers_roll_back() {
    let sd = agent_sd();
    sd.upstream("main", "g", "X", "upstream");
    sd.agent("#!/usr/bin/env bash\ntrue\n");
    let before = sd.rev("HEAD");
    let r = sd.run(&["restack", "--resolve"]);
    assert_eq!(r.status, 1);
    assert_has(&r.output, &["conflict markers"]);
    assert_eq!(sd.rev("HEAD"), before);
    assert!(!sd.rebase_in_progress());
}

// @test "--resolve: the agent step failing rolls back"
#[test]
fn resolve_the_agent_step_failing_rolls_back() {
    let sd = agent_sd();
    sd.upstream("main", "g", "X", "upstream");
    fs::write(sd.tmp.join("fx/fail"), "").unwrap();
    let before = sd.rev("HEAD");
    let r = sd.run(&["restack", "--resolve"]);
    assert_eq!(r.status, 1);
    assert_eq!(sd.rev("HEAD"), before);
    assert!(!sd.rebase_in_progress());
}

// @test "--resolve: verify runs on the result; a failure keeps the rebased branch and says so"
#[test]
fn resolve_verify_runs_on_the_result_a_failure_keeps_the_rebased_branch_and_says_so() {
    let sd = agent_sd();
    sd.verb_ok("config", &["set", "--global", "settings.verify", "grep -qx nope file.txt"]);
    sd.upstream("main", "g", "X", "upstream");
    sd.resolver("GX");
    let r = sd.run(&["restack", "--resolve"]);
    assert_eq!(r.status, 1);
    assert_has(&r.output, &["verify failed"]);
    assert_eq!(sd.rev("feat^"), sd.rev("origin/main"));
    assert_eq!(sd.base_pin(), sd.rev("origin/main"));
}

// @test "--resolve: more than 3 stops gives up and rolls back"
#[test]
fn resolve_more_than_3_stops_gives_up_and_rolls_back() {
    let sd = agent_sd();
    sd.edit("a", "A", "a1");
    sd.edit("b", "B", "b1");
    sd.edit("c", "C", "c1");
    sd.edit("d", "D", "d1");
    sd.upstream_all("main", &[("a", "1"), ("b", "2"), ("c", "3"), ("d", "4")], "upstream");
    sd.resolver("R");
    let before = sd.rev("HEAD");
    let r = sd.run(&["restack", "--resolve"]);
    assert_eq!(r.status, 1);
    assert_has(&r.output, &["stopped 4 times"]);
    assert_eq!(sd.rev("HEAD"), before);
    assert!(!sd.rebase_in_progress());
}

// @test "--resolve with nothing to resolve is a plain restack, no agent"
#[test]
fn resolve_with_nothing_to_resolve_is_a_plain_restack_no_agent() {
    let sd = agent_sd();
    sd.upstream("main", "b", "B", "upstream");
    let r = sd.run(&["restack", "--resolve"]);
    assert_eq!(r.status, 0);
    assert!(!sd.fx_exists("claude.calls"));
    assert_eq!(sd.rev("feat^"), sd.rev("origin/main"));
}
