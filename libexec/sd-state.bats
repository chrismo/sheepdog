#!/usr/bin/env bats

bats_require_minimum_version 1.5.0

# Test suite for libexec/sd-state (where a branch's PR stands; writes only its number).
#
# Real git: a bare origin, a working clone, and a second clone ("other") that
# plays everyone else pushing. GitHub is a fake `gh` that answers sd state's two
# GraphQL queries from $GH_FIXTURES/<branch>.json: a discovery by branch= gets
# the file, and a fetch by number= gets the node with that number from
# whichever file has it, its headRefName that file's branch unless the node
# names one. Discovery costs 1, a fetch 2, both with $GH_FAKE_REMAINING left.
# GH_FAKE_GONE=<n>: a fetch of #n finds no such PR.

SD_STATE="$BATS_TEST_DIRNAME/sd-state"

setup() {
  TMP="$(cd "$BATS_TEST_TMPDIR" && pwd -P)"
  export GIT_CONFIG_GLOBAL="$TMP/gitconfig-global"
  export GIT_CONFIG_SYSTEM=/dev/null
  export SHEEPDOG_CONFIG="$TMP/global.json"
  export SHEEPDOG_STATE_DIR="$TMP/state"   # usage.sup, for the branch's spend
  SD_CONFIG="$BATS_TEST_DIRNAME/sd-config"
  printf '[user]\n\temail = t@example.com\n\tname = Tester\n[init]\n\tdefaultBranch = main\n' > "$GIT_CONFIG_GLOBAL"

  export GH_FIXTURES="$TMP/gh"
  mkdir -p "$GH_FIXTURES" "$TMP/fakebin"
  cat > "$TMP/fakebin/gh" <<'EOF'
#!/usr/bin/env bash
if [[ -n ${GH_FAKE_FAIL:-} ]]; then echo "$GH_FAKE_FAIL" >&2; exit 1; fi
# REST: the base branch's rulesets and classic protection, from
# $GH_FIXTURES/rules.json and branch.json (default: none, unprotected).
# GH_FAKE_FAIL_REST fails the paths matching GH_FAKE_FAIL_REST_ON (default all).
if [[ $1 == api && $2 == repos/* ]]; then
  echo "$2" >> "$GH_FIXTURES/rest"
  # shellcheck disable=SC2053 # the pattern is a glob on purpose
  if [[ -n ${GH_FAKE_FAIL_REST:-} && $2 == ${GH_FAKE_FAIL_REST_ON:-*} ]]; then echo "$GH_FAKE_FAIL_REST" >&2; exit 1; fi
  case $2 in
    */rules/branches/*) cat "$GH_FIXTURES/rules.json" 2>/dev/null || echo '[]' ;;
    */branches/*) cat "$GH_FIXTURES/branch.json" 2>/dev/null || echo '{"name": "main", "protected": false}' ;;
    *) echo "fake gh: unexpected api call: $*" >&2; exit 2 ;;
  esac
  exit 0
fi
b="" c="" n="" prev=""
for a; do
  case $a in
    branch=*)
      b=${a#branch=}
      # real gh -F turns numbers/booleans/null into JSON types; GitHub then rejects String!
      if [[ $prev == -F && $b =~ ^([0-9]+|true|false|null)$ ]]; then
        echo "gh: Variable \$branch of type String! was provided invalid value" >&2; exit 1
      fi ;;
    number=*) n=${a#number=} ;;
    commits=*) c=$a ;;
  esac
  prev=$a
done
rl() { printf '{"cost": %s, "remaining": %s, "resetAt": "2026-10-16T13:00:00Z"}' "$1" "${GH_FAKE_REMAINING:-4999}"; }
if [[ -n $n ]]; then
  f=$(jq -rn --argjson n "$n" 'inputs | select(any(.data.repository.pullRequests.nodes[]?; .number == $n)) | input_filename' \
        "$GH_FIXTURES"/*.json 2>/dev/null)
  [[ $f != *$'\n'* ]] || { echo "fake gh: #$n is in more than one fixture: $f" >&2; exit 2; }
  b=$(basename "$f" .json) b=${b//__//}
  if [[ -n $b && ${GH_FAKE_FAIL_FOR:-} == "$b" ]]; then echo "gh: boom for $b" >&2; exit 1; fi
  echo "#$n" >> "$GH_FIXTURES/calls"
  echo "#$n ${c:-}" >> "$GH_FIXTURES/commits"
  if [[ -z $f || $n == "${GH_FAKE_GONE:-}" ]]; then
    echo "{\"data\":{\"rateLimit\":$(rl 1),\"repository\":{\"pullRequest\":null}}}"
    echo "gh: Could not resolve to a PullRequest with the number of $n." >&2; exit 1
  fi
  jq --argjson n "$n" --arg b "$b" --argjson rl "$(rl 2)" '.data as $d | {data: {viewer: $d.viewer, rateLimit: $rl,
    repository: {defaultBranchRef: $d.repository.defaultBranchRef,
                 pullRequest: ($d.repository.pullRequests.nodes[] | select(.number == $n) | .headRefName //= $b)}}}' "$f"
  exit 0
fi
[[ -n $b ]] || { echo "fake gh: no branch= or number= field in: $*" >&2; exit 2; }
if [[ ${GH_FAKE_FAIL_FOR:-} == "$b" ]]; then echo "gh: boom for $b" >&2; exit 1; fi
echo "$b" >> "$GH_FIXTURES/calls"
echo "$b${c:+ $c}" >> "$GH_FIXTURES/commits"
f="$GH_FIXTURES/${b//\//__}.json"
if [[ -f $f ]]; then jq --argjson rl "$(rl 1)" '.data.rateLimit = $rl' "$f"; else
  echo "{\"data\":{\"viewer\":{\"login\":\"me\"},\"rateLimit\":$(rl 1),\"repository\":{\"defaultBranchRef\":{\"name\":\"main\"},\"pullRequests\":{\"nodes\":[]}}}}"
fi
EOF
  chmod +x "$TMP/fakebin/gh"
  export PATH="$TMP/fakebin:$PATH"

  git init -q --bare "$TMP/origin.git"
  git clone -q "$TMP/origin.git" "$TMP/work" 2>/dev/null
  git clone -q "$TMP/origin.git" "$TMP/other" 2>/dev/null
  cd "$TMP/work"
  printf 'one\ntwo\nthree\n' > file.txt
  git add file.txt && git commit -qm init && git push -q origin main 2>/dev/null
  git -C "$TMP/other" pull -q 2>/dev/null
}

commit() { # commit <file> <content> [repo]
  local repo=${3:-$TMP/work}
  printf '%s\n' "$2" > "$repo/$1"
  git -C "$repo" add "$1" && git -C "$repo" commit -qm "edit $1"
}

# pr_fixture <branch> [jq-assignments...] — the GraphQL answer for <branch>'s PR.
# Defaults: open, base main, head = that branch's tip, no checks, no threads.
pr_fixture() {
  local branch=$1; shift
  local head; head=$(git rev-parse -q --verify "$branch" || echo 0000)
  local node
  node=$(jq -n --arg head "$head" '{
    number: 7, url: "https://example/pr/7", state: "OPEN", isDraft: false, isCrossRepository: false,
    baseRefName: "main", headRefOid: $head, mergeable: "MERGEABLE", mergedAt: null,
    mergeStateStatus: "CLEAN", reviewDecision: null, reviewRequests: {nodes: []},
    commits: {nodes: [{commit: {oid: $head, statusCheckRollup: null}}]},
    reviewThreads: {nodes: []}
  }')
  local a
  for a; do node=$(jq "$a" <<<"$node"); done
  jq -n --argjson node "$node" \
    '{data: {viewer: {login: "me"}, repository: {defaultBranchRef: {name: "main"}, pullRequests: {nodes: [$node]}}}}' \
    > "$GH_FIXTURES/${branch//\//__}.json"
}

checks() { # checks '<json array of contexts>'
  echo ".commits.nodes[0].commit.statusCheckRollup = {state: \"X\", contexts: {nodes: $1}}"
}

thread() { # thread <first-type> <first-login> <last-type> <last-login> [resolved] [outdated]
  printf '{id: "T-%s-%s", isResolved: %s, isOutdated: %s, path: "file.txt", line: 2,
    first: {nodes: [{author: {__typename: "%s", login: "%s"}}]},
    last: {nodes: [{author: {__typename: "%s", login: "%s"}}]}}' \
    "$2" "$4" "${5:-false}" "${6:-false}" "$1" "$2" "$3" "$4"
}

state() { run "$SD_STATE" --json "$@"; [ "$status" -eq 0 ] || { echo "$output"; return 1; }; }
field() { jq -r "$@" <<<"$output"; }

# ── basics ───────────────────────────────────────────────────────────────────

@test "outside a git repo: fails with a message" {
  cd "$TMP"
  run "$SD_STATE"
  [ "$status" -ne 0 ]
  [[ "$output" == *"not a git repository"* ]]
}

@test "runs under bash 5 via the bash5 shebang" {
  run head -1 "$SD_STATE"
  [ "$output" = "#!/usr/bin/env bash5" ]
}

@test "branch with no PR: pr is null and parent defaults to the default branch" {
  git switch -qc feat
  commit a.txt a
  state
  [ "$(field .branch)" = feat ]
  [ "$(field .pr)" = null ]
  [ "$(field .parent.name)" = main ]
  [ "$(field .parent.source)" = default ]
}

@test "a branch argument is inspected without switching to it" {
  git switch -qc feat
  commit a.txt a
  git switch -q main
  state feat
  [ "$(field .branch)" = feat ]
  [ "$(field .head)" = "$(git rev-parse feat)" ]
  [ "$(git branch --show-current)" = main ]
}

@test "makes exactly one gh call when the parent is the default branch and the PR's number is stored" {
  git switch -qc feat
  pr_fixture feat
  state
  rm "$GH_FIXTURES/calls"
  state
  [ "$(wc -l < "$GH_FIXTURES/calls" | tr -d ' ')" = 1 ]
}

# ── O1 PR lookup ─────────────────────────────────────────────────────────────

@test "PR fields are reported" {
  git switch -qc feat
  commit a.txt a
  pr_fixture feat '.isDraft = true'
  state
  [ "$(field .pr.number)" = 7 ]
  [ "$(field .pr.state)" = OPEN ]
  [ "$(field .pr.draft)" = true ]
  [ "$(field .pr.base)" = main ]
  [ "$(field .pr.mergeable)" = MERGEABLE ]
  [ "$(field .pr.merge_state)" = CLEAN ]
  [ "$(field .pr.review_decision)" = null ]
}

@test "PR merge gate: merge state, review decision and who's requested" {
  git switch -qc feat
  commit a.txt a
  pr_fixture feat '.mergeStateStatus = "BLOCKED" | .reviewDecision = "REVIEW_REQUIRED"' \
    '.reviewRequests.nodes = [{requestedReviewer: {__typename: "User", login: "alice"}},
                              {requestedReviewer: {__typename: "Team", slug: "core"}}]'
  state
  [ "$(field -c '.pr | {merge_state, review_decision, review_requests}')" \
    = '{"merge_state":"BLOCKED","review_decision":"REVIEW_REQUIRED","review_requests":["alice","core"]}' ]
  [ "$(field '[.next[].key] | any(. == "review-wait") and all(. != "ready")')" = true ]
}

@test "an open PR is preferred over a newer closed one for the same branch" {
  git switch -qc feat
  pr_fixture feat
  jq '.data.repository.pullRequests.nodes |= [(.[0] | .number = 9 | .state = "CLOSED"), .[0]]' \
    "$GH_FIXTURES/feat.json" > "$TMP/x" && mv "$TMP/x" "$GH_FIXTURES/feat.json"
  state
  [ "$(field .pr.number)" = 7 ]
}

@test "gh failing: git facts still reported, error recorded, exit 0" {
  git switch -qc feat
  commit a.txt a
  GH_FAKE_FAIL="gh: could not resolve host" state
  [ "$(field .pr)" = null ]
  [ "$(field .push.ahead)" = 1 ]
  [[ "$(field '.errors[0]')" == *"could not resolve host"* ]]
}

@test "a numeric branch name is sent to gh as a string" {
  git switch -qc 1234 && commit a.txt a
  pr_fixture 1234
  state
  [ "$(field .pr.number)" = 7 ]
  [ "$(field '.errors | length')" = 0 ]
}

@test "a branch stacked on another without sheepdog knowing is flagged" {
  git switch -qc par && commit a.txt a
  git switch -qc kid && commit b.txt b
  state kid
  [[ "$(field '.errors | join(" ")')" == *"stacked on par without sheepdog knowing: sd act enroll records it"* ]]
  [ "$(field .parent.name)" = main ]
}

@test "an unrecorded stack with two candidates names both" {
  git switch -qc par && commit a.txt a
  git branch twin
  git switch -qc kid && commit b.txt b
  state kid
  [[ "$(field '.errors | join(" ")')" == *"stacked on par or twin without sheepdog knowing"* ]]
}

@test "an unrecorded stack whose parent moved on: the child can't tell which way, the parent made the fork commit" {
  git switch -qc par && commit a.txt a
  git switch -qc kid && commit b.txt b
  git switch -q par && commit c.txt c
  state kid
  [[ "$(field '.errors | join(" ")')" == *"shares commits with par, and each has its own since, so git can't tell which, if either, was made from the other: git config branch.kid.sd-parent par if kid was, or git config branch.kid.sd-parent main if not"* ]]
  state par
  [ "$(field '.errors | length')" = 0 ]
}

@test "siblings whose parent is gone: flagged without claiming either was made from the other" {
  git switch -qc par && commit a.txt a
  git switch -qc kid && commit b.txt b
  git switch -q par && git switch -qc sib && commit c.txt c
  git branch -qD par
  state kid
  [[ "$(field '.errors | join(" ")')" == *"shares commits with sib, and each has its own since, so git can't tell which, if either, was made from the other"* ]]
}

@test "siblings that each moved on: all named, still without a direction" {
  git switch -qc par && commit a.txt a
  git switch -qc kid && commit b.txt b
  git switch -q par && git switch -qc sib && commit c.txt c
  git switch -q par && git switch -qc sib2 && commit d.txt d
  git branch -qD par
  state kid
  [[ "$(field '.errors | join(" ")')" == *"shares commits with sib or sib2, and each has its own since, so git can't tell which, if either, was made from the other: git config branch.kid.sd-parent <name> if kid was, or git config branch.kid.sd-parent main if not"* ]]
}

@test "an unrecorded stack isn't flagged when gh failed, since a PR may name the parent" {
  git switch -qc par && commit a.txt a
  git switch -qc kid && commit b.txt b
  GH_FAKE_FAIL="gh: down" state kid
  [[ "$(field '.errors | join(" ")')" != *"without sheepdog knowing"* ]]
}

@test "an unrecorded stack's tie lists each name once, pushed or not" {
  git switch -qc par && commit a.txt a
  git branch twin
  git push -q origin par twin 2>/dev/null
  git switch -qc kid && commit b.txt b
  state kid
  [[ "$(field '.errors | join(" ")')" == *"stacked on par or twin without sheepdog knowing: record one with git config branch.kid.sd-parent <name>"* ]]
}

@test "a recorded parent, or a PR, means no unrecorded-stack flag" {
  git switch -qc par && commit a.txt a
  git switch -qc kid && commit b.txt b
  git config branch.kid.sd-parent par
  state kid
  [[ "$(field '.errors | join(" ")')" != *"without sheepdog knowing"* ]]
  git config --unset branch.kid.sd-parent
  pr_fixture kid '.baseRefName = "par"'
  state kid
  [[ "$(field '.errors | join(" ")')" != *"without sheepdog knowing"* ]]
}

@test "a branch made from main alone isn't flagged" {
  git switch -qc par && commit a.txt a
  git switch -q main && git switch -qc solo && commit b.txt b
  state solo
  [ "$(field '.errors | length')" = 0 ]
}

@test "a branch name with a slash finds its PR" {
  git switch -qc chrismo/feat && commit a.txt a
  pr_fixture chrismo/feat
  state
  [ "$(field .pr.number)" = 7 ]
}

@test "a fork's PR with the same branch name is ignored" {
  git switch -qc feat && commit a.txt a
  pr_fixture feat
  jq '.data.repository.pullRequests.nodes |= [(.[0] | .number = 99 | .isCrossRepository = true | .headRepositoryOwner = {login: "stranger"}), .[0]]' \
    "$GH_FIXTURES/feat.json" > "$TMP/x" && mv "$TMP/x" "$GH_FIXTURES/feat.json"
  state
  [ "$(field .pr.number)" = 7 ]
}

@test "a PR from our own fork is kept" {
  git switch -qc feat && commit a.txt a
  pr_fixture feat '.isCrossRepository = true' '.headRepositoryOwner = {login: "me"}'
  state
  [ "$(field .pr.number)" = 7 ]
}

@test "gh failing: GitHub-derived answers are unknown, not clean" {
  git switch -qc feat && commit a.txt a
  GH_FAKE_FAIL="gh: down" state
  [ "$(field .fresh)" = null ]
  [ "$(field .threads.owed_bot)" = null ]
  [ "$(field .threads.owed_human)" = null ]
  [ "$(field .parent.merged)" = null ]
}

@test "parent lookup failing: parent merged is unknown" {
  git switch -qc parent && commit p.txt p && git push -q origin parent 2>/dev/null
  git switch -qc feat && commit c.txt c
  git config branch.feat.sd-parent parent
  GH_FAKE_FAIL_FOR=parent state
  [ "$(field .parent.merged)" = null ]
  [ "$(field .fresh)" = null ]
  [[ "$(field '.errors[0]')" == *"boom for parent"* ]]
}

# ── O1 the PR by its stored number (#122) ────────────────────────────────────

calls() { tr '\n' ' ' < "$GH_FIXTURES/calls"; rm -f "$GH_FIXTURES/calls"; }

@test "PR number: the one found is stored, and the next run asks for it by number alone" {
  git switch -qc feat && commit a.txt a
  pr_fixture feat
  state
  [ "$(calls)" = "feat #7 " ]
  [ "$(git config branch.feat.sd-pr)" = 7 ]
  state
  [ "$(calls)" = "#7 " ]
  [ "$(field .pr.number)" = 7 ]
}

@test "PR number: a stored one is used without discovery" {
  git switch -qc feat && commit a.txt a
  pr_fixture feat
  git config branch.feat.sd-pr 7
  state
  [ "$(calls)" = "#7 " ]
  [ "$(field .pr.state)" = OPEN ]
}

@test "PR number: a stored one whose PR has another head re-discovers and stores the branch's" {
  git switch -qc other && commit o.txt o
  pr_fixture other '.number = 8'
  git switch -qc feat && commit a.txt a
  pr_fixture feat
  git config branch.feat.sd-pr 8
  state
  [ "$(calls)" = "#8 feat #7 " ]
  [ "$(field .pr.number)" = 7 ]
  [ "$(git config branch.feat.sd-pr)" = 7 ]
}

@test "PR number: a stored one GitHub can't resolve re-discovers, with no error" {
  git switch -qc feat && commit a.txt a
  pr_fixture feat
  git config branch.feat.sd-pr 42
  state
  [ "$(calls)" = "#42 feat #7 " ]
  [ "$(field .pr.number)" = 7 ]
  [ "$(field '.errors | length')" = 0 ]
}

@test "PR number: a stored one closed without merging re-discovers, and a newer PR from the branch wins" {
  git switch -qc feat && commit a.txt a
  pr_fixture feat '.number = 9'
  jq '.data.repository.pullRequests.nodes |= . + [(.[0] | .number = 5 | .state = "CLOSED")]' \
    "$GH_FIXTURES/feat.json" > "$TMP/x" && mv "$TMP/x" "$GH_FIXTURES/feat.json"
  git config branch.feat.sd-pr 5
  state
  [ "$(calls)" = "#5 feat #9 " ]
  [ "$(field .pr.number)" = 9 ]
  [ "$(git config branch.feat.sd-pr)" = 9 ]
}

@test "PR number: a stored closed one that discovery still picks is kept, not fetched again" {
  git switch -qc feat && commit a.txt a
  pr_fixture feat '.number = 5' '.state = "CLOSED"'
  git config branch.feat.sd-pr 5
  state
  [ "$(calls)" = "#5 feat " ]
  [ "$(field .pr.number)" = 5 ]
  [ "$(field .pr.state)" = CLOSED ]
  [ "$(git config branch.feat.sd-pr)" = 5 ]
}

@test "PR number: a stored merged one stays, even with a newer open PR from the branch" {
  git switch -qc feat && commit a.txt a
  pr_fixture feat '.number = 9'
  jq '.data.repository.pullRequests.nodes |= . + [(.[0] | .number = 5 | .state = "MERGED")]' \
    "$GH_FIXTURES/feat.json" > "$TMP/x" && mv "$TMP/x" "$GH_FIXTURES/feat.json"
  git config branch.feat.sd-pr 5
  state
  [ "$(calls)" = "#5 " ]
  [ "$(field .pr.state)" = MERGED ]
}

@test "PR number: a stored one dropped when discovery finds no PR for the branch" {
  git switch -qc other && commit o.txt o
  pr_fixture other '.number = 8'
  git switch -qc feat && commit a.txt a
  git config branch.feat.sd-pr 8
  state
  [ "$(calls)" = "#8 feat " ]
  [ "$(field .pr)" = null ]
  [ -z "$(git config branch.feat.sd-pr || true)" ]
}

@test "PR number: no PR yet costs one discovery a run, and stores nothing" {
  git switch -qc feat && commit a.txt a
  state
  [ "$(calls)" = "feat " ]
  state
  [ "$(calls)" = "feat " ]
  [ -z "$(git config branch.feat.sd-pr || true)" ]
}

@test "PR number: the parent's is stored and used the same way" {
  git switch -qc parent && commit p.txt p && git push -q origin parent 2>/dev/null
  git switch -qc feat && commit c.txt c
  git config branch.feat.sd-parent parent
  pr_fixture feat
  pr_fixture parent '.number = 3'
  state
  [ "$(calls)" = "feat #7 parent #3 " ]
  [ "$(git config branch.parent.sd-pr)" = 3 ]
  state
  [ "$(calls)" = "#7 #3 " ]
  [ "$(field .parent.pr.number)" = 3 ]
}

@test "PR number: a PR discovery lists but the fetch can't resolve is an error, and no PR" {
  git switch -qc feat && commit a.txt a
  pr_fixture feat
  GH_FAKE_GONE=7 state
  [ "$(calls)" = "feat #7 " ]
  [ "$(field .pr)" = null ]
  [ "$(field .fresh)" = null ]
  [[ "$(field '.errors[0]')" == *"GitHub has no PR #7, which it just listed"* ]]
}

@test "PR number: a parent that isn't a local branch has its number looked up, not stored" {
  git switch -qc parent && commit p.txt p && git push -q origin parent 2>/dev/null
  git switch -qc feat && commit c.txt c
  git branch -D -q parent
  git config branch.feat.sd-parent parent
  pr_fixture feat
  pr_fixture parent '.number = 3'
  state
  [ "$(field .parent.pr.number)" = 3 ]
  [ -z "$(git config branch.parent.sd-pr || true)" ]
}

@test "PR number: gh failing stores nothing and keeps what was stored" {
  git switch -qc feat && commit a.txt a
  GH_FAKE_FAIL="gh: down" state
  [ -z "$(git config branch.feat.sd-pr || true)" ]
  git config branch.feat.sd-pr 7
  GH_FAKE_FAIL="gh: down" state
  [ "$(git config branch.feat.sd-pr)" = 7 ]
}

# ── rate limit (#122) ────────────────────────────────────────────────────────

@test "rate_limit: the calls' cost summed, with the last call's remaining and reset" {
  git switch -qc feat && commit a.txt a
  pr_fixture feat
  GH_FAKE_REMAINING=1234 state
  [ "$(field -c .rate_limit)" = '{"cost":3,"remaining":1234,"reset_at":"2026-10-16T13:00:00Z","limited":false}' ]
  state
  [ "$(field .rate_limit.cost)" = 2 ]
}

@test "rate_limit: GitHub refusing for the rate limit is limited; another failure isn't" {
  git switch -qc feat && commit a.txt a
  GH_FAKE_FAIL="gh: API rate limit exceeded for user ID 1." state
  [ "$(field -c .rate_limit)" = '{"cost":null,"remaining":null,"reset_at":null,"limited":true}' ]
  GH_FAKE_FAIL="gh: down" state
  [ "$(field .rate_limit.limited)" = false ]
}

@test "summary: errors are flagged" {
  git switch -qc feat && commit a.txt a
  GH_FAKE_FAIL="gh: down" run "$SD_STATE"
  [ "$status" -eq 0 ]
  [[ "$output" == *"⚠ 1 error(s)"* ]]
}

# ── O2 parent ────────────────────────────────────────────────────────────────

@test "parent comes from the PR base when nothing is configured" {
  git switch -qc parent && commit p.txt p && git push -q origin parent 2>/dev/null
  git switch -qc feat && commit c.txt c
  pr_fixture feat '.baseRefName = "parent"'
  state
  [ "$(field .parent.name)" = parent ]
  [ "$(field .parent.source)" = pr ]
}

@test "configured sd-parent wins over the PR base" {
  git switch -qc parent && commit p.txt p && git push -q origin parent 2>/dev/null
  git switch -qc feat && commit c.txt c
  git config branch.feat.sd-parent parent
  pr_fixture feat '.baseRefName = "main"'
  state
  [ "$(field .parent.name)" = parent ]
  [ "$(field .parent.source)" = config ]
}

@test "parent PR state is looked up for a non-default parent" {
  git switch -qc parent && commit p.txt p && git push -q origin parent 2>/dev/null
  git switch -qc feat && commit c.txt c
  git config branch.feat.sd-parent parent
  pr_fixture parent '.number = 3'
  state
  [ "$(field .parent.pr.number)" = 3 ]
  [ "$(field .parent.merged)" = false ]
}

@test "parent closed without merging: not merged, and the state says so" {
  git switch -qc parent && commit p.txt p && git push -q origin parent 2>/dev/null
  git switch -qc feat && commit c.txt c
  git config branch.feat.sd-parent parent
  pr_fixture parent '.number = 3' '.state = "CLOSED"'
  state
  [ "$(field .parent.merged)" = false ]
  [ "$(field .parent.pr.state)" = CLOSED ]
}

@test "parent PR's commits are listed, [] when GitHub gives none" {
  git switch -qc parent && commit p.txt p && git push -q origin parent 2>/dev/null
  git switch -qc feat && commit c.txt c
  git config branch.feat.sd-parent parent
  pr_fixture parent '.allCommits = {nodes: [{commit: {oid: "aaa"}}, {commit: {oid: "bbb"}}]}'
  state
  [ "$(field -c .parent.pr.commits)" = '["aaa","bbb"]' ]
  pr_fixture parent
  state
  [ "$(field -c .parent.pr.commits)" = '[]' ]
}

@test "only the parent lookup asks GitHub for its PR's commits" {
  git switch -qc parent && commit p.txt p && git push -q origin parent 2>/dev/null
  git switch -qc feat && commit c.txt c
  git config branch.feat.sd-parent parent
  pr_fixture feat
  pr_fixture parent '.number = 3'
  state
  [ "$(cat "$GH_FIXTURES/commits")" = "feat
#7 commits=false
parent
#3 commits=true" ]
}

@test "bot: the configured trigger command is reported, null when there is none" {
  git switch -qc feat
  state
  [ "$(field .bot.command)" = null ]
  "$SD_CONFIG" set --global settings.botCommand "/review-bot"
  state
  [ "$(field .bot.command)" = /review-bot ]
}

# ── O3 sync vs parent ────────────────────────────────────────────────────────

@test "sync: names the default branch" {
  git switch -qc feat
  state
  [ "$(field .sync.default)" = main ]
}

@test "sync: up to date with the parent" {
  git switch -qc feat && commit a.txt a
  state
  [ "$(field .sync.target)" = main ]
  [ "$(field .sync.behind)" = 0 ]
  [ "$(field .sync.up_to_date)" = true ]
}

@test "sync: ahead counts the branch's own commits over the target" {
  git switch -qc feat && commit a.txt a && commit b.txt b
  commit c.txt c "$TMP/other" && git -C "$TMP/other" push -q origin main 2>/dev/null
  git fetch -q
  state
  [ "$(field .sync.ahead)" = 2 ]
  [ "$(field .sync.behind)" = 1 ]
}

@test "sync: parent moved, restack would be clean" {
  git switch -qc feat && commit a.txt a
  commit b.txt b "$TMP/other" && git -C "$TMP/other" push -q origin main 2>/dev/null
  git fetch -q
  state
  [ "$(field .sync.behind)" = 1 ]
  [ "$(field .sync.up_to_date)" = false ]
  [ "$(field .sync.conflict)" = false ]
}

@test "sync: parent moved, restack would conflict" {
  git switch -qc feat && commit file.txt "one
OURS
three"
  commit file.txt "one
THEIRS
three" "$TMP/other" && git -C "$TMP/other" push -q origin main 2>/dev/null
  git fetch -q
  state
  [ "$(field .sync.conflict)" = true ]
}

@test "sync: squash-merged parent retargets to main and replays only our change" {
  git switch -qc parent && commit p.txt p && git push -q origin parent 2>/dev/null
  local parent_sha; parent_sha=$(git rev-parse parent)
  git switch -qc feat && commit c.txt c
  git config branch.feat.sd-parent parent
  # the squash merge lands on main elsewhere (own message, or it hashes identical to parent's commit)
  commit p.txt p "$TMP/other" && git -C "$TMP/other" commit -q --amend -m "Squash parent (#3)"
  git -C "$TMP/other" push -q origin main 2>/dev/null
  git fetch -q
  pr_fixture parent '.state = "MERGED"' ".headRefOid = \"$parent_sha\""
  state
  [ "$(field .parent.merged)" = true ]
  [ "$(field .sync.target)" = main ]
  [ "$(field .sync.old_base)" = "$parent_sha" ]
  [ "$(field .sync.up_to_date)" = false ]
  [ "$(field .sync.conflict)" = false ]
}

@test "sync: the base pin is the old base when it is an ancestor" {
  git switch -qc feat && commit a.txt a
  local pin; pin=$(git rev-parse main)
  git update-ref refs/sheepdog/feat/base "$pin"
  state
  [ "$(field .sync.old_base)" = "$pin" ]
  [ "$(field .sync.by_tree)" = false ]
}

@test "sync: rebased onto main outside sheepdog, pin left below it: the merge-base, not main's commits (#120)" {
  git switch -qc feat && commit a.txt a
  git update-ref refs/sheepdog/feat/base main
  commit m.txt m "$TMP/other" && git -C "$TMP/other" push -q origin main 2>/dev/null
  git fetch -q && git rebase -q origin/main
  state
  [ "$(field .sync.up_to_date)" = true ]
  [ "$(field .sync.old_base)" = "$(git rev-parse origin/main)" ]
  [ "$(field '.errors | length')" = 0 ]
}

@test "sync: stacked on an open parent, main moved on: the pin stays the base (#120)" {
  git switch -qc parent && commit p.txt p1 && git push -q origin parent 2>/dev/null
  git switch -qc feat && commit c.txt c
  git config branch.feat.sd-parent parent
  git update-ref refs/sheepdog/feat/base parent
  commit m.txt m "$TMP/other" && git -C "$TMP/other" push -q origin main 2>/dev/null
  git fetch -q
  state
  [ "$(field .sync.target)" = parent ]
  [ "$(field .sync.old_base)" = "$(git rev-parse parent)" ]
}

# lost_pin_stack [nopin|same]: parent (p1, p2) squash-merged into main and its
# branch deleted; feat stacked on it, rewritten outside sheepdog so p2 is a
# copy, not p2: a changed one, or with "same" an exact one. Pins the base at p2
# unless told "nopin". Sets $p1 and $p2.
lost_pin_stack() {
  git switch -qc parent && commit p.txt p1 && commit q.txt p2 && git push -q origin parent 2>/dev/null
  p1=$(git rev-parse parent~1) p2=$(git rev-parse parent)
  git switch -qc feat parent~1
  printf '%s\n' "$([[ ${1:-} == same ]] && echo p2 || echo 'p2 again')" > q.txt
  git add q.txt && git commit -qm "copy of p2"
  commit c.txt c
  git config branch.feat.sd-parent parent
  [[ ${1:-} == nopin ]] || git update-ref refs/sheepdog/feat/base "$p2"
  (cd "$TMP/other" && git fetch -q origin && git merge -q --squash origin/parent >/dev/null \
     && git commit -qm "parent (#3)" && git push -q origin main :parent 2>/dev/null)
  git fetch -q --prune
  pr_fixture parent '.state = "MERGED"' ".headRefOid = \"$p2\""
}

@test "sync: stacked, pin lost, parent merged and deleted: no old base, and says why" {
  lost_pin_stack
  state
  [ "$(field .sync.target)" = main ]
  [ "$(field .sync.old_base)" = null ]
  [ "$(field .sync.conflict)" = null ]
  [[ "$(field '.errors | join(" ")')" == *"base pin ${p2:0:7} is no longer in feat's history (rewritten outside sheepdog?)"* ]]
}

@test "sync: pin lost, but all of the merged parent is in HEAD: the merged head is the base, by tree" {
  lost_pin_stack same
  pr_fixture parent '.state = "MERGED"' ".headRefOid = \"$p2\"" \
    ".allCommits = {nodes: [{commit: {oid: \"$p1\"}}, {commit: {oid: \"$p2\"}}]}"
  state
  [ "$(field .sync.old_base)" = "$p2" ]
  [ "$(field .sync.by_tree)" = true ]
  [ "$(field .sync.conflict)" = false ]
}

# squash_parent_away: parent squash-merged into main elsewhere, its branch deleted.
squash_parent_away() {
  (cd "$TMP/other" && git fetch -q origin && git merge -q --squash origin/parent >/dev/null \
     && git commit -qm "parent (#3)" && git push -q origin main :parent 2>/dev/null)
  git fetch -q --prune
}

@test "sync: pin lost, HEAD still has a line the parent deleted: no tree base" {
  git switch -qc parent && printf 'x\ny\nz\n' > r.txt && git add r.txt && git commit -qm p1
  printf 'x\nz\n' > r.txt && git commit -qam "p2 drops y" && git push -q origin parent 2>/dev/null
  local p2; p2=$(git rev-parse parent)
  git switch -qc feat parent~1 && git commit -q --allow-empty -m "not really p2" && commit c.txt c
  git config branch.feat.sd-parent parent
  git update-ref refs/sheepdog/feat/base "$p2"
  squash_parent_away
  pr_fixture parent '.state = "MERGED"' ".headRefOid = \"$p2\"" \
    ".allCommits = {nodes: [{commit: {oid: \"$(git rev-parse "$p2~1")\"}}, {commit: {oid: \"$p2\"}}]}"
  state
  [ "$(field .sync.by_tree)" = false ]
  [ "$(field .sync.old_base)" = null ]
}

@test "sync: pin lost, HEAD holds the parent as one copy of two edits to a line: by tree" {
  git switch -qc parent && commit s.txt X && commit s.txt Y && git push -q origin parent 2>/dev/null
  local p2; p2=$(git rev-parse parent)
  git switch -qc feat main && commit s.txt Y && commit c.txt c
  git config branch.feat.sd-parent parent
  git update-ref refs/sheepdog/feat/base "$p2"
  squash_parent_away
  pr_fixture parent '.state = "MERGED"' ".headRefOid = \"$p2\""
  state
  [ "$(field .sync.by_tree)" = true ]
  [ "$(field .sync.old_base)" = "$p2" ]
}

@test "sync: pin at an older parent head, HEAD holds the whole merged parent: the merged head is the tree base" {
  git switch -qc parent && commit s.txt X && local p1; p1=$(git rev-parse parent)
  commit t.txt Y && git push -q origin parent 2>/dev/null
  local p2; p2=$(git rev-parse parent)
  git switch -qc feat main
  printf 'X\n' > s.txt && printf 'Y\n' > t.txt && git add s.txt t.txt && git commit -qm "parent, copied" && commit c.txt c
  git config branch.feat.sd-parent parent
  git update-ref refs/sheepdog/feat/base "$p1"
  squash_parent_away
  pr_fixture parent '.state = "MERGED"' ".headRefOid = \"$p2\""
  state
  [ "$(field .sync.by_tree)" = true ]
  [ "$(field .sync.old_base)" = "$p2" ]
}

@test "sync: pin lost, parent moved onto a newer main than HEAD has: no tree base, which would revert main" {
  git switch -qc parent && commit s.txt X && git push -q origin parent 2>/dev/null
  local p1; p1=$(git rev-parse parent)
  git switch -qc feat main
  printf 'X\n' > s.txt && git add s.txt && git commit -qm "copy of p1" && commit c.txt c
  git config branch.feat.sd-parent parent
  git update-ref refs/sheepdog/feat/base "$p1"
  (cd "$TMP/other" && commit m.txt M1 "$TMP/other" && git push -q origin main 2>/dev/null \
     && git fetch -q origin && git switch -qc parent origin/parent && git rebase -q main \
     && git push -qf origin parent 2>/dev/null && git switch -q main)
  git fetch -q
  local p1b; p1b=$(git rev-parse origin/parent)
  squash_parent_away
  pr_fixture parent '.state = "MERGED"' ".headRefOid = \"$p1b\""
  state
  [ "$(field .sync.by_tree)" = false ]
  [ "$(field .sync.old_base)" = null ]
}

@test "sync: pin lost, HEAD on a newer main than the parent was: no tree base, whose restack would roll back" {
  git switch -qc parent && commit s.txt X && git push -q origin parent 2>/dev/null
  local p1; p1=$(git rev-parse parent)
  (cd "$TMP/other" && commit m.txt M1 "$TMP/other" && git push -q origin main 2>/dev/null)
  git fetch -q && git switch -qc feat origin/main
  printf 'X\n' > s.txt && git add s.txt && git commit -qm "copy of p1" && commit c.txt c
  git config branch.feat.sd-parent parent
  git update-ref refs/sheepdog/feat/base "$p1"
  squash_parent_away
  pr_fixture parent '.state = "MERGED"' ".headRefOid = \"$p1\""
  state
  [ "$(field .sync.by_tree)" = false ]
  [ "$(field .sync.old_base)" = null ]
}

@test "sync: pin lost, parent merged with a merge commit: no tree base, whatever HEAD holds" {
  git switch -qc parent && commit p.txt p1 && commit q.txt p2 && git push -q origin parent 2>/dev/null
  local p2; p2=$(git rev-parse parent)
  git switch -qc feat parent~1 && commit q.txt "p2, rewritten" && commit c.txt c
  git config branch.feat.sd-parent parent
  git update-ref refs/sheepdog/feat/base "$p2"
  (cd "$TMP/other" && git fetch -q origin && git merge -q --no-ff --no-edit origin/parent \
     && git push -q origin main :parent 2>/dev/null)
  git fetch -q --prune
  pr_fixture parent '.state = "MERGED"' ".headRefOid = \"$p2\""
  state
  [ "$(field .sync.by_tree)" = false ]
}

@test "sync: pin lost, a parent commit that is in HEAD only changed: no old base" {
  lost_pin_stack
  pr_fixture parent '.state = "MERGED"' ".headRefOid = \"$p2\"" \
    ".allCommits = {nodes: [{commit: {oid: \"$p1\"}}, {commit: {oid: \"$p2\"}}]}"
  state
  [ "$(field .sync.old_base)" = null ]
  [ "$(field .sync.by_tree)" = false ]
}

@test "sync: stacked, no pin, parent merged and deleted: no old base, and says why" {
  lost_pin_stack nopin
  state
  [ "$(field .sync.old_base)" = null ]
  [[ "$(field '.errors | join(" ")')" == *"no base for feat on parent"* ]]
}

@test "sync: stacked, no base, but already on main (rebased by hand): up to date, no error" {
  git switch -qc parent && commit p.txt p1 && git push -q origin parent 2>/dev/null
  local p1; p1=$(git rev-parse parent)
  git switch -qc feat && commit c.txt c
  git config branch.feat.sd-parent parent
  (cd "$TMP/other" && git fetch -q origin && git merge -q --squash origin/parent >/dev/null \
     && git commit -qm "parent (#3)" && git push -q origin main :parent 2>/dev/null)
  git fetch -q --prune
  git rebase -q --onto origin/main parent feat
  pr_fixture parent '.state = "MERGED"' ".headRefOid = \"$p1\""
  state
  [ "$(field .sync.up_to_date)" = true ]
  [ "$(field .sync.old_base)" = "$(git rev-parse origin/main)" ]
  [ "$(field -c .errors)" = '[]' ]
}

@test "sync: stacked, rebased onto main by hand, stale pin: target is the base, pin reported" {
  git switch -qc parent && commit p.txt p1 && git push -q origin parent 2>/dev/null
  local p1; p1=$(git rev-parse parent)
  git switch -qc feat && commit c.txt c
  git config branch.feat.sd-parent parent
  git update-ref refs/sheepdog/feat/base "$(git commit-tree HEAD^{tree} -m elsewhere)"
  (cd "$TMP/other" && git fetch -q origin && git merge -q --squash origin/parent >/dev/null \
     && git commit -qm "parent (#3)" && git push -q origin main :parent 2>/dev/null)
  git fetch -q --prune
  git rebase -q --onto origin/main parent feat
  pr_fixture parent '.state = "MERGED"' ".headRefOid = \"$p1\"" ".allCommits = {nodes: [{commit: {oid: \"$p1\"}}]}"
  state
  [ "$(field .sync.up_to_date)" = true ]
  [ "$(field .sync.old_base)" = "$(git rev-parse origin/main)" ]
  [[ "$(field '.errors | join(" ")')" == *"is no longer in feat's history"* ]]
}

@test "sync: stacked, main merged in rather than rebased: no base, the parent's commits are still there" {
  git switch -qc parent && commit p.txt p1 && git push -q origin parent 2>/dev/null
  local p1; p1=$(git rev-parse parent)
  git switch -qc feat && commit c.txt c
  git config branch.feat.sd-parent parent
  git switch -q parent && commit q.txt p2 && git push -q origin parent 2>/dev/null
  local p2; p2=$(git rev-parse parent)
  git switch -q feat
  (cd "$TMP/other" && git fetch -q origin && git merge -q --squash origin/parent >/dev/null \
     && git commit -qm "parent (#3)" && git push -q origin main :parent 2>/dev/null)
  git fetch -q --prune
  git merge -q --no-edit origin/main
  pr_fixture parent '.state = "MERGED"' ".headRefOid = \"$p2\"" \
    ".allCommits = {nodes: [{commit: {oid: \"$p1\"}}, {commit: {oid: \"$p2\"}}]}"
  state
  [ "$(field .sync.up_to_date)" = true ]
  [ "$(field .sync.old_base)" = null ]
  [[ "$(field '.errors | join(" ")')" == *"no base for feat on parent"* ]]
}

@test "sync: already on main, but a stale pin: still reported" {
  git switch -qc feat && commit a.txt a
  git update-ref refs/sheepdog/feat/base "$(git rev-parse feat)"
  git commit -q --amend -m "rewritten"
  state
  [ "$(field .sync.up_to_date)" = true ]
  [[ "$(field '.errors | join(" ")')" == *"is no longer in feat's history"* ]]
}

# Without a pin or a fork-point, the merge-base with an open parent can lie
# below commits the branch holds from the parent's old head (#97).
@test "sync: stacked on an open parent, no pin or fork-point: no fallback to the merge-base, and says why (#97)" {
  git switch -qc parent && commit p.txt p && git push -q origin parent 2>/dev/null
  git switch -qc feat main && commit c.txt c
  git config branch.feat.sd-parent parent
  state
  [ "$(field .sync.target)" = parent ]
  [ "$(field .sync.up_to_date)" = false ]
  [ "$(field .sync.old_base)" = null ]
  [[ "$(field '.errors | join(" ")')" == *"no base for feat on parent: no base pin, and no fork-point"* ]]
}

@test "sync: open parent, stale pin, no fork-point: only the stale-pin error (#97)" {
  git switch -qc parent && commit p.txt p1 && git push -q origin parent 2>/dev/null
  git switch -qc feat main && commit c.txt c
  git config branch.feat.sd-parent parent
  local pin; pin=$(git rev-parse feat)
  git update-ref refs/sheepdog/feat/base "$pin"
  git commit -q --amend -m "rewritten"
  state
  [ "$(field .sync.old_base)" = null ]
  [ "$(field '.errors | length')" = 1 ]
  [[ "$(field '.errors[0]')" == *"base pin ${pin:0:7} is no longer in feat's history"* ]]
}

@test "sync: open parent amended and force-pushed, no pin, reflog gone: no old base below the parent's commits (#97)" {
  git switch -qc parent && commit p.txt p1 && git push -q origin parent 2>/dev/null
  git switch -qc feat && commit c.txt c
  git config branch.feat.sd-parent parent
  (cd "$TMP/other" && git fetch -q origin && git switch -qc parent origin/parent \
     && git commit -q --amend -m "p1, amended" && git push -qf origin parent 2>/dev/null && git switch -q main)
  git fetch -q
  git reflog expire --expire=now --expire-unreachable=now refs/remotes/origin/parent
  state
  [ "$(field .sync.old_base)" = null ]
  [[ "$(field '.errors | join(" ")')" == *"no base for feat on parent"* ]]
}

@test "sync: on an open parent's tip, no pin, reflog gone: up to date on the parent's tip, no error (#97)" {
  git switch -qc parent && commit p.txt p1 && git push -q origin parent 2>/dev/null
  git switch -qc feat && commit c.txt c
  git config branch.feat.sd-parent parent
  git reflog expire --expire=now --expire-unreachable=now refs/remotes/origin/parent
  state
  [ "$(field .sync.up_to_date)" = true ]
  [ "$(field .sync.old_base)" = "$(git rev-parse origin/parent)" ]
  [ "$(field '.errors | length')" = 0 ]
}

@test "sync: stacked on an open parent, no pin, fork-point in the parent's reflog: the fork-point" {
  git switch -qc parent && commit p.txt p1 && git push -q origin parent 2>/dev/null
  local p1; p1=$(git rev-parse parent)
  git switch -qc feat && commit c.txt c
  git config branch.feat.sd-parent parent
  (cd "$TMP/other" && git fetch -q origin && git switch -qc parent origin/parent \
     && git commit -q --amend -m "p1, amended" && git push -qf origin parent 2>/dev/null && git switch -q main)
  git fetch -q
  state
  [ "$(field .sync.old_base)" = "$p1" ]
  [ "$(field '.errors | length')" = 0 ]
}

@test "sync: a parent closed without merging, no pin or fork-point: the error says the parent closed, NEXT says parent-closed (#97)" {
  git switch -qc parent && commit p.txt p1 && git push -q origin parent 2>/dev/null
  git switch -qc feat && commit c.txt c
  (cd "$TMP/other" && git fetch -q origin && git switch -qc parent origin/parent \
     && git commit -q --allow-empty -m p2 && git push -q origin parent 2>/dev/null && git switch -q main)
  git fetch -q
  git reflog expire --expire=now --expire-unreachable=now refs/remotes/origin/parent
  pr_fixture parent '.number = 3' '.state = "CLOSED"'
  pr_fixture feat '.baseRefName = "parent"'
  state
  [ "$(field .sync.old_base)" = null ]
  [ "$(field '.errors | join(" ")')" = "no base for feat on parent: parent's PR #3 was closed without merging; decide what feat targets" ]
  [[ " $(field '[.next[].key] | join(" ")') " == *" parent-closed "* ]]
}

@test "sync: a parent closed without merging, no PR yet, no pin or fork-point: the error says the parent closed (#97)" {
  git switch -qc parent && commit p.txt p1 && git push -q origin parent 2>/dev/null
  git switch -qc feat && commit c.txt c
  git config branch.feat.sd-parent parent
  (cd "$TMP/other" && git fetch -q origin && git switch -qc parent origin/parent \
     && git commit -q --allow-empty -m p2 && git push -q origin parent 2>/dev/null && git switch -q main)
  git fetch -q
  git reflog expire --expire=now --expire-unreachable=now refs/remotes/origin/parent
  pr_fixture parent '.number = 3' '.state = "CLOSED"'
  state
  [[ "$(field '.errors | join(" ")')" == *"parent's PR #3 was closed without merging"* ]]
}

@test "sync: not stacked, pin lost: falls back to the merge-base, and reports the pin" {
  git switch -qc feat && commit a.txt a
  local fork; fork=$(git rev-parse main)
  local pin; pin=$(git rev-parse feat)
  git update-ref refs/sheepdog/feat/base "$pin"
  git commit -q --amend -m "rewritten"
  state
  [ "$(field .sync.old_base)" = "$fork" ]
  [[ "$(field '.errors | join(" ")')" == *"base pin ${pin:0:7} is no longer in feat's history"* ]]
}

# A restack, then git's own advice after a rejected plain push: pull --rebase
# onto the old remote head, which replays main's commit as a copy (#47).
@test "sync: a restack undone by a plain push is reported as undone, with the copied commits counted (#47)" {
  git switch -qc feat && commit a.txt a && git push -q -u origin feat 2>/dev/null
  local pre; pre=$(git rev-parse feat)
  commit m.txt m "$TMP/other" && git -C "$TMP/other" push -q origin main 2>/dev/null
  git fetch -q
  git rebase -q --onto origin/main main feat
  git update-ref refs/sheepdog/feat/base origin/main
  git reset -q --hard "$pre" && git cherry-pick origin/main >/dev/null
  state
  [ "$(field .sync.undone)" = 1 ]
  [ "$(field '.errors | length')" = 0 ]
  [ "$(field '.next[0].key')" = restack-undone ]
  local cmd; cmd=$(field '.next[0].cmd')
  [[ $cmd == "git rebase --onto origin/main "*" && sd act push" ]] || false
  # Running the suggested rebase puts the branch back on the target without the copy.
  eval "${cmd%% && *}" >/dev/null 2>&1
  [ "$(git log --format=%s origin/main..feat)" = "edit a.txt" ]
  state
  [ "$(field .sync.undone)" = 0 ]
  [ "$(field .sync.up_to_date)" = true ]
}

# A child rewritten by hand (a reword of the parent's commits it carries keeps
# their author dates and subjects): copies of the parent's commits, but the
# branch's own remote head is gone from its history, so not an undone restack.
@test "sync: a hand rewrite that keeps the parent's commits' dates and subjects is not an undone restack (#47)" {
  git switch -qc parent && commit p.txt p1 && commit q.txt p2 && git push -q origin parent 2>/dev/null
  local p1 p2; p1=$(git rev-parse parent~1) p2=$(git rev-parse parent)
  git switch -qc feat && commit c.txt c && git push -q -u origin feat 2>/dev/null
  git config branch.feat.sd-parent parent
  git update-ref refs/sheepdog/feat/base "$p2"
  local n1 n2 n3
  n1=$(GIT_AUTHOR_DATE=$(git log -1 --format=%ad "$p1") GIT_COMMITTER_DATE="2030-01-01T00:00:00" git commit-tree "$p1^{tree}" -p "$p1^" -m "edit p.txt")
  n2=$(GIT_AUTHOR_DATE=$(git log -1 --format=%ad "$p2") GIT_COMMITTER_DATE="2030-01-01T00:00:01" git commit-tree "$p2^{tree}" -p "$n1" -m "edit q.txt")
  n3=$(git commit-tree "feat^{tree}" -p "$n2" -m "edit c.txt")
  git reset -q --hard "$n3"
  state
  [ "$(field .sync.undone)" = 0 ]
  [[ "$(field '.errors | join(" ")')" == *"base pin ${p2:0:7} is no longer in feat's history"* ]]
  [ "$(field '.next[0].key')" = errors ]
}

@test "parent: a parent only here is reported as such, with no error, and NEXT ships it first (#105)" {
  git switch -qc parent && commit p.txt p
  git switch -qc feat && commit c.txt c
  git config branch.feat.sd-parent parent
  state
  [ "$(field .parent.local)" = true ]
  [ "$(field .parent.on_remote)" = false ]
  [ "$(field .parent.unpushed)" = true ]
  [ "$(field '.errors | length')" = 0 ]
  [ "$(field '.next[0].key')" = ship-parent ]
  [ "$(field '.next[0].cmd')" = "sd act ship parent" ]
  git push -q origin parent 2>/dev/null && git fetch -q
  state
  [ "$(field .parent.on_remote)" = true ]
  [ "$(field .parent.unpushed)" = false ]
  [ "$(field '.next[0].key')" != ship-parent ]
}

@test "parent: a parent closed without merging, its remote branch deleted, still errors (#105)" {
  git switch -qc parent && commit p.txt p && git push -q origin parent 2>/dev/null
  git switch -qc feat && commit c.txt c
  git config branch.feat.sd-parent parent
  pr_fixture parent '.state = "CLOSED"'
  git push -q origin :parent 2>/dev/null && git fetch -q --prune
  state
  [ "$(field .parent.on_remote)" = false ]
  [[ "$(field '.errors | join(" ")')" == *"no origin/parent ref"* ]]
  [ "$(field '.next[0].key')" = errors ]
}

@test "parent: an open parent PR whose ref isn't fetched is an error, not an unpushed parent (#105)" {
  git switch -qc parent && commit p.txt p
  git switch -qc feat && commit c.txt c
  git config branch.feat.sd-parent parent
  pr_fixture parent
  state
  [ "$(field .parent.on_remote)" = false ]
  [ "$(field .parent.unpushed)" = false ]
  [[ "$(field '.errors | join(" ")')" == *"no origin/parent ref"* ]]
  [ "$(field '.next[0].key')" = errors ]
}

@test "parent: a parent only here whose PR lookup fails keeps the error, not ship-parent (#105)" {
  git switch -qc parent && commit p.txt p
  git switch -qc feat && commit c.txt c
  git config branch.feat.sd-parent parent
  GH_FAKE_FAIL_FOR=parent state
  [ "$(field .parent.unpushed)" = false ]
  [[ "$(field '.errors | join(" ")')" == *"no origin/parent ref"* ]]
  [ "$(field '[.next[].key] | index("ship-parent")')" = null ]
}

@test "parent: a parent only here that already landed on main by hand is not unpushed (#105)" {
  git switch -qc parent && commit p.txt p
  git switch -q main && git merge -q --ff-only parent && git push -q origin main 2>/dev/null
  git switch -qc feat parent && commit c.txt c
  git config branch.feat.sd-parent parent
  state
  [ "$(field .parent.on_remote)" = false ]
  [ "$(field .parent.unpushed)" = false ]
  [ "$(field '[.next[].key] | index("ship-parent")')" = null ]
  [[ "$(field '.errors | join(" ")')" == *"parent has no commits beyond origin/main and isn't on origin"* ]]
}

@test "parent: a parent that exists nowhere is still an error" {
  git switch -qc feat && commit c.txt c
  git config branch.feat.sd-parent nosuch
  state
  [ "$(field .parent.local)" = false ]
  [[ "$(field '.errors | join(" ")')" == *"no origin/nosuch ref"* ]]
}

@test "sync: no shared history with the parent is reported, not guessed" {
  git switch -q --orphan feat && commit a.txt a
  state
  [ "$(field .sync.conflict)" = null ]
  [[ "$(field '.errors | join(" ")')" == *"no merge base"* ]]
}

# ── O4 local vs remote ───────────────────────────────────────────────────────

@test "push: unpushed branch has no remote" {
  git switch -qc feat && commit a.txt a && commit b.txt b
  state
  [ "$(field .push.remote_sha)" = null ]
  [ "$(field .push.ahead)" = 2 ]
}

@test "push: commits ahead of the pushed remote" {
  git switch -qc feat && commit a.txt a && git push -q -u origin feat 2>/dev/null
  commit b.txt b
  state
  [ "$(field .push.ahead)" = 1 ]
  [ "$(field .push.behind)" = 0 ]
  [ "$(field .push.diverged)" = false ]
}

@test "push: remote rewritten locally (amend after push) is ours" {
  git switch -qc feat && commit a.txt a && git push -q -u origin feat 2>/dev/null
  printf 'a2\n' > a.txt && git commit -qa --amend --no-edit
  state
  [ "$(field .push.diverged)" = true ]
  [ "$(field .push.remote_ours)" = true ]
}

@test "push: someone else's commit on the remote is not ours" {
  git switch -qc feat && commit a.txt a && git push -q -u origin feat 2>/dev/null
  git -C "$TMP/other" fetch -q && git -C "$TMP/other" switch -q feat
  commit x.txt x "$TMP/other" && git -C "$TMP/other" push -q origin feat 2>/dev/null
  git fetch -q
  state
  [ "$(field .push.behind)" = 1 ]
  [ "$(field .push.remote_ours)" = false ]
  [ "$(field .push.theirs)" = 1 ]
}

@test "push: our pre-amend commit under someone else's isn't counted as theirs" {
  git switch -qc feat && commit a.txt a && git push -q -u origin feat 2>/dev/null
  git -C "$TMP/other" fetch -q && git -C "$TMP/other" switch -q feat
  commit x.txt x "$TMP/other" && git -C "$TMP/other" push -q origin feat 2>/dev/null
  printf 'a2\n' > a.txt && git commit -qa --amend --no-edit
  git fetch -q
  state
  [ "$(field .push.behind)" = 2 ]
  [ "$(field .push.remote_ours)" = false ]
  [ "$(field .push.theirs)" = 1 ]
}

@test "push: theirs is null when the remote head is ours" {
  git switch -qc feat && commit a.txt a && git push -q -u origin feat 2>/dev/null
  printf 'a2\n' > a.txt && git commit -qa --amend --no-edit
  state
  [ "$(field .push.theirs)" = null ]
}

# ── freshness: GitHub's SHAs vs the local remote-tracking refs ──────────────

@test "fresh: remote-tracking refs match GitHub" {
  git switch -qc feat && commit a.txt a && git push -q -u origin feat 2>/dev/null
  pr_fixture feat
  state
  [ "$(field .fresh)" = true ]
}

@test "fresh: PR head moved on GitHub since the last fetch" {
  git switch -qc feat && commit a.txt a && git push -q -u origin feat 2>/dev/null
  git -C "$TMP/other" fetch -q && git -C "$TMP/other" switch -q feat
  commit x.txt x "$TMP/other" && git -C "$TMP/other" push -q origin feat 2>/dev/null
  pr_fixture feat ".headRefOid = \"$(git -C "$TMP/other" rev-parse HEAD)\""
  state
  [ "$(field .fresh)" = false ]
  [[ "$(field '.stale[0]')" == *"origin/feat"* ]]
}

@test "fresh: parent PR head moved on GitHub since the last fetch" {
  git switch -qc parent && commit p.txt p && git push -q origin parent 2>/dev/null
  git switch -qc feat && commit c.txt c
  git config branch.feat.sd-parent parent
  pr_fixture parent '.headRefOid = "1111111111111111111111111111111111111111"'
  state
  [ "$(field .fresh)" = false ]
  [[ "$(field '.stale[0]')" == *"origin/parent"* ]]
}

@test "summary: stale refs say to fetch" {
  git switch -qc feat && commit a.txt a && git push -q -u origin feat 2>/dev/null
  pr_fixture feat '.headRefOid = "1111111111111111111111111111111111111111"'
  run "$SD_STATE"
  [[ "$output" == *"stale: fetch"* ]]
}

# ── O5 CI ────────────────────────────────────────────────────────────────────

@test "ci: no checks reported" {
  git switch -qc feat && commit a.txt a
  pr_fixture feat
  state
  [ "$(field .ci.verdict)" = none ]
}

@test "ci: all success, skipped or neutral is green" {
  git switch -qc feat && commit a.txt a
  pr_fixture feat "$(checks '[{__typename:"CheckRun",name:"a",status:"COMPLETED",conclusion:"SUCCESS"},
    {__typename:"CheckRun",name:"b",status:"COMPLETED",conclusion:"SKIPPED"},
    {__typename:"StatusContext",context:"c",state:"SUCCESS"}]')"
  state
  [ "$(field .ci.verdict)" = green ]
  [ "$(field .ci.for_head)" = true ]
}

@test "ci: any failure is red, and names the failing checks" {
  git switch -qc feat && commit a.txt a
  pr_fixture feat "$(checks '[{__typename:"CheckRun",name:"lint",status:"COMPLETED",conclusion:"FAILURE"},
    {__typename:"StatusContext",context:"ext",state:"ERROR"},
    {__typename:"CheckRun",name:"test",status:"IN_PROGRESS",conclusion:null}]')"
  state
  [ "$(field .ci.verdict)" = red ]
  [ "$(field '.ci.failing | join(",")')" = "lint,ext" ]
}

@test "ci: unfinished checks are pending" {
  git switch -qc feat && commit a.txt a
  pr_fixture feat "$(checks '[{__typename:"CheckRun",name:"a",status:"COMPLETED",conclusion:"SUCCESS"},
    {__typename:"CheckRun",name:"test",status:"QUEUED",conclusion:null},
    {__typename:"StatusContext",context:"ext",state:"PENDING"}]')"
  state
  [ "$(field .ci.verdict)" = pending ]
  [ "$(field '.ci.pending | join(",")')" = "test,ext" ]
}

@test "ci: cancelled is inconclusive, not red" {
  git switch -qc feat && commit a.txt a
  pr_fixture feat "$(checks '[{__typename:"CheckRun",name:"a",status:"COMPLETED",conclusion:"SUCCESS"},
    {__typename:"CheckRun",name:"b",status:"COMPLETED",conclusion:"CANCELLED"}]')"
  state
  [ "$(field .ci.verdict)" = inconclusive ]
}

@test "ci: a check that didn't start or wants approval is blocked, not red" {
  git switch -qc feat && commit a.txt a && git push -q -u origin feat 2>/dev/null
  pr_fixture feat "$(checks '[{__typename:"CheckRun",name:"a",status:"COMPLETED",conclusion:"SUCCESS"},
    {__typename:"CheckRun",name:"wf",status:"COMPLETED",conclusion:"STARTUP_FAILURE"},
    {__typename:"CheckRun",name:"ok?",status:"COMPLETED",conclusion:"ACTION_REQUIRED"}]')"
  state
  [ "$(field .ci.verdict)" = blocked ]
  [ "$(field '.ci.blocked | join(",")')" = "wf,ok?" ]
  [ "$(field '.ci.failing | length')" = 0 ]
  [ "$(field '.next[0].key')" = ci-blocked ]
}

@test "ci: a failure outranks a blocked check" {
  git switch -qc feat && commit a.txt a
  pr_fixture feat "$(checks '[{__typename:"CheckRun",name:"lint",status:"COMPLETED",conclusion:"FAILURE"},
    {__typename:"CheckRun",name:"wf",status:"COMPLETED",conclusion:"STARTUP_FAILURE"}]')"
  state
  [ "$(field .ci.verdict)" = red ]
}

# Required checks (#123): read for a draft only, since undraft is the one
# decision GitHub's merge state doesn't already gate on them.
required_rules() { # required_rules <context>...: a ruleset requiring them
  jq -n '[{type: "deletion"}, {type: "required_status_checks", parameters: {required_status_checks:
    [$ARGS.positional[] | {context: ., integration_id: 15368}], strict_required_status_checks_policy: false}}]' \
    --args "$@" > "$GH_FIXTURES/rules.json"
}
# reviewed_draft: feat pushed, its local review covering HEAD, and its PR a
# draft, so CI is all that decides whether NEXT has the draft step.
reviewed_draft() {
  git switch -qc feat && commit a.txt a && git push -q -u origin feat 2>/dev/null
  git update-ref refs/sheepdog/feat/local HEAD
  pr_fixture feat '.isDraft = true' "$@"
}
keys() { field '[.next[].key] | join(" ")'; }

@test "ci: a draft's required check with no context on the head is pending, not none" {
  reviewed_draft
  required_rules test
  state
  [ "$(field .ci.verdict)" = pending ]
  [ "$(field -c .ci.required)" = '["test"]' ]
  [ "$(field -c .ci.missing)" = '["test"]' ]
  [ "$(keys)" = ci-wait ]
  [ "$(field '.next[0].say')" = "waiting on required check test to report; if it never runs on a draft, undraft by hand (gh pr ready)" ]
}

@test "ci: a draft's required check missing beside green ones is pending" {
  git switch -qc feat && commit a.txt a
  pr_fixture feat '.isDraft = true' "$(checks '[{__typename:"CheckRun",name:"lint",status:"COMPLETED",conclusion:"SUCCESS"}]')"
  echo '{"name": "main", "protected": true, "protection": {"enabled": true, "required_status_checks":
    {"enforcement_level": "non_admins", "contexts": ["lint", "ext"], "checks": [{"context": "lint", "app_id": 1}, {"context": "ext", "app_id": null}]}}}' \
    > "$GH_FIXTURES/branch.json"
  state
  [ "$(field .ci.verdict)" = pending ]
  [ "$(field -c .ci.required)" = '["ext","lint"]' ]
  [ "$(field -c .ci.missing)" = '["ext"]' ]
}

@test "ci: a draft's required checks all reported is green" {
  git switch -qc feat && commit a.txt a
  pr_fixture feat '.isDraft = true' "$(checks '[{__typename:"CheckRun",name:"test",status:"COMPLETED",conclusion:"SUCCESS"},
    {__typename:"StatusContext",context:"ext",state:"SUCCESS"}]')"
  required_rules test ext
  state
  [ "$(field .ci.verdict)" = green ]
  [ "$(field -c .ci.missing)" = '[]' ]
}

@test "ci: a draft with no required checks and nothing reported is still none, and undrafts" {
  reviewed_draft
  state
  [ "$(field .ci.verdict)" = none ]
  [ "$(field -c .ci.required)" = '[]' ]
  [ "$(field -c '[.errors[]]')" = '[]' ]
  [ "$(sort "$GH_FIXTURES/rest" | tr '\n' ' ')" = "repos/{owner}/{repo}/branches/main repos/{owner}/{repo}/rules/branches/main?per_page=100 " ]
  [ "$(keys)" = draft ]
  [ "$(field '.next[0].say')" = "reviewed, no CI checks: mark the PR ready for review" ]
}

@test "ci: a 404 or 403 from either source is no rules from it, not an error" {
  reviewed_draft
  required_rules test
  GH_FAKE_FAIL_REST="gh: Not Found (HTTP 404)" GH_FAKE_FAIL_REST_ON='*/rules/*' state
  [ "$(field -c '[.errors[]]')" = '[]' ]
  [ "$(field -c .ci.required)" = '[]' ]
  [ "$(keys)" = draft ]
  echo '{"name": "main", "protected": true, "protection": {"required_status_checks": {"contexts": ["test"], "checks": []}}}' \
    > "$GH_FIXTURES/branch.json"
  GH_FAKE_FAIL_REST="gh: Resource not accessible by integration (HTTP 403)" GH_FAKE_FAIL_REST_ON='*/rules/*' state
  [ "$(field -c .ci.required)" = '["test"]' ]
  [ "$(keys)" = ci-wait ]
  GH_FAKE_FAIL_REST="gh: Must have admin rights to Repository. (HTTP 403)" GH_FAKE_FAIL_REST_ON='*/branches/main' state
  [ "$(field -c '[.errors[]]')" = '[]' ]
  [ "$(field -c .ci.required)" = '["test"]' ]
}

@test "ci: a 403 for a rate limit is still an error" {
  reviewed_draft
  GH_FAKE_FAIL_REST="gh: API rate limit exceeded for user ID 1. (HTTP 403)" state
  [ "$(field .ci.required)" = null ]
  [[ "$(field '.errors | join(" ")')" == *"required checks on main"*"rate limit"* ]]
}

@test "ci: required checks aren't read for a PR that isn't a draft" {
  git switch -qc feat && commit a.txt a
  pr_fixture feat
  required_rules test
  state
  [ "$(field .ci.verdict)" = none ]
  [ "$(field .ci.required)" = null ]
  [ ! -e "$GH_FIXTURES/rest" ]
}

@test "ci: reading required checks failing is an error, and no draft step until it's known" {
  reviewed_draft
  GH_FAKE_FAIL_REST="gh: Server Error (HTTP 502)" state
  [ "$(field .ci.required)" = null ]
  [[ "$(field '.errors | join(" ")')" == *"required checks on main"*"HTTP 502"* ]]
  [ "$(keys)" = errors ]
}

@test "ci: checks on a SHA other than local HEAD are flagged" {
  git switch -qc feat && commit a.txt a
  pr_fixture feat "$(checks '[{__typename:"CheckRun",name:"a",status:"COMPLETED",conclusion:"SUCCESS"}]')"
  commit b.txt b
  state
  [ "$(field .ci.for_head)" = false ]
}

# ── O6 review pins ───────────────────────────────────────────────────────────

@test "review: no pins" {
  git switch -qc feat && commit a.txt a
  state
  [ "$(field .review.local.status)" = none ]
  [ "$(field .review.bot.status)" = none ]
}

@test "review: no findings record is null" {
  git switch -qc feat && commit a.txt a
  state
  [ "$(field .review.findings)" = null ]
}

@test "review: a findings record at HEAD, with its round, count and the cap" {
  git switch -qc feat && commit a.txt a
  rec=$(jq -nc --arg h "$(git rev-parse HEAD)" '{head: $h, base: $h, round: 2,
          findings: [{file: "a.txt", line: 1, severity: "error", body: "x"}]}')
  git update-ref refs/sheepdog/feat/findings "$(git hash-object -w --stdin <<<"$rec")"
  state
  [ "$(field .review.findings.round)" = 2 ]
  [ "$(field .review.findings.n)" = 1 ]
  [ "$(field .review.findings.at_head)" = true ]
  [ "$(field .review.findings.declined)" = false ]
  [ "$(field .review.findings.cap)" = 3 ]
  [ "$(field '.review.findings.items[0].body')" = x ]
  run "$SD_STATE"
  [[ "$output" == *"review: 1 finding (round 2/3)"* ]]
  run "$SD_STATE" --long
  [[ "$output" == *"error  a.txt:1  x"* ]]
  rec=$(git cat-file blob refs/sheepdog/feat/findings | jq -c '.findings[0] += {action: "declined", reply: "x is fine"}')
  git update-ref refs/sheepdog/feat/findings "$(git hash-object -w --stdin <<<"$rec")"
  run "$SD_STATE" --long
  [[ "$output" == *"error  a.txt:1  x"*"declined: x is fine"* ]]
  run "$SD_STATE"
  [[ "$output" == *"review: 1 finding (round 2/3, declined)"* ]]
  commit b.txt b
  state
  [ "$(field .review.findings.at_head)" = false ]
}

@test "review: findings are declined only when the agent declined every one that blocks" {
  git switch -qc feat && commit a.txt a
  rec=$(jq -nc --arg h "$(git rev-parse HEAD)" '{head: $h, base: $h, round: 1, findings: [
          {file: "a.txt", line: 1, severity: "error", body: "x", action: "declined", reply: "x is fine"},
          {file: "a.txt", line: 2, severity: "warning", body: "y"},
          {file: "a.txt", line: 3, severity: "nit", body: "z"}]}')
  git update-ref refs/sheepdog/feat/findings "$(git hash-object -w --stdin <<<"$rec")"
  state
  [ "$(field .review.findings.declined)" = false ]
  rec=$(git cat-file blob refs/sheepdog/feat/findings | jq -c '.findings[1] += {action: "declined", reply: "y too"}')
  git update-ref refs/sheepdog/feat/findings "$(git hash-object -w --stdin <<<"$rec")"
  state
  [ "$(field .review.findings.declined)" = true ]
}

@test "review: findings count only what blocks; a record missing fields is an error" {
  git switch -qc feat && commit a.txt a
  rec=$(jq -nc --arg h "$(git rev-parse HEAD)" '{head: $h, base: $h, round: 1, findings: [
          {file: "a.txt", line: 1, severity: "warning", body: "x"}, {file: "a.txt", line: 2, severity: "nit", body: "y"}]}')
  git update-ref refs/sheepdog/feat/findings "$(git hash-object -w --stdin <<<"$rec")"
  state
  [ "$(field .review.findings.n)" = 1 ]
  git update-ref refs/sheepdog/feat/findings "$(git hash-object -w --stdin <<<'{"head": "x"}')"
  state
  [ "$(field .review.findings)" = null ]
  [[ "$(field '.errors | join(" ")')" == *"isn't a findings record"* ]]
}

