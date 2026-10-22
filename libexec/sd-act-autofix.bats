#!/usr/bin/env bats

bats_require_minimum_version 1.5.0

# Test suite for `sd act autofix` (A11) as the agent steps run it: --help,
# and implement, which runs autofix when verify fails, then verifies again
# (address hooks in the same way, through the same function). The verb
# itself is tested in tests/act_commands.rs (#202).
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
