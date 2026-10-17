#!/usr/bin/env bats

bats_require_minimum_version 1.5.0

# Test suite for libexec/sd-watch (the observe-only watcher pass).
#
# Real git for repos, enrollment and fetch. sd state is faked: it serves
# $PS_FIXTURES/<branch>.json, so each glyph/notification rule can be driven
# directly. Notifications go to a stub configured as sheepdog.notify.

SD_WATCH="$BATS_TEST_DIRNAME/sd-watch"

setup() {
  TMP="$(cd "$BATS_TEST_TMPDIR" && pwd -P)"
  export GIT_CONFIG_GLOBAL="$TMP/gitconfig-global"
  export GIT_CONFIG_SYSTEM=/dev/null
  printf '[user]\n\temail = t@example.com\n\tname = Tester\n[init]\n\tdefaultBranch = main\n' > "$GIT_CONFIG_GLOBAL"
  export SHEEPDOG_STATE_DIR="$TMP/state"
  export STATUS_DIR="$TMP/status"
  export PS_FIXTURES="$TMP/ps"
  export SHEEPDOG_CONFIG="$TMP/global.json"
  SD_CONFIG="$BATS_TEST_DIRNAME/sd-config"
  mkdir -p "$PS_FIXTURES" "$TMP/fakebin"

  cat > "$TMP/fakebin/sd-state" <<'EOF'
#!/usr/bin/env bash
b=${@: -1}
echo "$PWD $b" >> "$PS_FIXTURES/calls"
f="$PS_FIXTURES/${b//\//__}.json"
[[ -f $f ]] || { echo "sd state: no fixture for $b" >&2; exit 1; }
cat "$f"
EOF
  cat > "$TMP/fakebin/notify" <<'EOF'
#!/usr/bin/env bash
printf '%s|%s\n' "$1" "$2" >> "$PS_FIXTURES/notified"
EOF
  chmod +x "$TMP/fakebin/sd-state" "$TMP/fakebin/notify"
  export PATH="$TMP/fakebin:$PATH"
  # Never the real macOS notifier: every config a test writes keeps this one.
  "$SD_CONFIG" set --global settings.notify "$TMP/fakebin/notify"

  git init -q --bare "$TMP/origin.git"
  git clone -q "$TMP/origin.git" "$TMP/repo" 2>/dev/null
  cd "$TMP/repo"
  git commit -q --allow-empty -m init && git push -q origin main 2>/dev/null
  git switch -qc feat
  enroll feat
}

enroll() { git -C "$TMP/repo" config "branch.$1.sheepdog" true; register "$TMP/repo"; }
register() {
  grep -qxF "$1" <<<"$(git config --global --get-all sheepdog.repo || true)" \
    || git config --global --add sheepdog.repo "$1"
}

# ps_fixture <branch> [jq-assignments...] — sd state's JSON. Default: ready to merge.
# .next comes from the real lib/next.sh, as in sd state, unless an assignment sets it.
ps_fixture() {
  local b=$1; shift
  local j
  j=$(jq -n --arg b "$b" '{
    branch: $b, pr: {number: 7, state: "OPEN", draft: false, merge_state: "CLEAN", review_decision: null, review_requests: []},
    parent: {name: "main", merged: false},
    sync: {target: "main", default: "main", behind: 0, up_to_date: true, conflict: false},
    push: {remote_sha: "abc", ahead: 0, behind: 0, diverged: false, remote_ours: true},
    fresh: true, stale: [],
    ci: {verdict: "green", failing: [], pending: [], for_head: true},
    review: {local: {status: "covered"}, bot: {status: "none"}, accepted: {status: "none"}},
    threads: {owed_bot: 0, owed_human: 0, outdated: 0, blocking: 0, info: 0, items: []},
    session: {enrolled: true, locked: false}, errors: []}')
  local a
  for a; do j=$(jq "$a" <<<"$j"); done
  if [[ $(jq 'has("next")' <<<"$j") == false ]]; then
    j=$(jq --argjson n "$(printf '%s\n' "$j" | bash5 -c 'source "$1/../lib/config.sh" && source "$1/../lib/next.sh" && prn_next origin' _ "$BATS_TEST_DIRNAME")" \
          '. + {next: $n}' <<<"$j")
  fi
  printf '%s\n' "$j" > "$PS_FIXTURES/${b//\//__}.json"
}

pass() { (cd "$TMP" && "$SD_WATCH") || { echo "pass failed"; return 1; }; }
glyph() { cut -f2 "$STATUS_DIR/$1.pr"; }
notified() { cat "$PS_FIXTURES/notified" 2>/dev/null || true; }

# ── basics ───────────────────────────────────────────────────────────────────

@test "runs under bash 5 via the bash5 shebang" {
  run head -1 "$SD_WATCH"
  [ "$output" = "#!/usr/bin/env bash5" ]
}

@test "no registered repos: nothing to do, exit 0" {
  git config --global --unset-all sheepdog.repo
  pass
  [ ! -e "$PS_FIXTURES/calls" ]
}

@test "only enrolled branches are checked, from the repo directory" {
  git branch other
  ps_fixture feat
  pass
  [ "$(cat "$PS_FIXTURES/calls")" = "$TMP/repo feat" ]
}

@test "a registered repo that no longer exists is skipped" {
  register "$TMP/gone"
  ps_fixture feat
  pass
  [ "$(cat "$PS_FIXTURES/calls")" = "$TMP/repo feat" ]
}

@test "fetches before checking" {
  git clone -q "$TMP/origin.git" "$TMP/other" 2>/dev/null
  git -C "$TMP/other" commit -q --allow-empty -m moved && git -C "$TMP/other" push -q origin main 2>/dev/null
  ps_fixture feat
  pass
  [ "$(git rev-parse origin/main)" = "$(git -C "$TMP/other" rev-parse main)" ]
}

@test "one branch's sd state failing doesn't stop the others" {
  git branch other && enroll other
  ps_fixture other
  pass
  [ -f "$SHEEPDOG_STATE_DIR/last-pass" ]
  grep -q "other" "$SHEEPDOG_STATE_DIR/last-pass" || false
  grep -q "feat.*sd state failed" "$SHEEPDOG_STATE_DIR/last-pass"
}

# ── glyph ────────────────────────────────────────────────────────────────────

@test "glyph is written for the worktree the branch is checked out in, with the branch" {
  ps_fixture feat
  pass
  [ "$(cat "$STATUS_DIR/repo.pr")" = "feat	✅" ]
}

@test "glyph follows a linked worktree" {
  git switch -q main
  git worktree add -q "$TMP/feat-wt" feat
  ps_fixture feat
  pass
  [ "$(glyph feat-wt)" = "✅" ]
  [ ! -e "$STATUS_DIR/repo.pr" ]
}

@test "a branch checked out nowhere gets no glyph but is still checked" {
  git switch -q main
  ps_fixture feat
  pass
  [ ! -e "$STATUS_DIR/repo.pr" ]
  grep -q feat "$SHEEPDOG_STATE_DIR/last-pass"
}

@test "a glyph left from a branch no longer enrolled is removed" {
  ps_fixture feat
  pass
  git config --unset branch.feat.sheepdog
  pass
  [ ! -e "$STATUS_DIR/repo.pr" ]
}

@test "a pass sweeps refs/sheepdog/<b>/* whose branch no longer exists" {
  git update-ref refs/sheepdog/feat/base HEAD
  git update-ref refs/sheepdog/gone/base HEAD
  git update-ref refs/sheepdog/gone/local HEAD
  git update-ref refs/sheepdog/also/gone/accepted HEAD
  ps_fixture feat
  pass
  [ -n "$(git for-each-ref refs/sheepdog/feat/)" ]
  [ -z "$(git for-each-ref refs/sheepdog/gone/)" ]
  [ -z "$(git for-each-ref refs/sheepdog/also/)" ]
}

@test "the sweep keeps the pins of a branch that exists but isn't enrolled" {
  git switch -qc parked && git switch -q feat
  git update-ref refs/sheepdog/parked/base HEAD
  ps_fixture feat
  pass
  [ -n "$(git for-each-ref refs/sheepdog/parked/)" ]
}

@test "glyph: CI red or blocked, conflict, human items, foreign push are ❌" {
  local a
  for a in '.ci.verdict = "red"' '.ci.verdict = "blocked" | .ci.blocked = ["wf"]' '.sync.up_to_date = false | .sync.conflict = true' \
           '.threads.owed_human = 1' '.push.behind = 1 | .push.theirs = 1 | .push.remote_ours = false'; do
    ps_fixture feat "$a"
    pass
    [ "$(glyph repo)" = "❌" ] || { echo "for: $a"; return 1; }
  done
}

@test "glyph: review findings are 🔧 while the agent fixes them, ❌ once chrismo decides" {
  local f='.review.local.status = "uncovered" | .review.findings = {sha: "x", round: 1, n: 1, at_head: true, declined: false, cap: 3, items: []}'
  ps_fixture feat "$f"
  pass
  [ "$(glyph repo)" = "🔧" ]
  ps_fixture feat "$f | .review.findings.round = 3"
  pass
  [ "$(glyph repo)" = "❌" ]
}

@test "glyph: waiting on the review bot is ⏳ and quiet; the bot silent past its timeout is ❌ and notified" {
  ps_fixture feat '.bot = {command: "/review-bot", timeout: 3600} | .review.bot = {status: "covered", replied: false, waited: 60}'
  pass
  [ "$(glyph repo)" = "⏳" ]
  [ -z "$(notified)" ]
  ps_fixture feat '.bot = {command: "/review-bot", timeout: 3600} | .review.bot = {status: "covered", replied: false, waited: 9000}'
  pass
  [ "$(glyph repo)" = "❌" ]
  [[ "$(notified)" == *"hasn't answered"* ]]
}

@test "glyph: work an agent will do later is 🔧" {
  local a
  for a in '.sync.up_to_date = false | .sync.behind = 2 | .sync.target = "base" | .sync.old_base = "abc"' \
           '.sync.up_to_date = false | .parent.merged = true | .sync.old_base = "abc"' '.threads.owed_bot = 1' \
           '.push.ahead = 1' '.pr = null' '.review.local.status = "uncovered"'; do
    ps_fixture feat "$a"
    pass
    [ "$(glyph repo)" = "🔧" ] || { echo "for: $a"; return 1; }
  done
}