@test "review: pin at HEAD is covered" {
  git switch -qc feat && commit a.txt a
  git update-ref refs/sheepdog/feat/local HEAD
  state
  [ "$(field .review.local.status)" = covered ]
}

@test "review: pin behind HEAD leaves commits uncovered" {
  git switch -qc feat && commit a.txt a
  git update-ref refs/sheepdog/feat/bot HEAD
  commit b.txt b && commit c.txt c
  state
  [ "$(field .review.bot.status)" = uncovered ]
  [ "$(field .review.bot.uncovered)" = 2 ]
}

@test "review: a rebase that keeps the change still counts as covered" {
  git switch -qc feat && commit a.txt a
  git update-ref refs/sheepdog/feat/local HEAD
  commit b.txt b "$TMP/other" && git -C "$TMP/other" push -q origin main 2>/dev/null
  git fetch -q && git rebase -q origin/main
  state
  [ "$(field .review.local.status)" = rewritten-same ]
}

@test "review: findings stay at HEAD across a rebase that keeps the change" {
  git switch -qc feat && commit a.txt a
  rec=$(jq -nc --arg h "$(git rev-parse HEAD)" --arg b "$(git rev-parse main)" '{head: $h, base: $b, round: 3,
          findings: [{file: "a.txt", line: 1, severity: "error", body: "x"}]}')
  git update-ref refs/sheepdog/feat/findings "$(git hash-object -w --stdin <<<"$rec")"
  commit b.txt b "$TMP/other" && git -C "$TMP/other" push -q origin main 2>/dev/null
  git fetch -q && git rebase -q origin/main
  state
  [ "$(field .review.findings.at_head)" = true ]
  commit c.txt c
  state
  [ "$(field .review.findings.at_head)" = false ]
}

