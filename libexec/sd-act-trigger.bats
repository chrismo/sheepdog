#!/usr/bin/env bats

bats_require_minimum_version 1.5.0

# Test suite for `sd act trigger` (A5): post settings.botCommand as a PR
# comment and pin bot at HEAD with its base, recording when GitHub says it
# was posted; skip when already pinned there unless --force.
# The bot reviews what is on GitHub, so an unpushed HEAD is refused, and
# neither the local review nor an accept covering HEAD is a soft refusal
# (--force).
#
# Real git and the real sd state. gh is a stub: graphql answers from
# $GH/<branch>.json (by number: the node of whichever file has it), the REST
# trigger comment is logged to $GH/calls.

SD_ACT="$BATS_TEST_DIRNAME/sd-act"
SD_CONFIG="$BATS_TEST_DIRNAME/sd-config"

setup() {
  TMP="$(cd "$BATS_TEST_TMPDIR" && pwd -P)"
  export GIT_CONFIG_GLOBAL="$TMP/gitconfig-global"
  export GIT_CONFIG_SYSTEM=/dev/null
  printf '[user]\n\temail = t@example.com\n\tname = Tester\n[init]\n\tdefaultBranch = main\n' > "$GIT_CONFIG_GLOBAL"
  export SHEEPDOG_CONFIG="$TMP/global.json"
  export GH="$TMP/gh"
  mkdir -p "$GH" "$TMP/fakebin"
  cat > "$TMP/fakebin/gh" <<'EOF'
#!/usr/bin/env bash
echo "${GH_REPO-unset}" >> "$GH/gh_repo"
q="" b="" fields=()
for a; do
  [[ $a == branch=* ]] && b=${a#branch=}
  [[ $a == query=* ]] && q=${a#query=}
  [[ $a == body=* || $a == subject=* || $a == url=* ]] && fields+=("$a")
done
# REST: the trigger comment, answered with GitHub's own created_at (or $GH/created_at's).
if [[ $1 == api && $2 == repos/*/issues/*/comments ]]; then
  printf 'comment %s %s\n' "$2" "${fields[*]}" >> "$GH/calls"
  [[ -e $GH/fail-comment ]] && { echo "gh: boom" >&2; exit 1; }
  jq -n --arg t "$(cat "$GH/created_at" 2>/dev/null || echo 2026-10-16T12:00:00Z)" '{id: 1, created_at: (if $t == "" then null else $t end)}' \
    | { filter=""; for a; do [[ $prev == --jq ]] && filter=$a; prev=$a; done; if [[ -n $filter ]]; then jq -r "$filter"; else cat; fi; }
  exit 0
fi
n=""; for a; do [[ $a == number=* ]] && n=${a#number=}; done
# By number: the node with that number, from the one branch file that has it.
if [[ -n $n ]]; then
  f=$(jq -rn --argjson n "$n" 'inputs | select(any(.data.repository.pullRequests.nodes[]?; .number == $n)) | input_filename' "$GH"/*.json 2>/dev/null)
  [[ $f != *$'\n'* ]] || { echo "fake gh: #$n is in more than one fixture: $f" >&2; exit 2; }
  [[ -n $f ]] || { echo "gh: Could not resolve to a PullRequest with the number of $n." >&2; exit 1; }
  exec jq --argjson n "$n" '.data.repository |= {defaultBranchRef, pullRequest: (.pullRequests.nodes[] | select(.number == $n))}' "$f"
fi
case $q in
  *) if [[ -f $GH/$b.json ]]; then cat "$GH/$b.json"
     else echo '{"data":{"viewer":{"login":"me"},"repository":{"defaultBranchRef":{"name":"main"},"pullRequests":{"nodes":[]}}}}'
     fi ;;
esac
EOF
  chmod +x "$TMP/fakebin/gh"
  export PATH="$BATS_TEST_DIRNAME:$TMP/fakebin:$PATH"
  "$SD_CONFIG" set --global settings.botCommand "/review-bot"

  git init -q --bare "$TMP/origin.git"
  git clone -q "$TMP/origin.git" "$TMP/repo" 2>/dev/null
  cd "$TMP/repo"
  printf 'a\nb\n' > file.txt && git add file.txt && git commit -qm init && git push -q origin main 2>/dev/null
  git switch -qc feat
  printf 'a\nb\nc\n' > file.txt && git commit -qam "add c"
  git push -q -u origin feat 2>/dev/null
  open_pr feat 7
  git update-ref refs/sheepdog/feat/local HEAD
  git update-ref refs/sheepdog/feat/local-base main
}

open_pr() { # open_pr <branch> <number> [state]
  jq -n --arg b "$1" --argjson n "$2" --arg st "${3:-OPEN}" --arg h "$(git rev-parse "$1")" '{data: {viewer: {login: "me"},
    repository: {defaultBranchRef: {name: "main"},
      pullRequests: {nodes: [{number: $n, url: "https://github.com/upstream/proj/pull/\($n)", state: $st, isDraft: true, baseRefName: "main",
                              headRefName: $b, headRefOid: $h, isCrossRepository: false}]}}}}' > "$GH/$1.json"
}
calls() { cat "$GH/calls" 2>/dev/null; }
pin() { git rev-parse -q --verify "refs/sheepdog/feat/$1" || true; }

@test "trigger: posts the bot command on the PR (REST) and pins bot at HEAD with its base" {
  run "$SD_ACT" trigger
  [ "$status" -eq 0 ]
  # The PR's repo from its URL, not {owner}/{repo}: gh's default repo may be a fork.
  [[ "$(calls)" == *"comment repos/upstream/proj/issues/7/comments body=/review-bot"* ]]
  [ "$(pin bot)" = "$(git rev-parse HEAD)" ]
  [ "$(pin bot-base)" = "$(git rev-parse origin/main)" ]
  [[ "$output" == *"triggered"* ]]
  [[ "$output" == *"waiting"* ]]
}

@test "trigger: a fork's clone asks GitHub about the repo file's settings.remote, not gh's pick among the remotes" {
  # A GitHub URL served from $TMP/ssh: the fake ssh runs git's command there.
  mkdir -p "$TMP/ssh/me" && git clone -q --bare "$TMP/origin.git" "$TMP/ssh/me/fork.git"
  printf '#!/usr/bin/env bash\nc=${@: -1}\ncd %q && eval "git ${c#git-}"\n' "$TMP/ssh" > "$TMP/fakebin/ssh"
  chmod +x "$TMP/fakebin/ssh"
  export GIT_SSH_COMMAND="$TMP/fakebin/ssh"
  git remote add fork git@github.com:me/fork.git
  "$SD_CONFIG" set --repo settings.remote fork
  git fetch -q fork
  run "$SD_ACT" trigger
  [ "$status" -eq 0 ]
  [ "$(sort -u "$GH/gh_repo")" = github.com/me/fork ]
}

@test "trigger: records when the comment was posted, by GitHub's clock" {
  echo 2026-10-16T12:34:56Z > "$GH/created_at"
  run "$SD_ACT" trigger
  [ "$status" -eq 0 ]
  [ "$(git config branch.feat.sd-bot-triggered)" = 2026-10-16T12:34:56Z ]
}

@test "trigger: no created_at in the answer: the local clock, and a note" {
  : > "$GH/created_at"
  run --separate-stderr "$SD_ACT" trigger
  [ "$status" -eq 0 ]
  [[ "$(git config branch.feat.sd-bot-triggered)" =~ ^20[0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9:]+Z$ ]]
  [[ "$stderr" == *"local clock"* ]]
}

@test "trigger: already pinned at HEAD: nothing posted; --force posts again and resets the time" {
  git update-ref refs/sheepdog/feat/bot HEAD
  git config branch.feat.sd-bot-triggered 2026-10-15T00:00:00Z
  run "$SD_ACT" trigger
  [ "$status" -eq 0 ]
  [[ "$output" == *"already"* ]]
  [ -z "$(calls)" ]
  run "$SD_ACT" trigger --force
  [ "$status" -eq 0 ]
  [[ "$(calls)" == *"comment "* ]]
  [ "$(git config branch.feat.sd-bot-triggered)" = 2026-10-16T12:00:00Z ]
}

@test "trigger: HEAD not on the remote yet is refused" {
  echo d >> file.txt && git commit -qam "add d"
  git update-ref refs/sheepdog/feat/local HEAD
  run "$SD_ACT" trigger
  [ "$status" -ne 0 ]
  [[ "$output" == *"push"* ]]
  [ -z "$(calls)" ]
  [ -z "$(pin bot)" ]
}

@test "trigger: the remote ahead of HEAD (a pushed commit dropped here) is refused: push first (#86)" {
  echo d >> file.txt && git commit -qam "add d" && git push -q origin feat 2>/dev/null
  git reset -q --hard HEAD~1
  git update-ref refs/sheepdog/feat/local HEAD
  run "$SD_ACT" trigger
  [ "$status" -ne 0 ]
  [[ "$output" == *"push"* ]] || false
  [ -z "$(calls)" ]
  [ -z "$(pin bot)" ]
}

@test "trigger: the remote ahead with someone else's commit is refused: take it first (#86)" {
  git update-ref refs/sheepdog/feat/local HEAD
  (cd "$TMP/repo" && git clone -q "$TMP/origin.git" "$TMP/other2" 2>/dev/null && cd "$TMP/other2" \
     && git switch -q feat && echo t > t.txt && git add t.txt && git -c user.email=them@example.com -c user.name=Them commit -qm theirs \
     && git push -q origin feat 2>/dev/null)
  git fetch -q origin
  run "$SD_ACT" trigger
  [ "$status" -ne 0 ]
  [[ "$output" == *"take its commits first"* ]] || false
  [ -z "$(calls)" ]
  [ -z "$(pin bot)" ]
}

@test "trigger: --force on a pinned HEAD the remote doesn't have is still refused" {
  echo d >> file.txt && git commit -qam "add d"
  git update-ref refs/sheepdog/feat/local HEAD
  git update-ref refs/sheepdog/feat/bot HEAD
  run "$SD_ACT" trigger --force
  [ "$status" -ne 0 ]
  [[ "$output" == *"push"* ]] || false
  [ -z "$(calls)" ]
}

@test "trigger: no PR is refused" {
  rm "$GH/feat.json"
  run "$SD_ACT" trigger
  [ "$status" -ne 0 ]
  [[ "$output" == *"no PR"* ]]
}

@test "trigger: a closed PR is refused" {
  open_pr feat 7 CLOSED
  run "$SD_ACT" trigger
  [ "$status" -ne 0 ]
  [[ "$output" == *"closed"* ]]
}

@test "trigger: no settings.botCommand is refused before any gh call" {
  "$SD_CONFIG" unset --global settings.botCommand
  run "$SD_ACT" trigger
  [ "$status" -ne 0 ]
  [[ "$output" == *"botCommand"* ]]
  [ -z "$(calls)" ]
}

@test "trigger: the local review not covering HEAD is a soft refusal; --force posts" {
  git update-ref -d refs/sheepdog/feat/local
  run "$SD_ACT" trigger
  [ "$status" -ne 0 ]
  [[ "$output" == *"local review"* ]]
  [ -z "$(calls)" ]
  run "$SD_ACT" trigger --force
  [ "$status" -eq 0 ]
  [ "$(pin bot)" = "$(git rev-parse HEAD)" ]
}

@test "trigger: an accepted pin at HEAD stands in for the local review; no --force (#181)" {
  git update-ref refs/sheepdog/feat/local HEAD~1
  git update-ref refs/sheepdog/feat/accepted HEAD
  git update-ref refs/sheepdog/feat/accepted-base main
  run "$SD_ACT" trigger
  [ "$status" -eq 0 ]
  [[ "$(calls)" == *"comment "* ]] || false
  [ "$(pin bot)" = "$(git rev-parse HEAD)" ]
}

@test "trigger: the comment failing pins nothing and records no time" {
  touch "$GH/fail-comment"
  run "$SD_ACT" trigger
  [ "$status" -ne 0 ]
  [ -z "$(pin bot)" ]
  run git config --get branch.feat.sd-bot-triggered
  [ "$status" -ne 0 ]
}
