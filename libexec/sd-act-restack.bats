#!/usr/bin/env bats

bats_require_minimum_version 1.5.0

# Test suite for `sd act restack`: move a branch onto its parent (or the
# default branch once the parent merged) with `rebase --onto`, check that its
# own change came through unchanged, and move refs/sheepdog/<b>/base. Never pushes.
#
# Real git (a bare origin, our clone, and $TMP/other for other people's
# pushes) and the real sd state. Fake gh: answers from $GH/<branch>.json, else
# "no PR", and a fetch by number with that node of whichever file has it;
# GH_FAIL_FOR=<branch> makes that branch's lookups fail.

SD_ACT="$BATS_TEST_DIRNAME/sd-act"

setup() {
  TMP="$(cd "$BATS_TEST_TMPDIR" && pwd -P)"
  export GIT_CONFIG_GLOBAL="$TMP/gitconfig-global"
  export GIT_CONFIG_SYSTEM=/dev/null
  export SHEEPDOG_STATE_DIR="$TMP/state"   # harness_run logs usage there
  printf '[user]\n\temail = t@example.com\n\tname = Tester\n[init]\n\tdefaultBranch = main\n' > "$GIT_CONFIG_GLOBAL"
  export SHEEPDOG_CONFIG="$TMP/global.json"
  export GH="$TMP/gh"
  mkdir -p "$GH" "$TMP/fakebin"
  cat > "$TMP/fakebin/gh" <<'EOF'
#!/usr/bin/env bash
if [[ $1 == pr && $2 == edit ]]; then echo "$*" >> "$GH/calls"; exit 0; fi
b="" n=""
for a; do [[ $a == branch=* ]] && b=${a#branch=}; [[ $a == number=* ]] && n=${a#number=}; done
# By number: the node with that number, from the one branch file that has it.
if [[ -n $n ]]; then
  f=$(jq -rn --argjson n "$n" 'inputs | select(any(.data.repository.pullRequests.nodes[]?; .number == $n)) | input_filename' "$GH"/*.json 2>/dev/null)
  [[ $f != *$'\n'* ]] || { echo "fake gh: #$n is in more than one fixture: $f" >&2; exit 2; }
  b=$(basename "${f:-none}" .json)
fi
[[ ${GH_FAIL_FOR:-} == "$b" ]] && { echo "gh: boom for $b" >&2; exit 1; }
if [[ -n $n ]]; then
  [[ -n $f ]] || { echo "gh: Could not resolve to a PullRequest with the number of $n." >&2; exit 1; }
  jq --argjson n "$n" '.data.repository |= {defaultBranchRef, pullRequest: (.pullRequests.nodes[] | select(.number == $n))}' "$f"
elif [[ -f $GH/$b.json ]]; then cat "$GH/$b.json"
else echo '{"data":{"viewer":{"login":"me"},"repository":{"defaultBranchRef":{"name":"main"},"pullRequests":{"nodes":[]}}}}'
fi
EOF
  export FX="$TMP/fx"; mkdir -p "$FX"
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
echo "$PWD" >> "$FX/claude.calls"
[[ -x $FX/agent ]] && "$FX/agent"
[[ -e $FX/fail ]] && { echo "claude: boom" >&2; exit 1; }
jq -n --argjson r "$(cat "$FX/result.json")" '{type: "result", is_error: false, structured_output: $r}'
EOF
  chmod +x "$TMP/fakebin/gh" "$TMP/fakebin/claude"
  echo '{"outcome": "done", "summary": "kept both"}' > "$FX/result.json"
  export PATH="$BATS_TEST_DIRNAME:$TMP/fakebin:$PATH"

  git init -q --bare "$TMP/origin.git"
  git clone -q "$TMP/origin.git" "$TMP/repo" 2>/dev/null
  cd "$TMP/repo"
  printf '%s\n' a b c d e f g h > file.txt
  git add file.txt && git commit -qm init && git push -q origin main 2>/dev/null
  git clone -q "$TMP/origin.git" "$TMP/other" 2>/dev/null
  git switch -qc feat
  edit 's/^g$/G/' feat
  git push -q -u origin feat 2>/dev/null
}

# edit <perl-expr> <message>: change file.txt here and commit it.
edit() { perl -pi -e "$1" file.txt && git commit -qam "$2"; }

# upstream <branch> <perl-expr> [message]: someone else commits to <branch> on origin.
upstream() {
  (cd "$TMP/other" && git fetch -q origin && git checkout -q -B "$1" "origin/$1" \
     && perl -pi -e "$2" file.txt && git commit -qam "${3:-upstream}" && git push -q origin "$1" 2>/dev/null)
}

# merged_pr <branch> <head-sha> [<commit>...]: gh says <branch>'s PR merged with that
# head, and those commits.
merged_pr() {
  local b=$1 h=$2; shift 2
  jq -n --arg b "$b" --arg h "$h" --args '{data: {viewer: {login: "me"},
    repository: {defaultBranchRef: {name: "main"},
      pullRequests: {nodes: [{number: 1, state: "MERGED", baseRefName: "main", headRefName: $b,
                              headRefOid: $h, isCrossRepository: false,
                              allCommits: {nodes: [$ARGS.positional[] | {commit: {oid: .}}]}}]}}}}' "$@" > "$GH/$b.json"
}

# open_pr <branch> <base> <number>: gh says <branch> has an open PR into <base>.
open_pr() {
  jq -n --arg b "$1" --arg base "$2" --argjson n "$3" --arg h "$(git rev-parse "$1")" '{data: {viewer: {login: "me"},
    repository: {defaultBranchRef: {name: "main"},
      pullRequests: {nodes: [{number: $n, state: "OPEN", baseRefName: $base, headRefName: $b,
                              headRefOid: $h, isCrossRepository: false}]}}}}' > "$GH/$1.json"
}

# closed_pr <branch> <head-sha>: gh says <branch>'s PR was closed without merging.
closed_pr() {
  jq -n --arg b "$1" --arg h "$2" '{data: {viewer: {login: "me"},
    repository: {defaultBranchRef: {name: "main"},
      pullRequests: {nodes: [{number: 1, state: "CLOSED", baseRefName: "main", headRefName: $b,
                              headRefOid: $h, isCrossRepository: false}]}}}}' > "$GH/$1.json"
}

# stacked_child: A off main, child off A with sd-parent A; both pushed. Sets $a_head.
stacked_child() {
  git switch -q main && git switch -qc A
  edit 's/^c$/C/' a1
  git push -q origin A 2>/dev/null
  a_head=$(git rev-parse A)
  git switch -qc child
  edit 's/^f$/F/' child
  git push -q -u origin child 2>/dev/null
  git config branch.child.sd-parent A
}

remote_feat() { git ls-remote origin refs/heads/feat | cut -f1; }
base_pin() { git rev-parse -q --verify refs/sheepdog/feat/base || true; }

@test "help lists restack and push" {
  run "$SD_ACT" --help
  [[ "$output" == *"sd act restack"* ]]
  [[ "$output" == *"sd act push"* ]]
}

@test "onto the moved default branch: rebased, base pinned, not pushed" {
  upstream main 's/^b$/B/'
  before_remote=$(remote_feat)
  run "$SD_ACT" restack
  [ "$status" -eq 0 ]
  [ "$(git rev-parse feat^)" = "$(git rev-parse origin/main)" ]
  [ "$(git log --format=%s origin/main..feat)" = feat ]
  grep -qx B file.txt && grep -qx G file.txt
  [ "$(base_pin)" = "$(git rev-parse origin/main)" ]
  [ "$(remote_feat)" = "$before_remote" ]
  [[ "$output" == *"restacked feat onto origin/main"* ]]
  [[ "$output" == *"not pushed: a plain git push is rejected"*"sd act push"* ]] || false
}

@test "onto the moved default branch, branch never pushed: no plain-push warning, just sd act push (#47)" {
  git push -q origin :feat 2>/dev/null && git fetch -q --prune
  upstream main 's/^b$/B/'
  run "$SD_ACT" restack
  [ "$status" -eq 0 ]
  [[ "$output" == *"; not pushed: sd act push" ]] || false
  [[ "$output" != *"rejected"* ]] || false
}

@test "already on the target: nothing to do" {
  before=$(git rev-parse HEAD)
  run "$SD_ACT" restack
  [ "$status" -eq 0 ]
  [[ "$output" == *"already on origin/main"* ]]
  [ "$(git rev-parse HEAD)" = "$before" ]
  [ -z "$(base_pin)" ]
}

@test "fetches first: a move it hasn't seen yet still counts" {
  upstream main 's/^b$/B/'
  [ "$(git rev-parse origin/main)" != "$(git -C "$TMP/other" rev-parse main)" ]
  run "$SD_ACT" restack
  [ "$status" -eq 0 ]
  [ "$(git rev-parse feat^)" = "$(git -C "$TMP/other" rev-parse main)" ]
}

@test "parent squash-merged: lands on main with only its own commits" {
  git switch -q main
  git switch -qc A
  edit 's/^c$/C/' a1
  edit 's/^d$/D/' a2
  git push -q origin A 2>/dev/null
  a_head=$(git rev-parse A)
  git switch -qc child
  edit 's/^f$/F/' child
  git config branch.child.sd-parent A
  (cd "$TMP/other" && git fetch -q origin && git checkout -q main && git merge -q --squash origin/A \
     && git commit -qm "A (#1)" && git push -q origin main 2>/dev/null)
  merged_pr A "$a_head" "$(git rev-parse A~1)" "$a_head"
  run "$SD_ACT" restack
  [ "$status" -eq 0 ]
  [ "$(git rev-parse child^)" = "$(git rev-parse origin/main)" ]
  [ "$(git log --format=%s origin/main..child)" = child ]
  grep -qx C file.txt && grep -qx D file.txt && grep -qx F file.txt
  [ "$(git rev-parse refs/sheepdog/child/base)" = "$(git rev-parse origin/main)" ]
  [ "$(git config branch.child.sd-parent)" = main ]
}

@test "parent merged but the PR still targets it: sd-parent kept, so retarget is still owed" {
  stacked_child
  (cd "$TMP/other" && git fetch -q origin && git checkout -q main && git merge -q --squash origin/A >/dev/null \
     && git commit -qm "A (#1)" && git push -q origin main 2>/dev/null)
  merged_pr A "$a_head"
  open_pr child A 2
  run "$SD_ACT" restack
  [ "$status" -eq 0 ]
  [ "$(git rev-parse child^)" = "$(git rev-parse origin/main)" ]
  [ "$(git config branch.child.sd-parent)" = A ]
  [[ "$output" == *"PR #2 still targets A: sd act retarget"* ]]
  run sd-state --json child
  [ "$(jq -r '[.next[].key] | index("retarget") != null' <<<"$output")" = true ]
  run "$SD_ACT" retarget
  [ "$status" -eq 0 ]
  [ "$(cat "$GH/calls")" = "pr edit 2 --base main" ]
  [ "$(git config branch.child.sd-parent)" = main ]
}

@test "parent merged, the branch's own PR lookup fails: sd-parent kept" {
  stacked_child
  (cd "$TMP/other" && git fetch -q origin && git checkout -q main && git merge -q --squash origin/A >/dev/null \
     && git commit -qm "A (#1)" && git push -q origin main 2>/dev/null)
  merged_pr A "$a_head"
  GH_FAIL_FOR=child run "$SD_ACT" restack
  [ "$status" -eq 0 ]
  [ "$(git rev-parse child^)" = "$(git rev-parse origin/main)" ]
  [ "$(git config branch.child.sd-parent)" = A ]
  [[ "$output" == *"sd-parent of child stays A: can't read its PR"* ]]
}

@test "restack onto a parent closed without merging is refused" {
  stacked_child
  closed_pr A "$a_head"
  upstream A 's/^d$/D/' a2
  run "$SD_ACT" restack
  [ "$status" -ne 0 ]
  [[ "$output" == *"closed without merging"* ]]
  [ "$(git rev-parse child^)" = "$a_head" ]
}

@test "stacked, pin lost, parent squash-merged and deleted: refused, no rebase, no agent" {
  git switch -q main && git switch -qc A
  edit 's/^c$/C/' a1
  edit 's/^d$/D/' a2
  git push -q origin A 2>/dev/null
  a_head=$(git rev-parse A)
  git switch -qc child A~1
  edit 's/^d$/DD/' "a2, changed"
  edit 's/^f$/F/' child
  git config branch.child.sd-parent A
  git update-ref refs/sheepdog/child/base "$a_head"
  (cd "$TMP/other" && git fetch -q origin && git checkout -q main && git merge -q --squash origin/A >/dev/null \
     && git commit -qm "A (#1)" && git push -q origin main :A 2>/dev/null)
  merged_pr A "$a_head"
  before=$(git rev-parse child)
  run "$SD_ACT" restack --resolve
  [ "$status" -ne 0 ]
  [[ "$output" == *"base pin ${a_head:0:7} is no longer in child's history"* ]]
  [ "$(git rev-parse child)" = "$before" ]
  ! rebase_in_progress
  [ ! -e "$FX/claude.calls" ]
}

# orphaned_pin: A (a1, a2) squash-merged into main and deleted; child was
# restacked onto A, then rewritten outside sheepdog: a2 is now an exact copy,
# so the pin at A's head isn't an ancestor. Sets $a1 and $a_head.
orphaned_pin() {
  git switch -q main && git switch -qc A
  edit 's/^c$/C/' a1
  edit 's/^d$/D/' a2
  git push -q origin A 2>/dev/null
  a1=$(git rev-parse A~1) a_head=$(git rev-parse A)
  git switch -qc child A~1
  edit 's/^d$/D/' "a2 again"
  edit 's/^f$/F/' child
  git config branch.child.sd-parent A
  git update-ref refs/sheepdog/child/base "$a_head"
  (cd "$TMP/other" && git fetch -q origin && git checkout -q main && git merge -q --squash origin/A >/dev/null \
     && git commit -qm "A (#1)" && git push -q origin main :A 2>/dev/null)
}

@test "orphaned pin, all of the parent in HEAD: one commit on main by tree, no agent" {
  orphaned_pin
  merged_pr A "$a_head" "$a1" "$a_head"
  old_head=$(git rev-parse child)
  run "$SD_ACT" restack --resolve
  [ "$status" -eq 0 ]
  [[ "$output" == *"restacked child onto origin/main by tree (1 commit)"* ]]
  [[ "$output" == *"child's commits are now one, committed by you; the old ones are at ${old_head:0:7}"* ]]
  [ "$(git rev-parse child^)" = "$(git rev-parse origin/main)" ]
  [ "$(git rev-list --count origin/main..child)" = 1 ]
  [ "$(git diff origin/main child)" = "$(git diff "$a_head" "$old_head")" ]
  [ "$(git log -1 --format=%B child)" = "child (restacked by tree onto origin/main)

Replaces:
child" ]
  [ "$(git rev-parse refs/sheepdog/child/base)" = "$(git rev-parse origin/main)" ]
  grep -qx F file.txt
  [ ! -e "$FX/claude.calls" ]
}

@test "a tree restack's commit is the branch's own in its reflog, not a reset (stacked_on, #94)" {
  orphaned_pin
  merged_pr A "$a_head" "$a1" "$a_head"
  run "$SD_ACT" restack --resolve
  [ "$status" -eq 0 ]
  [ "$(git reflog -1 --format=%H refs/heads/child)" = "$(git rev-parse child)" ]
  [[ "$(git reflog -1 --format=%gs refs/heads/child)" != "reset: moving to "* ]]
}

@test "orphaned pin, a tree restack that would conflict: refused even with --resolve" {
  orphaned_pin
  upstream main 's/^f$/X/'
  merged_pr A "$a_head" "$a1" "$a_head"
  before=$(git rev-parse child)
  run "$SD_ACT" restack --resolve
  [ "$status" -ne 0 ]
  [[ "$output" == *"by tree would conflict"* ]]
  [[ "$output" == *"By hand: git switch child && git reset --hard origin/main && git diff --binary ${a_head:0:12} ${before:0:12} | git apply -3, resolve and commit, then git update-ref refs/sheepdog/child/base origin/main (the old head stays at ${before:0:12})"* ]]
  [ "$(git rev-parse child)" = "$before" ]
  [ ! -e "$FX/claude.calls" ]
}

@test "parent squash-merged, base below the parent's commits: refused, no rebase, no agent" {
  git switch -q main && git switch -qc A
  edit 's/^c$/C/' a1
  edit 's/^d$/D/' a2
  git push -q origin A 2>/dev/null
  a1=$(git rev-parse A~1) a_head=$(git rev-parse A)
  git switch -qc child
  edit 's/^f$/F/' child
  git config branch.child.sd-parent A
  git update-ref refs/sheepdog/child/base "$(git rev-parse main)"
  (cd "$TMP/other" && git fetch -q origin && git checkout -q main && git merge -q --squash origin/A >/dev/null \
     && git commit -qm "A (#1)" && git push -q origin main 2>/dev/null)
  merged_pr A "$a_head" "$a1" "$a_head"
  before=$(git rev-parse child)
  run "$SD_ACT" restack --resolve
  [ "$status" -ne 0 ]
  [[ "$output" == *"wrong base: this would replay 2 of #1's commits onto origin/main; if the stack is linear, rebase past them by hand: git rebase --onto origin/main ${a_head:0:12} child && git update-ref refs/sheepdog/child/base origin/main"* ]]
  [ "$(git rev-parse child)" = "$before" ]
  ! rebase_in_progress
  [ ! -e "$FX/claude.calls" ]
  eval "${output##*by hand: }" 2>/dev/null
  [ "$(git log --format=%s origin/main..child)" = child ]
  [ "$(git rev-parse refs/sheepdog/child/base)" = "$(git rev-parse origin/main)" ]
  run "$SD_ACT" restack
  [ "$status" -eq 0 ]
  [[ "$output" == *"already on origin/main"* ]]
}

# ── retarget ─────────────────────────────────────────────────────────────────

@test "retarget: parent merged, PR still on it: base edited to main, sd-parent rewritten" {
  stacked_child
  merged_pr A "$a_head"
  open_pr child A 2
  run "$SD_ACT" retarget
  [ "$status" -eq 0 ]
  [ "$(cat "$GH/calls")" = "pr edit 2 --base main" ]
  [ "$(git config branch.child.sd-parent)" = main ]
}

@test "retarget: the PR already targets main (GitHub did it): no edit, sd-parent still rewritten" {
  stacked_child
  merged_pr A "$a_head"
  open_pr child main 2
  run "$SD_ACT" retarget
  [ "$status" -eq 0 ]
  [ ! -e "$GH/calls" ]
  [ "$(git config branch.child.sd-parent)" = main ]
}

@test "retarget: parent still open: refused" {
  stacked_child
  open_pr A main 1
  open_pr child A 2
  run "$SD_ACT" retarget
  [ "$status" -ne 0 ]
  [[ "$output" == *"hasn't merged"* ]]
  [ ! -e "$GH/calls" ]
  [ "$(git config branch.child.sd-parent)" = A ]
}

@test "retarget: parent closed without merging: refused, says so" {
  stacked_child
  closed_pr A "$a_head"
  open_pr child A 2
  run "$SD_ACT" retarget
  [ "$status" -ne 0 ]
  [[ "$output" == *"closed without merging"* ]]
  [ ! -e "$GH/calls" ]
}

@test "retarget: a branch with no PR is refused" {
  stacked_child
  merged_pr A "$a_head"
  run "$SD_ACT" retarget
  [ "$status" -ne 0 ]
  [[ "$output" == *"no PR"* ]]
}

@test "stacked on an open parent: onto the parent, not main" {
  git switch -q main
  git switch -qc A
  edit 's/^c$/C/' a1
  git push -q origin A 2>/dev/null
  git switch -qc child
  edit 's/^f$/F/' child
  git config branch.child.sd-parent A
  upstream A 's/^d$/D/' a2
  upstream main 's/^b$/B/'
  open_pr A main 1
  open_pr child A 2
  run "$SD_ACT" restack
  [ "$status" -eq 0 ]
  [ "$(git rev-parse child^)" = "$(git rev-parse origin/A)" ]
  ! grep -qx B file.txt
  [[ "$output" != *"sd act retarget"* ]]
  [ "$(git config branch.child.sd-parent)" = A ]
}

@test "a restack that would conflict is refused; nothing moves" {
  upstream main 's/^g$/X/'
  before=$(git rev-parse HEAD)
  run "$SD_ACT" restack
  [ "$status" -eq 1 ]
  [[ "$output" == *"would conflict"* ]]
  [[ "$output" == *"git rebase --onto origin/main $(git rev-parse --short=12 main) feat, then git update-ref refs/sheepdog/feat/base \$(git merge-base origin/main feat)"* ]]
  [[ "$output" == *"sd act restack --resolve"* ]]
  [ "$(git rev-parse HEAD)" = "$before" ]
  [ -z "$(base_pin)" ]
}

@test "a conflict partway through is rolled back" {
  # Net, feat leaves line e alone, so the merge sd state predicts is clean;
  # replaying e→E commit by commit still hits main's e→X.
  edit 's/^e$/E/' there
  edit 's/^E$/e/' back
  upstream main 's/^e$/X/'
  before=$(git rev-parse HEAD)
  run "$SD_ACT" restack
  [ "$status" -eq 1 ]
  [[ "$output" == *"rolled back"* ]]
  onto=$(git rev-parse --short=12 origin/main)
  [[ "$output" == *"git rebase --onto $onto $(git rev-parse --short=12 main) feat, then git update-ref refs/sheepdog/feat/base $onto"* ]]
  [ "$(git rev-parse HEAD)" = "$before" ]
  [ "$(git branch --show-current)" = feat ]
  [ -z "$(git status --porcelain)" ]
  [ ! -e "$(git rev-parse --git-path rebase-merge)" ]
  [ ! -e "$(git rev-parse --git-path rebase-apply)" ]
  [ -z "$(base_pin)" ]
}

@test "a change that comes out different is rolled back" {
  # main picks up the same edit as feat's first commit, so the rebase drops it
  # and the branch's diff shrinks.
  edit 's/^b$/B/' second
  upstream main 's/^g$/G/'
  before=$(git rev-parse HEAD)
  run "$SD_ACT" restack
  [ "$status" -eq 1 ]
  [[ "$output" == *"came out different"* ]]
  [[ "$output" == *"rolled back"* ]]
  [ "$(git rev-parse HEAD)" = "$before" ]
  [ -z "$(git status --porcelain)" ]
  [ -z "$(base_pin)" ]
}

@test "uncommitted changes are refused" {
  upstream main 's/^b$/B/'
  echo dirty >> file.txt
  run "$SD_ACT" restack
  [ "$status" -eq 1 ]
  [[ "$output" == *"uncommitted changes"* ]]
}

@test "taken over: refused unless --force" {
  upstream main 's/^b$/B/'
  git config branch.feat.sd-lock chrismo
  run "$SD_ACT" restack
  [ "$status" -eq 1 ]
  [[ "$output" == *"taken over"* ]]
  [[ "$output" == *"--force"* ]]
  run "$SD_ACT" restack --force
  [ "$status" -eq 0 ]
  [ "$(git rev-parse feat^)" = "$(git rev-parse origin/main)" ]
}

@test "a parent that isn't on the remote is refused with sd state's reason" {
  git config branch.feat.sd-parent nosuch
  run "$SD_ACT" restack
  [ "$status" -eq 1 ]
  [[ "$output" == *"no origin/nosuch ref"* ]]
}

@test "a parent whose merged state is unknown is refused" {
  git switch -q main
  git switch -qc A
  edit 's/^c$/C/' a1
  git push -q origin A 2>/dev/null
  git switch -qc child
  edit 's/^f$/F/' child
  git config branch.child.sd-parent A
  upstream A 's/^d$/D/' a2
  GH_FAIL_FOR=A run "$SD_ACT" restack
  [ "$status" -eq 1 ]
  [[ "$output" == *"can't tell whether A merged"* ]]
}

@test "a parent only here is refused with the step: ship it first (#105)" {
  git switch -q main && git switch -qc A
  edit 's/^c$/C/' a1
  git switch -qc child
  edit 's/^f$/F/' child
  git config branch.child.sd-parent A
  run "$SD_ACT" restack
  [ "$status" -eq 1 ]
  [[ "$output" == *"can't restack child: parent A isn't on origin yet: ship it first (sd act ship A)"* ]]
}

@test "a branch not checked out anywhere is refused" {
  git switch -q main
  run "$SD_ACT" restack feat
  [ "$status" -eq 1 ]
  [[ "$output" == *"isn't checked out in any worktree"* ]]
}

@test "a named branch is restacked in its own worktree" {
  upstream main 's/^b$/B/'
  git switch -q main
  git worktree add -q "$TMP/wt" feat
  run "$SD_ACT" restack feat
  [ "$status" -eq 0 ]
  [ "$(git rev-parse feat^)" = "$(git rev-parse origin/main)" ]
  grep -qx B "$TMP/wt/file.txt"
  [ "$(git branch --show-current)" = main ]
}

@test "enroll records a stacked branch's parent and base, so restack survives GitHub's retarget" {
  git switch -q main && git switch -qc A
  edit 's/^c$/C/' a1
  git push -q origin A 2>/dev/null
  a_head=$(git rev-parse A)
  git switch -qc child
  edit 's/^f$/F/' child
  git push -q -u origin child 2>/dev/null
  open_pr A main 1
  open_pr child A 2
  run "$SD_ACT" enroll child
  [ "$status" -eq 0 ]
  [ "$(git config branch.child.sd-parent)" = A ]
  [ "$(git rev-parse refs/sheepdog/child/base)" = "$a_head" ]
  (cd "$TMP/other" && git fetch -q origin && git checkout -q main && git merge -q --squash origin/A \
     && git commit -qm "A (#1)" && git push -q origin main 2>/dev/null)
  merged_pr A "$a_head"
  open_pr child main 2
  run "$SD_ACT" restack
  [ "$status" -eq 0 ]
  [ "$(git rev-parse child^)" = "$(git rev-parse origin/main)" ]
  [ "$(git log --format=%s origin/main..child)" = child ]
}

# As on a second machine: the parent moved on before this clone saw where the
# branch left it, so there's no fork-point and nothing to pin (#97).
@test "enroll of a stacked branch whose base git can't find: records the parent, pins nothing, says how to pin it" {
  git switch -q main && git switch -qc A
  edit 's/^c$/C/' a1
  git push -q origin A 2>/dev/null
  git switch -qc child
  edit 's/^f$/F/' child
  git push -q -u origin child 2>/dev/null
  upstream A 's/^d$/D/' a2
  git fetch -q
  git reflog expire --expire=now --expire-unreachable=now refs/remotes/origin/A
  open_pr A main 1
  open_pr child A 2
  run "$SD_ACT" enroll child
  [ "$status" -eq 0 ]
  [ "$(git config branch.child.sd-parent)" = A ]
  [ -z "$(git rev-parse -q --verify refs/sheepdog/child/base || true)" ]
  [[ "$output" == *"warning: no usable base for child: no base for child on A"*"git update-ref refs/sheepdog/child/base"* ]]
}

@test "enroll records nothing for a branch whose PR targets the default branch" {
  open_pr feat main 1
  run "$SD_ACT" enroll
  [ "$status" -eq 0 ]
  [ -z "$(git config branch.feat.sd-parent || true)" ]
  [ -z "$(base_pin)" ]
}

# ── --resolve: the agent resolves conflicts (J1) ─────────────────────────────

# resolver <replacement>: an agent that replaces every conflict block in
# file.txt with one line.
resolver() {
  printf '#!/usr/bin/env bash\nperl -0pi -e "s/<<<<<<<.*?>>>>>>>[^\\n]*\\n/%s\\n/sg" file.txt\n' "$1" > "$FX/agent"
  chmod +x "$FX/agent"
}
rebase_in_progress() { [[ -e $(git rev-parse --git-path rebase-merge) || -e $(git rev-parse --git-path rebase-apply) ]]; }

@test "--resolve: the agent resolves a predicted conflict; base pinned; its change is unreviewed" {
  upstream main 's/^g$/X/'
  resolver GX
  run "$SD_ACT" restack --resolve
  [ "$status" -eq 0 ]
  [ "$(git rev-parse feat^)" = "$(git rev-parse origin/main)" ]
  grep -qx GX file.txt
  ! grep -q '<<<<<<<' file.txt
  [ "$(base_pin)" = "$(git rev-parse origin/main)" ]
  [[ "$output" == *"resolved"* ]]
  [[ "$output" == *"sd act review"* ]]
  ! rebase_in_progress
  [ -z "$(git status --porcelain)" ]
}

@test "--resolve: an agent that changes CI config while resolving holds the restacked branch for chrismo (#124)" {
  upstream main 's/^g$/X/'
  resolver GX
  printf 'mkdir -p .github/workflows && echo x > .github/workflows/new.yml\n' >> "$FX/agent"
  run "$SD_ACT" restack --resolve
  [ "$status" -eq 0 ]
  [[ "$output" == *"held: the agent changed CI config (.github/workflows/new.yml): review, then sd act push"* ]]
  [ "$(git config --get branch.feat.sd-held)" = "$(git rev-parse feat)"$'\n'".github/workflows/new.yml" ]
}

@test "--resolve: a CI config edit the agent staged itself is held all the same (#124)" {
  upstream main 's/^g$/X/'
  resolver GX
  printf 'mkdir -p .github/workflows && echo x > .github/workflows/new.yml && git add .github/workflows/new.yml\n' >> "$FX/agent"
  run "$SD_ACT" restack --resolve
  [ "$status" -eq 0 ]
  [ "$(git config --get branch.feat.sd-held)" = "$(git rev-parse feat)"$'\n'".github/workflows/new.yml" ]
}

@test "--resolve: a CI config file with a non-ASCII name is held under its own name (#124)" {
  upstream main 's/^g$/X/'
  resolver GX
  printf 'mkdir -p .github/workflows && echo x > .github/workflows/tëst.yml\n' >> "$FX/agent"
  run "$SD_ACT" restack --resolve
  [ "$status" -eq 0 ]
  [ "$(git config --get branch.feat.sd-held)" = "$(git rev-parse feat)"$'\n'".github/workflows/tëst.yml" ]
}

@test "--resolve: a resolution that stays out of CI config holds nothing (#124)" {
  upstream main 's/^g$/X/'
  resolver GX
  run "$SD_ACT" restack --resolve
  [ "$status" -eq 0 ]
  run git config --get branch.feat.sd-held
  [ "$status" -eq 1 ]
}

@test "--resolve: settings.resolvePrompt replaces the guidance; the rebase boundaries stay" {
  git config branch.feat.sd-task "capitalise g"
  upstream main 's/^g$/X/'
  resolver GX
  printf 'Prefer the target.\n' > "$TMP/resolve.md"
  echo "{\"settings\": {\"resolvePrompt\": \"$TMP/resolve.md\"}}" > "$SHEEPDOG_CONFIG"
  "$SD_ACT" restack --resolve >/dev/null
  [ "$(head -1 "$FX/claude.prompt")" = "Prefer the target." ]
  ! grep -q 'keeps this branch.s intent' "$FX/claude.prompt" || false
  grep -q 'Do not run git add' "$FX/claude.prompt"
  grep -q 'already-on-target' "$FX/claude.prompt"
}

@test "--resolve: the agent sees the conflicting files and hunks, in the branch's session, edit tools only" {
  git config branch.feat.sd-task "capitalise g"
  upstream main 's/^g$/X/'
  resolver GX
  "$SD_ACT" restack --resolve >/dev/null
  grep -q 'file.txt' "$FX/claude.prompt"
  grep -q '<<<<<<<' "$FX/claude.prompt"
  grep -q 'capitalise g' "$FX/claude.prompt"
  [ "$(cat "$FX/claude.calls")" = "$TMP/repo" ]
  sid=$(git config branch.feat.sd-session)
  [ -n "$sid" ]
  grep -qx -- "$sid" "$FX/claude.args"
  allow=$(grep -A1 -x -- --allowedTools "$FX/claude.args" | tail -1)
  [[ $allow == *Edit* && $allow != *"git add"* && $allow != *"rebase"* ]]
}

@test "--resolve: the agent is told which commit it is replaying, out of which" {
  edit 's/^b$/B/' "feat two"
  upstream main 's/^g$/X/'
  resolver GX
  own1=$(git rev-parse --short feat~1) own2=$(git rev-parse --short feat)
  "$SD_ACT" restack --resolve >/dev/null
  grep -qx "Replaying: $own1 feat" "$FX/claude.prompt"
  grep -qx " file.txt | 2 +-" "$FX/claude.prompt"
  [ "$(sed -n '/^Commits being replayed, oldest first:$/,/^$/p' "$FX/claude.prompt")" = "Commits being replayed, oldest first:
$own1 feat
$own2 feat two" ]
}

@test "--resolve: the agent saying the change is already on the target rolls back as a wrong base" {
  upstream main 's/^g$/X/'
  echo '{"outcome": "already-on-target", "summary": "main already has this"}' > "$FX/result.json"
  before=$(git rev-parse HEAD) replayed=$(git rev-parse --short HEAD)
  run "$SD_ACT" restack --resolve
  [ "$status" -eq 1 ]
  [[ "$output" == *"wrong base: the agent says $replayed feat is already on origin/main (main already has this)"* ]]
  [[ "$output" == *"rebase past it by hand: git rebase --onto origin/main $replayed feat"* ]]
  [ "$(git rev-parse HEAD)" = "$before" ]
  ! rebase_in_progress
  [ -z "$(base_pin)" ]
}

@test "--resolve: a conflict partway through goes to the agent too" {
  edit 's/^e$/E/' there
  edit 's/^E$/e/' back
  upstream main 's/^e$/X/'
  resolver X
  run "$SD_ACT" restack --resolve
  [ "$status" -eq 0 ]
  # both stops resolved to main's line, so git dropped the two now-empty commits
  [ "$(wc -l < "$FX/claude.calls" | tr -d ' ')" = 2 ]
  [ "$(git rev-parse feat^)" = "$(git rev-parse origin/main)" ]
  grep -qx X file.txt && grep -qx G file.txt
  ! rebase_in_progress
}

@test "--resolve: the agent giving up rolls back" {
  upstream main 's/^g$/X/'
  echo '{"outcome": "gave-up", "summary": "no sensible merge"}' > "$FX/result.json"
  before=$(git rev-parse HEAD)
  run "$SD_ACT" restack --resolve
  [ "$status" -eq 1 ]
  [[ "$output" == *"gave up"* ]]
  [[ "$output" == *"no sensible merge"* ]]
  [ "$(git rev-parse HEAD)" = "$before" ]
  ! rebase_in_progress
  [ -z "$(git status --porcelain)" ]
  [ -z "$(base_pin)" ]
}

@test "--resolve: leftover conflict markers roll back" {
  upstream main 's/^g$/X/'
  printf '#!/usr/bin/env bash\ntrue\n' > "$FX/agent" && chmod +x "$FX/agent"
  before=$(git rev-parse HEAD)
  run "$SD_ACT" restack --resolve
  [ "$status" -eq 1 ]
  [[ "$output" == *"conflict markers"* ]]
  [ "$(git rev-parse HEAD)" = "$before" ]
  ! rebase_in_progress
}

@test "--resolve: the agent step failing rolls back" {
  upstream main 's/^g$/X/'
  touch "$FX/fail"
  before=$(git rev-parse HEAD)
  run "$SD_ACT" restack --resolve
  [ "$status" -eq 1 ]
  [ "$(git rev-parse HEAD)" = "$before" ]
  ! rebase_in_progress
}

@test "--resolve: verify runs on the result; a failure keeps the rebased branch and says so" {
  "$BATS_TEST_DIRNAME/sd-config" set --global settings.verify "grep -qx nope file.txt"
  upstream main 's/^g$/X/'
  resolver GX
  run "$SD_ACT" restack --resolve
  [ "$status" -eq 1 ]
  [[ "$output" == *"verify failed"* ]]
  [ "$(git rev-parse feat^)" = "$(git rev-parse origin/main)" ]
  [ "$(base_pin)" = "$(git rev-parse origin/main)" ]
}

@test "--resolve: more than 3 stops gives up and rolls back" {
  edit 's/^a$/A/' a1
  edit 's/^b$/B/' b1
  edit 's/^c$/C/' c1
  edit 's/^d$/D/' d1
  upstream main 's/^a$/1/; s/^b$/2/; s/^c$/3/; s/^d$/4/'
  resolver R
  before=$(git rev-parse HEAD)
  run "$SD_ACT" restack --resolve
  [ "$status" -eq 1 ]
  [[ "$output" == *"stopped 4 times"* ]]
  [ "$(git rev-parse HEAD)" = "$before" ]
  ! rebase_in_progress
}

@test "--resolve with nothing to resolve is a plain restack, no agent" {
  upstream main 's/^b$/B/'
  run "$SD_ACT" restack --resolve
  [ "$status" -eq 0 ]
  [ ! -e "$FX/claude.calls" ]
  [ "$(git rev-parse feat^)" = "$(git rev-parse origin/main)" ]
}
