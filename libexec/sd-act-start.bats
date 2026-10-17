#!/usr/bin/env bats

bats_require_minimum_version 1.5.0

# Test suite for `sd act start` (A0: a worktree beside the main worktree,
# named after the branch with slashes as dashes, set up by the repo's hook,
# with the task, parent and base recorded and the branch enrolled; then
# implement) and `sd act implement`
# (J0: the agent edits in the branch's session, the pipeline commits and runs
# verify).
#
# The worktree half duplicates agent-rig's wt-new (#14): same branch
# resolution, same hook locations, stdout exactly the path. Placement differs
# since #85: beside the main worktree, never beside a linked one.
#
# Real git (a bare origin, our clone at $TMP/repo, $TMP/other for pushes that
# aren't ours). gh is a stub with no PRs. claude edits through $FX/agent and
# prints $FX/result.json as structured output.

SD_ACT="$BATS_TEST_DIRNAME/sd-act"
SD_STATE="$BATS_TEST_DIRNAME/sd-state"

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
printf '%s\n' "$*" >> "$GH/calls"
echo '{"data":{"viewer":{"login":"me"},"repository":{"defaultBranchRef":{"name":"main"},"pullRequests":{"nodes":[]}}}}'
EOF
  cat > "$TMP/fakebin/claude" <<'EOF'
#!/usr/bin/env bash
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
EOF
  printf '#!/usr/bin/env bash\necho c >> file.txt\n' > "$FX/agent"
  chmod +x "$TMP/fakebin/gh" "$TMP/fakebin/claude" "$FX/agent"
  export PATH="$BATS_TEST_DIRNAME:$TMP/fakebin:$PATH"
  echo '{"outcome": "done", "commit_message": "Add c", "summary": "Added the third line."}' > "$FX/result.json"

  git init -q --bare "$TMP/origin.git"
  git clone -q "$TMP/origin.git" "$TMP/repo" 2>/dev/null
  cd "$TMP/repo"
  printf 'a\nb\n' > file.txt && git add file.txt && git commit -qm init && git push -q origin main 2>/dev/null
  git clone -q "$TMP/origin.git" "$TMP/other" 2>/dev/null
}

other_push() { # other_push <branch> <message>: a commit on origin that our clone hasn't fetched
  git -C "$TMP/other" switch -q "$1" 2>/dev/null || git -C "$TMP/other" switch -qc "$1"
  git -C "$TMP/other" commit -q --allow-empty -m "$2"
  git -C "$TMP/other" push -q origin "$1" 2>/dev/null
}
sha() { git rev-parse "$1"; }

# ── start: the worktree ──────────────────────────────────────────────────────

@test "start: no name is usage, nonzero" {
  run "$SD_ACT" start
  [ "$status" -ne 0 ]
  [[ "$output" == *"sd act start"* ]]
  [ ! -e "$TMP/feat-x" ]
}

@test "start: a worktree beside the main one, on a new branch of the same name; stdout is only its path" {
  run --separate-stderr "$SD_ACT" start --no-implement feat-x
  [ "$status" -eq 0 ]
  [ "$output" = "$TMP/feat-x" ]
  [ "$(git -C "$TMP/feat-x" branch --show-current)" = feat-x ]
}

@test "start: placed beside the main worktree, from a subdirectory too" {
  mkdir -p sub/deeper && cd sub/deeper
  run --separate-stderr "$SD_ACT" start --no-implement feat-x
  [ "$status" -eq 0 ]
  [ "$output" = "$TMP/feat-x" ]
}

@test "start: a new branch starts from the fetched origin default, not a stale local main" {
  other_push main "moved on"
  run --separate-stderr "$SD_ACT" start --no-implement feat-x
  [ "$status" -eq 0 ]
  [ "$(sha feat-x)" = "$(sha origin/main)" ]
  [ "$(sha feat-x)" != "$(sha main)" ]
}

@test "start: a new branch has no upstream until it's pushed" {
  "$SD_ACT" start --no-implement feat-x >/dev/null 2>&1
  run git config --get branch.feat-x.merge
  [ "$status" -ne 0 ]
}

@test "start: origin/HEAD names the default branch when it's set" {
  other_push trunk "trunk only"
  git fetch -q origin && git remote set-head origin trunk >/dev/null
  "$SD_ACT" start --no-implement feat-x >/dev/null 2>&1
  [ "$(sha feat-x)" = "$(sha origin/trunk)" ]
}

@test "start: an existing local branch gets the worktree, as it is" {
  git commit -q --allow-empty -m "local only" && git branch feat-x
  run --separate-stderr "$SD_ACT" start --no-implement feat-x
  [ "$status" -eq 0 ]
  [ "$(git -C "$TMP/feat-x" rev-parse HEAD)" = "$(sha main)" ]
  [ -z "$(git for-each-ref refs/sheepdog/feat-x/base)" ]
}

@test "start: a branch only on origin gets a local branch tracking it" {
  other_push feat-x "theirs"
  run --separate-stderr "$SD_ACT" start --no-implement feat-x
  [ "$status" -eq 0 ]
  [ "$(sha feat-x)" = "$(sha origin/feat-x)" ]
  [ "$(git config branch.feat-x.merge)" = refs/heads/feat-x ]
}

