#!/usr/bin/env bats

bats_require_minimum_version 1.5.0

# Test suite for lib/sheepdog.sh, the helpers shared by sd state, sd act and
# sd watch. Needs bash 5 first on PATH (the lib is sourced).

setup() {
  source "$BATS_TEST_DIRNAME/config.sh"
  source "$BATS_TEST_DIRNAME/sheepdog.sh"
  TMP="$(cd "$BATS_TEST_TMPDIR" && pwd -P)"
  export GIT_CONFIG_GLOBAL="$TMP/gitconfig-global"
  export GIT_CONFIG_SYSTEM=/dev/null
  printf '[user]\n\temail = t@example.com\n\tname = Tester\n[init]\n\tdefaultBranch = main\n' > "$GIT_CONFIG_GLOBAL"
  export SHEEPDOG_CONFIG="$TMP/global.json" SHEEPDOG_STATE_DIR="$TMP/state"
  # Where the harnesses keep their transcripts (harness_tally).
  export CLAUDE_CONFIG_DIR="$TMP/claude" PI_CODING_AGENT_SESSION_DIR="$TMP/pi"
  # Set when sheepdog runs these tests from its own step.
  unset HARNESS_STEP HARNESS_BRANCH
  git init -q "$TMP/repo" && cd "$TMP/repo"
  git commit -q --allow-empty -m init
  git switch -qc feat && git commit -q --allow-empty -m one
}

# ── stacked_on: a stack sheepdog wasn't told about (#94) ────────────────────

# c <branch> <msg>: a commit on <branch>, left checked out.
c() { git switch -q "$1" && git commit -q --allow-empty -m "$2"; }

stack_setup() {
  git switch -q main
  git update-ref refs/remotes/origin/main main
  git branch -qD feat
  git switch -qc par && git commit -q --allow-empty -m p1 && git commit -q --allow-empty -m p2
  git switch -qc kid && git commit -q --allow-empty -m k1
}

@test "stacked_on: a branch made from another's tip names it and its tip" {
  stack_setup
  run stacked_on kid origin main
  [ "$status" -eq 0 ]
  [ "$output" = "par $(git rev-parse par)" ]
}

@test "stacked_on: the parent moved on since; named, but either way round" {
  stack_setup
  c par p3
  run stacked_on kid origin main
  [ "$status" -eq 3 ]
  [ "$output" = "par" ]
}

@test "stacked_on: a moved parent made the fork commit itself, so its child isn't its parent" {
  stack_setup
  c par p3
  run stacked_on par origin main
  [ "$status" -eq 1 ]
  [ -z "$output" ]
}

@test "stacked_on: a backup of the branch taken before its last commit isn't its parent" {
  stack_setup
  git branch kid-bak kid
  c kid k2
  run stacked_on kid origin main
  [ "$status" -eq 0 ]
  [ "$output" = "par $(git rev-parse par)" ]
}

@test "stacked_on: a branch renamed right after it was made still names its parent" {
  stack_setup
  git switch -q par && git switch -qc tmp && git branch -m tmp kid2 && git commit -q --allow-empty -m k2
  run stacked_on kid2 origin main
  [ "$status" -eq 0 ]
  [ "$output" = "par $(git rev-parse par)" ]
}

@test "stacked_on: a backup taken after the branch was rebased isn't its parent" {
  stack_setup
  c par p3
  git switch -q kid && git rebase -q par
  git branch kid-bak kid
  c kid k2
  run stacked_on kid origin main
  [ "$status" -eq 0 ]
  [ "$output" = "par $(git rev-parse par)" ]
}

@test "stacked_on: siblings that each moved on tie without a direction" {
  stack_setup
  git switch -q par && git switch -qc sib1 && git commit -q --allow-empty -m s1
  git switch -q par && git switch -qc sib2 && git commit -q --allow-empty -m s2
  git switch -q kid && git branch -qD par
  run stacked_on kid origin main
  [ "$status" -eq 3 ]
  [ "$output" = "sib1 sib2" ]
}

@test "stacked_on: a branch reset onto its parent's tip still names the parent" {
  stack_setup
  git switch -q main && git switch -qc kid2 && git reset -q --hard par && git commit -q --allow-empty -m k2
  run stacked_on kid2 origin main
  [ "$status" -eq 0 ]
  [ "$output" = "par $(git rev-parse par)" ]
}

@test "stacked_on: a recorded child with no commits of its own isn't its parent's parent, reflog or not" {
  stack_setup
  git switch -q par && git branch -qD kid
  git branch fresh par && git config branch.fresh.sd-parent par
  c par p3
  git reflog expire --expire=all --all
  run stacked_on par origin main
  [ "$status" -eq 1 ]
  [ -z "$output" ]
}

@test "stacked_on: a moved parent whose commits came by fast-forward can't tell its child from a parent" {
  git update-ref refs/remotes/origin/main main
  git switch -qc up && git commit -q --allow-empty -m u1
  git switch -q main && git switch -qc mine && git merge -q --ff-only up && git branch -qD up
  git switch -qc kid && git commit -q --allow-empty -m k1
  c mine m2
  run stacked_on mine origin main
  [ "$status" -eq 3 ]
  [ "$output" = "kid" ]
}

