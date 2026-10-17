#!/usr/bin/env bats

bats_require_minimum_version 1.5.0

# Test suite for `sd act describe` (J6: the agent writes the PR title and
# body, read-only, stored for HEAD) and `sd act ship` (describe if needed,
# recheck HEAD, push, open a draft PR on the parent or fix up the existing
# one, enroll).
#
# Real git (a bare origin, our clone, $TMP/other for other people's pushes)
# and the real sd state. gh is a stub: graphql answers from $GH/<branch>.json
# (by number: the node of whichever file has it),
# `gh pr create|edit|ready` are logged to $GH/calls. claude prints
# $FX/result.json as structured output.

SD_ACT="$BATS_TEST_DIRNAME/sd-act"
SD_CONFIG="$BATS_TEST_DIRNAME/sd-config"

setup() {
  TMP="$(cd "$BATS_TEST_TMPDIR" && pwd -P)"
  export GIT_CONFIG_GLOBAL="$TMP/gitconfig-global"
  export GIT_CONFIG_SYSTEM=/dev/null
  export SHEEPDOG_STATE_DIR="$TMP/state"   # harness_run logs usage there
  printf '[user]\n\temail = t@example.com\n\tname = Tester\n[init]\n\tdefaultBranch = main\n' > "$GIT_CONFIG_GLOBAL"
  export SHEEPDOG_CONFIG="$TMP/global.json"
  export GH="$TMP/gh" FX="$TMP/fx"
  mkdir -p "$GH" "$FX" "$TMP/fakebin"
  cat > "$TMP/fakebin/gh" <<'EOF'
#!/usr/bin/env bash
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
EOF
  cat > "$TMP/fakebin/claude" <<'EOF'
#!/usr/bin/env bash
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
EOF
  chmod +x "$TMP/fakebin/gh" "$TMP/fakebin/claude"
  export PATH="$BATS_TEST_DIRNAME:$TMP/fakebin:$PATH"
  echo '{"title": "Add c", "body": "Adds the third line.\n\nRefs #1"}' > "$FX/result.json"

  git init -q --bare "$TMP/origin.git"
  git clone -q "$TMP/origin.git" "$TMP/repo" 2>/dev/null
  cd "$TMP/repo"
  printf 'a\nb\n' > file.txt && git add file.txt && git commit -qm init && git push -q origin main 2>/dev/null
  git clone -q "$TMP/origin.git" "$TMP/other" 2>/dev/null
  git switch -qc feat
  printf 'a\nb\nc\n' > file.txt && git commit -qam "add c"
}

open_pr() { # open_pr <branch> <base> <number>
  jq -n --arg b "$1" --arg base "$2" --argjson n "$3" --arg h "$(git rev-parse "$1")" '{data: {viewer: {login: "me"},
    repository: {defaultBranchRef: {name: "main"},
      pullRequests: {nodes: [{number: $n, state: "OPEN", isDraft: true, baseRefName: $base, headRefName: $b,
                              headRefOid: $h, isCrossRepository: false}]}}}}' > "$GH/$1.json"
}
calls() { cat "$GH/calls" 2>/dev/null; }
remote() { git ls-remote origin "refs/heads/$1" | cut -f1; }

# ── describe ─────────────────────────────────────────────────────────────────

@test "describe: the agent writes title and body, read-only, from the task and diff" {
  git config branch.feat.sd-task "add a third line"
  run --separate-stderr "$SD_ACT" describe
  [ "$status" -eq 0 ]
  [ "$(jq -r .title <<<"$output")" = "Add c" ]
  grep -q '^+c$' "$FX/claude.prompt"
  grep -q 'add a third line' "$FX/claude.prompt"
  grep -qx -- --json-schema "$FX/claude.args"
  allow=$(grep -A1 -x -- --allowedTools "$FX/claude.args" | tail -1)
  [[ $allow != *Edit* && $allow != *Write* ]]
}

