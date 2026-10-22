#!/usr/bin/env bats

bats_require_minimum_version 1.5.0

# Test suite for libexec/sd-act (mechanical pipeline verbs). Only enroll/unenroll
# so far: the config the watcher reads.

SD_ACT="$BATS_TEST_DIRNAME/sd-act"

setup() {
  TMP="$(cd "$BATS_TEST_TMPDIR" && pwd -P)"
  export GIT_CONFIG_GLOBAL="$TMP/gitconfig-global"
  export GIT_CONFIG_SYSTEM=/dev/null
  printf '[user]\n\temail = t@example.com\n\tname = Tester\n[init]\n\tdefaultBranch = main\n' > "$GIT_CONFIG_GLOBAL"
  git init -q "$TMP/repo"
  cd "$TMP/repo"
  git commit -q --allow-empty -m init
  git switch -qc feat
}

repos() { git config --global --get-all sheepdog.repo || true; }

@test "runs under bash 5 via the bash5 shebang" {
  run head -1 "$SD_ACT"
  [ "$output" = "#!/usr/bin/env bash5" ]
}

@test "no verb: usage on stderr, nonzero" {
  run "$SD_ACT"
  [ "$status" -ne 0 ]
  [[ "$output" == *"Usage"* ]]
}

@test "unknown verb fails" {
  run "$SD_ACT" frobnicate
  [ "$status" -ne 0 ]
  [[ "$output" == *"unknown verb"* ]]
}

@test "enroll: marks the current branch and registers the repo" {
  run "$SD_ACT" enroll
  [ "$status" -eq 0 ]
  [[ "$output" == *"enrolled feat"* ]]
  [ "$(git config branch.feat.sheepdog)" = true ]
  [ "$(repos)" = "$TMP/repo" ]
}

@test "enroll: a named branch" {
  git branch other
  run "$SD_ACT" enroll other
  [ "$status" -eq 0 ]
  [ "$(git config branch.other.sheepdog)" = true ]
  [ -z "$(git config branch.feat.sheepdog || true)" ]
}

# no_prs: an origin with main on it, sd state on PATH, and a gh with no PRs.
no_prs() {
  git init -q --bare "$TMP/origin.git"
  git remote add origin "$TMP/origin.git"
  git push -q origin HEAD:main 2>/dev/null
  git fetch -q origin
  git remote set-head origin main
  mkdir -p "$TMP/fakebin"
  cat > "$TMP/fakebin/gh" <<'SH'
#!/usr/bin/env bash
echo '{"data":{"viewer":{"login":"me"},"repository":{"defaultBranchRef":{"name":"main"},"pullRequests":{"nodes":[]}}}}'
SH
  chmod +x "$TMP/fakebin/gh"
  export PATH="$BATS_TEST_DIRNAME:$TMP/fakebin:$PATH" SHEEPDOG_CONFIG="$TMP/global.json"
}

@test "enroll: a branch made from another's tip, with no PR, records it as the parent (#94)" {
  git switch -q main && no_prs
  git switch -qc par && git commit -q --allow-empty -m p1
  git switch -qc kid && git commit -q --allow-empty -m k1
  run "$SD_ACT" enroll kid
  [ "$status" -eq 0 ]
  [[ "$output" == *"parent: par"* ]]
  [ "$(git config branch.kid.sd-parent)" = par ]
  [ "$(git rev-parse refs/sheepdog/kid/base)" = "$(git rev-parse par)" ]
}

@test "enroll: a lineage write git refuses is reported, and enroll still succeeds" {
  git switch -q main && no_prs
  git switch -qc par && git commit -q --allow-empty -m p1
  git switch -qc kid && git commit -q --allow-empty -m k1
  git update-ref refs/sheepdog/kid HEAD   # refs/sheepdog/kid/base can't be made
  run "$SD_ACT" enroll kid
  [ "$status" -eq 0 ]
  [[ "$output" == *"enrolled kid"*"parent: par"*"refs/sheepdog/kid"* ]]
  [ "$(git config branch.kid.sd-parent)" = par ]
}

@test "enroll: two candidate parents record neither, and say so" {
  git switch -q main && no_prs
  git switch -qc par && git commit -q --allow-empty -m p1
  git branch twin
  git switch -qc kid && git commit -q --allow-empty -m k1
  run "$SD_ACT" enroll kid
  [ "$status" -eq 0 ]
  [[ "$output" == *"kid looks stacked on par or twin"*"git config branch.kid.sd-parent"* ]]
  [ -z "$(git config branch.kid.sd-parent || true)" ]
}

