// Black-box tests of sd act ack and trigger (A5) (#202): the built binary,
// run as sd-act through a link (tests/common). Moved from
// libexec/sd-act-ack.bats and sd-act-trigger.bats; each test keeps the bats
// test's title above it.

mod common;

use common::{assert_has, assert_match, chomp, jq, Sd};
use std::fs;

/// `$(cat <f> 2>/dev/null)`
fn cat(sd: &Sd, f: &str) -> String {
    chomp(&fs::read_to_string(sd.tmp.join(f)).unwrap_or_default())
}

// ack: a fake sd state answers from $FX/state.json.
mod ack {
    use super::*;

    const SD_STATE: &str = r#"#!/usr/bin/env bash
echo "$*" >> "$FX/sd-state.calls"
cat "$FX/state.json"
"#;

    /// The bats setup: repo (cwd) with one commit on main, feat checked out,
    /// and the fake sd state first on PATH.
    fn sd() -> Sd {
        let mut sd = Sd::for_verb("act");
        sd.env = vec![("FX".to_string(), sd.path("fx"))];
        fs::create_dir(sd.tmp.join("fx")).unwrap();
        sd.fake("sd-state", SD_STATE);
        sd.git(&["init", "-q", &sd.path("repo")]);
        sd.cd("repo");
        sd.git(&["commit", "-q", "--allow-empty", "-m", "init"]);
        sd.git(&["switch", "-qc", "feat"]);
        sd
    }

    /// item <id> <type> <kind> <mark>
    fn item(id: &str, ty: &str, kind: &str, mark: &str) -> String {
        jq(
            &[
                "-nc", "--arg", "id", id, "--arg", "t", ty, "--arg", "k", kind, "--arg", "m", mark,
                r#"{id: $id, type: $t, kind: $k, mark: $m, author: "bot1", last: "bot1", url: "https://example/\($id)", excerpt: "about \($id)"}"#,
            ],
            "",
        )
    }

    impl Sd {
        /// state <item-json...>: sd state's answer, owing those items.
        fn state(&self, items: &[String]) {
            let filter = r#"{branch: "feat", pr: {number: 7, state: "OPEN"}, threads: {owed_bot: 0, items: $ARGS.positional}}
         | .threads.owed_bot = ([.threads.items[] | select(.kind == "bot")] | length)"#;
            let mut args = vec!["-n", filter, "--jsonargs"];
            args.extend(items.iter().map(String::as_str));
            fs::write(self.tmp.join("fx/state.json"), jq(&args, "") + "\n").unwrap();
        }

        /// The ids marked handled on feat, sorted.
        fn handled(&self) -> Vec<String> {
            let h = self.git_try(&["config", "--get-all", "branch.feat.sd-handled"]).unwrap_or_default();
            let mut ids: Vec<String> = h.lines().map(String::from).collect();
            ids.sort();
            ids
        }

