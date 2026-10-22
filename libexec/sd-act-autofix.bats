#!/usr/bin/env bats

bats_require_minimum_version 1.5.0

# Test suite for `sd act autofix` (A11): run settings.autofix (formatters,
# lint --fix) in the branch's worktree at zero tokens, under
# settings.stepTimeout, and commit what it changed. A failing command is
# nonzero with its output's tail, and leaves its changes uncommitted. implement
# runs it when verify fails, then verifies again (address hooks in the same
# way, through the same function).
#
# Real git; autofix commands are shell one-liners. gh is a stub that fails;
# claude is a stub whose edit $FX/agent makes, for the implement hook.

SD_ACT="$BATS_TEST_DIRNAME/sd-act"
SD_CONFIG="$BATS_TEST_DIRNAME/sd-config"

setup() {
  TMP="$(cd "$BATS_TEST_TMPDIR" && pwd -P)"
  export GIT_CONFIG_GLOBAL="$TMP/gitconfig-global"
  export GIT_CONFIG_SYSTEM=/dev/null
  export SHEEPDOG_STATE_DIR="$TMP/state"   # harness_run logs usage there
  printf '[user]\n\temail = t@example.com\n\tname = Tester\n[init]\n\tdefaultBranch = main\n' > "$GIT_CONFIG_GLOBAL"
  export SHEEPDOG_CONFIG="$TMP/global.json" FX="$TMP/fx"
  mkdir -p "$TMP/fakebin" "$FX"
  printf '#!/usr/bin/env bash\necho "gh called: $*" >&2\nexit 1\n' > "$TMP/fakebin/gh"
  cat > "$TMP/fakebin/claude" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$@" > "$FX/claude.args"
[[ -x $FX/agent ]] && "$FX/agent"
jq -n --argjson r "$(cat "$FX/result.json")" '{type: "result", is_error: false, structured_output: $r}'
EOF
  chmod +x "$TMP/fakebin/"*
  export PATH="$BATS_TEST_DIRNAME:$TMP/fakebin:$PATH"

  git init -q "$TMP/repo"
  cd "$TMP/repo"
  printf '%s\n' a b c > file.txt
  git add file.txt && git commit -qm init
  git switch -qc feat
}

autofix_cmd() { "$SD_CONFIG" set --global settings.autofix "$1"; }
head_sha() { git rev-parse HEAD; }

@test "no settings.autofix: says so and passes" {
  run "$SD_ACT" autofix
  [ "$status" -eq 0 ]
  [[ "$output" == *"no settings.autofix"* ]]
}

@test "a formatter that changes the tree: committed, the message names the command" {
  autofix_cmd "printf '%s\n' a b c d > file.txt"
  before=$(head_sha)
  run "$SD_ACT" autofix
  [ "$status" -eq 0 ]
  [[ "$output" == *"autofix committed"* ]]
  [ "$(head_sha)" != "$before" ]
  [ -z "$(git status --porcelain)" ]
  [ "$(git log -1 --format=%s)" = "autofix: printf '%s\n' a b c d > file.txt" ]
  [ "$(git show --stat --format= HEAD | tail -1)" = " 1 file changed, 1 insertion(+)" ]
}

@test "new files the formatter makes are committed too" {
  autofix_cmd "echo gen > generated.txt"
  run "$SD_ACT" autofix
  [ "$status" -eq 0 ]
  [ -z "$(git status --porcelain)" ]
  git cat-file -e HEAD:generated.txt
}

@test "a clean tree: nothing committed, says so" {
  autofix_cmd "true"
  before=$(head_sha)
  run "$SD_ACT" autofix
  [ "$status" -eq 0 ]
  [[ "$output" == *"autofix changed nothing"* ]]
  [ "$(head_sha)" = "$before" ]
}

@test "a failing command: nonzero, the output trimmed to its last 40 lines, nothing committed" {
  autofix_cmd "seq 1 60; echo 'lint: unfixable' >&2; exit 2"
  before=$(head_sha)
  run "$SD_ACT" autofix
  [ "$status" -ne 0 ]
  [[ "$output" == *"autofix failed"* ]]
  [[ "$output" == *"lint: unfixable"* ]]
  [[ "$output" == *"last 40 of 61 lines"* ]]
  grep -qx 60 <<<"$output"
  [ "$(head_sha)" = "$before" ]
}

@test "a failing command that changed files leaves them uncommitted, and says so" {
  autofix_cmd "echo half >> file.txt; exit 1"
  before=$(head_sha)
  run "$SD_ACT" autofix
  [ "$status" -ne 0 ]
  [[ "$output" == *"left uncommitted"* ]]
  [ "$(head_sha)" = "$before" ]
  [ -n "$(git status --porcelain)" ]
}

@test "uncommitted changes are refused before anything runs" {
  autofix_cmd "echo x > ran"
  echo dirty >> file.txt
  run "$SD_ACT" autofix
  [ "$status" -ne 0 ]
  [[ "$output" == *"uncommitted changes"* ]]
  [ ! -e ran ]
}

@test "runs in the branch's worktree, not the current one" {
  git worktree add -q "$TMP/wt" -b other
  autofix_cmd "echo fixed >> file.txt"
  run "$SD_ACT" autofix other
  [ "$status" -eq 0 ]
  [ "$(git -C "$TMP/wt" log -1 --format=%s)" = "autofix: echo fixed >> file.txt" ]
  [ "$(git log -1 --format=%s)" = init ]
}