@test "describe: settings.describePrompt replaces the guidance; read-only stays" {
  git config branch.feat.sd-task "add a third line"
  printf 'Titles in French.\n' > "$TMP/describe.md"
  echo "{\"settings\": {\"describePrompt\": \"$TMP/describe.md\"}}" > "$SHEEPDOG_CONFIG"
  "$SD_ACT" describe >/dev/null
  [ "$(head -1 "$FX/claude.prompt")" = "Titles in French." ]
  ! grep -q 'at most 70 characters' "$FX/claude.prompt" || false
  grep -q 'You are read-only here' "$FX/claude.prompt"
  grep -q '^+c$' "$FX/claude.prompt"
}

@test "describe: stored for HEAD, and ship reuses it without calling the agent" {
  "$SD_ACT" describe >/dev/null
  rm -f "$FX/claude.args"
  run "$SD_ACT" ship
  [ "$status" -eq 0 ]
  [ ! -e "$FX/claude.args" ]
  [[ "$(calls)" == *"--title Add c"* ]]
}

@test "describe: mints the branch's session, later runs resume it" {
  "$SD_ACT" describe >/dev/null
  sid=$(git config branch.feat.sd-session)
  [ -n "$sid" ]
  grep -qx -- --session-id "$FX/claude.args"
  "$SD_ACT" describe >/dev/null
  grep -qx -- --resume "$FX/claude.args"
  grep -qx -- "$sid" "$FX/claude.args"
}

# transcript <session-id> <tokens>: a claude transcript whose last turn's context is <tokens>.
transcript() {
  mkdir -p "$CLAUDE_CONFIG_DIR/projects/-repo"
  printf '{"message":{"role":"assistant","usage":{"input_tokens":1,"cache_creation_input_tokens":0,"cache_read_input_tokens":%s,"output_tokens":0}},"type":"assistant"}\n' \
    "$(( $2 - 1 ))" > "$CLAUDE_CONFIG_DIR/projects/-repo/$1.jsonl"
}

@test "describe: a session past settings.forkLimit tokens (100000) isn't resumed; a new one starts (#121)" {
  export CLAUDE_CONFIG_DIR="$TMP/claude"
  "$SD_ACT" describe >/dev/null
  sid=$(git config branch.feat.sd-session)
  transcript "$sid" 100000
  "$SD_ACT" describe >/dev/null
  grep -qx -- --resume "$FX/claude.args"
  transcript "$sid" 100001
  run --separate-stderr "$SD_ACT" describe
  [ "$status" -eq 0 ]
  [[ "$stderr" == *"session $sid is 100001 tokens, past settings.forkLimit (100000): starting a new one"* ]]
  grep -qx -- --session-id "$FX/claude.args"
  new=$(git config branch.feat.sd-session)
  [ "$new" != "$sid" ]
  grep -qx -- "$new" "$FX/claude.args"
  # 0 is no limit
  transcript "$new" 900000
  "$SD_CONFIG" set --global --json settings.forkLimit 0
  "$SD_ACT" describe >/dev/null
  grep -qx -- --resume "$FX/claude.args"
  grep -qx -- "$new" "$FX/claude.args"
}

# ── ship ─────────────────────────────────────────────────────────────────────

@test "ship: no PR yet: describe, push, a draft PR on the parent, enroll" {
  run "$SD_ACT" ship
  [ "$status" -eq 0 ]
  [ "$(remote feat)" = "$(git rev-parse HEAD)" ]
  c=$(calls)
  [[ $c == "pr create --draft --base main --head feat --title Add c --body "* ]]
  [[ $c == *"Refs #1"* ]]
  [[ "$output" == *"https://example/pr/9"* ]]
  [ "$(git config branch.feat.sheepdog)" = true ]
}