@test "start: refuses when the destination already exists, and creates no branch" {
  mkdir "$TMP/feat-x"
  run "$SD_ACT" start --no-implement feat-x
  [ "$status" -ne 0 ]
  [[ "$output" == *"$TMP/feat-x already exists"* ]]
  run git rev-parse -q --verify refs/heads/feat-x
  [ "$status" -ne 0 ]
}

@test "start: refuses a name git won't take as a branch" {
  run "$SD_ACT" start --no-implement "bad..name"
  [ "$status" -ne 0 ]
  [[ "$output" == *"not a valid branch name"* ]]
}

@test "start: git's chatter goes to stderr, never stdout" {
  run --separate-stderr "$SD_ACT" start --no-implement feat-x
  [ "$status" -eq 0 ]
  [ "$(wc -l <<<"$output" | tr -d ' ')" = 1 ]
}

# ── start: --parent ──────────────────────────────────────────────────────────

@test "start --parent: starts from the local parent, records sd-parent, pins base there" {
  git switch -qc A && git commit -q --allow-empty -m A && git switch -q main
  run --separate-stderr "$SD_ACT" start --no-implement --parent A feat-x
  [ "$status" -eq 0 ]
  [ "$(sha feat-x)" = "$(sha A)" ]
  [ "$(git config branch.feat-x.sd-parent)" = A ]
  [ "$(sha refs/sheepdog/feat-x/base)" = "$(sha A)" ]
}

@test "start --parent: a parent only on origin is used from there" {
  other_push A "A, theirs"
  run --separate-stderr "$SD_ACT" start --no-implement --parent A feat-x
  [ "$status" -eq 0 ]
  [ "$(sha feat-x)" = "$(sha origin/A)" ]
}

@test "start --parent: a parent that doesn't exist is refused before any worktree" {
  run "$SD_ACT" start --no-implement --parent nope feat-x
  [ "$status" -ne 0 ]
  [[ "$output" == *"no such parent branch: nope"* ]]
  [ ! -e "$TMP/feat-x" ]
}

# ── start: bookkeeping ───────────────────────────────────────────────────────

@test "start: records the task, pins base at the start point, enrolls the branch and the repo" {
  run --separate-stderr "$SD_ACT" start --no-implement feat-x "add a third line"
  [ "$status" -eq 0 ]
  [ "$(git config branch.feat-x.sd-task)" = "add a third line" ]
  [ "$(sha refs/sheepdog/feat-x/base)" = "$(sha origin/main)" ]
  [ "$(git config branch.feat-x.sheepdog)" = true ]
  [ "$(git config --global --get-all sheepdog.repo)" = "$TMP/repo" ]
}

@test "start: no session is minted until a step runs" {
  "$SD_ACT" start --no-implement feat-x "a task" >/dev/null 2>&1
  run git config --get branch.feat-x.sd-session
  [ "$status" -ne 0 ]
}

@test "start: without --no-implement the task is required" {
  run "$SD_ACT" start feat-x
  [ "$status" -ne 0 ]
  [[ "$output" == *"no task"* ]]
  [ ! -e "$TMP/feat-x" ]
}

# ── start: the setup hook (wt-new's two places) ──────────────────────────────

@test "start: runs \$GIT_COMMON_DIR/worktree-setup in the new worktree with WT_SRC, WT_DST, WT_BRANCH" {
  printf '#!/usr/bin/env bash\necho "$PWD|$WT_SRC|$WT_DST|$WT_BRANCH" > "%s/hook.out"\necho hook-stdout\n' "$TMP" > .git/worktree-setup
  chmod +x .git/worktree-setup
  run --separate-stderr "$SD_ACT" start --no-implement feat-x
  [ "$status" -eq 0 ]
  [ "$output" = "$TMP/feat-x" ]
  [ "$(cat "$TMP/hook.out")" = "$TMP/feat-x|$TMP/repo|$TMP/feat-x|feat-x" ]
  [[ "$stderr" == *hook-stdout* ]]
}

@test "start: a worktree-setup that isn't executable is skipped" {
  printf '#!/usr/bin/env bash\ntouch "%s/ran"\n' "$TMP" > .git/worktree-setup
  run --separate-stderr "$SD_ACT" start --no-implement feat-x
  [ "$status" -eq 0 ]
  [ ! -e "$TMP/ran" ]
}

@test "start: falls back to the wt.setup config command" {
  git config wt.setup 'echo "$WT_BRANCH" > "$WT_DST/../wt.out"'
  run --separate-stderr "$SD_ACT" start --no-implement feat-x
  [ "$status" -eq 0 ]
  [ "$(cat "$TMP/wt.out")" = feat-x ]
}

@test "start: the hook script wins over wt.setup" {
  printf '#!/usr/bin/env bash\ntouch "%s/script"\n' "$TMP" > .git/worktree-setup
  chmod +x .git/worktree-setup
  git config wt.setup "touch '$TMP/config'"
  "$SD_ACT" start --no-implement feat-x >/dev/null 2>&1
  [ -e "$TMP/script" ]
  [ ! -e "$TMP/config" ]
}