@test "enroll: a parent that moved on since: the child records nothing and says so, the parent isn't asked" {
  git switch -q main && no_prs
  git switch -qc par && git commit -q --allow-empty -m p1
  git switch -qc kid && git commit -q --allow-empty -m k1
  git switch -q par && git commit -q --allow-empty -m p2
  run "$SD_ACT" enroll kid
  [ "$status" -eq 0 ]
  [[ "$output" == *"kid shares commits with par"*"git config branch.kid.sd-parent par"*"git config branch.kid.sd-parent main"* ]]
  [ -z "$(git config branch.kid.sd-parent || true)" ]
  run "$SD_ACT" enroll par
  [ "$status" -eq 0 ]
  [[ "$output" != *"shares commits"* ]]
  [ -z "$(git config branch.par.sd-parent || true)" ]
}

@test "enroll: a PR on the default branch, or gh failing, records no git-inferred parent" {
  git switch -q main && no_prs
  git switch -qc par && git commit -q --allow-empty -m p1
  git switch -qc kid && git commit -q --allow-empty -m k1
  cat > "$TMP/fakebin/gh" <<'SH'
#!/usr/bin/env bash
echo "gh: API rate limit exceeded" >&2; exit 1
SH
  run "$SD_ACT" enroll kid
  [ "$status" -eq 0 ]
  [ -z "$(git config branch.kid.sd-parent || true)" ]
  # One answer for both of sd state's queries: discovery's list and the fetch by number.
  pr='{"number":3,"url":"u","state":"OPEN","isDraft":false,"baseRefName":"main","headRefName":"kid","headRefOid":"'"$(git rev-parse kid)"'",
  "mergeable":"MERGEABLE","mergeStateStatus":"CLEAN","reviewDecision":null,"mergedAt":null}'
  cat > "$TMP/fakebin/gh" <<SH
#!/usr/bin/env bash
cat <<'JSON'
{"data":{"viewer":{"login":"me"},"repository":{"defaultBranchRef":{"name":"main"},"pullRequests":{"nodes":[$pr]},"pullRequest":$pr}}}
JSON
SH
  run "$SD_ACT" enroll kid
  [ "$status" -eq 0 ]
  [ -z "$(git config branch.kid.sd-parent || true)" ]
}

@test "enroll: a branch made from main alone records no parent" {
  git switch -q main && no_prs
  git switch -qc solo && git commit -q --allow-empty -m s1
  run "$SD_ACT" enroll solo
  [ "$status" -eq 0 ]
  [ -z "$(git config branch.solo.sd-parent || true)" ]
}

@test "enroll: a missing branch fails" {
  run "$SD_ACT" enroll nope
  [ "$status" -ne 0 ]
  [[ "$output" == *"no such branch"* ]]
}

@test "enroll: registers the repo only once" {
  "$SD_ACT" enroll
  git branch other
  "$SD_ACT" enroll other
  [ "$(repos | wc -l | tr -d ' ')" = 1 ]
}

@test "enroll: from a linked worktree, registers the main worktree" {
  git switch -q main
  git worktree add -q "$TMP/wt" feat
  cd "$TMP/wt"
  run "$SD_ACT" enroll
  [ "$status" -eq 0 ]
  [ "$(repos)" = "$TMP/repo" ]
}

@test "unenroll: clears the mark" {
  "$SD_ACT" enroll
  run "$SD_ACT" unenroll
  [ "$status" -eq 0 ]
  [ -z "$(git config branch.feat.sheepdog || true)" ]
}

@test "unenroll: a branch that was never enrolled is fine" {
  run "$SD_ACT" unenroll
  [ "$status" -eq 0 ]
}

@test "forget: unenrolls and deletes refs/sheepdog/<branch>/*" {
  "$SD_ACT" enroll
  git update-ref refs/sheepdog/feat/base HEAD
  git update-ref refs/sheepdog/feat/local HEAD
  run "$SD_ACT" forget
  [ "$status" -eq 0 ]
  [ -z "$(git config branch.feat.sheepdog || true)" ]
  [ -z "$(git for-each-ref refs/sheepdog/feat/)" ]
}

@test "forget: a named branch; other branches' pins stay" {
  git switch -qc other
  git update-ref refs/sheepdog/feat/base HEAD
  git update-ref refs/sheepdog/other/base HEAD
  run "$SD_ACT" forget feat
  [ "$status" -eq 0 ]
  [ -z "$(git for-each-ref refs/sheepdog/feat/)" ]
  [ -n "$(git for-each-ref refs/sheepdog/other/)" ]
}

@test "forget: deletes the branch's stored description too (#153)" {
  mkdir -p .git/sheepdog
  echo '{}' > .git/sheepdog/feat.describe.json
  echo '{}' > .git/sheepdog/other.describe.json
  run "$SD_ACT" forget
  [ "$status" -eq 0 ]
  [ ! -e .git/sheepdog/feat.describe.json ]
  [ -e .git/sheepdog/other.describe.json ]
}

