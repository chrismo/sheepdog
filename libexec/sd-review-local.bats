#!/usr/bin/env bats

bats_require_minimum_version 1.5.0

# Test suite for libexec/sd-review-local, the default local reviewer (A8): one
# fresh, read-only model session over the diff SHEEPDOG_BASE..SHEEPDOG_HEAD,
# findings as a JSON list on stdout. claude and pi are stubs on PATH that
# record their arguments and print $FX/result.json.

REVIEW="$BATS_TEST_DIRNAME/sd-review-local"
SD_CONFIG="$BATS_TEST_DIRNAME/sd-config"

setup() {
  TMP="$(cd "$BATS_TEST_TMPDIR" && pwd -P)"
  export GIT_CONFIG_GLOBAL="$TMP/gitconfig-global"
  export GIT_CONFIG_SYSTEM=/dev/null
  export SHEEPDOG_STATE_DIR="$TMP/state"   # harness_run logs usage there
  printf '[user]\n\temail = t@example.com\n\tname = Tester\n[init]\n\tdefaultBranch = main\n' > "$GIT_CONFIG_GLOBAL"
  export FX="$TMP/fx"
  export SHEEPDOG_CONFIG="$TMP/global.json"
  mkdir -p "$FX" "$TMP/fakebin"
  cat > "$TMP/fakebin/claude" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$@" > "$FX/claude.args"
args=("$@") prompt= i=0
while (( i < $# )); do
  case ${args[i]} in
    --allowedTools|--disallowedTools)
      i=$((i + 1))
      while (( i < $# )) && [[ ${args[i]} != -* ]]; do i=$((i + 1)); done
      continue ;;
    --session-id|--resume|--output-format|--json-schema|--permission-mode|--model|--effort|--max-turns|--max-budget-usd)
      i=$((i + 2)); continue ;;
    -*) ;;
    *) prompt=${args[i]} ;;
  esac
  i=$((i + 1))
done
printf '%s' "$prompt" > "$FX/claude.prompt"
echo "$PWD" > "$FX/claude.cwd"
[[ -e $FX/fail ]] && { echo "claude: boom" >&2; exit 1; }
[[ -e $FX/earlier.json ]] && jq -n --argjson r "$(cat "$FX/earlier.json")" '{type: "result", is_error: false, structured_output: $r}'
jq -n --argjson r "$(cat "$FX/result.json")" '{type: "result", is_error: false, structured_output: $r}'
EOF
  cat > "$TMP/fakebin/pi" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$@" > "$FX/pi.args"
printf 'Done.\n\n```json\n%s\n```\n' "$(cat "$FX/result.json")"
EOF
  chmod +x "$TMP/fakebin/claude" "$TMP/fakebin/pi"
  export PATH="$TMP/fakebin:$PATH"
  echo '{"findings": []}' > "$FX/result.json"

  git init -q "$TMP/repo" && cd "$TMP/repo"
  printf 'one\ntwo\n' > file.txt && git add file.txt && git commit -qm init
  BASE=$(git rev-parse HEAD)
  printf 'one\ntwo\nthree added\n' > file.txt && git commit -qam "add three"
  HEAD_SHA=$(git rev-parse HEAD)
  export SHEEPDOG_BASE=$BASE SHEEPDOG_HEAD=$HEAD_SHA SHEEPDOG_TASK="add a third line"
}

@test "is a link to the Rust binary, which runs as the verb it's named for" {
  [ "$(readlink "$REVIEW")" = ../target/release/sd ]
  [ -x "$REVIEW" ]
}

@test "without SHEEPDOG_BASE and SHEEPDOG_HEAD: fails and says so" {
  unset SHEEPDOG_HEAD
  run "$REVIEW"
  [ "$status" -ne 0 ]
  [[ "$output" == *SHEEPDOG_HEAD* ]]
}

@test "prints the reviewer's findings as a JSON list" {
  echo '{"findings": [{"file": "file.txt", "line": 3, "severity": "warning", "body": "no newline"}]}' > "$FX/result.json"
  run --separate-stderr "$REVIEW"
  [ "$status" -eq 0 ]
  [ "$(jq -c . <<<"$output")" = '[{"file":"file.txt","line":3,"severity":"warning","body":"no newline"}]' ]
}

