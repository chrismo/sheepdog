#!/usr/bin/env bats

bats_require_minimum_version 1.5.0

# Test suite for `sd act ci-fix` (J2): on red CI, run settings.autofix and
# verify first (zero tokens), then hand the failing checks, their logs and the
# local verify output to the agent in the branch's session; the pipeline
# commits, verifies and pushes its fix. A "flaky" verdict is recorded against
# the red SHA so the same red doesn't burn another round.
#
# Real git (a bare origin and a clone). Fakes on PATH: sd state (answers from
# $FX/state.json), gh (REST: commit status from $FX/status.json, check runs
# from $FX/checks.json, a job's failed log from $FX/gha.log; every call logged
# to $FX/gh.calls), rwx (results from $FX/rwx-results.json, logs from
# $FX/rwx.log), claude (runs $FX/agent to edit, prints $FX/result.json, keeps
# its prompt in $FX/claude.prompt).

SD_ACT="$BATS_TEST_DIRNAME/sd-act"
SD_CONFIG="$BATS_TEST_DIRNAME/sd-config"

setup() {
  TMP="$(cd "$BATS_TEST_TMPDIR" && pwd -P)"
  export GIT_CONFIG_GLOBAL="$TMP/gitconfig-global"
  export GIT_CONFIG_SYSTEM=/dev/null
  export SHEEPDOG_STATE_DIR="$TMP/state"   # harness_run logs usage there
  printf '[user]\n\temail = t@example.com\n\tname = Tester\n[init]\n\tdefaultBranch = main\n' > "$GIT_CONFIG_GLOBAL"
  export FX="$TMP/fx" SHEEPDOG_CONFIG="$TMP/global.json"
  mkdir -p "$FX" "$TMP/fakebin"
  cat > "$TMP/fakebin/sd-state" <<'EOF'
#!/usr/bin/env bash
cat "$FX/state.json"
EOF
  cat > "$TMP/fakebin/gh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$FX/gh.calls"
filter=""; args=("$@"); for ((i = 0; i < $#; i++)); do [[ ${args[i]} == --jq ]] && filter=${args[i+1]}; done
case "$1 $2" in
  "api repos/{owner}/{repo}/commits/"*"/status") src=$FX/status.json ;;
  "api repos/{owner}/{repo}/commits/"*"/check-runs") src=$FX/checks.json ;;
  "run view") cat "$FX/gha.log"; exit ;;
  *) echo "gh stub: unexpected call: $*" >&2; exit 1 ;;
esac
if [[ -n $filter ]]; then jq -r "$filter" "$src"; else cat "$src"; fi   # gh's --jq is jq
EOF
  cat > "$TMP/fakebin/rwx" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$FX/rwx.calls"