# merged_pr <state> <head>: gh answers that feat's PR is <state> at <head>.
merged_pr() {
  local pr='{"number":3,"url":"u","state":"'"$1"'","isDraft":false,"baseRefName":"main","headRefName":"feat","headRefOid":"'"$2"'",
  "mergeable":"UNKNOWN","mergeStateStatus":"UNKNOWN","reviewDecision":null,"mergedAt":null}'
  cat > "$TMP/fakebin/gh" <<SH
#!/usr/bin/env bash
cat <<'JSON'
{"data":{"viewer":{"login":"me"},"repository":{"defaultBranchRef":{"name":"main"},"pullRequests":{"nodes":[$pr]},"pullRequest":$pr}}}
JSON
SH
}

# merged_wt: feat in its own worktree at $TMP/wt, one commit past main, its PR
# merged at that tip and the remote branch deleted (a squash merge).
merged_wt() {
  git switch -q main && no_prs
  git worktree add -q "$TMP/wt" feat
  git -C "$TMP/wt" commit -q --allow-empty -m f1
  merged_pr MERGED "$(git rev-parse feat)"
  "$SD_ACT" enroll feat >/dev/null
  git update-ref refs/sheepdog/feat/local feat
  cd "$TMP/wt"
}

@test "forget --remove: a merged branch's worktree is removed and the branch deleted (#153)" {
  merged_wt
  run "$SD_ACT" forget --remove
  [ "$status" -eq 0 ]
  [[ "$output" == *"forgot feat"* ]]
  [[ "$output" == *"removed $TMP/wt"* ]]
  [[ "$output" == *"deleted feat"* ]]
  cd "$TMP/repo"
  [ ! -d "$TMP/wt" ]
  [ -z "$(git worktree list --porcelain | grep "$TMP/wt" || true)" ]
  ! git rev-parse -q --verify refs/heads/feat || false
  [ -z "$(git for-each-ref refs/sheepdog/feat/)" ]
  [ -z "$(git config branch.feat.sheepdog || true)" ]
}

@test "forget --remove: a merged branch checked out nowhere is deleted" {
  merged_wt
  cd "$TMP/repo" && git worktree remove "$TMP/wt"
  run "$SD_ACT" forget --remove feat
  [ "$status" -eq 0 ]
  ! git rev-parse -q --verify refs/heads/feat || false
}

@test "forget --remove: names the ignored files that go with the worktree" {
  merged_wt
  mkdir -p "$TMP/repo/.git/info" && printf '.env\nbuild/\n' >> "$TMP/repo/.git/info/exclude"
  echo secret > .env && mkdir build && echo o > build/out
  run "$SD_ACT" forget --remove
  [ "$status" -eq 0 ]
  [[ "$output" == *"removed $TMP/wt with its ignored files: .env build/"* ]]
  [ ! -d "$TMP/wt" ]
}

@test "forget --remove: a branch already deleted is just forgotten" {
  git update-ref refs/sheepdog/gone/local HEAD
  run "$SD_ACT" forget --remove gone
  [ "$status" -eq 0 ]
  [[ "$output" == *"forgot gone"* ]]
  [ -z "$(git for-each-ref refs/sheepdog/gone/)" ]
}

@test "forget --remove: a worktree git won't remove changes nothing" {
  merged_wt
  git -C "$TMP/repo" worktree lock "$TMP/wt"
  run "$SD_ACT" forget --remove
  [ "$status" -ne 0 ]
  [ -d "$TMP/wt" ]
  git rev-parse -q --verify refs/heads/feat
  [ -n "$(git for-each-ref refs/sheepdog/feat/)" ]
  [ "$(git config branch.feat.sheepdog)" = true ]
}

@test "forget --remove: untracked files count even when status hides them" {
  merged_wt
  git config status.showUntrackedFiles no
  echo wip > wip.txt
  run "$SD_ACT" forget --remove
  [ "$status" -ne 0 ]
  [[ "$output" == *"uncommitted changes"* ]]
  [ -d "$TMP/wt" ]
}

@test "forget --remove: refuses with uncommitted changes, and changes nothing" {
  merged_wt
  echo wip > wip.txt
  run "$SD_ACT" forget --remove
  [ "$status" -ne 0 ]
  [[ "$output" == *"uncommitted changes"* ]]
  [ -d "$TMP/wt" ]
  git rev-parse -q --verify refs/heads/feat
  [ -n "$(git for-each-ref refs/sheepdog/feat/)" ]
  [ "$(git config branch.feat.sheepdog)" = true ]
}

