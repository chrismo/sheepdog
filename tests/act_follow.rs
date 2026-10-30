// Black-box tests of sd act follow (#250: reviewer mode): a teammate's PR,
// fetched by number from refs/pull/<n>/head, on a branch review/<n> in a
// worktree beside the main one, marked as a reviewer's branch and enrolled;
// sd act fetch, which takes the author's pushes; and the author's verbs,
// which a reviewer's branch refuses.
//
// Real git (a bare origin, our clone at repo/, the teammate's at other/). The
// teammate's PR is refs/pull/7/head on origin, as GitHub keeps one for every
// PR, a fork's included. gh is a stub (GH below) serving that PR by number.

mod common;

use common::{assert_has, assert_lacks, dead_pid, jq, running_marker, step_refusal, Holder, Sd};
use std::fs;

// By number, PR #<n> from a fork's their-fix, its head origin's
// refs/pull/<n>/head, as GitHub has it; $GH/reviews its reviews, if any.
const GH: &str = r#"#!/usr/bin/env bash
printf '%s\n' "$*" >> "$GH/calls"
n=
for a; do case $a in number=*) n=${a#number=} ;; esac; done
if [[ -n $n ]]; then
  head=$(git ls-remote origin "refs/pull/$n/head" | cut -f1)
  jq -n --argjson n "$n" --arg h "$head" --argjson r "$(cat "$GH/reviews" 2>/dev/null || echo '[]')" '{data: {viewer: {login: "me"},
    repository: {defaultBranchRef: {name: "main"}, pullRequest: {number: $n, url: "u", state: "OPEN", isDraft: false,
      isCrossRepository: true, headRepositoryOwner: {login: "teammate"}, headRefName: "their-fix", baseRefName: "main",
      headRefOid: $h, mergeable: "MERGEABLE", mergeStateStatus: "CLEAN", reviewDecision: null, reviewRequests: {nodes: []},
      commits: {nodes: []}, reviewThreads: {nodes: []}, reviews: {nodes: $r}, comments: {nodes: []}}}}}'
  exit
fi
echo '{"data":{"viewer":{"login":"me"},"repository":{"defaultBranchRef":{"name":"main"},"pullRequests":{"nodes":[]}}}}'
"#;

/// origin.git with file.txt on main, cloned to repo (cwd) and other; the
/// teammate's commit on their-fix, pushed only as PR #7's head ref.
fn sd() -> Sd {
    let mut sd = Sd::for_verb("act");
    sd.env = vec![("SHEEPDOG_STATE_DIR".into(), sd.path("state")), ("GH".into(), sd.path("gh"))];
    fs::create_dir(sd.tmp.join("gh")).unwrap();
    sd.fake("gh", GH);
    sd.origin(&["repo"]);
    sd.cd("repo");
    sd.write("file.txt", "a\nb\n");
    sd.git(&["add", "file.txt"]);
    sd.git(&["commit", "-qm", "init"]);
    sd.git(&["push", "-q", "origin", "main"]);
    sd.git(&["clone", "-q", &sd.path("origin.git"), &sd.path("other")]);
    sd.git_in("other", &["switch", "-qc", "their-fix"]);
    sd.git_in("other", &["commit", "-q", "--allow-empty", "-m", "their fix"]);
    sd.git_in("other", &["push", "-q", "origin", "HEAD:refs/pull/7/head"]);
    sd
}

#[test]
fn follow_no_number_is_usage_nonzero() {
    let sd = sd();
    for args in [&["follow"][..], &["follow", ""], &["follow", "feat"], &["follow", "7", "8"]] {
        let r = sd.run(args);
        assert_ne!(r.status, 0, "{args:?}");
        assert_has(&r.output, &["usage: sd act follow <number>"]);
    }
}

#[test]
fn follow_a_worktree_beside_the_main_one_on_review_n_at_the_prs_head_stdout_is_only_its_path() {
    let sd = sd();
    let r = sd.run_sep(&["follow", "7"]);
    assert_eq!(r.status, 0, "{}", r.stderr);
    assert_eq!(r.output, sd.path("review-7"));
    assert_eq!(sd.git_in("review-7", &["branch", "--show-current"]), "review/7");
    let theirs = sd.git_in("other", &["rev-parse", "HEAD"]);
    assert_eq!(sd.git_in("review-7", &["rev-parse", "HEAD"]), theirs);
    assert_eq!(sd.git(&["rev-parse", "refs/sheepdog/review/7/pr-head"]), theirs);
}