case $1 in
  results) cat "$FX/rwx-results.json" ;;
  logs) out=""; while (( $# )); do [[ $1 == --output ]] && out=$2; shift; done
        mkdir -p "$out" && cp "$FX/rwx.log" "$out/test.log" && echo "Logs downloaded to $out/" ;;
  *) exit 1 ;;
esac
EOF
  cat > "$TMP/fakebin/claude" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$@" > "$FX/claude.args"
args=("$@") prompt= i=0
while (( i < $# )); do
  case ${args[i]} in
    --allowedTools|--disallowedTools) i=$((i + 1)); while (( i < $# )) && [[ ${args[i]} != -* ]]; do i=$((i + 1)); done; continue ;;
    --session-id|--resume|--output-format|--json-schema|--permission-mode|--model|--effort|--max-turns|--max-budget-usd) i=$((i + 2)); continue ;;
    -*) ;;
    *) prompt=${args[i]} ;;
  esac
  i=$((i + 1))
done
printf '%s' "$prompt" > "$FX/claude.prompt"
[[ -x $FX/agent ]] && "$FX/agent"
jq -n --argjson r "$(cat "$FX/result.json")" '{type: "result", is_error: false, structured_output: $r}'
EOF
  chmod +x "$TMP/fakebin/"*
  export PATH="$TMP/fakebin:$BATS_TEST_DIRNAME:$PATH"   # the fakes first: sd-state is one

  git init -q --bare "$TMP/origin.git"
  git clone -q "$TMP/origin.git" "$TMP/repo" 2>/dev/null
  cd "$TMP/repo"
  printf 'one\ntwo\n' > file.txt
  git add file.txt && git commit -qm init && git push -q origin main 2>/dev/null
  git switch -qc feat
  printf 'one\ntwo\nthree\n' > file.txt
  git commit -qam "add three" && git push -q -u origin feat 2>/dev/null
  git config branch.feat.sd-task "add a third line"

  state_fixture
  echo '{"state":"failure","statuses":[{"context":"RWX: tests.yml (pull request)","state":"failure","description":"Failed in 24s","target_url":"https://cloud.rwx.com/clabs/runs/01a1run"}]}' > "$FX/status.json"
  echo '{"total_count":2,"check_runs":[
    {"name":"test","status":"completed","conclusion":"failure","details_url":"https://github.com/o/r/actions/runs/555/job/777","app":{"slug":"github-actions"},"output":{"title":"1 test failed","summary":"FAIL: TestThree wants THREE","text":null}},
    {"name":"lint","status":"completed","conclusion":"failure","details_url":"https://github.com/o/r/actions/runs/556/job/778","app":{"slug":"github-actions"},"output":{"title":null,"summary":null,"text":null}}]}' > "$FX/checks.json"
  echo '{"ID":"01a1run","ResultStatus":"failed","Tasks":[{"Key":"lint","ID":"task-ok","Status":{"Result":"succeeded"}},{"Key":"test","ID":"task-bad","Status":{"Result":"failed"}}]}' > "$FX/rwx-results.json"
  printf 'rwx line 1\nrwx: expected THREE, got three\n' > "$FX/rwx.log"
  printf 'gha line 1\ngha: --- FAIL: TestThree\n' > "$FX/gha.log"
  agent_fixes
}

# state_fixture [jq-assignments...]: sd state's JSON: open PR, in sync, CI red on HEAD.
state_fixture() {
  local j a head main
  head=$(git rev-parse HEAD) main=$(git rev-parse main)
  j=$(jq -n --arg head "$head" --arg main "$main" '{
    branch: "feat", head: $head,
    pr: {number: 7, url: "https://example/pr/7", state: "OPEN", draft: false, base: "main"},
    parent: {name: "main", merged: false, pr: null},
    sync: {target: "main", default: "main", ahead: 1, behind: 0, up_to_date: true, old_base: $main, conflict: false},
    push: {remote_sha: $head, ahead: 0, behind: 0, diverged: false, remote_ours: true},
    fresh: true,
    ci: {sha: $head, for_head: true, verdict: "red", failing: ["RWX: tests.yml (pull request)", "test"], pending: []},
    threads: {owed_bot: 0, owed_human: 0, outdated: 0, blocking: 0, info: 0, items: []},
    session: {enrolled: true, harness: null, id: null, locked: false}, errors: []}')
  for a; do j=$(jq "$a" <<<"$j"); done
  printf '%s\n' "$j" > "$FX/state.json"
}

agent_result() { jq -n "$1" > "$FX/result.json"; }

# Default agent: uppercases three, which is what the logs ask for.
agent_fixes() {
  printf '#!/usr/bin/env bash\nprintf "one\\ntwo\\nTHREE\\n" > file.txt\n' > "$FX/agent"
  chmod +x "$FX/agent"
  agent_result '{outcome: "fixed", commit_message: "Uppercase three", summary: "the test wants THREE"}'
}
agent_edits_nothing() { rm -f "$FX/agent"; }

remote_feat() { git -C "$TMP/origin.git" rev-parse feat; }
prompt() { cat "$FX/claude.prompt"; }
recorded() { git config --get branch.feat.sd-ci-fix; }

# ── gates ────────────────────────────────────────────────────────────────────

@test "CI green: nothing to fix, nothing runs" {
  state_fixture '.ci.verdict = "green" | .ci.failing = []'
  run "$SD_ACT" ci-fix
  [ "$status" -ne 0 ]
  [[ "$output" == *"CI is green"* ]]
  [ ! -e "$FX/claude.args" ]
}

@test "CI inconclusive (cancelled): rerun it instead" {
  state_fixture '.ci.verdict = "inconclusive" | .ci.failing = []'
  run "$SD_ACT" ci-fix
  [ "$status" -ne 0 ]
  [[ "$output" == *"rerun"* ]]
  [ ! -e "$FX/claude.args" ]
}