@test "forget --remove: refuses when the branch has commits the merged PR doesn't" {
  merged_wt
  git commit -q --allow-empty -m after-merge
  run "$SD_ACT" forget --remove
  [ "$status" -ne 0 ]
  [[ "$output" == *"not in PR #3"* ]]
  [ -d "$TMP/wt" ]
  git rev-parse -q --verify refs/heads/feat
}

@test "forget --remove: refuses when the PR isn't merged" {
  merged_wt
  merged_pr OPEN "$(git rev-parse feat)"
  run "$SD_ACT" forget --remove
  [ "$status" -ne 0 ]
  [[ "$output" == *"isn't merged"* ]]
  [ -d "$TMP/wt" ]
}

@test "forget --remove: in the main worktree, switches it to the default branch and deletes the branch, never removing it" {
  merged_wt
  cd "$TMP/repo" && git worktree remove "$TMP/wt" && git switch -q feat
  run "$SD_ACT" forget --remove
  [ "$status" -eq 0 ]
  [[ "$output" == *"switched $TMP/repo to main"* ]]
  [ -d "$TMP/repo/.git" ]
  [ "$(git branch --show-current)" = main ]
  ! git rev-parse -q --verify refs/heads/feat || false
  [ -z "$(git for-each-ref refs/sheepdog/feat/)" ]
}

@test "forget --remove: in the main worktree, refuses with uncommitted changes" {
  merged_wt
  cd "$TMP/repo" && git worktree remove "$TMP/wt" && git switch -q feat
  echo wip > wip.txt
  run "$SD_ACT" forget --remove
  [ "$status" -ne 0 ]
  [[ "$output" == *"uncommitted changes"* ]]
  [ "$(git branch --show-current)" = feat ]
  [ -e wip.txt ]
}

@test "forget --remove: a merged head this clone doesn't have says fetch, not extra commits" {
  merged_wt
  merged_pr MERGED 0123456789abcdef0123456789abcdef01234567
  run "$SD_ACT" forget --remove
  [ "$status" -ne 0 ]
  [[ "$output" == *"git fetch"* ]]
  [[ "$output" != *"commits the PR doesn't"* ]] || false
  [ -d "$TMP/wt" ]
  git rev-parse -q --verify refs/heads/feat
}

@test "--remove is for sd act forget only" {
  run "$SD_ACT" unenroll --remove
  [ "$status" -ne 0 ]
  [[ "$output" == *"--remove is for sd act forget only"* ]]
}

@test "forget: a branch with nothing stored, even one already deleted, is fine" {
  run "$SD_ACT" forget
  [ "$status" -eq 0 ]
  run "$SD_ACT" forget gone
  [ "$status" -eq 0 ]
}

@test "unregister: removes this repo from the watcher's list" {
  "$SD_ACT" enroll && "$SD_ACT" unenroll
  [ "$(repos)" = "$TMP/repo" ]
  run "$SD_ACT" unregister
  [ "$status" -eq 0 ]
  [ -z "$(repos)" ]
}

@test "unregister: refuses while branches are still enrolled; --force overrides" {
  "$SD_ACT" enroll
  run "$SD_ACT" unregister
  [ "$status" -ne 0 ]
  [[ "$output" == *"feat"* ]]
  [ "$(repos)" = "$TMP/repo" ]
  run "$SD_ACT" unregister --force
  [ "$status" -eq 0 ]
  [ -z "$(repos)" ]
}

@test "unregister: a path, for a repo whose directory is gone" {
  "$SD_ACT" enroll
  git config --global --add sheepdog.repo "$TMP/gone"
  run "$SD_ACT" unregister "$TMP/gone"
  [ "$status" -eq 0 ]
  [ "$(repos)" = "$TMP/repo" ]
}

@test "unregister: a repo that isn't registered is fine" {
  run "$SD_ACT" unregister
  [ "$status" -eq 0 ]
}

@test "--help: every verb has a usage line" {
  run "$SD_ACT" --help
  [ "$status" -eq 0 ]
  verbs=$(sed -n '/^case \$verb in/,/^esac/s/^  \([a-z-]*\)).*/\1/p' "$SD_ACT")
  [ -n "$verbs" ]
  for v in $verbs; do
    [[ "$output" =~ sd\ act\ $v\  ]] || { echo "missing: $v"; false; }
  done
}

@test "--help works outside a repo" {
  cd "$TMP"
  run "$SD_ACT" --help
  [[ "$output" == *"Usage"* ]]
}

@test "unenroll: a real config error is not swallowed" {
  "$SD_ACT" enroll
  touch .git/config.lock
  run "$SD_ACT" unenroll
  rm -f .git/config.lock
  [ "$status" -eq 1 ]
}