@test "no findings: an empty list, exit 0" {
  run --separate-stderr "$REVIEW"
  [ "$status" -eq 0 ]
  [ "$(jq -c . <<<"$output")" = '[]' ]
}

@test "the prompt carries the diff between base and head, and the task" {
  "$REVIEW" >/dev/null
  grep -q '^+three added' "$FX/claude.prompt"
  grep -q 'add a third line' "$FX/claude.prompt"
  [ "$(cat "$FX/claude.cwd")" = "$TMP/repo" ]
}

@test "a fresh session each run, read-only tools, a schema" {
  "$REVIEW" >/dev/null
  grep -qx -- --session-id "$FX/claude.args"
  ! grep -qx -- --resume "$FX/claude.args" || false
  grep -qx -- --json-schema "$FX/claude.args"
  allow=$(grep -A1 -x -- --allowedTools "$FX/claude.args" | tail -1)
  [[ $allow != *Edit* && $allow != *Write* ]]
  [[ $allow == *Read* ]]
}

@test "the reviewer failing is non-zero, with nothing on stdout" {
  touch "$FX/fail"
  run --separate-stderr "$REVIEW"
  [ "$status" -ne 0 ]
  [ -z "$output" ]
}

@test "a result without a findings list is a failure, never clean" {
  echo '{"verdict": "fine"}' > "$FX/result.json"
  run --separate-stderr "$REVIEW"
  [ "$status" -ne 0 ]
  [ -z "$output" ]
}

@test "steps.review picks the harness: pi, with read-only tools" {
  "$SD_CONFIG" set --global steps.review.harness pi
  echo '{"findings": [{"file": "file.txt", "line": 3, "severity": "nit", "body": "x"}]}' > "$FX/result.json"
  run --separate-stderr "$REVIEW"
  [ "$status" -eq 0 ]
  [ "$(jq length <<<"$output")" = 1 ]
  tools=$(grep -A1 -x -- --tools "$FX/pi.args" | tail -1)
  [ "$tools" = read ]
}

@test "says when the reviewer starts: which harness, which session, what range" {
  run --separate-stderr "$REVIEW"
  [ "$status" -eq 0 ]
  sid=$(grep -A1 -x -- --session-id "$FX/claude.args" | tail -1)
  [[ "$stderr" == *"claude (new session $sid) is reviewing main (${BASE:0:7}..${HEAD_SHA:0:7})"* ]]
}

@test "settings.reviewPrompt empty: the shipped guidance and the reporting rules" {
  "$REVIEW" >/dev/null
  grep -q 'Find what a careful reviewer would flag' "$FX/claude.prompt"
  grep -q 'No findings is a valid' "$FX/claude.prompt"
}

@test "settings.reviewPrompt: a custom file replaces the guidance, the reporting rules stay" {
  printf 'Look only for off-by-one errors.\n' > "$TMP/guide.md"
  "$SD_CONFIG" set --global settings.reviewPrompt "$TMP/guide.md"
  "$REVIEW" >/dev/null
  grep -q 'Look only for off-by-one errors.' "$FX/claude.prompt"
  ! grep -q 'Find what a careful reviewer would flag' "$FX/claude.prompt" || false
  grep -q 'No findings is a valid' "$FX/claude.prompt"
  grep -q '^+three added' "$FX/claude.prompt"
}

@test "settings.reviewPrompt: a relative path is read from the base commit, not the worktree" {
  git switch -q --detach "$BASE"
  mkdir -p .sheepdog sub
  printf 'Base guidance.\n' > .sheepdog/review-prompt.md
  git add .sheepdog && git commit -qm "review guidance"
  BASE=$(git rev-parse HEAD)
  printf 'Weaker guidance.\n' > .sheepdog/review-prompt.md
  git commit -qam "the branch rewrites it"
  export SHEEPDOG_BASE=$BASE SHEEPDOG_HEAD=$(git rev-parse HEAD)
  "$SD_CONFIG" set --global settings.reviewPrompt .sheepdog/review-prompt.md
  (cd sub && "$REVIEW" >/dev/null)
  [ "$(head -1 "$FX/claude.prompt")" = "Base guidance." ]
}