@test "red on an old head: push first; the fix may already be there" {
  state_fixture '.ci.for_head = false | .ci.sha = "0000000000000000000000000000000000000000"'
  run "$SD_ACT" ci-fix
  [ "$status" -ne 0 ]
  [[ "$output" == *"not HEAD"* ]]
  [ ! -e "$FX/claude.args" ]
}

@test "unpushed commits: push first" {
  state_fixture '.push.ahead = 1'
  run "$SD_ACT" ci-fix
  [ "$status" -ne 0 ]
  [[ "$output" == *"unpushed"* ]]
  [[ "$output" == *"sd act push"* ]]
  [ ! -e "$FX/claude.args" ]
}

@test "no PR, or a closed one: refuses" {
  state_fixture '.pr = null'
  run "$SD_ACT" ci-fix
  [ "$status" -ne 0 ]
  [[ "$output" == *"no PR"* ]]
  state_fixture '.pr.state = "MERGED"'
  run "$SD_ACT" ci-fix
  [ "$status" -ne 0 ]
  [[ "$output" == *"merged"* ]]
}

@test "a dirty worktree is refused before anything runs" {
  echo dirty >> file.txt
  run "$SD_ACT" ci-fix
  [ "$status" -ne 0 ]
  [[ "$output" == *"uncommitted changes"* ]]
}

# ── the step ─────────────────────────────────────────────────────────────────

@test "red: the agent gets the failing checks with their logs; the fix is committed, verified, pushed, recorded" {
  "$SD_CONFIG" set --global settings.verify "grep -q THREE file.txt"
  head=$(git rev-parse HEAD)
  run "$SD_ACT" ci-fix
  [ "$status" -eq 0 ]
  p=$(prompt)
  [[ $p == *"RWX: tests.yml (pull request)"* ]]
  [[ $p == *"test"* ]]
  [[ $p == *"Failed in 24s"* ]]                          # the status's description
  [[ $p == *"rwx: expected THREE, got three"* ]]          # rwx logs of the failed task
  [[ $p == *"1 test failed"* ]]                           # the check run's output title
  [[ $p == *"FAIL: TestThree wants THREE"* ]]             # and summary
  [[ $p == *"gha: --- FAIL: TestThree"* ]]                # gh run view --log-failed
  [[ $p == *"add a third line"* ]]                        # the task
  [[ $p == *"grep -q THREE file.txt"* ]]                  # the repo's checks
  [ "$(git log -1 --format=%s)" = "Uppercase three" ]
  [ "$(remote_feat)" = "$(git rev-parse HEAD)" ]
  [ "$(recorded)" = "$head fixed" ]
  [[ "$output" == *"pushed"* ]]
  grep -q '^results 01a1run --json' "$FX/rwx.calls"
  grep -q '^logs task-bad --output' "$FX/rwx.calls"
  ! grep -q 'task-ok' "$FX/rwx.calls" || false
  grep -q '^run view 555 --job 777 --log-failed' "$FX/gh.calls"
  grep -q '^run view 556 --job 778 --log-failed' "$FX/gh.calls"   # a check run with no output text still gets its log
  ! grep -q graphql "$FX/gh.calls" || false
}

@test "no settings.verify: the agent is told no local checks exist, not that they pass" {
  run "$SD_ACT" ci-fix
  [ "$status" -eq 0 ]
  [[ "$(prompt)" == *"No local checks are configured"* ]]
  [[ "$(prompt)" != *"pass locally"* ]]
}

@test "verify changing the tree is refused before the agent runs" {
  "$SD_CONFIG" set --global settings.verify "echo touched >> file.txt"
  run "$SD_ACT" ci-fix
  [ "$status" -ne 0 ]
  [[ "$output" == *"changed files"* ]]
  [ ! -e "$FX/claude.args" ]
}

@test "the agent may run the verify command, and is told what it may run" {
  "$SD_CONFIG" set --global settings.verify "grep -q THREE file.txt"
  run "$SD_ACT" ci-fix
  [ "$status" -eq 0 ]
  allow=$(grep -A1 -x -- --allowedTools "$FX/claude.args" | tail -1)
  [[ $allow == *"Bash(grep -q THREE file.txt)"* ]]
  [[ "$(prompt)" == *"Bash(grep -q THREE file.txt)"* ]]
}