@test "ship: held CI config is refused, even with --force, before describe or push; after sd act push it ships (#124)" {
  git config branch.feat.sd-held "$(git rev-parse HEAD)"$'\n'".github/workflows/test.yml"
  run "$SD_ACT" ship --force
  [ "$status" -ne 0 ]
  [[ "$output" == *"not shipping feat: the agent changed CI config (.github/workflows/test.yml): review, then sd act push"* ]]
  [ -z "$(remote feat)" ]
  [ ! -e "$FX/claude.args" ]
  [ -z "$(calls)" ]
  run "$SD_ACT" push
  [ "$status" -eq 0 ]
  [[ "$output" == *"released the CI config hold on feat"* ]]
  run "$SD_ACT" ship
  [ "$status" -eq 0 ]
}

@test "ship: a hold whose commit is already on the remote (pushed some other way) is cleared, not a refusal (#124)" {
  git push -q origin feat 2>/dev/null
  git config branch.feat.sd-held "$(git rev-parse HEAD)"$'\n'".github/workflows/test.yml"
  run "$SD_ACT" ship
  [ "$status" -eq 0 ]
  run git config --get branch.feat.sd-held
  [ "$status" -eq 1 ]
}

@test "ship: a stacked branch opens its PR on the parent" {
  git switch -q main && git switch -qc A && git commit -q --allow-empty -m A && git push -q origin A 2>/dev/null
  git switch -q feat && git rebase -q A && git config branch.feat.sd-parent A
  run "$SD_ACT" ship
  [ "$status" -eq 0 ]
  [[ "$(calls)" == "pr create --draft --base A --head feat "* ]]
  grep -q 'Base: this PR is stacked on A' "$FX/claude.prompt"
}

@test "describe: told the PR targets main when nothing is stacked (#105)" {
  "$SD_ACT" describe >/dev/null
  grep -q 'Base: this PR targets main\.' "$FX/claude.prompt"
}

@test "ship: a PR stacked on A with no sd-parent stays on A, and describe says so (#105)" {
  git switch -q main && git switch -qc A && git commit -q --allow-empty -m A && git push -q origin A 2>/dev/null
  git switch -q feat && git rebase -q A && git push -q -u origin feat 2>/dev/null
  open_pr feat A 7
  echo '{"data":{"viewer":{"login":"me"},"repository":{"defaultBranchRef":{"name":"main"},"pullRequests":{"nodes":[{"number": 3, "state": "OPEN", "baseRefName": "main", "headRefName": "A", "headRefOid": "'"$(git rev-parse A)"'", "isCrossRepository": false}]}}}}' > "$GH/A.json"
  run "$SD_ACT" ship
  [ "$status" -eq 0 ]
  grep -q 'Base: this PR is stacked on A' "$FX/claude.prompt"
  [[ "$(calls)" == "pr edit 7 "* ]]
  [[ "$(calls)" != *"--base"* ]]
}

@test "ship: a parent that landed on main by hand, never on origin, isn't shipped again (#105)" {
  git switch -q main && git switch -qc A && git commit -q --allow-empty -m A
  git switch -q main && git merge -q --ff-only A && git push -q origin main 2>/dev/null
  git switch -q feat && git rebase -q A && git config branch.feat.sd-parent A
  run "$SD_ACT" ship
  [ "$status" -ne 0 ]
  [[ "$output" == *"A has no commits beyond origin/main"*"point sd-parent at main"* ]]
  [[ "$output" != *"sd act ship A"* ]]
  [ -z "$(calls)" ]
}

@test "ship: a branch made from another's tip with no recorded parent: records it and opens its PR on it (#94)" {
  git switch -q main && git switch -qc A && git commit -q --allow-empty -m A && git push -q origin A 2>/dev/null
  git switch -qc kid && git commit -q --allow-empty -m kid
  run "$SD_ACT" ship
  [ "$status" -eq 0 ]
  [ "$(git config branch.kid.sd-parent)" = A ]
  [[ "$(calls)" == "pr create --draft --base A --head kid "* ]]
}

