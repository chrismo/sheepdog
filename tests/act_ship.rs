// Black-box tests of sd act describe (J6: the agent writes the PR title and
// body, read-only, stored for HEAD) and sd act ship (describe if needed,
// recheck HEAD, push, open a draft PR on the parent or fix up the existing
// one, enroll) (#203). The built binary, run as sd-act through a link
// (tests/common). Moved from libexec/sd-act-ship.bats; each test keeps the
// bats test's title above it.
//
// Real git (a bare origin, our clone, other/ for other people's pushes) and
// the real sd state, in-process. gh is a stub: graphql answers from
// $GH/<branch>.json (by number: the node of whichever file has it),
// `gh pr create|edit|ready` are logged to $GH/calls. claude prints
// $FX/result.json as structured output.

mod common;

use common::{assert_has, assert_lacks, chomp, dead_pid, jq, lock_by, running_marker, step_refusal, Holder, Sd};
use std::fs;

const GH: &str = r#"#!/usr/bin/env bash
if [[ $1 == pr ]]; then
  printf '%s\n' "$*" >> "$GH/calls"
  [[ -e $GH/fail-$2 ]] && { echo "gh: boom on pr $2" >&2; exit 1; }
  [[ $2 == create ]] && echo "https://example/pr/9"
  exit 0