@test "logs that can't be fetched are skipped, not fatal" {
  printf '#!/usr/bin/env bash\nexit 1\n' > "$TMP/fakebin/rwx"
  rm "$FX/gha.log"
  run "$SD_ACT" ci-fix
  [ "$status" -eq 0 ]
  [[ "$(prompt)" == *"RWX: tests.yml (pull request)"* ]]
  [ "$(remote_feat)" = "$(git rev-parse HEAD)" ]
}

@test "logs are cut to their last lines" {
  seq 1 400 | sed 's/^/gha /' > "$FX/gha.log"
  run "$SD_ACT" ci-fix
  [ "$status" -eq 0 ]
  [[ "$(prompt)" == *"gha 400"* ]]
  [[ "$(prompt)" != *"gha 1"$'\n'* ]]
}

@test "autofix fixes what verify saw: committed and pushed, no agent" {
  "$SD_CONFIG" set --global settings.verify "grep -q THREE file.txt"
  "$SD_CONFIG" set --global settings.autofix "sed -i.bak s/three/THREE/ file.txt && rm file.txt.bak"
  run "$SD_ACT" ci-fix
  [ "$status" -eq 0 ]
  [ ! -e "$FX/claude.args" ]
  [[ "$output" == *"autofix committed"* ]]
  [ "$(git log -1 --format=%s)" = "autofix: sed -i.bak s/three/THREE/ file.txt && rm file.txt.bak" ]
  [ "$(remote_feat)" = "$(git rev-parse HEAD)" ]
}

@test "verify failing locally: its output goes to the agent" {
  "$SD_CONFIG" set --global settings.verify "echo 'local: three should be THREE'; grep -q THREE file.txt"
  run "$SD_ACT" ci-fix
  [ "$status" -eq 0 ]
  [[ "$(prompt)" == *"local: three should be THREE"* ]]
}

@test "verify passing locally: the agent is told so" {
  "$SD_CONFIG" set --global settings.verify "true"
  run "$SD_ACT" ci-fix
  [ "$status" -eq 0 ]
  [[ "$(prompt)" == *"pass locally"* ]]
}

@test "flaky after editing files: not recorded, so no --force is needed once they are cleaned up" {
  agent_result '{outcome: "flaky", commit_message: null, summary: "flaky"}'
  run "$SD_ACT" ci-fix
  [ "$status" -ne 0 ]
  [[ "$output" == *"edited files"* ]]
  run recorded
  [ "$status" -ne 0 ]
}

@test "flaky: nothing committed or pushed; recorded; the same red SHA is refused next time, a new one runs" {
  agent_edits_nothing
  agent_result '{outcome: "flaky", commit_message: null, summary: "a timeout in the runner, not the change"}'
  head=$(git rev-parse HEAD) before=$(remote_feat)
  run "$SD_ACT" ci-fix
  [ "$status" -eq 0 ]
  [[ "$output" == *"flaky"* ]]
  [[ "$output" == *"a timeout in the runner"* ]]
  [ "$(git rev-parse HEAD)" = "$head" ]
  [ "$(remote_feat)" = "$before" ]
  [ "$(recorded)" = "$head flaky" ]
  rm "$FX/claude.args"
  run "$SD_ACT" ci-fix
  [ "$status" -ne 0 ]
  [[ "$output" == *"already"* ]]
  [[ "$output" == *"flaky"* ]]
  [ ! -e "$FX/claude.args" ]
  git commit -q --allow-empty -m "retry" && git push -q origin feat 2>/dev/null
  state_fixture
  run "$SD_ACT" ci-fix
  [ "$status" -eq 0 ]
  [ -e "$FX/claude.args" ]
}

@test "--force runs again on a SHA already judged flaky" {
  agent_edits_nothing
  agent_result '{outcome: "flaky", commit_message: null, summary: "flaky"}'
  run "$SD_ACT" ci-fix
  rm "$FX/claude.args"
  run "$SD_ACT" ci-fix --force
  [ "$status" -eq 0 ]
  [ -e "$FX/claude.args" ]
}

@test "fixed but changed nothing: fails" {
  agent_edits_nothing
  run "$SD_ACT" ci-fix
  [ "$status" -ne 0 ]
  [[ "$output" == *"changed nothing"* ]]
}