@test "review: --long lists a finding missing fields as itself" {
  git switch -qc feat && commit a.txt a
  rec=$(jq -nc --arg h "$(git rev-parse HEAD)" '{head: $h, base: $h, round: 1, findings: [{file: "a.txt", body: "x"}]}')
  git update-ref refs/sheepdog/feat/findings "$(git hash-object -w --stdin <<<"$rec")"
  run "$SD_STATE" --long
  [[ "$output" == *"r  ?  a.txt:?  x"* ]]
}

@test "review: a findings record whose base is gone is just not at HEAD" {
  git switch -qc feat && commit a.txt a
  rec=$(jq -nc --arg h "$(git rev-parse HEAD)" '{head: $h, base: "0123456789abcdef0123456789abcdef01234567", round: 1,
          findings: [{file: "a.txt", line: 1, severity: "error", body: "x"}]}')
  git update-ref refs/sheepdog/feat/findings "$(git hash-object -w --stdin <<<"$rec")"
  commit b.txt b
  state
  [ "$(field .review.findings.at_head)" = false ]
}

@test "review: a pin's recorded base keeps it covered across a restack past a squash-merged parent" {
  git switch -qc p && commit p.txt p
  git switch -qc feat && commit a.txt a
  git update-ref refs/sheepdog/feat/local HEAD
  git update-ref refs/sheepdog/feat/local-base p
  # p squash-merged: different content, else the squash commit can get p's own SHA (same tree, same second)
  commit p.txt "p, squashed" "$TMP/other" && git -C "$TMP/other" push -q origin main 2>/dev/null
  git fetch -q && git rebase -q --onto origin/main p feat
  state
  [ "$(field .review.local.status)" = rewritten-same ]
}