@test "stacked_on: without a reflog, a moved parent still can't tell its child from a parent" {
  stack_setup
  c par p3
  git reflog expire --expire=all --all
  run stacked_on par origin main
  [ "$status" -eq 3 ]
  [ "$output" = "kid" ]
}

@test "stacked_on: a moved parent skips a child that recorded it as its parent" {
  stack_setup
  c par p3
  git config branch.kid.sd-parent par
  run stacked_on par origin main
  [ "$status" -eq 1 ]
  [ -z "$output" ]
}

@test "stacked_on: a moved parent whose remote copy is still at the fork is the parent" {
  stack_setup
  local fork; fork=$(git rev-parse par)
  git update-ref refs/remotes/origin/par par
  c par p3
  run stacked_on kid origin main
  [ "$status" -eq 0 ]
  [ "$output" = "par $fork" ]
}

@test "stacked_on: a sibling made from the same tip loses to the parent" {
  stack_setup
  git switch -q par && git switch -qc sib && git commit -q --allow-empty -m s1
  run stacked_on kid origin main
  [ "$status" -eq 0 ]
  [ "$output" = "par $(git rev-parse par)" ]
}

@test "stacked_on: the default branch is stacked on nothing, unpushed commits or not" {
  git update-ref refs/remotes/origin/main main
  c main m1
  git switch -qc topic && git commit -q --allow-empty -m t1
  c main m2
  run stacked_on main origin main
  [ "$status" -eq 1 ]
  [ -z "$output" ]
}

@test "stacked_on: a local parent rewritten since is found by its remote-tracking ref" {
  stack_setup
  git update-ref refs/remotes/origin/par par
  git switch -q par && git commit -q --amend --allow-empty -m p2b
  run stacked_on kid origin main
  [ "$status" -eq 0 ]
  [ "$output" = "par $(git rev-parse origin/par)" ]
}

@test "stacked_on: only local branches and their remote-tracking refs are candidates" {
  stack_setup
  git update-ref refs/remotes/origin/par par
  git branch -qD par
  run stacked_on kid origin main
  [ "$status" -eq 1 ]
}

@test "stacked_on: a parent squash-merged but still local is still found" {
  stack_setup
  git switch -q main && git merge -q --squash par && git commit -q --allow-empty -m "par (#1)"
  git update-ref refs/remotes/origin/main main
  run stacked_on kid origin main
  [ "$status" -eq 0 ]
  [ "$output" = "par $(git rev-parse par)" ]
}

@test "stacked_on: a branch that merged the default branch in isn't stacked" {
  git update-ref refs/remotes/origin/main main
  c main m1 && git update-ref refs/remotes/origin/main main
  git branch other main
  git switch -q feat && git merge -q --no-edit main
  run stacked_on feat origin main
  [ "$status" -eq 1 ]
}

@test "stacked_on: sharing only commits already on the default branch isn't stacked" {
  git update-ref refs/remotes/origin/main main
  git branch older main
  c main m1 && git update-ref refs/remotes/origin/main main
  git switch -qc newer && git commit -q --allow-empty -m n1
  c main m2
  run stacked_on newer origin main
  [ "$status" -eq 1 ]
  run stacked_on feat origin main
  [ "$status" -eq 1 ]
}

@test "stacked_on: unpushed commits on the local default branch aren't a parent" {
  git update-ref refs/remotes/origin/main main
  c main m1
  git switch -qc topic && git commit -q --allow-empty -m t1
  run stacked_on topic origin main
  [ "$status" -eq 1 ]
}

@test "stacked_on: two topics made from unpushed commits on the local default branch aren't stacked" {
  git update-ref refs/remotes/origin/main main
  c main m1
  git switch -qc t1 && git commit -q --allow-empty -m t1
  git switch -q main && git switch -qc t2 && git commit -q --allow-empty -m t2
  run stacked_on t1 origin main
  [ "$status" -eq 1 ]
  [ -z "$output" ]
}

@test "stacked_on: a branch's own child isn't its parent" {
  stack_setup
  run stacked_on par origin main
  [ "$status" -eq 1 ]
  [ -z "$output" ]
}

@test "stacked_on: a branch on the default branch alone has none" {
  git update-ref refs/remotes/origin/main main
  run stacked_on feat origin main
  [ "$status" -eq 1 ]
}

@test "stacked_on: the nearest of a chain wins" {
  stack_setup
  git switch -qc grandkid && git commit -q --allow-empty -m g1
  run stacked_on grandkid origin main
  [ "$status" -eq 0 ]
  [ "$output" = "kid $(git rev-parse kid)" ]
}

@test "stacked_on: two branches sharing as much is ambiguous, and both are named" {
  stack_setup
  git branch twin par
  run stacked_on kid origin main
  [ "$status" -eq 2 ]
  [ "$output" = "par twin" ]
}