@test "glyph: CI pending is ⏳" {
  ps_fixture feat '.ci.verdict = "pending"'
  pass
  [ "$(glyph repo)" = "⏳" ]
}

@test "glyph: unknown state (errors, not fresh) is ⚠️" {
  ps_fixture feat '.errors = ["gh: down"] | .fresh = null'
  pass
  [ "$(glyph repo)" = "⚠️" ]
}

@test "glyph: merged 🏁, closed 🚫" {
  ps_fixture feat '.pr.state = "MERGED"'
  pass
  [ "$(glyph repo)" = "🏁" ]
  ps_fixture feat '.pr.state = "CLOSED"'
  pass
  [ "$(glyph repo)" = "🚫" ]
}

@test "glyph: only behind the default branch is still ✅" {
  ps_fixture feat '.sync.up_to_date = false | .sync.behind = 40'
  pass
  [ "$(glyph repo)" = "✅" ]
}

@test "glyph: ❌ outranks 🔧" {
  # Human items are ❌ and the push is 🔧. (ci-red would yield to the push while
  # commits are unpushed, so it can't carry this test.)
  ps_fixture feat '.threads.owed_human = 1 | .push.ahead = 3'
  pass
  [ "$(glyph repo)" = "❌" ]
}

@test "glyph: cancelled checks are 🔧, CI for an older commit is ⏳" {
  ps_fixture feat '.ci.verdict = "inconclusive"'
  pass
  [ "$(glyph repo)" = "🔧" ]
  ps_fixture feat '.ci.for_head = false'
  pass
  [ "$(glyph repo)" = "⏳" ]
}

@test "glyph: a repo with no checks at all can be ready" {
  ps_fixture feat '.ci.verdict = "none" | .ci.for_head = true'
  pass
  [ "$(glyph repo)" = "✅" ]
}

@test "glyph: follows sd state's next steps, not its raw fields" {
  ps_fixture feat '.next = [{key: "ci-wait", who: "wait", say: "CI running", cmd: null}]'
  pass
  [ "$(glyph repo)" = "⏳" ]
  ps_fixture feat '.next = [{key: "bot", who: "agent", say: "address 1 bot item", cmd: null}]'
  pass
  [ "$(glyph repo)" = "🔧" ]
}

@test "glyph: info-only bot items, waiting on chrismo's read, are 🔧" {
  ps_fixture feat '.threads.owed_bot = 1 | .threads.info = 1'
  pass
  [ "$(glyph repo)" = "🔧" ]
  [ -z "$(notified)" ]
}

@test "glyph: sd state output with no next steps is ⚠️" {
  ps_fixture feat '.next = null'
  pass
  [ "$(glyph repo)" = "⚠️" ]
  grep -q "feat.*no next steps" "$SHEEPDOG_STATE_DIR/last-pass"
}

@test "glyph: waiting on a required review is ⏳, never ✅" {
  ps_fixture feat '.pr.merge_state = "BLOCKED" | .pr.review_decision = "REVIEW_REQUIRED" | .pr.review_requests = ["alice"]'
  pass
  [ "$(glyph repo)" = "⏳" ]
  [ -z "$(notified)" ]
}

@test "glyph: GitHub blocking the merge for another reason is 🔧, never ✅" {
  ps_fixture feat '.pr.merge_state = "BLOCKED"'
  pass
  [ "$(glyph repo)" = "🔧" ]
  [ -z "$(notified)" ]
}

@test "glyph: a draft is never ready" {
  ps_fixture feat '.pr.draft = true'
  pass
  [ "$(glyph repo)" = "🔧" ]
  [ -z "$(notified)" ]
}

@test "glyph: sd state failing shows ⚠️ instead of dropping the glyph" {
  ps_fixture feat
  pass
  rm "$PS_FIXTURES/feat.json"
  pass
  [ "$(glyph repo)" = "⚠️" ]
}

@test "unparseable sd state output is ⚠️, and the next good pass still notifies" {
  printf 'warning: junk\n{"branch":' > "$PS_FIXTURES/feat.json"
  pass
  [ "$(glyph repo)" = "⚠️" ]
  grep -q "feat.*unparseable" "$SHEEPDOG_STATE_DIR/last-pass" || false
  ps_fixture feat '.ci.verdict = "red" | .ci.failing = ["lint"]'
  pass
  [ "$(notified)" = "repo feat|fix CI: lint" ]
}

@test "sd state stderr on success doesn't corrupt its JSON" {
  ps_fixture feat
  cat > "$TMP/fakebin/sd-state" <<'SH'
#!/usr/bin/env bash
echo "warning: something" >&2
cat "$PS_FIXTURES/${@: -1}.json"
SH
  pass
  [ "$(glyph repo)" = "✅" ]
}

@test "a hung sd state times out instead of stalling the pass" {
  git branch other && enroll other
  ps_fixture other
  cat > "$TMP/fakebin/sd-state" <<'SH'
#!/usr/bin/env bash
[[ ${@: -1} == feat ]] && sleep 30
cat "$PS_FIXTURES/${@: -1}.json"
SH
  SECONDS=0
  SHEEPDOG_WATCH_TIMEOUT=1 pass
  (( SECONDS < 10 )) || false
  grep -q "feat.*sd state failed" "$SHEEPDOG_STATE_DIR/last-pass" || false
  grep -q "other.*✅" "$SHEEPDOG_STATE_DIR/last-pass"
}