@test "start: a failing hook keeps the worktree, prints its path, exits nonzero, and doesn't implement" {
  printf '#!/usr/bin/env bash\necho broke >&2\nexit 3\n' > .git/worktree-setup
  chmod +x .git/worktree-setup
  run --separate-stderr "$SD_ACT" start feat-x "a task"
  [ "$status" -ne 0 ]
  [ "$output" = "$TMP/feat-x" ]
  [ -d "$TMP/feat-x" ]
  [[ "$stderr" == *broke* ]]
  [[ "$stderr" == *"sd act implement feat-x"* ]]
  [ ! -e "$FX/claude.args" ]
}

@test "start: from a linked worktree, the new one goes beside the main worktree, not beside the linked one (#85)" {
  printf '#!/usr/bin/env bash\necho "$WT_SRC" > "%s/src.out"\n' "$TMP" > .git/worktree-setup
  chmod +x .git/worktree-setup
  mkdir "$TMP/wts" && git worktree add -q -b one "$TMP/wts/one" 2>/dev/null
  cd "$TMP/wts/one"
  run --separate-stderr "$SD_ACT" start --no-implement feat-x
  [ "$status" -eq 0 ]
  [ "$output" = "$TMP/feat-x" ]
  [ "$(cat "$TMP/src.out")" = "$TMP/wts/one" ]
}

@test "start: a slash in the branch name becomes a dash in the directory name, every generation (#85)" {
  run --separate-stderr "$SD_ACT" start --no-implement 12/stack-parent
  [ "$status" -eq 0 ]
  [ "$output" = "$TMP/12-stack-parent" ]
  [ "$(git -C "$TMP/12-stack-parent" branch --show-current)" = 12/stack-parent ]
  cd "$TMP/12-stack-parent"
  run --separate-stderr "$SD_ACT" start --no-implement --parent 12/stack-parent 12/stack-child
  [ "$status" -eq 0 ]
  [ "$output" = "$TMP/12-stack-child" ]
  [ ! -e "$TMP/12" ]
}

# ── start, then implement ────────────────────────────────────────────────────

@test "start: runs implement in the new worktree; stdout is still only the path" {
  run --separate-stderr "$SD_ACT" start feat-x "add a third line"
  [ "$status" -eq 0 ]
  [ "$output" = "$TMP/feat-x" ]
  [ "$(cat "$FX/claude.cwd")" = "$TMP/feat-x" ]
  [ "$(git log -1 --format=%s feat-x)" = "Add c" ]
  [[ "$stderr" == *"Added the third line."* ]]
}

@test "start: a failed implement still prints the path, nonzero" {
  touch "$FX/fail"
  run --separate-stderr "$SD_ACT" start feat-x "a task"
  [ "$status" -ne 0 ]
  [ "$output" = "$TMP/feat-x" ]
  [ "$(git config branch.feat-x.sheepdog)" = true ]
}

# ── implement ────────────────────────────────────────────────────────────────

started() { # a worktree for feat-x with a task, no agent run yet
  "$SD_ACT" start --no-implement feat-x "${1:-add a third line}" >/dev/null 2>&1
  cd "$TMP/feat-x"
}

@test "implement: the agent edits, the pipeline commits with its message" {
  started
  run "$SD_ACT" implement
  [ "$status" -eq 0 ]
  [ "$(git log -1 --format=%s)" = "Add c" ]
  [ "$(tail -1 file.txt)" = c ]
  [ -z "$(git status --porcelain)" ]
  [[ "$output" == *"Added the third line."* ]]
  [[ "$output" == *"sd act review"* ]]
}

@test "implement: the prompt carries the task and the verify command" {
  started "add a third line"
  echo '{"settings": {"verify": "echo checks-ok"}}' > "$SHEEPDOG_CONFIG"
  "$SD_ACT" implement >/dev/null
  grep -q 'add a third line' "$FX/claude.prompt"
  grep -q 'echo checks-ok' "$FX/claude.prompt"
}

@test "implement: mints the branch's session and stores it; a second run resumes it" {
  started
  "$SD_ACT" implement >/dev/null
  sid=$(git config branch.feat-x.sd-session)
  [ -n "$sid" ]
  [ "$(git config branch.feat-x.sd-harness)" = claude ]
  grep -qx -- --session-id "$FX/claude.args"
  "$SD_ACT" implement >/dev/null
  grep -qx -- --resume "$FX/claude.args"
  grep -qx -- "$sid" "$FX/claude.args"
}

allowed() { grep -A1 -x -- --allowedTools "$FX/claude.args" | tail -1; }

@test "implement: no free shell: no bare Bash, read-only git; commit, push and gh denied" {
  started
  "$SD_ACT" implement >/dev/null
  allow=$(allowed)
  deny=$(grep -A1 -x -- --disallowedTools "$FX/claude.args" | tail -1)
  [[ ",$allow," != *",Bash,"* ]]
  [[ $allow == *"Bash(git diff *)"* && $allow == *Edit* ]]
  [[ $deny == *"git commit"* && $deny == *"git push"* && $deny == *"gh "* ]]
}