@test "stacked_on: the branch's own remote copy doesn't count" {
  stack_setup
  git update-ref refs/remotes/origin/kid kid
  run stacked_on kid origin main
  [ "$status" -eq 0 ]
  [ "$output" = "par $(git rev-parse par)" ]
}

@test "pin_write records the pin and the base it was taken against" {
  pin_write feat local HEAD main
  [ "$(git rev-parse refs/sheepdog/feat/local)" = "$(git rev-parse HEAD)" ]
  [ "$(git rev-parse refs/sheepdog/feat/local-base)" = "$(git rev-parse main)" ]
}

@test "pin_write overwrites both refs, so a stale base never outlives its pin" {
  pin_write feat bot HEAD main
  git commit -q --allow-empty -m two
  pin_write feat bot HEAD HEAD~1
  [ "$(git rev-parse refs/sheepdog/feat/bot)" = "$(git rev-parse HEAD)" ]
  [ "$(git rev-parse refs/sheepdog/feat/bot-base)" = "$(git rev-parse HEAD~1)" ]
}

@test "pin_write refuses a pin kind it doesn't know" {
  run pin_write feat nope HEAD main
  [ "$status" -ne 0 ]
  [[ "$output" == *"unknown pin"* ]]
}

# harness_run's tests: claude and pi are stubs that put protocol noise on
# stderr and print $FX/out on stdout, exiting $FX/status.
harness_stubs() {
  export FX="$TMP/fx"
  mkdir -p "$FX" "$TMP/fakebin"
  for h in claude pi; do
    cat > "$TMP/fakebin/$h" <<'EOF'
#!/usr/bin/env bash
echo '{"type":"system","subtype":"init","session_id":"x"}' >&2
echo "${ANTHROPIC_API_KEY-unset}" > "$FX/key"
printf '%s\n' "$@" > "$FX/args"
cat "$FX/out"
exit "$(cat "$FX/status" 2>/dev/null || echo 0)"
EOF
    chmod +x "$TMP/fakebin/$h"
  done
  export PATH="$TMP/fakebin:$PATH"
  echo '{"type": "result", "is_error": false, "structured_output": {"ok": true}}' > "$FX/out"
}

@test "harness_run: claude runs without ANTHROPIC_API_KEY, so the claude.ai login is used" {
  harness_stubs
  export ANTHROPIC_API_KEY=sk-no-credit
  run harness_run claude sid1 new 30 "do it" '{}'
  [ "$status" -eq 0 ]
  [ "$(cat "$FX/key")" = unset ]
}

@test "harness_run: harnesses.claude.useApiKey keeps the key" {
  harness_stubs
  export ANTHROPIC_API_KEY=sk-paid
  run harness_run claude sid1 new 30 "do it" '{}' '{"harness": "claude", "useApiKey": true}'
  [ "$status" -eq 0 ]
  [ "$(cat "$FX/key")" = sk-paid ]
  run harness_run claude sid1 new 30 "do it" '{}' '{"harness": "claude", "useApiKey": false}'
  [ "$(cat "$FX/key")" = unset ]
}

@test "harness_run: pi keeps the key; it can't use the claude.ai login" {
  harness_stubs
  export ANTHROPIC_API_KEY=sk-paid
  printf 'Done.\n\n```json\n{"ok": true}\n```\n' > "$FX/out"
  run harness_run pi sid1 new 30 "do it" '{}'
  [ "$status" -eq 0 ]
  [ "$(cat "$FX/key")" = sk-paid ]
}

@test "harness_run: settings.stepBudget caps a claude step's spend, 5 USD by default; 0 is no cap" {
  harness_stubs
  run harness_run claude sid1 new 30 "do it" '{}'
  [ "$status" -eq 0 ]
  [ "$(grep -A1 -x -- --max-budget-usd "$FX/args" | tail -1)" = 5 ]
  echo '{"settings": {"stepBudget": 2.5}}' > "$SHEEPDOG_CONFIG"
  run harness_run claude sid1 new 30 "do it" '{}'
  [ "$status" -eq 0 ]
  [ "$(grep -A1 -x -- --max-budget-usd "$FX/args" | tail -1)" = 2.5 ]
  echo '{"settings": {"stepBudget": 0}}' > "$SHEEPDOG_CONFIG"
  run harness_run claude sid1 new 30 "do it" '{}'
  [ "$status" -eq 0 ]
  ! grep -q -- --max-budget-usd "$FX/args" || false
}

@test "harness_run: a --max-budget-usd in harnesses.claude.args replaces stepBudget's" {
  harness_stubs
  run harness_run claude sid1 new 30 "do it" '{}' '{"harness": "claude", "args": "--verbose --max-budget-usd 10"}'
  [ "$status" -eq 0 ]
  [ "$(grep -cx -- --max-budget-usd "$FX/args")" -eq 1 ]
  [ "$(grep -A1 -x -- --max-budget-usd "$FX/args" | tail -1)" = 10 ]
  run harness_run claude sid1 new 30 "do it" '{}' '{"harness": "claude", "args": "--max-budget-usd=10"}'
  [ "$status" -eq 0 ]
  ! grep -qx -- --max-budget-usd "$FX/args" || false
  grep -qx -- --max-budget-usd=10 "$FX/args"
}