        /// A standing question record on feat.
        fn question(&self) {
            let head = self.git(&["rev-parse", "HEAD"]);
            let rec = jq(&["-nc", "--arg", "h", &head, r#"{head: $h, step: "ci-fix", mark: "m", text: "which?"}"#], "");
            let blob = self.git_with(&[], Some(&rec), &["hash-object", "-w", "--stdin"]);
            self.git(&["update-ref", "refs/sheepdog/feat/question", &blob]);
        }
    }

    // @test "ack: marks every owed info item handled and lists them"
    #[test]
    fn ack_marks_every_owed_info_item_handled_and_lists_them() {
        let sd = sd();
        sd.state(&[item("R1", "review", "bot", "i"), item("C1", "comment", "bot", "i"), item("T1", "thread", "bot", "!")]);
        let r = sd.run(&["ack"]);
        assert_eq!(r.status, 0);
        assert_eq!(sd.handled(), ["C1", "R1"]);
        assert_eq!(r.lines()[0], "acked 2 on feat:");
        assert_has(&r.output, &["review", "bot1", "about R1", "https://example/R1"]);
        assert_has(&r.output, &["https://example/C1"]);
    }

    // @test "ack: asks sd state about the named branch"
    #[test]
    fn ack_asks_sd_state_about_the_named_branch() {
        let sd = sd();
        sd.git(&["branch", "other"]);
        sd.state(&[item("R1", "review", "bot", "i")]);
        assert_eq!(sd.run(&["ack", "other"]).status, 0);
        assert_eq!(cat(&sd, "fx/sd-state.calls"), "--json other");
        assert_eq!(sd.git(&["config", "--get-all", "branch.other.sd-handled"]), "R1");
    }

    // @test "ack: nothing owed is fine and writes nothing"
    #[test]
    fn ack_nothing_owed_is_fine_and_writes_nothing() {
        let sd = sd();
        sd.state(&[item("T1", "thread", "bot", "!")]);
        let r = sd.run(&["ack"]);
        assert_eq!(r.status, 0);
        assert!(r.output.starts_with("nothing to ack on feat"), "{}", r.output);
        assert!(sd.handled().is_empty());
    }

    // @test "ack --id: only the named items"
    #[test]
    fn ack_id_only_the_named_items() {
        let sd = sd();
        sd.state(&[item("R1", "review", "bot", "i"), item("C1", "comment", "bot", "i"), item("R2", "review", "bot", "!")]);
        assert_eq!(sd.run(&["ack", "--id", "C1", "--id", "R2"]).status, 0);
        assert_eq!(sd.handled(), ["C1", "R2"]);
    }

    // @test "ack --id: a thread is refused; it gets resolved on GitHub"
    #[test]
    fn ack_id_a_thread_is_refused_it_gets_resolved_on_github() {
        let sd = sd();
        sd.state(&[item("T1", "thread", "bot", "!")]);
        let r = sd.run(&["ack", "--id", "T1"]);
        assert_eq!(r.status, 1);
        assert_has(&r.output, &["T1 is a thread"]);
        assert!(sd.handled().is_empty());
    }

    // @test "ack --id: a human item is refused"
    #[test]
    fn ack_id_a_human_item_is_refused() {
        let sd = sd();
        sd.state(&[item("H1", "review", "human", "!")]);
        let r = sd.run(&["ack", "--id", "H1"]);
        assert_eq!(r.status, 1);
        assert_has(&r.output, &["H1", "human"]);
        assert!(sd.handled().is_empty());
    }

    // @test "ack --id: an id that isn't owed is refused and nothing is written"
    #[test]
    fn ack_id_an_id_that_isnt_owed_is_refused_and_nothing_is_written() {
        let sd = sd();
        sd.state(&[item("R1", "review", "bot", "i")]);
        let r = sd.run(&["ack", "--id", "R1", "--id", "NOPE"]);
        assert_eq!(r.status, 1);
        assert_has(&r.output, &["NOPE isn't owed"]);
        assert!(sd.handled().is_empty());
    }

    // @test "ack: GitHub unreadable is refused"
    #[test]
    fn ack_github_unreadable_is_refused() {
        let sd = sd();
        let s = jq(&["-n", r#"{branch: "feat", pr: null, threads: {owed_bot: null, items: []}, errors: ["gh: boom"]}"#], "");
        fs::write(sd.tmp.join("fx/state.json"), s).unwrap();
        let r = sd.run(&["ack"]);
        assert_eq!(r.status, 1);
        assert_has(&r.output, &["can't read the PR", "gh: boom"]);
    }

    // @test "ack: sd state failing is an error"
    #[test]
    fn ack_sd_state_failing_is_an_error() {
        let sd = sd();
        sd.fake("sd-state", "#!/usr/bin/env bash\nexit 1\n");
        let r = sd.run(&["ack"]);
        assert_eq!(r.status, 1);
        assert_has(&r.output, &["sd state failed"]);
    }

    // @test "ack: sd state's warnings get through"
    #[test]
    fn ack_sd_states_warnings_get_through() {
        let sd = sd();
        sd.state(&[item("R1", "review", "bot", "i")]);
        sd.fake("sd-state", "#!/usr/bin/env bash\necho \"warning: from sd state\" >&2\ncat \"$FX/state.json\"\n");
        let r = sd.run(&["ack"]);
        assert_eq!(r.status, 0);
        assert_has(&r.output, &["warning: from sd state"]);
    }

    // @test "ack: an answer from sd state that isn't JSON says so"
    #[test]
    fn ack_an_answer_from_sd_state_that_isnt_json_says_so() {
        let sd = sd();
        sd.fake("sd-state", "#!/usr/bin/env bash\necho nope\n");
        let r = sd.run(&["ack"]);
        assert_eq!(r.status, 1);
        assert_has(&r.output, &["can't parse sd state's answer"]);
    }

    // ── the agent's question (#167) ──────────────────────────────────────────────

    // @test "ack: info items only; a standing question stays"
    #[test]
    fn ack_info_items_only_a_standing_question_stays() {
        let sd = sd();
        sd.question();
        sd.state(&[item("R1", "review", "bot", "i")]);
        assert_eq!(sd.run(&["ack"]).status, 0);
        assert_eq!(sd.handled(), ["R1"]);
        assert_eq!(sd.git_status(&["rev-parse", "-q", "--verify", "refs/sheepdog/feat/question"]), 0);
    }

    // @test "ack --question: drops the question only, without asking sd state"
    #[test]
    fn ack_question_drops_the_question_only_without_asking_sd_state() {
        let sd = sd();
        sd.question();
        sd.state(&[item("R1", "review", "bot", "i")]);
        let r = sd.run(&["ack", "--question"]);
        assert_eq!(r.status, 0);
        assert_eq!(r.output, "acked the agent's question on feat (ci-fix)");
        assert!(sd.handled().is_empty());
        assert!(!sd.tmp.join("fx/sd-state.calls").exists());
        assert_ne!(sd.git_status(&["rev-parse", "-q", "--verify", "refs/sheepdog/feat/question"]), 0);
    }

    // @test "ack --question: none standing says so; with --id, or on another verb, it's refused"
    #[test]
    fn ack_question_none_standing_says_so_with_id_or_on_another_verb_its_refused() {
        let sd = sd();
        let r = sd.run(&["ack", "--question"]);
        assert_eq!(r.status, 0);
        assert_eq!(r.output, "no question to ack on feat");
        let r = sd.run(&["ack", "--question", "--id", "R1"]);
        assert_eq!(r.status, 1);
        assert_has(&r.output, &["--question takes no --id"]);
        let r = sd.run(&["push", "--question"]);
        assert_eq!(r.status, 1);
        assert_has(&r.output, &["--question is for sd act ack only"]);
    }
}

// trigger: real git and the real sd state. gh is a stub: graphql answers from
// $GH/<branch>.json (by number: the node of whichever file has it), the REST
// trigger comment is logged to $GH/calls.
mod trigger {
    use super::*;

    const GH: &str = r#"#!/usr/bin/env bash
echo "${GH_REPO-unset}" >> "$GH/gh_repo"
q="" b="" fields=()
for a; do
  [[ $a == branch=* ]] && b=${a#branch=}
  [[ $a == query=* ]] && q=${a#query=}
  [[ $a == body=* || $a == subject=* || $a == url=* ]] && fields+=("$a")
done
# REST: the trigger comment, answered with GitHub's own created_at (or $GH/created_at's).
if [[ $1 == api && $2 == repos/*/issues/*/comments ]]; then
  printf 'comment %s %s\n' "$2" "${fields[*]}" >> "$GH/calls"
  [[ -e $GH/fail-comment ]] && { echo "gh: boom" >&2; exit 1; }
  jq -n --arg t "$(cat "$GH/created_at" 2>/dev/null || echo 2026-10-16T12:00:00Z)" '{id: 1, created_at: (if $t == "" then null else $t end)}' \
    | { filter=""; for a; do [[ $prev == --jq ]] && filter=$a; prev=$a; done; if [[ -n $filter ]]; then jq -r "$filter"; else cat; fi; }
  exit 0
fi
n=""; for a; do [[ $a == number=* ]] && n=${a#number=}; done
# By number: the node with that number, from the one branch file that has it.
if [[ -n $n ]]; then
  f=$(jq -rn --argjson n "$n" 'inputs | select(any(.data.repository.pullRequests.nodes[]?; .number == $n)) | input_filename' "$GH"/*.json 2>/dev/null)
  [[ $f != *$'\n'* ]] || { echo "fake gh: #$n is in more than one fixture: $f" >&2; exit 2; }
  [[ -n $f ]] || { echo "gh: Could not resolve to a PullRequest with the number of $n." >&2; exit 1; }
  exec jq --argjson n "$n" '.data.repository |= {defaultBranchRef, pullRequest: (.pullRequests.nodes[] | select(.number == $n))}' "$f"
fi
case $q in
  *) if [[ -f $GH/$b.json ]]; then cat "$GH/$b.json"
     else echo '{"data":{"viewer":{"login":"me"},"repository":{"defaultBranchRef":{"name":"main"},"pullRequests":{"nodes":[]}}}}'
     fi ;;
esac
"#;

    /// The bats setup: origin.git and its clone repo (cwd), feat one commit
    /// past main, pushed, with an open draft PR #7 and the local review
    /// covering HEAD; settings.botCommand set; the fake gh and the real sd
    /// state on PATH.
    fn sd() -> Sd {
        let mut sd = Sd::for_verb("act");
        sd.env = vec![("GH".to_string(), sd.path("gh"))];
        fs::create_dir(sd.tmp.join("gh")).unwrap();
        sd.fake("gh", GH);
        sd.on_path("state");
        sd.verb_ok("config", &["set", "--global", "settings.botCommand", "/review-bot"]);
        sd.origin(&["repo"]);
        sd.cd("repo");
        sd.write("file.txt", "a\nb\n");
        sd.git(&["add", "file.txt"]);
        sd.git(&["commit", "-qm", "init"]);
        sd.git(&["push", "-q", "origin", "main"]);
        sd.git(&["switch", "-qc", "feat"]);
        sd.write("file.txt", "a\nb\nc\n");
        sd.git(&["commit", "-qam", "add c"]);
        sd.git(&["push", "-q", "-u", "origin", "feat"]);
        sd.open_pr("feat", 7, "OPEN");
        sd.git(&["update-ref", "refs/sheepdog/feat/local", "HEAD"]);
        sd.git(&["update-ref", "refs/sheepdog/feat/local-base", "main"]);
        sd
    }

    impl Sd {
        fn open_pr(&self, b: &str, n: u32, st: &str) {
            let h = self.git(&["rev-parse", b]);
            let j = jq(
                &[
                    "-n", "--arg", "b", b, "--argjson", "n", &n.to_string(), "--arg", "st", st, "--arg", "h", &h,
                    r#"{data: {viewer: {login: "me"},
    repository: {defaultBranchRef: {name: "main"},
      pullRequests: {nodes: [{number: $n, url: "https://github.com/upstream/proj/pull/\($n)", state: $st, isDraft: true, baseRefName: "main",
                              headRefName: $b, headRefOid: $h, isCrossRepository: false}]}}}}"#,
                ],
                "",
            );
            fs::write(self.tmp.join(format!("gh/{b}.json")), j + "\n").unwrap();
        }

        fn calls(&self) -> String {
            cat(self, "gh/calls")
        }

        /// refs/sheepdog/feat/<kind>'s sha, or "".
        fn pin(&self, kind: &str) -> String {
            self.git_try(&["rev-parse", "-q", "--verify", &format!("refs/sheepdog/feat/{kind}")]).unwrap_or_default()
        }

        fn head(&self) -> String {
            self.git(&["rev-parse", "HEAD"])
        }

        /// echo d >> file.txt && git commit -qam "add d"
        fn add_d(&self) {
            self.write("file.txt", "a\nb\nc\nd\n");
            self.git(&["commit", "-qam", "add d"]);
        }
    }

    // @test "trigger: posts the bot command on the PR (REST) and pins bot at HEAD with its base"
    #[test]
    fn trigger_posts_the_bot_command_on_the_pr_rest_and_pins_bot_at_head_with_its_base() {
        let sd = sd();
        let r = sd.run(&["trigger"]);
        assert_eq!(r.status, 0);
        // The PR's repo from its URL, not {owner}/{repo}: gh's default repo may be a fork.
        assert_has(&sd.calls(), &["comment repos/upstream/proj/issues/7/comments body=/review-bot"]);
        assert_eq!(sd.pin("bot"), sd.head());
        assert_eq!(sd.pin("bot-base"), sd.git(&["rev-parse", "origin/main"]));
        assert_has(&r.output, &["triggered"]);
        assert_has(&r.output, &["waiting"]);
    }

    // @test "trigger: a fork's clone asks GitHub about the repo file's settings.remote, not gh's pick among the remotes"
    #[test]
    fn trigger_a_forks_clone_asks_github_about_the_repo_files_settings_remote_not_ghs_pick_among_the_remotes() {
        let mut sd = sd();
        // A GitHub URL served from ssh/: the fake ssh runs git's command there.
        fs::create_dir_all(sd.tmp.join("ssh/me")).unwrap();
        sd.git(&["clone", "-q", "--bare", &sd.path("origin.git"), &sd.path("ssh/me/fork.git")]);
        sd.fake("ssh", &format!("#!/usr/bin/env bash\nc=${{@: -1}}\ncd '{}' && eval \"git ${{c#git-}}\"\n", sd.path("ssh")));
        let ssh = sd.path("fakebin/ssh");
        sd.env.push(("GIT_SSH_COMMAND".to_string(), ssh));
        sd.git(&["remote", "add", "fork", "git@github.com:me/fork.git"]);
        sd.verb_ok("config", &["set", "--repo", "settings.remote", "fork"]);
        sd.git(&["fetch", "-q", "fork"]);
        assert_eq!(sd.run(&["trigger"]).status, 0);
        let mut repos: Vec<String> = cat(&sd, "gh/gh_repo").lines().map(String::from).collect();
        repos.sort();
        repos.dedup();
        assert_eq!(repos, ["github.com/me/fork"]);
    }

    // @test "trigger: records when the comment was posted, by GitHub's clock"
    #[test]
    fn trigger_records_when_the_comment_was_posted_by_githubs_clock() {
        let sd = sd();
        fs::write(sd.tmp.join("gh/created_at"), "2026-10-16T12:34:56Z\n").unwrap();
        assert_eq!(sd.run(&["trigger"]).status, 0);
        assert_eq!(sd.git(&["config", "branch.feat.sd-bot-triggered"]), "2026-10-16T12:34:56Z");
    }

    // @test "trigger: no created_at in the answer: the local clock, and a note"
    #[test]
    fn trigger_no_created_at_in_the_answer_the_local_clock_and_a_note() {
        let sd = sd();
        fs::write(sd.tmp.join("gh/created_at"), "").unwrap();
        let r = sd.run_sep(&["trigger"]);
        assert_eq!(r.status, 0);
        assert_match(&sd.git(&["config", "branch.feat.sd-bot-triggered"]), r"^20[0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9:]+Z$");
        assert_has(&r.stderr, &["local clock"]);
    }

    // @test "trigger: already pinned at HEAD: nothing posted; --force posts again and resets the time"
    #[test]
    fn trigger_already_pinned_at_head_nothing_posted_force_posts_again_and_resets_the_time() {
        let sd = sd();
        sd.git(&["update-ref", "refs/sheepdog/feat/bot", "HEAD"]);
        sd.git(&["config", "branch.feat.sd-bot-triggered", "2026-10-15T00:00:00Z"]);
        let r = sd.run(&["trigger"]);
        assert_eq!(r.status, 0);
        assert_has(&r.output, &["already"]);
        assert_eq!(sd.calls(), "");
        assert_eq!(sd.run(&["trigger", "--force"]).status, 0);
        assert_has(&sd.calls(), &["comment "]);
        assert_eq!(sd.git(&["config", "branch.feat.sd-bot-triggered"]), "2026-10-16T12:00:00Z");
    }

    // @test "trigger: HEAD not on the remote yet is refused"
    #[test]
    fn trigger_head_not_on_the_remote_yet_is_refused() {
        let sd = sd();
        sd.add_d();
        sd.git(&["update-ref", "refs/sheepdog/feat/local", "HEAD"]);
        let r = sd.run(&["trigger"]);
        assert_ne!(r.status, 0);
        assert_has(&r.output, &["push"]);
        assert_eq!(sd.calls(), "");
        assert_eq!(sd.pin("bot"), "");
    }

    // @test "trigger: the remote ahead of HEAD (a pushed commit dropped here) is refused: push first (#86)"
    #[test]
    fn trigger_the_remote_ahead_of_head_a_pushed_commit_dropped_here_is_refused_push_first_86() {
        let sd = sd();
        sd.add_d();
        sd.git(&["push", "-q", "origin", "feat"]);
        sd.git(&["reset", "-q", "--hard", "HEAD~1"]);
        sd.git(&["update-ref", "refs/sheepdog/feat/local", "HEAD"]);
        let r = sd.run(&["trigger"]);
        assert_ne!(r.status, 0);
        assert_has(&r.output, &["push"]);
        assert_eq!(sd.calls(), "");
        assert_eq!(sd.pin("bot"), "");
    }

    // @test "trigger: the remote ahead with someone else's commit is refused: take it first (#86)"
    #[test]
    fn trigger_the_remote_ahead_with_someone_elses_commit_is_refused_take_it_first_86() {
        let sd = sd();
        sd.git(&["update-ref", "refs/sheepdog/feat/local", "HEAD"]);
        sd.git(&["clone", "-q", &sd.path("origin.git"), &sd.path("other2")]);
        sd.git_in("other2", &["switch", "-q", "feat"]);
        fs::write(sd.tmp.join("other2/t.txt"), "t\n").unwrap();
        sd.git_in("other2", &["add", "t.txt"]);
        sd.git_in("other2", &["-c", "user.email=them@example.com", "-c", "user.name=Them", "commit", "-qm", "theirs"]);
        sd.git_in("other2", &["push", "-q", "origin", "feat"]);
        sd.git(&["fetch", "-q", "origin"]);
        let r = sd.run(&["trigger"]);
        assert_ne!(r.status, 0);
        assert_has(&r.output, &["take its commits first"]);
        assert_eq!(sd.calls(), "");
        assert_eq!(sd.pin("bot"), "");
    }

    // @test "trigger: --force on a pinned HEAD the remote doesn't have is still refused"
    #[test]
    fn trigger_force_on_a_pinned_head_the_remote_doesnt_have_is_still_refused() {
        let sd = sd();
        sd.add_d();
        sd.git(&["update-ref", "refs/sheepdog/feat/local", "HEAD"]);
        sd.git(&["update-ref", "refs/sheepdog/feat/bot", "HEAD"]);
        let r = sd.run(&["trigger", "--force"]);
        assert_ne!(r.status, 0);
        assert_has(&r.output, &["push"]);
        assert_eq!(sd.calls(), "");
    }

    // @test "trigger: no PR is refused"
    #[test]
    fn trigger_no_pr_is_refused() {
        let sd = sd();
        fs::remove_file(sd.tmp.join("gh/feat.json")).unwrap();
        let r = sd.run(&["trigger"]);
        assert_ne!(r.status, 0);
        assert_has(&r.output, &["no PR"]);
    }

    // @test "trigger: a closed PR is refused"
    #[test]
    fn trigger_a_closed_pr_is_refused() {
        let sd = sd();
        sd.open_pr("feat", 7, "CLOSED");
        let r = sd.run(&["trigger"]);
        assert_ne!(r.status, 0);
        assert_has(&r.output, &["closed"]);
    }

    // @test "trigger: no settings.botCommand is refused before any gh call"
    #[test]
    fn trigger_no_settings_botcommand_is_refused_before_any_gh_call() {
        let sd = sd();
        sd.verb_ok("config", &["unset", "--global", "settings.botCommand"]);
        let r = sd.run(&["trigger"]);
        assert_ne!(r.status, 0);
        assert_has(&r.output, &["botCommand"]);
        assert_eq!(sd.calls(), "");
    }

    // @test "trigger: the local review not covering HEAD is a soft refusal; --force posts"
    #[test]
    fn trigger_the_local_review_not_covering_head_is_a_soft_refusal_force_posts() {
        let sd = sd();
        sd.git(&["update-ref", "-d", "refs/sheepdog/feat/local"]);
        let r = sd.run(&["trigger"]);
        assert_ne!(r.status, 0);
        assert_has(&r.output, &["local review"]);
        assert_eq!(sd.calls(), "");
        assert_eq!(sd.run(&["trigger", "--force"]).status, 0);
        assert_eq!(sd.pin("bot"), sd.head());
    }

    // @test "trigger: an accepted pin at HEAD stands in for the local review; no --force (#181)"
    #[test]
    fn trigger_an_accepted_pin_at_head_stands_in_for_the_local_review_no_force_181() {
        let sd = sd();
        sd.git(&["update-ref", "refs/sheepdog/feat/local", "HEAD~1"]);
        sd.git(&["update-ref", "refs/sheepdog/feat/accepted", "HEAD"]);
        sd.git(&["update-ref", "refs/sheepdog/feat/accepted-base", "main"]);
        assert_eq!(sd.run(&["trigger"]).status, 0);
        assert_has(&sd.calls(), &["comment "]);
        assert_eq!(sd.pin("bot"), sd.head());
    }

    // @test "trigger: the comment failing pins nothing and records no time"
    #[test]
    fn trigger_the_comment_failing_pins_nothing_and_records_no_time() {
        let sd = sd();
        fs::write(sd.tmp.join("gh/fail-comment"), "").unwrap();
        assert_ne!(sd.run(&["trigger"]).status, 0);
        assert_eq!(sd.pin("bot"), "");
        assert_ne!(sd.git_status(&["config", "--get", "branch.feat.sd-bot-triggered"]), 0);
    }
}