@test "a failed state write leaves no temp file behind" {
  local real
  real=$(bash5 -c 'source "$1/../lib/config.sh"; command -v "$(prc_superdb_binary)"' _ "$BATS_TEST_DIRNAME")
  [[ $real != */shims/* ]] || skip "super is an asdf shim, which the fake's ASDF_DATA_DIR would break"
  cat > "$TMP/fakebin/super" <<EOF
#!/usr/bin/env bash
for a; do [[ \$a == *"state: s"* ]] && { echo '{"partial":'; exit 1; }; done
exec "$real" "\$@"
EOF
  chmod +x "$TMP/fakebin/super"
  export ASDF_DATA_DIR="$TMP/no-asdf"
  ps_fixture feat
  run --separate-stderr pass
  [[ "$stderr" == *"couldn't save"*"feat.json"* ]] || false
  [ -d "$SHEEPDOG_STATE_DIR/branches" ]
  [ -z "$(find "$SHEEPDOG_STATE_DIR/branches" -name '*.tmp')" ]
  [ -z "$(find "$SHEEPDOG_STATE_DIR/branches" -name 'feat.json')" ]
}

@test "a failed fetch keeps git's reason" {
  git remote set-url origin "$TMP/nowhere.git"
  ps_fixture feat
  pass
  grep -q "fetch from origin failed: .*nowhere" "$SHEEPDOG_STATE_DIR/last-pass"
}

# ── notifications ────────────────────────────────────────────────────────────

@test "notify: ready to merge, once" {
  ps_fixture feat
  pass
  pass
  [ "$(notified)" = "repo feat|ready to merge" ]
}

@test "notify: CI red names the failing checks" {
  ps_fixture feat '.ci.verdict = "red" | .ci.failing = ["lint", "test"]'
  pass
  [ "$(notified)" = "repo feat|fix CI: lint, test" ]
}

@test "notify: a reason that clears and comes back notifies again" {
  ps_fixture feat '.ci.verdict = "red" | .ci.failing = ["lint"]'
  pass
  ps_fixture feat '.ci.verdict = "pending"'
  pass
  ps_fixture feat '.ci.verdict = "red" | .ci.failing = ["lint"]'
  pass
  [ "$(notified | grep -c 'fix CI')" = 2 ]
}

@test "notify: agent-later work and pending CI stay quiet" {
  ps_fixture feat '.ci.verdict = "pending" | .push.ahead = 1 | .threads.owed_bot = 2'
  pass
  [ -z "$(notified)" ]
}

@test "notify: a restack undone from outside is ❌ and notifies (#47)" {
  ps_fixture feat '.sync.undone = 1 | .sync.up_to_date = false'
  pass
  run notified
  [ "${lines[0]}" = "repo feat|the restack was undone (1 of main's commits is a copy on feat; a plain git push after a restack?): redo it, and don't sd act push before that, which would publish the copies" ]
  run "$SD_WATCH" status
  [[ "$output" == *"feat"*"❌"* ]] || false
}

@test "notify: CI config an agent changed is ❌ and notifies; chrismo reviews and pushes (#124)" {
  ps_fixture feat '.push.ahead = 1 | .push.held = {"sha": "abc", "paths": [".github/workflows/test.yml"]}'
  pass
  run notified
  [ "${lines[0]}" = "repo feat|the agent changed CI config (.github/workflows/test.yml): review, then sd act push" ]
  run "$SD_WATCH" status
  [[ "$output" == *"feat"*"❌"* ]] || false
}

@test "notify: human items, conflict, foreign push, merged" {
  ps_fixture feat '.threads.owed_human = 2'
  pass
  ps_fixture feat '.sync.up_to_date = false | .sync.conflict = true'
  pass
  ps_fixture feat '.push.behind = 1 | .push.theirs = 1 | .push.remote_ours = false'
  pass
  ps_fixture feat '.pr.state = "MERGED"'
  pass
  run notified
  [ "${lines[0]}" = "repo feat|reply to 2 human items" ]
  [ "${lines[1]}" = "repo feat|restack onto main conflicts: resolve it" ]
  [ "${lines[2]}" = "repo feat|origin/feat has 1 commit this copy has never had" ]
  [ "${lines[3]}" = "repo feat|merged: drop the branch's pins and enrollment, then remove this worktree" ]
}

# ── a branch deleted with its state left behind (#64) ───────────────────────

# gh_pull <json>: what the fake gh serves for GET repos/{owner}/{repo}/pulls/<n>.
gh_pull() {
  printf '%s\n' "$1" > "$PS_FIXTURES/gh-pull"
  cat > "$TMP/fakebin/gh" <<'SH'
#!/usr/bin/env bash
echo "$*" >> "$PS_FIXTURES/gh-calls"
[[ -f $PS_FIXTURES/gh-pull ]] || { echo "gh: API rate limit exceeded" >&2; exit 1; }
cat "$PS_FIXTURES/gh-pull"
SH
  chmod +x "$TMP/fakebin/gh"
}

# gone <branch>: what gh pr merge --delete-branch does from the branch's checkout.
gone() { git switch -q main && git branch -qD "$1"; }

state_files() { find "$SHEEPDOG_STATE_DIR/branches" -name '*.json' 2>/dev/null; }

@test "gone: an enrolled branch deleted after its PR merged is notified merged once, and its state dropped" {
  ps_fixture feat '.ci.verdict = "pending"'
  pass
  gone feat
  gh_pull '{"state": "closed", "merged": true}'
  pass
  [ "$(notified)" = "repo feat|merged (#7); the local branch is already gone" ]
  grep -q "feat.*🏁.*merged (#7)" "$SHEEPDOG_STATE_DIR/last-pass"
  [ -z "$(state_files)" ]
  pass
  [ "$(notified | wc -l | tr -d ' ')" = 1 ]
  [ "$(cat "$PS_FIXTURES/gh-calls")" = "api repos/{owner}/{repo}/pulls/7" ]
}

@test "gone: a branch with a slash in its name is found" {
  git switch -qc 12/doc && enroll 12/doc
  ps_fixture 12/doc '.ci.verdict = "pending"'
  pass
  gone 12/doc
  gh_pull '{"state": "closed", "merged": true}'
  pass
  [[ "$(notified)" == *"repo 12/doc|merged (#7)"* ]] || false
  [ -z "$(state_files | grep doc)" ]
  [ -z "$(find "$SHEEPDOG_STATE_DIR/branches" -mindepth 2 -type d)" ]
}

@test "gone: GitHub not answering keeps the state, and the next pass tries again" {
  ps_fixture feat '.ci.verdict = "pending"'
  pass
  gone feat
  gh_pull '{}' && rm "$PS_FIXTURES/gh-pull"
  pass
  grep -q "feat.*⚠️.*branch is gone; GitHub didn't say what became of #7" "$SHEEPDOG_STATE_DIR/last-pass"
  [ -n "$(state_files)" ]
  [ -z "$(notified)" ]
  gh_pull '{"state": "closed", "merged": true}'
  pass
  [ "$(notified)" = "repo feat|merged (#7); the local branch is already gone" ]
}

@test "gone: a PR GitHub says doesn't exist drops the state" {
  git switch -qc 12/doc && enroll 12/doc
  ps_fixture 12/doc '.ci.verdict = "pending"'
  pass
  gone 12/doc
  cat > "$TMP/fakebin/gh" <<'SH'
#!/usr/bin/env bash
echo "gh: Not Found (HTTP 404)" >&2
exit 1
SH
  chmod +x "$TMP/fakebin/gh"
  pass
  grep -q "12/doc.*⚠️.*branch is gone; GitHub has no #7" "$SHEEPDOG_STATE_DIR/last-pass"
  [ -z "$(state_files | grep doc)" ]
  [ -z "$(find "$SHEEPDOG_STATE_DIR/branches" -mindepth 2 -type d)" ]
  [ -z "$(notified)" ]
}

@test "gone: the PR is asked for by its URL, not gh's default repo" {
  ps_fixture feat '.ci.verdict = "pending" | .pr.url = "https://github.com/up/stream/pull/7"'
  pass
  gone feat
  gh_pull '{"state": "closed", "merged": true}'
  pass
  [ "$(cat "$PS_FIXTURES/gh-calls")" = "api repos/up/stream/pulls/7" ]
}

@test "gone: a PR that wasn't merged drops the state quietly" {
  ps_fixture feat '.ci.verdict = "pending"'
  pass
  gone feat
  gh_pull '{"state": "closed", "merged": false}'
  pass
  [ -z "$(notified)" ]
  [ -z "$(state_files)" ]
  grep -q "feat.*branch is gone; #7 is closed, not merged" "$SHEEPDOG_STATE_DIR/last-pass"
}

@test "gone: a branch that never had a PR drops the state without asking GitHub" {
  ps_fixture feat '.pr = null'
  pass
  gone feat
  gh_pull '{"state": "closed", "merged": true}'
  pass
  [ -z "$(state_files)" ]
  [ ! -e "$PS_FIXTURES/gh-calls" ]
}

@test "gone: a branch that still exists but was unenrolled has its state dropped, quietly" {
  ps_fixture feat '.ci.verdict = "pending"'
  pass
  git config --unset branch.feat.sheepdog
  gh_pull '{"state": "closed", "merged": true}'
  pass
  [ -z "$(state_files)" ]
  [ ! -e "$PS_FIXTURES/gh-calls" ]
  gone feat
  pass
  [ -z "$(notified)" ]
}

@test "gone: a merge already notified doesn't ask GitHub or add a line when the branch goes" {
  ps_fixture feat '.pr.state = "MERGED"'
  pass
  gone feat
  gh_pull '{"state": "closed", "merged": true}'
  pass
  [ ! -e "$PS_FIXTURES/gh-calls" ]
  ! grep -q "already gone" "$SHEEPDOG_STATE_DIR/last-pass"
  [ -z "$(state_files)" ]
}

@test "gone: a merge notification that fails is tried once more, then given up on" {
  cat > "$TMP/fakebin/notify" <<'SH'
#!/usr/bin/env bash
echo "$2" >> "$PS_FIXTURES/attempts"
exit 1
SH
  ps_fixture feat '.ci.verdict = "pending"'
  pass
  gone feat
  gh_pull '{"state": "closed", "merged": true}'
  pass
  [ -n "$(state_files)" ]
  run --separate-stderr pass
  [[ "$stderr" == *"giving up on: merged (#7)"* ]] || false
  pass
  [ "$(grep -c merged "$PS_FIXTURES/attempts")" = 2 ]
  [ -z "$(state_files)" ]
}

@test "gone: a PR still open is ⚠️, not 🚫" {
  ps_fixture feat '.ci.verdict = "pending"'
  pass
  gone feat
  gh_pull '{"state": "open", "merged": false}'
  pass
  grep -q "feat.*⚠️.*branch is gone; #7 is open, not merged" "$SHEEPDOG_STATE_DIR/last-pass"
}

# ── notification policy (#52) ────────────────────────────────────────────────

# policy n|q: the step keys sd watch --help lists as notified (n) or not (q).
policy() {
  "$SD_WATCH" --help | awk -v want="$1" '
    /^Notified/ { m = "n"; next }
    /^Not notified/ { m = "q"; next }
    /^[^ ]/ { m = "" }
    m == want && /^  [a-z][a-z -]*:/ { sub(/^ +/, ""); sub(/:.*/, ""); print }' | tr ' ' '\n' | grep .
}

# one_step <key>: sd state's next is that single step, said as "say <key>".
one_step() {
  ps_fixture feat ".next = [{key: \"$1\", who: \"you\", say: \"say $1\", cmd: null}]"
}

@test "policy: every lib/next.sh step is in --help, notified or not, exactly once" {
  local steps listed
  steps=$(grep -oE '(step\(|agent\(budget, )"[a-z-]+"' "$BATS_TEST_DIRNAME/../lib/next.sh" | sed -E 's/.*"([a-z-]+)"$/\1/' | sort -u)
  listed=$( { policy n; policy q; } | sort)
  [ -n "$(policy n)" ]
  diff <(echo "$steps") <(echo "$listed")
}

@test "policy: a notified step notifies once on entering it, not on later passes" {
  local k
  for k in $(policy n); do
    rm -rf "$SHEEPDOG_STATE_DIR" "$PS_FIXTURES/notified"
    one_step "$k"
    pass
    pass
    pass
    [ "$(notified)" = "repo feat|say $k" ] || { echo "for $k: $(notified)"; return 1; }
  done
}

@test "policy: a step that isn't notified never is" {
  local k
  for k in $(policy q); do
    rm -rf "$SHEEPDOG_STATE_DIR"
    one_step "$k"
    pass
    [ -z "$(notified)" ] || { echo "for $k: $(notified)"; return 1; }
  done
}

@test "policy: a parent closed without merging is ❌ and notifies" {
  ps_fixture feat '.pr.base = "par" | .parent = {name: "par", merged: false, pr: {number: 5, state: "CLOSED"}}'
  pass
  [ "$(glyph repo)" = "❌" ]
  [[ "$(notified)" == "repo feat|par (#5) was closed without merging"* ]]
}

@test "notify: a conflict after CI red was notified still notifies, once" {
  ps_fixture feat '.ci.verdict = "red" | .ci.failing = ["lint"]'
  pass
  ps_fixture feat '.ci.verdict = "red" | .ci.failing = ["lint"] | .sync.up_to_date = false | .sync.conflict = true'
  pass
  pass
  run notified
  [ "${#lines[@]}" -eq 2 ]
  [ "${lines[0]}" = "repo feat|fix CI: lint" ]
  [ "${lines[1]}" = "repo feat|restack onto main conflicts: resolve it" ]
}

@test "notify: a conflict after ready to merge was notified still notifies, once" {
  ps_fixture feat
  pass
  ps_fixture feat '.sync.up_to_date = false | .sync.conflict = true'
  pass
  pass
  run notified
  [ "${#lines[@]}" -eq 2 ]
  [ "${lines[1]}" = "repo feat|restack onto main conflicts: resolve it" ]
}

@test "notify: an unknown pass (sd state errors) between two doesn't re-notify what's still true" {
  ps_fixture feat '.sync.up_to_date = false | .sync.conflict = true'
  pass
  ps_fixture feat '.errors = ["gh: down"]'
  pass
  [ "$(glyph repo)" = "⚠️" ]
  ps_fixture feat '.sync.up_to_date = false | .sync.conflict = true'
  pass
  [ "$(notified)" = "repo feat|restack onto main conflicts: resolve it" ]
}

@test "notify: errors alongside a ❌ step still keep what was notified" {
  ps_fixture feat '.ci.verdict = "red" | .ci.failing = ["lint"] | .threads.owed_human = 1'
  pass
  ps_fixture feat '.ci.verdict = "red" | .ci.failing = ["lint"] | .errors = ["gh: down"]'
  pass
  [ "$(glyph repo)" = "❌" ]
  ps_fixture feat '.ci.verdict = "red" | .ci.failing = ["lint"] | .threads.owed_human = 1'
  pass
  [ "$(notified | grep -c 'human item')" = 1 ]
}