@test "implement: a plain verify command becomes an exact rule" {
  started
  echo '{"settings": {"verify": "test -f file.txt"}}' > "$SHEEPDOG_CONFIG"
  "$SD_ACT" implement >/dev/null
  [[ ",$(allowed)," == *",Bash(test -f file.txt),"* ]]
}

@test "implement: a verify with shell syntax doesn't become a rule, and says where to allow it" {
  started
  echo '{"settings": {"verify": "ls *.txt"}}' > "$SHEEPDOG_CONFIG"
  run --separate-stderr "$SD_ACT" implement
  [ "$status" -eq 0 ]
  [[ $(allowed) != *"Bash(ls"* ]]
  [[ $stderr == *"settings.implementAllow"* ]]
}

@test "implement: settings.implementAllow adds its rules, and the prompt names them" {
  started
  echo '{"settings": {"implementAllow": ["Bash(bats *)", "WebFetch"]}}' > "$SHEEPDOG_CONFIG"
  "$SD_ACT" implement >/dev/null
  [[ ",$(allowed)," == *",Bash(bats *),WebFetch,"* ]]
  grep -qF 'Bash(bats *)' "$FX/claude.prompt"
}

@test "implement: pi gets no shell, and its prompt names no Claude rules" {
  started
  cat > "$TMP/fakebin/pi" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$@" > "$FX/pi.args"
[[ -x $FX/agent ]] && "$FX/agent"
printf '```json\n%s\n```\n' "$(cat "$FX/result.json")"
EOF
  chmod +x "$TMP/fakebin/pi"
  echo '{"settings": {"verify": "test -f file.txt", "implementAllow": ["Bash(bats *)"]}}' > "$SHEEPDOG_CONFIG"
  run "$SD_ACT" implement --harness pi
  [ "$status" -eq 0 ]
  [ "$(grep -A1 -x -- --tools "$FX/pi.args" | tail -1)" = read,edit,write ]
  [[ $(<"$FX/pi.args") != *"Bash("* ]]
  grep -q "can't run commands" "$FX/pi.args"
}

@test "implement: with nothing allowed, the prompt says the agent can't run commands" {
  started
  "$SD_ACT" implement >/dev/null
  grep -q "can't run commands" "$FX/claude.prompt"
}

@test "implement: commits the agent made itself count as its work" {
  started
  printf '#!/usr/bin/env bash\necho c >> file.txt && git commit -qam "agent made it"\n' > "$FX/agent"
  run "$SD_ACT" implement
  [ "$status" -eq 0 ]
  [ "$(git log -1 --format=%s)" = "agent made it" ]
}

@test "implement: no commit message from the agent falls back to the task's first line" {
  started "$(printf 'add a third line\n\nmore detail')"
  echo '{"outcome": "done", "commit_message": null, "summary": "ok"}' > "$FX/result.json"
  "$SD_ACT" implement >/dev/null
  [ "$(git log -1 --format=%s)" = "add a third line" ]
}

@test "implement: a commit that changes CI config is held for chrismo; the agent's own commits count (#124)" {
  started
  printf '#!/usr/bin/env bash\necho c >> file.txt\nmkdir -p .github/workflows && echo x > .github/workflows/ci.yml && git add -A && git commit -qm "agent made it"\necho d >> file.txt\n' > "$FX/agent"
  run "$SD_ACT" implement
  [ "$status" -eq 0 ]
  [[ "$output" == *"held: the agent changed CI config (.github/workflows/ci.yml): review, then sd act push"* ]]
  # Keyed on the commit that changed it, the agent's; the pipeline's commit of the rest follows.
  [ "$(git config --get branch.feat-x.sd-held)" = "$(git rev-parse HEAD~1)"$'\n'".github/workflows/ci.yml" ]
}

@test "implement: a CI config file with a non-ASCII name is held under its own name (#124)" {
  started
  printf '#!/usr/bin/env bash\nmkdir -p .github/workflows && echo x > .github/workflows/tëst.yml\n' > "$FX/agent"
  run "$SD_ACT" implement
  [ "$status" -eq 0 ]
  [ "$(git config --get branch.feat-x.sd-held)" = "$(git rev-parse HEAD)"$'\n'".github/workflows/tëst.yml" ]
}

@test "implement: a CI config change the agent committed itself before needing input is held (#124)" {
  started
  printf '#!/usr/bin/env bash\nmkdir -p .github/workflows && echo x > .github/workflows/ci.yml && git add -A && git commit -qm "agent made it"\n' > "$FX/agent"
  echo '{"outcome": "needs-input", "commit_message": null, "summary": "which runner?"}' > "$FX/result.json"
  run "$SD_ACT" implement
  [ "$status" -ne 0 ]
  [ "$(git config --get branch.feat-x.sd-held)" = "$(git rev-parse HEAD)"$'\n'".github/workflows/ci.yml" ]
}