@test "ship: gh failing on a second lookup still opens an unrecorded stack's PR on its parent (#94)" {
  git switch -q main && git switch -qc A && git commit -q --allow-empty -m A && git push -q origin A 2>/dev/null
  git switch -qc kid && git commit -q --allow-empty -m kid
  touch "$GH/fail-kid-after-1"
  run "$SD_ACT" ship
  [ "$status" -eq 0 ]
  [ "$(git config branch.kid.sd-parent)" = A ]
  [[ "$(calls)" == "pr create --draft --base A --head kid "* ]]
}

@test "ship: an unrecorded stack git can't orient (the parent moved on) is refused before describe or push (#94)" {
  git switch -q main && git switch -qc A && git commit -q --allow-empty -m A1
  git switch -qc kid && git commit -q --allow-empty -m kid
  git switch -q A && git commit -q --allow-empty -m A2 && git push -q origin A 2>/dev/null
  git switch -q kid
  run "$SD_ACT" ship
  [ "$status" -ne 0 ]
  [[ "$output" == *"git can't tell which, if either, was made from the other"* ]]
  [ -z "$(git config branch.kid.sd-parent || true)" ]
  [ ! -e "$FX/claude.args" ]
  [ -z "$(remote kid)" ]
  [ -z "$(calls)" ]
}

@test "ship: a branch a scratch branch was made from still ships onto the default branch (#94)" {
  git switch -q main && git switch -qc A && git commit -q --allow-empty -m A1
  git switch -qc scratch && git commit -q --allow-empty -m s1
  git switch -q A && git commit -q --allow-empty -m A2
  run "$SD_ACT" ship
  [ "$status" -eq 0 ]
  [ -z "$(git config branch.A.sd-parent || true)" ]
  [[ "$(calls)" == "pr create --draft --base main --head A "* ]]
}

@test "ship: a stale description (HEAD moved) is written again" {
  "$SD_ACT" describe >/dev/null
  echo d >> file.txt && git commit -qam "add d"
  echo '{"title": "Add c and d", "body": "both"}' > "$FX/result.json"
  run "$SD_ACT" ship
  [ "$status" -eq 0 ]
  [[ "$(calls)" == *"--title Add c and d"* ]]
}

@test "ship: the PR exists: push, then fix up title, body and base" {
  git push -q -u origin feat 2>/dev/null
  echo d >> file.txt && git commit -qam "add d"
  open_pr feat main 7
  git switch -q main && git switch -qc A && git commit -q --allow-empty -m A && git push -q origin A 2>/dev/null
  git switch -q feat && git rebase -q A && git config branch.feat.sd-parent A
  run "$SD_ACT" ship
  [ "$status" -eq 0 ]
  [ "$(remote feat)" = "$(git rev-parse HEAD)" ]
  [[ "$(calls)" == "pr edit 7 --title Add c --body "*"--base A"* ]]
  [[ "$output" == *"#7"* ]]
}

@test "ship: the PR exists and already has that base: no base edit" {
  git push -q -u origin feat 2>/dev/null
  open_pr feat main 7
  run "$SD_ACT" ship
  [ "$status" -eq 0 ]
  [[ "$(calls)" != *"--base"* ]]
}

@test "ship: local review not covering HEAD is a warning, not a refusal" {
  run --separate-stderr "$SD_ACT" ship
  [ "$status" -eq 0 ]
  [[ "$stderr" == *"local review"* ]]
}

@test "ship: a parent that isn't on the remote is refused before describe or push" {
  git switch -q main && git switch -qc A && git commit -q --allow-empty -m A   # never pushed
  git switch -q feat && git config branch.feat.sd-parent A
  run "$SD_ACT" ship
  [ "$status" -ne 0 ]
  [[ "$output" == *"A isn't on origin"*"sd act ship A"* ]]
  [ ! -e "$FX/claude.args" ]
  [ -z "$(remote feat)" ]
  [ -z "$(calls)" ]
}

