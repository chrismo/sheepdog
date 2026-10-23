// Black-box tests of sd act follow (#250: reviewer mode): a teammate's PR,
// fetched by number from refs/pull/<n>/head, on a branch review/<n> in a
// worktree beside the main one, marked as a reviewer's branch and enrolled;
// and the author's verbs, which a reviewer's branch refuses.
//
// Real git (a bare origin, our clone at repo/, the teammate's at other/). The
// teammate's PR is refs/pull/7/head on origin, as GitHub keeps one for every
// PR, a fork's included. gh is a stub with no PRs.

mod common;

use common::{assert_has, jq, Sd};
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

#[test]
fn follow_an_existing_worktree_fails() {
    let sd = sd();
    sd.ok(&["follow", "7"]);
    let r = sd.run(&["follow", "7"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["already exists"]);
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