@test "implement: a CI config change the agent committed itself is held even when the step then fails (#124)" {
  started
  printf '#!/usr/bin/env bash\nmkdir -p .github/workflows && echo x > .github/workflows/ci.yml && git add -A && git commit -qm "agent made it"\n' > "$FX/agent"
  echo 'not json' > "$FX/result.json"
  run "$SD_ACT" implement
  [ "$status" -ne 0 ]
  [ "$(git config --get branch.feat-x.sd-held)" = "$(git rev-parse HEAD)"$'\n'".github/workflows/ci.yml" ]
}

@test "implement: an agent that pushed its CI config change itself fails loudly; no hold that would vanish (#124)" {
  started
  printf '#!/usr/bin/env bash\nmkdir -p .github/workflows && echo x > .github/workflows/ci.yml && git add -A && git commit -qm "agent made it" && git push -q origin feat-x 2>/dev/null\n' > "$FX/agent"
  run "$SD_ACT" implement
  [ "$status" -ne 0 ]
  [[ "$output" == *"the agent pushed CI config itself (.github/workflows/ci.yml)"* ]]
  run git config --get branch.feat-x.sd-held
  [ "$status" -eq 1 ]
}

@test "implement: a review-fix round that changes CI config is held too, joined with the hold before it (#124)" {
  started
  git config branch.feat-x.sd-held "$(git rev-parse HEAD)"$'\n'".rwx/tests.yml"
  findings_at HEAD 1
  printf '#!/usr/bin/env bash\nmkdir -p .circleci && echo x > .circleci/config.yml\n' > "$FX/agent"
  run "$SD_ACT" implement
  [ "$status" -eq 0 ]
  [[ "$output" == *"held: the agent changed CI config (.circleci/config.yml, .rwx/tests.yml)"* ]]
}

@test "implement: done with nothing changed is a failure" {
  started
  rm "$FX/agent"
  run "$SD_ACT" implement
  [ "$status" -ne 0 ]
  [[ "$output" == *"changed nothing"* ]]
}

# findings_at <rev> <round>: sd act review's record of one blocking finding on
# <rev>, and the agent's answer: fixed.
findings_at() {
  local rec
  rec=$(jq -nc --arg h "$(git rev-parse "$1")" --argjson r "$2" '{head: $h, base: $h, round: $r,
          findings: [{file: "file.txt", line: 2, severity: "error", body: "b should be B"}]}')
  git update-ref refs/sheepdog/feat-x/findings "$(git hash-object -w --stdin <<<"$rec")"
  answers '{"id": "1", "action": "fixed", "reply": "B now"}'
}

# answers <item>...: the agent's result for a fix round, one item per finding.
answers() {
  jq -nc '{outcome: "done", commit_message: "Add c", summary: "Fixed what was right.", items: [$ARGS.positional[] | fromjson]}' \
    --args "$@" > "$FX/result.json"
}

two_findings() { # two blocking findings on HEAD, round 1
  local rec
  rec=$(jq -nc --arg h "$(git rev-parse HEAD)" '{head: $h, base: $h, round: 1, findings: [
          {file: "file.txt", line: 1, severity: "warning", body: "a should be A"},
          {file: "file.txt", line: 2, severity: "error", body: "b should be B"}]}')
  git update-ref refs/sheepdog/feat-x/findings "$(git hash-object -w --stdin <<<"$rec")"
}
findings_rec() { git cat-file blob refs/sheepdog/feat-x/findings | jq -c "$1"; }

@test "implement: a fix round numbers each finding and asks for an answer per id" {
  started
  two_findings
  answers '{"id": "1", "action": "fixed", "reply": "A now"}' '{"id": "2", "action": "fixed", "reply": "B now"}'
  "$SD_ACT" implement >/dev/null
  grep -qF -- '- [1] file.txt:1 (warning) a should be A' "$FX/claude.prompt"
  grep -qF -- '- [2] file.txt:2 (error) b should be B' "$FX/claude.prompt"
  grep -q 'declined' "$FX/claude.prompt"
  grep -qF '"action": {"enum": ["fixed", "declined", "ask"]}' "$FX/claude.args"
}

@test "implement: the first implement keeps its schema, with no items" {
  started
  "$SD_ACT" implement >/dev/null
  grep -qF '"required": ["outcome", "commit_message", "summary"]' "$FX/claude.args"
  ! grep -q '"items"' "$FX/claude.args" || false
}

@test "implement: fixed one, declined one: commits, records each answer, and the next review is due" {
  started
  two_findings
  answers '{"id": "1", "action": "declined", "reply": "a is lowercase on purpose"}' '{"id": "2", "action": "fixed", "reply": "B now"}'
  run "$SD_ACT" implement
  [ "$status" -eq 0 ]
  [ "$(git log -1 --format=%s)" = "Add c" ]
  [[ "$output" == *"declined"*"a is lowercase on purpose"* ]]
  [[ "$output" == *"sd act review"* ]]
  [ "$(findings_rec '[.findings[] | [.action, .reply]]')" = '[["declined","a is lowercase on purpose"],["fixed","B now"]]' ]
  [ "$("$SD_STATE" --json | jq -c '[.review.findings.at_head, any(.next[]; .key == "review")]')" = '[false,true]' ]
}

