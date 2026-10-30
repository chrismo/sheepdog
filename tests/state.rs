// Black-box tests of sd state (#200): the built binary, run as sd-state
// through a link (tests/common), reporting where a branch's PR stands; it
// writes only the PR's number. Moved from libexec/sd-state.bats; each test
// keeps the bats test's title above it.
//
// Real git: a bare origin, a working clone, and a second clone ("other") that
// plays everyone else pushing. GitHub is a fake `gh` that answers sd state's two
// GraphQL queries from $GH_FIXTURES/<branch>.json: a discovery by branch= gets
// the file, and a fetch by number= gets the node with that number from
// whichever file has it, its headRefName that file's branch unless the node
// names one. Discovery costs 1, a fetch 2, both with $GH_FAKE_REMAINING left.
// GH_FAKE_GONE=<n>: a fetch of #n finds no such PR.
//
// sd state reads the running marker and a question's worktree mark that sd
// act writes, so those two fixtures write them with sd act's own
// running::proc_start and git::worktree_mark.

mod common;

use common::{assert_has, assert_lacks, assert_match, chomp, jq, output, spawn, Run, Sd};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Once;

const GH: &str = r##"#!/usr/bin/env bash
echo "${GH_REPO-unset}" >> "$GH_FIXTURES/gh_repo"
if [[ -n ${GH_FAKE_FAIL:-} ]]; then echo "$GH_FAKE_FAIL" >&2; exit 1; fi
# REST: the base branch's rulesets and classic protection, from
# $GH_FIXTURES/rules.json and branch.json (default: none, unprotected).
# GH_FAKE_FAIL_REST fails the paths matching GH_FAKE_FAIL_REST_ON (default all).
if [[ $1 == api && $2 == repos/* ]]; then
  echo "$2" >> "$GH_FIXTURES/rest"
  # shellcheck disable=SC2053 # the pattern is a glob on purpose
  if [[ -n ${GH_FAKE_FAIL_REST:-} && $2 == ${GH_FAKE_FAIL_REST_ON:-*} ]]; then echo "$GH_FAKE_FAIL_REST" >&2; exit 1; fi
  case $2 in
    */rules/branches/*) cat "$GH_FIXTURES/rules.json" 2>/dev/null || echo '[]' ;;
    */branches/*) cat "$GH_FIXTURES/branch.json" 2>/dev/null || echo '{"name": "main", "protected": false}' ;;
    *) echo "fake gh: unexpected api call: $*" >&2; exit 2 ;;
  esac
  exit 0
fi
b="" c="" n="" prev=""
for a; do
  case $a in
    branch=*)
      b=${a#branch=}
      # real gh -F turns numbers/booleans/null into JSON types; GitHub then rejects String!
      if [[ $prev == -F && $b =~ ^([0-9]+|true|false|null)$ ]]; then
        echo "gh: Variable \$branch of type String! was provided invalid value" >&2; exit 1
      fi ;;
    number=*) n=${a#number=} ;;
    commits=*) c=$a ;;
  esac
  prev=$a
done
rl() { printf '{"cost": %s, "remaining": %s, "resetAt": "2026-10-16T13:00:00Z"}' "$1" "${GH_FAKE_REMAINING:-4999}"; }
if [[ -n $n ]]; then
  f=$(jq -rn --argjson n "$n" 'inputs | select(any(.data.repository.pullRequests.nodes[]?; .number == $n)) | input_filename' \
        "$GH_FIXTURES"/*.json 2>/dev/null)
  [[ $f != *$'\n'* ]] || { echo "fake gh: #$n is in more than one fixture: $f" >&2; exit 2; }
  b=$(basename "$f" .json) b=${b//__//}
  if [[ -n $b && ${GH_FAKE_FAIL_FOR:-} == "$b" ]]; then echo "gh: boom for $b" >&2; exit 1; fi
  echo "#$n" >> "$GH_FIXTURES/calls"
  echo "#$n ${c:-}" >> "$GH_FIXTURES/commits"
  if [[ -z $f || $n == "${GH_FAKE_GONE:-}" ]]; then
    echo "{\"data\":{\"rateLimit\":$(rl 1),\"repository\":{\"pullRequest\":null}}}"
    echo "gh: Could not resolve to a PullRequest with the number of $n." >&2; exit 1
  fi
  jq --argjson n "$n" --arg b "$b" --argjson rl "$(rl 2)" '.data as $d | {data: {viewer: $d.viewer, rateLimit: $rl,
    repository: {defaultBranchRef: $d.repository.defaultBranchRef,
                 pullRequest: ($d.repository.pullRequests.nodes[] | select(.number == $n) | .headRefName //= $b)}}}' "$f"
  exit 0
fi
[[ -n $b ]] || { echo "fake gh: no branch= or number= field in: $*" >&2; exit 2; }
if [[ ${GH_FAKE_FAIL_FOR:-} == "$b" ]]; then echo "gh: boom for $b" >&2; exit 1; fi
echo "$b" >> "$GH_FIXTURES/calls"
echo "$b${c:+ $c}" >> "$GH_FIXTURES/commits"
f="$GH_FIXTURES/${b//\//__}.json"
if [[ -f $f ]]; then jq --argjson rl "$(rl 1)" '.data.rateLimit = $rl' "$f"; else
  echo "{\"data\":{\"viewer\":{\"login\":\"me\"},\"rateLimit\":$(rl 1),\"repository\":{\"defaultBranchRef\":{\"name\":\"main\"},\"pullRequests\":{\"nodes\":[]}}}}"
fi
"##;

/// The bats setup: origin.git, the clones work (cwd) and other, file.txt
/// committed on main and pushed, and the fake gh first on PATH.
fn sd() -> Sd {
    let mut sd = Sd::for_verb("state");
    let env = vec![
        ("SHEEPDOG_STATE_DIR".to_string(), sd.path("state")), // usage.sup, for the branch's spend
        ("GH_FIXTURES".to_string(), sd.path("gh")),
    ];
    sd.env = env;
    fs::create_dir(sd.tmp.join("gh")).unwrap();
    sd.fake("gh", GH);
    sd.origin(&["work", "other"]);
    sd.cd("work");
    sd.write("file.txt", "one\ntwo\nthree\n");
    sd.git(&["add", "file.txt"]);
    sd.git(&["commit", "-qm", "init"]);
    sd.git(&["push", "-q", "origin", "main"]);
    sd.git_in("other", &["pull", "-q"]);
    sd
}

impl Sd {
    /// commit <file> <content>, in work.
    fn commit(&self, file: &str, content: &str) {
        self.commit_in("work", file, content);
    }

    /// commit <file> <content> <repo>
    fn commit_in(&self, repo: &str, file: &str, content: &str) {
        let f = self.tmp.join(repo).join(file);
        fs::create_dir_all(f.parent().unwrap()).unwrap();
        fs::write(f, format!("{content}\n")).unwrap();
        self.git_in(repo, &["add", file]);
        self.git_in(repo, &["commit", "-qm", &format!("edit {file}")]);
    }

    /// Someone else commits <file> <content> to main and pushes it.
    fn main_moves(&self, file: &str, content: &str) {
        self.commit_in("other", file, content);
        self.git_in("other", &["push", "-q", "origin", "main"]);
    }

    /// $GH_FIXTURES/<name>
    fn gh(&self, name: &str) -> PathBuf {
        self.tmp.join("gh").join(name)
    }

    fn fixture(&self, branch: &str) -> PathBuf {
        self.gh(&format!("{}.json", branch.replace('/', "__")))
    }

    /// pr_fixture <branch> [jq-assignments...] — the GraphQL answer for <branch>'s PR.
    /// Defaults: open, base main, head = that branch's tip, no checks, no threads.
    fn pr_fixture(&self, branch: &str, assigns: &[&str]) {
        let head = self.git_try(&["rev-parse", "-q", "--verify", branch]).unwrap_or_else(|| "0000".into());
        let mut node = jq(
            &["-n", "--arg", "head", &head, r#"{
    number: 7, url: "https://example/pr/7", state: "OPEN", isDraft: false, isCrossRepository: false,
    baseRefName: "main", headRefOid: $head, mergeable: "MERGEABLE", mergedAt: null,
    mergeStateStatus: "CLEAN", reviewDecision: null, reviewRequests: {nodes: []},
    commits: {nodes: [{commit: {oid: $head, statusCheckRollup: null}}]},
    reviewThreads: {nodes: []}
  }"#],
            "",
        );
        for a in assigns {
            node = jq(&[a], &node);
        }
        let doc = jq(
            &["-n", "--argjson", "node", &node,
              "{data: {viewer: {login: \"me\"}, repository: {defaultBranchRef: {name: \"main\"}, pullRequests: {nodes: [$node]}}}}"],
            "",
        );
        fs::write(self.fixture(branch), doc + "\n").unwrap();
    }

    /// jq <filter> over <branch>'s fixture, in place.
    fn edit_fixture(&self, branch: &str, filter: &str) {
        let f = self.fixture(branch);
        let doc = jq(&[filter], &fs::read_to_string(&f).unwrap());
        fs::write(f, doc + "\n").unwrap();
    }

    /// `run sd-state --json <args>`, which must succeed.
    fn state(&self, args: &[&str]) -> Run {
        self.state_env(&[], args)
    }

    /// `VAR=value state <args>`
    fn state_env(&self, env: &[(&str, &str)], args: &[&str]) -> Run {
        let r = self.run_env(env, &[&["--json"], args].concat());
        assert_eq!(r.status, 0, "{}", r.output);
        r
    }

    /// The GH_REPO values gh ran with, each once (`sort -u`).
    fn gh_repos(&self) -> String {
        let s = fs::read_to_string(self.gh("gh_repo")).unwrap_or_default();
        let mut v: Vec<&str> = s.lines().collect();
        v.sort();
        v.dedup();
        v.join("\n")
    }

    /// The gh calls since the last look, space-separated; forgets them.
    fn calls(&self) -> String {
        let f = self.gh("calls");
        let s = fs::read_to_string(&f).unwrap_or_default().replace('\n', " ");
        let _ = fs::remove_file(f);
        s
    }

    /// git update-ref <r> to a blob of <rec>, as `<<<"$rec"` wrote it.
    fn put_blob(&self, r: &str, rec: &str) {
        let blob = self.git_with(&[], Some(&format!("{rec}\n")), &["hash-object", "-w", "--stdin"]);
        self.git(&["update-ref", r, &blob]);
    }

    /// The cwd's git::worktree_mark. Its git runs with this process's
    /// environment, not Sd::command's, so git's config is cleared here too.
    fn worktree_mark(&self) -> String {
        static ENV: Once = Once::new();
        ENV.call_once(|| {
            std::env::set_var("GIT_CONFIG_GLOBAL", "/dev/null");
            std::env::set_var("GIT_CONFIG_SYSTEM", "/dev/null");
            for v in ["GIT_DIR", "GIT_WORK_TREE", "GIT_COMMON_DIR", "GIT_INDEX_FILE"] {
                std::env::remove_var(v);
            }
        });
        sheepdog::git::worktree_mark(&self.cwd.to_string_lossy()).expect("a worktree mark")
    }

    /// "$SD_CONFIG" <args>
    fn sd_config(&self, args: &[&str]) {
        self.verb_ok("config", args);
    }

    /// <parent> squash-merged into main elsewhere, its branch deleted.
    fn squash_merged(&self, parent: &str) {
        self.git_in("other", &["fetch", "-q", "origin"]);
        self.git_in("other", &["merge", "-q", "--squash", &format!("origin/{parent}")]);
        self.git_in("other", &["commit", "-qm", "parent (#3)"]);
        self.git_in("other", &["push", "-q", "origin", "main", &format!(":{parent}")]);
    }

    /// squash_parent_away: parent squash-merged into main elsewhere, its branch deleted.
    fn squash_parent_away(&self) {
        self.squash_merged("parent");
        self.git(&["fetch", "-q", "--prune"]);
    }

    /// Someone else amends parent's tip and force-pushes it.
    fn parent_amended_elsewhere(&self) {
        self.git_in("other", &["fetch", "-q", "origin"]);
        self.git_in("other", &["switch", "-qc", "parent", "origin/parent"]);
        self.git_in("other", &["commit", "-q", "--amend", "-m", "p1, amended"]);
        self.git_in("other", &["push", "-qf", "origin", "parent"]);
        self.git_in("other", &["switch", "-q", "main"]);
    }

    /// Someone else adds an empty commit p2 to parent and pushes it.
    fn parent_moves_elsewhere(&self) {
        self.git_in("other", &["fetch", "-q", "origin"]);
        self.git_in("other", &["switch", "-qc", "parent", "origin/parent"]);
        self.git_in("other", &["commit", "-q", "--allow-empty", "-m", "p2"]);
        self.git_in("other", &["push", "-q", "origin", "parent"]);
        self.git_in("other", &["switch", "-q", "main"]);
    }

    /// lost_pin_stack [nopin|same]: parent (p1, p2) squash-merged into main and its
    /// branch deleted; feat stacked on it, rewritten outside sheepdog so p2 is a
    /// copy, not p2: a changed one, or with "same" an exact one. Pins the base at p2
    /// unless told "nopin". Returns (p1, p2).
    fn lost_pin_stack(&self, mode: &str) -> (String, String) {
        self.git(&["switch", "-qc", "parent"]);
        self.commit("p.txt", "p1");
        self.commit("q.txt", "p2");
        self.git(&["push", "-q", "origin", "parent"]);
        let (p1, p2) = (self.git(&["rev-parse", "parent~1"]), self.git(&["rev-parse", "parent"]));
        self.git(&["switch", "-qc", "feat", "parent~1"]);
        self.write("q.txt", if mode == "same" { "p2\n" } else { "p2 again\n" });
        self.git(&["add", "q.txt"]);
        self.git(&["commit", "-qm", "copy of p2"]);
        self.commit("c.txt", "c");
        self.git(&["config", "branch.feat.sd-parent", "parent"]);
        if mode != "nopin" {
            self.git(&["update-ref", "refs/sheepdog/feat/base", &p2]);
        }
        self.squash_merged("parent");
        self.git(&["fetch", "-q", "--prune"]);
        self.pr_fixture("parent", &[r#".state = "MERGED""#, &format!(r#".headRefOid = "{p2}""#)]);
        (p1, p2)
    }

    /// ready_pr: feat pushed, reviewed, and an open CLEAN PR, so NEXT is ready; every
    /// call to origin (ls-remote's upload-pack) is logged to upload-pack.log.
    fn ready_pr(&self, assigns: &[&str]) {
        self.git(&["switch", "-qc", "feat"]);
        self.commit("a.txt", "a");
        self.git(&["push", "-q", "-u", "origin", "feat"]);
        self.git(&["update-ref", "refs/sheepdog/feat/local", "HEAD"]);
        self.pr_fixture("feat", assigns);
        self.fake(
            "upload-pack",
            &format!("#!/usr/bin/env bash\necho \"$@\" >> '{}'\nexec git-upload-pack \"$@\"\n", self.path("upload-pack.log")),
        );
        self.git(&["config", "remote.origin.uploadpack", &self.path("fakebin/upload-pack")]);
    }

    fn upload_packs(&self) -> Option<usize> {
        fs::read_to_string(self.tmp.join("upload-pack.log")).ok().map(|s| s.lines().count())
    }

    /// required_rules <context>...: a ruleset requiring them
    fn required_rules(&self, contexts: &[&str]) {
        let rules = jq(
            &[&["-n", r#"[{type: "deletion"}, {type: "required_status_checks", parameters: {required_status_checks:
    [$ARGS.positional[] | {context: ., integration_id: 15368}], strict_required_status_checks_policy: false}}]"#, "--args"][..], contexts].concat(),
            "",
        );
        fs::write(self.gh("rules.json"), rules + "\n").unwrap();
    }

    /// reviewed_draft: feat pushed, its local review covering HEAD, and its PR a
    /// draft, so CI is all that decides whether NEXT has the draft step.
    fn reviewed_draft(&self, assigns: &[&str]) {
        self.git(&["switch", "-qc", "feat"]);
        self.commit("a.txt", "a");
        self.git(&["push", "-q", "-u", "origin", "feat"]);
        self.git(&["update-ref", "refs/sheepdog/feat/local", "HEAD"]);
        self.pr_fixture("feat", &[&[".isDraft = true"][..], assigns].concat());
    }

    /// trigger_pin: bot pinned at HEAD, triggered at <time>
    fn trigger_pin(&self, time: &str) {
        self.git(&["update-ref", "refs/sheepdog/feat/bot", "HEAD"]);
        self.git(&["config", "branch.feat.sd-bot-triggered", time]);
    }

    fn usage_sup(&self) -> PathBuf {
        self.tmp.join("state/usage.sup")
    }

    fn append_usage(&self, line: &str) {
        use std::io::Write;
        fs::create_dir_all(self.tmp.join("state")).unwrap();
        let mut f = fs::OpenOptions::new().create(true).append(true).open(self.usage_sup()).unwrap();
        writeln!(f, "{line}").unwrap();
    }

    /// usage_row <branch> <cost_usd> [repo]
    fn usage_row(&self, branch: &str, cost: &str, repo: Option<&str>) {
        let repo = repo.map_or_else(|| self.path("work"), str::to_string);
        self.append_usage(&format!(
            r#"{{time:2026-10-16T20:33:06Z,repo:"{repo}",branch:"{branch}",step:"review",harness:"claude",session:"s",mode:"new",status:0,cost_usd:{cost}}}"#
        ));
    }

    /// The marker an agent step keeps while it runs: "<pid> <step> <since> <proc_start>".
    fn running(&self, pid: u32, step: &str) {
        fs::create_dir_all(self.cwd.join(".git/sheepdog")).unwrap();
        let start = sheepdog::running::proc_start(&pid.to_string());
        self.write(".git/sheepdog/feat.running", &format!("{pid} {step} 2026-10-18T14:03:00Z {start}\n"));
    }
}

impl Run {
    /// field <jq filter>: jq -r over the output.
    fn field(&self, filter: &str) -> String {
        jq(&["-r", filter], &self.output)
    }

    /// field -c <jq filter>
    fn field_c(&self, filter: &str) -> String {
        jq(&["-r", "-c", filter], &self.output)
    }

    fn errors(&self) -> String {
        self.field(r#".errors | join(" ")"#)
    }

    fn keys(&self) -> String {
        self.field(r#"[.next[].key] | join(" ")"#)
    }
}

/// checks '<json array of contexts>'
fn checks(contexts: &str) -> String {
    format!(r#".commits.nodes[0].commit.statusCheckRollup = {{state: "X", contexts: {{nodes: {contexts}}}}}"#)
}

/// thread <first-type> <first-login> <last-type> <last-login>
fn thread(ft: &str, fl: &str, lt: &str, ll: &str) -> String {
    thread_x(ft, fl, lt, ll, false, false)
}

/// thread <first-type> <first-login> <last-type> <last-login> <resolved> <outdated>
fn thread_x(ft: &str, fl: &str, lt: &str, ll: &str, resolved: bool, outdated: bool) -> String {
    format!(
        r#"{{id: "T-{fl}-{ll}", isResolved: {resolved}, isOutdated: {outdated}, path: "file.txt", line: 2,
    first: {{nodes: [{{author: {{__typename: "{ft}", login: "{fl}"}}}}]}},
    last: {{nodes: [{{author: {{__typename: "{lt}", login: "{ll}"}}}}]}}}}"#
    )
}

/// note <id> <type> <login> — a review or top-level comment node
fn note(id: &str, ty: &str, login: &str) -> String {
    note_at(id, ty, login, "findings", "2026-10-16T00:00:00Z")
}

/// note <id> <type> <login> <body>
fn note_body(id: &str, ty: &str, login: &str, body: &str) -> String {
    note_at(id, ty, login, body, "2026-10-16T00:00:00Z")
}

/// note <id> <type> <login> <body> <createdAt>
fn note_at(id: &str, ty: &str, login: &str, body: &str, at: &str) -> String {
    format!(r#"{{id: "{id}", body: "{body}", createdAt: "{at}", author: {{__typename: "{ty}", login: "{login}"}}}}"#)
}

/// review <id> <type> <login> <state> — a review node with a state.
fn review(id: &str, ty: &str, login: &str, state: &str) -> String {
    review_body(id, ty, login, state, "findings")
}

/// review <id> <type> <login> <state> <body>
fn review_body(id: &str, ty: &str, login: &str, state: &str, body: &str) -> String {
    format!(
        r#"{{id: "{id}", url: "https://example/pr/7#{id}", state: "{state}", body: "{body}", author: {{__typename: "{ty}", login: "{login}"}}}}"#
    )
}

/// check_run <id> <name> <conclusion|null>: a GitHub Actions check run; a null
/// conclusion is still in progress.
fn check_run(id: u32, name: &str, conclusion: &str) -> String {
    check_run_of(id, name, conclusion, "pr", "pull_request")
}

/// check_run <id> <name> <conclusion|null> <workflow> <event>
fn check_run_of(id: u32, name: &str, conclusion: &str, workflow: &str, event: &str) -> String {
    let (status, conclusion) = match conclusion {
        "null" => ("IN_PROGRESS", "null".to_string()),
        c => ("COMPLETED", format!("\"{c}\"")),
    };
    format!(
        r#"{{__typename:"CheckRun",databaseId:{id},name:"{name}",status:"{status}",conclusion:{conclusion},
    checkSuite:{{app:{{slug:"github-actions"}},workflowRun:{{event:"{event}",workflow:{{name:"{workflow}"}}}}}}}}"#
    )
}

fn head_oid(sha: &str) -> String {
    format!(r#".headRefOid = "{sha}""#)
}

fn all_commits(shas: &[&str]) -> String {
    let nodes: Vec<String> = shas.iter().map(|s| format!(r#"{{commit: {{oid: "{s}"}}}}"#)).collect();
    format!(".allCommits = {{nodes: [{}]}}", nodes.join(", "))
}

// ── basics ───────────────────────────────────────────────────────────────────

// @test "outside a git repo: fails with a message"
#[test]
fn outside_a_git_repo_fails_with_a_message() {
    let mut sd = sd();
    sd.cd("");
    let r = sd.run(&[]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["not a git repository"]);
}

#[test]
fn no_such_branch_is_reported_before_a_broken_config() {
    let sd = sd();
    sd.write_global("[]");
    let r = sd.run(&["nope"]);
    assert_ne!(r.status, 0);
    assert_eq!(r.output, "sd state: no such branch: nope");
}

// @test "branch with no PR: pr is null and parent defaults to the default branch"
#[test]
fn branch_with_no_pr_pr_is_null_and_parent_defaults_to_the_default_branch() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.txt", "a");
    let r = sd.state(&[]);
    assert_eq!(r.field(".branch"), "feat");
    assert_eq!(r.field(".pr"), "null");
    assert_eq!(r.field(".parent.name"), "main");
    assert_eq!(r.field(".parent.source"), "default");
}

// @test "a fork's clone: GitHub is asked about the repo file's settings.remote, not gh's pick among the remotes"
#[test]
fn a_forks_clone_github_is_asked_about_the_repo_files_settings_remote_not_ghs_pick_among_the_remotes() {
    let sd = sd();
    sd.git(&["remote", "add", "fork", "git@github.com:me/fork.git"]);
    sd.sd_config(&["set", "--repo", "settings.remote", "fork"]);
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.txt", "a");
    // the fork is never reached: no network
    let r = sd.state_env(&[("GIT_SSH_COMMAND", "false")], &[]);
    assert_eq!(sd.gh_repos(), "github.com/me/fork");
    assert_eq!(r.field(r#"[.next[] | select(.key == "push") | .say][0]"#), "push: not on fork yet");
}

// @test "a remote at a local path: GH_REPO is left as it was"
#[test]
fn a_remote_at_a_local_path_gh_repo_is_left_as_it_was() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.txt", "a");
    sd.state(&[]);
    assert_eq!(sd.gh_repos(), "unset");
}

// @test "task: the recorded sd-task, else null; NEXT implements a branch with nothing ahead (#143)"
#[test]
fn task_the_recorded_sd_task_else_null_next_implements_a_branch_with_nothing_ahead_143() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".task"), "null");
    assert_eq!(r.field_c("[.next[].key]"), r#"["nothing"]"#);
    sd.git(&["config", "branch.feat.sd-task", "add a"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".task"), "add a");
    assert_eq!(r.field_c("[.next[].key]"), r#"["implement"]"#);
    assert_eq!(r.field(".push.dirty"), "false");
}

// @test "dirty: reported for a branch with nothing ahead, where implement would refuse the worktree (#143)"
#[test]
fn dirty_reported_for_a_branch_with_nothing_ahead_where_implement_would_refuse_the_worktree_143() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.git(&["config", "branch.feat.sd-task", "add a"]);
    sd.write("wip.txt", "wip\n");
    let r = sd.state(&[]);
    assert_eq!(r.field(".push.dirty"), "true");
    assert_eq!(r.field_c("[.next[].key]"), r#"["nothing-dirty"]"#);
}

// @test "on the default branch: NEXT is one step, start a branch (#159)"
#[test]
fn on_the_default_branch_next_is_one_step_start_a_branch_159() {
    let sd = sd();
    let r = sd.state(&[]);
    assert_eq!(r.field_c("[.next[] | [.key, .who, .cmd]]"), r#"[["default","you","sd act start"]]"#);
}

// @test "a branch argument is inspected without switching to it"
#[test]
fn a_branch_argument_is_inspected_without_switching_to_it() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.txt", "a");
    sd.git(&["switch", "-q", "main"]);
    let r = sd.state(&["feat"]);
    assert_eq!(r.field(".branch"), "feat");
    assert_eq!(r.field(".head"), sd.git(&["rev-parse", "feat"]));
    assert_eq!(sd.git(&["branch", "--show-current"]), "main");
}

// @test "makes exactly one gh call when the parent is the default branch and the PR's number is stored"
#[test]
fn makes_exactly_one_gh_call_when_the_parent_is_the_default_branch_and_the_prs_number_is_stored() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.pr_fixture("feat", &[]);
    sd.state(&[]);
    fs::remove_file(sd.gh("calls")).unwrap();
    sd.state(&[]);
    assert_eq!(fs::read_to_string(sd.gh("calls")).unwrap().lines().count(), 1);
}

// ── O1 PR lookup ─────────────────────────────────────────────────────────────

// @test "PR fields are reported"
#[test]
fn pr_fields_are_reported() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.txt", "a");
    sd.pr_fixture("feat", &[".isDraft = true"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".pr.number"), "7");
    assert_eq!(r.field(".pr.state"), "OPEN");
    assert_eq!(r.field(".pr.draft"), "true");
    assert_eq!(r.field(".pr.base"), "main");
    assert_eq!(r.field(".pr.mergeable"), "MERGEABLE");
    assert_eq!(r.field(".pr.merge_state"), "CLEAN");
    assert_eq!(r.field(".pr.review_decision"), "null");
}

// @test "PR merge gate: merge state, review decision and who's requested"
#[test]
fn pr_merge_gate_merge_state_review_decision_and_whos_requested() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.txt", "a");
    sd.pr_fixture(
        "feat",
        &[
            r#".mergeStateStatus = "BLOCKED" | .reviewDecision = "REVIEW_REQUIRED""#,
            r#".reviewRequests.nodes = [{requestedReviewer: {__typename: "User", login: "alice"}},
                              {requestedReviewer: {__typename: "Team", slug: "core"}}]"#,
        ],
    );
    let r = sd.state(&[]);
    assert_eq!(
        r.field_c(".pr | {merge_state, review_decision, review_requests}"),
        r#"{"merge_state":"BLOCKED","review_decision":"REVIEW_REQUIRED","review_requests":["alice","core"]}"#
    );
    assert_eq!(r.field(r#"[.next[].key] | any(. == "review-wait") and all(. != "ready")"#), "true");
}

#[test]
fn pr_author_and_viewer_say_whose_pr_it_is_267() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.txt", "a");
    sd.pr_fixture("feat", &[r#".author = {login: "alice"}"#]);
    let r = sd.state(&[]);
    assert_eq!(r.field_c("{author: .pr.author, viewer}"), r#"{"author":"alice","viewer":"me"}"#);
    sd.git(&["switch", "-qc", "solo"]);
    let r = sd.state(&[]);
    assert_eq!(r.field_c("{pr, viewer}"), r#"{"pr":null,"viewer":"me"}"#);
}

// @test "an open PR is preferred over a newer closed one for the same branch"
#[test]
fn an_open_pr_is_preferred_over_a_newer_closed_one_for_the_same_branch() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.pr_fixture("feat", &[]);
    sd.edit_fixture("feat", r#".data.repository.pullRequests.nodes |= [(.[0] | .number = 9 | .state = "CLOSED"), .[0]]"#);
    let r = sd.state(&[]);
    assert_eq!(r.field(".pr.number"), "7");
}

// @test "gh failing: git facts still reported, error recorded, exit 0"
#[test]
fn gh_failing_git_facts_still_reported_error_recorded_exit_0() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.txt", "a");
    let r = sd.state_env(&[("GH_FAKE_FAIL", "gh: could not resolve host")], &[]);
    assert_eq!(r.field(".pr"), "null");
    assert_eq!(r.field(".push.ahead"), "1");
    assert_has(&r.field(".errors[0]"), &["could not resolve host"]);
}

// @test "a numeric branch name is sent to gh as a string"
#[test]
fn a_numeric_branch_name_is_sent_to_gh_as_a_string() {
    let sd = sd();
    sd.git(&["switch", "-qc", "1234"]);
    sd.commit("a.txt", "a");
    sd.pr_fixture("1234", &[]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".pr.number"), "7");
    assert_eq!(r.field(".errors | length"), "0");
}

// @test "a branch stacked on another without sheepdog knowing is flagged"
#[test]
fn a_branch_stacked_on_another_without_sheepdog_knowing_is_flagged() {
    let sd = sd();
    sd.git(&["switch", "-qc", "par"]);
    sd.commit("a.txt", "a");
    sd.git(&["switch", "-qc", "kid"]);
    sd.commit("b.txt", "b");
    let r = sd.state(&["kid"]);
    assert_has(&r.errors(), &["stacked on par without sheepdog knowing: sd act enroll records it"]);
    assert_eq!(r.field(".parent.name"), "main");
}

// @test "an unrecorded stack with two candidates names both"
#[test]
fn an_unrecorded_stack_with_two_candidates_names_both() {
    let sd = sd();
    sd.git(&["switch", "-qc", "par"]);
    sd.commit("a.txt", "a");
    sd.git(&["branch", "twin"]);
    sd.git(&["switch", "-qc", "kid"]);
    sd.commit("b.txt", "b");
    let r = sd.state(&["kid"]);
    assert_has(&r.errors(), &["stacked on par or twin without sheepdog knowing"]);
}

// @test "an unrecorded stack whose parent moved on: the child can't tell which way, the parent made the fork commit"
#[test]
fn an_unrecorded_stack_whose_parent_moved_on_the_child_cant_tell_which_way_the_parent_made_the_fork_commit() {
    let sd = sd();
    sd.git(&["switch", "-qc", "par"]);
    sd.commit("a.txt", "a");
    sd.git(&["switch", "-qc", "kid"]);
    sd.commit("b.txt", "b");
    sd.git(&["switch", "-q", "par"]);
    sd.commit("c.txt", "c");
    let r = sd.state(&["kid"]);
    assert_has(&r.errors(), &["shares commits with par, and each has its own since, so git can't tell which, if either, was made from the other: git config branch.kid.sd-parent par if kid was, or git config branch.kid.sd-parent main if not"]);
    let r = sd.state(&["par"]);
    assert_eq!(r.field(".errors | length"), "0");
}

// @test "siblings whose parent is gone: flagged without claiming either was made from the other"
#[test]
fn siblings_whose_parent_is_gone_flagged_without_claiming_either_was_made_from_the_other() {
    let sd = sd();
    sd.git(&["switch", "-qc", "par"]);
    sd.commit("a.txt", "a");
    sd.git(&["switch", "-qc", "kid"]);
    sd.commit("b.txt", "b");
    sd.git(&["switch", "-q", "par"]);
    sd.git(&["switch", "-qc", "sib"]);
    sd.commit("c.txt", "c");
    sd.git(&["branch", "-qD", "par"]);
    let r = sd.state(&["kid"]);
    assert_has(&r.errors(), &["shares commits with sib, and each has its own since, so git can't tell which, if either, was made from the other"]);
}

// @test "siblings that each moved on: all named, still without a direction"
#[test]
fn siblings_that_each_moved_on_all_named_still_without_a_direction() {
    let sd = sd();
    sd.git(&["switch", "-qc", "par"]);
    sd.commit("a.txt", "a");
    sd.git(&["switch", "-qc", "kid"]);
    sd.commit("b.txt", "b");
    sd.git(&["switch", "-q", "par"]);
    sd.git(&["switch", "-qc", "sib"]);
    sd.commit("c.txt", "c");
    sd.git(&["switch", "-q", "par"]);
    sd.git(&["switch", "-qc", "sib2"]);
    sd.commit("d.txt", "d");
    sd.git(&["branch", "-qD", "par"]);
    let r = sd.state(&["kid"]);
    assert_has(&r.errors(), &["shares commits with sib or sib2, and each has its own since, so git can't tell which, if either, was made from the other: git config branch.kid.sd-parent <name> if kid was, or git config branch.kid.sd-parent main if not"]);
}

// @test "an unrecorded stack isn't flagged when gh failed, since a PR may name the parent"
#[test]
fn an_unrecorded_stack_isnt_flagged_when_gh_failed_since_a_pr_may_name_the_parent() {
    let sd = sd();
    sd.git(&["switch", "-qc", "par"]);
    sd.commit("a.txt", "a");
    sd.git(&["switch", "-qc", "kid"]);
    sd.commit("b.txt", "b");
    let r = sd.state_env(&[("GH_FAKE_FAIL", "gh: down")], &["kid"]);
    assert_lacks(&r.errors(), "without sheepdog knowing");
}

// @test "an unrecorded stack's tie lists each name once, pushed or not"
#[test]
fn an_unrecorded_stacks_tie_lists_each_name_once_pushed_or_not() {
    let sd = sd();
    sd.git(&["switch", "-qc", "par"]);
    sd.commit("a.txt", "a");
    sd.git(&["branch", "twin"]);
    sd.git(&["push", "-q", "origin", "par", "twin"]);
    sd.git(&["switch", "-qc", "kid"]);
    sd.commit("b.txt", "b");
    let r = sd.state(&["kid"]);
    assert_has(&r.errors(), &["stacked on par or twin without sheepdog knowing: record one with git config branch.kid.sd-parent <name>"]);
}

// @test "a recorded parent, or a PR, means no unrecorded-stack flag"
#[test]
fn a_recorded_parent_or_a_pr_means_no_unrecorded_stack_flag() {
    let sd = sd();
    sd.git(&["switch", "-qc", "par"]);
    sd.commit("a.txt", "a");
    sd.git(&["switch", "-qc", "kid"]);
    sd.commit("b.txt", "b");
    sd.git(&["config", "branch.kid.sd-parent", "par"]);
    let r = sd.state(&["kid"]);
    assert_lacks(&r.errors(), "without sheepdog knowing");
    sd.git(&["config", "--unset", "branch.kid.sd-parent"]);
    sd.pr_fixture("kid", &[r#".baseRefName = "par""#]);
    let r = sd.state(&["kid"]);
    assert_lacks(&r.errors(), "without sheepdog knowing");
}

// @test "a branch made from main alone isn't flagged"
#[test]
fn a_branch_made_from_main_alone_isnt_flagged() {
    let sd = sd();
    sd.git(&["switch", "-qc", "par"]);
    sd.commit("a.txt", "a");
    sd.git(&["switch", "-q", "main"]);
    sd.git(&["switch", "-qc", "solo"]);
    sd.commit("b.txt", "b");
    let r = sd.state(&["solo"]);
    assert_eq!(r.field(".errors | length"), "0");
}

// @test "a branch name with a slash finds its PR"
#[test]
fn a_branch_name_with_a_slash_finds_its_pr() {
    let sd = sd();
    sd.git(&["switch", "-qc", "chrismo/feat"]);
    sd.commit("a.txt", "a");
    sd.pr_fixture("chrismo/feat", &[]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".pr.number"), "7");
}

// @test "a fork's PR with the same branch name is ignored"
#[test]
fn a_forks_pr_with_the_same_branch_name_is_ignored() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.txt", "a");
    sd.pr_fixture("feat", &[]);
    sd.edit_fixture(
        "feat",
        r#".data.repository.pullRequests.nodes |= [(.[0] | .number = 99 | .isCrossRepository = true | .headRepositoryOwner = {login: "stranger"}), .[0]]"#,
    );
    let r = sd.state(&[]);
    assert_eq!(r.field(".pr.number"), "7");
}

// @test "a PR from our own fork is kept"
#[test]
fn a_pr_from_our_own_fork_is_kept() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.txt", "a");
    sd.pr_fixture("feat", &[".isCrossRepository = true", r#".headRepositoryOwner = {login: "me"}"#]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".pr.number"), "7");
}

// @test "gh failing: GitHub-derived answers are unknown, not clean"
#[test]
fn gh_failing_github_derived_answers_are_unknown_not_clean() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.txt", "a");
    let r = sd.state_env(&[("GH_FAKE_FAIL", "gh: down")], &[]);
    assert_eq!(r.field(".fresh"), "null");
    assert_eq!(r.field(".threads.owed_bot"), "null");
    assert_eq!(r.field(".threads.owed_human"), "null");
    assert_eq!(r.field(".parent.merged"), "null");
}

// @test "parent lookup failing: parent merged is unknown"
#[test]
fn parent_lookup_failing_parent_merged_is_unknown() {
    let sd = sd();
    sd.git(&["switch", "-qc", "parent"]);
    sd.commit("p.txt", "p");
    sd.git(&["push", "-q", "origin", "parent"]);
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("c.txt", "c");
    sd.git(&["config", "branch.feat.sd-parent", "parent"]);
    let r = sd.state_env(&[("GH_FAKE_FAIL_FOR", "parent")], &[]);
    assert_eq!(r.field(".parent.merged"), "null");
    assert_eq!(r.field(".fresh"), "null");
    assert_has(&r.field(".errors[0]"), &["boom for parent"]);
}

// ── O1 the PR by its stored number (#122) ────────────────────────────────────

// @test "PR number: the one found is stored, and the next run asks for it by number alone"
#[test]
fn pr_number_the_one_found_is_stored_and_the_next_run_asks_for_it_by_number_alone() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.txt", "a");
    sd.pr_fixture("feat", &[]);
    sd.state(&[]);
    assert_eq!(sd.calls(), "feat #7 ");
    assert_eq!(sd.git(&["config", "branch.feat.sd-pr"]), "7");
    let r = sd.state(&[]);
    assert_eq!(sd.calls(), "#7 ");
    assert_eq!(r.field(".pr.number"), "7");
}

// @test "PR number: a stored one is used without discovery"
#[test]
fn pr_number_a_stored_one_is_used_without_discovery() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.txt", "a");
    sd.pr_fixture("feat", &[]);
    sd.git(&["config", "branch.feat.sd-pr", "7"]);
    let r = sd.state(&[]);
    assert_eq!(sd.calls(), "#7 ");
    assert_eq!(r.field(".pr.state"), "OPEN");
}

// @test "PR number: a stored one whose PR has another head re-discovers and stores the branch's"
#[test]
fn pr_number_a_stored_one_whose_pr_has_another_head_re_discovers_and_stores_the_branchs() {
    let sd = sd();
    sd.git(&["switch", "-qc", "other"]);
    sd.commit("o.txt", "o");
    sd.pr_fixture("other", &[".number = 8"]);
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.txt", "a");
    sd.pr_fixture("feat", &[]);
    sd.git(&["config", "branch.feat.sd-pr", "8"]);
    let r = sd.state(&[]);
    assert_eq!(sd.calls(), "#8 feat #7 ");
    assert_eq!(r.field(".pr.number"), "7");
    assert_eq!(sd.git(&["config", "branch.feat.sd-pr"]), "7");
}

// @test "PR number: a stored one GitHub can't resolve re-discovers, with no error"
#[test]
fn pr_number_a_stored_one_github_cant_resolve_re_discovers_with_no_error() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.txt", "a");
    sd.pr_fixture("feat", &[]);
    sd.git(&["config", "branch.feat.sd-pr", "42"]);
    let r = sd.state(&[]);
    assert_eq!(sd.calls(), "#42 feat #7 ");
    assert_eq!(r.field(".pr.number"), "7");
    assert_eq!(r.field(".errors | length"), "0");
}

// @test "PR number: a stored one closed without merging re-discovers, and a newer PR from the branch wins"
#[test]
fn pr_number_a_stored_one_closed_without_merging_re_discovers_and_a_newer_pr_from_the_branch_wins() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.txt", "a");
    sd.pr_fixture("feat", &[".number = 9"]);
    sd.edit_fixture("feat", r#".data.repository.pullRequests.nodes |= . + [(.[0] | .number = 5 | .state = "CLOSED")]"#);
    sd.git(&["config", "branch.feat.sd-pr", "5"]);
    let r = sd.state(&[]);
    assert_eq!(sd.calls(), "#5 feat #9 ");
    assert_eq!(r.field(".pr.number"), "9");
    assert_eq!(sd.git(&["config", "branch.feat.sd-pr"]), "9");
}

// @test "PR number: a stored closed one that discovery still picks is kept, not fetched again"
#[test]
fn pr_number_a_stored_closed_one_that_discovery_still_picks_is_kept_not_fetched_again() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.txt", "a");
    sd.pr_fixture("feat", &[".number = 5", r#".state = "CLOSED""#]);
    sd.git(&["config", "branch.feat.sd-pr", "5"]);
    let r = sd.state(&[]);
    assert_eq!(sd.calls(), "#5 feat ");
    assert_eq!(r.field(".pr.number"), "5");
    assert_eq!(r.field(".pr.state"), "CLOSED");
    assert_eq!(sd.git(&["config", "branch.feat.sd-pr"]), "5");
}

// @test "PR number: a stored merged one stays, even with a newer open PR from the branch"
#[test]
fn pr_number_a_stored_merged_one_stays_even_with_a_newer_open_pr_from_the_branch() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.txt", "a");
    sd.pr_fixture("feat", &[".number = 9"]);
    sd.edit_fixture("feat", r#".data.repository.pullRequests.nodes |= . + [(.[0] | .number = 5 | .state = "MERGED")]"#);
    sd.git(&["config", "branch.feat.sd-pr", "5"]);
    let r = sd.state(&[]);
    assert_eq!(sd.calls(), "#5 ");
    assert_eq!(r.field(".pr.state"), "MERGED");
}

// @test "PR number: a stored one dropped when discovery finds no PR for the branch"
#[test]
fn pr_number_a_stored_one_dropped_when_discovery_finds_no_pr_for_the_branch() {
    let sd = sd();
    sd.git(&["switch", "-qc", "other"]);
    sd.commit("o.txt", "o");
    sd.pr_fixture("other", &[".number = 8"]);
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.txt", "a");
    sd.git(&["config", "branch.feat.sd-pr", "8"]);
    let r = sd.state(&[]);
    assert_eq!(sd.calls(), "#8 feat ");
    assert_eq!(r.field(".pr"), "null");
    assert_eq!(sd.git_try(&["config", "branch.feat.sd-pr"]).unwrap_or_default(), "");
}

// @test "PR number: no PR yet costs one discovery a run, and stores nothing"
#[test]
fn pr_number_no_pr_yet_costs_one_discovery_a_run_and_stores_nothing() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.txt", "a");
    sd.state(&[]);
    assert_eq!(sd.calls(), "feat ");
    sd.state(&[]);
    assert_eq!(sd.calls(), "feat ");
    assert_eq!(sd.git_try(&["config", "branch.feat.sd-pr"]).unwrap_or_default(), "");
}

// @test "PR number: the parent's is stored and used the same way"
#[test]
fn pr_number_the_parents_is_stored_and_used_the_same_way() {
    let sd = sd();
    sd.git(&["switch", "-qc", "parent"]);
    sd.commit("p.txt", "p");
    sd.git(&["push", "-q", "origin", "parent"]);
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("c.txt", "c");
    sd.git(&["config", "branch.feat.sd-parent", "parent"]);
    sd.pr_fixture("feat", &[]);
    sd.pr_fixture("parent", &[".number = 3"]);
    sd.state(&[]);
    assert_eq!(sd.calls(), "feat #7 parent #3 ");
    assert_eq!(sd.git(&["config", "branch.parent.sd-pr"]), "3");
    let r = sd.state(&[]);
    assert_eq!(sd.calls(), "#7 #3 ");
    assert_eq!(r.field(".parent.pr.number"), "3");
}

// @test "PR number: a PR discovery lists but the fetch can't resolve is an error, and no PR"
#[test]
fn pr_number_a_pr_discovery_lists_but_the_fetch_cant_resolve_is_an_error_and_no_pr() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.txt", "a");
    sd.pr_fixture("feat", &[]);
    let r = sd.state_env(&[("GH_FAKE_GONE", "7")], &[]);
    assert_eq!(sd.calls(), "feat #7 ");
    assert_eq!(r.field(".pr"), "null");
    assert_eq!(r.field(".fresh"), "null");
    assert_has(&r.field(".errors[0]"), &["GitHub has no PR #7, which it just listed"]);
}

// @test "PR number: a parent that isn't a local branch has its number looked up, not stored"
#[test]
fn pr_number_a_parent_that_isnt_a_local_branch_has_its_number_looked_up_not_stored() {
    let sd = sd();
    sd.git(&["switch", "-qc", "parent"]);
    sd.commit("p.txt", "p");
    sd.git(&["push", "-q", "origin", "parent"]);
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("c.txt", "c");
    sd.git(&["branch", "-D", "-q", "parent"]);
    sd.git(&["config", "branch.feat.sd-parent", "parent"]);
    sd.pr_fixture("feat", &[]);
    sd.pr_fixture("parent", &[".number = 3"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".parent.pr.number"), "3");
    assert_eq!(sd.git_try(&["config", "branch.parent.sd-pr"]).unwrap_or_default(), "");
}

// @test "PR number: gh failing stores nothing and keeps what was stored"
#[test]
fn pr_number_gh_failing_stores_nothing_and_keeps_what_was_stored() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.txt", "a");
    sd.state_env(&[("GH_FAKE_FAIL", "gh: down")], &[]);
    assert_eq!(sd.git_try(&["config", "branch.feat.sd-pr"]).unwrap_or_default(), "");
    sd.git(&["config", "branch.feat.sd-pr", "7"]);
    sd.state_env(&[("GH_FAKE_FAIL", "gh: down")], &[]);
    assert_eq!(sd.git(&["config", "branch.feat.sd-pr"]), "7");
}

// ── rate limit (#122) ────────────────────────────────────────────────────────

// @test "rate_limit: the calls' cost summed, with the last call's remaining and reset"
#[test]
fn rate_limit_the_calls_cost_summed_with_the_last_calls_remaining_and_reset() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.txt", "a");
    sd.pr_fixture("feat", &[]);
    let r = sd.state_env(&[("GH_FAKE_REMAINING", "1234")], &[]);
    assert_eq!(r.field_c(".rate_limit"), r#"{"cost":3,"remaining":1234,"reset_at":"2026-10-16T13:00:00Z","limited":false}"#);
    let r = sd.state(&[]);
    assert_eq!(r.field(".rate_limit.cost"), "2");
}

// @test "rate_limit: GitHub refusing for the rate limit is limited; another failure isn't"
#[test]
fn rate_limit_github_refusing_for_the_rate_limit_is_limited_another_failure_isnt() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.txt", "a");
    let r = sd.state_env(&[("GH_FAKE_FAIL", "gh: API rate limit exceeded for user ID 1.")], &[]);
    assert_eq!(r.field_c(".rate_limit"), r#"{"cost":null,"remaining":null,"reset_at":null,"limited":true}"#);
    let r = sd.state_env(&[("GH_FAKE_FAIL", "gh: down")], &[]);
    assert_eq!(r.field(".rate_limit.limited"), "false");
}

// @test "summary: errors are flagged"
#[test]
fn summary_errors_are_flagged() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.txt", "a");
    let r = sd.run_env(&[("GH_FAKE_FAIL", "gh: down")], &[]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &["⚠ 1 error(s)"]);
}

// ── O2 parent ────────────────────────────────────────────────────────────────

impl Sd {
    /// parent pushed with p.txt, then feat on it with c.txt.
    fn parent_and_feat(&self) {
        self.git(&["switch", "-qc", "parent"]);
        self.commit("p.txt", "p");
        self.git(&["push", "-q", "origin", "parent"]);
        self.git(&["switch", "-qc", "feat"]);
        self.commit("c.txt", "c");
    }
}

// @test "parent comes from the PR base when nothing is configured"
#[test]
fn parent_comes_from_the_pr_base_when_nothing_is_configured() {
    let sd = sd();
    sd.parent_and_feat();
    sd.pr_fixture("feat", &[r#".baseRefName = "parent""#]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".parent.name"), "parent");
    assert_eq!(r.field(".parent.source"), "pr");
}

// @test "configured sd-parent wins over the PR base"
#[test]
fn configured_sd_parent_wins_over_the_pr_base() {
    let sd = sd();
    sd.parent_and_feat();
    sd.git(&["config", "branch.feat.sd-parent", "parent"]);
    sd.pr_fixture("feat", &[r#".baseRefName = "main""#]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".parent.name"), "parent");
    assert_eq!(r.field(".parent.source"), "config");
}

// @test "parent PR state is looked up for a non-default parent"
#[test]
fn parent_pr_state_is_looked_up_for_a_non_default_parent() {
    let sd = sd();
    sd.parent_and_feat();
    sd.git(&["config", "branch.feat.sd-parent", "parent"]);
    sd.pr_fixture("parent", &[".number = 3"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".parent.pr.number"), "3");
    assert_eq!(r.field(".parent.merged"), "false");
}

// @test "parent closed without merging: not merged, and the state says so"
#[test]
fn parent_closed_without_merging_not_merged_and_the_state_says_so() {
    let sd = sd();
    sd.parent_and_feat();
    sd.git(&["config", "branch.feat.sd-parent", "parent"]);
    sd.pr_fixture("parent", &[".number = 3", r#".state = "CLOSED""#]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".parent.merged"), "false");
    assert_eq!(r.field(".parent.pr.state"), "CLOSED");
}

// @test "parent PR's commits are listed, [] when GitHub gives none"
#[test]
fn parent_prs_commits_are_listed_when_github_gives_none() {
    let sd = sd();
    sd.parent_and_feat();
    sd.git(&["config", "branch.feat.sd-parent", "parent"]);
    sd.pr_fixture("parent", &[r#".allCommits = {nodes: [{commit: {oid: "aaa"}}, {commit: {oid: "bbb"}}]}"#]);
    let r = sd.state(&[]);
    assert_eq!(r.field_c(".parent.pr.commits"), r#"["aaa","bbb"]"#);
    sd.pr_fixture("parent", &[]);
    let r = sd.state(&[]);
    assert_eq!(r.field_c(".parent.pr.commits"), "[]");
}

// @test "only the parent lookup asks GitHub for its PR's commits"
#[test]
fn only_the_parent_lookup_asks_github_for_its_prs_commits() {
    let sd = sd();
    sd.parent_and_feat();
    sd.git(&["config", "branch.feat.sd-parent", "parent"]);
    sd.pr_fixture("feat", &[]);
    sd.pr_fixture("parent", &[".number = 3"]);
    sd.state(&[]);
    assert_eq!(chomp(&fs::read_to_string(sd.gh("commits")).unwrap()), "feat\n#7 commits=false\nparent\n#3 commits=true");
}

// @test "bot: the configured trigger command is reported, null when there is none"
#[test]
fn bot_the_configured_trigger_command_is_reported_null_when_there_is_none() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".bot.command"), "null");
    sd.sd_config(&["set", "--global", "settings.botCommand", "/review-bot"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".bot.command"), "/review-bot");
}

// ── O3 sync vs parent ────────────────────────────────────────────────────────

// @test "sync: names the default branch"
#[test]
fn sync_names_the_default_branch() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".sync.default"), "main");
}

// @test "sync: with GitHub unreachable, the default is the remote's HEAD, as sd act push reads it (#159)"
#[test]
fn sync_with_github_unreachable_the_default_is_the_remotes_head_as_sd_act_push_reads_it_159() {
    let sd = sd();
    sd.git(&["push", "-q", "origin", "main:develop"]);
    sd.git_in("origin.git", &["symbolic-ref", "HEAD", "refs/heads/develop"]);
    sd.git_status(&["remote", "set-head", "origin", "-d"]);
    sd.git(&["switch", "-qc", "feat"]);
    let r = sd.state_env(&[("GH_FAKE_FAIL", "gh: network down")], &[]);
    assert_eq!(r.field(".sync.default"), "develop");
}

// @test "sync: up to date with the parent"
#[test]
fn sync_up_to_date_with_the_parent() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.txt", "a");
    let r = sd.state(&[]);
    assert_eq!(r.field(".sync.target"), "main");
    assert_eq!(r.field(".sync.behind"), "0");
    assert_eq!(r.field(".sync.up_to_date"), "true");
}

// @test "sync: ahead counts the branch's own commits over the target"
#[test]
fn sync_ahead_counts_the_branchs_own_commits_over_the_target() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.txt", "a");
    sd.commit("b.txt", "b");
    sd.main_moves("c.txt", "c");
    sd.git(&["fetch", "-q"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".sync.ahead"), "2");
    assert_eq!(r.field(".sync.behind"), "1");
}

// @test "sync: parent moved, restack would be clean"
#[test]
fn sync_parent_moved_restack_would_be_clean() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.txt", "a");
    sd.main_moves("b.txt", "b");
    sd.git(&["fetch", "-q"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".sync.behind"), "1");
    assert_eq!(r.field(".sync.up_to_date"), "false");
    assert_eq!(r.field(".sync.conflict"), "false");
}

// @test "sync: parent moved, restack would conflict"
#[test]
fn sync_parent_moved_restack_would_conflict() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("file.txt", "one\nOURS\nthree");
    sd.main_moves("file.txt", "one\nTHEIRS\nthree");
    sd.git(&["fetch", "-q"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".sync.conflict"), "true");
}

// @test "sync: squash-merged parent retargets to main and replays only our change"
#[test]
fn sync_squash_merged_parent_retargets_to_main_and_replays_only_our_change() {
    let sd = sd();
    sd.git(&["switch", "-qc", "parent"]);
    sd.commit("p.txt", "p");
    sd.git(&["push", "-q", "origin", "parent"]);
    let parent_sha = sd.git(&["rev-parse", "parent"]);
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("c.txt", "c");
    sd.git(&["config", "branch.feat.sd-parent", "parent"]);
    // the squash merge lands on main elsewhere (own message, or it hashes identical to parent's commit)
    sd.commit_in("other", "p.txt", "p");
    sd.git_in("other", &["commit", "-q", "--amend", "-m", "Squash parent (#3)"]);
    sd.git_in("other", &["push", "-q", "origin", "main"]);
    sd.git(&["fetch", "-q"]);
    sd.pr_fixture("parent", &[r#".state = "MERGED""#, &head_oid(&parent_sha)]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".parent.merged"), "true");
    assert_eq!(r.field(".sync.target"), "main");
    assert_eq!(r.field(".sync.old_base"), parent_sha);
    assert_eq!(r.field(".sync.up_to_date"), "false");
    assert_eq!(r.field(".sync.conflict"), "false");
}

// @test "sync: the base pin is the old base when it is an ancestor"
#[test]
fn sync_the_base_pin_is_the_old_base_when_it_is_an_ancestor() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.txt", "a");
    let pin = sd.git(&["rev-parse", "main"]);
    sd.git(&["update-ref", "refs/sheepdog/feat/base", &pin]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".sync.old_base"), pin);
    assert_eq!(r.field(".sync.by_tree"), "false");
}

// @test "sync: rebased onto main outside sheepdog, pin left below it: the merge-base, not main's commits (#120)"
#[test]
fn sync_rebased_onto_main_outside_sheepdog_pin_left_below_it_the_merge_base_not_mains_commits_120() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.txt", "a");
    sd.git(&["update-ref", "refs/sheepdog/feat/base", "main"]);
    sd.main_moves("m.txt", "m");
    sd.git(&["fetch", "-q"]);
    sd.git(&["rebase", "-q", "origin/main"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".sync.up_to_date"), "true");
    assert_eq!(r.field(".sync.old_base"), sd.git(&["rev-parse", "origin/main"]));
    assert_eq!(r.field(".errors | length"), "0");
}

// @test "sync: stacked on an open parent, main moved on: the pin stays the base (#120)"
#[test]
fn sync_stacked_on_an_open_parent_main_moved_on_the_pin_stays_the_base_120() {
    let sd = sd();
    sd.git(&["switch", "-qc", "parent"]);
    sd.commit("p.txt", "p1");
    sd.git(&["push", "-q", "origin", "parent"]);
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("c.txt", "c");
    sd.git(&["config", "branch.feat.sd-parent", "parent"]);
    sd.git(&["update-ref", "refs/sheepdog/feat/base", "parent"]);
    sd.main_moves("m.txt", "m");
    sd.git(&["fetch", "-q"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".sync.target"), "parent");
    assert_eq!(r.field(".sync.old_base"), sd.git(&["rev-parse", "parent"]));
}

// @test "sync: stacked, pin lost, parent merged and deleted: no old base, and says why"
#[test]
fn sync_stacked_pin_lost_parent_merged_and_deleted_no_old_base_and_says_why() {
    let sd = sd();
    let (_, p2) = sd.lost_pin_stack("");
    let r = sd.state(&[]);
    assert_eq!(r.field(".sync.target"), "main");
    assert_eq!(r.field(".sync.old_base"), "null");
    assert_eq!(r.field(".sync.conflict"), "null");
    assert_has(&r.errors(), &[&format!("base pin {} is no longer in feat's history (rewritten outside sheepdog?)", &p2[..7])]);
}

// @test "sync: pin lost, but all of the merged parent is in HEAD: the merged head is the base, by tree"
#[test]
fn sync_pin_lost_but_all_of_the_merged_parent_is_in_head_the_merged_head_is_the_base_by_tree() {
    let sd = sd();
    let (p1, p2) = sd.lost_pin_stack("same");
    sd.pr_fixture("parent", &[r#".state = "MERGED""#, &head_oid(&p2), &all_commits(&[&p1, &p2])]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".sync.old_base"), p2);
    assert_eq!(r.field(".sync.by_tree"), "true");
    assert_eq!(r.field(".sync.conflict"), "false");
}

// @test "sync: pin lost, HEAD still has a line the parent deleted: no tree base"
#[test]
fn sync_pin_lost_head_still_has_a_line_the_parent_deleted_no_tree_base() {
    let sd = sd();
    sd.git(&["switch", "-qc", "parent"]);
    sd.write("r.txt", "x\ny\nz\n");
    sd.git(&["add", "r.txt"]);
    sd.git(&["commit", "-qm", "p1"]);
    sd.write("r.txt", "x\nz\n");
    sd.git(&["commit", "-qam", "p2 drops y"]);
    sd.git(&["push", "-q", "origin", "parent"]);
    let p2 = sd.git(&["rev-parse", "parent"]);
    sd.git(&["switch", "-qc", "feat", "parent~1"]);
    sd.git(&["commit", "-q", "--allow-empty", "-m", "not really p2"]);
    sd.commit("c.txt", "c");
    sd.git(&["config", "branch.feat.sd-parent", "parent"]);
    sd.git(&["update-ref", "refs/sheepdog/feat/base", &p2]);
    sd.squash_parent_away();
    let p1 = sd.git(&["rev-parse", &format!("{p2}~1")]);
    sd.pr_fixture("parent", &[r#".state = "MERGED""#, &head_oid(&p2), &all_commits(&[&p1, &p2])]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".sync.by_tree"), "false");
    assert_eq!(r.field(".sync.old_base"), "null");
}

// @test "sync: pin lost, HEAD holds the parent as one copy of two edits to a line: by tree"
#[test]
fn sync_pin_lost_head_holds_the_parent_as_one_copy_of_two_edits_to_a_line_by_tree() {
    let sd = sd();
    sd.git(&["switch", "-qc", "parent"]);
    sd.commit("s.txt", "X");
    sd.commit("s.txt", "Y");
    sd.git(&["push", "-q", "origin", "parent"]);
    let p2 = sd.git(&["rev-parse", "parent"]);
    sd.git(&["switch", "-qc", "feat", "main"]);
    sd.commit("s.txt", "Y");
    sd.commit("c.txt", "c");
    sd.git(&["config", "branch.feat.sd-parent", "parent"]);
    sd.git(&["update-ref", "refs/sheepdog/feat/base", &p2]);
    sd.squash_parent_away();
    sd.pr_fixture("parent", &[r#".state = "MERGED""#, &head_oid(&p2)]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".sync.by_tree"), "true");
    assert_eq!(r.field(".sync.old_base"), p2);
}

// @test "sync: pin at an older parent head, HEAD holds the whole merged parent: the merged head is the tree base"
#[test]
fn sync_pin_at_an_older_parent_head_head_holds_the_whole_merged_parent_the_merged_head_is_the_tree_base() {
    let sd = sd();
    sd.git(&["switch", "-qc", "parent"]);
    sd.commit("s.txt", "X");
    let p1 = sd.git(&["rev-parse", "parent"]);
    sd.commit("t.txt", "Y");
    sd.git(&["push", "-q", "origin", "parent"]);
    let p2 = sd.git(&["rev-parse", "parent"]);
    sd.git(&["switch", "-qc", "feat", "main"]);
    sd.write("s.txt", "X\n");
    sd.write("t.txt", "Y\n");
    sd.git(&["add", "s.txt", "t.txt"]);
    sd.git(&["commit", "-qm", "parent, copied"]);
    sd.commit("c.txt", "c");
    sd.git(&["config", "branch.feat.sd-parent", "parent"]);
    sd.git(&["update-ref", "refs/sheepdog/feat/base", &p1]);
    sd.squash_parent_away();
    sd.pr_fixture("parent", &[r#".state = "MERGED""#, &head_oid(&p2)]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".sync.by_tree"), "true");
    assert_eq!(r.field(".sync.old_base"), p2);
}

// @test "sync: pin lost, parent moved onto a newer main than HEAD has: no tree base, which would revert main"
#[test]
fn sync_pin_lost_parent_moved_onto_a_newer_main_than_head_has_no_tree_base_which_would_revert_main() {
    let sd = sd();
    sd.git(&["switch", "-qc", "parent"]);
    sd.commit("s.txt", "X");
    sd.git(&["push", "-q", "origin", "parent"]);
    let p1 = sd.git(&["rev-parse", "parent"]);
    sd.git(&["switch", "-qc", "feat", "main"]);
    sd.write("s.txt", "X\n");
    sd.git(&["add", "s.txt"]);
    sd.git(&["commit", "-qm", "copy of p1"]);
    sd.commit("c.txt", "c");
    sd.git(&["config", "branch.feat.sd-parent", "parent"]);
    sd.git(&["update-ref", "refs/sheepdog/feat/base", &p1]);
    sd.main_moves("m.txt", "M1");
    sd.git_in("other", &["fetch", "-q", "origin"]);
    sd.git_in("other", &["switch", "-qc", "parent", "origin/parent"]);
    sd.git_in("other", &["rebase", "-q", "main"]);
    sd.git_in("other", &["push", "-qf", "origin", "parent"]);
    sd.git_in("other", &["switch", "-q", "main"]);
    sd.git(&["fetch", "-q"]);
    let p1b = sd.git(&["rev-parse", "origin/parent"]);
    sd.squash_parent_away();
    sd.pr_fixture("parent", &[r#".state = "MERGED""#, &head_oid(&p1b)]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".sync.by_tree"), "false");
    assert_eq!(r.field(".sync.old_base"), "null");
}

// @test "sync: pin lost, HEAD on a newer main than the parent was: no tree base, whose restack would roll back"
#[test]
fn sync_pin_lost_head_on_a_newer_main_than_the_parent_was_no_tree_base_whose_restack_would_roll_back() {
    let sd = sd();
    sd.git(&["switch", "-qc", "parent"]);
    sd.commit("s.txt", "X");
    sd.git(&["push", "-q", "origin", "parent"]);
    let p1 = sd.git(&["rev-parse", "parent"]);
    sd.main_moves("m.txt", "M1");
    sd.git(&["fetch", "-q"]);
    sd.git(&["switch", "-qc", "feat", "origin/main"]);
    sd.write("s.txt", "X\n");
    sd.git(&["add", "s.txt"]);
    sd.git(&["commit", "-qm", "copy of p1"]);
    sd.commit("c.txt", "c");
    sd.git(&["config", "branch.feat.sd-parent", "parent"]);
    sd.git(&["update-ref", "refs/sheepdog/feat/base", &p1]);
    sd.squash_parent_away();
    sd.pr_fixture("parent", &[r#".state = "MERGED""#, &head_oid(&p1)]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".sync.by_tree"), "false");
    assert_eq!(r.field(".sync.old_base"), "null");
}

// @test "sync: pin lost, parent merged with a merge commit: no tree base, whatever HEAD holds"
#[test]
fn sync_pin_lost_parent_merged_with_a_merge_commit_no_tree_base_whatever_head_holds() {
    let sd = sd();
    sd.git(&["switch", "-qc", "parent"]);
    sd.commit("p.txt", "p1");
    sd.commit("q.txt", "p2");
    sd.git(&["push", "-q", "origin", "parent"]);
    let p2 = sd.git(&["rev-parse", "parent"]);
    sd.git(&["switch", "-qc", "feat", "parent~1"]);
    sd.commit("q.txt", "p2, rewritten");
    sd.commit("c.txt", "c");
    sd.git(&["config", "branch.feat.sd-parent", "parent"]);
    sd.git(&["update-ref", "refs/sheepdog/feat/base", &p2]);
    sd.git_in("other", &["fetch", "-q", "origin"]);
    sd.git_in("other", &["merge", "-q", "--no-ff", "--no-edit", "origin/parent"]);
    sd.git_in("other", &["push", "-q", "origin", "main", ":parent"]);
    sd.git(&["fetch", "-q", "--prune"]);
    sd.pr_fixture("parent", &[r#".state = "MERGED""#, &head_oid(&p2)]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".sync.by_tree"), "false");
}

// @test "sync: pin lost, a parent commit that is in HEAD only changed: no old base"
#[test]
fn sync_pin_lost_a_parent_commit_that_is_in_head_only_changed_no_old_base() {
    let sd = sd();
    let (p1, p2) = sd.lost_pin_stack("");
    sd.pr_fixture("parent", &[r#".state = "MERGED""#, &head_oid(&p2), &all_commits(&[&p1, &p2])]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".sync.old_base"), "null");
    assert_eq!(r.field(".sync.by_tree"), "false");
}

// @test "sync: stacked, no pin, parent merged and deleted: no old base, and says why"
#[test]
fn sync_stacked_no_pin_parent_merged_and_deleted_no_old_base_and_says_why() {
    let sd = sd();
    sd.lost_pin_stack("nopin");
    let r = sd.state(&[]);
    assert_eq!(r.field(".sync.old_base"), "null");
    assert_has(&r.errors(), &["no base for feat on parent"]);
}

// @test "sync: stacked, no base, but already on main (rebased by hand): up to date, no error"
#[test]
fn sync_stacked_no_base_but_already_on_main_rebased_by_hand_up_to_date_no_error() {
    let sd = sd();
    sd.git(&["switch", "-qc", "parent"]);
    sd.commit("p.txt", "p1");
    sd.git(&["push", "-q", "origin", "parent"]);
    let p1 = sd.git(&["rev-parse", "parent"]);
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("c.txt", "c");
    sd.git(&["config", "branch.feat.sd-parent", "parent"]);
    sd.squash_merged("parent");
    sd.git(&["fetch", "-q", "--prune"]);
    sd.git(&["rebase", "-q", "--onto", "origin/main", "parent", "feat"]);
    sd.pr_fixture("parent", &[r#".state = "MERGED""#, &head_oid(&p1)]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".sync.up_to_date"), "true");
    assert_eq!(r.field(".sync.old_base"), sd.git(&["rev-parse", "origin/main"]));
    assert_eq!(r.field_c(".errors"), "[]");
}

// @test "sync: stacked, rebased onto main by hand, stale pin: target is the base, pin reported"
#[test]
fn sync_stacked_rebased_onto_main_by_hand_stale_pin_target_is_the_base_pin_reported() {
    let sd = sd();
    sd.git(&["switch", "-qc", "parent"]);
    sd.commit("p.txt", "p1");
    sd.git(&["push", "-q", "origin", "parent"]);
    let p1 = sd.git(&["rev-parse", "parent"]);
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("c.txt", "c");
    sd.git(&["config", "branch.feat.sd-parent", "parent"]);
    let elsewhere = sd.git(&["commit-tree", "HEAD^{tree}", "-m", "elsewhere"]);
    sd.git(&["update-ref", "refs/sheepdog/feat/base", &elsewhere]);
    sd.squash_merged("parent");
    sd.git(&["fetch", "-q", "--prune"]);
    sd.git(&["rebase", "-q", "--onto", "origin/main", "parent", "feat"]);
    sd.pr_fixture("parent", &[r#".state = "MERGED""#, &head_oid(&p1), &all_commits(&[&p1])]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".sync.up_to_date"), "true");
    assert_eq!(r.field(".sync.old_base"), sd.git(&["rev-parse", "origin/main"]));
    assert_has(&r.errors(), &["is no longer in feat's history"]);
}

// @test "sync: stacked, main merged in rather than rebased: no base, the parent's commits are still there"
#[test]
fn sync_stacked_main_merged_in_rather_than_rebased_no_base_the_parents_commits_are_still_there() {
    let sd = sd();
    sd.git(&["switch", "-qc", "parent"]);
    sd.commit("p.txt", "p1");
    sd.git(&["push", "-q", "origin", "parent"]);
    let p1 = sd.git(&["rev-parse", "parent"]);
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("c.txt", "c");
    sd.git(&["config", "branch.feat.sd-parent", "parent"]);
    sd.git(&["switch", "-q", "parent"]);
    sd.commit("q.txt", "p2");
    sd.git(&["push", "-q", "origin", "parent"]);
    let p2 = sd.git(&["rev-parse", "parent"]);
    sd.git(&["switch", "-q", "feat"]);
    sd.squash_merged("parent");
    sd.git(&["fetch", "-q", "--prune"]);
    sd.git(&["merge", "-q", "--no-edit", "origin/main"]);
    sd.pr_fixture("parent", &[r#".state = "MERGED""#, &head_oid(&p2), &all_commits(&[&p1, &p2])]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".sync.up_to_date"), "true");
    assert_eq!(r.field(".sync.old_base"), "null");
    assert_has(&r.errors(), &["no base for feat on parent"]);
}

// @test "sync: already on main, but a stale pin: still reported"
#[test]
fn sync_already_on_main_but_a_stale_pin_still_reported() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.txt", "a");
    sd.git(&["update-ref", "refs/sheepdog/feat/base", &sd.git(&["rev-parse", "feat"])]);
    sd.git(&["commit", "-q", "--amend", "-m", "rewritten"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".sync.up_to_date"), "true");
    assert_has(&r.errors(), &["is no longer in feat's history"]);
}

// Without a pin or a fork-point, the merge-base with an open parent can lie
// below commits the branch holds from the parent's old head (#97).
// @test "sync: stacked on an open parent, no pin or fork-point: no fallback to the merge-base, and says why (#97)"
#[test]
fn sync_stacked_on_an_open_parent_no_pin_or_fork_point_no_fallback_to_the_merge_base_and_says_why_97() {
    let sd = sd();
    sd.git(&["switch", "-qc", "parent"]);
    sd.commit("p.txt", "p");
    sd.git(&["push", "-q", "origin", "parent"]);
    sd.git(&["switch", "-qc", "feat", "main"]);
    sd.commit("c.txt", "c");
    sd.git(&["config", "branch.feat.sd-parent", "parent"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".sync.target"), "parent");
    assert_eq!(r.field(".sync.up_to_date"), "false");
    assert_eq!(r.field(".sync.old_base"), "null");
    assert_has(&r.errors(), &["no base for feat on parent: no base pin, and no fork-point"]);
}

// @test "sync: open parent, stale pin, no fork-point: only the stale-pin error (#97)"
#[test]
fn sync_open_parent_stale_pin_no_fork_point_only_the_stale_pin_error_97() {
    let sd = sd();
    sd.git(&["switch", "-qc", "parent"]);
    sd.commit("p.txt", "p1");
    sd.git(&["push", "-q", "origin", "parent"]);
    sd.git(&["switch", "-qc", "feat", "main"]);
    sd.commit("c.txt", "c");
    sd.git(&["config", "branch.feat.sd-parent", "parent"]);
    let pin = sd.git(&["rev-parse", "feat"]);
    sd.git(&["update-ref", "refs/sheepdog/feat/base", &pin]);
    sd.git(&["commit", "-q", "--amend", "-m", "rewritten"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".sync.old_base"), "null");
    assert_eq!(r.field(".errors | length"), "1");
    assert_has(&r.field(".errors[0]"), &[&format!("base pin {} is no longer in feat's history", &pin[..7])]);
}

// @test "sync: open parent amended and force-pushed, no pin, reflog gone: no old base below the parent's commits (#97)"
#[test]
fn sync_open_parent_amended_and_force_pushed_no_pin_reflog_gone_no_old_base_below_the_parents_commits_97() {
    let sd = sd();
    sd.git(&["switch", "-qc", "parent"]);
    sd.commit("p.txt", "p1");
    sd.git(&["push", "-q", "origin", "parent"]);
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("c.txt", "c");
    sd.git(&["config", "branch.feat.sd-parent", "parent"]);
    sd.parent_amended_elsewhere();
    sd.git(&["fetch", "-q"]);
    sd.git(&["reflog", "expire", "--expire=now", "--expire-unreachable=now", "refs/remotes/origin/parent"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".sync.old_base"), "null");
    assert_has(&r.errors(), &["no base for feat on parent"]);
}

// @test "sync: on an open parent's tip, no pin, reflog gone: up to date on the parent's tip, no error (#97)"
#[test]
fn sync_on_an_open_parents_tip_no_pin_reflog_gone_up_to_date_on_the_parents_tip_no_error_97() {
    let sd = sd();
    sd.git(&["switch", "-qc", "parent"]);
    sd.commit("p.txt", "p1");
    sd.git(&["push", "-q", "origin", "parent"]);
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("c.txt", "c");
    sd.git(&["config", "branch.feat.sd-parent", "parent"]);
    sd.git(&["reflog", "expire", "--expire=now", "--expire-unreachable=now", "refs/remotes/origin/parent"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".sync.up_to_date"), "true");
    assert_eq!(r.field(".sync.old_base"), sd.git(&["rev-parse", "origin/parent"]));
    assert_eq!(r.field(".errors | length"), "0");
}

// @test "sync: stacked on an open parent, no pin, fork-point in the parent's reflog: the fork-point"
#[test]
fn sync_stacked_on_an_open_parent_no_pin_fork_point_in_the_parents_reflog_the_fork_point() {
    let sd = sd();
    sd.git(&["switch", "-qc", "parent"]);
    sd.commit("p.txt", "p1");
    sd.git(&["push", "-q", "origin", "parent"]);
    let p1 = sd.git(&["rev-parse", "parent"]);
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("c.txt", "c");
    sd.git(&["config", "branch.feat.sd-parent", "parent"]);
    sd.parent_amended_elsewhere();
    sd.git(&["fetch", "-q"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".sync.old_base"), p1);
    assert_eq!(r.field(".errors | length"), "0");
}

// @test "sync: a parent closed without merging, no pin or fork-point: the error says the parent closed, NEXT says parent-closed (#97)"
#[test]
fn sync_a_parent_closed_without_merging_no_pin_or_fork_point_the_error_says_the_parent_closed_next_says_parent_closed_97() {
    let sd = sd();
    sd.git(&["switch", "-qc", "parent"]);
    sd.commit("p.txt", "p1");
    sd.git(&["push", "-q", "origin", "parent"]);
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("c.txt", "c");
    sd.parent_moves_elsewhere();
    sd.git(&["fetch", "-q"]);
    sd.git(&["reflog", "expire", "--expire=now", "--expire-unreachable=now", "refs/remotes/origin/parent"]);
    sd.pr_fixture("parent", &[".number = 3", r#".state = "CLOSED""#]);
    sd.pr_fixture("feat", &[r#".baseRefName = "parent""#]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".sync.old_base"), "null");
    assert_eq!(r.errors(), "no base for feat on parent: parent's PR #3 was closed without merging; decide what feat targets");
    assert_has(&format!(" {} ", r.keys()), &[" parent-closed "]);
}

// @test "sync: a parent closed without merging, no PR yet, no pin or fork-point: the error says the parent closed (#97)"
#[test]
fn sync_a_parent_closed_without_merging_no_pr_yet_no_pin_or_fork_point_the_error_says_the_parent_closed_97() {
    let sd = sd();
    sd.git(&["switch", "-qc", "parent"]);
    sd.commit("p.txt", "p1");
    sd.git(&["push", "-q", "origin", "parent"]);
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("c.txt", "c");
    sd.git(&["config", "branch.feat.sd-parent", "parent"]);
    sd.parent_moves_elsewhere();
    sd.git(&["fetch", "-q"]);
    sd.git(&["reflog", "expire", "--expire=now", "--expire-unreachable=now", "refs/remotes/origin/parent"]);
    sd.pr_fixture("parent", &[".number = 3", r#".state = "CLOSED""#]);
    let r = sd.state(&[]);
    assert_has(&r.errors(), &["parent's PR #3 was closed without merging"]);
}

// @test "sync: not stacked, pin lost: falls back to the merge-base, and reports the pin"
#[test]
fn sync_not_stacked_pin_lost_falls_back_to_the_merge_base_and_reports_the_pin() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.txt", "a");
    let fork = sd.git(&["rev-parse", "main"]);
    let pin = sd.git(&["rev-parse", "feat"]);
    sd.git(&["update-ref", "refs/sheepdog/feat/base", &pin]);
    sd.git(&["commit", "-q", "--amend", "-m", "rewritten"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".sync.old_base"), fork);
    assert_has(&r.errors(), &[&format!("base pin {} is no longer in feat's history", &pin[..7])]);
}

// A restack, then git's own advice after a rejected plain push: pull --rebase
// onto the old remote head, which replays main's commit as a copy (#47).
// @test "sync: a restack undone by a plain push is reported as undone, with the copied commits counted (#47)"
#[test]
fn sync_a_restack_undone_by_a_plain_push_is_reported_as_undone_with_the_copied_commits_counted_47() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.txt", "a");
    sd.git(&["push", "-q", "-u", "origin", "feat"]);
    let pre = sd.git(&["rev-parse", "feat"]);
    sd.main_moves("m.txt", "m");
    sd.git(&["fetch", "-q"]);
    sd.git(&["rebase", "-q", "--onto", "origin/main", "main", "feat"]);
    sd.git(&["update-ref", "refs/sheepdog/feat/base", "origin/main"]);
    sd.git(&["reset", "-q", "--hard", &pre]);
    sd.git(&["cherry-pick", "origin/main"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".sync.undone"), "1");
    assert_eq!(r.field(".errors | length"), "0");
    assert_eq!(r.field(".next[0].key"), "restack-undone");
    let cmd = r.field(".next[0].cmd");
    assert!(cmd.starts_with("git rebase --onto origin/main ") && cmd.ends_with(" && sd act push"), "{cmd}");
    // Running the suggested rebase puts the branch back on the target without the copy.
    let rebase = cmd.split(" && ").next().unwrap();
    assert!(output(sd.command(Path::new("sh")).args(["-c", rebase]), None).status.success());
    assert_eq!(sd.git(&["log", "--format=%s", "origin/main..feat"]), "edit a.txt");
    let r = sd.state(&[]);
    assert_eq!(r.field(".sync.undone"), "0");
    assert_eq!(r.field(".sync.up_to_date"), "true");
}

// A child rewritten by hand (a reword of the parent's commits it carries keeps
// their author dates and subjects): copies of the parent's commits, but the
// branch's own remote head is gone from its history, so not an undone restack.
// @test "sync: a hand rewrite that keeps the parent's commits' dates and subjects is not an undone restack (#47)"
#[test]
fn sync_a_hand_rewrite_that_keeps_the_parents_commits_dates_and_subjects_is_not_an_undone_restack_47() {
    let sd = sd();
    sd.git(&["switch", "-qc", "parent"]);
    sd.commit("p.txt", "p1");
    sd.commit("q.txt", "p2");
    sd.git(&["push", "-q", "origin", "parent"]);
    let (p1, p2) = (sd.git(&["rev-parse", "parent~1"]), sd.git(&["rev-parse", "parent"]));
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("c.txt", "c");
    sd.git(&["push", "-q", "-u", "origin", "feat"]);
    sd.git(&["config", "branch.feat.sd-parent", "parent"]);
    sd.git(&["update-ref", "refs/sheepdog/feat/base", &p2]);
    let d1 = sd.git(&["log", "-1", "--format=%ad", &p1]);
    let n1 = sd.git_with(
        &[("GIT_AUTHOR_DATE", &d1), ("GIT_COMMITTER_DATE", "2030-01-01T00:00:00")],
        None,
        &["commit-tree", &format!("{p1}^{{tree}}"), "-p", &format!("{p1}^"), "-m", "edit p.txt"],
    );
    let d2 = sd.git(&["log", "-1", "--format=%ad", &p2]);
    let n2 = sd.git_with(
        &[("GIT_AUTHOR_DATE", &d2), ("GIT_COMMITTER_DATE", "2030-01-01T00:00:01")],
        None,
        &["commit-tree", &format!("{p2}^{{tree}}"), "-p", &n1, "-m", "edit q.txt"],
    );
    let n3 = sd.git(&["commit-tree", "feat^{tree}", "-p", &n2, "-m", "edit c.txt"]);
    sd.git(&["reset", "-q", "--hard", &n3]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".sync.undone"), "0");
    assert_has(&r.errors(), &[&format!("base pin {} is no longer in feat's history", &p2[..7])]);
    assert_eq!(r.field(".next[0].key"), "errors");
}

// @test "parent: a parent only here is reported as such, with no error, and NEXT ships it first (#105)"
#[test]
fn parent_a_parent_only_here_is_reported_as_such_with_no_error_and_next_ships_it_first_105() {
    let sd = sd();
    sd.git(&["switch", "-qc", "parent"]);
    sd.commit("p.txt", "p");
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("c.txt", "c");
    sd.git(&["config", "branch.feat.sd-parent", "parent"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".parent.local"), "true");
    assert_eq!(r.field(".parent.on_remote"), "false");
    assert_eq!(r.field(".parent.unpushed"), "true");
    assert_eq!(r.field(".errors | length"), "0");
    assert_eq!(r.field(".next[0].key"), "ship-parent");
    assert_eq!(r.field(".next[0].cmd"), "sd act ship parent");
    sd.git(&["push", "-q", "origin", "parent"]);
    sd.git(&["fetch", "-q"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".parent.on_remote"), "true");
    assert_eq!(r.field(".parent.unpushed"), "false");
    assert_ne!(r.field(".next[0].key"), "ship-parent");
}

// @test "parent: a parent closed without merging, its remote branch deleted, still errors (#105)"
#[test]
fn parent_a_parent_closed_without_merging_its_remote_branch_deleted_still_errors_105() {
    let sd = sd();
    sd.parent_and_feat();
    sd.git(&["config", "branch.feat.sd-parent", "parent"]);
    sd.pr_fixture("parent", &[r#".state = "CLOSED""#]);
    sd.git(&["push", "-q", "origin", ":parent"]);
    sd.git(&["fetch", "-q", "--prune"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".parent.on_remote"), "false");
    assert_has(&r.errors(), &["no origin/parent ref"]);
    assert_eq!(r.field(".next[0].key"), "errors");
}

// @test "parent: an open parent PR whose ref isn't fetched is an error, not an unpushed parent (#105)"
#[test]
fn parent_an_open_parent_pr_whose_ref_isnt_fetched_is_an_error_not_an_unpushed_parent_105() {
    let sd = sd();
    sd.git(&["switch", "-qc", "parent"]);
    sd.commit("p.txt", "p");
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("c.txt", "c");
    sd.git(&["config", "branch.feat.sd-parent", "parent"]);
    sd.pr_fixture("parent", &[]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".parent.on_remote"), "false");
    assert_eq!(r.field(".parent.unpushed"), "false");
    assert_has(&r.errors(), &["no origin/parent ref"]);
    assert_eq!(r.field(".next[0].key"), "errors");
}

// @test "parent: a parent only here whose PR lookup fails keeps the error, not ship-parent (#105)"
#[test]
fn parent_a_parent_only_here_whose_pr_lookup_fails_keeps_the_error_not_ship_parent_105() {
    let sd = sd();
    sd.git(&["switch", "-qc", "parent"]);
    sd.commit("p.txt", "p");
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("c.txt", "c");
    sd.git(&["config", "branch.feat.sd-parent", "parent"]);
    let r = sd.state_env(&[("GH_FAKE_FAIL_FOR", "parent")], &[]);
    assert_eq!(r.field(".parent.unpushed"), "false");
    assert_has(&r.errors(), &["no origin/parent ref"]);
    assert_eq!(r.field(r#"[.next[].key] | index("ship-parent")"#), "null");
}

// @test "parent: a parent only here that already landed on main by hand is not unpushed (#105)"
#[test]
fn parent_a_parent_only_here_that_already_landed_on_main_by_hand_is_not_unpushed_105() {
    let sd = sd();
    sd.git(&["switch", "-qc", "parent"]);
    sd.commit("p.txt", "p");
    sd.git(&["switch", "-q", "main"]);
    sd.git(&["merge", "-q", "--ff-only", "parent"]);
    sd.git(&["push", "-q", "origin", "main"]);
    sd.git(&["switch", "-qc", "feat", "parent"]);
    sd.commit("c.txt", "c");
    sd.git(&["config", "branch.feat.sd-parent", "parent"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".parent.on_remote"), "false");
    assert_eq!(r.field(".parent.unpushed"), "false");
    assert_eq!(r.field(r#"[.next[].key] | index("ship-parent")"#), "null");
    assert_has(&r.errors(), &["parent has no commits beyond origin/main and isn't on origin"]);
}

// @test "parent: a parent that exists nowhere is still an error"
#[test]
fn parent_a_parent_that_exists_nowhere_is_still_an_error() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("c.txt", "c");
    sd.git(&["config", "branch.feat.sd-parent", "nosuch"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".parent.local"), "false");
    assert_has(&r.errors(), &["no origin/nosuch ref"]);
}

// @test "sync: no shared history with the parent is reported, not guessed"
#[test]
fn sync_no_shared_history_with_the_parent_is_reported_not_guessed() {
    let sd = sd();
    sd.git(&["switch", "-q", "--orphan", "feat"]);
    sd.commit("a.txt", "a");
    let r = sd.state(&[]);
    assert_eq!(r.field(".sync.conflict"), "null");
    assert_has(&r.errors(), &["no merge base"]);
}

// ── O4 local vs remote ───────────────────────────────────────────────────────

impl Sd {
    /// feat with a.txt, pushed with upstream set.
    fn feat_pushed(&self) {
        self.git(&["switch", "-qc", "feat"]);
        self.commit("a.txt", "a");
        self.git(&["push", "-q", "-u", "origin", "feat"]);
    }

    /// Someone else commits <file> <content> to feat and pushes it.
    fn feat_moves_elsewhere(&self, file: &str, content: &str) {
        self.git_in("other", &["fetch", "-q"]);
        self.git_in("other", &["switch", "-q", "feat"]);
        self.commit_in("other", file, content);
        self.git_in("other", &["push", "-q", "origin", "feat"]);
    }

    fn amend_a(&self) {
        self.write("a.txt", "a2\n");
        self.git(&["commit", "-qa", "--amend", "--no-edit"]);
    }
}

// @test "push: unpushed branch has no remote"
#[test]
fn push_unpushed_branch_has_no_remote() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.txt", "a");
    sd.commit("b.txt", "b");
    let r = sd.state(&[]);
    assert_eq!(r.field(".push.remote_sha"), "null");
    assert_eq!(r.field(".push.ahead"), "2");
}

// @test "push: commits ahead of the pushed remote"
#[test]
fn push_commits_ahead_of_the_pushed_remote() {
    let sd = sd();
    sd.feat_pushed();
    sd.commit("b.txt", "b");
    let r = sd.state(&[]);
    assert_eq!(r.field(".push.ahead"), "1");
    assert_eq!(r.field(".push.behind"), "0");
    assert_eq!(r.field(".push.diverged"), "false");
}

// @test "push: remote rewritten locally (amend after push) is ours"
#[test]
fn push_remote_rewritten_locally_amend_after_push_is_ours() {
    let sd = sd();
    sd.feat_pushed();
    sd.amend_a();
    let r = sd.state(&[]);
    assert_eq!(r.field(".push.diverged"), "true");
    assert_eq!(r.field(".push.remote_ours"), "true");
}

// @test "push: someone else's commit on the remote is not ours"
#[test]
fn push_someone_elses_commit_on_the_remote_is_not_ours() {
    let sd = sd();
    sd.feat_pushed();
    sd.feat_moves_elsewhere("x.txt", "x");
    sd.git(&["fetch", "-q"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".push.behind"), "1");
    assert_eq!(r.field(".push.remote_ours"), "false");
    assert_eq!(r.field(".push.theirs"), "1");
}

// @test "push: our pre-amend commit under someone else's isn't counted as theirs"
#[test]
fn push_our_pre_amend_commit_under_someone_elses_isnt_counted_as_theirs() {
    let sd = sd();
    sd.feat_pushed();
    sd.feat_moves_elsewhere("x.txt", "x");
    sd.amend_a();
    sd.git(&["fetch", "-q"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".push.behind"), "2");
    assert_eq!(r.field(".push.remote_ours"), "false");
    assert_eq!(r.field(".push.theirs"), "1");
}

// @test "push: theirs is null when the remote head is ours"
#[test]
fn push_theirs_is_null_when_the_remote_head_is_ours() {
    let sd = sd();
    sd.feat_pushed();
    sd.amend_a();
    let r = sd.state(&[]);
    assert_eq!(r.field(".push.theirs"), "null");
}

// ── freshness: GitHub's SHAs vs the local remote-tracking refs ──────────────

// @test "fresh: remote-tracking refs match GitHub"
#[test]
fn fresh_remote_tracking_refs_match_github() {
    let sd = sd();
    sd.feat_pushed();
    sd.pr_fixture("feat", &[]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".fresh"), "true");
}

// @test "fresh: PR head moved on GitHub since the last fetch"
#[test]
fn fresh_pr_head_moved_on_github_since_the_last_fetch() {
    let sd = sd();
    sd.feat_pushed();
    sd.feat_moves_elsewhere("x.txt", "x");
    sd.pr_fixture("feat", &[&head_oid(&sd.git_in("other", &["rev-parse", "HEAD"]))]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".fresh"), "false");
    assert_has(&r.field(".stale[0]"), &["origin/feat"]);
}

// @test "fresh: parent PR head moved on GitHub since the last fetch"
#[test]
fn fresh_parent_pr_head_moved_on_github_since_the_last_fetch() {
    let sd = sd();
    sd.parent_and_feat();
    sd.git(&["config", "branch.feat.sd-parent", "parent"]);
    sd.pr_fixture("parent", &[r#".headRefOid = "1111111111111111111111111111111111111111""#]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".fresh"), "false");
    assert_has(&r.field(".stale[0]"), &["origin/parent"]);
}

// @test "summary: stale refs say to fetch"
#[test]
fn summary_stale_refs_say_to_fetch() {
    let sd = sd();
    sd.feat_pushed();
    sd.pr_fixture("feat", &[r#".headRefOid = "1111111111111111111111111111111111111111""#]);
    let r = sd.run(&[]);
    assert_has(&r.output, &["stale: fetch"]);
}

// GitHub can lag a push it was just sent: its head is an older value of the
// remote-tracking ref, so a fetch would get nothing (#246).
// @test "fresh: GitHub's PR head is behind origin/feat after a push: wait, not fetch"
#[test]
fn fresh_github_s_pr_head_is_behind_origin_feat_after_a_push_wait_not_fetch() {
    let sd = sd();
    sd.feat_pushed();
    sd.pr_fixture("feat", &[]);
    sd.commit("b.txt", "b");
    sd.git(&["push", "-q"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".fresh"), "true");
    assert_eq!(r.field(".stale | length"), "0");
    assert_has(&r.field(".lag[0]"), &["origin/feat"]);
    assert_eq!(r.field(".next[0].key"), "gh-wait");
    assert_eq!(r.field(".next[0].who"), "wait");
    let r = sd.run(&[]);
    assert_lacks(r.lines()[0], "stale");
    assert_has(r.lines()[0], &["GitHub lagging"]);
}

// @test "fresh: GitHub's PR head is behind a force-push of origin/feat: wait, not fetch"
#[test]
fn fresh_github_s_pr_head_is_behind_a_force_push_of_origin_feat_wait_not_fetch() {
    let sd = sd();
    sd.feat_pushed();
    sd.pr_fixture("feat", &[]);
    sd.amend_a();
    sd.git(&["push", "-qf"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".fresh"), "true");
    assert_has(&r.field(".lag[0]"), &["origin/feat"]);
    assert_eq!(r.field(".next[0].key"), "gh-wait");
}

// ── freshness: the PR's base on the remote, before ready (#169) ─────────────

// @test "base fresh: main moved on the remote since the last fetch: fetch, not ready"
#[test]
fn base_fresh_main_moved_on_the_remote_since_the_last_fetch_fetch_not_ready() {
    let sd = sd();
    sd.ready_pr(&[]);
    sd.main_moves("m.txt", "m");
    let r = sd.state(&[]);
    assert_eq!(r.field(".fresh"), "false");
    assert_has(&r.field(".stale[0]"), &["origin/main"]);
    assert_eq!(r.field(".next[0].key"), "stale");
    assert_eq!(r.field(".next[0].who"), "run");
    assert_eq!(r.field(".next[0].cmd"), "git fetch --prune origin");
    assert_eq!(r.field(r#"[.next[] | select(.key == "ready")] | length"#), "0");
    assert!(fs::metadata(sd.tmp.join("upload-pack.log")).is_ok_and(|m| m.len() > 0));
}

// @test "base fresh: main unchanged on the remote: ready, after one ls-remote"
#[test]
fn base_fresh_main_unchanged_on_the_remote_ready_after_one_ls_remote() {
    let sd = sd();
    sd.ready_pr(&[]);
    let r = sd.run(&[]);
    assert_eq!(r.status, 0);
    assert_eq!(r.lines()[1], "NEXT  you    ready to merge  → gh pr merge");
    assert_eq!(sd.upload_packs(), Some(1));
}

// @test "base fresh: ls-remote fails: ready as before, the rest intact"
#[test]
fn base_fresh_ls_remote_fails_ready_as_before_the_rest_intact() {
    let sd = sd();
    sd.ready_pr(&[]);
    sd.git(&["remote", "set-url", "origin", &sd.path("nowhere.git")]);
    let r = sd.run(&[]);
    assert_eq!(r.status, 0);
    assert!(r.lines()[0].starts_with("https://example/pr/7 open · "), "{}", r.output);
    assert_lacks(r.lines()[0], "stale");
    assert_eq!(r.lines()[1], "NEXT  you    ready to merge  → gh pr merge");
}

// @test "base fresh: no ls-remote where nothing spends or merges first"
#[test]
fn base_fresh_no_ls_remote_where_nothing_spends_or_merges_first() {
    let sd = sd();
    sd.ready_pr(&[".isDraft = true"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".next[0].key"), "draft");
    sd.pr_fixture("feat", &[r#".mergeStateStatus = "BLOCKED""#]);
    sd.state(&[]);
    sd.pr_fixture("feat", &[r#".reviewDecision = "REVIEW_REQUIRED""#]);
    sd.state(&[]);
    sd.pr_fixture("feat", &[&checks(r#"[{__typename:"CheckRun",name:"test",status:"IN_PROGRESS",conclusion:null}]"#)]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".next[0].key"), "ci-wait");
    sd.pr_fixture("feat", &[r#".state = "MERGED""#]);
    sd.state(&[]);
    sd.git(&["switch", "-qc", "nopr", "main"]);
    sd.commit("n.txt", "n");
    let r = sd.state(&[]);
    assert_eq!(r.field(".next[0].key"), "push");
    assert_eq!(sd.upload_packs(), None);
}

// GitHub's DIRTY with no conflict here: the base may have moved since the
// last fetch, so it's checked as before ready (#246).
// @test "base fresh: GitHub says DIRTY and main moved on the remote: fetch, not blocked"
#[test]
fn base_fresh_github_says_dirty_and_main_moved_on_the_remote_fetch_not_blocked() {
    let sd = sd();
    sd.ready_pr(&[r#".mergeStateStatus = "DIRTY""#, r#".mergeable = "CONFLICTING""#]);
    sd.main_moves("m.txt", "m");
    let r = sd.state(&[]);
    assert_has(&r.field(".stale[0]"), &["origin/main"]);
    assert_eq!(r.field(".next[0].key"), "stale");
    assert_eq!(r.field(".next[0].cmd"), "git fetch --prune origin");
}

// @test "base fresh: GitHub says DIRTY and main unchanged on the remote: blocked, after one ls-remote"
#[test]
fn base_fresh_github_says_dirty_and_main_unchanged_on_the_remote_blocked_after_one_ls_remote() {
    let sd = sd();
    sd.ready_pr(&[r#".mergeStateStatus = "DIRTY""#, r#".mergeable = "CONFLICTING""#]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".fresh"), "true");
    assert_eq!(r.field(".next[0].key"), "blocked");
    assert_eq!(sd.upload_packs(), Some(1));
}

// Before a step that spends tokens or triggers the bot, too (#182): a head
// about to be restacked would get the review or bot round again.
// @test "base fresh: main moved on the remote before review: fetch, not review"
#[test]
fn base_fresh_main_moved_on_the_remote_before_review_fetch_not_review() {
    let sd = sd();
    sd.ready_pr(&[]);
    sd.git(&["update-ref", "-d", "refs/sheepdog/feat/local"]);
    sd.main_moves("m.txt", "m");
    let r = sd.state(&[]);
    assert_has(&r.field(".stale[0]"), &["origin/main"]);
    assert_eq!(r.field(".next[0].key"), "stale");
    assert_eq!(r.field(".next[0].cmd"), "git fetch --prune origin");
}

// @test "base fresh: main unchanged on the remote: review, after one ls-remote"
#[test]
fn base_fresh_main_unchanged_on_the_remote_review_after_one_ls_remote() {
    let sd = sd();
    sd.ready_pr(&[]);
    sd.git(&["update-ref", "-d", "refs/sheepdog/feat/local"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".fresh"), "true");
    assert_eq!(r.field(".next[0].key"), "review");
    assert_eq!(sd.upload_packs(), Some(1));
}

// @test "base fresh: main moved on the remote before bot-trigger: fetch, not trigger"
#[test]
fn base_fresh_main_moved_on_the_remote_before_bot_trigger_fetch_not_trigger() {
    let sd = sd();
    sd.ready_pr(&[]);
    sd.sd_config(&["set", "--global", "settings.botCommand", "/review-bot"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".next[0].key"), "bot-trigger");
    sd.main_moves("m.txt", "m");
    let r = sd.state(&[]);
    assert_eq!(r.field(".next[0].key"), "stale");
    assert_has(&r.field(".stale[0]"), &["origin/main"]);
}

// @test "base fresh: no PR yet: the target moved on the remote before ship: fetch, not ship"
#[test]
fn base_fresh_no_pr_yet_the_target_moved_on_the_remote_before_ship_fetch_not_ship() {
    let sd = sd();
    sd.ready_pr(&[]);
    sd.git(&["switch", "-qc", "nopr", "main"]);
    sd.commit("n.txt", "n");
    sd.git(&["push", "-q", "-u", "origin", "nopr"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".pr"), "null");
    assert_eq!(r.field(".next[0].key"), "open-pr");
    sd.main_moves("m.txt", "m");
    let r = sd.state(&[]);
    assert_eq!(r.field(".next[0].key"), "stale");
    assert_has(&r.field(".stale[0]"), &["origin/main"]);
}

// ── O5 CI ────────────────────────────────────────────────────────────────────

impl Sd {
    fn feat_pr(&self, assigns: &[&str]) {
        self.git(&["switch", "-qc", "feat"]);
        self.commit("a.txt", "a");
        self.pr_fixture("feat", assigns);
    }
}

// @test "ci: no checks reported"
#[test]
fn ci_no_checks_reported() {
    let sd = sd();
    sd.feat_pr(&[]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".ci.verdict"), "none");
}

// @test "ci: all success, skipped or neutral is green"
#[test]
fn ci_all_success_skipped_or_neutral_is_green() {
    let sd = sd();
    sd.feat_pr(&[&checks(
        r#"[{__typename:"CheckRun",name:"a",status:"COMPLETED",conclusion:"SUCCESS"},
    {__typename:"CheckRun",name:"b",status:"COMPLETED",conclusion:"SKIPPED"},
    {__typename:"StatusContext",context:"c",state:"SUCCESS"}]"#,
    )]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".ci.verdict"), "green");
    assert_eq!(r.field(".ci.for_head"), "true");
}

// @test "ci: any failure is red, and names the failing checks"
#[test]
fn ci_any_failure_is_red_and_names_the_failing_checks() {
    let sd = sd();
    sd.feat_pr(&[&checks(
        r#"[{__typename:"CheckRun",name:"lint",status:"COMPLETED",conclusion:"FAILURE"},
    {__typename:"StatusContext",context:"ext",state:"ERROR"},
    {__typename:"CheckRun",name:"test",status:"IN_PROGRESS",conclusion:null}]"#,
    )]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".ci.verdict"), "red");
    assert_eq!(r.field(r#".ci.failing | join(",")"#), "lint,ext");
}

// @test "ci: unfinished checks are pending"
#[test]
fn ci_unfinished_checks_are_pending() {
    let sd = sd();
    sd.feat_pr(&[&checks(
        r#"[{__typename:"CheckRun",name:"a",status:"COMPLETED",conclusion:"SUCCESS"},
    {__typename:"CheckRun",name:"test",status:"QUEUED",conclusion:null},
    {__typename:"StatusContext",context:"ext",state:"PENDING"}]"#,
    )]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".ci.verdict"), "pending");
    assert_eq!(r.field(r#".ci.pending | join(",")"#), "test,ext");
}

// @test "ci: cancelled is inconclusive, not red"
#[test]
fn ci_cancelled_is_inconclusive_not_red() {
    let sd = sd();
    sd.feat_pr(&[&checks(
        r#"[{__typename:"CheckRun",name:"a",status:"COMPLETED",conclusion:"SUCCESS"},
    {__typename:"CheckRun",name:"b",status:"COMPLETED",conclusion:"CANCELLED"}]"#,
    )]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".ci.verdict"), "inconclusive");
}

// @test "ci: a check that didn't start or wants approval is blocked, not red"
#[test]
fn ci_a_check_that_didnt_start_or_wants_approval_is_blocked_not_red() {
    let sd = sd();
    sd.feat_pushed();
    sd.pr_fixture(
        "feat",
        &[&checks(
            r#"[{__typename:"CheckRun",name:"a",status:"COMPLETED",conclusion:"SUCCESS"},
    {__typename:"CheckRun",name:"wf",status:"COMPLETED",conclusion:"STARTUP_FAILURE"},
    {__typename:"CheckRun",name:"ok?",status:"COMPLETED",conclusion:"ACTION_REQUIRED"}]"#,
        )],
    );
    let r = sd.state(&[]);
    assert_eq!(r.field(".ci.verdict"), "blocked");
    assert_eq!(r.field(r#".ci.blocked | join(",")"#), "wf,ok?");
    assert_eq!(r.field(".ci.failing | length"), "0");
    assert_eq!(r.field(".next[0].key"), "ci-blocked");
}

// @test "ci: a failure outranks a blocked check"
#[test]
fn ci_a_failure_outranks_a_blocked_check() {
    let sd = sd();
    sd.feat_pr(&[&checks(
        r#"[{__typename:"CheckRun",name:"lint",status:"COMPLETED",conclusion:"FAILURE"},
    {__typename:"CheckRun",name:"wf",status:"COMPLETED",conclusion:"STARTUP_FAILURE"}]"#,
    )]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".ci.verdict"), "red");
}

// @test "ci: a newer run of the same check supersedes an older failure on the same SHA (#195)"
#[test]
fn ci_a_newer_run_of_the_same_check_supersedes_an_older_failure_on_the_same_sha_195() {
    let sd = sd();
    sd.feat_pushed();
    sd.git(&["update-ref", "refs/sheepdog/feat/local", "HEAD"]);
    sd.pr_fixture(
        "feat",
        &[&checks(&format!(
            "[{}, {}, {}]",
            check_run(20, "title", "SUCCESS"),
            check_run(10, "title", "FAILURE"),
            check_run(11, "test", "SUCCESS")
        ))],
    );
    let r = sd.state(&[]);
    assert_eq!(r.field(".ci.verdict"), "green");
    assert_eq!(r.field(".ci.failing | length"), "0");
    assert_lacks(&r.keys(), "ci-fix");
}

// @test "ci: a newer failure of the same check is red over an older pass (#195)"
#[test]
fn ci_a_newer_failure_of_the_same_check_is_red_over_an_older_pass_195() {
    let sd = sd();
    sd.feat_pr(&[&checks(&format!("[{}, {}]", check_run(10, "title", "SUCCESS"), check_run(20, "title", "FAILURE")))]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".ci.verdict"), "red");
    assert_eq!(r.field(r#".ci.failing | join(",")"#), "title");
}

// @test "ci: a replacement run still in progress is pending, not the old failure's red (#195)"
#[test]
fn ci_a_replacement_run_still_in_progress_is_pending_not_the_old_failures_red_195() {
    let sd = sd();
    sd.feat_pr(&[&checks(&format!("[{}, {}]", check_run(10, "title", "FAILURE"), check_run(20, "title", "null")))]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".ci.verdict"), "pending");
    assert_eq!(r.field(r#".ci.pending | join(",")"#), "title");
}

// @test "ci: same-named checks from another workflow or event are distinct checks (#195)"
#[test]
fn ci_same_named_checks_from_another_workflow_or_event_are_distinct_checks_195() {
    let sd = sd();
    sd.feat_pr(&[&checks(&format!(
        "[{}, {}]",
        check_run_of(10, "test", "FAILURE", "ci", "pull_request"),
        check_run_of(20, "test", "SUCCESS", "lint", "pull_request")
    ))]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".ci.verdict"), "red");
    sd.pr_fixture(
        "feat",
        &[&checks(&format!(
            "[{}, {}]",
            check_run_of(10, "test", "FAILURE", "ci", "push"),
            check_run_of(20, "test", "SUCCESS", "ci", "pull_request")
        ))],
    );
    let r = sd.state(&[]);
    assert_eq!(r.field(".ci.verdict"), "red");
}

// Required checks (#123): read for a draft only, since undraft is the one
// decision GitHub's merge state doesn't already gate on them.

// @test "ci: a draft's required check with no context on the head is pending, not none"
#[test]
fn ci_a_drafts_required_check_with_no_context_on_the_head_is_pending_not_none() {
    let sd = sd();
    sd.reviewed_draft(&[]);
    sd.required_rules(&["test"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".ci.verdict"), "pending");
    assert_eq!(r.field_c(".ci.required"), r#"["test"]"#);
    assert_eq!(r.field_c(".ci.missing"), r#"["test"]"#);
    assert_eq!(r.keys(), "ci-wait");
    assert_eq!(
        r.field(".next[0].say"),
        "waiting on required check test to report; if it never runs on a draft, undraft by hand (gh pr ready)"
    );
}

// @test "ci: a draft's required check missing beside green ones is pending"
#[test]
fn ci_a_drafts_required_check_missing_beside_green_ones_is_pending() {
    let sd = sd();
    sd.feat_pr(&[
        ".isDraft = true",
        &checks(r#"[{__typename:"CheckRun",name:"lint",status:"COMPLETED",conclusion:"SUCCESS"}]"#),
    ]);
    fs::write(
        sd.gh("branch.json"),
        r#"{"name": "main", "protected": true, "protection": {"enabled": true, "required_status_checks":
    {"enforcement_level": "non_admins", "contexts": ["lint", "ext"], "checks": [{"context": "lint", "app_id": 1}, {"context": "ext", "app_id": null}]}}}
"#,
    )
    .unwrap();
    let r = sd.state(&[]);
    assert_eq!(r.field(".ci.verdict"), "pending");
    assert_eq!(r.field_c(".ci.required"), r#"["ext","lint"]"#);
    assert_eq!(r.field_c(".ci.missing"), r#"["ext"]"#);
}

// @test "ci: a draft's required checks all reported is green"
#[test]
fn ci_a_drafts_required_checks_all_reported_is_green() {
    let sd = sd();
    sd.feat_pr(&[
        ".isDraft = true",
        &checks(
            r#"[{__typename:"CheckRun",name:"test",status:"COMPLETED",conclusion:"SUCCESS"},
    {__typename:"StatusContext",context:"ext",state:"SUCCESS"}]"#,
        ),
    ]);
    sd.required_rules(&["test", "ext"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".ci.verdict"), "green");
    assert_eq!(r.field_c(".ci.missing"), "[]");
}

// @test "ci: a draft with no required checks and nothing reported is still none, and undrafts"
#[test]
fn ci_a_draft_with_no_required_checks_and_nothing_reported_is_still_none_and_undrafts() {
    let sd = sd();
    sd.reviewed_draft(&[]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".ci.verdict"), "none");
    assert_eq!(r.field_c(".ci.required"), "[]");
    assert_eq!(r.field_c("[.errors[]]"), "[]");
    let mut rest: Vec<String> = fs::read_to_string(sd.gh("rest")).unwrap().lines().map(String::from).collect();
    rest.sort();
    assert_eq!(rest.join(" "), "repos/{owner}/{repo}/branches/main repos/{owner}/{repo}/rules/branches/main?per_page=100");
    assert_eq!(r.keys(), "draft");
    assert_eq!(r.field(".next[0].say"), "reviewed, no CI checks: mark the PR ready for review");
}

// @test "ci: a 404 or 403 from either source is no rules from it, not an error"
#[test]
fn ci_a_404_or_403_from_either_source_is_no_rules_from_it_not_an_error() {
    let sd = sd();
    sd.reviewed_draft(&[]);
    sd.required_rules(&["test"]);
    let r = sd.state_env(&[("GH_FAKE_FAIL_REST", "gh: Not Found (HTTP 404)"), ("GH_FAKE_FAIL_REST_ON", "*/rules/*")], &[]);
    assert_eq!(r.field_c("[.errors[]]"), "[]");
    assert_eq!(r.field_c(".ci.required"), "[]");
    assert_eq!(r.keys(), "draft");
    fs::write(
        sd.gh("branch.json"),
        r#"{"name": "main", "protected": true, "protection": {"required_status_checks": {"contexts": ["test"], "checks": []}}}
"#,
    )
    .unwrap();
    let r = sd.state_env(
        &[("GH_FAKE_FAIL_REST", "gh: Resource not accessible by integration (HTTP 403)"), ("GH_FAKE_FAIL_REST_ON", "*/rules/*")],
        &[],
    );
    assert_eq!(r.field_c(".ci.required"), r#"["test"]"#);
    assert_eq!(r.keys(), "ci-wait");
    let r = sd.state_env(
        &[("GH_FAKE_FAIL_REST", "gh: Must have admin rights to Repository. (HTTP 403)"), ("GH_FAKE_FAIL_REST_ON", "*/branches/main")],
        &[],
    );
    assert_eq!(r.field_c("[.errors[]]"), "[]");
    assert_eq!(r.field_c(".ci.required"), r#"["test"]"#);
}

// @test "ci: a 403 for a rate limit is still an error"
#[test]
fn ci_a_403_for_a_rate_limit_is_still_an_error() {
    let sd = sd();
    sd.reviewed_draft(&[]);
    let r = sd.state_env(&[("GH_FAKE_FAIL_REST", "gh: API rate limit exceeded for user ID 1. (HTTP 403)")], &[]);
    assert_eq!(r.field(".ci.required"), "null");
    assert_has(&r.errors(), &["required checks on main", "rate limit"]);
}

// @test "ci: required checks aren't read for a PR that isn't a draft"
#[test]
fn ci_required_checks_arent_read_for_a_pr_that_isnt_a_draft() {
    let sd = sd();
    sd.feat_pr(&[]);
    sd.required_rules(&["test"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".ci.verdict"), "none");
    assert_eq!(r.field(".ci.required"), "null");
    assert!(!sd.gh("rest").exists());
}

// @test "ci: reading required checks failing is an error, and no draft step until it's known"
#[test]
fn ci_reading_required_checks_failing_is_an_error_and_no_draft_step_until_its_known() {
    let sd = sd();
    sd.reviewed_draft(&[]);
    let r = sd.state_env(&[("GH_FAKE_FAIL_REST", "gh: Server Error (HTTP 502)")], &[]);
    assert_eq!(r.field(".ci.required"), "null");
    assert_has(&r.errors(), &["required checks on main", "HTTP 502"]);
    assert_eq!(r.keys(), "errors");
}

// @test "ci: checks on a SHA other than local HEAD are flagged"
#[test]
fn ci_checks_on_a_sha_other_than_local_head_are_flagged() {
    let sd = sd();
    sd.feat_pr(&[&checks(r#"[{__typename:"CheckRun",name:"a",status:"COMPLETED",conclusion:"SUCCESS"}]"#)]);
    sd.commit("b.txt", "b");
    let r = sd.state(&[]);
    assert_eq!(r.field(".ci.for_head"), "false");
}

// ── O6 review pins ───────────────────────────────────────────────────────────

impl Sd {
    fn feat(&self) {
        self.git(&["switch", "-qc", "feat"]);
        self.commit("a.txt", "a");
    }

    /// A findings record (or any blob) as refs/sheepdog/feat/findings.
    fn findings(&self, rec: &str) {
        self.put_blob("refs/sheepdog/feat/findings", rec);
    }

    fn edit_findings(&self, filter: &str) {
        let rec = jq(&["-c", filter], &self.git(&["cat-file", "blob", "refs/sheepdog/feat/findings"]));
        self.findings(&rec);
    }

    fn rebased_on_moved_main(&self) {
        self.main_moves("b.txt", "b");
        self.git(&["fetch", "-q"]);
        self.git(&["rebase", "-q", "origin/main"]);
    }
}

// @test "review: no pins"
#[test]
fn review_no_pins() {
    let sd = sd();
    sd.feat();
    let r = sd.state(&[]);
    assert_eq!(r.field(".review.local.status"), "none");
    assert_eq!(r.field(".review.bot.status"), "none");
}

// @test "review: no findings record is null"
#[test]
fn review_no_findings_record_is_null() {
    let sd = sd();
    sd.feat();
    let r = sd.state(&[]);
    assert_eq!(r.field(".review.findings"), "null");
}

// @test "review: a findings record at HEAD, with its round, count and the cap"
#[test]
fn review_a_findings_record_at_head_with_its_round_count_and_the_cap() {
    let sd = sd();
    sd.feat();
    let h = sd.git(&["rev-parse", "HEAD"]);
    sd.findings(&format!(
        r#"{{"head":"{h}","base":"{h}","round":2,"findings":[{{"file":"a.txt","line":1,"severity":"error","body":"x"}}]}}"#
    ));
    let r = sd.state(&[]);
    assert_eq!(r.field(".review.findings.round"), "2");
    assert_eq!(r.field(".review.findings.n"), "1");
    assert_eq!(r.field(".review.findings.at_head"), "true");
    assert_eq!(r.field(".review.findings.declined"), "false");
    assert_eq!(r.field(".review.findings.cap"), "3");
    assert_eq!(r.field(".review.findings.items[0].body"), "x");
    let r = sd.run(&[]);
    assert_has(&r.output, &["review: 1 finding (round 2/3)"]);
    let r = sd.run(&["--long"]);
    assert_has(&r.output, &["error  a.txt:1  x"]);
    sd.edit_findings(r#".findings[0] += {action: "declined", reply: "x is fine"}"#);
    let r = sd.run(&["--long"]);
    assert_has(&r.output, &["error  a.txt:1  x", "declined: x is fine"]);
    let r = sd.run(&[]);
    assert_has(&r.output, &["review: 1 finding (round 2/3, declined)"]);
    sd.commit("b.txt", "b");
    let r = sd.state(&[]);
    assert_eq!(r.field(".review.findings.at_head"), "false");
}

// @test "question: stands at HEAD with the worktree the step left; NEXT's question step (#167)"
#[test]
fn question_stands_at_head_with_the_worktree_the_step_left_nexts_question_step_167() {
    let sd = sd();
    sd.feat();
    sd.write("a.txt", "edit\n");
    let rec = jq(
        &[
            "-nc",
            "--arg",
            "h",
            &sd.git(&["rev-parse", "HEAD"]),
            "--arg",
            "m",
            &sd.worktree_mark(),
            r#"{head: $h, step: "ci-fix", mark: $m, text: "which test?\nrevert it, or keep it"}"#,
        ],
        "",
    );
    sd.put_blob("refs/sheepdog/feat/question", &rec);
    let r = sd.state(&[]);
    assert_eq!(r.field(".question.step"), "ci-fix");
    assert_eq!(r.field(".question.at_head"), "true");
    assert_eq!(r.field(".next[0].key"), "question");
    let r = sd.run(&[]);
    assert_has(&r.output, &["? ci-fix asks"]);
    let r = sd.run(&["--long"]);
    assert_has(&r.output, &["?  ci-fix asks:", "which test?", "revert it, or keep it"]);
    sd.write("b.txt", "more\n"); // the worktree moved on
    let r = sd.state(&[]);
    assert_eq!(r.field(".question.at_head"), "false");
    fs::remove_file(sd.cwd.join("b.txt")).unwrap();
    let r = sd.state(&[]);
    assert_eq!(r.field(".question.at_head"), "true");
    sd.git(&["commit", "-qam", "edit"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".question.at_head"), "false");
}

#[test]
fn failed_agent_steps_show_on_the_one_liner_and_in_long_three_in_a_row_hand_the_agent_steps_to_chrismo_242() {
    let sd = sd();
    sd.feat();
    let rec = |n: u32| format!(r#"{{"step": "implement", "harness": "pi", "count": {n}, "since": "2026-10-22T20:00:00Z", "at": "2026-10-22T20:05:00Z", "error": "no quota"}}"#);
    sd.put_blob("refs/sheepdog/feat/failed", &rec(1));
    let r = sd.state(&[]);
    assert_eq!(r.field(".failed.step"), "implement");
    assert_eq!(r.field(".failed.count"), "1");
    assert_eq!(r.field(r#"[.next[].key] | join(" ")"#), "push open-pr");
    let r = sd.run(&[]);
    assert_has(&r.output, &["✗ 1 failed"]);
    let r = sd.run(&["--long"]);
    assert_has(&r.output, &["✗  1 agent step failed in a row, the last pi implement at 2026-10-22T20:05:00Z: no quota"]);
    sd.put_blob("refs/sheepdog/feat/failed", &rec(3));
    let r = sd.state(&[]);
    assert_eq!(r.field(r#"[.next[].key] | join(" ")"#), "push failed");
    sd.put_blob("refs/sheepdog/feat/failed", r#"{"step": "implement"}"#);
    let r = sd.state(&[]);
    assert_eq!(r.field(".failed"), "null");
    assert_has(&r.field(".errors[0]"), &["refs/sheepdog/feat/failed isn't a failure record"]);
}

#[test]
fn verify_red_stands_at_the_commit_it_failed_on_on_the_one_liner_and_next_325() {
    let sd = sd();
    sd.feat();
    let head = sd.git(&["rev-parse", "HEAD"]);
    let rec = format!(r#"{{"head": "{head}", "step": "implement", "cmd": "make test", "at": "2026-10-29T12:00:00Z"}}"#);
    sd.put_blob("refs/sheepdog/feat/verify-red", &rec);
    let r = sd.state(&[]);
    assert_eq!(r.field(".verify.sha"), head);
    assert_eq!(r.field(".verify.step"), "implement");
    assert_eq!(r.field(".verify.cmd"), "make test");
    assert_eq!(r.field(".verify.at_head"), "true");
    assert_eq!(r.field(r#"[.next[].key] | join(" ")"#), "verify-red");
    let r = sd.run(&[]);
    assert_has(&r.output, &["✗ verify red"]);
    sd.commit("b.txt", "b"); // a new HEAD isn't the one verify failed on
    let r = sd.state(&[]);
    assert_eq!(r.field(".verify.at_head"), "false");
    assert_eq!(r.field(r#"[.next[].key] | join(" ")"#), "push open-pr");
    let r = sd.run(&[]);
    assert_lacks(&r.output, "verify red");
    sd.put_blob("refs/sheepdog/feat/verify-red", r#"{"head": "x"}"#);
    let r = sd.state(&[]);
    assert_eq!(r.field(".verify"), "null");
    assert_has(&r.field(".errors[0]"), &["refs/sheepdog/feat/verify-red isn't a verify record"]);
}

#[test]
fn question_a_give_up_says_so_on_the_one_liner_and_in_long_254() {
    let sd = sd();
    sd.feat();
    let rec = jq(
        &["-nc", "--arg", "h", &sd.git(&["rev-parse", "HEAD"]), "--arg", "m", &sd.worktree_mark(),
          r#"{head: $h, step: "implement", kind: "gave-up", mark: $m, text: "makes no sense"}"#],
        "",
    );
    sd.put_blob("refs/sheepdog/feat/question", &rec);
    let r = sd.state(&[]);
    assert_eq!(r.field(".question.kind"), "gave-up");
    let r = sd.run(&[]);
    assert_has(&r.output, &["? implement gave up"]);
    let r = sd.run(&["--long"]);
    assert_has(&r.output, &["?  implement gave up:", "makes no sense"]);
}

// @test "review: findings are declined only when the agent declined every one that blocks"
#[test]
fn review_findings_are_declined_only_when_the_agent_declined_every_one_that_blocks() {
    let sd = sd();
    sd.feat();
    let h = sd.git(&["rev-parse", "HEAD"]);
    sd.findings(&format!(
        r#"{{"head":"{h}","base":"{h}","round":1,"findings":[{{"file":"a.txt","line":1,"severity":"error","body":"x","action":"declined","reply":"x is fine"}},{{"file":"a.txt","line":2,"severity":"warning","body":"y"}},{{"file":"a.txt","line":3,"severity":"nit","body":"z"}}]}}"#
    ));
    let r = sd.state(&[]);
    assert_eq!(r.field(".review.findings.declined"), "false");
    sd.edit_findings(r#".findings[1] += {action: "declined", reply: "y too"}"#);
    let r = sd.state(&[]);
    assert_eq!(r.field(".review.findings.declined"), "true");
}

// @test "review: findings count only what blocks; a record missing fields is an error"
#[test]
fn review_findings_count_only_what_blocks_a_record_missing_fields_is_an_error() {
    let sd = sd();
    sd.feat();
    let h = sd.git(&["rev-parse", "HEAD"]);
    sd.findings(&format!(
        r#"{{"head":"{h}","base":"{h}","round":1,"findings":[{{"file":"a.txt","line":1,"severity":"warning","body":"x"}},{{"file":"a.txt","line":2,"severity":"nit","body":"y"}}]}}"#
    ));
    let r = sd.state(&[]);
    assert_eq!(r.field(".review.findings.n"), "1");
    sd.findings(r#"{"head": "x"}"#);
    let r = sd.state(&[]);
    assert_eq!(r.field(".review.findings"), "null");
    assert_has(&r.errors(), &["isn't a findings record"]);
}

// @test "review: pin at HEAD is covered"
#[test]
fn review_pin_at_head_is_covered() {
    let sd = sd();
    sd.feat();
    sd.git(&["update-ref", "refs/sheepdog/feat/local", "HEAD"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".review.local.status"), "covered");
}

// @test "review: pin behind HEAD leaves commits uncovered"
#[test]
fn review_pin_behind_head_leaves_commits_uncovered() {
    let sd = sd();
    sd.feat();
    sd.git(&["update-ref", "refs/sheepdog/feat/bot", "HEAD"]);
    sd.commit("b.txt", "b");
    sd.commit("c.txt", "c");
    let r = sd.state(&[]);
    assert_eq!(r.field(".review.bot.status"), "uncovered");
    assert_eq!(r.field(".review.bot.uncovered"), "2");
}

// @test "review: a rebase that keeps the change still counts as covered"
#[test]
fn review_a_rebase_that_keeps_the_change_still_counts_as_covered() {
    let sd = sd();
    sd.feat();
    sd.git(&["update-ref", "refs/sheepdog/feat/local", "HEAD"]);
    sd.rebased_on_moved_main();
    let r = sd.state(&[]);
    assert_eq!(r.field(".review.local.status"), "rewritten-same");
}

// @test "review: findings stay at HEAD across a rebase that keeps the change"
#[test]
fn review_findings_stay_at_head_across_a_rebase_that_keeps_the_change() {
    let sd = sd();
    sd.feat();
    let (h, b) = (sd.git(&["rev-parse", "HEAD"]), sd.git(&["rev-parse", "main"]));
    sd.findings(&format!(
        r#"{{"head":"{h}","base":"{b}","round":3,"findings":[{{"file":"a.txt","line":1,"severity":"error","body":"x"}}]}}"#
    ));
    sd.rebased_on_moved_main();
    let r = sd.state(&[]);
    assert_eq!(r.field(".review.findings.at_head"), "true");
    sd.commit("c.txt", "c");
    let r = sd.state(&[]);
    assert_eq!(r.field(".review.findings.at_head"), "false");
}

// @test "review: --long lists a finding missing fields as itself"
#[test]
fn review_long_lists_a_finding_missing_fields_as_itself() {
    let sd = sd();
    sd.feat();
    let h = sd.git(&["rev-parse", "HEAD"]);
    sd.findings(&format!(r#"{{"head":"{h}","base":"{h}","round":1,"findings":[{{"file":"a.txt","body":"x"}}]}}"#));
    let r = sd.run(&["--long"]);
    assert_has(&r.output, &["r  ?  a.txt:?  x"]);
}

// @test "review: a findings record whose base is gone is just not at HEAD"
#[test]
fn review_a_findings_record_whose_base_is_gone_is_just_not_at_head() {
    let sd = sd();
    sd.feat();
    let h = sd.git(&["rev-parse", "HEAD"]);
    sd.findings(&format!(
        r#"{{"head":"{h}","base":"0123456789abcdef0123456789abcdef01234567","round":1,"findings":[{{"file":"a.txt","line":1,"severity":"error","body":"x"}}]}}"#
    ));
    sd.commit("b.txt", "b");
    let r = sd.state(&[]);
    assert_eq!(r.field(".review.findings.at_head"), "false");
}

impl Sd {
    /// feat stacked on p, reviewed, then p squash-merged (different content,
    /// else the squash commit can get p's own SHA: same tree, same second) and
    /// feat restacked onto main past it.
    fn restacked_past_squashed_p(&self, base_pin: bool) {
        self.git(&["switch", "-qc", "p"]);
        self.commit("p.txt", "p");
        self.git(&["switch", "-qc", "feat"]);
        self.commit("a.txt", "a");
        self.git(&["update-ref", "refs/sheepdog/feat/local", "HEAD"]);
        if base_pin {
            self.git(&["update-ref", "refs/sheepdog/feat/local-base", "p"]);
        }
        self.main_moves("p.txt", "p, squashed");
        self.git(&["fetch", "-q"]);
        self.git(&["rebase", "-q", "--onto", "origin/main", "p", "feat"]);
    }
}

// @test "review: a pin's recorded base keeps it covered across a restack past a squash-merged parent"
#[test]
fn review_a_pins_recorded_base_keeps_it_covered_across_a_restack_past_a_squash_merged_parent() {
    let sd = sd();
    sd.restacked_past_squashed_p(true);
    let r = sd.state(&[]);
    assert_eq!(r.field(".review.local.status"), "rewritten-same");
}

// @test "review: without a recorded base, the same restack looks like a changed change (why -base exists)"
#[test]
fn review_without_a_recorded_base_the_same_restack_looks_like_a_changed_change_why_base_exists() {
    let sd = sd();
    sd.restacked_past_squashed_p(false);
    let r = sd.state(&[]);
    assert_eq!(r.field(".review.local.status"), "rewritten-changed");
}

// @test "review: a rewrite that changes the change is not covered"
#[test]
fn review_a_rewrite_that_changes_the_change_is_not_covered() {
    let sd = sd();
    sd.feat();
    sd.git(&["update-ref", "refs/sheepdog/feat/local", "HEAD"]);
    sd.write("a.txt", "different\n");
    sd.git(&["commit", "-qa", "--amend", "--no-edit"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".review.local.status"), "rewritten-changed");
}

// @test "review: a rewritten pin that can't be compared is unknown, not covered"
#[test]
fn review_a_rewritten_pin_that_cant_be_compared_is_unknown_not_covered() {
    let sd = sd();
    sd.git(&["switch", "-q", "--orphan", "feat"]);
    sd.commit("a.txt", "a");
    sd.git(&["update-ref", "refs/sheepdog/feat/local", "HEAD"]);
    sd.write("a.txt", "different\n");
    sd.git(&["commit", "-qa", "--amend", "--no-edit"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".review.local.status"), "unknown");
}

// @test "review: stacked on an open parent with no base: a rewritten pin is unknown, not compared from the merge-base (#120)"
#[test]
fn review_stacked_on_an_open_parent_with_no_base_a_rewritten_pin_is_unknown_not_compared_from_the_merge_base_120() {
    let sd = sd();
    sd.git(&["switch", "-qc", "parent"]);
    sd.commit("p.txt", "p1");
    sd.git(&["push", "-q", "origin", "parent"]);
    sd.git(&["switch", "-qc", "feat", "main"]);
    sd.commit("c.txt", "c");
    sd.git(&["config", "branch.feat.sd-parent", "parent"]);
    sd.git(&["update-ref", "refs/sheepdog/feat/local", "HEAD"]);
    sd.git(&["commit", "-q", "--amend", "-m", "reworded"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".sync.old_base"), "null");
    assert_eq!(r.field(".review.local.status"), "unknown");
}

// @test "review: stacked on an open parent, base from the fork-point: a rewritten pin with no recorded base is unknown (#120)"
#[test]
fn review_stacked_on_an_open_parent_base_from_the_fork_point_a_rewritten_pin_with_no_recorded_base_is_unknown_120() {
    let sd = sd();
    sd.git(&["switch", "-qc", "parent"]);
    sd.commit("p.txt", "p1");
    sd.git(&["push", "-q", "origin", "parent"]);
    let p1 = sd.git(&["rev-parse", "parent"]);
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("c.txt", "c");
    sd.git(&["config", "branch.feat.sd-parent", "parent"]);
    sd.git(&["update-ref", "refs/sheepdog/feat/local", "HEAD"]);
    sd.parent_amended_elsewhere();
    sd.git(&["fetch", "-q"]);
    sd.git(&["commit", "-q", "--amend", "-m", "reworded"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".sync.old_base"), p1);
    assert_eq!(r.field(".review.local.status"), "unknown");
}

// @test "review: stacked on a merged parent with no base: a rewritten pin is unknown, not compared from the merge-base (#120)"
#[test]
fn review_stacked_on_a_merged_parent_with_no_base_a_rewritten_pin_is_unknown_not_compared_from_the_merge_base_120() {
    let sd = sd();
    sd.lost_pin_stack("nopin");
    sd.git(&["update-ref", "refs/sheepdog/feat/local", "HEAD"]);
    sd.git(&["commit", "-q", "--amend", "-m", "reworded"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".sync.target"), "main");
    assert_eq!(r.field(".sync.old_base"), "null");
    assert_eq!(r.field(".review.local.status"), "unknown");
}

// ── O7 threads owed a reply ──────────────────────────────────────────────────

// @test "threads: unresolved and last word not ours are owed, split bot vs human"
#[test]
fn threads_unresolved_and_last_word_not_ours_are_owed_split_bot_vs_human() {
    let sd = sd();
    sd.feat();
    sd.sd_config(&["set", "--global", "--json", "settings.botLogin", r#"["marge"]"#]);
    sd.pr_fixture(
        "feat",
        &[&format!(
            ".reviewThreads.nodes = [\n    {},\n    {},\n    {},\n    {},\n    {},\n    {}]",
            thread("Bot", "copilot", "Bot", "copilot"),
            thread("User", "marge", "User", "marge"),
            thread("User", "alice", "User", "alice"),
            thread("User", "bob", "User", "me"),
            thread_x("User", "carol", "User", "carol", true, false),
            thread_x("Bot", "copilot", "Bot", "copilot", false, true)
        )],
    );
    let r = sd.state(&[]);
    assert_eq!(r.field(".threads.owed_bot"), "3");
    assert_eq!(r.field(".threads.owed_human"), "1");
    assert_eq!(r.field(".threads.outdated"), "1");
    assert_eq!(r.field(".threads.items | length"), "4");
}

// ── O6, the bot's reply since the trigger (#63) ─────────────────────────────

// @test "review.bot: triggered and no bot word since: replied false, waited counts"
#[test]
fn review_bot_triggered_and_no_bot_word_since_replied_false_waited_counts() {
    let sd = sd();
    sd.feat();
    sd.trigger_pin("2026-10-15T12:00:00Z");
    sd.pr_fixture("feat", &[&format!(".reviews.nodes = [{}]", note_at("R0", "Bot", "copilot", "earlier", "2026-10-15T11:59:59Z"))]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".review.bot.status"), "covered");
    assert_eq!(r.field(".review.bot.replied"), "false");
    assert_eq!(r.field(".review.bot.triggered"), "2026-10-15T12:00:00Z");
    let w: i64 = r.field(".review.bot.waited").parse().unwrap(); // seconds since a past trigger: not 0, not nanoseconds
    assert!((3600..315360000).contains(&w), "{w}");
}

// @test "review.bot: a bot review or comment after the trigger means replied; a human's doesn't"
#[test]
fn review_bot_a_bot_review_or_comment_after_the_trigger_means_replied_a_humans_doesnt() {
    let sd = sd();
    sd.feat();
    sd.trigger_pin("2026-10-15T12:00:00Z");
    sd.pr_fixture("feat", &[&format!(".reviews.nodes = [{}]", note_at("R1", "User", "alice", "lgtm", "2026-10-15T12:05:00Z"))]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".review.bot.replied"), "false");
    sd.pr_fixture("feat", &[&format!(".reviews.nodes = [{}]", note_at("R1", "Bot", "copilot", "findings", "2026-10-15T12:00:01Z"))]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".review.bot.replied"), "true");
    sd.pr_fixture("feat", &[&format!(".comments.nodes = [{}]", note_at("C1", "Bot", "copilot", "done", "2026-10-15T12:05:00Z"))]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".review.bot.replied"), "true");
}

// @test "review.bot: a review started before the trigger and submitted after it is the reply"
#[test]
fn review_bot_a_review_started_before_the_trigger_and_submitted_after_it_is_the_reply() {
    let sd = sd();
    sd.feat();
    sd.trigger_pin("2026-10-15T12:00:00Z");
    sd.pr_fixture(
        "feat",
        &[r#".reviews.nodes = [{id: "R1", body: "findings", createdAt: "2026-10-15T11:50:00Z", submittedAt: "2026-10-15T12:05:00Z", author: {__typename: "Bot", login: "copilot"}}]"#],
    );
    let r = sd.state(&[]);
    assert_eq!(r.field(".review.bot.replied"), "true");
}

// @test "review.bot: an ignored bot's word doesn't count as the reply"
#[test]
fn review_bot_an_ignored_bots_word_doesnt_count_as_the_reply() {
    let sd = sd();
    sd.feat();
    sd.trigger_pin("2026-10-15T12:00:00Z");
    sd.sd_config(&["set", "--global", "--json", "settings.botIgnore", r#"["dependabot"]"#]);
    sd.pr_fixture("feat", &[&format!(".comments.nodes = [{}]", note_at("C1", "Bot", "dependabot", "bump", "2026-10-15T12:05:00Z"))]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".review.bot.replied"), "false");
}

// @test "review.bot: a placeholder matching botIgnoreBody isn't the reply; an inline-only review (blank body) is"
#[test]
fn review_bot_a_placeholder_matching_botignorebody_isnt_the_reply_an_inline_only_review_blank_body_is() {
    let sd = sd();
    sd.feat();
    sd.trigger_pin("2026-10-15T12:00:00Z");
    sd.write_global(r#"{"settings": {"botIgnoreBody": ["is on the job\\."]}}"#);
    sd.pr_fixture("feat", &[&format!(".comments.nodes = [{}]", note_at("C1", "Bot", "marge", "Marge is on the job.", "2026-10-15T12:00:05Z"))]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".review.bot.replied"), "false");
    sd.pr_fixture("feat", &[&format!(".reviews.nodes = [{}]", note_at("R1", "Bot", "marge", "", "2026-10-15T12:05:00Z"))]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".review.bot.replied"), "true");
}

// @test "review.bot: no trigger time (a pin from before, or by hand), or no pin: replied null"
#[test]
fn review_bot_no_trigger_time_a_pin_from_before_or_by_hand_or_no_pin_replied_null() {
    let sd = sd();
    sd.feat_pr(&[]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".review.bot.replied"), "null");
    sd.git(&["update-ref", "refs/sheepdog/feat/bot", "HEAD"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".review.bot.status"), "covered");
    assert_eq!(r.field(".review.bot.replied"), "null");
    assert_eq!(r.field(".review.bot.waited"), "null");
}

// @test "review.bot: gh failing: replied and waited are unknown, not a wait"
#[test]
fn review_bot_gh_failing_replied_and_waited_are_unknown_not_a_wait() {
    let sd = sd();
    sd.feat();
    sd.trigger_pin("2026-10-15T12:00:00Z");
    let r = sd.state_env(&[("GH_FAKE_FAIL", "gh: down")], &[]);
    assert_eq!(r.field(".review.bot.status"), "covered");
    assert_eq!(r.field(".review.bot.replied"), "null");
    assert_eq!(r.field(".review.bot.waited"), "null");
}

// @test "review.accepted: after is the bot trigger sd act review --accept followed; null without one (#181)"
#[test]
fn review_accepted_after_is_the_bot_trigger_sd_act_review_accept_followed_null_without_one_181() {
    let sd = sd();
    sd.feat_pr(&[]);
    sd.git(&["update-ref", "refs/sheepdog/feat/accepted", "HEAD"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".review.accepted.status"), "covered");
    assert_eq!(r.field(".review.accepted.after"), "null");
    sd.git(&["config", "branch.feat.sd-accepted-after", "2026-10-15T12:30:00Z"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".review.accepted.after"), "2026-10-15T12:30:00Z");
}

// @test "bot: settings.botTimeout is reported, 3600 s by default"
#[test]
fn bot_settings_bottimeout_is_reported_3600_s_by_default() {
    let sd = sd();
    sd.feat_pr(&[]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".bot.timeout"), "3600");
    sd.sd_config(&["set", "--global", "--json", "settings.botTimeout", "600"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".bot.timeout"), "600");
}

// @test "summary: bot ⏳ while waiting on its reply, bot ✓ once it came"
#[test]
fn summary_bot_while_waiting_on_its_reply_bot_once_it_came() {
    let sd = sd();
    sd.feat();
    sd.trigger_pin("2026-10-15T12:00:00Z");
    sd.pr_fixture("feat", &[]);
    let r = sd.run(&[]);
    assert_has(&r.output, &["bot ⏳"]);
    sd.pr_fixture("feat", &[&format!(".reviews.nodes = [{}]", note_at("R1", "Bot", "copilot", "findings", "2026-10-15T12:00:01Z"))]);
    let r = sd.run(&[]);
    assert_has(&r.output, &["bot ✓"]);
}

// @test "threads: inline thread items are typed thread"
#[test]
fn threads_inline_thread_items_are_typed_thread() {
    let sd = sd();
    sd.feat_pr(&[&format!(".reviewThreads.nodes = [{}]", thread("Bot", "copilot", "Bot", "copilot"))]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".threads.items[0].type"), "thread");
    assert_eq!(r.field(".threads.items[0].path"), "file.txt");
}

// @test "threads: bot review bodies are owed; empty bodies and humans' are not"
#[test]
fn threads_bot_review_bodies_are_owed_empty_bodies_and_humans_are_not() {
    let sd = sd();
    sd.feat_pr(&[&format!(
        ".reviews.nodes = [\n    {},\n    {},\n    {},\n    {}]",
        note("R1", "Bot", "copilot"),
        note_body("R2", "Bot", "copilot", "  "),
        note_body("R3", "Bot", "copilot", ""),
        note("R4", "User", "alice")
    )]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".threads.owed_bot"), "1");
    assert_eq!(r.field(".threads.owed_human"), "0");
    assert_eq!(r.field(".threads.items | length"), "1");
    assert_eq!(
        r.field_c(".threads.items[0]"),
        r#"{"id":"R1","type":"review","path":null,"line":null,"outdated":null,"kind":"bot","last":"copilot","mark":"i","author":"copilot","url":null,"excerpt":"findings"}"#
    );
}

// @test "threads: bot top-level comments are owed; humans' are out of scope"
#[test]
fn threads_bot_top_level_comments_are_owed_humans_are_out_of_scope() {
    let sd = sd();
    sd.feat_pr(&[&format!(".comments.nodes = [{}, {}]", note("C1", "Bot", "coderabbitai"), note("C2", "User", "alice"))]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".threads.owed_bot"), "1");
    assert_eq!(r.field(".threads.owed_human"), "0");
    assert_eq!(r.field(".threads.items[0].type"), "comment");
    assert_eq!(r.field(".threads.items[0].id"), "C1");
    assert_eq!(r.field(".threads.items[0].outdated"), "null");
}

// @test "threads: reviews and comments listed in branch sd-handled are not owed"
#[test]
fn threads_reviews_and_comments_listed_in_branch_sd_handled_are_not_owed() {
    let sd = sd();
    sd.feat();
    sd.git(&["config", "--add", "branch.feat.sd-handled", "R1"]);
    sd.git(&["config", "--add", "branch.feat.sd-handled", "C1"]);
    sd.pr_fixture(
        "feat",
        &[
            &format!(".reviews.nodes = [{}, {}]", note("R1", "Bot", "copilot"), note("R2", "Bot", "copilot")),
            &format!(".comments.nodes = [{}]", note("C1", "Bot", "copilot")),
        ],
    );
    let r = sd.state(&[]);
    assert_eq!(r.field(".threads.owed_bot"), "1");
    assert_eq!(r.field(r#"[.threads.items[].id] | join(",")"#), "R2");
}

// @test "threads: by_type counts owed bot items of each type"
#[test]
fn threads_by_type_counts_owed_bot_items_of_each_type() {
    let sd = sd();
    sd.feat_pr(&[
        &format!(".reviewThreads.nodes = [{}, {}]", thread("Bot", "copilot", "Bot", "copilot"), thread("User", "alice", "User", "alice")),
        &format!(".reviews.nodes = [{}, {}]", note("R1", "Bot", "copilot"), note("R2", "Bot", "copilot")),
        &format!(".comments.nodes = [{}]", note("C1", "Bot", "copilot")),
    ]);
    let r = sd.state(&[]);
    assert_eq!(r.field_c(".threads.by_type"), r#"{"thread":1,"review":2,"comment":1}"#);
    assert_eq!(r.field(".threads.owed_bot"), "4");
    assert_eq!(r.field(".threads.owed_human"), "1");
}

// @test "threads: gh failing leaves by_type unknown"
#[test]
fn threads_gh_failing_leaves_by_type_unknown() {
    let sd = sd();
    sd.feat();
    let r = sd.state_env(&[("GH_FAKE_FAIL", "gh: down")], &[]);
    assert_eq!(r.field(".threads.by_type"), "null");
}

// @test "threads: settings.botLogin takes several logins, with or without [bot]"
#[test]
fn threads_settings_botlogin_takes_several_logins_with_or_without_bot() {
    let sd = sd();
    sd.feat();
    sd.sd_config(&["set", "--global", "--json", "settings.botLogin", r#"["marge", "homer[bot]"]"#]);
    sd.pr_fixture(
        "feat",
        &[
            &format!(".reviewThreads.nodes = [{}, {}]", thread("User", "marge", "User", "marge"), thread("User", "homer", "User", "homer")),
            &format!(".reviews.nodes = [{}]", note("R1", "User", "marge[bot]")),
            &format!(".comments.nodes = [{}, {}]", note("C1", "User", "homer"), note("C2", "User", "alice")),
        ],
    );
    let r = sd.state(&[]);
    assert_eq!(r.field(".threads.owed_bot"), "4");
    assert_eq!(r.field(".threads.owed_human"), "0");
}

// @test "threads: bots in settings.botIgnore are never owed"
#[test]
fn threads_bots_in_settings_botignore_are_never_owed() {
    let sd = sd();
    sd.feat();
    sd.sd_config(&["set", "--global", "--json", "settings.botIgnore", r#"["github-actions", "vercel[bot]"]"#]);
    sd.pr_fixture(
        "feat",
        &[
            &format!(
                ".reviewThreads.nodes = [{}, {}]",
                thread("Bot", "github-actions", "Bot", "github-actions"),
                thread("Bot", "copilot", "Bot", "copilot")
            ),
            &format!(".reviews.nodes = [{}]", note("R1", "Bot", "vercel")),
            &format!(".comments.nodes = [{}, {}]", note("C1", "Bot", "github-actions[bot]"), note("C2", "Bot", "copilot")),
        ],
    );
    let r = sd.state(&[]);
    assert_eq!(r.field(".threads.owed_bot"), "2");
    assert_eq!(r.field(r#"[.threads.items[].last] | join(",")"#), "copilot,copilot");
}

// @test "threads: git config sheepdog.botLogin is not read"
#[test]
fn threads_git_config_sheepdog_botlogin_is_not_read() {
    let sd = sd();
    sd.feat();
    sd.git(&["config", "sheepdog.botLogin", "marge"]);
    sd.pr_fixture("feat", &[&format!(".reviews.nodes = [{}]", note("R1", "User", "marge"))]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".threads.owed_bot"), "0");
}

// @test "threads: botLogin and botIgnore come from the branch file too"
#[test]
fn threads_botlogin_and_botignore_come_from_the_branch_file_too() {
    let sd = sd();
    sd.feat();
    fs::create_dir_all(sd.cwd.join(".git/sheepdog")).unwrap();
    sd.write(".git/sheepdog/feat.json", "{\"settings\": {\"botLogin\": [\"marge\"]}}\n");
    sd.pr_fixture("feat", &[&format!(".reviews.nodes = [{}]", note("R1", "User", "marge"))]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".threads.owed_bot"), "1");
}

// @test "threads: botLogin and botIgnore come from the config file"
#[test]
fn threads_botlogin_and_botignore_come_from_the_config_file() {
    let sd = sd();
    sd.feat();
    sd.write_global(r#"{"settings": {"botLogin": ["marge"], "botIgnore": ["vercel"]}}"#);
    sd.pr_fixture("feat", &[&format!(".reviews.nodes = [{}, {}]", note("R1", "User", "marge"), note("R2", "Bot", "vercel"))]);
    let r = sd.state(&[]);
    assert_eq!(r.field(r#"[.threads.items[].id] | join(",")"#), "R1");
}

// @test "threads: bot reviews and comments whose body matches botIgnoreBody are never owed"
#[test]
fn threads_bot_reviews_and_comments_whose_body_matches_botignorebody_are_never_owed() {
    let sd = sd();
    sd.feat();
    sd.write_global(r#"{"settings": {"botIgnoreBody": ["is on the job\\.", "^Deployed"]}}"#);
    sd.pr_fixture(
        "feat",
        &[
            &format!(
                ".reviews.nodes = [{}, {}]",
                note_body("R1", "Bot", "marge", "Marge is on the job. Wait."),
                note_body("R2", "Bot", "marge", "Real finding")
            ),
            &format!(
                ".comments.nodes = [{}, {}]",
                note_body("C1", "Bot", "github-actions", "Deployed to preview"),
                note_body("C2", "Bot", "github-actions", "Not Deployed")
            ),
        ],
    );
    let r = sd.state(&[]);
    assert_eq!(r.field(r#"[.threads.items[].id] | join(",")"#), "R2,C2");
}

// @test "threads: an invalid botIgnoreBody regex fails loudly"
#[test]
fn threads_an_invalid_botignorebody_regex_fails_loudly() {
    let sd = sd();
    sd.feat();
    sd.write_global(r#"{"settings": {"botIgnoreBody": ["("]}}"#);
    sd.pr_fixture("feat", &[&format!(".comments.nodes = [{}]", note("C1", "Bot", "copilot"))]);
    let r = sd.run(&["--json"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["botIgnoreBody"]);
}

// @test "config: remote comes from the config file"
#[test]
fn config_remote_comes_from_the_config_file() {
    let sd = sd();
    sd.git(&["remote", "rename", "origin", "upstream"]);
    sd.write_global(r#"{"settings": {"remote": "upstream"}}"#);
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.txt", "a");
    sd.git(&["push", "-q", "upstream", "feat"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".push.remote_sha"), sd.git(&["rev-parse", "feat"]));
}

// @test "config: an unreadable config file fails"
#[test]
fn config_an_unreadable_config_file_fails() {
    let sd = sd();
    sd.write_global("not json");
    let r = sd.run(&["--json"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["sd config"]);
}

// @test "summary: review bodies and comments count as info, threads as blocking"
#[test]
fn summary_review_bodies_and_comments_count_as_info_threads_as_blocking() {
    let sd = sd();
    sd.feat_pr(&[
        &format!(".reviewThreads.nodes = [{}]", thread("Bot", "copilot", "Bot", "copilot")),
        &format!(".reviews.nodes = [{}]", note("R1", "Bot", "copilot")),
        &format!(".comments.nodes = [{}]", note("C1", "Bot", "copilot")),
    ]);
    let r = sd.run(&[]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &[" · !:1 i:2"]);
    assert_lacks(&r.output, "threads:");
}

// @test "summary: nothing owed still shows both counts"
#[test]
fn summary_nothing_owed_still_shows_both_counts() {
    let sd = sd();
    sd.feat_pr(&[]);
    let r = sd.run(&[]);
    assert_has(&r.output, &["!:0 i:0"]);
}

// ── marks: ! blocks, i is to read ────────────────────────────────────────────

// @test "marks: every thread is !, bot review bodies and comments are i"
#[test]
fn marks_every_thread_is_bot_review_bodies_and_comments_are_i() {
    let sd = sd();
    sd.feat_pr(&[
        &format!(".reviewThreads.nodes = [{}]", thread("User", "alice", "User", "alice")),
        &format!(".reviews.nodes = [{}]", review("R1", "Bot", "copilot", "COMMENTED")),
        &format!(".comments.nodes = [{}]", note("C1", "Bot", "copilot")),
    ]);
    let r = sd.state(&[]);
    assert_eq!(r.field(r#"[.threads.items[] | "\(.id)=\(.mark)"] | join(",")"#), "T-alice-alice=!,R1=i,C1=i");
    assert_eq!(r.field(".threads.blocking"), "1");
    assert_eq!(r.field(".threads.info"), "2");
}

// @test "marks: a bot review that requests changes is !"
#[test]
fn marks_a_bot_review_that_requests_changes_is() {
    let sd = sd();
    sd.feat_pr(&[&format!(".reviews.nodes = [{}]", review("R1", "Bot", "copilot", "CHANGES_REQUESTED"))]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".threads.items[0].mark"), "!");
    assert_eq!(r.field(".threads.blocking"), "1");
    assert_eq!(r.field(".threads.info"), "0");
}

// @test "marks: a human's latest review requesting changes is !; a later approval clears it"
#[test]
fn marks_a_humans_latest_review_requesting_changes_is_a_later_approval_clears_it() {
    let sd = sd();
    sd.feat_pr(&[&format!(".reviews.nodes = [{}]", review_body("R1", "User", "alice", "CHANGES_REQUESTED", "Please split this"))]);
    let r = sd.state(&[]);
    assert_eq!(r.field_c("[.threads.items[] | {id, kind, mark}]"), r#"[{"id":"R1","kind":"human","mark":"!"}]"#);
    assert_eq!(r.field(".threads.owed_human"), "1");
    sd.pr_fixture(
        "feat",
        &[&format!(
            ".reviews.nodes = [{}, {}]",
            review("R1", "User", "alice", "CHANGES_REQUESTED"),
            review_body("R2", "User", "alice", "APPROVED", "")
        )],
    );
    let r = sd.state(&[]);
    assert_eq!(r.field(".threads.blocking"), "0");
    assert_eq!(r.field(".threads.items | length"), "0");
}

// @test "marks: a later comment-only review doesn't clear a request for changes"
#[test]
fn marks_a_later_comment_only_review_doesnt_clear_a_request_for_changes() {
    let sd = sd();
    sd.feat_pr(&[&format!(
        ".reviews.nodes = [{}, {}]",
        review("R1", "User", "alice", "CHANGES_REQUESTED"),
        review_body("R2", "User", "alice", "COMMENTED", "one more thing")
    )]);
    let r = sd.state(&[]);
    assert_eq!(r.field_c("[.threads.items[] | {id, mark}]"), r#"[{"id":"R1","mark":"!"}]"#);
}

// @test "items: url, author and a one-line excerpt"
#[test]
fn items_url_author_and_a_one_line_excerpt() {
    let sd = sd();
    let body = r#"<!-- linear-linkback -->\n<p><a href=\"https://linear.app/x\">DEVOPS-1 Do the thing</a></p>"#;
    sd.feat_pr(&[
        &format!(".reviews.nodes = [{}]", review_body("R1", "Bot", "claude", "COMMENTED", r"\n## Review: Move monitors (pass 2)\n\nmore")),
        &format!(r#".comments.nodes = [{{id: "C1", url: "https://example/pr/7#C1", body: "{body}", author: {{__typename: "Bot", login: "linear-code"}}}}]"#),
    ]);
    let r = sd.state(&[]);
    assert_eq!(
        r.field_c("[.threads.items[] | {author, excerpt, url}]"),
        r#"[{"author":"claude","excerpt":"Review: Move monitors (pass 2)","url":"https://example/pr/7#R1"},{"author":"linear-code","excerpt":"DEVOPS-1 Do the thing","url":"https://example/pr/7#C1"}]"#
    );
}

// @test "items: a thread's url and excerpt come from its first comment"
#[test]
fn items_a_threads_url_and_excerpt_come_from_its_first_comment() {
    let sd = sd();
    sd.feat_pr(&[
        &format!(".reviewThreads.nodes = [{}]", thread("Bot", "copilot", "Bot", "copilot")),
        r#".reviewThreads.nodes[0].first.nodes[0] += {url: "https://example/pr/7#t1", body: "Rename `one`?"}"#,
    ]);
    let r = sd.state(&[]);
    assert_eq!(
        r.field_c(".threads.items[0] | {author, excerpt, url}"),
        r#"{"author":"copilot","excerpt":"Rename `one`?","url":"https://example/pr/7#t1"}"#
    );
}

// @test "items: a multi-line HTML comment is skipped in the excerpt"
#[test]
fn items_a_multi_line_html_comment_is_skipped_in_the_excerpt() {
    let sd = sd();
    sd.feat_pr(&[&format!(".comments.nodes = [{}]", note_body("C1", "Bot", "copilot", r"<!--\nccr-overview\n-->\n## Real summary"))]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".threads.items[0].excerpt"), "Real summary");
}

// @test "items: a long excerpt is cut to 100 characters"
#[test]
fn items_a_long_excerpt_is_cut_to_100_characters() {
    let sd = sd();
    sd.feat_pr(&[&format!(".comments.nodes = [{}]", note_body("C1", "Bot", "copilot", &"x".repeat(150)))]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".threads.items[0].excerpt | length"), "100");
}

// ── stored: what sheepdog keeps for the branch ────────────────────────────────

// @test "stored: handled ids and the branch's sheepdog refs"
#[test]
fn stored_handled_ids_and_the_branchs_sheepdog_refs() {
    let sd = sd();
    sd.feat();
    sd.git(&["config", "--add", "branch.feat.sd-handled", "R1"]);
    sd.git(&["config", "--add", "branch.feat.sd-handled", "C1"]);
    sd.git(&["update-ref", "refs/sheepdog/feat/local", "HEAD"]);
    sd.git(&["update-ref", "refs/sheepdog/feat/bot", "HEAD"]);
    sd.git(&["update-ref", "refs/sheepdog/other/local", "HEAD"]);
    let r = sd.state(&[]);
    assert_eq!(r.field_c(".stored.handled"), r#"["R1","C1"]"#);
    assert_eq!(r.field_c(".stored.refs | keys"), r#"["bot","local"]"#);
    assert_eq!(r.field(".stored.refs.local"), sd.git(&["rev-parse", "HEAD"]));
}

// @test "stored: a branch with a slash keeps the rest of the ref name"
#[test]
fn stored_a_branch_with_a_slash_keeps_the_rest_of_the_ref_name() {
    let sd = sd();
    sd.git(&["switch", "-qc", "topic/sub"]);
    sd.commit("a.txt", "a");
    sd.git(&["update-ref", "refs/sheepdog/topic/sub/local", "HEAD"]);
    let r = sd.state(&[]);
    assert_eq!(r.field_c(".stored.refs | keys"), r#"["local"]"#);
}

// @test "stored: nothing stored is empty, not missing"
#[test]
fn stored_nothing_stored_is_empty_not_missing() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    let r = sd.state(&[]);
    assert_eq!(r.field_c(".stored"), r#"{"handled":[],"refs":{}}"#);
}

// ── --long: the one-liner, then one line per item ────────────────────────────

// @test "--long: one line per item with its mark, type, author, excerpt and url"
#[test]
fn long_one_line_per_item_with_its_mark_type_author_excerpt_and_url() {
    let sd = sd();
    sd.feat_pushed();
    sd.git(&["update-ref", "refs/sheepdog/feat/local", "HEAD"]);
    sd.pr_fixture(
        "feat",
        &[
            &format!(".reviewThreads.nodes = [{}]", thread("Bot", "copilot", "Bot", "copilot")),
            r#".reviewThreads.nodes[0].first.nodes[0] += {url: "https://example/pr/7#t1", body: "Rename it"}"#,
            &format!(".reviews.nodes = [{}]", review_body("R1", "Bot", "claude", "COMMENTED", "Looks fine")),
        ],
    );
    let r = sd.run(&["--long"]);
    assert_eq!(r.status, 0);
    let lines = r.lines();
    assert_eq!(lines.len(), 4, "{}", r.output);
    assert!(lines[0].starts_with("https://example/pr/7 open · ") && lines[0].contains("!:1 i:1"), "{}", lines[0]);
    assert_eq!(lines[1], "NEXT  agent  address 2 bot items  → sd act address");
    assert_match(lines[2], r"^ +! +thread +copilot +file.txt:2 +Rename it +https://example/pr/7#t1$");
    assert_match(lines[3], r"^ +i +review +claude +Looks fine +https://example/pr/7#R1$");
}

// @test "--long: nothing owed is the one-liner and the next steps"
#[test]
fn long_nothing_owed_is_the_one_liner_and_the_next_steps() {
    let sd = sd();
    sd.feat();
    sd.git(&["update-ref", "refs/sheepdog/feat/local", "HEAD"]);
    sd.pr_fixture("feat", &[]);
    let r = sd.run(&["--long"]);
    assert_eq!(r.status, 0);
    let lines = r.lines();
    assert_eq!(lines.len(), 3, "{}", r.output);
    assert!(lines[1].starts_with("NEXT  run    fetch"), "{}", lines[1]);
    assert!(lines[2].starts_with("then  run    push"), "{}", lines[2]);
}

// @test "--long and --json together is a usage error"
#[test]
fn long_and_json_together_is_a_usage_error() {
    let sd = sd();
    let r = sd.run(&["--long", "--json"]);
    assert_ne!(r.status, 0);
}

// ── flags: suppressions, skips, edited tests, for chrismo (#125) ─────────────

// @test "flags: a suppression added on the branch, with the commit that added it"
#[test]
fn flags_a_suppression_added_on_the_branch_with_the_commit_that_added_it() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.sh", "echo hi\n# shellcheck disable=SC2086\necho $x");
    let added = sd.git(&["rev-parse", "HEAD"]);
    sd.commit("b.txt", "b");
    let r = sd.state(&[]);
    assert_eq!(r.field(".flags | length"), "1");
    assert_eq!(r.field(r#".flags[0] | [.kind, .file, .line, .n] | join(" ")"#), "suppression a.sh 2 1");
    assert_eq!(r.field(".flags[0].text"), "# shellcheck disable=SC2086");
    assert_eq!(r.field(r#".flags[0].commits | join(" ")"#), added);
}

// @test "flags: hits in one file each name their own commit"
#[test]
fn flags_hits_in_one_file_each_name_their_own_commit() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.sh", "# shellcheck disable=SC1\necho a");
    let first = sd.git(&["rev-parse", "HEAD"]);
    sd.commit("a.sh", "# shellcheck disable=SC1\necho a\n# eslint-disable-line\n# noqa");
    let second = sd.git(&["rev-parse", "HEAD"]);
    let r = sd.state(&[]);
    assert_eq!(
        r.field(r#"[.flags[] | "\(.line) \(.commits[0])"] | join(", ")"#),
        format!("1 {first}, 3 {second}, 4 {second}")
    );
}

// @test "flags: each default suppression is caught"
#[test]
fn flags_each_default_suppression_is_caught() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit(
        "s.txt",
        "// eslint-disable-next-line\nx = 1  # noqa: E501\n// @ts-ignore\n// @ts-expect-error\ny = 2  # type: ignore\nz := 3 //nolint\n# rubocop:disable Style\n#[allow(dead_code)]\nplain line",
    );
    let r = sd.state(&[]);
    assert_eq!(r.field(r#"[.flags[] | select(.kind == "suppression") | .line] | join(" ")"#), "1 2 3 4 5 6 7 8");
}

// @test "flags: a test skip in a test file; the same word elsewhere isn't one"
#[test]
fn flags_a_test_skip_in_a_test_file_the_same_word_elsewhere_isnt_one() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("x.bats", "@test \"a\" {\n  skip \"later\"\n  true\n}");
    sd.commit("notes.txt", "skip this\nxit(1)");
    sd.commit("spec/a_spec.rb", "it \"b\" do\n  pending \"soon\"\nend");
    sd.commit("test/a.test.js", "xit(\"c\", () => {})\ndescribe.skip(\"d\")\nxdescribe(\"e\")");
    sd.commit("b_test.go", "func TestB(t *testing.T) {\n\tt.Skip(\"no\")\n}");
    sd.commit("tests/test_c.py", "@pytest.mark.skip(reason=\"x\")\ndef test_c(): pass");
    let r = sd.state(&[]);
    assert_eq!(
        r.field(r#"[.flags[] | select(.kind == "skip") | "\(.file):\(.line)"] | join(" ")"#),
        "b_test.go:2 spec/a_spec.rb:2 test/a.test.js:1 test/a.test.js:2 test/a.test.js:3 tests/test_c.py:1 x.bats:2"
    );
}

// @test "flags: a guarded bats skip (cond || skip, && skip, ; skip) is a skip; skipped as a word isn't"
#[test]
fn flags_a_guarded_bats_skip_cond_skip_skip_skip_is_a_skip_skipped_as_a_word_isnt() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit(
        "x.bats",
        "@test \"a\" {\n  command -v plutil || skip \"no plutil\"\n  [ -n \"$CI\" ] && skip\n  true; skip \"later\"\n  echo skipped\n}",
    );
    let r = sd.state(&[]);
    assert_eq!(r.field(r#"[.flags[] | select(.kind == "skip") | .line] | join(" ")"#), "2 3 4");
}

// @test "flags: a test file moved out of the test files, even unchanged, is a test-edit"
#[test]
fn flags_a_test_file_moved_out_of_the_test_files_even_unchanged_is_a_test_edit() {
    let sd = sd();
    sd.commit("a.bats", "@test \"a\" {\n  true\n}");
    sd.commit("test/b.sh", "true");
    sd.commit("c.bats", "@test \"c\" {\n  true\n}");
    sd.git(&["push", "-q", "origin", "main"]);
    sd.git(&["switch", "-qc", "feat"]);
    sd.git(&["mv", "a.bats", "a.bats.off"]);
    sd.git(&["mv", "test/b.sh", "b.sh"]);
    sd.git(&["mv", "c.bats", "d.bats"]);
    sd.git(&["commit", "-qm", "move"]);
    let moved = sd.git(&["rev-parse", "HEAD"]);
    let r = sd.state(&[]);
    assert_eq!(
        r.field(r#"[.flags[] | "\(.kind) \(.file):\(.line) -\(.n)"] | join(", ")"#),
        "test-edit a.bats:1 -3, test-edit test/b.sh:1 -1"
    );
    assert_eq!(r.field(r#".flags[] | select(.file == "a.bats") | .text"#), "moved to a.bats.off, out of the test files");
    assert_eq!(r.field(r#".flags[] | select(.file == "a.bats") | .commits | join(" ")"#), moved);
}

// @test "flags: a test file renamed and edited past git's rename detection is one test-edit: renamed, N lines changed (#327)"
#[test]
fn flags_a_test_file_renamed_and_edited_past_gits_rename_detection_is_one_test_edit() {
    let sd = sd();
    let mut old = String::from("setup() {\n  export CLAUDE_DIR=\"$BATS_TEST_TMPDIR/claude\"\n  MONITOR=bin/claude-usage-monitor\n}\n");
    for i in 1..=8 {
        old.push_str(&format!(
            "\n@test \"claude-usage-monitor case {i}\" {{\n  echo '{{\"tokens\": {i}}}' > \"$CLAUDE_DIR/{i}.json\"\n  run \"$MONITOR\" --case {i}\n  [ \"$status\" -eq 0 ]\n  [[ \"$output\" == *\"claude usage: {i}\"* ]]\n}}\n"
        ));
    }
    let new = old.replace("claude", "ai").replace("CLAUDE", "AI");
    sd.commit("claude-usage-monitor.bats", old.trim_end());
    sd.git(&["push", "-q", "origin", "main"]);
    sd.git(&["switch", "-qc", "feat"]);
    sd.git(&["rm", "-q", "claude-usage-monitor.bats"]);
    sd.commit("ai-usage-monitor.bats", new.trim_end());
    let renamed = sd.git(&["rev-parse", "HEAD"]);
    // the precondition: git's own -M sees a delete and an add
    assert_has(&sd.git(&["diff", "-M", "--name-status", "main", "feat"]), &["D\tclaude-usage-monitor.bats"]);
    let r = sd.state(&[]);
    assert_eq!(
        r.field(r#"[.flags[] | "\(.kind) \(.file):\(.line) -\(.n)"] | join(", ")"#),
        "test-edit claude-usage-monitor.bats:1 -26"
    );
    assert_eq!(r.field(".flags[0].text"), "renamed to ai-usage-monitor.bats, 26 of its 60 lines changed");
    assert_eq!(r.field(".flags[0].commits | join(\" \")"), renamed);
}

// @test "flags: a test file deleted and an unrelated one added, alike only in boilerplate and the word test, is still a deletion (#327)"
#[test]
fn flags_a_test_file_deleted_and_an_unrelated_one_added_alike_in_boilerplate_is_still_a_deletion() {
    let sd = sd();
    let file = |name: &str| {
        let mut s = String::from("setup() {\n  load helper\n}\n");
        for i in 1..=4 {
            s.push_str(&format!(
                "\n@test \"{name} {i}\" {{\n  run {name} {i}\n  [ \"$status\" -eq 0 ]\n  [[ \"$output\" == *\"{name} {i}\"* ]]\n}}\n"
            ));
        }
        s
    };
    sd.commit("test_foo.bats", file("foo").trim_end());
    sd.git(&["push", "-q", "origin", "main"]);
    sd.git(&["switch", "-qc", "feat"]);
    sd.git(&["rm", "-q", "test_foo.bats"]);
    sd.commit("test_bar.bats", file("bar").trim_end());
    // the precondition: git's own -M sees a delete and an add, and -M20% alone would pair them
    assert_has(&sd.git(&["diff", "-M", "--name-status", "main", "feat"]), &["D\ttest_foo.bats"]);
    assert_has(&sd.git(&["diff", "-M20%", "--name-status", "main", "feat"]), &["R"]);
    let r = sd.state(&[]);
    assert_eq!(
        r.field(r#"[.flags[] | "\(.kind) \(.file):\(.line) -\(.n)"] | join(", ")"#),
        "test-edit test_foo.bats:1 -27"
    );
}

// @test "flags: a test file renamed with only lines added, past -M's 50%, isn't one: nothing it had changed"
#[test]
fn flags_a_test_file_renamed_with_only_lines_added_past_gits_rename_detection_isnt_one() {
    let sd = sd();
    let old = "@test \"usage a\" {\n  run usage a\n  [ \"$status\" -eq 0 ]\n}";
    sd.commit("old-usage.bats", old);
    sd.git(&["push", "-q", "origin", "main"]);
    sd.git(&["switch", "-qc", "feat"]);
    sd.git(&["rm", "-q", "old-usage.bats"]);
    sd.commit("new-usage.bats", &format!("{old}\n\n{}\n\n{}", old.replace(" a", " b"), old.replace(" a", " c")));
    // the precondition: git's own -M sees a delete and an add, and -M20% alone would pair them
    assert_has(&sd.git(&["diff", "-M", "--name-status", "main", "feat"]), &["D\told-usage.bats"]);
    assert_has(&sd.git(&["diff", "-M20%", "--name-status", "main", "feat"]), &["R"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(r#"[.flags[] | select(.kind == "test-edit")] | length"#), "0");
}

// @test "flags: a base that isn't in HEAD's history (a tree restack due) isn't diffed: flags_error says why"
#[test]
fn flags_a_base_that_isnt_in_heads_history_a_tree_restack_due_isnt_diffed_flags_error_says_why() {
    let sd = sd();
    sd.git(&["switch", "-qc", "parent"]);
    sd.commit("s.txt", "X");
    sd.commit("t.txt", "Y");
    sd.git(&["push", "-q", "origin", "parent"]);
    let (p1, p2) = (sd.git(&["rev-parse", "parent~1"]), sd.git(&["rev-parse", "parent"]));
    sd.git(&["switch", "-qc", "feat", "main"]);
    sd.write("s.txt", "X\n");
    sd.write("t.txt", "Y\n");
    sd.git(&["add", "s.txt", "t.txt"]);
    sd.git(&["commit", "-qm", "parent, copied"]);
    sd.commit("c.sh", "# noqa");
    sd.git(&["config", "branch.feat.sd-parent", "parent"]);
    sd.git(&["update-ref", "refs/sheepdog/feat/base", &p1]);
    sd.squash_parent_away();
    sd.pr_fixture("parent", &[r#".state = "MERGED""#, &head_oid(&p2)]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".sync.by_tree"), "true");
    assert_eq!(r.field(".flags"), "null");
    assert_has(&r.field(".flags_error"), &["isn't in feat's history"]);
}

// @test "flags: lines changed or deleted in a test file the base had; added lines aren't"
#[test]
fn flags_lines_changed_or_deleted_in_a_test_file_the_base_had_added_lines_arent() {
    let sd = sd();
    sd.commit("t.bats", "@test \"one\" {\n  [ 1 -eq 1 ]\n}\n@test \"two\" {\n  true\n}");
    sd.commit("gone.bats", "@test \"x\" {\n  true\n}");
    sd.git(&["push", "-q", "origin", "main"]);
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("t.bats", "@test \"one\" {\n  [ 1 -eq 2 ]\n}\n@test \"two\" {\n  true\n}\n@test \"three\" {\n  true\n}");
    let edited = sd.git(&["rev-parse", "HEAD"]);
    sd.git(&["rm", "-q", "gone.bats"]);
    sd.git(&["commit", "-qm", "drop gone"]);
    let dropped = sd.git(&["rev-parse", "HEAD"]);
    sd.commit("new.bats", "@test \"new\" {\n  true\n}");
    sd.commit("file.txt", "changed");
    let r = sd.state(&[]);
    assert_eq!(
        r.field(r#"[.flags[] | "\(.kind) \(.file):\(.line) -\(.n)"] | join(", ")"#),
        "test-edit gone.bats:1 -3, test-edit t.bats:2 -1"
    );
    assert_eq!(r.field(r#".flags[] | select(.file == "t.bats") | .text"#), "  [ 1 -eq 1 ]");
    assert_eq!(r.field(r#".flags[] | select(.file == "t.bats") | .commits | join(" ")"#), edited);
    assert_eq!(r.field(r#".flags[] | select(.file == "gone.bats") | .commits | join(" ")"#), dropped);
}

// @test "flags: a line added and removed again on the branch isn't one"
#[test]
fn flags_a_line_added_and_removed_again_on_the_branch_isnt_one() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.sh", "# shellcheck disable=SC2086\necho $x");
    sd.commit("a.sh", "echo \"$x\"");
    let r = sd.state(&[]);
    assert_eq!(r.field(".flags"), "[]");
}

// @test "flags: settings.flags replaces a list; the others keep their defaults"
#[test]
fn flags_settings_flags_replaces_a_list_the_others_keep_their_defaults() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.sh", "# shellcheck disable=SC2086\n# lint-off: here");
    sd.commit("checks/a.sh", "skip");
    sd.sd_config(&["set", "--global", "--json", "settings.flags", r#"{"suppressions": ["lint-off"], "testFiles": ["checks/"]}"#]);
    let r = sd.state(&[]);
    assert_eq!(
        r.field(r#"[.flags[] | "\(.kind) \(.file):\(.line)"] | join(", ")"#),
        "suppression a.sh:2, skip checks/a.sh:1"
    );
}

// @test "flags: an invalid pattern is flags_error, not an error that steers NEXT; flags is null"
#[test]
fn flags_an_invalid_pattern_is_flags_error_not_an_error_that_steers_next_flags_is_null() {
    let sd = sd();
    sd.feat();
    sd.sd_config(&["set", "--global", "--json", "settings.flags", r#"{"skips": ["("]}"#]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".flags"), "null");
    assert_has(&r.field(".flags_error"), &["settings.flags"]);
    assert_eq!(r.field(".errors | length"), "0");
    assert_eq!(r.field(r#"[.next[].key] | index("errors")"#), "null");
    let r = sd.run(&[]);
    assert_eq!(r.status, 0);
    assert_has(r.lines()[0], &[" · ⚑ check failed"]);
}

// @test "flags: flags_error is null when the check ran"
#[test]
fn flags_flags_error_is_null_when_the_check_ran() {
    let sd = sd();
    sd.feat();
    let r = sd.state(&[]);
    assert_eq!(r.field(".flags_error"), "null");
}

// @test "flags: no base, no check: flags is null"
#[test]
fn flags_no_base_no_check_flags_is_null() {
    let sd = sd();
    sd.feat();
    sd.git(&["config", "branch.feat.sd-parent", "gone"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".flags"), "null");
}

// @test "flags: counted on the one-liner, one line each in --long"
#[test]
fn flags_counted_on_the_one_liner_one_line_each_in_long() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.sh", "# shellcheck disable=SC2086\necho $x");
    let added = sd.git(&["rev-parse", "--short=7", "HEAD"]);
    let r = sd.run(&[]);
    assert_eq!(r.status, 0);
    assert_has(r.lines()[0], &[" · ⚑ 1 flagged"]);
    let r = sd.run(&["--long"]);
    assert_eq!(r.status, 0);
    assert_match(
        r.lines().last().unwrap(),
        &format!(r"^ +⚑ +suppression +a\.sh:1 +# shellcheck disable=SC2086 +{added}$"),
    );
}

// ── O8 session ───────────────────────────────────────────────────────────────

// @test "session: enrollment, harness, session and lock come from branch config"
#[test]
fn session_enrollment_harness_session_and_lock_come_from_branch_config() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.git(&["config", "branch.feat.sheepdog", "true"]);
    sd.git(&["config", "branch.feat.sd-harness", "pi"]);
    sd.git(&["config", "branch.feat.sd-session", "abc-123"]);
    sd.git(&["config", "branch.feat.sd-lock", "chrismo"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".session.enrolled"), "true");
    assert_eq!(r.field(".session.harness"), "pi");
    assert_eq!(r.field(".session.id"), "abc-123");
    assert_eq!(r.field(".session.locked"), "true");
}

// @test "session: an agent step running on the branch (harness_run's marker) is NEXT's only step; a dead one's marker is cleared (#170)"
#[test]
fn session_an_agent_step_running_on_the_branch_harness_runs_marker_is_nexts_only_step_a_dead_ones_marker_is_cleared_170() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.git(&["config", "branch.feat.sd-task", "add a"]);
    sd.write("wip.txt", "wip\n");
    let pid = std::process::id();
    sd.running(pid, "implement");
    let r = sd.state(&[]);
    assert_eq!(
        r.field_c(".session.running"),
        format!(r#"{{"step":"implement","pid":{pid},"since":"2026-10-18T14:03:00Z"}}"#)
    );
    assert_eq!(
        r.field_c("[.next[] | [.key, .who, .say]]"),
        r#"[["running","wait","implement running since 2026-10-18T14:03:00Z"]]"#
    );
    let mut dead = spawn(Command::new("sh").args(["-c", "exit 0"]));
    dead.wait().unwrap();
    sd.write(".git/sheepdog/feat.running", &format!("{} implement 2026-10-18T14:03:00Z\n", dead.id()));
    let r = sd.state(&[]);
    assert_eq!(r.field(".session.running"), "null");
    assert_eq!(r.field_c("[.next[].key]"), r#"["nothing-dirty"]"#);
    assert!(!sd.cwd.join(".git/sheepdog/feat.running").exists());
}

// @test "session: mid restack --resolve (detached, rebase stopped), the branch comes from the rebase and the live marker outranks the conflict (#179)"
#[test]
fn session_mid_restack_resolve_detached_rebase_stopped_the_branch_comes_from_the_rebase_and_the_live_marker_outranks_the_conflict_179() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("file.txt", "one\nOURS\nthree");
    sd.main_moves("file.txt", "one\nTHEIRS\nthree");
    sd.git(&["fetch", "-q"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".next[0].key"), "conflict");
    assert_ne!(sd.git_status(&["rebase", "-q", "origin/main"]), 0);
    assert!(sd.cwd.join(".git/rebase-merge").is_dir());
    assert_eq!(sd.git(&["branch", "--show-current"]), "");
    sd.running(std::process::id(), "resolve");
    let r = sd.state(&[]);
    assert_eq!(r.field(".branch"), "feat");
    assert_eq!(
        r.field_c("[.next[] | [.key, .who, .say]]"),
        r#"[["running","wait","resolve running since 2026-10-18T14:03:00Z"]]"#
    );
}

// @test "detached HEAD with no rebase and no branch named: fails with a message"
#[test]
fn detached_head_with_no_rebase_and_no_branch_named_fails_with_a_message() {
    let sd = sd();
    sd.commit("a.txt", "a");
    sd.git(&["switch", "-q", "--detach"]);
    let r = sd.run(&[]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["detached HEAD; name a branch"]);
}

// @test "session: unenrolled branch"
#[test]
fn session_unenrolled_branch() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".session.enrolled"), "false");
    assert_eq!(r.field(".session.locked"), "false");
}

// ── spend: what the branch has cost, against settings.branchBudget (#121) ────

// @test "spend: this repo's steps on the branch, summed from usage.sup, and settings.branchBudget (25 USD)"
#[test]
fn spend_this_repos_steps_on_the_branch_summed_from_usage_sup_and_settings_branchbudget_25_usd() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    let r = sd.state(&[]);
    assert_eq!(r.field_c(".spend"), r#"{"usd":0,"budget":25}"#);
    sd.usage_row("feat", "1.5", None);
    sd.usage_row("feat", "2.25", None);
    sd.usage_row("other", "9.", None);
    sd.usage_row("feat", "9.", Some("/elsewhere"));
    sd.usage_row("feat", "3.", None);
    sd.sd_config(&["set", "--json", "settings.branchBudget", "10"]);
    let r = sd.state(&[]);
    assert_eq!(r.field_c(".spend"), r#"{"usd":6.75,"budget":10}"#);
}

// @test "spend: past settings.branchBudget, NEXT is chrismo's instead of the agent's"
#[test]
fn spend_past_settings_branchbudget_next_is_chrismos_instead_of_the_agents() {
    let sd = sd();
    sd.feat_pushed();
    sd.git(&["update-ref", "refs/sheepdog/feat/local", "HEAD"]);
    sd.pr_fixture("feat", &[&format!(".reviewThreads.nodes = [{}]", thread("Bot", "copilot", "Bot", "copilot"))]);
    sd.sd_config(&["set", "--json", "settings.branchBudget", "2"]);
    sd.usage_row("feat", "2.5", None);
    let r = sd.run(&[]);
    assert_eq!(r.status, 0);
    assert_eq!(
        r.lines()[1],
        "NEXT  you    spent $2.50 on this branch, past settings.branchBudget ($2.00): raise it, or take over  → sd act attach"
    );
}

// @test "spend: a line of usage.sup that isn't a record is skipped with a warning, not an error on every branch"
#[test]
fn spend_a_line_of_usage_sup_that_isnt_a_record_is_skipped_with_a_warning_not_an_error_on_every_branch() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.usage_row("feat", "1.5", None);
    sd.append_usage(r#"{time:2026-10-16T20:34:06Z,repo:"cut off"#);
    sd.usage_row("feat", "2.", None);
    let r = sd.run_sep(&["--json"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.field(".spend.usd"), "3.5");
    assert_eq!(r.field_c(".errors"), "[]");
    let want = format!("sd state: warning: 1 line(s) of {} aren't usage records", sd.usage_sup().display());
    assert!(r.stderr.starts_with(&want), "{}", r.stderr);
}

// @test "spend: a usage.sup that still won't read is an error, not a stop"
#[test]
fn spend_a_usage_sup_that_still_wont_read_is_an_error_not_a_stop() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.append_usage("{time:soon}");
    let r = sd.state(&[]);
    assert_eq!(r.field(".spend.usd"), "null");
    assert_has(&r.field(".errors[]"), &["usage.sup"]);
}

// ── summary line ─────────────────────────────────────────────────────────────

// @test "summary: default output is one line naming the key facts, then NEXT"
#[test]
fn summary_default_output_is_one_line_naming_the_key_facts_then_next() {
    let sd = sd();
    sd.feat_pushed();
    sd.pr_fixture(
        "feat",
        &[
            &checks(r#"[{__typename:"CheckRun",name:"lint",status:"COMPLETED",conclusion:"FAILURE"}]"#),
            &format!(".reviewThreads.nodes = [{}]", thread("User", "alice", "User", "alice")),
        ],
    );
    let r = sd.run(&[]);
    assert_eq!(r.status, 0);
    let lines = r.lines();
    assert_eq!(lines.len(), 2, "{}", r.output);
    assert!(lines[0].starts_with("https://example/pr/7 open · "), "{}", lines[0]);
    assert_lacks(lines[0], "feat");
    assert_lacks(lines[0], "#7");
    assert_has(lines[0], &["CI red"]);
    assert_has(lines[0], &["!:1 i:0"]);
    assert!(
        lines[1].starts_with("NEXT  you    reply to 1 human item  → sd state --long  (+") && lines[1].ends_with(" more)"),
        "{}",
        lines[1]
    );
}

// @test "summary: a remote commit this copy pushed and then reset away says so (#86)"
#[test]
fn summary_a_remote_commit_this_copy_pushed_and_then_reset_away_says_so_86() {
    let sd = sd();
    sd.feat_pushed();
    sd.commit("b.txt", "b");
    sd.git(&["push", "-q", "origin", "feat"]);
    sd.git(&["reset", "-q", "--hard", "HEAD~1"]);
    sd.pr_fixture("feat", &[&head_oid(&sd.git(&["rev-parse", "origin/feat"]))]);
    let r = sd.run(&[]);
    assert_eq!(r.status, 0);
    assert_has(r.lines()[0], &["remote +1 (reset away here)"]);
    assert!(
        r.lines()[1].starts_with("NEXT  run    force-push: drop 1 commit the remote has, reset away here  → sd act push"),
        "{}",
        r.lines()[1]
    );
    let r = sd.state(&[]);
    assert_eq!(r.field(".push.remote_made"), "true");
    assert_eq!(r.field(".push.dirty"), "false");
    sd.write("x.txt", "x\n");
    let r = sd.state(&[]);
    assert_eq!(r.field(".push.dirty"), "true");
    assert_eq!(r.field(".next[0].key"), "reset-dirty");
}

// @test "push: someone's commit taken with a fast-forward and reset away wasn't made here: take, not drop (#86)"
#[test]
fn push_someones_commit_taken_with_a_fast_forward_and_reset_away_wasnt_made_here_take_not_drop_86() {
    let sd = sd();
    sd.feat_pushed();
    sd.git_in("other", &["fetch", "-q"]);
    sd.git_in("other", &["switch", "-q", "feat"]);
    sd.git_in("other", &["config", "user.email", "them@example.com"]);
    sd.commit_in("other", "x.txt", "x");
    sd.git_in("other", &["push", "-q", "origin", "feat"]);
    sd.git(&["fetch", "-q"]);
    sd.git(&["merge", "-q", "--ff-only", "origin/feat"]);
    sd.git(&["reset", "-q", "--hard", "HEAD~1"]);
    sd.pr_fixture("feat", &[&head_oid(&sd.git(&["rev-parse", "origin/feat"]))]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".push.remote_ours"), "true");
    assert_eq!(r.field(".push.remote_made"), "false");
    assert_eq!(r.field(".next[0].key"), "take");
}

// @test "push: remote_made is null unless the remote is ahead of a reset"
#[test]
fn push_remote_made_is_null_unless_the_remote_is_ahead_of_a_reset() {
    let sd = sd();
    sd.feat_pushed();
    let r = sd.state(&[]);
    assert_eq!(r.field(".push.remote_made"), "null");
}

// @test "push: dirty is reported for any checked-out branch, as address and ci-fix refuse it too (#178)"
#[test]
fn push_dirty_is_reported_for_any_checked_out_branch_as_address_and_ci_fix_refuse_it_too_178() {
    let sd = sd();
    sd.feat_pushed();
    let r = sd.state(&[]);
    assert_eq!(r.field(".push.dirty"), "false");
    sd.write("a.txt", "a\nx\n");
    let r = sd.state(&[]);
    assert_eq!(r.field(".push.dirty"), "true");
}

// @test "push: held CI config is reported until its commit is on the remote, and NEXT hands it to chrismo (#124)"
#[test]
fn push_held_ci_config_is_reported_until_its_commit_is_on_the_remote_and_next_hands_it_to_chrismo_124() {
    let sd = sd();
    sd.feat_pushed();
    sd.pr_fixture("feat", &[&head_oid(&sd.git(&["rev-parse", "origin/feat"]))]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".push.held"), "null");
    sd.commit("b.txt", "b");
    let head = sd.git(&["rev-parse", "HEAD"]);
    sd.git(&["config", "branch.feat.sd-held", &format!("{head}\n.github/workflows/test.yml\n.rwx/tests.yml")]);
    let r = sd.state(&[]);
    assert_eq!(
        r.field_c(".push.held"),
        format!(r#"{{"sha":"{head}","paths":[".github/workflows/test.yml",".rwx/tests.yml"]}}"#)
    );
    assert_eq!(r.field(".next[0].key"), "ci-config");
    let r = sd.run(&[]);
    assert!(
        r.lines()[1].starts_with(
            "NEXT  you    the agent changed CI config (.github/workflows/test.yml, .rwx/tests.yml): review, then sd act push  → sd act push"
        ),
        "{}",
        r.lines()[1]
    );
    sd.git(&["push", "-q", "origin", "feat"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".push.held"), "null");
}

// @test "push: a pushed commit reset away on a branch checked out nowhere: made here, dirty unknown"
#[test]
fn push_a_pushed_commit_reset_away_on_a_branch_checked_out_nowhere_made_here_dirty_unknown() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.txt", "a");
    sd.commit("b.txt", "b");
    sd.git(&["push", "-q", "-u", "origin", "feat"]);
    sd.git(&["switch", "-q", "main"]);
    sd.git(&["branch", "-qf", "feat", "feat~1"]);
    let r = sd.state(&["feat"]);
    assert_eq!(r.field(".push.remote_made"), "true");
    assert_eq!(r.field(".push.dirty"), "null");
    assert_eq!(r.field(".next[0].key"), "push");
}

// @test "next: --json carries the ordered steps"
#[test]
fn next_json_carries_the_ordered_steps() {
    let sd = sd();
    sd.feat_pushed();
    sd.pr_fixture("feat", &[&format!(".reviewThreads.nodes = [{}]", thread("Bot", "copilot", "Bot", "copilot"))]);
    sd.commit("b.txt", "b");
    sd.git(&["update-ref", "refs/sheepdog/feat/local", "HEAD"]);
    let r = sd.state(&[]);
    assert_eq!(r.keys(), "bot push"); // no ci-wait: CI can't run before the push
    assert_eq!(r.field(".next[1].cmd"), "sd act push");
}

// @test "next: a pushed branch with an open PR and nothing owed is ready to merge"
#[test]
fn next_a_pushed_branch_with_an_open_pr_and_nothing_owed_is_ready_to_merge() {
    let sd = sd();
    sd.feat_pushed();
    sd.git(&["update-ref", "refs/sheepdog/feat/local", "HEAD"]);
    sd.pr_fixture("feat", &[]);
    let r = sd.run(&[]);
    assert_eq!(r.status, 0);
    assert_eq!(r.lines()[1], "NEXT  you    ready to merge  → gh pr merge");
}

// @test "next: uses the configured remote"
#[test]
fn next_uses_the_configured_remote() {
    let sd = sd();
    sd.git(&["remote", "rename", "origin", "up"]);
    sd.feat();
    sd.sd_config(&["set", "--global", "settings.remote", "up"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(r#"[.next[] | select(.key == "push") | .say][0]"#), "push: not on up yet");
}

// @test "summary: a draft PR's link, then draft"
#[test]
fn summary_a_draft_prs_link_then_draft() {
    let sd = sd();
    sd.feat_pushed();
    sd.pr_fixture("feat", &[".isDraft = true"]);
    let r = sd.run(&[]);
    assert_eq!(r.status, 0);
    assert!(r.lines()[0].starts_with("https://example/pr/7 draft · "), "{}", r.output);
}

// @test "summary: no PR"
#[test]
fn summary_no_pr() {
    let sd = sd();
    sd.feat();
    let r = sd.run(&[]);
    assert_eq!(r.status, 0);
    assert!(r.lines()[0].starts_with("no PR · "), "{}", r.output);
    assert_has(&r.output, &["1 unpushed"]);
}

#[test]
fn summary_names_the_profile_in_effect_the_branchs_else_the_repos_293() {
    let sd = sd();
    sd.feat();
    sd.write_global(r#"{"profiles": {"base": {}, "billing": {}, "fast": {}}}"#);
    let r = sd.run(&[]);
    assert_lacks(r.lines()[0], "profile");
    assert_eq!(sd.run(&["--json"]).field_c(".session.profile"), "null");

    sd.write(".git/sheepdog.json", r#"{"profile": "base"}"#);
    let r = sd.run(&[]);
    assert!(r.lines()[0].starts_with("no PR · profile base · "), "{}", r.output);

    sd.verb_ok("config", &["set", "--json", "profile", r#"["billing", "fast"]"#]);
    let r = sd.run(&[]);
    assert!(r.lines()[0].starts_with("no PR · profile billing, fast · "), "{}", r.output);
    assert_eq!(sd.run(&["--json"]).field_c(".session.profile"), r#"["billing","fast"]"#);
}

// @test "summary: a merged PR whose remote branch is gone counts nothing unpushed (#153)"
#[test]
fn summary_a_merged_pr_whose_remote_branch_is_gone_counts_nothing_unpushed_153() {
    let sd = sd();
    sd.feat_pr(&[r#".state = "MERGED""#]);
    let r = sd.run(&[]);
    assert_eq!(r.status, 0);
    assert!(r.lines()[0].starts_with("https://example/pr/7 merged"), "{}", r.output);
    assert_lacks(r.lines()[0], "unpushed");
    assert_lacks(r.lines()[0], "not pushed");
}

// ── reviewer mode: a teammate's PR, followed by number (#250) ────────────────

impl Sd {
    /// The teammate commits <msg> on their-fix in other and pushes it as PR
    /// #7's head ref alone, as a fork's PR has it; its SHA.
    fn author_pushes(&self, msg: &str) -> String {
        if self.git_status(&["-C", &self.path("other"), "switch", "-q", "their-fix"]) != 0 {
            self.git_in("other", &["switch", "-qc", "their-fix"]);
        }
        self.git_in("other", &["commit", "-q", "--allow-empty", "-m", msg]);
        self.git_in("other", &["push", "-q", "-f", "origin", "HEAD:refs/pull/7/head"]);
        self.git_in("other", &["rev-parse", "HEAD"])
    }

    /// The PR's head fetched to refs/sheepdog/review/7/pr-head, as sd act follow
    /// and fetch do.
    fn fetch_pr_head(&self) {
        self.git(&["fetch", "-q", "origin", "+refs/pull/7/head:refs/sheepdog/review/7/pr-head"]);
    }

    /// sd act follow 7, by hand: review/7 at the PR's head, checked out here,
    /// its number and reviewer mode recorded; GitHub has PR #7 at that head,
    /// from a fork's their-fix.
    fn following(&self, assigns: &[&str]) {
        self.author_pushes("their fix");
        self.fetch_pr_head();
        self.git(&["switch", "-qc", "review/7", "refs/sheepdog/review/7/pr-head"]);
        self.git(&["config", "branch.review/7.sd-pr", "7"]);
        self.git(&["config", "branch.review/7.sd-mode", "reviewer"]);
        let all = [&[r#".headRefName = "their-fix""#, ".isCrossRepository = true"][..], assigns].concat();
        self.pr_fixture("review/7", &all);
    }
}

/// A submitted review of <commit> by <login>, as GitHub lists one.
fn review_of(id: &str, login: &str, state: &str, commit: &str) -> String {
    review_said(id, login, state, commit, "")
}

/// The same with a top-level body.
fn review_said(id: &str, login: &str, state: &str, commit: &str, body: &str) -> String {
    format!(r#"{{id: "{id}", url: "u", state: "{state}", body: "{body}", commit: {{oid: "{commit}"}}, author: {{__typename: "User", login: "{login}"}}}}"#)
}

#[test]
fn reviewer_mode_the_pr_is_its_stored_number_alone_whatever_its_head_is_called() {
    let sd = sd();
    sd.following(&[]);
    let r = sd.state(&[]);
    assert_eq!(sd.calls(), "#7 ");
    assert_eq!(r.field(".mode"), "reviewer");
    assert_eq!(r.field(".pr.number"), "7");
    assert_eq!(r.field(".errors | length"), "0", "{}", r.errors());
    assert_eq!(sd.git(&["config", "branch.review/7.sd-pr"]), "7");
}

#[test]
fn reviewer_mode_a_number_github_cant_resolve_is_an_error_not_a_discovery() {
    let sd = sd();
    sd.following(&[]);
    sd.git(&["config", "branch.review/7.sd-pr", "42"]);
    let r = sd.state(&[]);
    assert_eq!(sd.calls(), "#42 ");
    assert_eq!(r.field(".pr"), "null");
    assert_has(&r.errors(), &["GitHub has no PR #42"]);
    assert_eq!(sd.git(&["config", "branch.review/7.sd-pr"]), "42");
}

#[test]
fn reviewer_mode_the_remote_is_the_prs_head_ref_not_a_remote_branch() {
    let sd = sd();
    sd.following(&[]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".push.remote_sha"), sd.git(&["rev-parse", "refs/sheepdog/review/7/pr-head"]));
    assert_eq!(r.field_c("[.push.ahead, .push.behind, .push.remote_ours, .push.theirs, .push.author_pushed]"), "[0,0,null,null,false]");
    assert_eq!(r.field(".fresh"), "true");
}

#[test]
fn reviewer_mode_the_author_pushed_github_is_ahead_of_the_head_ref() {
    let sd = sd();
    sd.following(&[]);
    let theirs = sd.author_pushes("more");
    sd.edit_fixture("review/7", &format!(r#".data.repository.pullRequests.nodes[0].headRefOid = "{theirs}""#));
    let r = sd.state(&[]);
    assert_eq!(r.field(".fresh"), "false");
    assert_eq!(r.field(".push.author_pushed"), "true");
    assert_has(&r.field(".stale[]"), &[&format!("PR #7's head is not at GitHub's {}", &theirs[..7])]);
    sd.fetch_pr_head();
    let r = sd.state(&[]);
    assert_eq!(r.field(".fresh"), "true");
    assert_eq!(r.field_c("[.push.ahead, .push.behind]"), "[0,1]");
}

#[test]
fn reviewer_mode_a_force_push_is_behind_and_ahead_never_foreign() {
    let sd = sd();
    sd.following(&[]);
    sd.git_in("other", &["commit", "-q", "--amend", "--allow-empty", "-m", "their fix, amended"]);
    sd.git_in("other", &["push", "-q", "-f", "origin", "HEAD:refs/pull/7/head"]);
    let theirs = sd.git_in("other", &["rev-parse", "HEAD"]);
    sd.edit_fixture("review/7", &format!(r#".data.repository.pullRequests.nodes[0].headRefOid = "{theirs}""#));
    sd.fetch_pr_head();
    let r = sd.state(&[]);
    assert_eq!(r.field_c("[.push.ahead, .push.behind, .push.diverged, .push.remote_ours]"), "[1,1,true,null]");
    assert_lacks(&r.keys(), "foreign");
}

#[test]
fn reviewer_mode_github_review_is_the_viewers_last_submitted_one_and_whether_its_of_head() {
    let sd = sd();
    sd.following(&[]);
    let head = sd.git(&["rev-parse", "HEAD"]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".review.github"), "null");
    let reviews = [
        review_of("r1", "me", "CHANGES_REQUESTED", "0123456"),
        review_of("r2", "someone", "APPROVED", &head),
        review_said("r3", "me", "COMMENTED", &head, "a few questions"),
        review_of("r4", "me", "PENDING", &head),
    ]
    .join(", ");
    sd.edit_fixture("review/7", &format!(".data.repository.pullRequests.nodes[0].reviews.nodes = [{reviews}]"));
    let r = sd.state(&[]);
    assert_eq!(r.field_c(".review.github"), format!(r#"{{"sha":"{head}","state":"COMMENTED","at_head":true,"pending":true}}"#));
    let dismissed = format!("[{}, {}]", review_of("r1", "me", "CHANGES_REQUESTED", "0123456"), review_of("r5", "me", "DISMISSED", &head));
    sd.edit_fixture("review/7", &format!(".data.repository.pullRequests.nodes[0].reviews.nodes = {dismissed}"));
    let r = sd.run(&["--json"]);
    assert_eq!(r.field(".review.github.sha"), "0123456");
    sd.edit_fixture("review/7", ".data.repository.pullRequests.nodes[0].reviews.nodes |= .[:1]");
    let r = sd.state(&[]);
    assert_eq!(r.field_c(".review.github"), r#"{"sha":"0123456","state":"CHANGES_REQUESTED","at_head":false,"pending":false}"#);
}

// GitHub makes a review of each lone inline comment or thread reply:
// COMMENTED, no body. Replying isn't reviewing.
#[test]
fn reviewer_mode_a_thread_reply_isnt_a_review_of_head() {
    let sd = sd();
    sd.following(&[]);
    let head = sd.git(&["rev-parse", "HEAD"]);
    let reviews = [review_of("r1", "me", "CHANGES_REQUESTED", "0123456"), review_of("r2", "me", "COMMENTED", &head)].join(", ");
    sd.edit_fixture("review/7", &format!(".data.repository.pullRequests.nodes[0].reviews.nodes = [{reviews}]"));
    let r = sd.state(&[]);
    assert_eq!(r.field_c("[.review.github.sha, .review.github.at_head]"), r#"["0123456",false]"#);
}

#[test]
fn reviewer_mode_a_stacked_prs_stale_parent_isnt_stale() {
    let sd = sd();
    sd.git_in("other", &["push", "-q", "origin", "main:refs/heads/base"]);
    sd.git(&["fetch", "-q", "origin"]);
    sd.following(&[r#".baseRefName = "base""#]);
    sd.pr_fixture("base", &[".number = 6", r#".headRefOid = "ffffffffffffffffffffffffffffffffffffffff""#]);
    let r = sd.state(&[]);
    assert_eq!(r.field(".parent.pr.number"), "6");
    assert_eq!(r.field(".fresh"), "true", "{}", r.field(".stale[]"));
}

#[test]
fn an_authors_branch_has_no_mode_and_no_github_review() {
    let sd = sd();
    sd.git(&["switch", "-qc", "feat"]);
    sd.commit("a.txt", "a");
    sd.pr_fixture("feat", &[]);
    let r = sd.state(&[]);
    assert_eq!(r.field_c("[.mode, .review.github, .push.author_pushed]"), "[null,null,null]");
}

#[test]
fn reviewer_mode_summary_says_reviewing_and_counts_the_authors_pushes_as_theirs() {
    let sd = sd();
    sd.following(&[]);
    let r = sd.run(&[]);
    assert!(r.lines()[0].starts_with("https://example/pr/7 open · reviewing"), "{}", r.output);
    let theirs = sd.author_pushes("more");
    sd.edit_fixture("review/7", &format!(r#".data.repository.pullRequests.nodes[0].headRefOid = "{theirs}""#));
    sd.fetch_pr_head();
    let r = sd.run(&[]);
    assert_has(r.lines()[0], &["author +1"]);
    assert_lacks(r.lines()[0], "remote +");
    sd.git(&["commit", "-q", "--amend", "--allow-empty", "-m", "mine"]);
    let r = sd.run(&[]);
    assert_has(r.lines()[0], &["diverged from the PR's head"]);
    assert_lacks(r.lines()[0], "not ours");
}
