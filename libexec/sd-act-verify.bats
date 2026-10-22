#!/usr/bin/env bats

bats_require_minimum_version 1.5.0

# Test suite for `sd act verify` (A10): run settings.verify in the branch's
# worktree at zero tokens. Pass: one line. Fail: nonzero, the command's output
# trimmed to its tail for the fix step that follows. Gates local review.
#
# Real git; verify commands are shell one-liners. gh is a stub that fails.

SD_ACT="$BATS_TEST_DIRNAME/sd-act"
SD_CONFIG="$BATS_TEST_DIRNAME/sd-config"

setup() {
  TMP="$(cd "$BATS_TEST_TMPDIR" && pwd -P)"
  export GIT_CONFIG_GLOBAL="$TMP/gitconfig-global"
  export GIT_CONFIG_SYSTEM=/dev/null
  printf '[user]\n\temail = t@example.com\n\tname = Tester\n[init]\n\tdefaultBranch = main\n' > "$GIT_CONFIG_GLOBAL"
  export SHEEPDOG_CONFIG="$TMP/global.json"
  mkdir -p "$TMP/fakebin"
  printf '#!/usr/bin/env bash\necho "gh called: $*" >&2\nexit 1\n' > "$TMP/fakebin/gh"
  chmod +x "$TMP/fakebin/gh"
  export PATH="$TMP/fakebin:$PATH"

  git init -q "$TMP/repo"
  cd "$TMP/repo"
  printf '%s\n' a b c > file.txt
  git add file.txt && git commit -qm init
  git switch -qc feat
}

verify_cmd() { "$SD_CONFIG" set --global settings.verify "$1"; }

@test "no settings.verify: says so and passes" {
  run "$SD_ACT" verify
  [ "$status" -eq 0 ]
  [[ "$output" == *"no settings.verify"* ]]
}

@test "passing: one line, the command's output is not shown" {
  verify_cmd "seq 1 3"
  run "$SD_ACT" verify
  [ "$status" -eq 0 ]
  [[ "$output" == *"verify passed"* ]]
  ! grep -qx 2 <<<"$output"
}

@test "failing: nonzero, the output trimmed to its last 40 lines" {
  verify_cmd "seq 1 60; echo 'FAIL: the end' >&2; exit 2"
  run "$SD_ACT" verify
  [ "$status" -ne 0 ]
  [[ "$output" == *"verify failed"* ]]
  [[ "$output" == *"FAIL: the end"* ]]
  [[ "$output" == *"last 40 of 61 lines"* ]]
  grep -qx 60 <<<"$output"
  ! grep -qx 1 <<<"$output"
}

@test "failing: short output is shown whole, untrimmed" {
  verify_cmd "echo only line; false"
  run "$SD_ACT" verify
  [ "$status" -ne 0 ]
  [[ "$output" == *"only line"* ]]
  [[ "$output" != *"last 40"* ]]
}

@test "runs in the branch's worktree, not the current one" {
  git worktree add -q "$TMP/wt" -b other
  touch "$TMP/wt/marker"
  verify_cmd "test -f marker"
  run "$SD_ACT" verify other
  [ "$status" -eq 0 ]
  run "$SD_ACT" verify feat
  [ "$status" -ne 0 ]
}

@test "a branch checked out nowhere fails" {
  git branch parked
  verify_cmd true
  run "$SD_ACT" verify parked
  [ "$status" -ne 0 ]
  [[ "$output" == *"not checked out"* ]]
}

@test "a command that changes the tree fails, even when it exits 0" {
  verify_cmd "echo formatted >> file.txt"
  run "$SD_ACT" verify
  [ "$status" -ne 0 ]
  [[ "$output" == *"changed files"* ]]
}

@test "a list-valued verify is refused before anything runs" {
  run "$SD_CONFIG" set --global --json settings.verify '["true","false"]'
  [ "$status" -eq 1 ]
  printf '%s\n' '{"settings":{"verify":["true","false"]}}' > "$SHEEPDOG_CONFIG"
  run "$SD_ACT" verify
  [ "$status" -ne 0 ]
  [[ "$output" == *'settings.verify: expected a string, got ["true","false"]'* ]]
}

@test "the branch file's verify beats the global file's" {
  verify_cmd true
  "$SD_CONFIG" set settings.verify false
  run "$SD_ACT" verify
  [ "$status" -ne 0 ]
}

@test "runs under settings.stepTimeout" {
  verify_cmd "sleep 5"
  "$SD_CONFIG" set --global --json settings.stepTimeout 1
  run "$SD_ACT" verify
  [ "$status" -ne 0 ]
  [[ "$output" == *"timed out"* ]]
}

@test "a TERM or INT to it ends the command, and it waits for that (#170)" {
  local sig job i
  for sig in TERM INT; do
    rm -f "$TMP/cmd"
    verify_cmd "echo \$\$ > '$TMP/cmd.tmp' && mv '$TMP/cmd.tmp' '$TMP/cmd' && exec sleep 30"
    # Job control, so the job doesn't start with INT ignored.
    set -m
    "$SD_ACT" verify &
    job=$!
    set +m
    for i in $(seq 100); do [ -s "$TMP/cmd" ] && break; sleep 0.1; done
    kill -"$sig" "$job"
    wait "$job" || true
    run kill -0 "$(cat "$TMP/cmd")"
    [ "$status" -ne 0 ]
  done
}