@test "implement: a finding the agent marks ask goes to chrismo: nonzero, edits uncommitted, nothing recorded" {
  started
  two_findings
  answers '{"id": "1", "action": "ask", "reply": "A or a?"}' '{"id": "2", "action": "fixed", "reply": "B now"}'
  run "$SD_ACT" implement
  [ "$status" -ne 0 ]
  [[ "$output" == *"needs input"*"A or a?"* ]]
  [ -n "$(git status --porcelain)" ]
  [ "$(git log -1 --format=%s)" = init ]
  [ "$(findings_rec '[.findings[] | .action]')" = '[null,null]' ]
  # implement refuses the dirty worktree, so it isn't the next step
  [[ "$output" == *"sd act attach"* ]]
  [[ "$output" != *"then sd act implement"* ]]
}

@test "implement: fixed needs no reply text; declined does" {
  started
  two_findings
  answers '{"id": "1", "action": "fixed", "reply": ""}' '{"id": "2", "action": "fixed", "reply": "B now"}'
  run "$SD_ACT" implement
  [ "$status" -eq 0 ]
  [ "$(git log -1 --format=%s)" = "Add c" ]
  git reset -q --hard HEAD~1
  two_findings
  answers '{"id": "1", "action": "declined", "reply": " "}' '{"id": "2", "action": "fixed", "reply": "B now"}'
  run "$SD_ACT" implement
  [ "$status" -ne 0 ]
  [[ "$output" == *"[1]"*"no reply text"* ]]
}

@test "implement: a finding the agent didn't answer counts as ask" {
  started
  two_findings
  answers '{"id": "2", "action": "fixed", "reply": "B now"}'
  run "$SD_ACT" implement
  [ "$status" -ne 0 ]
  [[ "$output" == *"[1]"*"did not answer"* ]]
  [ "$(git log -1 --format=%s)" = init ]
}

@test "implement: fixed with nothing changed counts as ask" {
  started
  findings_at HEAD 1
  rm "$FX/agent"
  run "$SD_ACT" implement
  [ "$status" -ne 0 ]
  [[ "$output" == *"changed nothing"*"B now"* ]]
  [ "$(findings_rec '.findings[0].action')" = null ]
}

@test "implement: a finding declined in an earlier answer isn't sent again" {
  started
  two_findings
  rec=$(findings_rec '.findings[0] += {action: "declined", reply: "no"}')
  git update-ref refs/sheepdog/feat-x/findings "$(git hash-object -w --stdin <<<"$rec")"
  answers '{"id": "2", "action": "fixed", "reply": "B now"}'
  run "$SD_ACT" implement
  [ "$status" -eq 0 ]
  ! grep -q 'a should be A' "$FX/claude.prompt" || false
  grep -qF -- '- [2] file.txt:2' "$FX/claude.prompt"
  [ "$(findings_rec '[.findings[] | [.action, .reply]]')" = '[["declined","no"],["fixed","B now"]]' ]
}

@test "implement: review findings on HEAD go into the prompt" {
  started
  findings_at HEAD 1
  run "$SD_ACT" implement
  [ "$status" -eq 0 ]
  grep -q 'b should be B' "$FX/claude.prompt"
  grep -q 'file.txt:2' "$FX/claude.prompt"
  [ "$(git log -1 --format=%s)" = "Add c" ]
}

@test "implement: a fix round is framed as fixing the findings, with only those that block" {
  started
  rec=$(jq -nc --arg h "$(git rev-parse HEAD)" '{head: $h, base: $h, round: 1, findings: [
          {file: "file.txt", line: 2, severity: "error", body: "b should be B"},
          {file: "file.txt", line: 1, severity: "nit", body: "a could be A"}]}')
  git update-ref refs/sheepdog/feat-x/findings "$(git hash-object -w --stdin <<<"$rec")"
  answers '{"id": "1", "action": "fixed", "reply": "B now"}'
  "$SD_ACT" implement >/dev/null
  [[ "$(head -1 "$FX/claude.prompt")" == *"review"* ]]
  ! grep -q '^Implement the task below' "$FX/claude.prompt" || false
  grep -q 'b should be B' "$FX/claude.prompt"
  ! grep -q 'a could be A' "$FX/claude.prompt" || false
  grep -q 'add a third line' "$FX/claude.prompt"
}

@test "implement: review findings on HEAD run even with no task recorded" {
  started
  git config --unset branch.feat-x.sd-task
  findings_at HEAD 1
  run "$SD_ACT" implement
  [ "$status" -eq 0 ]
  grep -q 'b should be B' "$FX/claude.prompt"
  grep -q '(none recorded)' "$FX/claude.prompt"
}