@test "needs-input and gave-up: edits left uncommitted, nothing pushed" {
  before=$(remote_feat)
  agent_result '{outcome: "needs-input", commit_message: null, summary: "which test is right?"}'
  run "$SD_ACT" ci-fix
  [ "$status" -ne 0 ]
  [[ "$output" == *"which test is right?"* ]]
  [ -n "$(git status --porcelain)" ]
  git checkout -q file.txt
  agent_result '{outcome: "gave-up", commit_message: null, summary: "no idea"}'
  run "$SD_ACT" ci-fix
  [ "$status" -ne 0 ]
  [[ "$output" == *"no idea"* ]]
  [ "$(remote_feat)" = "$before" ]
}

@test "--no-publish: committed, not pushed" {
  before=$(remote_feat)
  run "$SD_ACT" ci-fix --no-publish
  [ "$status" -eq 0 ]
  [ "$(git log -1 --format=%s)" = "Uppercase three" ]
  [ "$(remote_feat)" = "$before" ]
  [[ "$output" == *"not pushed"* ]]
}

@test "verify failing after the fix: commit kept, nothing pushed" {
  "$SD_CONFIG" set --global settings.verify "grep -q FOUR file.txt"
  before=$(remote_feat)
  run "$SD_ACT" ci-fix
  [ "$status" -ne 0 ]
  [[ "$output" == *"verify failed"* ]]
  [ "$(git log -1 --format=%s)" = "Uppercase three" ]
  [ "$(remote_feat)" = "$before" ]
}

# ── CI config: held for chrismo (#124) ───────────────────────────────────────

agent_touches_ci() {
  printf '#!/usr/bin/env bash\nprintf "one\\ntwo\\nTHREE\\n" > file.txt\nmkdir -p .github/workflows && echo "on: push" > .github/workflows/test.yml\n' > "$FX/agent"
  chmod +x "$FX/agent"
}
held() { git config --get branch.feat.sd-held; }

@test "a fix that changes CI config is committed, not pushed, and held; chrismo's sd act push releases it (#124)" {
  agent_touches_ci
  before=$(remote_feat)
  run "$SD_ACT" ci-fix
  [ "$status" -ne 0 ]
  [[ "$output" == *"the agent changed CI config (.github/workflows/test.yml): review, then sd act push"* ]]
  [ "$(git log -1 --format=%s)" = "Uppercase three" ]
  [ "$(remote_feat)" = "$before" ]
  [ "$(held)" = "$(git rev-parse HEAD)"$'\n'".github/workflows/test.yml" ]
  run "$SD_ACT" push
  [ "$status" -eq 0 ]
  [ "$(remote_feat)" = "$(git rev-parse HEAD)" ]
  run held
  [ "$status" -eq 1 ]
}

@test "a CI config change the agent committed itself is held, whatever it answers (#124)" {
  printf '#!/usr/bin/env bash\nmkdir -p .github/workflows && echo x > .github/workflows/x.yml && git add -A && git commit -qm "agent made it"\n' > "$FX/agent"
  chmod +x "$FX/agent"
  for r in '{outcome: "fixed", commit_message: null, summary: "s"}' '{outcome: "flaky", commit_message: null, summary: "s"}' \
           '{outcome: "needs-input", commit_message: null, summary: "s"}'; do
    git reset -q --hard origin/feat && git config --unset branch.feat.sd-held || true
    git config --unset branch.feat.sd-ci-fix || true
    agent_result "$r"
    run "$SD_ACT" ci-fix
    [ "$(held)" = "$(git rev-parse HEAD)"$'\n'".github/workflows/x.yml" ] || { echo "for: $r"; return 1; }
  done
}

@test "a CI config change the agent committed itself is held even when the step then fails (#124)" {
  printf '#!/usr/bin/env bash\nmkdir -p .github/workflows && echo x > .github/workflows/x.yml && git add -A && git commit -qm "agent made it"\n' > "$FX/agent"
  chmod +x "$FX/agent"
  echo 'not json' > "$FX/result.json"
  run "$SD_ACT" ci-fix
  [ "$status" -ne 0 ]
  [ "$(held)" = "$(git rev-parse HEAD)"$'\n'".github/workflows/x.yml" ]
}

@test "--no-publish with a CI config change: held all the same (#124)" {
  agent_touches_ci
  run "$SD_ACT" ci-fix --no-publish
  [[ "$output" == *"the agent changed CI config (.github/workflows/test.yml)"* ]]
  [ -n "$(held)" ]
}