@test "ship: a parent that has merged and is gone from the remote: restack first, not ship it" {
  git switch -q main && git switch -qc A && git commit -q --allow-empty -m A && git push -q origin A 2>/dev/null
  a_head=$(git rev-parse A)
  git switch -q feat && git config branch.feat.sd-parent A
  (cd "$TMP/other" && git fetch -q origin && git checkout -q main && git merge -q --squash origin/A \
     && git commit -qm "A (#1)" --allow-empty && git push -q origin main 2>/dev/null && git push -q origin :A 2>/dev/null)
  jq -n --arg h "$a_head" '{data: {viewer: {login: "me"}, repository: {defaultBranchRef: {name: "main"},
    pullRequests: {nodes: [{number: 1, state: "MERGED", baseRefName: "main", headRefName: "A", headRefOid: $h, isCrossRepository: false}]}}}}' > "$GH/A.json"
  run "$SD_ACT" ship
  [ "$status" -ne 0 ]
  [[ "$output" == *"A has merged"*"sd act restack"* ]]
  [ ! -e "$FX/claude.args" ]
  [ -z "$(remote feat)" ]
  [ -z "$(calls)" ]
}

@test "ship: a parent that has merged but is still on the remote: restack first, nothing pushed" {
  git switch -q main && git switch -qc A && git commit -q --allow-empty -m A && git push -q origin A 2>/dev/null
  a_head=$(git rev-parse A)
  git switch -q feat && git config branch.feat.sd-parent A
  (cd "$TMP/other" && git fetch -q origin && git checkout -q main && git merge -q --squash origin/A \
     && git commit -qm "A (#1)" --allow-empty && git push -q origin main 2>/dev/null)
  jq -n --arg h "$a_head" '{data: {viewer: {login: "me"}, repository: {defaultBranchRef: {name: "main"},
    pullRequests: {nodes: [{number: 1, state: "MERGED", baseRefName: "main", headRefName: "A", headRefOid: $h, isCrossRepository: false}]}}}}' > "$GH/A.json"
  run "$SD_ACT" ship
  [ "$status" -ne 0 ]
  [[ "$output" == *"A has merged"*"sd act restack"* ]]
  [ ! -e "$FX/claude.args" ]
  [ -z "$(remote feat)" ]
  [ -z "$(calls)" ]
}

@test "ship: a parent closed without merging is refused: decide what the branch targets" {
  git switch -q main && git switch -qc A && git commit -q --allow-empty -m A && git push -q origin A 2>/dev/null
  a_head=$(git rev-parse A)
  git switch -q feat && git config branch.feat.sd-parent A
  jq -n --arg h "$a_head" '{data: {viewer: {login: "me"}, repository: {defaultBranchRef: {name: "main"},
    pullRequests: {nodes: [{number: 1, state: "CLOSED", baseRefName: "main", headRefName: "A", headRefOid: $h, isCrossRepository: false}]}}}}' > "$GH/A.json"
  run "$SD_ACT" ship
  [ "$status" -ne 0 ]
  [[ "$output" == *"A (#1) was closed without merging"*"sd-parent"* ]]
  [ ! -e "$FX/claude.args" ]
  [ -z "$(remote feat)" ]
  [ -z "$(calls)" ]
}

@test "ship: a parent merged by fast-forward, so the branch is already on main: ships onto main and rewrites sd-parent" {
  git switch -q main && git switch -qc A && git commit -q --allow-empty -m A && git push -q origin A 2>/dev/null
  a_head=$(git rev-parse A)
  git switch -q feat && git rebase -q A && git config branch.feat.sd-parent A
  (cd "$TMP/other" && git fetch -q origin && git checkout -q main && git merge -q --ff-only origin/A \
     && git push -q origin main 2>/dev/null && git push -q origin :A 2>/dev/null)
  jq -n --arg h "$a_head" '{data: {viewer: {login: "me"}, repository: {defaultBranchRef: {name: "main"},
    pullRequests: {nodes: [{number: 1, state: "MERGED", baseRefName: "main", headRefName: "A", headRefOid: $h, isCrossRepository: false}]}}}}' > "$GH/A.json"
  run "$SD_ACT" ship
  [ "$status" -eq 0 ]
  [[ "$(calls)" == "pr create --draft --base main --head feat "* ]]
  [ "$(git config branch.feat.sd-parent)" = main ]
  grep -q 'Base: A has merged into main, so this PR targets main' "$FX/claude.prompt"
}