@test "review: without a recorded base, the same restack looks like a changed change (why -base exists)" {
  git switch -qc p && commit p.txt p
  git switch -qc feat && commit a.txt a
  git update-ref refs/sheepdog/feat/local HEAD
  commit p.txt "p, squashed" "$TMP/other" && git -C "$TMP/other" push -q origin main 2>/dev/null
  git fetch -q && git rebase -q --onto origin/main p feat
  state
  [ "$(field .review.local.status)" = rewritten-changed ]
}

@test "review: a rewrite that changes the change is not covered" {
  git switch -qc feat && commit a.txt a
  git update-ref refs/sheepdog/feat/local HEAD
  printf 'different\n' > a.txt && git commit -qa --amend --no-edit
  state
  [ "$(field .review.local.status)" = rewritten-changed ]
}

@test "review: a rewritten pin that can't be compared is unknown, not covered" {
  git switch -q --orphan feat && commit a.txt a
  git update-ref refs/sheepdog/feat/local HEAD
  printf 'different\n' > a.txt && git commit -qa --amend --no-edit
  state
  [ "$(field .review.local.status)" = unknown ]
}

@test "review: stacked on an open parent with no base: a rewritten pin is unknown, not compared from the merge-base (#120)" {
  git switch -qc parent && commit p.txt p1 && git push -q origin parent 2>/dev/null
  git switch -qc feat main && commit c.txt c
  git config branch.feat.sd-parent parent
  git update-ref refs/sheepdog/feat/local HEAD
  git commit -q --amend -m "reworded"
  state
  [ "$(field .sync.old_base)" = null ]
  [ "$(field .review.local.status)" = unknown ]
}

