// Black-box tests of sd act start (A0: a worktree beside the main worktree,
// named after the branch with slashes as dashes, set up by the repo's hook,
// with the task, parent and base recorded and the branch enrolled; then
// implement) and sd act implement (J0: the agent edits in the branch's
// session, the pipeline commits and runs verify), #203. The built binary, run
// as sd-act through a link (tests/common). Moved from
// libexec/sd-act-start.bats; each test keeps the bats test's title above it.
//
// The worktree half duplicates agent-rig's wt-new (#14): same branch
// resolution, same hook locations, stdout exactly the path. Placement differs
// since #85: beside the main worktree, never beside a linked one.
//
// Real git (a bare origin, our clone at repo/, other/ for pushes that aren't
// ours). sd state is the real one, in-process. gh is a stub with no PRs. claude edits
// through $FX/agent and prints $FX/result.json as structured output.

mod common;

use common::{assert_has, assert_lacks, assert_match, chomp, jq, Sd};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

const GH: &str = r#"#!/usr/bin/env bash
printf '%s\n' "$*" >> "$GH/calls"
echo '{"data":{"viewer":{"login":"me"},"repository":{"defaultBranchRef":{"name":"main"},"pullRequests":{"nodes":[]}}}}'
"#;

const CLAUDE: &str = r#"#!/usr/bin/env bash
printf '%s\n' "$@" > "$FX/claude.args"
pwd > "$FX/claude.cwd"
args=("$@") prompt= i=0
while (( i < $# )); do
  case ${args[i]} in
    --allowedTools|--disallowedTools)
      i=$((i + 1)); while (( i < $# )) && [[ ${args[i]} != -* ]]; do i=$((i + 1)); done; continue ;;
    --session-id|--resume|--output-format|--json-schema|--permission-mode|--model|--effort|--max-budget-usd) i=$((i + 2)); continue ;;
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

const PI: &str = r#"#!/usr/bin/env bash
printf '%s\n' "$@" > "$FX/pi.args"
[[ -x $FX/agent ]] && "$FX/agent"
printf '```json\n%s\n```\n' "$(cat "$FX/result.json")"
"#;

/// An agent that commits a CI config file itself.
const AGENT_COMMITS_CI: &str = "#!/usr/bin/env bash\nmkdir -p .github/workflows && echo x > .github/workflows/ci.yml && git add -A && git commit -qm \"agent made it\"\n";

/// The bats setup: origin.git and its clones repo (cwd) and other, file.txt
/// on main; the stub gh, the fake claude with an agent that adds a line, and
/// its result.
fn sd() -> Sd {
    let mut sd = Sd::for_verb("act");
    sd.env = vec![
        ("SHEEPDOG_STATE_DIR".into(), sd.path("state")),
        ("GH".into(), sd.path("gh")),
        ("FX".into(), sd.path("fx")),
    ];
    for d in ["gh", "fx"] {
        fs::create_dir(sd.tmp.join(d)).unwrap();
    }
    sd.fake("gh", GH);
    sd.fake("claude", CLAUDE);
    sd.agent("#!/usr/bin/env bash\necho c >> file.txt\n");
    sd.put("result.json", r#"{"outcome": "done", "commit_message": "Add c", "summary": "Added the third line."}"#);

    sd.origin(&["repo"]);
    sd.cd("repo");
    sd.write("file.txt", "a\nb\n");
    sd.git(&["add", "file.txt"]);
    sd.git(&["commit", "-qm", "init"]);
    sd.git(&["push", "-q", "origin", "main"]);
    sd.git(&["clone", "-q", &sd.path("origin.git"), &sd.path("other")]);
    sd
}

impl Sd {
    /// $FX/<f> := <s>, newline-terminated.
    fn put(&self, f: &str, s: &str) {
        fs::write(self.tmp.join("fx").join(f), format!("{s}\n")).unwrap();
    }

    /// `$(cat "$FX/<f>")`, or "" when it's missing.
    fn fx(&self, f: &str) -> String {
        chomp(&fs::read_to_string(self.tmp.join("fx").join(f)).unwrap_or_default())
    }

    fn ran(&self, f: &str) -> bool {
        self.tmp.join("fx").join(f).exists()
    }

    fn rm(&self, f: &str) {
        let _ = fs::remove_file(self.tmp.join("fx").join(f));
    }

    /// `$(cat "$TMP/<f>")`
    fn cat(&self, f: &str) -> String {
        chomp(&fs::read_to_string(self.tmp.join(f)).unwrap_or_default())
    }

    /// $FX/agent := <script>, what the agent does.
    fn agent(&self, script: &str) {
        self.script(&self.tmp.join("fx/agent"), script);
    }

    /// .git/worktree-setup := <script>, executable.
    fn hook(&self, script: &str) {
        self.script(&self.cwd.join(".git/worktree-setup"), script);
    }

    /// other_push <branch> <message>: a commit on origin that our clone hasn't fetched.
    fn other_push(&self, b: &str, msg: &str) {
        if self.git_status(&["-C", &self.path("other"), "switch", "-q", b]) != 0 {
            self.git_in("other", &["switch", "-qc", b]);
        }
        self.git_in("other", &["commit", "-q", "--allow-empty", "-m", msg]);
        self.git_in("other", &["push", "-q", "origin", b]);
    }

    fn sha(&self, r: &str) -> String {
        self.git(&["rev-parse", r])
    }

    fn subject(&self) -> String {
        self.git(&["log", "-1", "--format=%s"])
    }

    fn porcelain(&self) -> String {
        self.git(&["status", "--porcelain"])
    }

    /// `tail -1 file.txt`
    fn last_line(&self) -> String {
        let f = fs::read_to_string(self.cwd.join("file.txt")).unwrap();
        f.lines().last().unwrap_or_default().to_string()
    }

    /// started [<task>]: a worktree for feat-x with a task, no agent run yet.
    fn started(&mut self, task: &str) {
        self.ok(&["start", "--no-implement", "feat-x", task]);
        self.cd("feat-x");
    }

    fn prompt(&self) -> String {
        self.fx("claude.prompt")
    }

    /// `head -1 "$FX/claude.prompt"`
    fn prompt_head(&self) -> String {
        self.prompt().lines().next().unwrap_or_default().to_string()
    }

    /// `grep -A1 -x -- <flag> "$FX/<f>" | tail -1`: the word after the last <flag>.
    fn after(&self, f: &str, flag: &str) -> String {
        let args = self.fx(f);
        let lines: Vec<&str> = args.lines().collect();
        let i = lines.iter().rposition(|l| *l == flag).unwrap_or_else(|| panic!("no {flag} in {args}"));
        lines.get(i + 1).unwrap_or(&"").to_string()
    }

    fn allowed(&self) -> String {
        self.after("claude.args", "--allowedTools")
    }

    /// `grep -qx -- <word> "$FX/claude.args"`
    fn has_word(&self, word: &str) -> bool {
        self.fx("claude.args").lines().any(|l| l == word)
    }

    /// git update-ref refs/sheepdog/feat-x/findings to a blob of <rec>.
    fn put_findings(&self, rec: &str) {
        let blob = self.git_with(&[], Some(&format!("{rec}\n")), &["hash-object", "-w", "--stdin"]);
        self.git(&["update-ref", "refs/sheepdog/feat-x/findings", &blob]);
    }

    /// The findings record for HEAD (head and base both), round 1, with <findings>.
    fn findings_on_head(&self, findings: &str) {
        let rec = jq(&["-nc", "--arg", "h", &self.sha("HEAD"), &format!("{{head: $h, base: $h, round: 1, findings: {findings}}}")], "");
        self.put_findings(&rec);
    }

    /// findings_at <rev> <round>: sd act review's record of one blocking
    /// finding on <rev>, and the agent's answer: fixed.
    fn findings_at(&self, rev: &str, round: &str) {
        let rec = jq(
            &[
                "-nc", "--arg", "h", &self.sha(rev), "--argjson", "r", round,
                r#"{head: $h, base: $h, round: $r,
          findings: [{file: "file.txt", line: 2, severity: "error", body: "b should be B"}]}"#,
            ],
            "",
        );
        self.put_findings(&rec);
        self.answers(&[r#"{"id": "1", "action": "fixed", "reply": "B now"}"#]);
    }

    /// answers <item>...: the agent's result for a fix round, one item per finding.
    fn answers(&self, items: &[&str]) {
        let mut args = vec![
            "-nc",
            r#"{outcome: "done", commit_message: "Add c", summary: "Fixed what was right.", items: [$ARGS.positional[] | fromjson]}"#,
            "--args",
        ];
        args.extend(items);
        self.put("result.json", &jq(&args, ""));
    }

    /// two blocking findings on HEAD, round 1
    fn two_findings(&self) {
        self.findings_on_head(
            r#"[
          {file: "file.txt", line: 1, severity: "warning", body: "a should be A"},
          {file: "file.txt", line: 2, severity: "error", body: "b should be B"}]"#,
        );
    }

    fn findings_rec(&self, expr: &str) -> String {
        jq(&["-c", expr], &self.git(&["cat-file", "blob", "refs/sheepdog/feat-x/findings"]))
    }

    /// `git cat-file blob refs/sheepdog/feat-x/question | jq -r <expr>`
    fn question(&self, expr: &str) -> String {
        jq(&["-r", expr], &self.git(&["cat-file", "blob", "refs/sheepdog/feat-x/question"]))
    }

    /// `"$SD_STATE" --json | jq -c <expr>`
    fn state(&self, expr: &str) -> String {
        jq(&["-c", expr], &self.verb_ok("state", &["--json"]))
    }

    /// git config branch.feat-x.sd-held
    fn held(&self) -> String {
        self.git(&["config", "--get", "branch.feat-x.sd-held"])
    }
}

/// echo <line> >> <f>
fn append(f: &Path, line: &str) {
    writeln!(OpenOptions::new().append(true).open(f).unwrap(), "{line}").unwrap();
}

/// `grep -q '^<prefix>' <text>`
fn has_line(text: &str, prefix: &str) -> bool {
    text.lines().any(|l| l.starts_with(prefix))
}

// ── start: the worktree ──────────────────────────────────────────────────────

// @test "start: no name is usage, nonzero"
#[test]
fn start_no_name_is_usage_nonzero() {
    let sd = sd();
    let r = sd.run(&["start"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["sd act start"]);
    assert!(!sd.tmp.join("feat-x").exists());
}

// @test "start: a worktree beside the main one, on a new branch of the same name; stdout is only its path"
#[test]
fn start_a_worktree_beside_the_main_one_on_a_new_branch_of_the_same_name_stdout_is_only_its_path() {
    let sd = sd();
    let r = sd.run_sep(&["start", "--no-implement", "feat-x"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, sd.path("feat-x"));
    assert_eq!(sd.git_in("feat-x", &["branch", "--show-current"]), "feat-x");
}

// git worktree add's own output reaches stderr in git's order, as bash's
// `git worktree add … >&2` streamed it: "Preparing worktree" before "HEAD is
// now at".
#[test]
fn start_passes_git_worktree_adds_output_through_to_stderr_in_order() {
    let sd = sd();
    let r = sd.run_sep(&["start", "--no-implement", "feat-x"]);
    assert_eq!(r.status, 0);
    let (prep, head) = (r.stderr.find("Preparing worktree"), r.stderr.find("HEAD is now at"));
    assert!(prep.is_some() && head.is_some() && prep < head, "{}", r.stderr);
}

// @test "start: placed beside the main worktree, from a subdirectory too"
#[test]
fn start_placed_beside_the_main_worktree_from_a_subdirectory_too() {
    let mut sd = sd();
    fs::create_dir_all(sd.cwd.join("sub/deeper")).unwrap();
    sd.cd("repo/sub/deeper");
    let r = sd.run_sep(&["start", "--no-implement", "feat-x"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, sd.path("feat-x"));
}

// @test "start: a new branch starts from the fetched origin default, not a stale local main"
#[test]
fn start_a_new_branch_starts_from_the_fetched_origin_default_not_a_stale_local_main() {
    let sd = sd();
    sd.other_push("main", "moved on");
    let r = sd.run_sep(&["start", "--no-implement", "feat-x"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.sha("feat-x"), sd.sha("origin/main"));
    assert_ne!(sd.sha("feat-x"), sd.sha("main"));
}

// @test "start: a new branch has no upstream until it's pushed"
#[test]
fn start_a_new_branch_has_no_upstream_until_its_pushed() {
    let sd = sd();
    sd.ok(&["start", "--no-implement", "feat-x"]);
    assert_ne!(sd.git_status(&["config", "--get", "branch.feat-x.merge"]), 0);
}

// @test "start: origin/HEAD names the default branch when it's set"
#[test]
fn start_origin_head_names_the_default_branch_when_its_set() {
    let sd = sd();
    sd.other_push("trunk", "trunk only");
    sd.git(&["fetch", "-q", "origin"]);
    sd.git(&["remote", "set-head", "origin", "trunk"]);
    sd.ok(&["start", "--no-implement", "feat-x"]);
    assert_eq!(sd.sha("feat-x"), sd.sha("origin/trunk"));
}

// @test "start: an existing local branch gets the worktree, as it is"
#[test]
fn start_an_existing_local_branch_gets_the_worktree_as_it_is() {
    let sd = sd();
    sd.git(&["commit", "-q", "--allow-empty", "-m", "local only"]);
    sd.git(&["branch", "feat-x"]);
    let r = sd.run_sep(&["start", "--no-implement", "feat-x"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.git_in("feat-x", &["rev-parse", "HEAD"]), sd.sha("main"));
    assert!(sd.git(&["for-each-ref", "refs/sheepdog/feat-x/base"]).is_empty());
}

// @test "start: a branch only on origin gets a local branch tracking it"
#[test]
fn start_a_branch_only_on_origin_gets_a_local_branch_tracking_it() {
    let sd = sd();
    sd.other_push("feat-x", "theirs");
    let r = sd.run_sep(&["start", "--no-implement", "feat-x"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.sha("feat-x"), sd.sha("origin/feat-x"));
    assert_eq!(sd.git(&["config", "branch.feat-x.merge"]), "refs/heads/feat-x");
}

// @test "start: refuses when the destination already exists, and creates no branch"
#[test]
fn start_refuses_when_the_destination_already_exists_and_creates_no_branch() {
    let sd = sd();
    fs::create_dir(sd.tmp.join("feat-x")).unwrap();
    let r = sd.run(&["start", "--no-implement", "feat-x"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &[&format!("{} already exists", sd.path("feat-x"))]);
    assert_ne!(sd.git_status(&["rev-parse", "-q", "--verify", "refs/heads/feat-x"]), 0);
}

// @test "start: refuses a name git won't take as a branch"
#[test]
fn start_refuses_a_name_git_wont_take_as_a_branch() {
    let sd = sd();
    let r = sd.run(&["start", "--no-implement", "bad..name"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["not a valid branch name"]);
}

// @test "start: git's chatter goes to stderr, never stdout"
#[test]
fn start_gits_chatter_goes_to_stderr_never_stdout() {
    let sd = sd();
    let r = sd.run_sep(&["start", "--no-implement", "feat-x"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output.lines().count(), 1);
}

// ── start: --parent ──────────────────────────────────────────────────────────

// @test "start --parent: starts from the local parent, records sd-parent, pins base there"
#[test]
fn start_parent_starts_from_the_local_parent_records_sd_parent_pins_base_there() {
    let sd = sd();
    sd.git(&["switch", "-qc", "A"]);
    sd.git(&["commit", "-q", "--allow-empty", "-m", "A"]);
    sd.git(&["switch", "-q", "main"]);
    let r = sd.run_sep(&["start", "--no-implement", "--parent", "A", "feat-x"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.sha("feat-x"), sd.sha("A"));
    assert_eq!(sd.git(&["config", "branch.feat-x.sd-parent"]), "A");
    assert_eq!(sd.sha("refs/sheepdog/feat-x/base"), sd.sha("A"));
}

// @test "start --parent: a parent only on origin is used from there"
#[test]
fn start_parent_a_parent_only_on_origin_is_used_from_there() {
    let sd = sd();
    sd.other_push("A", "A, theirs");
    let r = sd.run_sep(&["start", "--no-implement", "--parent", "A", "feat-x"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.sha("feat-x"), sd.sha("origin/A"));
}

// @test "start --parent: a parent that doesn't exist is refused before any worktree"
#[test]
fn start_parent_a_parent_that_doesnt_exist_is_refused_before_any_worktree() {
    let sd = sd();
    let r = sd.run(&["start", "--no-implement", "--parent", "nope", "feat-x"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["no such parent branch: nope"]);
    assert!(!sd.tmp.join("feat-x").exists());
}

// ── start: bookkeeping ───────────────────────────────────────────────────────

// @test "start: records the task, pins base at the start point, enrolls the branch and the repo"
#[test]
fn start_records_the_task_pins_base_at_the_start_point_enrolls_the_branch_and_the_repo() {
    let sd = sd();
    let r = sd.run_sep(&["start", "--no-implement", "feat-x", "add a third line"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.git(&["config", "branch.feat-x.sd-task"]), "add a third line");
    assert_eq!(sd.sha("refs/sheepdog/feat-x/base"), sd.sha("origin/main"));
    assert_eq!(sd.git(&["config", "branch.feat-x.sheepdog"]), "true");
    assert_eq!(sd.git(&["config", "--global", "--get-all", "sheepdog.repo"]), sd.path("repo"));
}

// @test "start: no session is minted until a step runs"
#[test]
fn start_no_session_is_minted_until_a_step_runs() {
    let sd = sd();
    sd.ok(&["start", "--no-implement", "feat-x", "a task"]);
    assert_ne!(sd.git_status(&["config", "--get", "branch.feat-x.sd-session"]), 0);
}

// @test "start: without --no-implement the task is required"
#[test]
fn start_without_no_implement_the_task_is_required() {
    let sd = sd();
    let r = sd.run(&["start", "feat-x"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["no task"]);
    assert!(!sd.tmp.join("feat-x").exists());
}

// ── start: the setup hook (wt-new's two places) ──────────────────────────────

// @test "start: runs \$GIT_COMMON_DIR/worktree-setup in the new worktree with WT_SRC, WT_DST, WT_BRANCH"
#[test]
fn start_runs_git_common_dir_worktree_setup_in_the_new_worktree_with_wt_src_wt_dst_wt_branch() {
    let sd = sd();
    sd.hook(&format!(
        "#!/usr/bin/env bash\necho \"$PWD|$WT_SRC|$WT_DST|$WT_BRANCH\" > \"{}\"\necho hook-stdout\n",
        sd.path("hook.out")
    ));
    let r = sd.run_sep(&["start", "--no-implement", "feat-x"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, sd.path("feat-x"));
    let (dst, src) = (sd.path("feat-x"), sd.path("repo"));
    assert_eq!(sd.cat("hook.out"), format!("{dst}|{src}|{dst}|feat-x"));
    assert_has(&r.stderr, &["hook-stdout"]);
}

// @test "start: a worktree-setup that isn't executable is skipped"
#[test]
fn start_a_worktree_setup_that_isnt_executable_is_skipped() {
    let sd = sd();
    fs::write(sd.cwd.join(".git/worktree-setup"), format!("#!/usr/bin/env bash\ntouch \"{}\"\n", sd.path("ran"))).unwrap();
    let r = sd.run_sep(&["start", "--no-implement", "feat-x"]);
    assert_eq!(r.status, 0);
    assert!(!sd.tmp.join("ran").exists());
}

// @test "start: falls back to the wt.setup config command"
#[test]
fn start_falls_back_to_the_wt_setup_config_command() {
    let sd = sd();
    sd.git(&["config", "wt.setup", r#"echo "$WT_BRANCH" > "$WT_DST/../wt.out""#]);
    let r = sd.run_sep(&["start", "--no-implement", "feat-x"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.cat("wt.out"), "feat-x");
}

// @test "start: the hook script wins over wt.setup"
#[test]
fn start_the_hook_script_wins_over_wt_setup() {
    let sd = sd();
    sd.hook(&format!("#!/usr/bin/env bash\ntouch \"{}\"\n", sd.path("script")));
    sd.git(&["config", "wt.setup", &format!("touch '{}'", sd.path("config"))]);
    sd.ok(&["start", "--no-implement", "feat-x"]);
    assert!(sd.tmp.join("script").exists());
    assert!(!sd.tmp.join("config").exists());
}

// @test "start: a failing hook keeps the worktree, prints its path, exits nonzero, and doesn't implement"
#[test]
fn start_a_failing_hook_keeps_the_worktree_prints_its_path_exits_nonzero_and_doesnt_implement() {
    let sd = sd();
    sd.hook("#!/usr/bin/env bash\necho broke >&2\nexit 3\n");
    let r = sd.run_sep(&["start", "feat-x", "a task"]);
    assert_ne!(r.status, 0);
    assert_eq!(r.output, sd.path("feat-x"));
    assert!(sd.tmp.join("feat-x").is_dir());
    assert_has(&r.stderr, &["broke"]);
    assert_has(&r.stderr, &["sd act implement feat-x"]);
    assert!(!sd.ran("claude.args"));
}

// @test "start: from a linked worktree, the new one goes beside the main worktree, not beside the linked one (#85)"
#[test]
fn start_from_a_linked_worktree_the_new_one_goes_beside_the_main_worktree_not_beside_the_linked_one_85() {
    let mut sd = sd();
    sd.hook(&format!("#!/usr/bin/env bash\necho \"$WT_SRC\" > \"{}\"\n", sd.path("src.out")));
    fs::create_dir(sd.tmp.join("wts")).unwrap();
    sd.git(&["worktree", "add", "-q", "-b", "one", &sd.path("wts/one")]);
    sd.cd("wts/one");
    let r = sd.run_sep(&["start", "--no-implement", "feat-x"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, sd.path("feat-x"));
    assert_eq!(sd.cat("src.out"), sd.path("wts/one"));
}

// @test "start: a slash in the branch name becomes a dash in the directory name, every generation (#85)"
#[test]
fn start_a_slash_in_the_branch_name_becomes_a_dash_in_the_directory_name_every_generation_85() {
    let mut sd = sd();
    let r = sd.run_sep(&["start", "--no-implement", "12/stack-parent"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, sd.path("12-stack-parent"));
    assert_eq!(sd.git_in("12-stack-parent", &["branch", "--show-current"]), "12/stack-parent");
    sd.cd("12-stack-parent");
    let r = sd.run_sep(&["start", "--no-implement", "--parent", "12/stack-parent", "12/stack-child"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, sd.path("12-stack-child"));
    assert!(!sd.tmp.join("12").exists());
}

// ── start, then implement ────────────────────────────────────────────────────

// @test "start: runs implement in the new worktree; stdout is still only the path"
#[test]
fn start_runs_implement_in_the_new_worktree_stdout_is_still_only_the_path() {
    let sd = sd();
    let r = sd.run_sep(&["start", "feat-x", "add a third line"]);
    assert_eq!(r.status, 0);
    assert_eq!(r.output, sd.path("feat-x"));
    assert_eq!(sd.fx("claude.cwd"), sd.path("feat-x"));
    assert_eq!(sd.git(&["log", "-1", "--format=%s", "feat-x"]), "Add c");
    assert_has(&r.stderr, &["Added the third line."]);
}

// @test "start: a failed implement still prints the path, nonzero"
#[test]
fn start_a_failed_implement_still_prints_the_path_nonzero() {
    let sd = sd();
    sd.put("fail", "");
    let r = sd.run_sep(&["start", "feat-x", "a task"]);
    assert_ne!(r.status, 0);
    assert_eq!(r.output, sd.path("feat-x"));
    assert_eq!(sd.git(&["config", "branch.feat-x.sheepdog"]), "true");
}

// ── implement ────────────────────────────────────────────────────────────────

const TASK: &str = "add a third line";

// @test "implement: the agent edits, the pipeline commits with its message"
#[test]
fn implement_the_agent_edits_the_pipeline_commits_with_its_message() {
    let mut sd = sd();
    sd.started(TASK);
    let r = sd.run(&["implement"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.subject(), "Add c");
    assert_eq!(sd.last_line(), "c");
    assert!(sd.porcelain().is_empty());
    assert_has(&r.output, &["Added the third line."]);
}

// sd state's NEXT is the one place the order lives (#144): after implement it
// may say restack or push before review.
// @test "implement: names no next command; sd state's NEXT does that (#144)"
#[test]
fn implement_names_no_next_command_sd_states_next_does_that_144() {
    let mut sd = sd();
    sd.started(TASK);
    let r = sd.run(&["implement"]);
    assert_eq!(r.status, 0);
    assert_lacks(&r.output, "next:");
    assert_lacks(&r.output, "sd act review");
}

#[test]
fn start_records_harness_before_implement_so_a_failed_first_step_keeps_it_242() {
    let mut sd = sd();
    sd.fake("pi", "#!/usr/bin/env bash\necho 'pi: no quota' >&2\nexit 1\n");
    let r = sd.run_sep(&["start", "--harness", "pi", "feat-x", "add c"]);
    assert_ne!(r.status, 0);
    assert_eq!(sd.git(&["config", "branch.feat-x.sd-harness"]), "pi");
    sd.fake("pi", PI);
    sd.cd("feat-x");
    sd.ok(&["implement"]);
    assert!(sd.ran("pi.args"));
}

#[test]
fn start_leaves_the_harness_of_a_branch_that_has_a_session_242() {
    let mut sd = sd();
    sd.started(TASK);
    sd.ok(&["implement"]);
    sd.cd("repo");
    sd.git(&["worktree", "remove", "--force", "../feat-x"]);
    let r = sd.run_sep(&["start", "--harness", "pi", "--no-implement", "feat-x"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.git(&["config", "branch.feat-x.sd-harness"]), "claude");
}

// @test "implement: the prompt carries the task and the verify command"
#[test]
fn implement_the_prompt_carries_the_task_and_the_verify_command() {
    let mut sd = sd();
    sd.started("add a third line");
    sd.write_global(r#"{"settings": {"verify": "echo checks-ok"}}"#);
    sd.ok(&["implement"]);
    assert_has(&sd.prompt(), &["add a third line"]);
    assert_has(&sd.prompt(), &["echo checks-ok"]);
}

// @test "implement: mints the branch's session and stores it; a second run resumes it"
#[test]
fn implement_mints_the_branchs_session_and_stores_it_a_second_run_resumes_it() {
    let mut sd = sd();
    sd.started(TASK);
    sd.ok(&["implement"]);
    let sid = sd.git(&["config", "branch.feat-x.sd-session"]);
    assert!(!sid.is_empty());
    assert_eq!(sd.git(&["config", "branch.feat-x.sd-harness"]), "claude");
    assert!(sd.has_word("--session-id"));
    sd.ok(&["implement", "--force"]);
    assert!(sd.has_word("--resume"));
    assert!(sd.has_word(&sid));
}

// @test "implement: no free shell: no bare Bash, read-only git; commit, push and gh denied"
#[test]
fn implement_no_free_shell_no_bare_bash_read_only_git_commit_push_and_gh_denied() {
    let mut sd = sd();
    sd.started(TASK);
    sd.ok(&["implement"]);
    let allow = sd.allowed();
    let deny = sd.after("claude.args", "--disallowedTools");
    assert_lacks(&format!(",{allow},"), ",Bash,");
    assert_has(&allow, &["Bash(git diff *)"]);
    assert_has(&allow, &["Edit"]);
    assert_has(&deny, &["git commit"]);
    assert_has(&deny, &["git push"]);
    assert_has(&deny, &["gh "]);
}

// @test "implement: a plain verify command becomes an exact rule"
#[test]
fn implement_a_plain_verify_command_becomes_an_exact_rule() {
    let mut sd = sd();
    sd.started(TASK);
    sd.write_global(r#"{"settings": {"verify": "test -f file.txt"}}"#);
    sd.ok(&["implement"]);
    assert_has(&format!(",{},", sd.allowed()), &[",Bash(test -f file.txt),"]);
}

// @test "implement: a verify with shell syntax doesn't become a rule, and says where to allow it"
#[test]
fn implement_a_verify_with_shell_syntax_doesnt_become_a_rule_and_says_where_to_allow_it() {
    let mut sd = sd();
    sd.started(TASK);
    sd.write_global(r#"{"settings": {"verify": "ls *.txt"}}"#);
    let r = sd.run_sep(&["implement"]);
    assert_eq!(r.status, 0);
    assert_lacks(&sd.allowed(), "Bash(ls");
    assert_has(&r.stderr, &["settings.implementAllow"]);
}

// @test "implement: settings.implementAllow adds its rules, and the prompt names them"
#[test]
fn implement_settings_implementallow_adds_its_rules_and_the_prompt_names_them() {
    let mut sd = sd();
    sd.started(TASK);
    sd.write_global(r#"{"settings": {"implementAllow": ["Bash(bats *)", "WebFetch"]}}"#);
    sd.ok(&["implement"]);
    assert_has(&format!(",{},", sd.allowed()), &[",Bash(bats *),WebFetch,"]);
    assert_has(&sd.prompt(), &["Bash(bats *)"]);
}

// @test "implement: pi gets no shell, and its prompt names no Claude rules"
#[test]
fn implement_pi_gets_no_shell_and_its_prompt_names_no_claude_rules() {
    let mut sd = sd();
    sd.started(TASK);
    sd.fake("pi", PI);
    sd.write_global(r#"{"settings": {"verify": "test -f file.txt", "implementAllow": ["Bash(bats *)"]}}"#);
    let r = sd.run(&["implement", "--harness", "pi"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.after("pi.args", "--tools"), "read,edit,write");
    assert_lacks(&sd.fx("pi.args"), "Bash(");
    assert_has(&sd.fx("pi.args"), &["can't run commands"]);
}

// @test "implement: with nothing allowed, the prompt says the agent can't run commands"
#[test]
fn implement_with_nothing_allowed_the_prompt_says_the_agent_cant_run_commands() {
    let mut sd = sd();
    sd.started(TASK);
    sd.ok(&["implement"]);
    assert_has(&sd.prompt(), &["can't run commands"]);
}

// @test "implement: commits the agent made itself count as its work"
#[test]
fn implement_commits_the_agent_made_itself_count_as_its_work() {
    let mut sd = sd();
    sd.started(TASK);
    sd.agent("#!/usr/bin/env bash\necho c >> file.txt && git commit -qam \"agent made it\"\n");
    let r = sd.run(&["implement"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.subject(), "agent made it");
}

// @test "implement: no commit message from the agent falls back to the task's first line"
#[test]
fn implement_no_commit_message_from_the_agent_falls_back_to_the_tasks_first_line() {
    let mut sd = sd();
    sd.started("add a third line\n\nmore detail");
    sd.put("result.json", r#"{"outcome": "done", "commit_message": null, "summary": "ok"}"#);
    sd.ok(&["implement"]);
    assert_eq!(sd.subject(), "add a third line");
}

// @test "implement: a commit that changes CI config is held for chrismo; the agent's own commits count (#124)"
#[test]
fn implement_a_commit_that_changes_ci_config_is_held_for_chrismo_the_agents_own_commits_count_124() {
    let mut sd = sd();
    sd.started(TASK);
    sd.agent("#!/usr/bin/env bash\necho c >> file.txt\nmkdir -p .github/workflows && echo x > .github/workflows/ci.yml && git add -A && git commit -qm \"agent made it\"\necho d >> file.txt\n");
    let r = sd.run(&["implement"]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &["held: the agent changed CI config (.github/workflows/ci.yml): review, then sd act push"]);
    // Keyed on the commit that changed it, the agent's; the pipeline's commit of the rest follows.
    assert_eq!(sd.held(), format!("{}\n.github/workflows/ci.yml", sd.sha("HEAD~1")));
}

// @test "implement: a CI config file with a non-ASCII name is held under its own name (#124)"
#[test]
fn implement_a_ci_config_file_with_a_non_ascii_name_is_held_under_its_own_name_124() {
    let mut sd = sd();
    sd.started(TASK);
    sd.agent("#!/usr/bin/env bash\nmkdir -p .github/workflows && echo x > .github/workflows/tëst.yml\n");
    let r = sd.run(&["implement"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.held(), format!("{}\n.github/workflows/tëst.yml", sd.sha("HEAD")));
}

// @test "implement: a CI config change the agent committed itself before needing input is held (#124)"
#[test]
fn implement_a_ci_config_change_the_agent_committed_itself_before_needing_input_is_held_124() {
    let mut sd = sd();
    sd.started(TASK);
    sd.agent(AGENT_COMMITS_CI);
    sd.put("result.json", r#"{"outcome": "needs-input", "commit_message": null, "summary": "which runner?"}"#);
    let r = sd.run(&["implement"]);
    assert_ne!(r.status, 0);
    assert_eq!(sd.held(), format!("{}\n.github/workflows/ci.yml", sd.sha("HEAD")));
}

// @test "implement: a CI config change the agent committed itself is held even when the step then fails (#124)"
#[test]
fn implement_a_ci_config_change_the_agent_committed_itself_is_held_even_when_the_step_then_fails_124() {
    let mut sd = sd();
    sd.started(TASK);
    sd.agent(AGENT_COMMITS_CI);
    sd.put("result.json", "not json");
    let r = sd.run(&["implement"]);
    assert_ne!(r.status, 0);
    assert_eq!(sd.held(), format!("{}\n.github/workflows/ci.yml", sd.sha("HEAD")));
}

// @test "implement: an agent that pushed its CI config change itself fails loudly; no hold that would vanish (#124)"
#[test]
fn implement_an_agent_that_pushed_its_ci_config_change_itself_fails_loudly_no_hold_that_would_vanish_124() {
    let mut sd = sd();
    sd.started(TASK);
    sd.agent("#!/usr/bin/env bash\nmkdir -p .github/workflows && echo x > .github/workflows/ci.yml && git add -A && git commit -qm \"agent made it\" && git push -q origin feat-x 2>/dev/null\n");
    let r = sd.run(&["implement"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["the agent pushed CI config itself (.github/workflows/ci.yml)"]);
    assert_eq!(sd.git_status(&["config", "--get", "branch.feat-x.sd-held"]), 1);
}

// @test "implement: a review-fix round that changes CI config is held too, joined with the hold before it (#124)"
#[test]
fn implement_a_review_fix_round_that_changes_ci_config_is_held_too_joined_with_the_hold_before_it_124() {
    let mut sd = sd();
    sd.started(TASK);
    sd.git(&["config", "branch.feat-x.sd-held", &format!("{}\n.rwx/tests.yml", sd.sha("HEAD"))]);
    sd.findings_at("HEAD", "1");
    sd.agent("#!/usr/bin/env bash\nmkdir -p .circleci && echo x > .circleci/config.yml\n");
    let r = sd.run(&["implement"]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &["held: the agent changed CI config (.circleci/config.yml, .rwx/tests.yml)"]);
}

// @test "implement: done with nothing changed is a failure"
#[test]
fn implement_done_with_nothing_changed_is_a_failure() {
    let mut sd = sd();
    sd.started(TASK);
    sd.rm("agent");
    let r = sd.run(&["implement"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["changed nothing"]);
}

// @test "implement: a fix round numbers each finding and asks for an answer per id"
#[test]
fn implement_a_fix_round_numbers_each_finding_and_asks_for_an_answer_per_id() {
    let mut sd = sd();
    sd.started(TASK);
    sd.two_findings();
    sd.answers(&[r#"{"id": "1", "action": "fixed", "reply": "A now"}"#, r#"{"id": "2", "action": "fixed", "reply": "B now"}"#]);
    sd.ok(&["implement"]);
    let p = sd.prompt();
    assert_has(&p, &["- [1] file.txt:1 (warning) a should be A"]);
    assert_has(&p, &["- [2] file.txt:2 (error) b should be B"]);
    assert_has(&p, &["declined"]);
    assert_has(&sd.fx("claude.args"), &[r#""action": {"enum": ["fixed", "declined", "ask"]}"#]);
}

// @test "implement: the first implement keeps its schema, with no items"
#[test]
fn implement_the_first_implement_keeps_its_schema_with_no_items() {
    let mut sd = sd();
    sd.started(TASK);
    sd.ok(&["implement"]);
    let args = sd.fx("claude.args");
    assert_has(&args, &[r#""required": ["outcome", "commit_message", "summary"]"#]);
    assert_lacks(&args, r#""items""#);
}

// @test "implement: fixed one, declined one: commits, records each answer, and the next review is due"
#[test]
fn implement_fixed_one_declined_one_commits_records_each_answer_and_the_next_review_is_due() {
    let mut sd = sd();
    sd.started(TASK);
    sd.two_findings();
    sd.answers(&[
        r#"{"id": "1", "action": "declined", "reply": "a is lowercase on purpose"}"#,
        r#"{"id": "2", "action": "fixed", "reply": "B now"}"#,
    ]);
    let r = sd.run(&["implement"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.subject(), "Add c");
    assert_has(&r.output, &["declined", "a is lowercase on purpose"]);
    assert_lacks(&r.output, "next:");
    assert_eq!(sd.findings_rec("[.findings[] | [.action, .reply]]"), r#"[["declined","a is lowercase on purpose"],["fixed","B now"]]"#);
    assert_eq!(sd.state(r#"[.review.findings.at_head, any(.next[]; .key == "review")]"#), "[false,true]");
}

// @test "implement: a finding the agent marks ask goes to chrismo: nonzero, edits uncommitted, nothing recorded"
#[test]
fn implement_a_finding_the_agent_marks_ask_goes_to_chrismo_nonzero_edits_uncommitted_nothing_recorded() {
    let mut sd = sd();
    sd.started(TASK);
    sd.two_findings();
    sd.answers(&[r#"{"id": "1", "action": "ask", "reply": "A or a?"}"#, r#"{"id": "2", "action": "fixed", "reply": "B now"}"#]);
    let r = sd.run(&["implement"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["needs input", "A or a?"]);
    assert!(!sd.porcelain().is_empty());
    assert_eq!(sd.subject(), "init");
    assert_eq!(sd.findings_rec("[.findings[] | .action]"), "[null,null]");
    // implement refuses the dirty worktree, so it isn't the next step
    assert_has(&r.output, &["sd act attach"]);
    assert_lacks(&r.output, "then sd act implement");
}

// @test "implement: fixed needs no reply text; declined does"
#[test]
fn implement_fixed_needs_no_reply_text_declined_does() {
    let mut sd = sd();
    sd.started(TASK);
    sd.two_findings();
    sd.answers(&[r#"{"id": "1", "action": "fixed", "reply": ""}"#, r#"{"id": "2", "action": "fixed", "reply": "B now"}"#]);
    let r = sd.run(&["implement"]);
    assert_eq!(r.status, 0);
    assert_eq!(sd.subject(), "Add c");
    sd.git(&["reset", "-q", "--hard", "HEAD~1"]);
    sd.two_findings();
    sd.answers(&[r#"{"id": "1", "action": "declined", "reply": " "}"#, r#"{"id": "2", "action": "fixed", "reply": "B now"}"#]);
    let r = sd.run(&["implement"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["[1]", "no reply text"]);
}

// @test "implement: a finding the agent didn't answer counts as ask"
#[test]
fn implement_a_finding_the_agent_didnt_answer_counts_as_ask() {
    let mut sd = sd();
    sd.started(TASK);
    sd.two_findings();
    sd.answers(&[r#"{"id": "2", "action": "fixed", "reply": "B now"}"#]);
    let r = sd.run(&["implement"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["[1]", "did not answer"]);
    assert_eq!(sd.subject(), "init");
}

// @test "implement: fixed with nothing changed counts as ask"
#[test]
fn implement_fixed_with_nothing_changed_counts_as_ask() {
    let mut sd = sd();
    sd.started(TASK);
    sd.findings_at("HEAD", "1");
    sd.rm("agent");
    let r = sd.run(&["implement"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["changed nothing", "B now"]);
    assert_eq!(sd.findings_rec(".findings[0].action"), "null");
}

// @test "implement: a finding declined in an earlier answer isn't sent again"
#[test]
fn implement_a_finding_declined_in_an_earlier_answer_isnt_sent_again() {
    let mut sd = sd();
    sd.started(TASK);
    sd.two_findings();
    let rec = sd.findings_rec(r#".findings[0] += {action: "declined", reply: "no"}"#);
    sd.put_findings(&rec);
    sd.answers(&[r#"{"id": "2", "action": "fixed", "reply": "B now"}"#]);
    let r = sd.run(&["implement"]);
    assert_eq!(r.status, 0);
    assert_lacks(&sd.prompt(), "a should be A");
    assert_has(&sd.prompt(), &["- [2] file.txt:2"]);
    assert_eq!(sd.findings_rec("[.findings[] | [.action, .reply]]"), r#"[["declined","no"],["fixed","B now"]]"#);
}

// @test "implement: review findings on HEAD go into the prompt"
#[test]
fn implement_review_findings_on_head_go_into_the_prompt() {
    let mut sd = sd();
    sd.started(TASK);
    sd.findings_at("HEAD", "1");
    let r = sd.run(&["implement"]);
    assert_eq!(r.status, 0);
    assert_has(&sd.prompt(), &["b should be B"]);
    assert_has(&sd.prompt(), &["file.txt:2"]);
    assert_eq!(sd.subject(), "Add c");
}

// @test "implement: a fix round is framed as fixing the findings, with only those that block"
#[test]
fn implement_a_fix_round_is_framed_as_fixing_the_findings_with_only_those_that_block() {
    let mut sd = sd();
    sd.started(TASK);
    sd.findings_on_head(
        r#"[
          {file: "file.txt", line: 2, severity: "error", body: "b should be B"},
          {file: "file.txt", line: 1, severity: "nit", body: "a could be A"}]"#,
    );
    sd.answers(&[r#"{"id": "1", "action": "fixed", "reply": "B now"}"#]);
    sd.ok(&["implement"]);
    assert_has(&sd.prompt_head(), &["review"]);
    assert!(!has_line(&sd.prompt(), "Implement the task below"));
    assert_has(&sd.prompt(), &["b should be B"]);
    assert_lacks(&sd.prompt(), "a could be A");
    assert_has(&sd.prompt(), &["add a third line"]);
}

// @test "implement: review findings on HEAD run even with no task recorded"
#[test]
fn implement_review_findings_on_head_run_even_with_no_task_recorded() {
    let mut sd = sd();
    sd.started(TASK);
    sd.git(&["config", "--unset", "branch.feat-x.sd-task"]);
    sd.findings_at("HEAD", "1");
    let r = sd.run(&["implement"]);
    assert_eq!(r.status, 0);
    assert_has(&sd.prompt(), &["b should be B"]);
    assert_has(&sd.prompt(), &["(none recorded)"]);
}

// @test "implement: findings the agent already declined aren't sent again"
#[test]
fn implement_findings_the_agent_already_declined_arent_sent_again() {
    let mut sd = sd();
    sd.started(TASK);
    sd.findings_on_head(r#"[{file: "file.txt", line: 2, severity: "error", body: "b should be B", action: "declined", reply: "no"}]"#);
    sd.ok(&["implement"]);
    assert_lacks(&sd.prompt(), "b should be B");
    assert!(has_line(&sd.prompt(), "Implement the task below"));
    sd.git(&["config", "--unset", "branch.feat-x.sd-task"]);
    let r = sd.run(&["implement"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["no task recorded"]);
}

// @test "implement: findings on a rebased HEAD count when sd state says they're about it"
#[test]
fn implement_findings_on_a_rebased_head_count_when_sd_state_says_theyre_about_it() {
    let mut sd = sd();
    sd.started(TASK);
    sd.git(&["update-ref", "-d", "refs/sheepdog/feat-x/base"]);
    append(&sd.cwd.join("file.txt"), "c");
    sd.git(&["commit", "-qam", "add c"]);
    let rec = jq(
        &[
            "-nc", "--arg", "h", &sd.sha("HEAD"), "--arg", "b", &sd.sha("HEAD~1"),
            r#"{head: $h, base: $b, round: 1,
          findings: [{file: "file.txt", line: 3, severity: "error", body: "c should be C"}]}"#,
        ],
        "",
    );
    sd.put_findings(&rec);
    sd.other_push("main", "main moves");
    sd.git(&["fetch", "-q"]);
    sd.git(&["rebase", "-q", "origin/main"]);
    assert_eq!(sd.state(".review.findings.at_head"), "true");
    sd.answers(&[r#"{"id": "1", "action": "fixed", "reply": "C now"}"#]);
    sd.ok(&["implement"]);
    assert_has(&sd.prompt(), &["c should be C"]);
}

// @test "implement: with a findings record, sd state failing is a failure, not the task prompt"
#[test]
fn implement_with_a_findings_record_sd_state_failing_is_a_failure_not_the_task_prompt() {
    let mut sd = sd();
    sd.started(TASK);
    sd.findings_at("HEAD", "1");
    sd.verb_ok("config", &["set", "--global", "--json", "settings.botIgnoreBody", r#"["("]"#]);
    let r = sd.run(&["implement"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["sd state failed"]);
    assert!(!sd.ran("claude.prompt"));
}

// @test "implement: a fix round with no commit message from the agent says it fixed review findings"
#[test]
fn implement_a_fix_round_with_no_commit_message_from_the_agent_says_it_fixed_review_findings() {
    let mut sd = sd();
    sd.started(TASK);
    sd.findings_at("HEAD", "1");
    sd.put("result.json", r#"{"outcome": "done", "summary": "Fixed b.", "items": [{"id": "1", "action": "fixed", "reply": "B now"}]}"#);
    sd.ok(&["implement"]);
    assert_eq!(sd.subject(), "Fix the local review findings");
}

// @test "--accept belongs to sd act review only"
#[test]
fn accept_belongs_to_sd_act_review_only() {
    let mut sd = sd();
    sd.started(TASK);
    let r = sd.run(&["implement", "--accept"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["--accept"]);
    assert!(!sd.ran("claude.prompt"));
}

// @test "implement: a finding missing fields still reads as itself in the prompt"
#[test]
fn implement_a_finding_missing_fields_still_reads_as_itself_in_the_prompt() {
    let mut sd = sd();
    sd.started(TASK);
    sd.findings_on_head(r#"[{file: "file.txt", body: "b?"}]"#);
    sd.answers(&[r#"{"id": "1", "action": "fixed", "reply": "B now"}"#]);
    sd.ok(&["implement"]);
    assert_has(&sd.prompt(), &["- [1] file.txt:? (?) b?"]);
}

// @test "implement: settings.implementPrompt replaces the guidance; the contract and task stay"
#[test]
fn implement_settings_implementprompt_replaces_the_guidance_the_contract_and_task_stay() {
    let mut sd = sd();
    sd.started(TASK);
    fs::write(sd.tmp.join("implement.md"), "Work in tiny steps.\n").unwrap();
    sd.write_global(&format!(r#"{{"settings": {{"implementPrompt": "{}"}}}}"#, sd.path("implement.md")));
    sd.ok(&["implement"]);
    assert_eq!(sd.prompt_head(), "Work in tiny steps.");
    let p = sd.prompt();
    assert_lacks(&p, "Implement the task below");
    assert_has(&p, &["do not commit, push, or run gh"]);
    assert_has(&p, &[r#"Return outcome "done""#]);
    assert_has(&p, &["add a third line"]);
}

// @test "implement: .sheepdog/implement.md in the branch's base is the repo's guidance"
#[test]
fn implement_sheepdog_implement_md_in_the_branchs_base_is_the_repos_guidance() {
    let mut sd = sd();
    fs::create_dir(sd.cwd.join(".sheepdog")).unwrap();
    sd.write(".sheepdog/implement.md", "Repo implement guidance.\n");
    sd.git(&["add", ".sheepdog"]);
    sd.git(&["commit", "-qm", "guidance"]);
    sd.git(&["push", "-q", "origin", "main"]);
    sd.started(TASK);
    sd.ok(&["implement"]);
    assert_eq!(sd.prompt_head(), "Repo implement guidance.");
}

// @test "implement: a branch with no base pin and no remote still gets its guidance"
#[test]
fn implement_a_branch_with_no_base_pin_and_no_remote_still_gets_its_guidance() {
    let mut sd = sd();
    sd.git(&["init", "-q", &sd.path("local")]);
    sd.cd("local");
    fs::create_dir(sd.cwd.join(".sheepdog")).unwrap();
    sd.write(".sheepdog/implement.md", "Local repo guidance.\n");
    sd.write("file.txt", "a\nb\n");
    sd.git(&["add", "."]);
    sd.git(&["commit", "-qm", "init"]);
    sd.git(&["switch", "-qc", "feat"]);
    sd.git(&["config", "branch.feat.sd-task", "add c"]);
    sd.ok(&["implement"]);
    assert_eq!(sd.prompt_head(), "Local repo guidance.");
}

// @test "implement: with no base to trust, the branch's own .sheepdog/ is ignored"
#[test]
fn implement_with_no_base_to_trust_the_branchs_own_sheepdog_is_ignored() {
    let mut sd = sd();
    sd.git(&["init", "-q", "-b", "feat", &sd.path("lone")]);
    sd.cd("lone");
    fs::create_dir(sd.cwd.join(".sheepdog")).unwrap();
    sd.write(".sheepdog/implement.md", "Branch-written guidance.\n");
    sd.write("file.txt", "a\nb\n");
    sd.git(&["add", "."]);
    sd.git(&["commit", "-qm", "init"]);
    sd.git(&["config", "branch.feat.sd-task", "add c"]);
    sd.ok(&["implement"]);
    assert_ne!(sd.prompt_head(), "Branch-written guidance.");
    assert_has(&sd.prompt(), &["Implement the task below"]);
}

// @test "implement: on the default branch itself, its own .sheepdog/ is not trusted"
#[test]
fn implement_on_the_default_branch_itself_its_own_sheepdog_is_not_trusted() {
    let mut sd = sd();
    sd.git(&["init", "-q", "-b", "main", &sd.path("onmain")]);
    sd.cd("onmain");
    sd.write("file.txt", "a\nb\n");
    sd.git(&["add", "."]);
    sd.git(&["commit", "-qm", "init"]);
    fs::create_dir(sd.cwd.join(".sheepdog")).unwrap();
    sd.write(".sheepdog/implement.md", "Agent-written guidance.\n");
    sd.git(&["add", "."]);
    sd.git(&["commit", "-qm", "the agent's commit"]);
    sd.git(&["config", "branch.main.sd-task", "add c"]);
    sd.ok(&["implement", "main"]);
    assert_has(&sd.prompt(), &["Implement the task below"]);
}

// @test "implement: on the default branch itself, pushed, its own .sheepdog/ is not trusted"
#[test]
fn implement_on_the_default_branch_itself_pushed_its_own_sheepdog_is_not_trusted() {
    let sd = sd();
    fs::create_dir(sd.cwd.join(".sheepdog")).unwrap();
    sd.write(".sheepdog/implement.md", "Agent-written guidance.\n");
    sd.git(&["add", "."]);
    sd.git(&["commit", "-qm", "the agent's commit"]);
    sd.git(&["push", "-q", "origin", "main"]);
    sd.git(&["config", "branch.main.sd-task", "add c"]);
    sd.ok(&["implement", "main"]);
    assert_has(&sd.prompt(), &["Implement the task below"]);
}

// @test "implement: settings.reviewFixPrompt replaces a fix round's guidance; the contract stays"
#[test]
fn implement_settings_reviewfixprompt_replaces_a_fix_rounds_guidance_the_contract_stays() {
    let mut sd = sd();
    sd.started(TASK);
    sd.findings_at("HEAD", "1");
    fs::write(sd.tmp.join("fix.md"), "Only fix errors.\n").unwrap();
    sd.write_global(&format!(r#"{{"settings": {{"reviewFixPrompt": "{}"}}}}"#, sd.path("fix.md")));
    sd.ok(&["implement"]);
    assert_eq!(sd.prompt_head(), "Only fix errors.");
    assert_has(&sd.prompt(), &["do not commit, push, or run gh"]);
    assert_has(&sd.prompt(), &["b should be B"]);
}

// @test "implement: findings on an older head stay out of the prompt"
#[test]
fn implement_findings_on_an_older_head_stay_out_of_the_prompt() {
    let mut sd = sd();
    sd.started(TASK);
    sd.findings_at("HEAD", "1");
    sd.git(&["commit", "-q", "--allow-empty", "-m", "later"]);
    sd.ok(&["implement", "--force"]);
    assert_lacks(&sd.prompt(), "b should be B");
}

// @test "implement: declining every review finding records each, for chrismo to decide"
#[test]
fn implement_declining_every_review_finding_records_each_for_chrismo_to_decide() {
    let mut sd = sd();
    sd.started(TASK);
    sd.findings_at("HEAD", "2");
    sd.rm("agent");
    sd.answers(&[r#"{"id": "1", "action": "declined", "reply": "b is right as it is."}"#]);
    let r = sd.run(&["implement"]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &["declined", "b is right as it is."]);
    assert_lacks(&r.output, "next:");
    assert_eq!(sd.findings_rec("[.round,.findings[0].action, .findings[0].reply]"), r#"[2,"declined","b is right as it is."]"#);
    assert_eq!(sd.state(r#"[.review.findings.declined, any(.next[]; .key == "review-decide")]"#), "[true,true]");
}

// @test "implement: needs-input prints the question, leaves the edits uncommitted, nonzero"
#[test]
fn implement_needs_input_prints_the_question_leaves_the_edits_uncommitted_nonzero() {
    let mut sd = sd();
    sd.started(TASK);
    sd.put("result.json", r#"{"outcome": "needs-input", "commit_message": null, "summary": "Which file?"}"#);
    let r = sd.run(&["implement"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["Which file?"]);
    assert!(!sd.porcelain().is_empty());
    assert_eq!(sd.subject(), "init");
    assert_eq!(sd.question(r#".step + " " + .text"#), "implement Which file?");
}

#[test]
fn implement_on_a_branch_ahead_of_its_target_refuses_at_zero_tokens_unless_force_241() {
    let mut sd = sd();
    sd.started(TASK);
    sd.ok(&["implement"]);
    sd.rm("claude.args");
    let r = sd.run(&["implement"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["nothing to implement: feat-x is already 1 commit ahead of main; --force to implement its task anyway"]);
    assert!(!sd.ran("claude.args"));
    sd.ok(&["implement", "--force"]);
    assert!(sd.ran("claude.args"));
}

#[test]
fn implement_a_review_fix_round_still_runs_once_the_task_is_done_241() {
    let mut sd = sd();
    sd.started(TASK);
    sd.ok(&["implement"]);
    sd.findings_at("HEAD", "1");
    sd.ok(&["implement"]);
    assert_has(&sd.prompt(), &["b should be B"]);
}

// @test "implement: with its question standing, a rerun refuses before the agent runs; an ack lets it (#167)"
#[test]
fn implement_with_its_question_standing_a_rerun_refuses_before_the_agent_runs_an_ack_lets_it_167() {
    let mut sd = sd();
    sd.started(TASK);
    sd.rm("agent");
    sd.put("result.json", r#"{"outcome": "needs-input", "commit_message": null, "summary": "Which file?"}"#);
    let r = sd.run(&["implement"]);
    assert_has(&r.output, &[r#"answer with sd act answer "<your answer>" feat-x; or answer in its session (sd act attach feat-x), then sd act ack --question and sd act implement"#]);
    sd.rm("claude.args");
    let r = sd.run(&["implement"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["not running implement on feat-x", "Which file?"]);
    assert!(!sd.ran("claude.args"));
    sd.ok(&["ack", "--question", "feat-x"]);
    sd.run(&["implement"]);
    assert!(sd.ran("claude.args"));
}

// @test "implement: sd act answer resumes its session with the answer and commits its edits (#178)"
#[test]
fn implement_sd_act_answer_resumes_its_session_with_the_answer_and_commits_its_edits_178() {
    let mut sd = sd();
    sd.started(TASK);
    sd.put("result.json", r#"{"outcome": "needs-input", "commit_message": null, "summary": "Which file?"}"#);
    let r = sd.run(&["implement"]);
    assert_has(&r.output, &[r#"sd act answer "<your answer>" feat-x"#]);
    let sid = sd.git(&["config", "branch.feat-x.sd-session"]);
    sd.rm("agent");
    sd.put("result.json", r#"{"outcome": "done", "commit_message": "Edit file.txt", "summary": "done"}"#);
    let r = sd.run(&["answer", "the one with two lines"]);
    assert_eq!(r.status, 0, "{}", r.output);
    assert_has(&sd.prompt(), &["the one with two lines"]);
    assert_lacks(&sd.prompt(), "add a third line"); // the session has the task already
    assert_eq!(sd.after("claude.args", "--resume"), sid);
    assert_eq!(sd.subject(), "Edit file.txt");
    assert_eq!(sd.last_line(), "c");
    assert_ne!(sd.git_status(&["rev-parse", "-q", "--verify", "refs/sheepdog/feat-x/question"]), 0);
}

// @test "implement: a fix round's ask is review-fix's question; a rerun refuses unless --force (#167)"
#[test]
fn implement_a_fix_rounds_ask_is_review_fixs_question_a_rerun_refuses_unless_force_167() {
    let mut sd = sd();
    sd.started(TASK);
    sd.findings_at("HEAD", "1");
    sd.rm("agent");
    sd.answers(&[r#"{"id": "1", "action": "ask", "reply": "B, or b?"}"#]);
    let r = sd.run(&["implement"]);
    assert_ne!(r.status, 0);
    assert_eq!(sd.question(".step"), "review-fix");
    assert_has(&sd.question(".text"), &["[1] file.txt:2 B, or b?"]);
    sd.rm("claude.args");
    let r = sd.run(&["implement"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["not running review-fix on feat-x", "B, or b?"]);
    assert!(!sd.ran("claude.args"));
    let r = sd.run(&["implement", "--force"]);
    assert_has(&r.output, &["the review-fix agent's question on feat-x stands; running anyway"]);
    assert!(sd.ran("claude.args"));
}

// @test "implement: gave-up is nonzero with the agent's reason, nothing committed"
#[test]
fn implement_gave_up_is_nonzero_with_the_agents_reason_nothing_committed() {
    let mut sd = sd();
    sd.started(TASK);
    sd.put("result.json", r#"{"outcome": "gave-up", "commit_message": null, "summary": "makes no sense"}"#);
    let r = sd.run(&["implement"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["makes no sense"]);
    assert_eq!(sd.subject(), "init");
}

#[test]
fn implement_gave_up_stops_for_chrismo_a_rerun_refuses_at_zero_tokens_until_an_ack_254() {
    let mut sd = sd();
    sd.started(TASK);
    sd.rm("agent");
    sd.put("result.json", r#"{"outcome": "gave-up", "commit_message": null, "summary": "makes no sense"}"#);
    let r = sd.run(&["implement"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["the agent gave up on feat-x: makes no sense", "NEXT shows the question until feat-x's HEAD"]);
    let rec = sd.git(&["cat-file", "blob", "refs/sheepdog/feat-x/question"]);
    assert_eq!(jq(&["-r", "[.step, .kind, .text] | join(\"|\")"], &rec), "implement|gave-up|makes no sense");
    sd.rm("claude.args");
    let r = sd.run(&["implement"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["not running implement on feat-x: its agent gave up, and nothing has changed since"]);
    assert!(!sd.ran("claude.args"));
    sd.ok(&["ack", "--question"]);
    sd.agent("#!/usr/bin/env bash\necho c >> file.txt\n");
    sd.put("result.json", r#"{"outcome": "done", "commit_message": "Add c", "summary": "Added it."}"#);
    sd.ok(&["implement"]);
}

#[test]
fn implement_done_with_nothing_changed_stops_for_chrismo_too_254() {
    let mut sd = sd();
    sd.started(TASK);
    sd.rm("agent");
    let r = sd.run(&["implement"]);
    assert_ne!(r.status, 0);
    let rec = sd.git(&["cat-file", "blob", "refs/sheepdog/feat-x/question"]);
    assert_eq!(jq(&["-r", "[.kind, .text] | join(\"|\")"], &rec), "gave-up|said done but changed nothing: Added the third line.");
}

/// A claude that wrote its session's transcript, then failed.
const CLAUDE_FAILS_MIDWAY: &str = r#"#!/usr/bin/env bash
sid= prev=
for a in "$@"; do [[ $prev == --session-id || $prev == --resume ]] && sid=$a; prev=$a; done
mkdir -p "$HOME/.claude/projects/p" && echo '{}' > "$HOME/.claude/projects/p/$sid.jsonl"
echo "API Error: 529 overloaded" >&2
exit 1
"#;

#[test]
fn implement_a_failed_step_keeps_a_session_the_harness_wrote_so_a_retry_resumes_it_254() {
    let mut sd = sd();
    sd.started(TASK);
    sd.fake("claude", CLAUDE_FAILS_MIDWAY);
    let r = sd.run(&["implement"]);
    assert_ne!(r.status, 0);
    let sid = sd.git(&["config", "branch.feat-x.sd-session"]);
    assert!(!sid.is_empty());
    assert_eq!(sd.git(&["config", "branch.feat-x.sd-harness"]), "claude");
    sd.fake("claude", CLAUDE);
    sd.ok(&["implement"]);
    assert!(sd.has_word("--resume"));
    assert!(sd.has_word(&sid));
}

#[test]
fn implement_a_failed_step_with_no_transcript_stores_no_session_254() {
    let mut sd = sd();
    sd.started(TASK);
    sd.put("fail", "");
    let r = sd.run(&["implement"]);
    assert_ne!(r.status, 0);
    assert_eq!(sd.git_try(&["config", "branch.feat-x.sd-session"]), None);
}

// @test "implement: verify runs after the commit; a failure is nonzero and keeps the commit"
#[test]
fn implement_verify_runs_after_the_commit_a_failure_is_nonzero_and_keeps_the_commit() {
    let mut sd = sd();
    sd.started(TASK);
    sd.write_global(r#"{"settings": {"verify": "echo verify-ran; exit 1"}}"#);
    let r = sd.run(&["implement"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["verify-ran"]);
    assert_eq!(sd.subject(), "Add c");
}

// @test "implement: the branch's running marker lasts through the commit and verify, then is gone (#170)"
#[test]
fn implement_the_branchs_running_marker_lasts_through_the_commit_and_verify_then_is_gone_170() {
    let mut sd = sd();
    sd.started(TASK);
    let (marker, during) = (sd.path("repo/.git/sheepdog/feat-x.running"), sd.path("fx/during"));
    sd.write_global(&format!(r#"{{"settings": {{"verify": "cat {marker} >> {during}; git log -1 --format=%s >> {during}"}}}}"#));
    let r = sd.run(&["implement"]);
    assert_eq!(r.status, 0);
    let during = sd.fx("during");
    assert_match(during.lines().next().unwrap_or_default(), "^[0-9]+ implement ");
    assert_eq!(during.lines().nth(1).unwrap_or_default(), "Add c");
    assert!(!Path::new(&marker).exists());
}

// @test "start: implement's running marker lasts through its verify, then is gone (#170)"
#[test]
fn start_implements_running_marker_lasts_through_its_verify_then_is_gone_170() {
    let sd = sd();
    let marker = sd.path("repo/.git/sheepdog/feat-x.running");
    sd.write_global(&format!(r#"{{"settings": {{"verify": "cat {marker} >> {}"}}}}"#, sd.path("fx/during")));
    let r = sd.run(&["start", "feat-x", "add a third line"]);
    assert_eq!(r.status, 0);
    assert_match(&sd.fx("during"), "(?m)^[0-9]+ implement ");
    assert!(!Path::new(&marker).exists());
}

// @test "implement: refuses a dirty worktree without calling the agent"
#[test]
fn implement_refuses_a_dirty_worktree_without_calling_the_agent() {
    let mut sd = sd();
    sd.started(TASK);
    sd.write("dirt.txt", "dirt\n");
    let r = sd.run(&["implement"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["uncommitted changes"]);
    assert!(!sd.ran("claude.args"));
}

// @test "implement: refuses with no task recorded"
#[test]
fn implement_refuses_with_no_task_recorded() {
    let mut sd = sd();
    sd.ok(&["start", "--no-implement", "feat-x"]);
    sd.cd("feat-x");
    let r = sd.run(&["implement"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["no task recorded"]);
}

// @test "implement: the agent failing is nonzero and names the session"
#[test]
fn implement_the_agent_failing_is_nonzero_and_names_the_session() {
    let mut sd = sd();
    sd.started(TASK);
    sd.put("fail", "");
    let r = sd.run(&["implement"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["implement step failed"]);
}