#[test]
fn follow_records_the_pr_number_and_reviewer_mode_and_enrolls() {
    let sd = sd();
    let r = sd.run_sep(&["follow", "7"]);
    assert_eq!(r.status, 0, "{}", r.stderr);
    assert_eq!(sd.git(&["config", "--get", "branch.review/7.sd-pr"]), "7");
    assert_eq!(sd.git(&["config", "--get", "branch.review/7.sd-mode"]), "reviewer");
    assert_eq!(sd.git(&["config", "--get", "branch.review/7.sheepdog"]), "true");
    assert_has(&r.stderr, &["enrolled review/7"]);
}

#[test]
fn follow_a_pr_the_remote_has_no_head_for_fails_and_makes_nothing() {
    let sd = sd();
    let r = sd.run(&["follow", "9"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["no PR #9 on origin"]);
    assert!(!sd.tmp.join("review-9").exists());
    assert!(sd.git_try(&["rev-parse", "--verify", "-q", "refs/heads/review/9"]).is_none());
}

impl Sd {
    /// .git/worktree-setup, logging each run's branch to hook.log.
    fn hook(&self) {
        let log = self.path("hook.log");
        self.script(&self.tmp.join("repo/.git/worktree-setup"), &format!("#!/usr/bin/env bash\necho \"$WT_BRANCH\" >> '{log}'\n"));
    }

    /// The branches the setup hook ran for, one a line.
    fn hook_runs(&self) -> String {
        fs::read_to_string(self.tmp.join("hook.log")).unwrap_or_default()
    }

    /// Everything sheepdog keeps for <b>: its config and its refs.
    fn kept(&self, b: &str) -> String {
        let cfg = self.git_try(&["config", "--get-regexp", &format!("^branch\\.{}\\.", b.replace('/', "\\/"))]).unwrap_or_default();
        format!("{cfg}\n{}", self.git(&["for-each-ref", &format!("refs/sheepdog/{b}/")]))
    }

    /// PR #7's head fetched to <b>, in a worktree at <dir>, as gh pr checkout
    /// makes a fork's PR: on the head's branch name, tracking refs/pull/7/head.
    fn checkout_by_hand(&self, b: &str, dir: &str) {
        self.git(&["fetch", "-q", "origin", &format!("refs/pull/7/head:{b}")]);
        self.git(&["config", &format!("branch.{b}.remote"), "origin"]);
        self.git(&["config", &format!("branch.{b}.merge"), "refs/pull/7/head"]);
        self.git(&["worktree", "add", "-q", &self.path(dir), b]);
    }
}

#[test]
fn follow_again_reuses_its_worktree_and_moves_nothing_no_setup_hook() {
    let mut sd = sd();
    sd.hook();
    let path = sd.out(&["follow", "7"]);
    sd.cd("review-7");
    sd.git(&["commit", "-q", "--allow-empty", "-m", "mine"]);
    let mine = sd.git(&["rev-parse", "HEAD"]);
    fs::write(sd.cwd.join("file.txt"), "mine\n").unwrap();
    let pr_head = sd.git(&["rev-parse", "refs/sheepdog/review/7/pr-head"]);
    sd.author_pushes("more", false);
    for from in ["review-7", "repo"] {
        sd.cd(from);
        let r = sd.run_sep(&["follow", "7"]);
        assert_eq!(r.status, 0, "{}", r.stderr);
        assert_eq!(r.output, path);
        assert_has(&r.stderr, &[&format!("sd act: review/7 is already checked out at {path}; using it"), "enrolled review/7"]);
    }
    assert_eq!(sd.hook_runs(), "review/7\n");
    assert_eq!(sd.git_in("review-7", &["rev-parse", "HEAD"]), mine);
    assert_eq!(fs::read_to_string(sd.tmp.join("review-7/file.txt")).unwrap(), "mine\n");
    assert_eq!(sd.git(&["rev-parse", "refs/sheepdog/review/7/pr-head"]), pr_head);
}

#[test]
fn follow_again_keeps_the_review_state() {
    let mut sd = sd();
    sd.script(&sd.tmp.join("reviewer"), "#!/usr/bin/env bash\necho '[]'\n");
    sd.verb_ok("config", &["set", "--global", "settings.localReview", &sd.path("reviewer")]);
    sd.followed();
    sd.ok(&["review"]);
    let kept = sd.kept("review/7");
    assert_has(&kept, &["branch.review/7.sd-mode reviewer", "refs/sheepdog/review/7/local", "refs/sheepdog/review/7/pr-head"]);
    sd.ok(&["follow", "7"]);
    assert_eq!(sd.kept("review/7"), kept);
}

#[test]
fn follow_adopts_the_prs_checkout_it_runs_in_any_branch_name_any_path() {
    let mut sd = sd();
    sd.hook();
    sd.checkout_by_hand("their-fix", "elsewhere/their-fix");
    sd.cd("elsewhere/their-fix");
    fs::write(sd.cwd.join("file.txt"), "mine\n").unwrap();
    let r = sd.run_sep(&["follow", "7"]);
    assert_eq!(r.status, 0, "{}", r.stderr);
    assert_eq!(r.output, sd.path("elsewhere/their-fix"));
    assert_has(&r.stderr, &["enrolled their-fix"]);
    assert_eq!(sd.git(&["config", "--get", "branch.their-fix.sd-pr"]), "7");
    assert_eq!(sd.git(&["config", "--get", "branch.their-fix.sd-mode"]), "reviewer");
    assert_eq!(sd.git(&["config", "--get", "branch.their-fix.sheepdog"]), "true");
    let theirs = sd.git_in("other", &["rev-parse", "HEAD"]);
    assert_eq!(sd.git(&["rev-parse", "refs/sheepdog/their-fix/pr-head"]), theirs);
    assert_eq!(fs::read_to_string(sd.cwd.join("file.txt")).unwrap(), "mine\n");
    assert_eq!(sd.hook_runs(), "");
    assert!(!sd.tmp.join("review-7").exists());
    assert!(sd.git_try(&["rev-parse", "--verify", "-q", "refs/heads/review/7"]).is_none());

    sd.cd("repo");
    assert_eq!(sd.out(&["follow", "7"]), sd.path("elsewhere/their-fix"));
    sd.cd("elsewhere/their-fix");
    sd.git(&["checkout", "-q", "file.txt"]);
    let theirs = sd.author_pushes("more", false);
    sd.ok(&["fetch"]);
    assert_eq!(sd.git(&["rev-parse", "HEAD"]), theirs);
}

#[test]
fn follow_adopts_a_checkout_behind_the_pr_or_diverged_from_it_and_moves_neither() {
    let mut sd = sd();
    sd.checkout_by_hand("behind", "behind");
    sd.checkout_by_hand("ahead", "ahead");
    let old = sd.git(&["rev-parse", "behind"]);
    sd.git_in("ahead", &["commit", "-q", "--allow-empty", "-m", "mine"]);
    let mine = sd.git(&["rev-parse", "ahead"]);
    let theirs = sd.author_pushes("more", false);

    sd.cd("behind");
    sd.ok(&["follow", "7"]);
    assert_eq!(sd.git(&["rev-parse", "HEAD"]), old);
    assert_eq!(sd.git(&["rev-parse", "refs/sheepdog/behind/pr-head"]), old);
    sd.ok(&["fetch"]);
    assert_eq!(sd.git(&["rev-parse", "HEAD"]), theirs);

    // PR #7 is followed now; ahead's is #8, the same commits, and ahead and
    // the PR have each moved on from old.
    sd.git_in("other", &["push", "-q", "origin", "HEAD:refs/pull/8/head"]);
    sd.git(&["config", "branch.ahead.merge", "refs/pull/8/head"]);
    sd.cd("ahead");
    sd.ok(&["follow", "8"]);
    assert_eq!(sd.git(&["rev-parse", "HEAD"]), mine);
    assert_eq!(sd.git(&["rev-parse", "refs/sheepdog/ahead/pr-head"]), old);
    let r = sd.run(&["fetch"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["ahead has commits the PR never had"]);
}

#[test]
fn follow_adopts_a_same_repo_prs_branch_checked_out_tracking_it() {
    let mut sd = sd();
    sd.git_in("other", &["push", "-q", "origin", "their-fix"]);
    sd.git(&["fetch", "-q", "origin"]);
    sd.git(&["worktree", "add", "-q", "--track", "-b", "their-fix", &sd.path("their-fix"), "origin/their-fix"]);
    sd.cd("their-fix");
    let r = sd.run_sep(&["follow", "7"]);
    assert_eq!(r.status, 0, "{}", r.stderr);
    assert_eq!(r.output, sd.path("their-fix"));
    assert_eq!(sd.git(&["config", "--get", "branch.their-fix.sd-mode"]), "reviewer");
}

#[test]
fn follow_adopts_a_fork_prs_checkout_tracking_the_forks_branch_by_url_behind_the_pr() {
    let mut sd = sd();
    let fork = sd.path("fork.git");
    sd.git(&["init", "-q", "--bare", &fork]);
    sd.git_in("other", &["push", "-q", &fork, "their-fix"]);
    sd.git(&["fetch", "-q", &fork, "their-fix:their-fix"]);
    sd.git(&["config", "branch.their-fix.remote", &fork]);
    sd.git(&["config", "branch.their-fix.merge", "refs/heads/their-fix"]);
    sd.git(&["worktree", "add", "-q", &sd.path("their-fix"), "their-fix"]);
    let theirs = sd.author_pushes("more", false);
    sd.git_in("other", &["push", "-q", &fork, "their-fix"]);
    sd.cd("their-fix");
    let r = sd.run_sep(&["follow", "7"]);
    assert_eq!(r.status, 0, "{}", r.stderr);
    assert_eq!(r.output, sd.path("their-fix"));
    sd.ok(&["fetch"]);
    assert_eq!(sd.git(&["rev-parse", "HEAD"]), theirs);
}

#[test]
fn follow_says_nothing_of_a_review_branch_checked_out_here_and_takes_it() {
    let mut sd = sd();
    sd.git(&["fetch", "-q", "origin", "refs/pull/7/head:review/7"]);
    sd.git(&["worktree", "add", "-q", &sd.path("review-7"), "review/7"]);
    sd.cd("review-7");
    let r = sd.run_sep(&["follow", "7"]);
    assert_eq!(r.status, 0, "{}", r.stderr);
    assert_eq!(r.output, sd.path("review-7"));
    assert_lacks(&r.stderr, "isn't PR #7's");
}

impl Sd {
    /// main gone, here and on origin: no default branch to tell by.
    fn no_default_branch(&self) {
        self.git(&["branch", "-m", "main", "trunk"]);
        self.git(&["update-ref", "-d", "refs/remotes/origin/main"]);
        self.git(&["update-ref", "-d", "refs/remotes/origin/HEAD"]);
        self.git_in("origin.git", &["symbolic-ref", "HEAD", "refs/heads/trunk"]);
        self.git_in("origin.git", &["update-ref", "-d", "refs/heads/main"]);
    }
}

#[test]
fn follow_with_no_default_branch_adopts_the_checkout_tracking_the_pr_and_refuses_a_review_branch_past_its_head() {
    let mut sd = sd();
    sd.checkout_by_hand("their-fix", "their-fix");
    sd.git(&["fetch", "-q", "origin", "refs/pull/7/head:review/7"]);
    sd.git(&["update-ref", "refs/heads/review/7", &sd.git(&["commit-tree", "-p", "review/7", "-m", "mine", "review/7^{tree}"])]);
    sd.no_default_branch();
    let r = sd.run(&["follow", "7"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["review/7 already exists, and with no default branch to tell by, follow can't say it's PR #7's"]);
    sd.cd("their-fix");
    assert_eq!(sd.out(&["follow", "7"]), sd.path("their-fix"));
}

#[test]
fn follow_with_no_default_branch_takes_a_review_branch_at_the_prs_head() {
    let sd = sd();
    sd.git(&["fetch", "-q", "origin", "refs/pull/7/head:review/7"]);
    sd.no_default_branch();
    assert_eq!(sd.out(&["follow", "7"]), sd.path("review-7"));
}

#[test]
fn follow_adopting_a_checkout_with_no_history_in_common_with_the_pr_drops_a_left_over_pr_head() {
    let mut sd = sd();
    sd.checkout_by_hand("their-fix", "their-fix");
    sd.git(&["update-ref", "refs/sheepdog/their-fix/pr-head", "main"]);
    sd.cd("their-fix");
    let unrelated = sd.git(&["commit-tree", "-m", "unrelated", "HEAD^{tree}"]);
    sd.git(&["reset", "-q", "--hard", &unrelated]);
    sd.ok(&["follow", "7"]);
    assert!(sd.git_try(&["rev-parse", "--verify", "-q", "refs/sheepdog/their-fix/pr-head"]).is_none());
}

#[test]
fn follow_from_the_branch_a_stacked_pr_is_on_makes_review_n_and_leaves_the_base_alone() {
    let mut sd = sd();
    sd.git_in("other", &["switch", "-qc", "base", "main"]);
    sd.git_in("other", &["commit", "-q", "--allow-empty", "-m", "the base"]);
    sd.git_in("other", &["push", "-q", "origin", "base"]);
    sd.git_in("other", &["switch", "-qc", "on-base"]);
    sd.git_in("other", &["commit", "-q", "--allow-empty", "-m", "on the base"]);
    sd.git_in("other", &["push", "-q", "-f", "origin", "HEAD:refs/pull/7/head"]);
    sd.git(&["fetch", "-q", "origin"]);
    sd.git(&["worktree", "add", "-q", "--track", "-b", "base", &sd.path("base"), "origin/base"]);
    sd.cd("base");
    let r = sd.run_sep(&["follow", "7"]);
    assert_eq!(r.status, 0, "{}", r.stderr);
    assert_eq!(r.output, sd.path("review-7"));
    assert_has(&r.stderr, &["sd act: not taking base for PR #7: its upstream, if any, isn't at the PR's head as its remote has it"]);
    assert!(sd.git_try(&["config", "--get", "branch.base.sd-mode"]).is_none());
}

#[test]
fn follow_makes_review_n_from_scratch_whatever_pr_head_was_left_behind() {
    let sd = sd();
    sd.git(&["update-ref", "refs/sheepdog/review/7/pr-head", "main"]);
    sd.ok(&["follow", "7"]);
    assert_eq!(sd.git(&["rev-parse", "refs/sheepdog/review/7/pr-head"]), sd.git_in("other", &["rev-parse", "HEAD"]));
}

#[test]
fn follow_from_an_unrelated_worktree_makes_review_n_and_leaves_that_branch_alone() {
    let mut sd = sd();
    sd.git(&["worktree", "add", "-q", "-b", "feat", &sd.path("feat"), "main"]);
    sd.cd("feat");
    sd.git(&["commit", "-q", "--allow-empty", "-m", "my feature"]);
    let r = sd.run_sep(&["follow", "7"]);
    assert_eq!(r.status, 0, "{}", r.stderr);
    assert_eq!(r.output, sd.path("review-7"));
    assert!(sd.git_try(&["config", "--get", "branch.feat.sd-mode"]).is_none());
    assert!(sd.git_try(&["config", "--get", "branch.feat.sd-pr"]).is_none());
}

#[test]
fn follow_refuses_the_prs_checkout_when_its_branch_is_already_another_prs_or_an_authors() {
    let mut sd = sd();
    sd.checkout_by_hand("their-fix", "their-fix");
    sd.cd("their-fix");
    for (k, v, says) in [
        ("sd-pr", "9", "their-fix is PR #9's"),
        ("sd-mode", "reviewer", "their-fix already follows PR #9"),
    ] {
        sd.git(&["config", &format!("branch.their-fix.{k}"), v]);
        let r = sd.run(&["follow", "7"]);
        assert_ne!(r.status, 0, "{k}");
        assert_has(&r.output, &[says]);
        assert_eq!(sd.git(&["config", "--get", "branch.their-fix.sd-pr"]), "9");
    }
    sd.git(&["config", "--unset", "branch.their-fix.sd-pr"]);
    sd.git(&["config", "--unset", "branch.their-fix.sd-mode"]);
    sd.git(&["config", "branch.their-fix.sd-task", "a task of mine"]);
    let r = sd.run(&["follow", "7"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["their-fix is an author's branch (sd act start or enroll)"]);
    assert!(sd.git_try(&["config", "--get", "branch.their-fix.sd-mode"]).is_none());
    assert!(!sd.tmp.join("review-7").exists());
}

#[test]
fn follow_takes_a_review_branch_with_no_worktree_and_runs_the_setup_hook_for_the_new_one() {
    let sd = sd();
    sd.hook();
    sd.ok(&["follow", "7"]);
    sd.git_in("review-7", &["commit", "-q", "--allow-empty", "-m", "mine"]);
    let mine = sd.git(&["rev-parse", "review/7"]);
    sd.git(&["worktree", "remove", &sd.path("review-7")]);
    let r = sd.run_sep(&["follow", "7"]);
    assert_eq!(r.status, 0, "{}", r.stderr);
    assert_eq!(r.output, sd.path("review-7"));
    assert_eq!(sd.git_in("review-7", &["rev-parse", "HEAD"]), mine);
    assert_eq!(sd.hook_runs(), "review/7\nreview/7\n");
}

#[test]
fn follow_takes_a_review_branch_made_by_hand_at_the_prs_head() {
    let sd = sd();
    sd.git(&["fetch", "-q", "origin", "refs/pull/7/head:review/7"]);
    let r = sd.run_sep(&["follow", "7"]);
    assert_eq!(r.status, 0, "{}", r.stderr);
    assert_eq!(r.output, sd.path("review-7"));
    assert_eq!(sd.git(&["config", "--get", "branch.review/7.sd-mode"]), "reviewer");
    assert_eq!(sd.git(&["rev-parse", "refs/sheepdog/review/7/pr-head"]), sd.git_in("other", &["rev-parse", "HEAD"]));
}

#[test]
fn follow_refuses_a_review_branch_that_isnt_the_prs() {
    let sd = sd();
    sd.git(&["branch", "review/7"]);
    let r = sd.run(&["follow", "7"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["review/7 already exists and isn't PR #7's"]);
    assert!(!sd.tmp.join("review-7").exists());
    assert!(sd.git_try(&["config", "--get", "branch.review/7.sd-mode"]).is_none());
}

#[test]
fn follow_refuses_a_destination_that_exists() {
    let sd = sd();
    fs::create_dir(sd.tmp.join("review-7")).unwrap();
    let r = sd.run(&["follow", "7"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &[&format!("{} already exists", sd.path("review-7"))]);
    assert!(sd.git_try(&["rev-parse", "--verify", "-q", "refs/heads/review/7"]).is_none());
}

#[test]
fn follow_refuses_a_worktree_registered_for_the_branch_whose_directory_is_gone() {
    let sd = sd();
    sd.ok(&["follow", "7"]);
    fs::remove_dir_all(sd.tmp.join("review-7")).unwrap();
    let r = sd.run_sep(&["follow", "7"]);
    assert_ne!(r.status, 0);
    assert!(r.output.is_empty(), "{}", r.output);
    assert_has(&r.stderr, &[&sd.path("review-7"), "git worktree prune"]);
}

#[test]
fn follow_refuses_a_pr_followed_on_two_branches() {
    let sd = sd();
    sd.ok(&["follow", "7"]);
    sd.git(&["branch", "again", "review/7"]);
    sd.git(&["config", "branch.again.sd-pr", "7"]);
    sd.git(&["config", "branch.again.sd-mode", "reviewer"]);
    let r = sd.run(&["follow", "7"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["PR #7 is followed on more than one branch: review/7 again"]);
}

#[test]
fn a_reviewers_branch_refuses_the_authors_verbs() {
    let sd = sd();
    sd.ok(&["follow", "7"]);
    let head = sd.git(&["rev-parse", "review/7"]);
    for verb in ["implement", "address", "ci-fix", "restack", "retarget", "push", "ship", "describe", "trigger", "autofix"] {
        let r = sd.run(&[verb, "review/7"]);
        assert_ne!(r.status, 0, "{verb}");
        assert_has(&r.output, &[&format!("review/7 is a PR you're reviewing (sd act follow): sd act {verb} is the author's")]);
    }
    let r = sd.run(&["answer", "yes", "review/7"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["sd act answer is the author's"]);
    assert_eq!(sd.git(&["rev-parse", "review/7"]), head);
    assert!(sd.git_try(&["rev-parse", "--verify", "-q", "refs/remotes/origin/review/7"]).is_none());
}

impl Sd {
    /// The teammate commits <msg> and pushes it as PR #7's head; <amend>
    /// rewrites their last commit instead, a force-push. Its SHA.
    fn author_pushes(&self, msg: &str, amend: bool) -> String {
        let mut c = vec!["commit", "-q", "--allow-empty", "-m", msg];
        if amend {
            c.push("--amend");
        }
        self.git_in("other", &c);
        self.git_in("other", &["push", "-q", "-f", "origin", "HEAD:refs/pull/7/head"]);
        self.git_in("other", &["rev-parse", "HEAD"])
    }

    /// sd act follow 7, then cd into its worktree.
    fn followed(&mut self) {
        self.ok(&["follow", "7"]);
        self.cd("review-7");
    }
}

#[test]
fn fetch_takes_the_authors_push() {
    let mut sd = sd();
    sd.followed();
    let theirs = sd.author_pushes("more", false);
    let r = sd.run(&["fetch"]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_eq!(sd.git(&["rev-parse", "HEAD"]), theirs);
    assert_eq!(sd.git(&["rev-parse", "refs/sheepdog/review/7/pr-head"]), theirs);
    assert_has(&r.output, &[&format!("review/7 at the PR's head, {}", &theirs[..7])]);
}

#[test]
fn fetch_takes_the_authors_force_push_as_a_normal_event() {
    let mut sd = sd();
    sd.followed();
    let theirs = sd.author_pushes("their fix, amended", true);
    let r = sd.run(&["fetch"]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_eq!(sd.git(&["rev-parse", "HEAD"]), theirs);
}

#[test]
fn fetch_with_nothing_new_changes_nothing() {
    let mut sd = sd();
    sd.followed();
    let head = sd.git(&["rev-parse", "HEAD"]);
    let r = sd.run(&["fetch"]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_eq!(sd.git(&["rev-parse", "HEAD"]), head);
}

#[test]
fn fetch_refuses_a_dirty_worktree_and_moves_nothing() {
    let mut sd = sd();
    sd.followed();
    let head = sd.git(&["rev-parse", "HEAD"]);
    sd.author_pushes("more", false);
    fs::write(sd.cwd.join("file.txt"), "mine\n").unwrap();
    let r = sd.run(&["fetch"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["uncommitted changes"]);
    assert_eq!(sd.git(&["rev-parse", "HEAD"]), head);
}

#[test]
fn fetch_refuses_commits_on_the_branch_that_arent_the_authors() {
    let mut sd = sd();
    sd.followed();
    sd.git(&["commit", "-q", "--allow-empty", "-m", "mine"]);
    let mine = sd.git(&["rev-parse", "HEAD"]);
    sd.author_pushes("more", false);
    let r = sd.run(&["fetch"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["review/7 has commits the PR never had"]);
    assert_eq!(sd.git(&["rev-parse", "HEAD"]), mine);
}

#[test]
fn fetch_moves_a_followed_branch_checked_out_nowhere() {
    let mut sd = sd();
    sd.followed();
    sd.cd("repo");
    sd.git(&["worktree", "remove", &sd.path("review-7")]);
    let theirs = sd.author_pushes("more", false);
    let r = sd.run(&["fetch", "review/7"]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_eq!(sd.git(&["rev-parse", "review/7"]), theirs);
}

#[test]
fn fetch_that_cant_move_the_branch_leaves_the_pr_head_as_it_was_and_a_rerun_takes_the_force_push() {
    let mut sd = sd();
    sd.followed();
    let before = sd.git(&["rev-parse", "refs/sheepdog/review/7/pr-head"]);
    let theirs = sd.author_pushes("their fix, amended", true);
    let lock = sd.tmp.join("repo/.git/refs/heads/review/7.lock");
    fs::write(&lock, "").unwrap();
    let r = sd.run(&["fetch"]);
    assert_ne!(r.status, 0);
    assert_eq!(sd.git(&["rev-parse", "refs/sheepdog/review/7/pr-head"]), before);
    fs::remove_file(&lock).unwrap();
    let r = sd.run(&["fetch"]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_eq!(sd.git(&["rev-parse", "HEAD"]), theirs);
}

#[test]
fn fetch_with_no_pr_head_recorded_takes_a_head_the_branch_is_in() {
    let mut sd = sd();
    sd.followed();
    sd.git(&["update-ref", "-d", "refs/sheepdog/review/7/pr-head"]);
    let theirs = sd.author_pushes("more", false);
    let r = sd.run(&["fetch"]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_eq!(sd.git(&["rev-parse", "HEAD"]), theirs);
    assert_eq!(sd.git(&["rev-parse", "refs/sheepdog/review/7/pr-head"]), theirs);
}

#[test]
fn follow_whose_worktree_add_fails_leaves_no_reviewer_config() {
    let sd = sd();
    sd.git(&["branch", "review"]);
    let r = sd.run(&["follow", "7"]);
    assert_ne!(r.status, 0);
    assert!(sd.git_try(&["config", "--get", "branch.review/7.sd-mode"]).is_none());
    assert!(sd.git_try(&["config", "--get", "branch.review/7.sd-pr"]).is_none());
}

#[test]
fn fetch_leaves_the_remotes_other_refs_unpruned() {
    let mut sd = sd();
    sd.git_in("other", &["push", "-q", "origin", "HEAD:refs/heads/gone-soon"]);
    sd.followed();
    sd.git(&["fetch", "-q", "origin"]);
    sd.git_in("other", &["push", "-q", "origin", ":refs/heads/gone-soon"]);
    sd.author_pushes("more", false);
    sd.ok(&["fetch"]);
    assert!(sd.git_try(&["rev-parse", "--verify", "-q", "refs/remotes/origin/gone-soon"]).is_some());
}

// @test "fetch: refused while a live step holds the running marker; nothing fetched, HEAD and the worktree don't move (#270)"
#[test]
fn fetch_refused_while_a_live_step_holds_the_running_marker_nothing_fetched_head_and_the_worktree_dont_move_270() {
    let mut sd = sd();
    sd.followed();
    let head = sd.git(&["rev-parse", "HEAD"]);
    let pr_head = sd.git(&["rev-parse", "refs/sheepdog/review/7/pr-head"]);
    sd.author_pushes("more", false);
    let holder = Holder::new();
    let marker = running_marker(&sd.tmp.join("repo/.git"), "review/7", "review", holder.pid());
    let r = sd.run(&["fetch"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &[&step_refusal("review", "review/7", holder.pid(), "fetching")]);
    assert_eq!(sd.git(&["rev-parse", "HEAD"]), head);
    assert_eq!(sd.git(&["rev-parse", "refs/sheepdog/review/7/pr-head"]), pr_head);
    assert!(sd.git_try(&["rev-parse", "--verify", "-q", "refs/sheepdog/review/7/pr-fetched"]).is_none());
    assert_eq!(sd.git(&["status", "--porcelain"]), "");
    assert!(marker.exists());
}

// @test "fetch: the step's own child (RUNNING_HELD=<b>) fetches under its marker, and a dead holder's marker doesn't refuse (#270)"
#[test]
fn fetch_the_steps_own_child_fetches_under_its_marker_and_a_dead_holders_marker_doesnt_refuse_270() {
    let mut sd = sd();
    sd.followed();
    let holder = Holder::new();
    running_marker(&sd.tmp.join("repo/.git"), "review/7", "fetch", holder.pid());
    let theirs = sd.author_pushes("more", false);
    let r = sd.run_env(&[("RUNNING_HELD", "review/7")], &["fetch"]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_eq!(sd.git(&["rev-parse", "HEAD"]), theirs);

    running_marker(&sd.tmp.join("repo/.git"), "review/7", "review", dead_pid());
    let theirs = sd.author_pushes("again", false);
    let r = sd.run(&["fetch"]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_eq!(sd.git(&["rev-parse", "HEAD"]), theirs);
}

#[test]
fn fetch_is_for_a_followed_pr_only() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    let r = sd.run(&["fetch"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["feat isn't a PR you follow (sd act follow <number>)"]);
}

impl Sd {
    /// sd state's NEXT keys, space-separated.
    fn next_keys(&self) -> String {
        jq(&["-r", r#"[.next[].key] | join(" ")"#], &self.verb_ok("state", &["--json"]))
    }
}

#[test]
fn follow_through_a_round_review_submit_the_author_pushes_fetch_review_again() {
    let mut sd = sd();
    sd.script(&sd.tmp.join("reviewer"), "#!/usr/bin/env bash\necho '[]'\n");
    sd.verb_ok("config", &["set", "--global", "settings.localReview", &sd.path("reviewer")]);
    sd.followed();
    assert_eq!(sd.next_keys(), "review");
    let r = sd.run(&["review"]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_has(&r.output, &["clean: review/7"]);
    assert_eq!(sd.next_keys(), "submit");
    let head = sd.git(&["rev-parse", "HEAD"]);
    let mine = format!(r#"[{{"id": "r1", "state": "APPROVED", "commit": {{"oid": "{head}"}}, "author": {{"login": "me"}}}}]"#);
    fs::write(sd.tmp.join("gh/reviews"), mine).unwrap();
    assert_eq!(sd.next_keys(), "author-wait");
    sd.author_pushes("more", false);
    assert_eq!(sd.next_keys(), "fetch");
    sd.ok(&["fetch"]);
    let again = jq(&["-r", r#".next[] | select(.key == "review") | .say"#], &sd.verb_ok("state", &["--json"]));
    assert_eq!(again, "review again: 1 commit since the last review");
}