@test "review: stacked on an open parent, base from the fork-point: a rewritten pin with no recorded base is unknown (#120)" {
  git switch -qc parent && commit p.txt p1 && git push -q origin parent 2>/dev/null
  local p1; p1=$(git rev-parse parent)
  git switch -qc feat && commit c.txt c
  git config branch.feat.sd-parent parent
  git update-ref refs/sheepdog/feat/local HEAD
  (cd "$TMP/other" && git fetch -q origin && git switch -qc parent origin/parent \
     && git commit -q --amend -m "p1, amended" && git push -qf origin parent 2>/dev/null && git switch -q main)
  git fetch -q
  git commit -q --amend -m "reworded"
  state
  [ "$(field .sync.old_base)" = "$p1" ]
  [ "$(field .review.local.status)" = unknown ]
}

@test "review: stacked on a merged parent with no base: a rewritten pin is unknown, not compared from the merge-base (#120)" {
  lost_pin_stack nopin
  git update-ref refs/sheepdog/feat/local HEAD
  git commit -q --amend -m "reworded"
  state
  [ "$(field .sync.target)" = main ]
  [ "$(field .sync.old_base)" = null ]
  [ "$(field .review.local.status)" = unknown ]
}

# ── O7 threads owed a reply ──────────────────────────────────────────────────