@test "harness_run: pi has no budget flag" {
  harness_stubs
  echo '{"settings": {"stepBudget": 2.5}}' > "$SHEEPDOG_CONFIG"
  printf 'Done.\n\n```json\n{"ok": true}\n```\n' > "$FX/out"
  run harness_run pi sid1 new 30 "do it" '{}'
  [ "$status" -eq 0 ]
  ! grep -q -- budget "$FX/args" || false
}

@test "harness_run: a step that hits the budget fails and says so" {
  harness_stubs
  echo '{"settings": {"stepBudget": 0.1}}' > "$SHEEPDOG_CONFIG"
  echo '{"type": "result", "is_error": true, "subtype": "error_max_budget_usd", "result": null, "total_cost_usd": 0.1353}' > "$FX/out"
  run --separate-stderr harness_run claude sid1 new 30 "do it" '{}'
  [ "$status" -ne 0 ]
  [[ "$stderr" == *"hit the step budget (settings.stepBudget 0.1 USD)"* ]]
  [[ "$stderr" == *"0.1353"* ]]
  # A cap from harnesses.claude.args instead: no stepBudget to blame.
  rm "$SHEEPDOG_CONFIG"
  run --separate-stderr harness_run claude sid1 new 30 "do it" '{}' '{"harness": "claude", "args": "--max-budget-usd 0.1"}'
  [ "$status" -ne 0 ]
  [[ "$stderr" == *"hit the step budget"* ]]
  [[ "$stderr" != *"settings.stepBudget"* ]]
}

@test "harness_run: a negative stepBudget, or an unreadable config, fails the step before it runs" {
  harness_stubs
  echo '{"settings": {"stepBudget": -1}}' > "$SHEEPDOG_CONFIG"
  run --separate-stderr harness_run claude sid1 new 30 "do it" '{}'
  [ "$status" -ne 0 ]
  [[ "$stderr" == *"settings.stepBudget must be 0 or more"* ]]
  [ ! -e "$FX/args" ]
  echo 'not json' > "$SHEEPDOG_CONFIG"
  run --separate-stderr harness_run claude sid1 new 30 "do it" '{}'
  [ "$status" -ne 0 ]
  [ ! -e "$FX/args" ]
}

# usage_last <expr>: <expr> over usage.sup's last record, printed raw.
usage_last() { prc_super -i sup -f line -c "$PRC_FNS"$'\n'"tail 1 | values $1" "$SHEEPDOG_STATE_DIR/usage.sup"; }
usage_count() { prc_super -i sup -f line -c 'count()' "$SHEEPDOG_STATE_DIR/usage.sup"; }

@test "harness_run: every claude step appends a typed record to usage.sup, one line each" {
  harness_stubs
  echo '{"type": "result", "is_error": false, "structured_output": {"ok": true}, "total_cost_usd": 0.25, "duration_ms": 4321, "num_turns": 3,
         "usage": {"input_tokens": 10, "cache_creation_input_tokens": 200, "cache_read_input_tokens": 3000, "output_tokens": 40}}' > "$FX/out"
  HARNESS_STEP=implement run harness_run claude sid1 resume 30 "do it" '{}'
  [ "$status" -eq 0 ]
  [ "$(wc -l < "$SHEEPDOG_STATE_DIR/usage.sup")" -eq 1 ]
  [ "$(usage_last 'typeof(time) == <time> and typeof(cost_usd) == <float64> and typeof(input) == <int64> and estimated == false')" = true ]
  [ "$(usage_last 'f"{repo}|{branch}|{step}|{harness}|{session}|{mode}|{status}|{cost_usd}|{input}|{cache_create}|{cache_read}|{output}|{duration_ms}|{turns}"')" \
    = "$TMP/repo|feat|implement|claude|sid1|resume|0|0.25|10|200|3000|40|4321|3" ]
  run harness_run claude sid1 resume 30 "again" '{}'
  [ "$(wc -l < "$SHEEPDOG_STATE_DIR/usage.sup")" -eq 2 ]
  [ "$(usage_last 'f"{branch}|{nul(step)}|{session}"')" = "feat|true|sid1" ]
}

@test "harness_run: HARNESS_BRANCH names the branch when the worktree is detached (restack --resolve)" {
  harness_stubs
  git switch -q --detach
  HARNESS_BRANCH=feat run harness_run claude sid1 resume 30 "do it" '{}'
  [ "$status" -eq 0 ]
  [ "$(usage_last branch)" = feat ]
  run harness_run claude sid1 resume 30 "do it" '{}'
  [ "$(usage_count)" = 2 ]
  [ "$(usage_last 'nul(branch)')" = true ]
}