@test "a branch checked out nowhere fails" {
  git branch parked
  autofix_cmd true
  run "$SD_ACT" autofix parked
  [ "$status" -ne 0 ]
  [[ "$output" == *"not checked out"* ]]
}

@test "a list-valued autofix is refused before anything runs" {
  run "$SD_CONFIG" set --global --json settings.autofix '["true","false"]'
  [ "$status" -eq 1 ]
  printf '%s\n' '{"settings":{"autofix":["true","false"]}}' > "$SHEEPDOG_CONFIG"
  run "$SD_ACT" autofix
  [ "$status" -ne 0 ]
  [[ "$output" == *'settings.autofix: expected a string, got ["true","false"]'* ]]
}

@test "the branch file's autofix beats the global file's" {
  autofix_cmd "echo global >> file.txt"
  "$SD_CONFIG" set settings.autofix "echo branch >> file.txt"
  run "$SD_ACT" autofix
  [ "$status" -eq 0 ]
  [ "$(tail -1 file.txt)" = branch ]
}

@test "runs under settings.stepTimeout" {
  autofix_cmd "sleep 5"
  "$SD_CONFIG" set --global --json settings.stepTimeout 1
  run "$SD_ACT" autofix
  [ "$status" -ne 0 ]
  [[ "$output" == *"timed out"* ]]
}

@test "the commit's hooks are heard: a pre-commit hook's output passes through" {
  autofix_cmd "echo d >> file.txt"
  printf '#!/bin/sh\necho hook-out\necho hook-err >&2\n' > .git/hooks/pre-commit
  chmod +x .git/hooks/pre-commit
  run "$SD_ACT" autofix
  [ "$status" -eq 0 ]
  [[ "$output" == *"hook-out"* ]]
  [[ "$output" == *"hook-err"* ]]
  [[ "$output" == *"autofix committed"* ]]
}

@test "--help lists autofix" {
  run "$SD_ACT" --help
  [[ "$output" =~ sd\ act\ autofix\  ]]
}

# ── the implement hook: autofix when verify fails, then verify again ─────────

implement_setup() {
  git config branch.feat.sd-task "add a fourth line"
  echo '{"outcome": "done", "summary": "added d", "commit_message": "add d"}' > "$FX/result.json"
  printf '#!/usr/bin/env bash\nprintf "%%s\\n" a b c "d  " > file.txt\n' > "$FX/agent"   # trailing spaces: verify fails
  chmod +x "$FX/agent"
  "$SD_CONFIG" set --global settings.verify "! grep -q ' $' file.txt"
}

@test "implement: a failed verify runs autofix; when it changes the tree, verify runs again" {
  implement_setup
  autofix_cmd "sed -i.bak 's/ *$//' file.txt && rm file.txt.bak"
  run "$SD_ACT" implement
  [ "$status" -eq 0 ]
  [[ "$output" == *"autofix committed"* ]]
  [[ "$output" == *"verify passed"* ]]
  [ "$(git log -1 --format=%s)" = "autofix: sed -i.bak 's/ *$//' file.txt && rm file.txt.bak" ]
  [ "$(git log -2 --format=%s | tail -1)" = "add d" ]
  [ -z "$(git status --porcelain)" ]
}

@test "implement: a failed verify with no autofix fails as before" {
  implement_setup
  run "$SD_ACT" implement
  [ "$status" -ne 0 ]
  [[ "$output" == *"verify failed"* ]]
  [[ "$output" != *"autofix"* ]]
  [ "$(git log -1 --format=%s)" = "add d" ]
}

@test "implement: autofix that changes nothing doesn't rerun verify; the failure stands" {
  implement_setup
  autofix_cmd "true"
  run "$SD_ACT" implement
  [ "$status" -ne 0 ]
  [[ "$output" == *"autofix changed nothing"* ]]
  [[ "$output" == *"verify failed"* ]]
}

@test "implement: autofix failing after it changed files: reverted, the verify failure stands" {
  implement_setup
  autofix_cmd "echo half >> file.txt; exit 1"
  run "$SD_ACT" implement
  [ "$status" -ne 0 ]
  [[ "$output" == *"reverted"* ]]
  [[ "$output" == *"verify failed"* ]]
  [ -z "$(git status --porcelain)" ]
  [ "$(git log -1 --format=%s)" = "add d" ]
}

@test "implement: autofix commits but verify still fails: nonzero, the autofix commit stays" {
  implement_setup
  autofix_cmd "echo more >> file.txt"
  run "$SD_ACT" implement
  [ "$status" -ne 0 ]
  [[ "$output" == *"autofix committed"* ]]
  [[ "$output" == *"verify failed"* ]]
  [ "$(git log -1 --format=%s)" = "autofix: echo more >> file.txt" ]
}

@test "implement: verify dirtying the tree is verify's failure; autofix doesn't run" {
  implement_setup
  "$SD_CONFIG" set --global settings.verify "echo touched >> file.txt"
  autofix_cmd "echo ran > autofix-ran"
  run "$SD_ACT" implement
  [ "$status" -ne 0 ]
  [[ "$output" == *"changed files"* ]]
  [[ "$output" != *"clean tree"* ]]
  [ ! -e autofix-ran ]
}

@test "implement: verify passing first time never runs autofix" {
  implement_setup
  printf '#!/usr/bin/env bash\nprintf "%%s\\n" a b c d > file.txt\n' > "$FX/agent"
  autofix_cmd "echo ran > autofix-ran"
  run "$SD_ACT" implement
  [ "$status" -eq 0 ]
  [ ! -e autofix-ran ]
}