@test "notify: a state file from before notified was recorded doesn't re-notify" {
  ps_fixture feat
  pass
  local f; f=$(find "$SHEEPDOG_STATE_DIR/branches" -name feat.json)
  jq -c 'del(.notified)' "$f" > "$f.old" && mv "$f.old" "$f"
  pass
  [ "$(notified)" = "repo feat|ready to merge" ]
}

@test "notify: sd state failing between two passes doesn't re-notify what's still true" {
  ps_fixture feat '.sync.up_to_date = false | .sync.conflict = true'
  pass
  mv "$PS_FIXTURES/feat.json" "$TMP/feat.json"
  pass
  mv "$TMP/feat.json" "$PS_FIXTURES/feat.json"
  pass
  [ "$(notified)" = "repo feat|restack onto main conflicts: resolve it" ]
}

@test "notify: a pass with no next steps between two doesn't re-notify what's still true" {
  ps_fixture feat '.sync.up_to_date = false | .sync.conflict = true'
  pass
  ps_fixture feat '.next = []'
  pass
  [ "$(glyph repo)" = "⚠️" ]
  ps_fixture feat '.sync.up_to_date = false | .sync.conflict = true'
  pass
  [ "$(notified)" = "repo feat|restack onto main conflicts: resolve it" ]
}