@test "settings.protectedPaths [] holds nothing: the fix is pushed (#124)" {
  agent_touches_ci
  "$SD_CONFIG" set --global --json settings.protectedPaths '[]'
  run "$SD_ACT" ci-fix
  [ "$status" -eq 0 ]
  [ "$(remote_feat)" = "$(git rev-parse HEAD)" ]
  run held
  [ "$status" -eq 1 ]
}

@test "the fix is committed under the agent's message, else the first failing check" {
  agent_result '{outcome: "fixed", commit_message: "", summary: "done"}'
  run "$SD_ACT" ci-fix
  [ "$status" -eq 0 ]
  [ "$(git log -1 --format=%s)" = "Fix CI: RWX: tests.yml (pull request)" ]
}

# ── the prompt ───────────────────────────────────────────────────────────────

@test "the shipped prompt is used by default" {
  run "$SD_ACT" ci-fix
  [ "$status" -eq 0 ]
  [[ "$(prompt)" == *"$(head -1 "$BATS_TEST_DIRNAME/../lib/prompts/ci-fix.md")"* ]]
}

@test "settings.ciFixPrompt names a file in the repo, read from the base commit" {
  git switch -q main
  echo "OUR CI RULES: run rwx run first" > ci-fix.md
  git add ci-fix.md && git commit -qm "ci guidance" && git push -q origin main 2>/dev/null
  git switch -q feat
  state_fixture   # old_base follows main
  "$SD_CONFIG" set --global settings.ciFixPrompt ci-fix.md
  run "$SD_ACT" ci-fix
  [ "$status" -eq 0 ]
  [[ "$(prompt)" == *"OUR CI RULES: run rwx run first"* ]]
  [[ "$(prompt)" != *"$(head -1 "$BATS_TEST_DIRNAME/../lib/prompts/ci-fix.md")"* ]]
}

@test "settings.ciFixPrompt: an absolute path is read from disk; a missing file is an error" {
  echo "ABSOLUTE RULES" > "$TMP/rules.md"
  "$SD_CONFIG" set --global settings.ciFixPrompt "$TMP/rules.md"
  run "$SD_ACT" ci-fix
  [ "$status" -eq 0 ]
  [[ "$(prompt)" == *"ABSOLUTE RULES"* ]]
  "$SD_CONFIG" set --global settings.ciFixPrompt "$TMP/nope.md"
  run "$SD_ACT" ci-fix
  [ "$status" -ne 0 ]
  [[ "$output" == *"no such file"* ]]
}

@test "no old_base from sd state: the branch's own .sheepdog/ci-fix.md is ignored" {
  mkdir -p .sheepdog && echo "BRANCH-WRITTEN CI RULES" > .sheepdog/ci-fix.md
  git add .sheepdog && git commit -qm "the agent's guidance" && git push -q origin feat 2>/dev/null
  state_fixture '.sync.old_base = null'
  run "$SD_ACT" ci-fix
  [ "$status" -eq 0 ]
  [[ "$(prompt)" != *"BRANCH-WRITTEN CI RULES"* ]]
  [[ "$(prompt)" == *"$(head -1 "$BATS_TEST_DIRNAME/../lib/prompts/ci-fix.md")"* ]]
}

@test "no old_base and no default branch to trust: a relative ciFixPrompt is refused, not read from the tip" {
  echo "BRANCH-WRITTEN CI RULES" > ci-fix.md
  git add ci-fix.md && git commit -qm "the agent's guidance" && git push -q origin feat 2>/dev/null
  state_fixture '.sync.old_base = null'
  git -C "$TMP/origin.git" update-ref -d refs/heads/main   # ci-fix fetches
  git update-ref -d refs/remotes/origin/main && git branch -qD main
  "$SD_CONFIG" set --global settings.ciFixPrompt ci-fix.md
  run "$SD_ACT" ci-fix
  [ "$status" -ne 0 ]
  [[ "$output" == *"no base commit to read ci-fix.md from"* ]]
  [ ! -e "$FX/claude.prompt" ]
}

@test "--help lists ci-fix" {
  run "$SD_ACT" --help
  [[ "$output" =~ sd\ act\ ci-fix\  ]]
}