fi
b="" n=""
for a; do [[ $a == branch=* ]] && b=${a#branch=}; [[ $a == number=* ]] && n=${a#number=}; done
# By number: the node with that number, from the one branch file that has it.
if [[ -n $n ]]; then
  f=$(jq -rn --argjson n "$n" 'inputs | select(any(.data.repository.pullRequests.nodes[]?; .number == $n)) | input_filename' "$GH"/*.json 2>/dev/null)
  [[ $f != *$'\n'* ]] || { echo "fake gh: #$n is in more than one fixture: $f" >&2; exit 2; }
  b=$(basename "${f:-none}" .json)
fi
[[ -e $GH/fail-$b ]] && { echo "gh: boom for $b" >&2; exit 1; }
if [[ -e $GH/fail-$b-after-1 ]]; then
  [[ -e $GH/seen-$b ]] && { echo "gh: boom for $b" >&2; exit 1; }
  touch "$GH/seen-$b"
fi
if [[ -n $n ]]; then
  [[ -n $f ]] || { echo "gh: Could not resolve to a PullRequest with the number of $n." >&2; exit 1; }
  jq --argjson n "$n" '.data.repository |= {defaultBranchRef, pullRequest: (.pullRequests.nodes[] | select(.number == $n))}' "$f"
elif [[ -f $GH/$b.json ]]; then cat "$GH/$b.json"
else echo '{"data":{"viewer":{"login":"me"},"repository":{"defaultBranchRef":{"name":"main"},"pullRequests":{"nodes":[]}}}}'
fi
"#;

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
[[ -x $FX/agent ]] && "$FX/agent"
[[ -e $FX/fail ]] && { echo "claude: boom" >&2; exit 1; }
jq -n --argjson r "$(cat "$FX/result.json")" '{type: "result", is_error: false, structured_output: $r}'
"#;

/// The bats setup: origin.git and its clones repo (cwd) and other, main
/// pushed, feat one commit past it and not pushed.
fn sd() -> Sd {
    let mut sd = Sd::for_verb("act");
    sd.env = vec![
        ("SHEEPDOG_STATE_DIR".to_string(), sd.path("state")),
        ("GH".to_string(), sd.path("gh")),
        ("FX".to_string(), sd.path("fx")),
    ];
    for d in ["gh", "fx"] {
        fs::create_dir(sd.tmp.join(d)).unwrap();
    }
    sd.fake("gh", GH);
    sd.fake("claude", CLAUDE);
    sd.result(r#"{"title": "Add c", "body": "Adds the third line.\n\nRefs #1"}"#);
    sd.origin(&["repo"]);
    sd.cd("repo");
    sd.write("file.txt", "a\nb\n");
    sd.git(&["add", "file.txt"]);
    sd.git(&["commit", "-qm", "init"]);
    sd.git(&["push", "-q", "origin", "main"]);
    sd.git(&["clone", "-q", &sd.path("origin.git"), &sd.path("other")]);
    sd.git(&["switch", "-qc", "feat"]);
    sd.write("file.txt", "a\nb\nc\n");
    sd.git(&["commit", "-qam", "add c"]);
    sd
}

impl Sd {
    fn rev(&self, r: &str) -> String {
        self.git(&["rev-parse", r])
    }

    /// git config <key>'s value, or "" when unset.
    fn config(&self, key: &str) -> String {
        self.git_try(&["config", key]).unwrap_or_default()
    }

    /// A failed verify recorded against <commit> (#325).
    fn verify_red(&self, commit: &str) {
        let rec = format!(r#"{{"head": "{}", "step": "implement", "cmd": "make test", "at": "2026-10-29T12:00:00Z"}}"#, self.rev(commit));
        let blob = self.git_with(&[], Some(&format!("{rec}\n")), &["hash-object", "-w", "--stdin"]);
        self.git(&["update-ref", "refs/sheepdog/feat/verify-red", &blob]);
    }

    fn result(&self, json: &str) {
        fs::write(self.tmp.join("fx/result.json"), format!("{json}\n")).unwrap();
    }

    fn fx_exists(&self, name: &str) -> bool {
        self.tmp.join("fx").join(name).exists()
    }

    fn rm_claude_args(&self) {
        let _ = fs::remove_file(self.tmp.join("fx/claude.args"));
    }

    fn claude_args(&self) -> String {
        fs::read_to_string(self.tmp.join("fx/claude.args")).unwrap()
    }

    fn prompt(&self) -> String {
        fs::read_to_string(self.tmp.join("fx/claude.prompt")).unwrap()
    }

    fn gh_touch(&self, name: &str) {
        fs::write(self.tmp.join("gh").join(name), "").unwrap();
    }

    fn gh_fixture(&self, b: &str, json: &str) {
        fs::write(self.tmp.join(format!("gh/{b}.json")), format!("{json}\n")).unwrap();
    }

    fn open_pr(&self, b: &str, base: &str, n: u32) {
        let h = self.rev(b);
        let j = jq(
            &[
                "-n", "--arg", "b", b, "--arg", "base", base, "--argjson", "n", &n.to_string(), "--arg", "h", &h,
                r#"{data: {viewer: {login: "me"},
    repository: {defaultBranchRef: {name: "main"},
      pullRequests: {nodes: [{number: $n, state: "OPEN", isDraft: true, baseRefName: $base, headRefName: $b,
                              headRefOid: $h, isCrossRepository: false}]}}}}"#,
            ],
            "",
        );
        self.gh_fixture(b, &j);
    }

    /// $GH/A.json: A's PR #1 into main, in <state>, its head at <h>.
    fn pr_a(&self, state: &str, h: &str) {
        let j = jq(
            &[
                "-n", "--arg", "h", h, "--arg", "st", state,
                r#"{data: {viewer: {login: "me"}, repository: {defaultBranchRef: {name: "main"},
    pullRequests: {nodes: [{number: 1, state: $st, baseRefName: "main", headRefName: "A", headRefOid: $h, isCrossRepository: false}]}}}}"#,
            ],
            "",
        );
        self.gh_fixture("A", &j);
    }

    /// calls: `gh pr` as the stub logged them.
    fn calls(&self) -> String {
        chomp(&fs::read_to_string(self.tmp.join("gh/calls")).unwrap_or_default())
    }

    /// remote <b>: git ls-remote origin refs/heads/<b> | cut -f1
    fn remote(&self, b: &str) -> String {
        let out = self.git(&["ls-remote", "origin", &format!("refs/heads/{b}")]);
        out.split('\t').next().unwrap_or("").to_string()
    }

    /// tracks <b>: the branch's upstream, or "".
    fn tracks(&self, b: &str) -> String {
        self.git_try(&["rev-parse", "--abbrev-ref", &format!("{b}@{{upstream}}")]).unwrap_or_default()
    }

    /// git switch -q main && git switch -qc A && git commit -q --allow-empty -m <msg>
    fn branch_a(&self, msg: &str) {
        self.git(&["switch", "-q", "main"]);
        self.git(&["switch", "-qc", "A"]);
        self.empty_commit(msg);
    }

    /// branch_a A, pushed to origin.
    fn pushed_a(&self) {
        self.branch_a("A");
        self.git(&["push", "-q", "origin", "A"]);
    }

    fn empty_commit(&self, msg: &str) {
        self.git(&["commit", "-q", "--allow-empty", "-m", msg]);
    }

    /// git switch -q feat && git rebase -q A && git config branch.feat.sd-parent A
    fn feat_on_a(&self) {
        self.git(&["switch", "-q", "feat"]);
        self.git(&["rebase", "-q", "A"]);
        self.git(&["config", "branch.feat.sd-parent", "A"]);
    }

    /// echo <line> >> file.txt && git commit -qam <msg>
    fn add_line(&self, line: &str, msg: &str) {
        let f = self.cwd.join("file.txt");
        let s = fs::read_to_string(&f).unwrap();
        fs::write(&f, format!("{s}{line}\n")).unwrap();
        self.git(&["commit", "-qam", msg]);
    }

    /// In other/: fetch, check out main and merge origin/A with <how>
    /// (--squash, then a commit "A (#1)", or --ff-only), push main, and
    /// delete A from origin when <delete>.
    fn other_merges_a(&self, how: &str, delete: bool) {
        self.git_in("other", &["fetch", "-q", "origin"]);
        self.git_in("other", &["checkout", "-q", "main"]);
        self.git_in("other", &["merge", "-q", how, "origin/A"]);
        if how == "--squash" {
            self.git_in("other", &["commit", "-qm", "A (#1)", "--allow-empty"]);
        }
        self.git_in("other", &["push", "-q", "origin", "main"]);
        if delete {
            self.git_in("other", &["push", "-q", "origin", ":A"]);
        }
    }

    /// transcript <session-id> <tokens>: a claude transcript whose last
    /// turn's context is <tokens>.
    fn transcript(&self, sid: &str, tokens: u64) {
        let dir = self.tmp.join("claude/projects/-repo");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(format!("{sid}.jsonl")),
            format!(
                "{{\"message\":{{\"role\":\"assistant\",\"usage\":{{\"input_tokens\":1,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":{},\"output_tokens\":0}}}},\"type\":\"assistant\"}}\n",
                tokens - 1
            ),
        )
        .unwrap();
    }
}

/// grep -q <re> over <s>, a line at a time.
fn grep(s: &str, re: &str) -> bool {
    regex::Regex::new(&format!("(?m){re}")).unwrap().is_match(s)
}

/// grep -qx <line> over <s>.
fn has_line(s: &str, line: &str) -> bool {
    s.lines().any(|l| l == line)
}

#[track_caller]
fn assert_starts(s: &str, prefix: &str) {
    assert!(s.starts_with(prefix), "{s:?}\ndoesn't start with {prefix:?}");
}

// ── describe ─────────────────────────────────────────────────────────────────

// @test "describe: the agent writes title and body, read-only, from the task and diff"
#[test]
fn describe_the_agent_writes_title_and_body_read_only_from_the_task_and_diff() {
    let sd = sd();
    sd.git(&["config", "branch.feat.sd-task", "add a third line"]);
    let r = sd.run_sep(&["describe"]);
    assert_eq!(r.status, 0);
    assert_eq!(jq(&["-r", ".title"], &r.output), "Add c");
    let prompt = sd.prompt();
    assert!(grep(&prompt, r"^\+c$"));
    assert!(grep(&prompt, "add a third line"));
    let args = sd.claude_args();
    assert!(has_line(&args, "--json-schema"));
    let lines: Vec<&str> = args.lines().collect();
    let i = lines.iter().rposition(|l| *l == "--allowedTools").unwrap();
    let allow = lines.get(i + 1).unwrap_or(&"");
    assert!(!allow.contains("Edit") && !allow.contains("Write"), "{allow}");
}

// The verbs enforce settings.branchBudget themselves, not only NEXT (#349):
// --force doesn't spend past it, and a step of unknown cost holds too.
#[test]
fn describe_refused_past_settings_branchbudget_even_with_force_349() {
    let sd = sd();
    sd.write_global(r#"{"settings": {"branchBudget": 2}}"#);
    sd.spent("feat", "implement", "2.5");
    let r = sd.run(&["describe", "--force"]);
    assert_eq!(r.status, 1);
    assert_has(&r.output, &["not running describe on feat: spent $2.50 on this branch, past settings.branchBudget ($2.00): raise it, or take over"]);
    assert!(!sd.fx_exists("claude.args"));
    sd.write_global(r#"{"settings": {"branchBudget": 3}}"#);
    sd.spent("feat", "review", "null");
    let r = sd.run(&["describe"]);
    assert_eq!(r.status, 1);
    assert_has(&r.output, &["not running describe on feat: can't tell what 1 step on this branch cost (review at 2026-10-30T00:00:00Z, session s1)"]);
    assert!(!sd.fx_exists("claude.args"));
    // No budget, no guard.
    sd.write_global(r#"{"settings": {"branchBudget": 0}}"#);
    sd.ok(&["describe"]);
}

// ship-parent's command is sd act ship <parent>: its describe answers to
// the parent's own budget, whichever branch NEXT came from (#148).
#[test]
fn ship_past_the_branchs_budget_is_refused_before_describe_or_push_148() {
    let sd = sd();
    sd.write_global(r#"{"settings": {"branchBudget": 2}}"#);
    sd.spent("feat", "implement", "2.5");
    let r = sd.run(&["ship"]);
    assert_eq!(r.status, 1);
    assert_has(&r.output, &["not running describe on feat: spent $2.50"]);
    assert!(!sd.fx_exists("claude.args"));
    assert_eq!(sd.remote("feat"), "");
}

// @test "describe: settings.describePrompt replaces the guidance; read-only stays"
#[test]
fn describe_settings_describeprompt_replaces_the_guidance_read_only_stays() {
    let sd = sd();
    sd.git(&["config", "branch.feat.sd-task", "add a third line"]);
    fs::write(sd.tmp.join("describe.md"), "Titles in French.\n").unwrap();
    sd.write_global(&format!(r#"{{"settings": {{"describePrompt": "{}"}}}}"#, sd.path("describe.md")));
    sd.ok(&["describe"]);
    let prompt = sd.prompt();
    assert_eq!(prompt.lines().next(), Some("Titles in French."));
    assert!(!grep(&prompt, "at most 70 characters"));
    assert!(grep(&prompt, "You are read-only here"));
    assert!(grep(&prompt, r"^\+c$"));
}

// @test "ship on the default branch: refused before describe, push or PR (#159)"
#[test]
fn ship_on_the_default_branch_refused_before_describe_push_or_pr_159() {
    let sd = sd();
    sd.git(&["switch", "-q", "main"]);
    sd.write("file.txt", "a\nb\nd\n");
    sd.git(&["commit", "-qam", "add d"]);
    let r = sd.run(&["ship"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["not shipping main: it's the default branch"]);
    assert!(!sd.fx_exists("claude.args"));
    assert_eq!(sd.calls(), "");
    assert_ne!(sd.remote("main"), sd.rev("main"));
}

// @test "ship: refused while a live step holds the running marker, --force or not, before describe, push or PR (#270)"
#[test]
fn ship_refused_while_a_live_step_holds_the_running_marker_force_or_not_before_describe_push_or_pr_270() {
    let sd = sd();
    let holder = Holder::new();
    let marker = running_marker(&sd.tmp.join("repo/.git"), "feat", "implement", holder.pid());
    for args in [&["ship"][..], &["ship", "--force"]] {
        let r = sd.run(args);
        assert_ne!(r.status, 0, "{args:?}");
        assert_has(&r.output, &[&step_refusal("implement", "feat", holder.pid(), "shipping")]);
    }
    assert!(!sd.fx_exists("claude.args"));
    assert_eq!(sd.calls(), "");
    assert_eq!(sd.remote("feat"), "");
    assert_eq!(sd.config("branch.feat.sheepdog"), "");
    assert!(marker.exists());
}

// @test "ship: the step's own child (RUNNING_HELD=<b>) ships under its marker, and a dead holder's marker doesn't refuse (#270)"
#[test]
fn ship_the_steps_own_child_ships_under_its_marker_and_a_dead_holders_marker_doesnt_refuse_270() {
    let holder = Holder::new();
    for (pid, env) in [(holder.pid(), &[("RUNNING_HELD", "feat")][..]), (dead_pid(), &[])] {
        let sd = sd();
        running_marker(&sd.tmp.join("repo/.git"), "feat", "ship", pid);
        let r = sd.run_env(env, &["ship"]);
        assert_eq!(r.status, 0, "{env:?}: {}", r.output);
        assert_eq!(sd.remote("feat"), sd.rev("feat"));
        assert_has(&sd.calls(), &["pr create"]);
    }
}

// @test "describe: stored for HEAD, and ship reuses it without calling the agent"
#[test]
fn describe_stored_for_head_and_ship_reuses_it_without_calling_the_agent() {
    let sd = sd();
    sd.ok(&["describe"]);
    sd.rm_claude_args();
    let r = sd.run(&["ship"]);
    assert_eq!(r.status, 0);
    assert!(!sd.fx_exists("claude.args"));
    assert_has(&sd.calls(), &["--title Add c"]);
}

#[test]
fn describe_mints_the_branchs_session_later_runs_fork_it_and_the_fork_is_the_branchs_69() {
    let sd = sd();
    sd.ok(&["describe"]);
    let sid = sd.config("branch.feat.sd-session");
    assert!(!sid.is_empty());
    assert!(has_line(&sd.claude_args(), "--session-id"));
    sd.ok(&["describe"]);
    let args = sd.claude_args();
    assert!(has_line(&args, "--resume") && has_line(&args, &sid) && has_line(&args, "--fork-session"));
    let fork = sd.config("branch.feat.sd-session");
    assert_ne!(fork, sid);
    assert!(has_line(&args, &fork));
}

// @test "describe: a session past settings.forkLimit tokens (100000) isn't forked; a new one starts (#121)"
#[test]
fn describe_a_session_past_settings_forklimit_tokens_100000_isnt_forked_a_new_one_starts_121() {
    let mut sd = sd();
    sd.env.push(("CLAUDE_CONFIG_DIR".to_string(), sd.path("claude")));
    sd.ok(&["describe"]);
    let sid = sd.config("branch.feat.sd-session");
    sd.transcript(&sid, 100000);
    sd.ok(&["describe"]);
    assert!(has_line(&sd.claude_args(), "--resume"));
    let sid = sd.config("branch.feat.sd-session");
    sd.transcript(&sid, 100001);
    let r = sd.run_sep(&["describe"]);
    assert_eq!(r.status, 0);
    assert_has(
        &r.stderr,
        &[&format!("session {sid} is 100001 tokens, past settings.forkLimit (100000): starting a new one")],
    );
    assert!(has_line(&sd.claude_args(), "--session-id"));
    let new = sd.config("branch.feat.sd-session");
    assert_ne!(new, sid);
    assert!(has_line(&sd.claude_args(), &new));
    // 0 is no limit
    sd.transcript(&new, 900000);
    sd.verb_ok("config", &["set", "--global", "--json", "settings.forkLimit", "0"]);
    sd.ok(&["describe"]);
    let args = sd.claude_args();
    assert!(has_line(&args, "--resume"));
    assert!(has_line(&args, &new));
}

// ── ship ─────────────────────────────────────────────────────────────────────

// @test "ship: no PR yet: describe, push, a draft PR on the parent, enroll"
#[test]
fn ship_no_pr_yet_describe_push_a_draft_pr_on_the_parent_enroll() {
    let sd = sd();
    let r = sd.run(&["ship"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.remote("feat"), sd.rev("HEAD"));
    let c = sd.calls();
    assert_starts(&c, "pr create --draft --base main --head feat --title Add c --body ");
    assert_has(&c, &["Refs #1"]);
    assert_has(&r.output, &["https://example/pr/9"]);
    assert_eq!(sd.config("branch.feat.sheepdog"), "true");
}

// @test "ship: held CI config is refused, even with --force, before describe or push; after sd act push it ships (#124)"
#[test]
fn ship_held_ci_config_is_refused_even_with_force_before_describe_or_push_after_sd_act_push_it_ships_124() {
    let sd = sd();
    sd.git(&["config", "branch.feat.sd-held", &format!("{}\n.github/workflows/test.yml", sd.rev("HEAD"))]);
    let r = sd.run(&["ship", "--force"]);
    assert_ne!(r.status, 0);
    assert_has(
        &r.output,
        &["not shipping feat: the agent changed CI config (.github/workflows/test.yml): review, then sd act push"],
    );
    assert_eq!(sd.remote("feat"), "");
    assert!(!sd.fx_exists("claude.args"));
    assert_eq!(sd.calls(), "");
    let r = sd.run(&["push"]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &["released the CI config hold on feat"]);
    let r = sd.run(&["ship"]);
    assert_eq!(r.status, 0);
}

#[test]
fn ship_refused_while_verify_is_red_at_head_before_describe_or_push_force_ships_anyway_325() {
    let sd = sd();
    sd.verify_red("HEAD");
    let r = sd.run(&["ship"]);
    assert_ne!(r.status, 0);
    assert_has(
        &r.output,
        &["not shipping feat: verify is red at HEAD (implement: make test); fix it, then sd act verify, or --force to ship anyway"],
    );
    assert_eq!(sd.remote("feat"), "");
    assert!(!sd.fx_exists("claude.args"));
    assert_eq!(sd.calls(), "");
    let r = sd.run(&["ship", "--force"]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &["shipping feat with verify red at HEAD (implement: make test): --force"]);
    assert_eq!(sd.remote("feat"), sd.rev("HEAD"));
}

#[test]
fn ship_a_verify_red_record_on_an_older_commit_isnt_a_refusal_325() {
    let sd = sd();
    sd.verify_red("HEAD~1");
    let r = sd.run(&["ship"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.remote("feat"), sd.rev("HEAD"));
}

// @test "ship: a hold whose commit is already on the remote (pushed some other way) is cleared, not a refusal (#124)"
#[test]
fn ship_a_hold_whose_commit_is_already_on_the_remote_pushed_some_other_way_is_cleared_not_a_refusal_124() {
    let sd = sd();
    sd.git(&["push", "-q", "origin", "feat"]);
    sd.git(&["config", "branch.feat.sd-held", &format!("{}\n.github/workflows/test.yml", sd.rev("HEAD"))]);
    let r = sd.run(&["ship"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.git_status(&["config", "--get", "branch.feat.sd-held"]), 1);
}

// @test "ship: a stacked branch opens its PR on the parent"
#[test]
fn ship_a_stacked_branch_opens_its_pr_on_the_parent() {
    let sd = sd();
    sd.pushed_a();
    sd.feat_on_a();
    let r = sd.run(&["ship"]);
    assert_eq!(r.status, 0);
    assert_starts(&sd.calls(), "pr create --draft --base A --head feat ");
    assert!(grep(&sd.prompt(), "Base: this PR is stacked on A"));
}

// @test "describe: told the PR targets main when nothing is stacked (#105)"
#[test]
fn describe_told_the_pr_targets_main_when_nothing_is_stacked_105() {
    let sd = sd();
    sd.ok(&["describe"]);
    assert!(grep(&sd.prompt(), r"Base: this PR targets main\."));
}

// @test "ship: a PR stacked on A with no sd-parent stays on A, and describe says so (#105)"
#[test]
fn ship_a_pr_stacked_on_a_with_no_sd_parent_stays_on_a_and_describe_says_so_105() {
    let sd = sd();
    sd.pushed_a();
    sd.git(&["switch", "-q", "feat"]);
    sd.git(&["rebase", "-q", "A"]);
    sd.git(&["push", "-q", "-u", "origin", "feat"]);
    sd.open_pr("feat", "A", 7);
    sd.gh_fixture(
        "A",
        &format!(
            r#"{{"data":{{"viewer":{{"login":"me"}},"repository":{{"defaultBranchRef":{{"name":"main"}},"pullRequests":{{"nodes":[{{"number": 3, "state": "OPEN", "baseRefName": "main", "headRefName": "A", "headRefOid": "{}", "isCrossRepository": false}}]}}}}}}}}"#,
            sd.rev("A")
        ),
    );
    let r = sd.run(&["ship"]);
    assert_eq!(r.status, 0);
    assert!(grep(&sd.prompt(), "Base: this PR is stacked on A"));
    assert_starts(&sd.calls(), "pr edit 7 ");
    assert_lacks(&sd.calls(), "--base");
}

// @test "ship: a parent that landed on main by hand, never on origin, isn't shipped again (#105)"
#[test]
fn ship_a_parent_that_landed_on_main_by_hand_never_on_origin_isnt_shipped_again_105() {
    let sd = sd();
    sd.branch_a("A");
    sd.git(&["switch", "-q", "main"]);
    sd.git(&["merge", "-q", "--ff-only", "A"]);
    sd.git(&["push", "-q", "origin", "main"]);
    sd.feat_on_a();
    let r = sd.run(&["ship"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["A has no commits beyond origin/main", "point sd-parent at main"]);
    assert_lacks(&r.output, "sd act ship A");
    assert_eq!(sd.calls(), "");
}

// @test "ship: a branch made from another's tip with no recorded parent: records it and opens its PR on it (#94)"
#[test]
fn ship_a_branch_made_from_anothers_tip_with_no_recorded_parent_records_it_and_opens_its_pr_on_it_94() {
    let sd = sd();
    sd.pushed_a();
    sd.git(&["switch", "-qc", "kid"]);
    sd.empty_commit("kid");
    let r = sd.run(&["ship"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.config("branch.kid.sd-parent"), "A");
    assert_starts(&sd.calls(), "pr create --draft --base A --head kid ");
}

// @test "ship: gh failing on a second lookup still opens an unrecorded stack's PR on its parent (#94)"
#[test]
fn ship_gh_failing_on_a_second_lookup_still_opens_an_unrecorded_stacks_pr_on_its_parent_94() {
    let sd = sd();
    sd.pushed_a();
    sd.git(&["switch", "-qc", "kid"]);
    sd.empty_commit("kid");
    sd.gh_touch("fail-kid-after-1");
    let r = sd.run(&["ship"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.config("branch.kid.sd-parent"), "A");
    assert_starts(&sd.calls(), "pr create --draft --base A --head kid ");
}

// @test "ship: an unrecorded stack git can't orient (the parent moved on) is refused before describe or push (#94)"
#[test]
fn ship_an_unrecorded_stack_git_cant_orient_the_parent_moved_on_is_refused_before_describe_or_push_94() {
    let sd = sd();
    sd.branch_a("A1");
    sd.git(&["switch", "-qc", "kid"]);
    sd.empty_commit("kid");
    sd.git(&["switch", "-q", "A"]);
    sd.empty_commit("A2");
    sd.git(&["push", "-q", "origin", "A"]);
    sd.git(&["switch", "-q", "kid"]);
    let r = sd.run(&["ship"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["git can't tell which, if either, was made from the other"]);
    assert_eq!(sd.config("branch.kid.sd-parent"), "");
    assert!(!sd.fx_exists("claude.args"));
    assert_eq!(sd.remote("kid"), "");
    assert_eq!(sd.calls(), "");
}

// @test "ship: a branch a scratch branch was made from still ships onto the default branch (#94)"
#[test]
fn ship_a_branch_a_scratch_branch_was_made_from_still_ships_onto_the_default_branch_94() {
    let sd = sd();
    sd.branch_a("A1");
    sd.git(&["switch", "-qc", "scratch"]);
    sd.empty_commit("s1");
    sd.git(&["switch", "-q", "A"]);
    sd.empty_commit("A2");
    let r = sd.run(&["ship"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.config("branch.A.sd-parent"), "");
    assert_starts(&sd.calls(), "pr create --draft --base main --head A ");
}

// @test "ship: a stale description (HEAD moved) is written again"
#[test]
fn ship_a_stale_description_head_moved_is_written_again() {
    let sd = sd();
    sd.ok(&["describe"]);
    sd.add_line("d", "add d");
    sd.result(r#"{"title": "Add c and d", "body": "both"}"#);
    let r = sd.run(&["ship"]);
    assert_eq!(r.status, 0);
    assert_has(&sd.calls(), &["--title Add c and d"]);
}

// @test "ship: the PR exists: push, then fix up title, body and base"
#[test]
fn ship_the_pr_exists_push_then_fix_up_title_body_and_base() {
    let sd = sd();
    sd.git(&["push", "-q", "-u", "origin", "feat"]);
    sd.add_line("d", "add d");
    sd.open_pr("feat", "main", 7);
    sd.pushed_a();
    sd.feat_on_a();
    let r = sd.run(&["ship"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.remote("feat"), sd.rev("HEAD"));
    let c = sd.calls();
    assert_starts(&c, "pr edit 7 --title Add c --body ");
    assert_has(&c, &["pr edit 7 --title Add c --body ", "--base A"]);
    assert_has(&r.output, &["#7"]);
}

// @test "ship: the PR exists and already has that base: no base edit"
#[test]
fn ship_the_pr_exists_and_already_has_that_base_no_base_edit() {
    let sd = sd();
    sd.git(&["push", "-q", "-u", "origin", "feat"]);
    sd.open_pr("feat", "main", 7);
    let r = sd.run(&["ship"]);
    assert_eq!(r.status, 0);
    assert_lacks(&sd.calls(), "--base");
}

// @test "ship: local review not covering HEAD is a warning, not a refusal"
#[test]
fn ship_local_review_not_covering_head_is_a_warning_not_a_refusal() {
    let sd = sd();
    let r = sd.run_sep(&["ship"]);
    assert_eq!(r.status, 0);
    assert_has(&r.stderr, &["local review"]);
}

// @test "ship: a parent that isn't on the remote is refused before describe or push"
#[test]
fn ship_a_parent_that_isnt_on_the_remote_is_refused_before_describe_or_push() {
    let sd = sd();
    sd.branch_a("A"); // never pushed
    sd.git(&["switch", "-q", "feat"]);
    sd.git(&["config", "branch.feat.sd-parent", "A"]);
    let r = sd.run(&["ship"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["A isn't on origin", "sd act ship A"]);
    assert!(!sd.fx_exists("claude.args"));
    assert_eq!(sd.remote("feat"), "");
    assert_eq!(sd.calls(), "");
}

// @test "ship: a parent that has merged and is gone from the remote: restack first, not ship it"
#[test]
fn ship_a_parent_that_has_merged_and_is_gone_from_the_remote_restack_first_not_ship_it() {
    let sd = sd();
    sd.pushed_a();
    let a_head = sd.rev("A");
    sd.git(&["switch", "-q", "feat"]);
    sd.git(&["config", "branch.feat.sd-parent", "A"]);
    sd.other_merges_a("--squash", true);
    sd.pr_a("MERGED", &a_head);
    let r = sd.run(&["ship"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["A has merged", "sd act restack"]);
    assert!(!sd.fx_exists("claude.args"));
    assert_eq!(sd.remote("feat"), "");
    assert_eq!(sd.calls(), "");
}

// @test "ship: a parent that has merged but is still on the remote: restack first, nothing pushed"
#[test]
fn ship_a_parent_that_has_merged_but_is_still_on_the_remote_restack_first_nothing_pushed() {
    let sd = sd();
    sd.pushed_a();
    let a_head = sd.rev("A");
    sd.git(&["switch", "-q", "feat"]);
    sd.git(&["config", "branch.feat.sd-parent", "A"]);
    sd.other_merges_a("--squash", false);
    sd.pr_a("MERGED", &a_head);
    let r = sd.run(&["ship"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["A has merged", "sd act restack"]);
    assert!(!sd.fx_exists("claude.args"));
    assert_eq!(sd.remote("feat"), "");
    assert_eq!(sd.calls(), "");
}

// @test "ship: a parent closed without merging is refused: decide what the branch targets"
#[test]
fn ship_a_parent_closed_without_merging_is_refused_decide_what_the_branch_targets() {
    let sd = sd();
    sd.pushed_a();
    let a_head = sd.rev("A");
    sd.git(&["switch", "-q", "feat"]);
    sd.git(&["config", "branch.feat.sd-parent", "A"]);
    sd.pr_a("CLOSED", &a_head);
    let r = sd.run(&["ship"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["A (#1) was closed without merging", "sd-parent"]);
    assert!(!sd.fx_exists("claude.args"));
    assert_eq!(sd.remote("feat"), "");
    assert_eq!(sd.calls(), "");
}

// @test "ship: a parent merged by fast-forward, so the branch is already on main: ships onto main and rewrites sd-parent"
#[test]
fn ship_a_parent_merged_by_fast_forward_so_the_branch_is_already_on_main_ships_onto_main_and_rewrites_sd_parent() {
    let sd = sd();
    sd.pushed_a();
    let a_head = sd.rev("A");
    sd.feat_on_a();
    sd.other_merges_a("--ff-only", true);
    sd.pr_a("MERGED", &a_head);
    let r = sd.run(&["ship"]);
    assert_eq!(r.status, 0);
    assert_starts(&sd.calls(), "pr create --draft --base main --head feat ");
    assert_eq!(sd.config("branch.feat.sd-parent"), "main");
    assert!(grep(&sd.prompt(), "Base: A has merged into main, so this PR targets main"));
}

// @test "ship: a description written while stacked is written again once the parent merged, HEAD unchanged (#105)"
#[test]
fn ship_a_description_written_while_stacked_is_written_again_once_the_parent_merged_head_unchanged_105() {
    let sd = sd();
    sd.pushed_a();
    let a_head = sd.rev("A");
    sd.feat_on_a();
    sd.ok(&["describe"]);
    assert!(grep(&sd.prompt(), "Base: this PR is stacked on A"));
    sd.other_merges_a("--ff-only", true);
    sd.pr_a("MERGED", &a_head);
    sd.result(r#"{"title": "Add c on main", "body": "A has landed."}"#);
    let r = sd.run(&["ship"]);
    assert_eq!(r.status, 0);
    assert!(grep(&sd.prompt(), "Base: A has merged into main"));
    assert_starts(&sd.calls(), "pr create --draft --base main --head feat --title Add c on main ");
    // sd-parent is main now; the PR's target hasn't changed, so the body stands
    sd.rm_claude_args();
    sd.open_pr("feat", "main", 9);
    let r = sd.run(&["ship"]);
    assert_eq!(r.status, 0);
    assert!(!sd.fx_exists("claude.args"));
}

// @test "describe: a parent only here says to ship it first, not an empty reason (#105)"
#[test]
fn describe_a_parent_only_here_says_to_ship_it_first_not_an_empty_reason_105() {
    let sd = sd();
    sd.branch_a("A");
    sd.feat_on_a();
    let r = sd.run(&["describe"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["A isn't on origin yet: ship it first (sd act ship A)"]);
}

// @test "ship: a parent whose PR can't be looked up is refused, never shipped onto"
#[test]
fn ship_a_parent_whose_pr_cant_be_looked_up_is_refused_never_shipped_onto() {
    let sd = sd();
    sd.pushed_a();
    sd.git(&["switch", "-q", "feat"]);
    sd.git(&["config", "branch.feat.sd-parent", "A"]);
    sd.gh_touch("fail-A");
    let r = sd.run(&["ship"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["can't tell whether A merged"]);
    assert!(!sd.fx_exists("claude.args"));
    assert_eq!(sd.remote("feat"), "");
    assert_eq!(sd.calls(), "");
}

// @test "ship: a merged or closed PR is refused"
#[test]
fn ship_a_merged_or_closed_pr_is_refused() {
    let sd = sd();
    sd.open_pr("feat", "main", 7);
    let f = sd.tmp.join("gh/feat.json");
    let j = jq(&[r#".data.repository.pullRequests.nodes[0].state = "MERGED""#], &fs::read_to_string(&f).unwrap());
    fs::write(&f, j + "\n").unwrap();
    let r = sd.run(&["ship"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["merged"]);
    assert_eq!(sd.calls(), "");
}

// @test "ship: commits on the remote this copy never had: refused before any PR call"
#[test]
fn ship_commits_on_the_remote_this_copy_never_had_refused_before_any_pr_call() {
    let sd = sd();
    sd.git(&["push", "-q", "-u", "origin", "feat"]);
    sd.git_in("other", &["fetch", "-q", "origin"]);
    sd.git_in("other", &["checkout", "-q", "-B", "feat", "origin/feat"]);
    fs::write(sd.tmp.join("other/x.txt"), "x\n").unwrap();
    sd.git_in("other", &["add", "x.txt"]);
    sd.git_in("other", &["commit", "-qm", "theirs"]);
    sd.git_in("other", &["push", "-q", "origin", "feat"]);
    sd.add_line("d", "add d");
    let r = sd.run(&["ship"]);
    assert_ne!(r.status, 0);
    assert_eq!(sd.calls(), "");
}

// @test "ship: the describe step failing stops before the push"
#[test]
fn ship_the_describe_step_failing_stops_before_the_push() {
    let sd = sd();
    fs::write(sd.tmp.join("fx/fail"), "").unwrap();
    let r = sd.run(&["ship"]);
    assert_ne!(r.status, 0);
    assert_eq!(sd.remote("feat"), "");
    assert_eq!(sd.calls(), "");
}

// @test "ship: HEAD moving during describe is refused; nothing pushed"
#[test]
fn ship_head_moving_during_describe_is_refused_nothing_pushed() {
    let sd = sd();
    sd.script(
        &sd.tmp.join("fx/agent"),
        "#!/usr/bin/env bash\necho sneaky >> file.txt && git commit -qam sneaky\n",
    );
    let r = sd.run(&["ship"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["HEAD moved"]);
    assert_eq!(sd.remote("feat"), "");
}

// @test "ship: uncommitted changes are refused"
#[test]
fn ship_uncommitted_changes_are_refused() {
    let sd = sd();
    sd.write("file.txt", "a\nb\nc\nd\n");
    let r = sd.run(&["ship"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["uncommitted"]);
}

// @test "ship: gh pr create failing is an error after the push"
#[test]
fn ship_gh_pr_create_failing_is_an_error_after_the_push() {
    let sd = sd();
    sd.gh_touch("fail-create");
    let r = sd.run(&["ship"]);
    assert_ne!(r.status, 0);
    assert_eq!(sd.remote("feat"), sd.rev("HEAD"));
}

// @test "ship: stacked on an open parent it isn't built on, no pin: refused, says how to pin (#97)"
#[test]
fn ship_stacked_on_an_open_parent_it_isnt_built_on_no_pin_refused_says_how_to_pin_97() {
    let sd = sd();
    sd.pushed_a();
    sd.git(&["switch", "-q", "feat"]);
    sd.git(&["config", "branch.feat.sd-parent", "A"]);
    let r = sd.run(&["ship"]);
    assert_ne!(r.status, 0);
    assert_has(
        &r.output,
        &["can't find feat's base: no base for feat on A: no base pin", "git update-ref refs/sheepdog/feat/base"],
    );
    assert_eq!(sd.calls(), "");
}

// ── ship: the upstream (#39) ─────────────────────────────────────────────────

// @test "ship: twice on a published branch tracking origin/main: tracks origin/feat, nothing else changes"
#[test]
fn ship_twice_on_a_published_branch_tracking_origin_main_tracks_origin_feat_nothing_else_changes() {
    let sd = sd();
    sd.pushed_a();
    sd.feat_on_a();
    sd.git(&["push", "-q", "origin", "feat"]);
    sd.git(&["branch", "-q", "--set-upstream-to=origin/main", "feat"]);
    sd.open_pr("feat", "A", 7);
    sd.ok(&["describe"]);
    sd.rm_claude_args();
    let head = sd.rev("feat");
    for _ in 1..=2 {
        let r = sd.run(&["ship"]);
        assert_eq!(r.status, 0);
        assert_has(&r.output, &["nothing to push on feat"]);
        assert_eq!(sd.tracks("feat"), "origin/feat");
    }
    assert!(!sd.fx_exists("claude.args"));
    assert_eq!(sd.rev("feat"), head);
    assert_eq!(sd.remote("feat"), head);
    assert_eq!(sd.config("branch.feat.sd-parent"), "A");
    let calls = sd.calls();
    assert_eq!(calls.lines().filter(|l| l.starts_with("pr create")).count(), 0);
    assert_eq!(calls.lines().filter(|l| l.starts_with("pr edit 7 ")).count(), 2);
    assert_lacks(&calls, "--base");
}

// @test "ship: retried after gh pr create failed: the PR opens, tracking origin/feat"
#[test]
fn ship_retried_after_gh_pr_create_failed_the_pr_opens_tracking_origin_feat() {
    let sd = sd();
    sd.git(&["branch", "-q", "--set-upstream-to=origin/main", "feat"]);
    sd.gh_touch("fail-create");
    let r = sd.run(&["ship"]);
    assert_ne!(r.status, 0);
    let head = sd.rev("feat");
    assert_eq!(sd.remote("feat"), head);
    fs::remove_file(sd.tmp.join("gh/fail-create")).unwrap();
    sd.git(&["branch", "-q", "--set-upstream-to=origin/main", "feat"]); // as an sd from before #39 left it
    let r = sd.run(&["ship"]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &["nothing to push on feat"]);
    assert_has(&r.output, &["https://example/pr/9"]);
    assert_eq!(sd.tracks("feat"), "origin/feat");
    assert_eq!(sd.rev("feat"), head);
}

// The interlocks sd act's declaration gives ship and describe (#341). ship:
// while attached, refused before describe or the push, unless --force.
// describe writes nothing of the branch, so only its agent holds the marker:
// while another step holds it, no agent runs.
#[test]
fn ship_refused_while_attached_before_describe_or_push_341() {
    let sd = sd();
    let attach = Holder::new();
    sd.git(&["config", "branch.feat.sd-lock", &lock_by(attach.pid())]);
    let r = sd.run(&["ship"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["--force"]);
    assert!(!sd.fx_exists("claude.args"));
    assert_eq!(sd.remote("feat"), "");
}

#[test]
fn describe_runs_no_agent_while_another_step_holds_the_marker_341() {
    let sd = sd();
    let holder = Holder::new();
    let marker = running_marker(&sd.tmp.join("repo/.git"), "feat", "implement", holder.pid());
    let r = sd.run(&["describe"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &[&format!("implement is running on feat (pid {}", holder.pid())]);
    assert!(!sd.fx_exists("claude.args"));
    assert!(marker.exists());
}