@test "threads: unresolved and last word not ours are owed, split bot vs human" {
  git switch -qc feat && commit a.txt a
  "$SD_CONFIG" set --global --json settings.botLogin '["marge"]'
  pr_fixture feat ".reviewThreads.nodes = [
    $(thread Bot copilot Bot copilot),
    $(thread User marge User marge),
    $(thread User alice User alice),
    $(thread User bob User me),
    $(thread User carol User carol true),
    $(thread Bot copilot Bot copilot false true)]"
  state
  [ "$(field .threads.owed_bot)" = 3 ]
  [ "$(field .threads.owed_human)" = 1 ]
  [ "$(field .threads.outdated)" = 1 ]
  [ "$(field '.threads.items | length')" = 4 ]
}

note() { # note <id> <type> <login> [body] [createdAt] — a review or top-level comment node
  printf '{id: "%s", body: "%s", createdAt: "%s", author: {__typename: "%s", login: "%s"}}' "$1" "${4-findings}" "${5-2026-10-16T00:00:00Z}" "$2" "$3"
}

# ── O6, the bot's reply since the trigger (#63) ─────────────────────────────

trigger_pin() { # bot pinned at HEAD, triggered at <time>
  git update-ref refs/sheepdog/feat/bot HEAD
  git config branch.feat.sd-bot-triggered "$1"
}

@test "review.bot: triggered and no bot word since: replied false, waited counts" {
  git switch -qc feat && commit a.txt a
  trigger_pin 2026-10-15T12:00:00Z
  pr_fixture feat ".reviews.nodes = [$(note R0 Bot copilot earlier 2026-10-15T11:59:59Z)]"
  state
  [ "$(field .review.bot.status)" = covered ]
  [ "$(field .review.bot.replied)" = false ]
  [ "$(field .review.bot.triggered)" = 2026-10-15T12:00:00Z ]
  w=$(field .review.bot.waited)   # seconds since a past trigger: not 0, not nanoseconds
  (( w >= 3600 && w < 315360000 ))
}

@test "review.bot: a bot review or comment after the trigger means replied; a human's doesn't" {
  git switch -qc feat && commit a.txt a
  trigger_pin 2026-10-15T12:00:00Z
  pr_fixture feat ".reviews.nodes = [$(note R1 User alice lgtm 2026-10-15T12:05:00Z)]"
  state
  [ "$(field .review.bot.replied)" = false ]
  pr_fixture feat ".reviews.nodes = [$(note R1 Bot copilot findings 2026-10-15T12:00:01Z)]"
  state
  [ "$(field .review.bot.replied)" = true ]
  pr_fixture feat ".comments.nodes = [$(note C1 Bot copilot done 2026-10-15T12:05:00Z)]"
  state
  [ "$(field .review.bot.replied)" = true ]
}

@test "review.bot: a review started before the trigger and submitted after it is the reply" {
  git switch -qc feat && commit a.txt a
  trigger_pin 2026-10-15T12:00:00Z
  pr_fixture feat '.reviews.nodes = [{id: "R1", body: "findings", createdAt: "2026-10-15T11:50:00Z", submittedAt: "2026-10-15T12:05:00Z", author: {__typename: "Bot", login: "copilot"}}]'
  state
  [ "$(field .review.bot.replied)" = true ]
}

@test "review.bot: an ignored bot's word doesn't count as the reply" {
  git switch -qc feat && commit a.txt a
  trigger_pin 2026-10-15T12:00:00Z
  "$SD_CONFIG" set --global --json settings.botIgnore '["dependabot"]'
  pr_fixture feat ".comments.nodes = [$(note C1 Bot dependabot bump 2026-10-15T12:05:00Z)]"
  state
  [ "$(field .review.bot.replied)" = false ]
}

@test "review.bot: a placeholder matching botIgnoreBody isn't the reply; an inline-only review (blank body) is" {
  git switch -qc feat && commit a.txt a
  trigger_pin 2026-10-15T12:00:00Z
  echo '{"settings": {"botIgnoreBody": ["is on the job\\."]}}' > "$SHEEPDOG_CONFIG"
  pr_fixture feat ".comments.nodes = [$(note C1 Bot marge 'Marge is on the job.' 2026-10-15T12:00:05Z)]"
  state
  [ "$(field .review.bot.replied)" = false ]
  pr_fixture feat ".reviews.nodes = [$(note R1 Bot marge '' 2026-10-15T12:05:00Z)]"
  state
  [ "$(field .review.bot.replied)" = true ]
}

@test "review.bot: no trigger time (a pin from before, or by hand), or no pin: replied null" {
  git switch -qc feat && commit a.txt a
  pr_fixture feat
  state
  [ "$(field .review.bot.replied)" = null ]
  git update-ref refs/sheepdog/feat/bot HEAD
  state
  [ "$(field .review.bot.status)" = covered ]
  [ "$(field .review.bot.replied)" = null ]
  [ "$(field .review.bot.waited)" = null ]
}

@test "review.bot: gh failing: replied and waited are unknown, not a wait" {
  git switch -qc feat && commit a.txt a
  trigger_pin 2026-10-15T12:00:00Z
  GH_FAKE_FAIL="gh: down" state
  [ "$(field .review.bot.status)" = covered ]
  [ "$(field .review.bot.replied)" = null ]
  [ "$(field .review.bot.waited)" = null ]
}

@test "bot: settings.botTimeout is reported, 3600 s by default" {
  git switch -qc feat && commit a.txt a
  pr_fixture feat
  state
  [ "$(field .bot.timeout)" = 3600 ]
  "$SD_CONFIG" set --global --json settings.botTimeout 600
  state
  [ "$(field .bot.timeout)" = 600 ]
}

@test "summary: bot ⏳ while waiting on its reply, bot ✓ once it came" {
  git switch -qc feat && commit a.txt a
  trigger_pin 2026-10-15T12:00:00Z
  pr_fixture feat
  run "$SD_STATE"
  [[ "$output" == *"bot ⏳"* ]]
  pr_fixture feat ".reviews.nodes = [$(note R1 Bot copilot findings 2026-10-15T12:00:01Z)]"
  run "$SD_STATE"
  [[ "$output" == *"bot ✓"* ]]
}

@test "threads: inline thread items are typed thread" {
  git switch -qc feat && commit a.txt a
  pr_fixture feat ".reviewThreads.nodes = [$(thread Bot copilot Bot copilot)]"
  state
  [ "$(field '.threads.items[0].type')" = thread ]
  [ "$(field '.threads.items[0].path')" = file.txt ]
}

@test "threads: bot review bodies are owed; empty bodies and humans' are not" {
  git switch -qc feat && commit a.txt a
  pr_fixture feat ".reviews.nodes = [
    $(note R1 Bot copilot),
    $(note R2 Bot copilot '  '),
    $(note R3 Bot copilot ''),
    $(note R4 User alice)]"
  state
  [ "$(field .threads.owed_bot)" = 1 ]
  [ "$(field .threads.owed_human)" = 0 ]
  [ "$(field '.threads.items | length')" = 1 ]
  [ "$(field -c '.threads.items[0]')" = '{"id":"R1","type":"review","path":null,"line":null,"outdated":null,"kind":"bot","last":"copilot","mark":"i","author":"copilot","url":null,"excerpt":"findings"}' ]
}

@test "threads: bot top-level comments are owed; humans' are out of scope" {
  git switch -qc feat && commit a.txt a
  pr_fixture feat ".comments.nodes = [$(note C1 Bot coderabbitai), $(note C2 User alice)]"
  state
  [ "$(field .threads.owed_bot)" = 1 ]
  [ "$(field .threads.owed_human)" = 0 ]
  [ "$(field '.threads.items[0].type')" = comment ]
  [ "$(field '.threads.items[0].id')" = C1 ]
  [ "$(field '.threads.items[0].outdated')" = null ]
}

@test "threads: reviews and comments listed in branch sd-handled are not owed" {
  git switch -qc feat && commit a.txt a
  git config --add branch.feat.sd-handled R1
  git config --add branch.feat.sd-handled C1
  pr_fixture feat ".reviews.nodes = [$(note R1 Bot copilot), $(note R2 Bot copilot)]" \
    ".comments.nodes = [$(note C1 Bot copilot)]"
  state
  [ "$(field .threads.owed_bot)" = 1 ]
  [ "$(field '[.threads.items[].id] | join(",")')" = R2 ]
}

@test "threads: by_type counts owed bot items of each type" {
  git switch -qc feat && commit a.txt a
  pr_fixture feat ".reviewThreads.nodes = [$(thread Bot copilot Bot copilot), $(thread User alice User alice)]" \
    ".reviews.nodes = [$(note R1 Bot copilot), $(note R2 Bot copilot)]" \
    ".comments.nodes = [$(note C1 Bot copilot)]"
  state
  [ "$(field -c .threads.by_type)" = '{"thread":1,"review":2,"comment":1}' ]
  [ "$(field .threads.owed_bot)" = 4 ]
  [ "$(field .threads.owed_human)" = 1 ]
}

@test "threads: gh failing leaves by_type unknown" {
  git switch -qc feat && commit a.txt a
  GH_FAKE_FAIL="gh: down" state
  [ "$(field .threads.by_type)" = null ]
}

@test "threads: settings.botLogin takes several logins, with or without [bot]" {
  git switch -qc feat && commit a.txt a
  "$SD_CONFIG" set --global --json settings.botLogin '["marge", "homer[bot]"]'
  pr_fixture feat ".reviewThreads.nodes = [$(thread User marge User marge), $(thread User homer User homer)]" \
    ".reviews.nodes = [$(note R1 User 'marge[bot]')]" \
    ".comments.nodes = [$(note C1 User homer), $(note C2 User alice)]"
  state
  [ "$(field .threads.owed_bot)" = 4 ]
  [ "$(field .threads.owed_human)" = 0 ]
}

@test "threads: bots in settings.botIgnore are never owed" {
  git switch -qc feat && commit a.txt a
  "$SD_CONFIG" set --global --json settings.botIgnore '["github-actions", "vercel[bot]"]'
  pr_fixture feat ".reviewThreads.nodes = [$(thread Bot github-actions Bot github-actions), $(thread Bot copilot Bot copilot)]" \
    ".reviews.nodes = [$(note R1 Bot vercel)]" \
    ".comments.nodes = [$(note C1 Bot 'github-actions[bot]'), $(note C2 Bot copilot)]"
  state
  [ "$(field .threads.owed_bot)" = 2 ]
  [ "$(field '[.threads.items[].last] | join(",")')" = "copilot,copilot" ]
}