@test "harness_run: a failed step is logged with its status, and pi's record has no cost without a transcript" {
  harness_stubs
  echo 1 > "$FX/status"
  run harness_run claude sid1 new 30 "do it" '{}'
  [ "$status" -ne 0 ]
  [ "$(usage_last 'f"{harness}|{session}|{mode}|{status}"')" = "claude|sid1|new|1" ]
  rm "$FX/status"
  printf 'Done.\n\n```json\n{"ok": true}\n```\n' > "$FX/out"
  run harness_run pi sid2 new 30 "do it" '{}'
  [ "$status" -eq 0 ]
  [ "$(usage_last 'f"{harness}|{session}|{status}|{nul(cost_usd)}|{nul(input)}|{nul(context)}"')" = "pi|sid2|0|true|true|true" ]
}

@test "harness_tally: claude's context is its session's last turn, read from the transcript" {
  run harness_tally claude sid1
  [ "$output" = '{}' ]
  mkdir -p "$CLAUDE_CONFIG_DIR/projects/-repo"
  cat > "$CLAUDE_CONFIG_DIR/projects/-repo/sid1.jsonl" <<'EOF'
{"type":"user","message":{"role":"user","content":"do it"}}
{"parentUuid":"a","isSidechain":false,"message":{"role":"assistant","usage":{"input_tokens":5,"cache_creation_input_tokens":1000,"cache_read_input_tokens":0,"output_tokens":50}},"type":"assistant"}
{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"{\"type\":\"assistant\"}"}]}}
{"parentUuid":"b","isSidechain":false,"message":{"role":"assistant","usage":{"input_tokens":2,"cache_creation_input_tokens":300,"cache_read_input_tokens":120000,"output_tokens":700}},"type":"assistant"}
{"type":"progress","data":{"message":{"type":"assistant","message":{"usage":{"input_tokens":999999}}}}}
EOF
  run harness_tally claude sid1
  [ "$status" -eq 0 ]
  [ "$(prc_val "$output" context)" = 121002 ]
}

@test "harness_tally: claude's synthetic entries (an API error, e.g. prompt too long) are neither the context nor a turn" {
  mkdir -p "$CLAUDE_CONFIG_DIR/projects/-repo"
  cat > "$CLAUDE_CONFIG_DIR/projects/-repo/sid1.jsonl" <<'EOF'
{"message":{"role":"assistant","model":"claude-opus","usage":{"input_tokens":2,"cache_read_input_tokens":190000,"output_tokens":8}},"type":"assistant"}
{"message":{"role":"assistant","model":"<synthetic>","usage":{"input_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":0}},"type":"assistant"}
EOF
  run harness_tally claude sid1
  [ "$status" -eq 0 ]
  [ "$(prc_val "$output" 'f"{context}|{entries}"')" = "190010|1" ]
}

@test "harness_run: a claude step that got only an API error isn't charged its cap" {
  harness_stubs
  claude_stub_appends sid1 '{"message":{"role":"assistant","model":"<synthetic>","usage":{"input_tokens":0,"output_tokens":0}},"type":"assistant"}'
  run harness_run claude sid1 resume 30 "do it" '{}'
  [ "$status" -ne 0 ]
  [ "$(usage_last 'f"{nul(cost_usd)}|{estimated}"')" = "true|false" ]
}

@test "harness_tally: pi's cost and tokens summed over its transcript, its context the last turn's" {
  mkdir -p "$PI_CODING_AGENT_SESSION_DIR/--repo--"
  cat > "$PI_CODING_AGENT_SESSION_DIR/--repo--/2026-10-17T00-00-00-000Z_sid2.jsonl" <<'EOF'
{"type":"session","id":"sid2"}
{"type":"message","message":{"role":"user","content":"do it"}}
{"type":"message","message":{"role":"assistant","usage":{"input":12937,"output":21,"cacheRead":0,"cacheWrite":0,"totalTokens":12958,"cost":{"total":0.002}}}}
{"type":"message","message":{"role":"assistant","usage":{"input":1473,"output":44,"cacheRead":11520,"cacheWrite":10,"totalTokens":13047,"cost":{"total":0.0005}}}}
EOF
  run harness_tally pi sid2
  [ "$status" -eq 0 ]
  [ "$(prc_val "$output" 'f"{cost}|{input}|{cache_create}|{cache_read}|{output}|{turns}|{context}"')" = "0.0025|14410|10|11520|65|2|13047" ]
}

@test "harness_run: a pi step's usage is what its transcript gained during the step" {
  harness_stubs
  local t="$PI_CODING_AGENT_SESSION_DIR/--repo--/2026-10-17T00-00-00-000Z_sid2.jsonl"
  mkdir -p "${t%/*}"
  echo '{"type":"message","message":{"role":"assistant","usage":{"input":100,"output":10,"cacheRead":0,"cacheWrite":0,"totalTokens":110,"cost":{"total":0.5}}}}' > "$t"
  # The stub's step adds a turn to the transcript.
  echo '{"type":"message","message":{"role":"assistant","usage":{"input":20,"output":5,"cacheRead":100,"cacheWrite":0,"totalTokens":125,"cost":{"total":0.25}}}}' > "$FX/append"
  cat > "$TMP/fakebin/pi" <<EOF
#!/usr/bin/env bash
cat "\$FX/append" >> '$t'
cat "\$FX/out"
EOF
  printf 'Done.\n\n```json\n{"ok": true}\n```\n' > "$FX/out"
  run harness_run pi sid2 resume 30 "do it" '{}'
  [ "$status" -eq 0 ]
  [ "$(usage_last 'f"{cost_usd}|{input}|{cache_read}|{output}|{turns}|{context}|{duration_ms >= 0}"')" = "0.25|20|100|5|1|125|true" ]
}