@test "ship: a description written while stacked is written again once the parent merged, HEAD unchanged (#105)" {
  git switch -q main && git switch -qc A && git commit -q --allow-empty -m A && git push -q origin A 2>/dev/null
  a_head=$(git rev-parse A)
  git switch -q feat && git rebase -q A && git config branch.feat.sd-parent A
  "$SD_ACT" describe >/dev/null
  grep -q 'Base: this PR is stacked on A' "$FX/claude.prompt"
  (cd "$TMP/other" && git fetch -q origin && git checkout -q main && git merge -q --ff-only origin/A \
     && git push -q origin main 2>/dev/null && git push -q origin :A 2>/dev/null)
  jq -n --arg h "$a_head" '{data: {viewer: {login: "me"}, repository: {defaultBranchRef: {name: "main"},
    pullRequests: {nodes: [{number: 1, state: "MERGED", baseRefName: "main", headRefName: "A", headRefOid: $h, isCrossRepository: false}]}}}}' > "$GH/A.json"
  echo '{"title": "Add c on main", "body": "A has landed."}' > "$FX/result.json"
  run "$SD_ACT" ship
  [ "$status" -eq 0 ]
  grep -q 'Base: A has merged into main' "$FX/claude.prompt"
  [[ "$(calls)" == "pr create --draft --base main --head feat --title Add c on main "* ]]
  # sd-parent is main now; the PR's target hasn't changed, so the body stands
  rm -f "$FX/claude.args"
  open_pr feat main 9
  run "$SD_ACT" ship
  [ "$status" -eq 0 ]
  [ ! -e "$FX/claude.args" ]
}

@test "describe: a parent only here says to ship it first, not an empty reason (#105)" {
  git switch -q main && git switch -qc A && git commit -q --allow-empty -m A
  git switch -q feat && git rebase -q A && git config branch.feat.sd-parent A
  run "$SD_ACT" describe
  [ "$status" -ne 0 ]
  [[ "$output" == *"A isn't on origin yet: ship it first (sd act ship A)"* ]]
}

@test "ship: a parent whose PR can't be looked up is refused, never shipped onto" {
  git switch -q main && git switch -qc A && git commit -q --allow-empty -m A && git push -q origin A 2>/dev/null
  git switch -q feat && git config branch.feat.sd-parent A
  touch "$GH/fail-A"
  run "$SD_ACT" ship
  [ "$status" -ne 0 ]
  [[ "$output" == *"can't tell whether A merged"* ]]
  [ ! -e "$FX/claude.args" ]
  [ -z "$(remote feat)" ]
  [ -z "$(calls)" ]
}

@test "ship: a merged or closed PR is refused" {
  open_pr feat main 7
  jq '.data.repository.pullRequests.nodes[0].state = "MERGED"' "$GH/feat.json" > "$GH/t" && mv "$GH/t" "$GH/feat.json"
  run "$SD_ACT" ship
  [ "$status" -ne 0 ]
  [[ "$output" == *"merged"* ]]
  [ -z "$(calls)" ]
}

@test "ship: commits on the remote this copy never had: refused before any PR call" {
  git push -q -u origin feat 2>/dev/null
  (cd "$TMP/other" && git fetch -q origin && git checkout -q -B feat origin/feat && echo x > x.txt && git add x.txt \
     && git commit -qm theirs && git push -q origin feat 2>/dev/null)
  echo d >> file.txt && git commit -qam "add d"
  run "$SD_ACT" ship
  [ "$status" -ne 0 ]
  [ -z "$(calls)" ]
}