@test "notify: a reason the pass couldn't send because super failed is sent next pass" {
  local real
  real=$(bash5 -c 'source "$1/../lib/config.sh"; command -v "$(prc_superdb_binary)"' _ "$BATS_TEST_DIRNAME")
  [[ $real != */shims/* ]] || skip "super is an asdf shim, which the fake's ASDF_DATA_DIR would break"
  ps_fixture feat '.sync.up_to_date = false | .sync.conflict = true'
  cat > "$TMP/fakebin/super" <<EOF
#!/usr/bin/env bash
if [[ ! -e $PS_FIXTURES/super-works ]]; then
  for a; do [[ \$a == *"r.msg}"* ]] && { echo panic >&2; exit 2; }; done
fi
exec "$real" "\$@"
EOF
  chmod +x "$TMP/fakebin/super"
  export ASDF_DATA_DIR="$TMP/no-asdf"
  pass
  [ -z "$(notified)" ]
  touch "$PS_FIXTURES/super-works"
  pass
  pass
  [ "$(notified)" = "repo feat|restack onto main conflicts: resolve it" ]
}

@test "notify: a notification that fails is tried again next pass, then not again" {
  cat > "$TMP/fakebin/notify" <<'SH'
#!/usr/bin/env bash
[[ -e $PS_FIXTURES/notify-works ]] || exit 1
printf '%s|%s\n' "$1" "$2" >> "$PS_FIXTURES/notified"
SH
  ps_fixture feat '.sync.up_to_date = false | .sync.conflict = true'
  run --separate-stderr pass
  [[ "$stderr" == *"notify failed"* ]] || false
  [ -z "$(notified)" ]
  touch "$PS_FIXTURES/notify-works"
  pass
  pass
  [ "$(notified)" = "repo feat|restack onto main conflicts: resolve it" ]
}

@test "notify: a notifier that keeps failing is tried twice, then given up on" {
  cat > "$TMP/fakebin/notify" <<'SH'
#!/usr/bin/env bash
echo "$2" >> "$PS_FIXTURES/attempts"
exit 1
SH
  ps_fixture feat '.sync.up_to_date = false | .sync.conflict = true'
  pass
  run --separate-stderr pass
  [[ "$stderr" == *"giving up on: restack onto main conflicts"* ]] || false
  pass
  pass
  [ "$(wc -l < "$PS_FIXTURES/attempts" | tr -d ' ')" = 2 ]
}

@test "notify: GitHub recomputing mergeability doesn't make ready re-notify" {
  ps_fixture feat
  pass
  one_step merge-wait
  pass
  ps_fixture feat
  pass
  [ "$(notified)" = "repo feat|ready to merge" ]
}

@test "notify: an attached session hiding ready doesn't make ready re-notify" {
  ps_fixture feat
  pass
  one_step attached
  pass
  ps_fixture feat
  pass
  [ "$(notified)" = "repo feat|ready to merge" ]
}

@test "notify: merge-wait doesn't reset a failed ready's one retry" {
  cat > "$TMP/fakebin/notify" <<'SH'
#!/usr/bin/env bash
echo "$2" >> "$PS_FIXTURES/attempts"
exit 1
SH
  local i
  for i in 1 2 3; do
    ps_fixture feat
    pass
    one_step merge-wait
    pass
  done
  [ "$(wc -l < "$PS_FIXTURES/attempts" | tr -d ' ')" = 2 ]
}

@test "notify: merge-wait carries only ready: a conflict that comes back after it notifies" {
  ps_fixture feat '.sync.up_to_date = false | .sync.conflict = true'
  pass
  one_step merge-wait
  pass
  ps_fixture feat '.sync.up_to_date = false | .sync.conflict = true'
  pass
  [ "$(notified | grep -c conflicts)" = 2 ]
}

@test "notify: an unsure pass doesn't reset a failed notification's one retry" {
  cat > "$TMP/fakebin/notify" <<'SH'
#!/usr/bin/env bash
echo "$2" >> "$PS_FIXTURES/attempts"
exit 1
SH
  ps_fixture feat '.sync.up_to_date = false | .sync.conflict = true'
  pass
  ps_fixture feat '.errors = ["gh: down"]'
  pass
  ps_fixture feat '.sync.up_to_date = false | .sync.conflict = true'
  pass
  pass
  [ "$(wc -l < "$PS_FIXTURES/attempts" | tr -d ' ')" = 2 ]
}

@test "notify: a notifier that reads stdin doesn't swallow later reasons" {
  cat > "$TMP/fakebin/notify" <<'SH'
#!/usr/bin/env bash
cat >/dev/null
printf '%s|%s\n' "$1" "$2" >> "$PS_FIXTURES/notified"
SH
  ps_fixture feat '.ci.verdict = "red" | .ci.failing = ["lint"] | .threads.owed_human = 1'
  pass
  [ "$(notified | wc -l | tr -d ' ')" = 2 ]
}

@test "notify: notifier output stays out of the pass record" {
  cat > "$TMP/fakebin/notify" <<'SH'
#!/usr/bin/env bash
echo "NOISE"
SH
  ps_fixture feat
  pass
  ! grep -q NOISE "$SHEEPDOG_STATE_DIR/last-pass"
}

@test "notify: a configured command with a space in its path works" {
  mkdir -p "$TMP/my tools"
  cp "$TMP/fakebin/notify" "$TMP/my tools/notify"
  "$SD_CONFIG" set --global settings.notify "'$TMP/my tools/notify'"
  ps_fixture feat
  pass
  [ "$(notified)" = "repo feat|ready to merge" ]
}

# ── stale enrollments (#51) ──────────────────────────────────────────────────

# aged <branch> <seconds>: the branch's stale clock started that long ago.
aged() {
  local f; f=$(find "$SHEEPDOG_STATE_DIR/branches" -path "*/$1.json")
  jq -c --argjson s "$2" '.stale.since -= $s' "$f" > "$f.new" && mv "$f.new" "$f"
}
enrolled() { [ "$(git config "branch.$1.sheepdog")" = true ]; }

@test "stale: a branch checked out nowhere is unenrolled after vacuumAfter, and only unenrolled" {
  git update-ref refs/sheepdog/feat/base HEAD
  git switch -q main
  ps_fixture feat '.ci.verdict = "pending"'
  pass
  enrolled feat
  aged feat 604801
  pass
  ! enrolled feat || false
  grep -q "feat.*unenrolled: checked out nowhere for 7d" "$SHEEPDOG_STATE_DIR/last-pass"
  [ "$(notified)" = "repo feat|unenrolled: checked out nowhere for 7d (sd act enroll feat to watch it again)" ]
  git rev-parse -q --verify refs/heads/feat
  [ -n "$(git for-each-ref refs/sheepdog/feat/)" ]
}

@test "stale: re-enrolling after an unenroll starts a fresh clock" {
  ps_fixture feat '.pr.state = "MERGED"'
  pass
  aged feat 604801
  pass
  ! enrolled feat || false
  enroll feat
  pass
  enrolled feat
}

@test "vacuum --dry-run: one past its threshold says the next pass takes it too" {
  git switch -q main
  ps_fixture feat '.ci.verdict = "pending"'
  pass
  aged feat 700000
  run "$SD_WATCH" vacuum --dry-run
  [[ "$output" == *"checked out nowhere for 8d: vacuum unenrolls it, as does the next pass"* ]] || false
}

@test "stale: a worktree whose directory was deleted doesn't count" {
  git switch -q main
  git worktree add -q "$TMP/feat-wt" feat
  rm -rf "$TMP/feat-wt"
  ps_fixture feat '.ci.verdict = "pending"'
  pass
  aged feat 604801
  pass
  ! enrolled feat
}

@test "stale: not before vacuumAfter" {
  git switch -q main
  ps_fixture feat '.ci.verdict = "pending"'
  pass
  aged feat 604000
  pass
  enrolled feat
}

@test "stale: checking the branch out again stops the clock" {
  git switch -q main
  ps_fixture feat '.ci.verdict = "pending"'
  pass
  aged feat 604000
  git switch -q feat
  pass
  git switch -q main
  pass
  aged feat 1000
  pass
  enrolled feat
}

@test "stale: merged or closed is unenrolled after vacuumAfter, even while checked out" {
  local st
  for st in MERGED CLOSED; do
    enroll feat
    rm -rf "$SHEEPDOG_STATE_DIR"
    ps_fixture feat ".pr.state = \"$st\""
    pass
    aged feat 604801
    pass
    ! enrolled feat || { echo "for $st"; return 1; }
    grep -q "feat.*unenrolled: $(tr A-Z a-z <<<"$st") for 7d" "$SHEEPDOG_STATE_DIR/last-pass"
  done
}

@test "stale: an unsure pass neither starts, resets nor acts on the clock" {
  git switch -q main
  ps_fixture feat '.errors = ["gh: down"]'
  pass
  [ "$(jq -c .stale "$(find "$SHEEPDOG_STATE_DIR/branches" -name feat.json)")" = null ]
  ps_fixture feat '.ci.verdict = "pending"'
  pass
  aged feat 604801
  ps_fixture feat '.errors = ["gh: down"]'
  pass
  enrolled feat
  ps_fixture feat '.ci.verdict = "pending"'
  pass
  ! enrolled feat
}

@test "stale: an enrolled branch with no local branch is unenrolled at once" {
  git switch -q main
  git update-ref -d refs/heads/feat
  enrolled feat
  pass
  ! enrolled feat || false
  grep -q "feat.*unenrolled: no local branch" "$SHEEPDOG_STATE_DIR/last-pass"
  [ ! -e "$PS_FIXTURES/calls" ]
}

@test "stale: a new reason restarts the clock" {
  git switch -q main
  ps_fixture feat '.ci.verdict = "pending"'
  pass
  aged feat 604000
  ps_fixture feat '.pr.state = "MERGED"'
  pass
  aged feat 1000
  pass
  enrolled feat
  [ "$(jq -r .stale.reason "$(find "$SHEEPDOG_STATE_DIR/branches" -name feat.json)")" = merged ]
}

@test "stale: a refresh can unenroll too" {
  ps_fixture feat '.pr.state = "MERGED"'
  pass
  aged feat 604801
  run "$SD_WATCH" refresh
  [ "$status" -eq 0 ]
  [[ "$output" == *"feat  -  unenrolled: merged for 7d"* ]] || false
  ! enrolled feat
}

@test "vacuum --dry-run: thresholds under a day read in hours or minutes, and a config error is said" {
  write_cfg '{"settings":{"vacuumAfter":7200}}'
  git switch -q main
  ps_fixture feat '.ci.verdict = "pending"'
  pass
  aged feat 90
  run "$SD_WATCH" vacuum --dry-run
  [[ "$output" == *"checked out nowhere for 1m: vacuum unenrolls it; a pass, after 2h"* ]] || false
  printf '{not json\n' > "$SHEEPDOG_CONFIG"
  run "$SD_WATCH" vacuum --dry-run
  [ "$status" -ne 0 ]
  [[ "$output" == *"sd config failed"* ]] || false
}

@test "vacuum --dry-run: durations under a minute read in seconds, and a clock running behind reads 0s" {
  write_cfg '{"settings":{"vacuumAfter":30}}'
  git switch -q main
  ps_fixture feat '.ci.verdict = "pending"'
  pass
  aged feat 5
  run "$SD_WATCH" vacuum --dry-run
  [[ "$output" == *"checked out nowhere for "[5-9]"s: vacuum unenrolls it; a pass, after 30s"* ]] || false
  aged feat -3600
  run "$SD_WATCH" vacuum --dry-run
  [[ "$output" == *"checked out nowhere for 0s: vacuum unenrolls it; a pass, after 30s"* ]] || false
}

@test "stale: vacuumAfter 0 never unenrolls" {
  write_cfg '{"settings":{"vacuumAfter":0}}'
  git switch -q main
  ps_fixture feat '.ci.verdict = "pending"'
  pass
  aged feat 99999999
  pass
  enrolled feat
}

@test "stale: vacuumAfter from the config file" {
  write_cfg '{"settings":{"vacuumAfter":60}}'
  git switch -q main
  ps_fixture feat '.ci.verdict = "pending"'
  pass
  aged feat 61
  pass
  ! enrolled feat
}

@test "vacuum --dry-run: says what vacuum would unenroll, and when a pass would, and changes nothing" {
  git branch other && enroll other
  git switch -q main
  ps_fixture feat '.ci.verdict = "pending"'
  ps_fixture other '.pr.state = "MERGED"'
  git worktree add -q "$TMP/other-wt" other
  pass
  aged feat 172800
  aged other 120
  local before; before=$(cat "$SHEEPDOG_STATE_DIR/last-pass")
  run "$SD_WATCH" vacuum --dry-run
  [ "$status" -eq 0 ]
  [[ "$output" == *"repo  feat  checked out nowhere for 2d: vacuum unenrolls it; a pass, after 7d"* ]] || false
  [[ "$output" == *"repo  other  merged for 2m: vacuum unenrolls it; a pass, after 7d"* ]] || false
  enrolled feat && enrolled other
  [ "$(cat "$SHEEPDOG_STATE_DIR/last-pass")" = "$before" ]
}

@test "vacuum --dry-run: a branch enrolled since the last pass is said to be unchecked, not an error" {
  ps_fixture feat
  pass
  git branch other && enroll other
  run "$SD_WATCH" vacuum --dry-run
  [ "$status" -eq 0 ]
  [ "$output" = "repo  other  not checked yet: vacuum checks it first" ]
}

@test "vacuum --dry-run: vacuumAfter 0, an unsure last check, and a gone branch" {
  write_cfg '{"settings":{"vacuumAfter":0}}'
  git branch other && enroll other
  git branch gone && enroll gone
  git switch -q main
  ps_fixture feat '.pr.state = "MERGED"'
  ps_fixture other '.errors = ["gh: down"]'
  ps_fixture gone
  pass
  git update-ref -d refs/heads/gone
  aged feat 120
  run "$SD_WATCH" vacuum --dry-run
  [ "$status" -eq 0 ]
  [[ "$output" == *"repo  feat  merged for 2m: vacuum unenrolls it; a pass doesn't (vacuumAfter 0)"* ]] || false
  [[ "$output" == *"repo  other  unsure at the last saved check: vacuum skips it if sd state still is"* ]] || false
  [[ "$output" == *"repo  gone  no local branch: vacuum unenrolls it, as does the next pass"* ]] || false
  enrolled feat && enrolled other && enrolled gone
}

@test "vacuum: -h prints the help; arguments are refused" {
  run "$SD_WATCH" vacuum -h
  [ "$status" -eq 0 ]
  [[ "$output" == *"Usage: sd watch"* ]] || false
  run "$SD_WATCH" vacuum feat
  [ "$status" -ne 0 ]
  [[ "$output" == *"usage: sd watch vacuum [--dry-run]"* ]] || false
  run "$SD_WATCH" vacuum --dry-run feat
  [ "$status" -ne 0 ]
  [[ "$output" == *"usage: sd watch vacuum [--dry-run]"* ]] || false
  ps_fixture feat '.pr.state = "MERGED"'
  run "$SD_WATCH" vacuum ""
  [ "$status" -ne 0 ]
  [[ "$output" == *"usage: sd watch vacuum [--dry-run]"* ]] || false
  enrolled feat
}

@test "stale: a worktree paused mid-rebase still has the branch checked out" {
  echo a > f && git add f && git commit -qm a
  git switch -q main
  echo b > f && git add f && git commit -qm b
  git switch -q feat
  ! git rebase -q main 2>/dev/null
  ps_fixture feat '.ci.verdict = "pending"'
  pass
  git rebase --abort
  [ "$(jq -c .stale "$(find "$SHEEPDOG_STATE_DIR/branches" -name feat.json)")" = null ]
  [ "$(glyph repo)" = "⏳" ]
}

@test "vacuum --dry-run: nothing stale says so" {
  ps_fixture feat
  pass
  run "$SD_WATCH" vacuum --dry-run
  [ "$status" -eq 0 ]
  [ "$output" = "nothing stale" ]
}

@test "vacuum: unenrolls a merged branch inside the clock, and only unenrolls it" {
  git update-ref refs/sheepdog/feat/base HEAD
  ps_fixture feat '.pr.state = "MERGED"'
  pass
  aged feat 120
  run "$SD_WATCH" vacuum
  [ "$status" -eq 0 ]
  [ "$output" = "repo  feat  unenrolled: merged for 2m (sd act enroll feat to watch it again)" ]
  ! enrolled feat || false
  git rev-parse -q --verify refs/heads/feat
  [ -n "$(git for-each-ref refs/sheepdog/feat/)" ]
  [[ "$(notified)" != *unenrolled* ]] || false
  grep -q "feat.*unenrolled: merged for 2m" "$SHEEPDOG_STATE_DIR/last-pass"
}

@test "vacuum: observes before acting, so a branch no pass has seen is judged too" {
  git branch other && enroll other
  ps_fixture feat
  ps_fixture other '.pr.state = "CLOSED"'
  run "$SD_WATCH" vacuum
  [ "$status" -eq 0 ]
  [ "$output" = "repo  other  unenrolled: closed for 0s (sd act enroll other to watch it again)" ]
  enrolled feat
  ! enrolled other || false
}

@test "vacuum: acts with vacuumAfter 0 too, and on a branch that's gone" {
  write_cfg '{"settings":{"vacuumAfter":0}}'
  git branch other && enroll other
  git update-ref -d refs/heads/other
  git switch -q main
  ps_fixture feat '.ci.verdict = "pending"'
  run "$SD_WATCH" vacuum
  [ "$status" -eq 0 ]
  [[ "$output" == *"repo  other  unenrolled: no local branch"* ]] || false
  [[ "$output" == *"repo  feat  unenrolled: checked out nowhere for 0s (sd act enroll feat to watch it again)"* ]] || false
  ! enrolled feat && ! enrolled other
}

@test "vacuum: VACUUM_LOG from the environment doesn't make a pass a vacuum" {
  git switch -q main
  ps_fixture feat '.ci.verdict = "pending"'
  echo keep > "$TMP/keep"
  VACUUM_LOG="$TMP/keep" pass
  enrolled feat
  [ "$(cat "$TMP/keep")" = keep ]
}

@test "vacuum: an active branch is kept" {
  ps_fixture feat
  run "$SD_WATCH" vacuum
  [ "$status" -eq 0 ]
  [ "$output" = "nothing stale" ]
  enrolled feat
}

@test "vacuum: an unsure branch is skipped, and says why" {
  git branch other && enroll other
  git switch -q main
  ps_fixture feat '.ci.verdict = "pending"'
  pass
  aged feat 120
  ps_fixture feat '.errors = ["gh: down"]'
  run "$SD_WATCH" vacuum
  [ "$status" -eq 0 ]
  [[ "$output" == *"repo  feat  skipped, sd state is unsure: gh: down"* ]] || false
  [[ "$output" == *"repo  other  skipped, sd state is unsure: sd state failed: sd state: no fixture for other"* ]] || false
  enrolled feat && enrolled other
}

@test "vacuum: a repo whose config can't be read is skipped" {
  ps_fixture feat '.pr.state = "MERGED"'
  printf '{not json\n' > "$SHEEPDOG_CONFIG"
  run "$SD_WATCH" vacuum
  [ "$status" -eq 0 ]
  [[ "$output" == *"repo  feat  skipped: sd config failed"* ]] || false
  enrolled feat
}

@test "vacuum: no next steps or unparseable sd state output is unsure too" {
  git branch other && enroll other
  git switch -q main
  ps_fixture feat '.next = []'
  echo 'not json {' > "$PS_FIXTURES/other.json"
  run "$SD_WATCH" vacuum
  [ "$status" -eq 0 ]
  [[ "$output" == *"repo  feat  skipped, sd state is unsure: sd state gave no next steps"* ]] || false
  [[ "$output" == *"repo  other  skipped, sd state is unsure: unparseable sd state output"* ]] || false
  enrolled feat && enrolled other
}

@test "vacuum: unsure reasons that can't be read aren't called no next steps" {
  git switch -q main
  ps_fixture feat '.errors = ["gh: down"]'
  jq -c '.errors = [{"odd": 1}]' "$PS_FIXTURES/feat.json" > "$TMP/f" && mv "$TMP/f" "$PS_FIXTURES/feat.json"
  run "$SD_WATCH" vacuum
  [ "$status" -eq 0 ]
  [ "$output" = "repo  feat  skipped, sd state is unsure: its reasons couldn't be read" ]
  enrolled feat
}

@test "vacuum: an unenroll that fails is said" {
  [ "$(id -u)" -ne 0 ] || skip "root writes through a read-only .git"
  git switch -q main
  ps_fixture feat '.pr.state = "MERGED"'
  chmod a-w .git
  run "$SD_WATCH" vacuum
  chmod u+w .git
  [ "$status" -eq 0 ]
  [[ "$output" == *"repo  feat  couldn't unenroll: merged for 0s"* ]] || false
  enrolled feat
}

@test "vacuum: stale remote refs are unsure too" {
  git switch -q main
  ps_fixture feat '.fresh = false | .stale = ["origin/feat is not at GitHub'"'"'s abc1234"]'
  run "$SD_WATCH" vacuum
  [ "$status" -eq 0 ]
  [ "$output" = "repo  feat  skipped, sd state is unsure: origin/feat is not at GitHub's abc1234" ]
  enrolled feat
}

# ── glyph lookup (#3) ────────────────────────────────────────────────────────

@test "glyph: prints the current worktree's glyph, from a subdirectory too" {
  ps_fixture feat
  pass
  run "$SD_WATCH" glyph
  [ "$status" -eq 0 ]
  [ "$output" = "✅" ]
  mkdir -p sub
  [ "$(cd sub && "$SD_WATCH" glyph)" = "✅" ]
}

@test "glyph: takes a path" {
  git switch -q main
  git worktree add -q "$TMP/feat-wt" feat
  ps_fixture feat '.ci.verdict = "red" | .ci.failing = ["lint"]'
  pass
  [ "$(cd "$TMP" && "$SD_WATCH" glyph "$TMP/feat-wt")" = "❌" ]
  [ -z "$("$SD_WATCH" glyph)" ]
}

@test "glyph: nothing when the worktree has moved to another branch, or no pass saw it" {
  ps_fixture feat
  pass
  git switch -q main
  run "$SD_WATCH" glyph
  [ "$status" -eq 0 ]
  [ -z "$output" ]
  run "$SD_WATCH" glyph "$TMP"
  [ "$status" -eq 0 ]
  [ -z "$output" ]
}

@test "glyph: a file's path means its worktree; -h prints the help" {
  ps_fixture feat
  pass
  touch notes.txt
  [ "$(cd "$TMP" && "$SD_WATCH" glyph "$TMP/repo/notes.txt")" = "✅" ]
  run "$SD_WATCH" glyph -h
  [[ "$output" == *"Usage: sd watch"* ]] || false
}

@test "glyph: a worktree paused mid-rebase still shows its branch's glyph" {
  echo a > f && git add f && git commit -qm a
  git switch -q main
  echo b > f && git add f && git commit -qm b
  git switch -q feat
  ! git rebase -q main 2>/dev/null
  [ -d "$(git rev-parse --git-path rebase-merge)" ]
  ps_fixture feat '.sync.up_to_date = false | .sync.conflict = true'
  pass
  [ "$("$SD_WATCH" glyph)" = "❌" ]
  git rebase --abort
}

@test "glyph: a tag with the branch's name doesn't hide the glyph" {
  ps_fixture feat
  pass
  git tag feat
  [ "$("$SD_WATCH" glyph)" = "✅" ]
}

@test "glyph: an unknown option or a second path is refused" {
  run "$SD_WATCH" glyph --bogus
  [ "$status" -ne 0 ]
  [[ "$output" == *"unknown option: --bogus"* ]] || false
  run "$SD_WATCH" glyph a b
  [ "$status" -ne 0 ]
  [[ "$output" == *"usage: sd watch glyph [<path>]"* ]] || false
}

@test "glyph: before any pass, nothing" {
  run "$SD_WATCH" glyph
  [ "$status" -eq 0 ]
  [ -z "$output" ]
}

@test "glyph: a refresh updates it, keeping other worktrees'" {
  git branch other && enroll other
  git worktree add -q "$TMP/other-wt" other
  ps_fixture feat
  ps_fixture other
  pass
  ps_fixture feat '.ci.verdict = "pending"'
  "$SD_WATCH" refresh
  [ "$("$SD_WATCH" glyph)" = "⏳" ]
  [ "$("$SD_WATCH" glyph "$TMP/other-wt")" = "✅" ]
}

@test "glyph: gone after a pass once the branch isn't enrolled" {
  ps_fixture feat
  pass
  git config --unset branch.feat.sheepdog
  pass
  [ -z "$("$SD_WATCH" glyph)" ]
}

# ── same-named worktrees (#4) ───────────────────────────────────────────────

# twin <branch-in-repo2>: a second registered repo whose <branch> is checked
# out in $TMP/y/wt, while this repo's feat is in $TMP/x/wt. sd state for
# repo2 serves <branch>'s fixture made CI red.
twin() {
  git clone -q "$TMP/origin.git" "$TMP/repo2" 2>/dev/null
  git -C "$TMP/repo2" branch -q "$1"
  git -C "$TMP/repo2" config "branch.$1.sheepdog" true
  register "$TMP/repo2"
  git switch -q main
  mkdir -p "$TMP/x" "$TMP/y"
  git worktree add -q "$TMP/x/wt" feat
  git -C "$TMP/repo2" worktree add -q "$TMP/y/wt" "$1"
  ps_fixture feat
  ps_fixture "$1"
  jq '.ci.verdict = "red" | .ci.failing = ["lint"] | .next = [{key: "ci-red", who: "agent", say: "fix CI: lint", cmd: null}]' \
    "$PS_FIXTURES/$1.json" > "$PS_FIXTURES/repo2-$1.json"
  cat > "$TMP/fakebin/sd-state" <<'SH'
#!/usr/bin/env bash
b=${@: -1}
echo "$PWD $b" >> "$PS_FIXTURES/calls"
f="$PS_FIXTURES/$(basename "$PWD")-${b//\//__}.json"
[[ -f $f ]] || f="$PS_FIXTURES/${b//\//__}.json"
cat "$f"
SH
}

@test "same name: two worktrees on the same branch get no shared .pr file, and glyph tells them apart" {
  twin feat
  pass
  [ ! -e "$STATUS_DIR/wt.pr" ]
  [ "$("$SD_WATCH" glyph "$TMP/x/wt")" = "✅" ]
  [ "$("$SD_WATCH" glyph "$TMP/y/wt")" = "❌" ]
}

@test "same name: on different branches the .pr file stays, its branch telling whose it is" {
  twin other
  pass
  [ "$(cat "$STATUS_DIR/wt.pr")" = $'feat\t✅' ] || [ "$(cat "$STATUS_DIR/wt.pr")" = $'other\t❌' ]
  [ "$("$SD_WATCH" glyph "$TMP/x/wt")" = "✅" ]
  [ "$("$SD_WATCH" glyph "$TMP/y/wt")" = "❌" ]
}

@test "same name: a refresh doesn't put the shared .pr file back" {
  twin feat
  pass
  (cd "$TMP/x/wt" && "$SD_WATCH" refresh) >/dev/null
  [ ! -e "$STATUS_DIR/wt.pr" ]
  [ "$("$SD_WATCH" glyph "$TMP/x/wt")" = "✅" ]
  [ "$("$SD_WATCH" glyph "$TMP/y/wt")" = "❌" ]
}

@test "same name: a third one on another branch keeps the .pr file it wrote last" {
  twin feat
  git clone -q "$TMP/origin.git" "$TMP/repo3" 2>/dev/null
  git -C "$TMP/repo3" branch -q other
  git -C "$TMP/repo3" config branch.other.sheepdog true
  register "$TMP/repo3"
  mkdir -p "$TMP/z"
  git -C "$TMP/repo3" worktree add -q "$TMP/z/wt" other
  ps_fixture other
  pass
  [ "$(cat "$STATUS_DIR/wt.pr")" = $'other\t✅' ]
}

@test "same name: a repo registered twice isn't its own twin" {
  ps_fixture feat
  git config --global --add sheepdog.repo "$TMP/repo"
  pass
  [ "$(glyph repo)" = "✅" ]
}

# ── GitHub's rate limit (#122) ───────────────────────────────────────────────

# low <branch> [remaining]: sd state for <branch> leaves that much of the quota.
low() { ps_fixture "$1" ".rate_limit = {cost: 2, remaining: ${2:-100}, reset_at: \"2026-10-16T13:00:00Z\", limited: false}"; }
# rate_gh: a fake gh whose REST rate_limit says GraphQL resets at 14:00.
rate_gh() {
  cat > "$TMP/fakebin/gh" <<'SH'
#!/usr/bin/env bash
echo "$*" >> "$PS_FIXTURES/gh-calls"
[[ $1 == api && $2 == rate_limit ]] || exit 1
echo 2026-10-16T14:00:00Z
SH
  chmod +x "$TMP/fakebin/gh"
}

@test "rate limit: remaining under settings.rateLimitFloor stops the pass after that branch, and status says until when" {
  git branch other && enroll other
  low feat
  ps_fixture other
  pass
  [ "$(cat "$PS_FIXTURES/calls")" = "$TMP/repo feat" ]
  run "$SD_WATCH" status
  [[ "$output" == *"repo  feat  ✅"* ]]
  [[ "$output" == *"GitHub rate limit: 100 points left, under settings.rateLimitFloor (250): the branches below weren't checked (the quota resets at 2026-10-16T13:00:00Z)"* ]]
  [[ "$output" == *"repo  other  -  not checked: rate limit"* ]]
}

@test "rate limit: low after the pass's last branch stops nothing" {
  git branch other && enroll other
  ps_fixture feat
  low other
  pass
  [ "$(wc -l < "$PS_FIXTURES/calls" | tr -d ' ')" = 2 ]
  ! grep -q "rate limit" "$SHEEPDOG_STATE_DIR/last-pass"
}

@test "rate limit: a stopped pass still does the later repos' local housekeeping" {
  low feat
  git clone -q "$TMP/origin.git" "$TMP/repo2" 2>/dev/null
  git -C "$TMP/repo2" config branch.gone.sheepdog true
  git -C "$TMP/repo2" branch -q z && git -C "$TMP/repo2" config branch.z.sheepdog true
  git -C "$TMP/repo2" update-ref refs/sheepdog/old/base HEAD
  register "$TMP/repo2"
  ps_fixture z
  pass
  [ "$(cat "$PS_FIXTURES/calls")" = "$TMP/repo feat" ]
  [ -z "$(git -C "$TMP/repo2" config branch.gone.sheepdog || true)" ]
  [ -z "$(git -C "$TMP/repo2" for-each-ref refs/sheepdog/)" ]
  grep -q "repo2	z	-	not checked: rate limit" "$SHEEPDOG_STATE_DIR/last-pass"
}

@test "rate limit: plenty left, or none reported, checks every branch" {
  git branch other && enroll other
  git branch third && enroll third
  ps_fixture feat
  low other 4000
  ps_fixture third
  pass
  [ "$(cat "$PS_FIXTURES/calls")" = "$TMP/repo feat
$TMP/repo other
$TMP/repo third" ]
  ! grep -q "rate limit" "$SHEEPDOG_STATE_DIR/last-pass"
}

@test "rate limit: settings.rateLimitFloor sets the floor" {
  git branch other && enroll other
  "$SD_CONFIG" set --global settings.rateLimitFloor 50
  low feat
  ps_fixture other
  pass
  [ "$(wc -l < "$PS_FIXTURES/calls" | tr -d ' ')" = 2 ]
}

@test "rate limit: a call refused for the rate limit stops the pass, other repos too, with GitHub's reset time" {
  rate_gh
  ps_fixture feat '.rate_limit = {cost: null, remaining: null, reset_at: null, limited: true}
                   | .errors = ["gh (feat): API rate limit exceeded for user ID 1."] | .fresh = null'
  git clone -q "$TMP/origin.git" "$TMP/repo2" 2>/dev/null
  git -C "$TMP/repo2" branch -q z && git -C "$TMP/repo2" config branch.z.sheepdog true
  register "$TMP/repo2"
  ps_fixture z
  pass
  [ "$(cat "$PS_FIXTURES/calls")" = "$TMP/repo feat" ]
  grep -q "GitHub refused a call for the rate limit: the branches below weren't checked (the quota resets at 2026-10-16T14:00:00Z)" "$SHEEPDOG_STATE_DIR/last-pass"
  grep -q "repo2	z	-	not checked: rate limit" "$SHEEPDOG_STATE_DIR/last-pass"
}

@test "rate limit: a stopped pass leaves the unchecked branches' glyphs as they were" {
  git branch other && enroll other
  git worktree add -q "$TMP/other-wt" other
  ps_fixture feat
  ps_fixture other
  pass
  [ "$(glyph other-wt)" = ✅ ]
  low feat
  ps_fixture other '.ci.verdict = "red"'
  pass
  [ "$(glyph other-wt)" = ✅ ]
  [ "$("$SD_WATCH" glyph "$TMP/other-wt")" = ✅ ]
  [ "$(glyph repo)" = ✅ ]
}

@test "rate limit: refresh checks its branch and notes nothing to stop" {
  low feat
  run "$SD_WATCH" refresh feat
  [ "$status" -eq 0 ]
  [[ "$output" == *"feat  ✅"* ]]
  [[ "$output" != *"rate limit"* ]]
}

@test "rate limit: a stopped pass keeps only the skipped branches' glyphs" {
  git branch other && enroll other
  git worktree add -q "$TMP/other-wt" other
  git clone -q "$TMP/origin.git" "$TMP/repo2" 2>/dev/null
  git -C "$TMP/repo2" branch -q z && git -C "$TMP/repo2" config branch.z.sheepdog true
  git -C "$TMP/repo2" worktree add -q "$TMP/z-wt" z
  register "$TMP/repo2"
  ps_fixture feat && ps_fixture other && ps_fixture z
  pass
  [ "$(glyph z-wt)" = ✅ ]
  git config --global --unset sheepdog.repo "$TMP/repo2"
  low feat
  pass
  [ "$(glyph other-wt)" = ✅ ]
  [ ! -e "$STATUS_DIR/z-wt.pr" ]
  [ -z "$("$SD_WATCH" glyph "$TMP/z-wt")" ]
}

# ── status ───────────────────────────────────────────────────────────────────

@test "status: prints the last pass" {
  ps_fixture feat '.ci.verdict = "red" | .ci.failing = ["lint"]'
  pass
  run "$SD_WATCH" status
  [ "$status" -eq 0 ]
  [[ "$output" == *"repo"*"feat"*"❌"*"fix CI: lint"* ]] || false
  [[ "$output" != *$'\t'* ]]
}

@test "status: before any pass" {
  run "$SD_WATCH" status
  [ "$status" -eq 0 ]
  [[ "$output" == *"no pass yet"* ]]
}

# ── refresh ──────────────────────────────────────────────────────────────────

refresh() { "$SD_WATCH" refresh "$@"; }

@test "refresh: checks only the current branch, from the main worktree" {
  git branch other && enroll other
  ps_fixture feat
  ps_fixture other
  run refresh
  [ "$status" -eq 0 ]
  [ "$(cat "$PS_FIXTURES/calls")" = "$TMP/repo feat" ]
  [[ "$output" == *"repo  feat  ✅  ready to merge"* ]]
}

@test "refresh <branch>: checks that branch" {
  git branch other && enroll other
  ps_fixture other '.ci.verdict = "red" | .ci.failing = ["lint"]'
  refresh other
  [ "$(cat "$PS_FIXTURES/calls")" = "$TMP/repo other" ]
  [ "$(notified)" = "repo other|fix CI: lint" ]
}

@test "refresh: writes the glyph and notifies, and a later pass doesn't notify again" {
  ps_fixture feat '.ci.verdict = "red" | .ci.failing = ["lint"]'
  refresh
  [ "$(glyph repo)" = "❌" ]
  pass
  refresh
  [ "$(notified)" = "repo feat|fix CI: lint" ]
}

@test "refresh: after a pass, doesn't notify what the pass did" {
  ps_fixture feat
  pass
  refresh
  [ "$(notified)" = "repo feat|ready to merge" ]
}

@test "refresh: from a linked worktree, its branch and the pass's state" {
  git switch -q main
  git worktree add -q "$TMP/feat-wt" feat
  ps_fixture feat
  (cd "$TMP/feat-wt" && "$SD_WATCH" refresh)
  [ "$(glyph feat-wt)" = "✅" ]
  [ "$(cat "$PS_FIXTURES/calls")" = "$TMP/repo feat" ]
  pass
  [ "$(notified)" = "repo feat|ready to merge" ]
}

@test "refresh: fetches the configured remote" {
  git clone -q "$TMP/origin.git" "$TMP/other" 2>/dev/null
  git -C "$TMP/other" commit -q --allow-empty -m moved && git -C "$TMP/other" push -q origin main 2>/dev/null
  git remote add up "$TMP/origin.git"
  write_cfg '{"settings":{"remote":"up"}}'
  ps_fixture feat
  refresh
  [ "$(git rev-parse up/main)" = "$(git -C "$TMP/other" rev-parse main)" ]
}

@test "refresh: a failed fetch is reported and the branch still checked" {
  git remote set-url origin "$TMP/nowhere.git"
  ps_fixture feat
  run refresh
  [ "$status" -eq 0 ]
  [[ "$output" == *"fetch from origin failed: "*"nowhere"* ]] || false
  [ "$(glyph repo)" = "✅" ]
}

@test "refresh: isn't recorded as a pass, and leaves other glyphs alone" {
  git branch other && enroll other
  git worktree add -q "$TMP/other-wt" other
  ps_fixture feat '.ci.verdict = "red" | .ci.failing = ["lint"]'
  ps_fixture other
  pass
  local before; before=$(cat "$SHEEPDOG_STATE_DIR/last-pass")
  ps_fixture feat
  refresh
  [ "$(cat "$SHEEPDOG_STATE_DIR/last-pass")" = "$before" ]
  [ "$(glyph other-wt)" = "✅" ]
  [ "$(glyph repo)" = "✅" ]
}

@test "refresh: before any pass, status still says so" {
  ps_fixture feat
  refresh
  run "$SD_WATCH" status
  [[ "$output" == *"no pass yet"* ]]
}

@test "refresh: sd state failing is ⚠️ and exits non-zero" {
  run refresh
  [ "$status" -ne 0 ]
  [[ "$output" == *"feat  ⚠️  sd state failed"* ]] || false
  [ "$(glyph repo)" = "⚠️" ]
}

@test "refresh: unparseable sd state output is ⚠️ and exits non-zero" {
  printf 'warning: junk\n{"branch":' > "$PS_FIXTURES/feat.json"
  run refresh
  [ "$status" -ne 0 ]
  [[ "$output" == *"feat  ⚠️  unparseable"* ]]
}

@test "refresh: an unreadable config file exits non-zero without checking" {
  ps_fixture feat
  printf '{not json\n' > "$SHEEPDOG_CONFIG"
  run refresh
  [ "$status" -ne 0 ]
  [[ "$output" == *"sd config failed"* ]] || false
  [ ! -e "$PS_FIXTURES/calls" ]
}

@test "refresh: refuses a branch that isn't enrolled, or doesn't exist" {
  git branch other
  run refresh other
  [ "$status" -ne 0 ]
  [[ "$output" == *"other isn't enrolled"*"sd act enroll"* ]] || false
  run refresh nope
  [ "$status" -ne 0 ]
  [[ "$output" == *"no such branch: nope"* ]] || false
  [ ! -e "$PS_FIXTURES/calls" ]
}

@test "refresh: -h prints the help; another option or a second branch is refused" {
  run refresh -h
  [ "$status" -eq 0 ]
  [[ "$output" == *"Usage: sd watch"* ]] || false
  run refresh --nope
  [ "$status" -ne 0 ]
  [[ "$output" == *"unknown option: --nope"* ]] || false
  git branch other && enroll other
  run refresh feat other
  [ "$status" -ne 0 ]
  [[ "$output" == *"usage: sd watch refresh [<branch>]"* ]] || false
  [ ! -e "$PS_FIXTURES/calls" ]
}

@test "refresh: refuses a repo the pass doesn't watch" {
  git config --global --unset-all sheepdog.repo
  ps_fixture feat
  run refresh
  [ "$status" -ne 0 ]
  [[ "$output" == *"$TMP/repo isn't registered"*"sd act enroll"* ]] || false
  [ ! -e "$PS_FIXTURES/calls" ]
}

@test "refresh: refuses a detached HEAD with no branch named" {
  git switch -q --detach
  run refresh
  [ "$status" -ne 0 ]
  [[ "$output" == *"not on a branch"* ]]
}

@test "refresh: outside a git repo, refuses" {
  cd "$TMP"
  run refresh
  [ "$status" -ne 0 ]
  [[ "$output" == *"not in a git repository"* ]]
}

# ── launchd ──────────────────────────────────────────────────────────────────

# Not in setup_launchd: tests refused before plutil runs should still run on Linux.
needs_plutil() {
  command -v plutil >/dev/null || skip "needs plutil (macOS)"
}

setup_launchd() {
  unset LOCAL_BIN
  export SHEEPDOG_WATCH_LAUNCH_AGENTS="$TMP/LaunchAgents"
  cat > "$TMP/fakebin/launchctl" <<'EOF'
#!/usr/bin/env bash
echo "$*" >> "$PS_FIXTURES/launchctl"
EOF
  chmod +x "$TMP/fakebin/launchctl"
  PLIST="$SHEEPDOG_WATCH_LAUNCH_AGENTS/com.sheepdog.watch.plist"
}

OLD_LABELS=(com.agent-rig.pr-watch com.pr-tend.pr-watch)

old_plists() {
  mkdir -p "$SHEEPDOG_WATCH_LAUNCH_AGENTS"
  local l
  for l in "${OLD_LABELS[@]}"; do echo old > "$SHEEPDOG_WATCH_LAUNCH_AGENTS/$l.plist"; done
}

old_labels_retired() {
  local l
  for l in "${OLD_LABELS[@]}"; do
    grep -q "bootout gui/$(id -u)/$l" "$PS_FIXTURES/launchctl"
    [ ! -e "$SHEEPDOG_WATCH_LAUNCH_AGENTS/$l.plist" ]
  done
}

@test "install: writes a valid plist and bootstraps it" {
  needs_plutil
  setup_launchd
  run "$SD_WATCH" install
  [ "$status" -eq 0 ]
  plutil -lint "$PLIST"
  [ "$(plutil -extract StartInterval raw "$PLIST")" = 300 ]
  [[ "$(plutil -extract EnvironmentVariables.PATH raw "$PLIST")" == "$HOME/.local/bin:/"* ]]
  grep -q "bootstrap gui/$(id -u) $PLIST" "$PS_FIXTURES/launchctl"
}

@test "install: the job runs sd watch" {
  needs_plutil
  setup_launchd
  run "$SD_WATCH" install
  [ "$status" -eq 0 ]
  [[ "$(plutil -extract ProgramArguments.0 raw "$PLIST")" == */sd ]]
  [ -x "$(plutil -extract ProgramArguments.0 raw "$PLIST")" ]
  [ "$(plutil -extract ProgramArguments.1 raw "$PLIST")" = watch ]
}

@test "install: the job runs the sd in LOCAL_BIN, which leads its PATH ahead of ~/.local/bin" {
  needs_plutil
  setup_launchd
  mkdir -p "$TMP/local-bin"
  ln -s "$BATS_TEST_DIRNAME/../bin/sd" "$TMP/local-bin/sd"
  LOCAL_BIN="$TMP/local-bin" run "$SD_WATCH" install
  [ "$status" -eq 0 ]
  [ "$(plutil -extract ProgramArguments.0 raw "$PLIST")" = "$TMP/local-bin/sd" ]
  [[ "$(plutil -extract EnvironmentVariables.PATH raw "$PLIST")" == "$TMP/local-bin:$HOME/.local/bin:/"* ]]
}

@test "install: a relative LOCAL_BIN is made absolute, since launchd runs jobs from /" {
  needs_plutil
  setup_launchd
  mkdir -p "$TMP/local-bin"
  ln -s "$BATS_TEST_DIRNAME/../bin/sd" "$TMP/local-bin/sd"
  cd "$TMP"
  LOCAL_BIN=local-bin run "$SD_WATCH" install
  [ "$status" -eq 0 ]
  [ "$(plutil -extract ProgramArguments.0 raw "$PLIST")" = "$TMP/local-bin/sd" ]
  [[ "$(plutil -extract EnvironmentVariables.PATH raw "$PLIST")" == "$TMP/local-bin:"* ]]
}

@test "uninstall: boots out and removes the plist" {
  needs_plutil
  setup_launchd
  "$SD_WATCH" install
  run "$SD_WATCH" uninstall
  [ "$status" -eq 0 ]
  [ ! -e "$PLIST" ]
  grep -q "bootout gui/$(id -u)/com.sheepdog.watch" "$PS_FIXTURES/launchctl"
}

@test "install: boots out the jobs under the old labels and removes their plists" {
  needs_plutil
  setup_launchd
  old_plists
  run "$SD_WATCH" install
  [ "$status" -eq 0 ]
  old_labels_retired
  [ -e "$PLIST" ]
}

@test "uninstall: also boots out the old labels" {
  needs_plutil
  setup_launchd
  old_plists
  run "$SD_WATCH" uninstall
  [ "$status" -eq 0 ]
  old_labels_retired
}

@test "install: a non-numeric interval is refused and the old job is kept" {
  needs_plutil
  setup_launchd
  "$SD_WATCH" install
  : > "$PS_FIXTURES/launchctl"
  write_cfg '{"settings":{"interval":"5m"}}'
  run "$SD_WATCH" install
  [ "$status" -ne 0 ]
  [ "$(plutil -extract StartInterval raw "$PLIST")" = 300 ]
  [ ! -s "$PS_FIXTURES/launchctl" ]
}

@test "install: paths are XML-escaped" {
  needs_plutil
  setup_launchd
  export SHEEPDOG_STATE_DIR="$TMP/a&b"
  "$SD_WATCH" install
  plutil -lint "$PLIST"
  [ "$(plutil -extract StandardOutPath raw "$PLIST")" = "$TMP/a&b/launchd.log" ]
}

# ── settings via sd config: the global file over the defaults ───────────────

# write_cfg <json>: the global file, keeping the stub notifier unless <json> sets one.
write_cfg() {
  jq -n --arg n "$TMP/fakebin/notify" --argjson c "$1" '{settings: {notify: $n}} * $c' > "$SHEEPDOG_CONFIG"
}

@test "config: git config sheepdog.notify is not read" {
  git config --global sheepdog.notify "$TMP/fakebin/missing"
  ps_fixture feat
  pass
  [ "$(cat "$PS_FIXTURES/notified")" = "repo feat|ready to merge" ]
}

@test "config: a branch file in the working directory does not steer the watcher" {
  write_cfg "{\"settings\":{\"notify\":\"$TMP/fakebin/notify\"}}"
  mkdir -p "$TMP/repo/.git/sheepdog"
  printf '{"settings":{"notify":"%s"}}\n' "$TMP/fakebin/missing" > "$TMP/repo/.git/sheepdog/feat.json"
  ps_fixture feat
  (cd "$TMP/repo" && "$SD_WATCH")
  [ "$(cat "$PS_FIXTURES/notified")" = "repo feat|ready to merge" ]
}

@test "config: remote from the file" {
  write_cfg '{"settings":{"remote":"nowhere"}}'
  ps_fixture feat
  pass
  grep -q "fetch from nowhere failed" "$SHEEPDOG_STATE_DIR/last-pass"
}

@test "config: install reads interval from the file" {
  needs_plutil
  setup_launchd
  write_cfg '{"settings":{"interval":120}}'
  "$SD_WATCH" install
  [ "$(plutil -extract StartInterval raw "$PLIST")" = 120 ]
}

@test "config: a non-numeric interval in the file is refused" {
  setup_launchd
  write_cfg '{"settings":{"interval":"5m"}}'
  run "$SD_WATCH" install
  [ "$status" -ne 0 ]
  [ ! -e "$PLIST" ]
}

@test "config: an unreadable config file is reported for the repo, not silently defaulted" {
  printf '{not json\n' > "$SHEEPDOG_CONFIG"
  ps_fixture feat
  pass
  grep -q "repo.*sd config failed" "$SHEEPDOG_STATE_DIR/last-pass"
  [ ! -f "$PS_FIXTURES/calls" ]
}

@test "config: install refuses when the config file is unreadable" {
  setup_launchd
  printf '{not json\n' > "$SHEEPDOG_CONFIG"
  run "$SD_WATCH" install
  [ "$status" -ne 0 ]
  [ ! -e "$PLIST" ]
}

@test "rate limit: vacuum skips the branches the pass didn't check, and says so" {
  git branch other && enroll other
  low feat
  ps_fixture other '.pr.state = "MERGED"'
  run "$SD_WATCH" vacuum
  [ "$status" -eq 0 ]
  [ "$output" = "repo  other  skipped, not checked: GitHub rate limit" ]
  enrolled other
}