@test "harness_run: a claude record carries its session's context, from the transcript" {
  harness_stubs
  mkdir -p "$CLAUDE_CONFIG_DIR/projects/-repo"
  echo '{"message":{"role":"assistant","usage":{"input_tokens":1,"cache_creation_input_tokens":2,"cache_read_input_tokens":3,"output_tokens":4}},"type":"assistant"}' \
    > "$CLAUDE_CONFIG_DIR/projects/-repo/sid1.jsonl"
  run harness_run claude sid1 new 30 "do it" '{}'
  [ "$status" -eq 0 ]
  [ "$(usage_last context)" = 10 ]
}

@test "branch_spend: this repo's branch's cost_usd summed over usage.sup; 0 with none" {
  run branch_spend feat
  [ "$status" -eq 0 ]
  [ "$output" = 0 ]
  mkdir -p "$SHEEPDOG_STATE_DIR"
  cat > "$SHEEPDOG_STATE_DIR/usage.sup" <<EOF
{time:2026-10-16T20:33:06Z,repo:"$TMP/repo",branch:"feat",step:"review",harness:"claude",session:"a",mode:"new",status:0,cost_usd:1.25,input:1}
{time:2026-10-16T20:34:06Z,repo:"$TMP/repo",branch:"other",step:"review",harness:"claude",session:"b",mode:"new",status:0,cost_usd:4.,input:1}
{time:2026-10-16T20:35:06Z,repo:"/elsewhere",branch:"feat",step:"review",harness:"claude",session:"c",mode:"new",status:0,cost_usd:8.,input:1}
{time:2026-10-16T20:36:06Z,repo:"$TMP/repo",branch:"feat",step:"implement",harness:"pi",session:"d",mode:"new",status:0,cost_usd:null,input:null}
{time:2026-10-16T20:37:06Z,repo:"$TMP/repo",branch:"feat",step:"implement",harness:"claude",session:"a",mode:"resume",status:1,cost_usd:0.5,input:1}
EOF
  run branch_spend feat
  [ "$status" -eq 0 ]
  [ "$output" = 1.75 ]
}