@test "ship: the describe step failing stops before the push" {
  touch "$FX/fail"
  run "$SD_ACT" ship
  [ "$status" -ne 0 ]
  [ -z "$(remote feat)" ]
  [ -z "$(calls)" ]
}

@test "ship: HEAD moving during describe is refused; nothing pushed" {
  printf '#!/usr/bin/env bash\necho sneaky >> file.txt && git commit -qam sneaky\n' > "$FX/agent"
  chmod +x "$FX/agent"
  run "$SD_ACT" ship
  [ "$status" -ne 0 ]
  [[ "$output" == *"HEAD moved"* ]]
  [ -z "$(remote feat)" ]
}

@test "ship: uncommitted changes are refused" {
  echo d >> file.txt
  run "$SD_ACT" ship
  [ "$status" -ne 0 ]
  [[ "$output" == *"uncommitted"* ]]
}

@test "ship: gh pr create failing is an error after the push" {
  touch "$GH/fail-create"
  run "$SD_ACT" ship
  [ "$status" -ne 0 ]
  [ "$(remote feat)" = "$(git rev-parse HEAD)" ]
}

@test "ship: stacked on an open parent it isn't built on, no pin: refused, says how to pin (#97)" {
  git switch -q main && git switch -qc A && git commit -q --allow-empty -m A && git push -q origin A 2>/dev/null
  git switch -q feat && git config branch.feat.sd-parent A
  run "$SD_ACT" ship
  [ "$status" -ne 0 ]
  [[ "$output" == *"can't find feat's base: no base for feat on A: no base pin"*"git update-ref refs/sheepdog/feat/base"* ]]
  [ -z "$(calls)" ]
}

# ── ship: the upstream (#39) ─────────────────────────────────────────────────

tracks() { git rev-parse --abbrev-ref "$1@{upstream}" 2>/dev/null; }

@test "ship: twice on a published branch tracking origin/main: tracks origin/feat, nothing else changes" {
  git switch -q main && git switch -qc A && git commit -q --allow-empty -m A && git push -q origin A 2>/dev/null
  git switch -q feat && git rebase -q A && git config branch.feat.sd-parent A
  git push -q origin feat 2>/dev/null
  git branch -q --set-upstream-to=origin/main feat
  open_pr feat A 7
  "$SD_ACT" describe >/dev/null
  rm -f "$FX/claude.args"
  head=$(git rev-parse feat)
  for _ in 1 2; do
    run "$SD_ACT" ship
    [ "$status" -eq 0 ]
    [[ "$output" == *"nothing to push on feat"* ]]
    [ "$(tracks feat)" = origin/feat ]
  done
  [ ! -e "$FX/claude.args" ]
  [ "$(git rev-parse feat)" = "$head" ]
  [ "$(remote feat)" = "$head" ]
  [ "$(git config branch.feat.sd-parent)" = A ]
  [ "$(calls | grep -c '^pr create')" -eq 0 ]
  [ "$(calls | grep -c '^pr edit 7 ')" -eq 2 ]
  [[ "$(calls)" != *"--base"* ]]
}

@test "ship: retried after gh pr create failed: the PR opens, tracking origin/feat" {
  git branch -q --set-upstream-to=origin/main feat
  touch "$GH/fail-create"
  run "$SD_ACT" ship
  [ "$status" -ne 0 ]
  head=$(git rev-parse feat)
  [ "$(remote feat)" = "$head" ]
  rm "$GH/fail-create"
  git branch -q --set-upstream-to=origin/main feat  # as an sd from before #39 left it
  run "$SD_ACT" ship
  [ "$status" -eq 0 ]
  [[ "$output" == *"nothing to push on feat"* ]]
  [[ "$output" == *"https://example/pr/9"* ]]
  [ "$(tracks feat)" = origin/feat ]
  [ "$(git rev-parse feat)" = "$head" ]
}