@test "threads: git config sheepdog.botLogin is not read" {
  git switch -qc feat && commit a.txt a
  git config sheepdog.botLogin marge
  pr_fixture feat ".reviews.nodes = [$(note R1 User marge)]"
  state
  [ "$(field .threads.owed_bot)" = 0 ]
}

@test "threads: botLogin and botIgnore come from the branch file too" {
  git switch -qc feat && commit a.txt a
  mkdir -p .git/sheepdog
  echo '{"settings": {"botLogin": ["marge"]}}' > .git/sheepdog/feat.json
  pr_fixture feat ".reviews.nodes = [$(note R1 User marge)]"
  state
  [ "$(field .threads.owed_bot)" = 1 ]
}

@test "threads: botLogin and botIgnore come from the config file" {
  git switch -qc feat && commit a.txt a
  echo '{"settings": {"botLogin": ["marge"], "botIgnore": ["vercel"]}}' > "$SHEEPDOG_CONFIG"
  pr_fixture feat ".reviews.nodes = [$(note R1 User marge), $(note R2 Bot vercel)]"
  state
  [ "$(field '[.threads.items[].id] | join(",")')" = R1 ]
}

@test "threads: bot reviews and comments whose body matches botIgnoreBody are never owed" {
  git switch -qc feat && commit a.txt a
  echo '{"settings": {"botIgnoreBody": ["is on the job\\.", "^Deployed"]}}' > "$SHEEPDOG_CONFIG"
  pr_fixture feat ".reviews.nodes = [$(note R1 Bot marge 'Marge is on the job. Wait.'), $(note R2 Bot marge 'Real finding')]" \
    ".comments.nodes = [$(note C1 Bot github-actions 'Deployed to preview'), $(note C2 Bot github-actions 'Not Deployed')]"
  state
  [ "$(field '[.threads.items[].id] | join(",")')" = R2,C2 ]
}

@test "threads: an invalid botIgnoreBody regex fails loudly" {
  git switch -qc feat && commit a.txt a
  echo '{"settings": {"botIgnoreBody": ["("]}}' > "$SHEEPDOG_CONFIG"
  pr_fixture feat ".comments.nodes = [$(note C1 Bot copilot)]"
  run "$SD_STATE" --json
  [ "$status" -ne 0 ]
  [[ "$output" == *"botIgnoreBody"* ]]
}

@test "config: remote comes from the config file" {
  git remote rename origin upstream
  echo '{"settings": {"remote": "upstream"}}' > "$SHEEPDOG_CONFIG"
  git switch -qc feat && commit a.txt a && git push -q upstream feat 2>/dev/null
  state
  [ "$(field .push.remote_sha)" = "$(git rev-parse feat)" ]
}

@test "config: an unreadable config file fails" {
  echo 'not json' > "$SHEEPDOG_CONFIG"
  run "$SD_STATE" --json
  [ "$status" -ne 0 ]
  [[ "$output" == *"sd config"* ]]
}

@test "summary: review bodies and comments count as info, threads as blocking" {
  git switch -qc feat && commit a.txt a
  pr_fixture feat ".reviewThreads.nodes = [$(thread Bot copilot Bot copilot)]" \
    ".reviews.nodes = [$(note R1 Bot copilot)]" ".comments.nodes = [$(note C1 Bot copilot)]"
  run "$SD_STATE"
  [ "$status" -eq 0 ]
  [[ "$output" == *" · !:1 i:2"* ]] || false
  [[ "$output" != *"threads:"* ]]
}

@test "summary: nothing owed still shows both counts" {
  git switch -qc feat && commit a.txt a
  pr_fixture feat
  run "$SD_STATE"
  [[ "$output" == *"!:0 i:0"* ]]
}

# ── marks: ! blocks, i is to read ────────────────────────────────────────────

# review <id> <type> <login> <state> [body] — a review node with a state.
review() {
  printf '{id: "%s", url: "https://example/pr/7#%s", state: "%s", body: "%s", author: {__typename: "%s", login: "%s"}}' \
    "$1" "$1" "$4" "${5-findings}" "$2" "$3"
}

@test "marks: every thread is !, bot review bodies and comments are i" {
  git switch -qc feat && commit a.txt a
  pr_fixture feat ".reviewThreads.nodes = [$(thread User alice User alice)]" \
    ".reviews.nodes = [$(review R1 Bot copilot COMMENTED)]" ".comments.nodes = [$(note C1 Bot copilot)]"
  state
  [ "$(field '[.threads.items[] | "\(.id)=\(.mark)"] | join(",")')" = 'T-alice-alice=!,R1=i,C1=i' ]
  [ "$(field .threads.blocking)" = 1 ]
  [ "$(field .threads.info)" = 2 ]
}

@test "marks: a bot review that requests changes is !" {
  git switch -qc feat && commit a.txt a
  pr_fixture feat ".reviews.nodes = [$(review R1 Bot copilot CHANGES_REQUESTED)]"
  state
  [ "$(field '.threads.items[0].mark')" = '!' ]
  [ "$(field .threads.blocking)" = 1 ]
  [ "$(field .threads.info)" = 0 ]
}

@test "marks: a human's latest review requesting changes is !; a later approval clears it" {
  git switch -qc feat && commit a.txt a
  pr_fixture feat ".reviews.nodes = [$(review R1 User alice CHANGES_REQUESTED 'Please split this')]"
  state
  [ "$(field -c '[.threads.items[] | {id, kind, mark}]')" = '[{"id":"R1","kind":"human","mark":"!"}]' ]
  [ "$(field .threads.owed_human)" = 1 ]
  pr_fixture feat ".reviews.nodes = [$(review R1 User alice CHANGES_REQUESTED), $(review R2 User alice APPROVED '')]"
  state
  [ "$(field .threads.blocking)" = 0 ]
  [ "$(field '.threads.items | length')" = 0 ]
}

@test "marks: a later comment-only review doesn't clear a request for changes" {
  git switch -qc feat && commit a.txt a
  pr_fixture feat ".reviews.nodes = [$(review R1 User alice CHANGES_REQUESTED), $(review R2 User alice COMMENTED 'one more thing')]"
  state
  [ "$(field -c '[.threads.items[] | {id, mark}]')" = '[{"id":"R1","mark":"!"}]' ]
}

@test "items: url, author and a one-line excerpt" {
  git switch -qc feat && commit a.txt a
  body='<!-- linear-linkback -->\n<p><a href=\"https://linear.app/x\">DEVOPS-1 Do the thing</a></p>'
  pr_fixture feat ".reviews.nodes = [$(review R1 Bot claude COMMENTED '\n## Review: Move monitors (pass 2)\n\nmore')]" \
    ".comments.nodes = [{id: \"C1\", url: \"https://example/pr/7#C1\", body: \"$body\", author: {__typename: \"Bot\", login: \"linear-code\"}}]"
  state
  [ "$(field -c '[.threads.items[] | {author, excerpt, url}]')" = '[{"author":"claude","excerpt":"Review: Move monitors (pass 2)","url":"https://example/pr/7#R1"},{"author":"linear-code","excerpt":"DEVOPS-1 Do the thing","url":"https://example/pr/7#C1"}]' ]
}

@test "items: a thread's url and excerpt come from its first comment" {
  git switch -qc feat && commit a.txt a
  pr_fixture feat ".reviewThreads.nodes = [$(thread Bot copilot Bot copilot)]" \
    '.reviewThreads.nodes[0].first.nodes[0] += {url: "https://example/pr/7#t1", body: "Rename `one`?"}'
  state
  [ "$(field -c '.threads.items[0] | {author, excerpt, url}')" = '{"author":"copilot","excerpt":"Rename `one`?","url":"https://example/pr/7#t1"}' ]
}

@test "items: a multi-line HTML comment is skipped in the excerpt" {
  git switch -qc feat && commit a.txt a
  pr_fixture feat ".comments.nodes = [$(note C1 Bot copilot '<!--\nccr-overview\n-->\n## Real summary')]"
  state
  [ "$(field '.threads.items[0].excerpt')" = "Real summary" ]
}

@test "items: a long excerpt is cut to 100 characters" {
  git switch -qc feat && commit a.txt a
  long=$(printf 'x%.0s' {1..150})
  pr_fixture feat ".comments.nodes = [$(note C1 Bot copilot "$long")]"
  state
  [ "$(field '.threads.items[0].excerpt | length')" = 100 ]
}

# ── stored: what sheepdog keeps for the branch ────────────────────────────────

@test "stored: handled ids and the branch's sheepdog refs" {
  git switch -qc feat && commit a.txt a
  git config --add branch.feat.sd-handled R1
  git config --add branch.feat.sd-handled C1
  git update-ref refs/sheepdog/feat/local HEAD
  git update-ref refs/sheepdog/feat/bot HEAD
  git update-ref refs/sheepdog/other/local HEAD
  state
  [ "$(field -c .stored.handled)" = '["R1","C1"]' ]
  [ "$(field -c '.stored.refs | keys')" = '["bot","local"]' ]
  [ "$(field .stored.refs.local)" = "$(git rev-parse HEAD)" ]
}

@test "stored: a branch with a slash keeps the rest of the ref name" {
  git switch -qc topic/sub && commit a.txt a
  git update-ref refs/sheepdog/topic/sub/local HEAD
  state
  [ "$(field -c '.stored.refs | keys')" = '["local"]' ]
}

@test "stored: nothing stored is empty, not missing" {
  git switch -qc feat
  state
  [ "$(field -c .stored)" = '{"handled":[],"refs":{}}' ]
}

# ── --long: the one-liner, then one line per item ────────────────────────────