@test "branch_spend: a line that isn't a whole record (a cut-off append, a stray edit) is skipped, with a warning" {
  mkdir -p "$SHEEPDOG_STATE_DIR"
  cat > "$SHEEPDOG_STATE_DIR/usage.sup" <<EOF
{time:2026-10-16T20:33:06Z,repo:"$TMP/repo",branch:"feat",step:"review",harness:"claude",session:"a",mode:"new",status:0,cost_usd:1.25}
not sup {
{time:2026-10-16T20:34:06Z,repo:"$TMP/repo",branch:"fe
{time:2026-10-16T20:35:06Z,repo:"$TMP/repo",branch:"feat",step:"review",harness:"claude",session:"a",mode:"new",status:0,cost_usd:0.5}
EOF
  run --separate-stderr branch_spend feat
  [ "$status" -eq 0 ]
  [ "$output" = 1.75 ]
  [[ "$stderr" == *"2 line(s) of $SHEEPDOG_STATE_DIR/usage.sup aren't usage records; their cost isn't counted"* ]]
  # Shaped like a record but not one: the file can't be read.
  echo '{time:soon}' >> "$SHEEPDOG_STATE_DIR/usage.sup"
  run branch_spend feat
  [ "$status" -ne 0 ]
}

# claude_stub_appends <session-id> <transcript-line|"">: a claude stub that
# adds the line to the session's transcript, then fails without a result, as a
# step killed by stepTimeout does.
claude_stub_appends() {
  local t="$CLAUDE_CONFIG_DIR/projects/-repo/$1.jsonl"
  mkdir -p "${t%/*}"
  echo '{"message":{"role":"assistant","usage":{"input_tokens":1,"output_tokens":1}},"type":"assistant"}' > "$t"
  printf '%s\n' "$2" > "$FX/append"
  cat > "$TMP/fakebin/claude" <<EOF
#!/usr/bin/env bash
printf '%s\n' "\$@" > "\$FX/args"
grep . "\$FX/append" >> '$t'
echo "timed out after 30s" >&2
exit 124
EOF
}

@test "harness_run: a claude step that ran turns but printed no result (timed out) is charged its cap, marked estimated" {
  harness_stubs
  claude_stub_appends sid1 '{"message":{"role":"assistant","usage":{"input_tokens":9,"output_tokens":9}},"type":"assistant"}'
  run harness_run claude sid1 resume 30 "do it" '{}'
  [ "$status" -ne 0 ]
  [ "$(usage_last 'f"{typeof(cost_usd)}|{cost_usd == 5.}|{estimated}|{nul(input)}"')" = "<float64>|true|true|true" ]
  # With no cap there's nothing to charge: unknown, but still marked.
  echo '{"settings": {"stepBudget": 0}}' > "$SHEEPDOG_CONFIG"
  run harness_run claude sid1 resume 30 "do it" '{}'
  [ "$(usage_last 'f"{nul(cost_usd)}|{estimated}"')" = "true|true" ]
}

@test "harness_run: a timed-out claude step whose cap came from harnesses.claude.args is charged that cap" {
  harness_stubs
  claude_stub_appends sid1 '{"message":{"role":"assistant","usage":{"input_tokens":9,"output_tokens":9}},"type":"assistant"}'
  run harness_run claude sid1 resume 30 "do it" '{}' '{"harness": "claude", "args": "--verbose --max-budget-usd 7.5"}'
  [ "$(usage_last 'f"{cost_usd == 7.5}|{estimated}"')" = "true|true" ]
  run harness_run claude sid1 resume 30 "do it" '{}' '{"harness": "claude", "args": "--max-budget-usd=2"}'
  [ "$(usage_last 'f"{cost_usd == 2.}|{estimated}"')" = "true|true" ]
}

@test "harness_run: a claude step that failed before any turn costs nothing, and isn't an estimate" {
  harness_stubs
  claude_stub_appends sid1 ""
  run harness_run claude sid1 resume 30 "do it" '{}'
  [ "$status" -ne 0 ]
  [ "$(usage_last 'f"{nul(cost_usd)}|{estimated}"')" = "true|false" ]
}

@test "harness_run: a successful run keeps the harness's stderr off the terminal" {
  harness_stubs
  run --separate-stderr harness_run claude sid1 new 30 "do it" '{}'
  [ "$status" -eq 0 ]
  [ "$(jq -c . <<<"$output")" = '{"ok":true}' ]
  [ -z "$stderr" ]
}

@test "harness_run: a failed run shows the harness's stderr" {
  harness_stubs
  echo 1 > "$FX/status"
  run --separate-stderr harness_run claude sid1 new 30 "do it" '{}'
  [ "$status" -ne 0 ]
  [[ "$stderr" == *'"subtype":"init"'* ]]
}

@test "harness_run: an error result is a failure, so the stderr shows" {
  harness_stubs
  echo '{"type": "result", "is_error": true, "result": "out of turns"}' > "$FX/out"
  run --separate-stderr harness_run claude sid1 new 30 "do it" '{}'
  [ "$status" -ne 0 ]
  [ -z "$output" ]
  [[ "$stderr" == *'"subtype":"init"'* ]]
  [[ "$stderr" == *"out of turns"* ]]
}

@test "harness_run: an error result without result text names its subtype" {
  harness_stubs
  echo '{"type": "result", "subtype": "error_max_turns", "is_error": true}' > "$FX/out"
  run --separate-stderr harness_run claude sid1 new 30 "do it" '{}'
  [ "$status" -ne 0 ]
  [[ "$stderr" == *error_max_turns* ]]
}

@test "harness_run: a success result with nothing usable doesn't print its subtype" {
  harness_stubs
  echo '{"type": "result", "subtype": "success", "is_error": false, "result": ""}' > "$FX/out"
  run --separate-stderr harness_run claude sid1 new 30 "do it" '{}'
  [ "$status" -ne 0 ]
  [[ "$stderr" != *success* ]]
}

@test "harness_run: pi replying without a json block shows the end of the reply" {
  harness_stubs
  printf 'I need to know which file you mean.\n' > "$FX/out"
  run --separate-stderr harness_run pi sid1 new 30 "do it" '{}'
  [ "$status" -ne 0 ]
  [[ "$stderr" == *"which file you mean"* ]]
}

@test "harness_run: claude failing with stdout that isn't JSON shows its end" {
  harness_stubs
  printf 'Error: unknown option --bogus\n' > "$FX/out"
  echo 1 > "$FX/status"
  run --separate-stderr harness_run claude sid1 new 30 "do it" '{}'
  [ "$status" -ne 0 ]
  [[ "$stderr" == *"unknown option --bogus"* ]]
}

@test "harness_run: claude's error text is cut to its last 20 lines" {
  harness_stubs
  jq -n '{type: "result", is_error: true, result: ([range(1; 31) | "line \(.)"] | join("\n"))}' > "$FX/out"
  run --separate-stderr harness_run claude sid1 new 30 "do it" '{}'
  [ "$status" -ne 0 ]
  [[ "$stderr" == *"line 11"$'\n'*"line 30"* ]]
  [[ "$stderr" != *"line 10"$'\n'* ]]
}

@test "harness_run: pi's stderr follows the same rule" {
  harness_stubs
  printf 'Done.\n\n```json\n{"ok": true}\n```\n' > "$FX/out"
  run --separate-stderr harness_run pi sid1 new 30 "do it" '{}'
  [ "$status" -eq 0 ]
  [ -z "$stderr" ]
  echo 1 > "$FX/status"
  run --separate-stderr harness_run pi sid1 new 30 "do it" '{}'
  [ "$status" -ne 0 ]
  [[ "$stderr" == *'"subtype":"init"'* ]]
}

# step_guidance's tests: a repo whose base commit can hold .sheepdog/ files.
guidance_repo() {
  export SHEEPDOG_CONFIG="$TMP/global.json"
  printf 'shipped\n' > "$TMP/default.md"
}

@test "step_guidance: empty setting, no convention file: the shipped default" {
  guidance_repo
  [ "$(step_guidance reviewPrompt feat HEAD "$TMP/default.md")" = shipped ]
}

@test "step_guidance: empty setting: .sheepdog/<name>.md in the base commit, kebab-cased" {
  guidance_repo
  mkdir -p .sheepdog && printf 'repo review-fix\n' > .sheepdog/review-fix.md
  git add .sheepdog && git commit -qm guidance
  base=$(git rev-parse HEAD)
  printf 'branch rewrite\n' > .sheepdog/review-fix.md && git commit -qam rewrite
  [ "$(step_guidance reviewFixPrompt feat "$base" "$TMP/default.md")" = "repo review-fix" ]
}

@test "step_guidance: a set path wins over the convention file" {
  guidance_repo
  mkdir -p .sheepdog && printf 'convention\n' > .sheepdog/review.md && printf 'named\n' > .sheepdog/mine.md
  git add .sheepdog && git commit -qm guidance
  printf '{"settings": {"reviewPrompt": ".sheepdog/mine.md"}}\n' > "$SHEEPDOG_CONFIG"
  [ "$(step_guidance reviewPrompt feat HEAD "$TMP/default.md")" = named ]
}

@test "step_guidance: no trusted base: the convention is skipped, a relative path refused" {
  guidance_repo
  mkdir -p .sheepdog && printf 'branch-written\n' > .sheepdog/review.md
  git add .sheepdog && git commit -qm guidance
  [ "$(step_guidance reviewPrompt feat "" "$TMP/default.md")" = shipped ]
  printf '{"settings": {"reviewPrompt": ".sheepdog/review.md"}}\n' > "$SHEEPDOG_CONFIG"
  run step_guidance reviewPrompt feat "" "$TMP/default.md"
  [ "$status" -ne 0 ]
  [[ "$output" == *"no base commit"* ]]
}

# ── protected paths: CI config an agent's commit may not publish (#124) ─────

@test "protected_paths: the default list, with no setting and no .sheepdog/protected-paths; the list's file is protected too" {
  guidance_repo
  run protected_paths feat HEAD
  [ "$status" -eq 0 ]
  [ "$output" = "$(printf '%s\n' .github/workflows/ .github/actions/ .rwx/ .captain/ .circleci/ .gitlab-ci.yml .buildkite/ .sheepdog/protected-paths)" ]
}

@test "protected_paths: .sheepdog/protected-paths in the base commit, comments and blanks dropped; the branch's own copy ignored; the file itself always protected" {
  guidance_repo
  mkdir -p .sheepdog && printf '# ours\n.github/\n\n  deploy/pipeline.yml  # the deploy\n' > .sheepdog/protected-paths
  git add .sheepdog && git commit -qm paths
  base=$(git rev-parse HEAD)
  printf 'nothing\n' > .sheepdog/protected-paths && git commit -qam weaken
  [ "$(protected_paths feat "$base")" = "$(printf '%s\n' .github/ deploy/pipeline.yml .sheepdog/protected-paths)" ]
  printf '# none\n' > .sheepdog/protected-paths && git commit -qam emptied
  [ "$(protected_paths feat HEAD)" = .sheepdog/protected-paths ]
  [ "$(protected_paths feat "")" = "$(protected_paths feat HEAD~3)" ]   # no base: the default list
}

@test "protected_paths: settings.protectedPaths wins, and an empty list protects nothing" {
  guidance_repo
  mkdir -p .sheepdog && printf '.github/\n' > .sheepdog/protected-paths
  git add .sheepdog && git commit -qm paths
  printf '{"settings": {"protectedPaths": ["ci/", "Makefile"]}}\n' > "$SHEEPDOG_CONFIG"
  [ "$(protected_paths feat HEAD)" = "$(printf '%s\n' ci/ Makefile)" ]
  printf '{"settings": {"protectedPaths": []}}\n' > "$SHEEPDOG_CONFIG"
  run protected_paths feat HEAD
  [ "$status" -eq 0 ]
  [ -z "$output" ]
}

@test "protected_hits: a path covers itself and what's under it, trailing / or not" {
  local list=$'.github/workflows/\n.gitlab-ci.yml\n.rwx'
  run protected_hits "$list" $'.github/workflows/test.yml\n.github/dependabot.yml\n.gitlab-ci.yml\n.gitlab-ci.yml.bak\nsrc/.github/workflows/x.yml\n.rwx/tests.yml\n.rwxrc'
  [ "$output" = "$(printf '%s\n' .github/workflows/test.yml .gitlab-ci.yml .rwx/tests.yml)" ]
  run protected_hits "" ".github/workflows/test.yml"
  [ -z "$output" ]
}