@test "settings.reviewPrompt: a missing file fails, nothing on stdout, no review run" {
  "$SD_CONFIG" set --global settings.reviewPrompt nope.md
  run --separate-stderr "$REVIEW"
  [ "$status" -ne 0 ]
  [ -z "$output" ]
  [[ "$stderr" == *nope.md* ]]
  [ ! -e "$FX/claude.args" ]
}

@test "settings.reviewPrompt: a relative path only the worktree has is missing" {
  mkdir -p .sheepdog && printf 'Uncommitted.\n' > .sheepdog/review-prompt.md
  "$SD_CONFIG" set --global settings.reviewPrompt .sheepdog/review-prompt.md
  run --separate-stderr "$REVIEW"
  [ "$status" -ne 0 ]
  [ -z "$output" ]
  [[ "$stderr" == *".sheepdog/review-prompt.md"* ]]
  [ ! -e "$FX/claude.args" ]
}

@test "settings.reviewPrompt: ./ is the worktree's top, wherever the reviewer runs" {
  mkdir -p .sheepdog sub/.sheepdog
  printf 'Top guidance.\n' > .sheepdog/review-prompt.md
  printf 'Sub guidance.\n' > sub/.sheepdog/review-prompt.md
  git add . && git commit -qm guidance
  export SHEEPDOG_BASE=$(git rev-parse HEAD) SHEEPDOG_HEAD=$(git rev-parse HEAD)
  "$SD_CONFIG" set --global settings.reviewPrompt ./.sheepdog/review-prompt.md
  (cd sub && "$REVIEW" >/dev/null)
  [ "$(head -1 "$FX/claude.prompt")" = "Top guidance." ]
}

@test "settings.reviewPrompt: a directory is not a guidance file" {
  mkdir -p .sheepdog && printf 'x\n' > .sheepdog/review-prompt.md
  git add . && git commit -qm guidance
  export SHEEPDOG_BASE=$(git rev-parse HEAD) SHEEPDOG_HEAD=$(git rev-parse HEAD)
  "$SD_CONFIG" set --global settings.reviewPrompt .sheepdog
  run --separate-stderr "$REVIEW"
  [ "$status" -ne 0 ]
  [ ! -e "$FX/claude.args" ]
}

@test "settings.reviewPrompt: a missing absolute path fails, no review run" {
  "$SD_CONFIG" set --global settings.reviewPrompt "$TMP/nope.md"
  run --separate-stderr "$REVIEW"
  [ "$status" -ne 0 ]
  [[ "$stderr" == *"$TMP/nope.md"* ]]
  [ ! -e "$FX/claude.args" ]
}

@test "settings.reviewPrompt empty: .sheepdog/review.md in the base commit replaces the shipped guidance" {
  git switch -q --detach "$BASE"
  mkdir -p .sheepdog && printf 'Repo review guidance.\n' > .sheepdog/review.md
  git add .sheepdog && git commit -qm guidance
  export SHEEPDOG_BASE=$(git rev-parse HEAD)
  echo x >> file.txt && git commit -qam change
  export SHEEPDOG_HEAD=$(git rev-parse HEAD)
  "$REVIEW" >/dev/null
  [ "$(head -1 "$FX/claude.prompt")" = "Repo review guidance." ]
  grep -q 'No findings is a valid' "$FX/claude.prompt"
}

@test "two results from the harness: stdout is still one findings list, the last" {
  echo '{"findings": [{"file": "a", "line": 1, "severity": "nit", "body": "earlier"}]}' > "$FX/earlier.json"
  echo '{"findings": [{"file": "b", "line": 2, "severity": "nit", "body": "last"}]}' > "$FX/result.json"
  run --separate-stderr "$REVIEW"
  [ "$status" -eq 0 ]
  [ "$(printf '%s\n' "$output" | wc -l | tr -d ' ')" -eq 1 ]
  [ "$(jq -r '.[0].body' <<<"$output")" = last ]
}