@test "--long: one line per item with its mark, type, author, excerpt and url" {
  git switch -qc feat && commit a.txt a && git push -q -u origin feat 2>/dev/null
  git update-ref refs/sheepdog/feat/local HEAD
  pr_fixture feat ".reviewThreads.nodes = [$(thread Bot copilot Bot copilot)]" \
    '.reviewThreads.nodes[0].first.nodes[0] += {url: "https://example/pr/7#t1", body: "Rename it"}' \
    ".reviews.nodes = [$(review R1 Bot claude COMMENTED 'Looks fine')]"
  run "$SD_STATE" --long
  [ "$status" -eq 0 ]
  [ "${#lines[@]}" -eq 4 ]
  [[ "${lines[0]}" == "https://example/pr/7 open · "*"!:1 i:1"* ]] || false
  [ "${lines[1]}" = "NEXT  agent  address 2 bot items  → sd act address" ]
  [[ "${lines[2]}" =~ ^\ +!\ +thread\ +copilot\ +file.txt:2\ +Rename\ it\ +https://example/pr/7#t1$ ]] || false
  [[ "${lines[3]}" =~ ^\ +i\ +review\ +claude\ +Looks\ fine\ +https://example/pr/7#R1$ ]]
}

@test "--long: nothing owed is the one-liner and the next steps" {
  git switch -qc feat && commit a.txt a
  git update-ref refs/sheepdog/feat/local HEAD
  pr_fixture feat
  run "$SD_STATE" --long
  [ "$status" -eq 0 ]
  [ "${#lines[@]}" -eq 3 ]
  [[ "${lines[1]}" == "NEXT  run    fetch"* ]] || false
  [[ "${lines[2]}" == "then  run    push"* ]]
}

@test "--long and --json together is a usage error" {
  run "$SD_STATE" --long --json
  [ "$status" -ne 0 ]
}

# ── flags: suppressions, skips, edited tests, for chrismo (#125) ─────────────

@test "flags: a suppression added on the branch, with the commit that added it" {
  git switch -qc feat
  commit a.sh $'echo hi\n# shellcheck disable=SC2086\necho $x'
  added=$(git rev-parse HEAD)
  commit b.txt b
  state
  [ "$(field '.flags | length')" = 1 ]
  [ "$(field '.flags[0] | [.kind, .file, .line, .n] | join(" ")')" = "suppression a.sh 2 1" ]
  [ "$(field '.flags[0].text')" = "# shellcheck disable=SC2086" ]
  [ "$(field '.flags[0].commits | join(" ")')" = "$added" ]
}

@test "flags: hits in one file each name their own commit" {
  git switch -qc feat
  commit a.sh $'# shellcheck disable=SC1\necho a'
  first=$(git rev-parse HEAD)
  commit a.sh $'# shellcheck disable=SC1\necho a\n# eslint-disable-line\n# noqa'
  second=$(git rev-parse HEAD)
  state
  [ "$(field '[.flags[] | "\(.line) \(.commits[0])"] | join(", ")')" = "1 $first, 3 $second, 4 $second" ]
}

@test "flags: each default suppression is caught" {
  git switch -qc feat
  commit s.txt $'// eslint-disable-next-line\nx = 1  # noqa: E501\n// @ts-ignore\n// @ts-expect-error\ny = 2  # type: ignore\nz := 3 //nolint\n# rubocop:disable Style\n#[allow(dead_code)]\nplain line'
  state
  [ "$(field '[.flags[] | select(.kind == "suppression") | .line] | join(" ")')" = "1 2 3 4 5 6 7 8" ]
}

@test "flags: a test skip in a test file; the same word elsewhere isn't one" {
  git switch -qc feat
  commit x.bats $'@test "a" {\n  skip "later"\n  true\n}'
  commit notes.txt $'skip this\nxit(1)'
  mkdir -p spec test tests
  commit spec/a_spec.rb $'it "b" do\n  pending "soon"\nend'
  commit test/a.test.js $'xit("c", () => {})\ndescribe.skip("d")\nxdescribe("e")'
  commit b_test.go $'func TestB(t *testing.T) {\n\tt.Skip("no")\n}'
  commit tests/test_c.py $'@pytest.mark.skip(reason="x")\ndef test_c(): pass'
  state
  [ "$(field '[.flags[] | select(.kind == "skip") | "\(.file):\(.line)"] | join(" ")')" \
    = "b_test.go:2 spec/a_spec.rb:2 test/a.test.js:1 test/a.test.js:2 test/a.test.js:3 tests/test_c.py:1 x.bats:2" ]
}

@test "flags: a guarded bats skip (cond || skip, && skip, ; skip) is a skip; skipped as a word isn't" {
  git switch -qc feat
  commit x.bats $'@test "a" {\n  command -v plutil || skip "no plutil"\n  [ -n "$CI" ] && skip\n  true; skip "later"\n  echo skipped\n}'
  state
  [ "$(field '[.flags[] | select(.kind == "skip") | .line] | join(" ")')" = "2 3 4" ]
}

@test "flags: a test file moved out of the test files, even unchanged, is a test-edit" {
  commit a.bats $'@test "a" {\n  true\n}'
  mkdir -p test && commit test/b.sh 'true'
  commit c.bats $'@test "c" {\n  true\n}'
  git push -q origin main 2>/dev/null
  git switch -qc feat
  git mv a.bats a.bats.off && git mv test/b.sh b.sh && git mv c.bats d.bats && git commit -qm "move"
  moved=$(git rev-parse HEAD)
  state
  [ "$(field '[.flags[] | "\(.kind) \(.file):\(.line) -\(.n)"] | join(", ")')" = "test-edit a.bats:1 -3, test-edit test/b.sh:1 -1" ]
  [ "$(field '.flags[] | select(.file == "a.bats") | .text')" = "moved to a.bats.off, out of the test files" ]
  [ "$(field '.flags[] | select(.file == "a.bats") | .commits | join(" ")')" = "$moved" ]
}

@test "flags: a base that isn't in HEAD's history (a tree restack due) isn't diffed: flags_error says why" {
  git switch -qc parent && commit s.txt X && commit t.txt Y && git push -q origin parent 2>/dev/null
  local p1 p2; p1=$(git rev-parse parent~1) p2=$(git rev-parse parent)
  git switch -qc feat main
  printf 'X\n' > s.txt && printf 'Y\n' > t.txt && git add s.txt t.txt && git commit -qm "parent, copied" && commit c.sh '# noqa'
  git config branch.feat.sd-parent parent
  git update-ref refs/sheepdog/feat/base "$p1"
  squash_parent_away
  pr_fixture parent '.state = "MERGED"' ".headRefOid = \"$p2\""
  state
  [ "$(field .sync.by_tree)" = true ]
  [ "$(field .flags)" = null ]
  [[ "$(field .flags_error)" == *"isn't in feat's history"* ]]
}

@test "flags: lines changed or deleted in a test file the base had; added lines aren't" {
  commit t.bats $'@test "one" {\n  [ 1 -eq 1 ]\n}\n@test "two" {\n  true\n}'
  commit gone.bats $'@test "x" {\n  true\n}'
  git push -q origin main 2>/dev/null
  git switch -qc feat
  commit t.bats $'@test "one" {\n  [ 1 -eq 2 ]\n}\n@test "two" {\n  true\n}\n@test "three" {\n  true\n}'
  edited=$(git rev-parse HEAD)
  git rm -q gone.bats && git commit -qm "drop gone"
  dropped=$(git rev-parse HEAD)
  commit new.bats $'@test "new" {\n  true\n}'
  commit file.txt changed
  state
  [ "$(field '[.flags[] | "\(.kind) \(.file):\(.line) -\(.n)"] | join(", ")')" = "test-edit gone.bats:1 -3, test-edit t.bats:2 -1" ]
  [ "$(field '.flags[] | select(.file == "t.bats") | .text')" = "  [ 1 -eq 1 ]" ]
  [ "$(field '.flags[] | select(.file == "t.bats") | .commits | join(" ")')" = "$edited" ]
  [ "$(field '.flags[] | select(.file == "gone.bats") | .commits | join(" ")')" = "$dropped" ]
}

@test "flags: a line added and removed again on the branch isn't one" {
  git switch -qc feat
  commit a.sh $'# shellcheck disable=SC2086\necho $x'
  commit a.sh 'echo "$x"'
  state
  [ "$(field '.flags')" = "[]" ]
}

@test "flags: settings.flags replaces a list; the others keep their defaults" {
  git switch -qc feat
  commit a.sh $'# shellcheck disable=SC2086\n# lint-off: here'
  mkdir checks && commit checks/a.sh 'skip'
  "$SD_CONFIG" set --global --json settings.flags '{"suppressions": ["lint-off"], "testFiles": ["checks/"]}' >/dev/null
  state
  [ "$(field '[.flags[] | "\(.kind) \(.file):\(.line)"] | join(", ")')" = "suppression a.sh:2, skip checks/a.sh:1" ]
}

@test "flags: an invalid pattern is flags_error, not an error that steers NEXT; flags is null" {
  git switch -qc feat && commit a.txt a
  "$SD_CONFIG" set --global --json settings.flags '{"skips": ["("]}' >/dev/null
  state
  [ "$(field '.flags')" = null ]
  [[ "$(field '.flags_error')" == *"settings.flags"* ]] || false
  [ "$(field '.errors | length')" = 0 ]
  [ "$(field '[.next[].key] | index("errors")')" = null ]
  run "$SD_STATE"
  [ "$status" -eq 0 ]
  [[ "${lines[0]}" == *" · ⚑ check failed"* ]]
}

@test "flags: flags_error is null when the check ran" {
  git switch -qc feat && commit a.txt a
  state
  [ "$(field '.flags_error')" = null ]
}

@test "flags: no base, no check: flags is null" {
  git switch -qc feat && commit a.txt a
  git config branch.feat.sd-parent gone
  state
  [ "$(field '.flags')" = null ]
}

@test "flags: counted on the one-liner, one line each in --long" {
  git switch -qc feat
  commit a.sh $'# shellcheck disable=SC2086\necho $x'
  added=$(git rev-parse --short=7 HEAD)
  run "$SD_STATE"
  [ "$status" -eq 0 ]
  [[ "${lines[0]}" == *" · ⚑ 1 flagged"* ]] || false
  run "$SD_STATE" --long
  [ "$status" -eq 0 ]
  [[ "${lines[-1]}" =~ ^\ +⚑\ +suppression\ +a\.sh:1\ +#\ shellcheck\ disable=SC2086\ +$added$ ]]
}

# ── O8 session ───────────────────────────────────────────────────────────────

@test "session: enrollment, harness, session and lock come from branch config" {
  git switch -qc feat
  git config branch.feat.sheepdog true
  git config branch.feat.sd-harness pi
  git config branch.feat.sd-session abc-123
  git config branch.feat.sd-lock chrismo
  state
  [ "$(field .session.enrolled)" = true ]
  [ "$(field .session.harness)" = pi ]
  [ "$(field .session.id)" = abc-123 ]
  [ "$(field .session.locked)" = true ]
}

@test "session: unenrolled branch" {
  git switch -qc feat
  state
  [ "$(field .session.enrolled)" = false ]
  [ "$(field .session.locked)" = false ]
}

# ── spend: what the branch has cost, against settings.branchBudget (#121) ────

usage_row() { # usage_row <branch> <cost_usd> [repo]
  mkdir -p "$SHEEPDOG_STATE_DIR"
  printf '{time:2026-10-16T20:33:06Z,repo:"%s",branch:"%s",step:"review",harness:"claude",session:"s",mode:"new",status:0,cost_usd:%s}\n' \
    "${3:-$TMP/work}" "$1" "$2" >> "$SHEEPDOG_STATE_DIR/usage.sup"
}

@test "spend: this repo's steps on the branch, summed from usage.sup, and settings.branchBudget (25 USD)" {
  git switch -qc feat
  state
  [ "$(field -c .spend)" = '{"usd":0,"budget":25}' ]
  usage_row feat 1.5; usage_row feat 2.25; usage_row other 9.; usage_row feat 9. /elsewhere; usage_row feat 3.
  "$SD_CONFIG" set --json settings.branchBudget 10
  state
  [ "$(field -c .spend)" = '{"usd":6.75,"budget":10}' ]
}

@test "spend: past settings.branchBudget, NEXT is chrismo's instead of the agent's" {
  git switch -qc feat && commit a.txt a && git push -q -u origin feat 2>/dev/null
  git update-ref refs/sheepdog/feat/local HEAD
  pr_fixture feat ".reviewThreads.nodes = [$(thread Bot copilot Bot copilot)]"
  "$SD_CONFIG" set --json settings.branchBudget 2
  usage_row feat 2.5
  run "$SD_STATE"
  [ "$status" -eq 0 ]
  [ "${lines[1]}" = 'NEXT  you    spent $2.50 on this branch, past settings.branchBudget ($2.00): raise it, or take over  → sd act attach' ]
}

@test "spend: a line of usage.sup that isn't a record is skipped with a warning, not an error on every branch" {
  git switch -qc feat
  usage_row feat 1.5
  echo '{time:2026-10-16T20:34:06Z,repo:"cut off' >> "$SHEEPDOG_STATE_DIR/usage.sup"
  usage_row feat 2.
  run --separate-stderr "$SD_STATE" --json
  [ "$status" -eq 0 ]
  [ "$(field .spend.usd)" = 3.5 ]
  [ "$(field -c .errors)" = '[]' ]
  [[ "$stderr" == "sd state: warning: 1 line(s) of $SHEEPDOG_STATE_DIR/usage.sup aren't usage records"* ]]
}

@test "spend: a usage.sup that still won't read is an error, not a stop" {
  git switch -qc feat
  mkdir -p "$SHEEPDOG_STATE_DIR"
  echo '{time:soon}' > "$SHEEPDOG_STATE_DIR/usage.sup"
  state
  [ "$(field .spend.usd)" = null ]
  [[ "$(field '.errors[]')" == *"usage.sup"* ]]
}

# ── summary line ─────────────────────────────────────────────────────────────

@test "summary: default output is one line naming the key facts, then NEXT" {
  git switch -qc feat && commit a.txt a && git push -q -u origin feat 2>/dev/null
  pr_fixture feat "$(checks '[{__typename:"CheckRun",name:"lint",status:"COMPLETED",conclusion:"FAILURE"}]')" \
    ".reviewThreads.nodes = [$(thread User alice User alice)]"
  run "$SD_STATE"
  [ "$status" -eq 0 ]
  [ "${#lines[@]}" -eq 2 ]
  [[ "${lines[0]}" == "https://example/pr/7 open · "* ]] || false
  [[ "${lines[0]}" != *"feat"* ]] || false
  [[ "${lines[0]}" != *"#7"* ]] || false
  [[ "${lines[0]}" == *"CI red"* ]] || false
  [[ "${lines[0]}" == *"!:1 i:0"* ]] || false
  [[ "${lines[1]}" == "NEXT  you    reply to 1 human item  → sd state --long  (+"*" more)" ]]
}

@test "summary: a remote commit this copy pushed and then reset away says so (#86)" {
  git switch -qc feat && commit a.txt a && git push -q -u origin feat 2>/dev/null
  commit b.txt b && git push -q origin feat 2>/dev/null
  git reset -q --hard HEAD~1
  pr_fixture feat ".headRefOid = \"$(git rev-parse origin/feat)\""
  run "$SD_STATE"
  [ "$status" -eq 0 ]
  [[ "${lines[0]}" == *"remote +1 (reset away here)"* ]] || false
  [[ "${lines[1]}" == "NEXT  run    force-push: drop 1 commit the remote has, reset away here  → sd act push"* ]]
  state
  [ "$(field .push.remote_made)" = true ]
  [ "$(field .push.dirty)" = false ]
  echo x > x.txt
  state
  [ "$(field .push.dirty)" = true ]
  [ "$(field '.next[0].key')" = reset-dirty ]
}

@test "push: someone's commit taken with a fast-forward and reset away wasn't made here: take, not drop (#86)" {
  git switch -qc feat && commit a.txt a && git push -q -u origin feat 2>/dev/null
  git -C "$TMP/other" fetch -q && git -C "$TMP/other" switch -q feat && git -C "$TMP/other" config user.email them@example.com
  commit x.txt x "$TMP/other" && git -C "$TMP/other" push -q origin feat 2>/dev/null
  git fetch -q && git merge -q --ff-only origin/feat && git reset -q --hard HEAD~1
  pr_fixture feat ".headRefOid = \"$(git rev-parse origin/feat)\""
  state
  [ "$(field .push.remote_ours)" = true ]
  [ "$(field .push.remote_made)" = false ]
  [ "$(field '.next[0].key')" = take ]
}

@test "push: remote_made and dirty are null unless the remote is ahead of a reset" {
  git switch -qc feat && commit a.txt a && git push -q -u origin feat 2>/dev/null
  state
  [ "$(field .push.remote_made)" = null ]
  [ "$(field .push.dirty)" = null ]
}

@test "push: held CI config is reported until its commit is on the remote, and NEXT hands it to chrismo (#124)" {
  git switch -qc feat && commit a.txt a && git push -q -u origin feat 2>/dev/null
  pr_fixture feat ".headRefOid = \"$(git rev-parse origin/feat)\""
  state
  [ "$(field .push.held)" = null ]
  commit b.txt b
  git config branch.feat.sd-held "$(git rev-parse HEAD)"$'\n'".github/workflows/test.yml"$'\n'".rwx/tests.yml"
  state
  [ "$(field -c .push.held)" = "{\"sha\":\"$(git rev-parse HEAD)\",\"paths\":[\".github/workflows/test.yml\",\".rwx/tests.yml\"]}" ]
  [ "$(field '.next[0].key')" = ci-config ]
  run "$SD_STATE"
  [[ "${lines[1]}" == "NEXT  you    the agent changed CI config (.github/workflows/test.yml, .rwx/tests.yml): review, then sd act push  → sd act push"* ]]
  git push -q origin feat 2>/dev/null
  state
  [ "$(field .push.held)" = null ]
}

@test "push: a pushed commit reset away on a branch checked out nowhere: made here, dirty unknown" {
  git switch -qc feat && commit a.txt a && commit b.txt b && git push -q -u origin feat 2>/dev/null
  git switch -q main
  git branch -qf feat feat~1
  state feat
  [ "$(field .push.remote_made)" = true ]
  [ "$(field .push.dirty)" = null ]
  [ "$(field '.next[0].key')" = push ]
}

@test "next: --json carries the ordered steps" {
  git switch -qc feat && commit a.txt a && git push -q -u origin feat 2>/dev/null
  pr_fixture feat ".reviewThreads.nodes = [$(thread Bot copilot Bot copilot)]"
  commit b.txt b
  git update-ref refs/sheepdog/feat/local HEAD
  state
  [ "$(field '[.next[].key] | join(" ")')" = "bot push" ]  # no ci-wait: CI can't run before the push
  [ "$(field '.next[1].cmd')" = "sd act push" ]
}

@test "next: a pushed branch with an open PR and nothing owed is ready to merge" {
  git switch -qc feat && commit a.txt a && git push -q -u origin feat 2>/dev/null
  git update-ref refs/sheepdog/feat/local HEAD
  pr_fixture feat
  run "$SD_STATE"
  [ "$status" -eq 0 ]
  [ "${lines[1]}" = "NEXT  you    ready to merge  → gh pr merge" ]
}

@test "next: uses the configured remote" {
  git remote rename origin up
  git switch -qc feat && commit a.txt a
  "$SD_CONFIG" set --global settings.remote up >/dev/null
  state
  [ "$(field '[.next[] | select(.key == "push") | .say][0]')" = "push: not on up yet" ]
}

@test "summary: a draft PR's link, then draft" {
  git switch -qc feat && commit a.txt a && git push -q -u origin feat 2>/dev/null
  pr_fixture feat '.isDraft = true'
  run "$SD_STATE"
  [ "$status" -eq 0 ]
  [[ "${lines[0]}" == "https://example/pr/7 draft · "* ]]
}

@test "summary: no PR" {
  git switch -qc feat && commit a.txt a
  run "$SD_STATE"
  [ "$status" -eq 0 ]
  [[ "${lines[0]}" == "no PR · "* ]] || false
  [[ "$output" == *"1 unpushed"* ]]
}