@test "implement: findings the agent already declined aren't sent again" {
  started
  rec=$(jq -nc --arg h "$(git rev-parse HEAD)" '{head: $h, base: $h, round: 1,
          findings: [{file: "file.txt", line: 2, severity: "error", body: "b should be B", action: "declined", reply: "no"}]}')
  git update-ref refs/sheepdog/feat-x/findings "$(git hash-object -w --stdin <<<"$rec")"
  "$SD_ACT" implement >/dev/null
  ! grep -q 'b should be B' "$FX/claude.prompt" || false
  grep -q '^Implement the task below' "$FX/claude.prompt"
  git config --unset branch.feat-x.sd-task
  run "$SD_ACT" implement
  [ "$status" -ne 0 ]
  [[ "$output" == *"no task recorded"* ]]
}

@test "implement: findings on a rebased HEAD count when sd state says they're about it" {
  started
  git update-ref -d refs/sheepdog/feat-x/base
  echo c >> file.txt && git commit -qam "add c"
  rec=$(jq -nc --arg h "$(git rev-parse HEAD)" --arg b "$(git rev-parse HEAD~1)" '{head: $h, base: $b, round: 1,
          findings: [{file: "file.txt", line: 3, severity: "error", body: "c should be C"}]}')
  git update-ref refs/sheepdog/feat-x/findings "$(git hash-object -w --stdin <<<"$rec")"
  other_push main "main moves"
  git fetch -q && git rebase -q origin/main
  [ "$("$SD_STATE" --json | jq .review.findings.at_head)" = true ]
  answers '{"id": "1", "action": "fixed", "reply": "C now"}'
  "$SD_ACT" implement >/dev/null
  grep -q 'c should be C' "$FX/claude.prompt"
}

@test "implement: with a findings record, sd state failing is a failure, not the task prompt" {
  started
  findings_at HEAD 1
  mkdir -p "$TMP/broken" && printf '#!/usr/bin/env bash\nexit 1\n' > "$TMP/broken/sd-state" && chmod +x "$TMP/broken/sd-state"
  PATH="$TMP/broken:$PATH" run "$SD_ACT" implement
  [ "$status" -ne 0 ]
  [[ "$output" == *"sd state failed"* ]]
  [ ! -e "$FX/claude.prompt" ]
}

@test "implement: a fix round with no commit message from the agent says it fixed review findings" {
  started
  findings_at HEAD 1
  echo '{"outcome": "done", "summary": "Fixed b.", "items": [{"id": "1", "action": "fixed", "reply": "B now"}]}' > "$FX/result.json"
  "$SD_ACT" implement >/dev/null
  [ "$(git log -1 --format=%s)" = "Fix the local review findings" ]
}

@test "--accept belongs to sd act review only" {
  started
  run "$SD_ACT" implement --accept
  [ "$status" -ne 0 ]
  [[ "$output" == *"--accept"* ]]
  [ ! -e "$FX/claude.prompt" ]
}

@test "implement: a finding missing fields still reads as itself in the prompt" {
  started
  rec=$(jq -nc --arg h "$(git rev-parse HEAD)" '{head: $h, base: $h, round: 1, findings: [{file: "file.txt", body: "b?"}]}')
  git update-ref refs/sheepdog/feat-x/findings "$(git hash-object -w --stdin <<<"$rec")"
  answers '{"id": "1", "action": "fixed", "reply": "B now"}'
  "$SD_ACT" implement >/dev/null
  grep -qF -- '- [1] file.txt:? (?) b?' "$FX/claude.prompt"
}

@test "implement: settings.implementPrompt replaces the guidance; the contract and task stay" {
  started
  printf 'Work in tiny steps.\n' > "$TMP/implement.md"
  echo "{\"settings\": {\"implementPrompt\": \"$TMP/implement.md\"}}" > "$SHEEPDOG_CONFIG"
  "$SD_ACT" implement >/dev/null
  [ "$(head -1 "$FX/claude.prompt")" = "Work in tiny steps." ]
  ! grep -q 'Implement the task below' "$FX/claude.prompt" || false
  grep -q 'do not commit, push, or run gh' "$FX/claude.prompt"
  grep -q 'Return outcome "done"' "$FX/claude.prompt"
  grep -q 'add a third line' "$FX/claude.prompt"
}

@test "implement: .sheepdog/implement.md in the branch's base is the repo's guidance" {
  mkdir -p .sheepdog && printf 'Repo implement guidance.\n' > .sheepdog/implement.md
  git add .sheepdog && git commit -qm guidance && git push -q origin main 2>/dev/null
  started
  "$SD_ACT" implement >/dev/null
  [ "$(head -1 "$FX/claude.prompt")" = "Repo implement guidance." ]
}

@test "implement: a branch with no base pin and no remote still gets its guidance" {
  git init -q "$TMP/local" && cd "$TMP/local"
  mkdir -p .sheepdog && printf 'Local repo guidance.\n' > .sheepdog/implement.md
  printf 'a\nb\n' > file.txt && git add . && git commit -qm init
  git switch -qc feat && git config branch.feat.sd-task "add c"
  "$SD_ACT" implement >/dev/null
  [ "$(head -1 "$FX/claude.prompt")" = "Local repo guidance." ]
}

@test "implement: with no base to trust, the branch's own .sheepdog/ is ignored" {
  git init -q -b feat "$TMP/lone" && cd "$TMP/lone"
  mkdir -p .sheepdog && printf 'Branch-written guidance.\n' > .sheepdog/implement.md
  printf 'a\nb\n' > file.txt && git add . && git commit -qm init
  git config branch.feat.sd-task "add c"
  "$SD_ACT" implement >/dev/null
  [ "$(head -1 "$FX/claude.prompt")" != "Branch-written guidance." ]
  grep -q 'Implement the task below' "$FX/claude.prompt"
}

@test "implement: on the default branch itself, its own .sheepdog/ is not trusted" {
  git init -q -b main "$TMP/onmain" && cd "$TMP/onmain"
  printf 'a\nb\n' > file.txt && git add . && git commit -qm init
  mkdir -p .sheepdog && printf 'Agent-written guidance.\n' > .sheepdog/implement.md
  git add . && git commit -qm "the agent's commit"
  git config branch.main.sd-task "add c"
  "$SD_ACT" implement main >/dev/null
  grep -q 'Implement the task below' "$FX/claude.prompt"
}

@test "implement: on the default branch itself, pushed, its own .sheepdog/ is not trusted" {
  mkdir -p .sheepdog && printf 'Agent-written guidance.\n' > .sheepdog/implement.md
  git add . && git commit -qm "the agent's commit" && git push -q origin main 2>/dev/null
  git config branch.main.sd-task "add c"
  "$SD_ACT" implement main >/dev/null
  grep -q 'Implement the task below' "$FX/claude.prompt"
}

@test "implement: settings.reviewFixPrompt replaces a fix round's guidance; the contract stays" {
  started
  findings_at HEAD 1
  printf 'Only fix errors.\n' > "$TMP/fix.md"
  echo "{\"settings\": {\"reviewFixPrompt\": \"$TMP/fix.md\"}}" > "$SHEEPDOG_CONFIG"
  "$SD_ACT" implement >/dev/null
  [ "$(head -1 "$FX/claude.prompt")" = "Only fix errors." ]
  grep -q 'do not commit, push, or run gh' "$FX/claude.prompt"
  grep -q 'b should be B' "$FX/claude.prompt"
}

@test "implement: findings on an older head stay out of the prompt" {
  started
  findings_at HEAD 1
  git commit -q --allow-empty -m later
  "$SD_ACT" implement >/dev/null
  ! grep -q 'b should be B' "$FX/claude.prompt" || false
}

@test "implement: declining every review finding records each, for chrismo to decide" {
  started
  findings_at HEAD 2
  rm "$FX/agent"
  answers '{"id": "1", "action": "declined", "reply": "b is right as it is."}'
  run "$SD_ACT" implement
  [ "$status" -eq 0 ]
  [[ "$output" == *"declined"*"b is right as it is."* ]]
  [ "$(findings_rec '[.round, .findings[0].action, .findings[0].reply]')" = '[2,"declined","b is right as it is."]' ]
  [ "$("$SD_STATE" --json | jq -c '[.review.findings.declined, any(.next[]; .key == "review-decide")]')" = '[true,true]' ]
}

@test "implement: needs-input prints the question, leaves the edits uncommitted, nonzero" {
  started
  echo '{"outcome": "needs-input", "commit_message": null, "summary": "Which file?"}' > "$FX/result.json"
  run "$SD_ACT" implement
  [ "$status" -ne 0 ]
  [[ "$output" == *"Which file?"* ]]
  [ -n "$(git status --porcelain)" ]
  [ "$(git log -1 --format=%s)" = init ]
}

@test "implement: gave-up is nonzero with the agent's reason, nothing committed" {
  started
  echo '{"outcome": "gave-up", "commit_message": null, "summary": "makes no sense"}' > "$FX/result.json"
  run "$SD_ACT" implement
  [ "$status" -ne 0 ]
  [[ "$output" == *"makes no sense"* ]]
  [ "$(git log -1 --format=%s)" = init ]
}

@test "implement: verify runs after the commit; a failure is nonzero and keeps the commit" {
  started
  echo '{"settings": {"verify": "echo verify-ran; exit 1"}}' > "$SHEEPDOG_CONFIG"
  run "$SD_ACT" implement
  [ "$status" -ne 0 ]
  [[ "$output" == *verify-ran* ]]
  [ "$(git log -1 --format=%s)" = "Add c" ]
}

@test "implement: refuses a dirty worktree without calling the agent" {
  started
  echo dirt > dirt.txt
  run "$SD_ACT" implement
  [ "$status" -ne 0 ]
  [[ "$output" == *"uncommitted changes"* ]]
  [ ! -e "$FX/claude.args" ]
}

@test "implement: refuses with no task recorded" {
  "$SD_ACT" start --no-implement feat-x >/dev/null 2>&1
  cd "$TMP/feat-x"
  run "$SD_ACT" implement
  [ "$status" -ne 0 ]
  [[ "$output" == *"no task recorded"* ]]
}

@test "implement: the agent failing is nonzero and names the session" {
  started
  touch "$FX/fail"
  run "$SD_ACT" implement
  [ "$status" -ne 0 ]
  [[ "$output" == *"implement step failed"* ]]
}
