#!/usr/bin/env bats

bats_require_minimum_version 1.5.0

# Test suite for `sd act push`: publish a branch. A fast-forward is a plain
# push; rewritten history is force-pushed with a lease on the remote head,
# only when that head is in the branch's reflog (we put it there).
#
# Real git: a bare origin, our clone, and $TMP/other for other people's
# pushes. gh is a stub that fails: push never talks to GitHub.

SD_ACT="$BATS_TEST_DIRNAME/sd-act"

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

  git init -q --bare "$TMP/origin.git"
  git clone -q "$TMP/origin.git" "$TMP/repo" 2>/dev/null
  cd "$TMP/repo"
  printf '%s\n' a b c > file.txt
  git add file.txt && git commit -qm init && git push -q origin main 2>/dev/null
  git clone -q "$TMP/origin.git" "$TMP/other" 2>/dev/null
  git switch -qc feat
  commit feat
  git push -q -u origin feat 2>/dev/null
}

commit() { echo "$1" >> file.txt && git commit -qam "$1"; }

# upstream <message>: someone else commits to feat on origin.
upstream() {
  (cd "$TMP/other" && git fetch -q origin && git checkout -q -B feat origin/feat \
     && echo "$1" >> other.txt && git add other.txt && git -c user.email=them@example.com -c user.name=Them commit -qm "$1" \
     && git push -q origin feat 2>/dev/null)
}

remote() { git ls-remote origin "refs/heads/$1" | cut -f1; }

tracks() { git rev-parse --abbrev-ref "$1@{upstream}" 2>/dev/null; }
inherit_main() { git branch -q --set-upstream-to=origin/main "${1:-feat}"; }

@test "nothing to push: says so" {
  run "$SD_ACT" push
  [ "$status" -eq 0 ]
  [ "$output" = "nothing to push on feat" ]
}

@test "the default branch: refused, nothing pushed (#159)" {
  git switch -q main
  commit local
  run "$SD_ACT" push
  [ "$status" -ne 0 ]
  [[ "$output" == *"not pushing main: it's the default branch"* ]]
  [ "$(remote main)" != "$(git rev-parse main)" ]
}

@test "new commits: a plain push" {
  commit two
  run "$SD_ACT" push
  [ "$status" -eq 0 ]
  [ "$(remote feat)" = "$(git rev-parse feat)" ]
  [ "$output" = "pushed feat to origin (1 commit)" ]
}

@test "not on the remote yet: pushed, with the upstream set" {
  git switch -qc fresh
  commit fresh
  run "$SD_ACT" push
  [ "$status" -eq 0 ]
  [ "$(remote fresh)" = "$(git rev-parse fresh)" ]
  [ "$(git config branch.fresh.remote)" = origin ]
  [ "$output" = "pushed fresh to origin (new branch)" ]
}

@test "rewritten, and the remote head is ours: force-pushed with a lease" {
  git commit -q --amend -m "feat, reworded"
  run "$SD_ACT" push
  [ "$status" -eq 0 ]
  [ "$(remote feat)" = "$(git rev-parse feat)" ]
  [[ "$output" == "force-pushed feat to origin (rewritten; the remote head "*" was ours)" ]]
}

@test "rewritten, with someone else's commit on the remote: refused, even with --force" {
  inherit_main
  upstream theirs
  theirs=$(remote feat)
  git commit -q --amend -m "feat, reworded"
  run "$SD_ACT" push
  [ "$status" -eq 1 ]
  [[ "$output" == *"origin/feat has 1 commit this copy has never had; not pushing over it"* ]]
  [[ "$output" == *"git log feat..origin/feat"* ]]
  [[ "$output" == *"then rebase onto or merge origin/feat and push again"* ]]
  [ "$(remote feat)" = "$theirs" ]
  run "$SD_ACT" push --force
  [ "$status" -eq 1 ]
  [ "$(remote feat)" = "$theirs" ]
  [ "$(tracks feat)" = origin/main ]
}

@test "a pushed commit reset away locally: the remote head is ours, so it's force-pushed with a lease (#86)" {
  commit oops
  "$SD_ACT" push >/dev/null
  git reset -q --hard HEAD~1
  run "$SD_ACT" push
  [ "$status" -eq 0 ]
  [ "$(remote feat)" = "$(git rev-parse feat)" ]
  [[ "$output" == "force-pushed feat to origin (dropped 1 commit; the remote head "*" was ours)" ]]
}

@test "a soft reset (changes still in the worktree) is not a drop: refused, nothing pushed (#86)" {
  commit oops
  "$SD_ACT" push >/dev/null
  pushed=$(remote feat)
  git reset -q --soft HEAD~1
  run "$SD_ACT" push
  [ "$status" -eq 1 ]
  [[ "$output" == *"uncommitted or untracked changes"*"commit them"*"git reset --hard"* ]] || false
  [ "$(remote feat)" = "$pushed" ]
  git commit -qm "oops, redone"
  run "$SD_ACT" push
  [ "$status" -eq 0 ]
  [ "$(remote feat)" = "$(git rev-parse feat)" ]
}

@test "a mixed reset of a commit that added a file leaves it untracked: still not a drop (#86)" {
  echo new > new.txt && git add new.txt && git commit -qm "add new"
  "$SD_ACT" push >/dev/null
  pushed=$(remote feat)
  git reset -q HEAD~1
  [ "$(git status --porcelain)" = "?? new.txt" ]
  run "$SD_ACT" push
  [ "$status" -eq 1 ]
  [[ "$output" == *"git clean"* ]] || false
  [ "$(remote feat)" = "$pushed" ]
  rm new.txt
  run "$SD_ACT" push
  [ "$status" -eq 0 ]
  [[ "$output" == "force-pushed feat to origin (dropped 1 commit;"* ]]
}

@test "someone else's fast-forward taken, then reset away: not ours to drop, refused (#86)" {
  inherit_main
  upstream theirs
  theirs=$(remote feat)
  git fetch -q origin && git merge -q --ff-only origin/feat
  [ "$(git rev-parse feat)" = "$theirs" ]
  git reset -q --hard ORIG_HEAD
  run "$SD_ACT" push
  [ "$status" -eq 1 ]
  [[ "$output" == *"didn't make them all"*"git merge --ff-only origin/feat"* ]] || false
  [ "$(remote feat)" = "$theirs" ]
}

@test "someone's fast-forward taken, a commit on top, pushed, then both reset away: not ours to drop (#86)" {
  inherit_main
  upstream theirs
  git fetch -q origin && git merge -q --ff-only origin/feat
  commit mine
  "$SD_ACT" push >/dev/null
  pushed=$(remote feat)
  git reset -q --hard HEAD~2
  run "$SD_ACT" push
  [ "$status" -eq 1 ]
  [[ "$output" == *"didn't make them all"* ]] || false
  [ "$(remote feat)" = "$pushed" ]
}

@test "someone's commit taken with a fast-forward rebase, then reset away: not ours to drop (#86)" {
  inherit_main
  upstream theirs
  theirs=$(remote feat)
  git fetch -q origin && git rebase -q origin/feat
  [ "$(git rev-parse feat)" = "$theirs" ]
  git reset -q --hard HEAD~1
  run "$SD_ACT" push
  [ "$status" -eq 1 ]
  [[ "$output" == *"didn't make them all"* ]] || false
  [ "$(remote feat)" = "$theirs" ]
}

@test "someone's commit taken, then replayed by a restack and pushed, then reset away: still theirs, refused (#86)" {
  inherit_main
  upstream theirs
  git fetch -q origin && git merge -q --ff-only origin/feat
  (cd "$TMP/other" && git checkout -q main && git pull -q origin main && echo m >> main.txt && git add main.txt && git commit -qm m && git push -q origin main 2>/dev/null)
  git fetch -q origin && git rebase -q origin/main
  [ "$(git log -1 --format=%ae)" = them@example.com ]
  [ "$(git log -1 --format=%ce)" = t@example.com ]
  "$SD_ACT" push >/dev/null
  pushed=$(remote feat)
  [ "$pushed" = "$(git rev-parse feat)" ]
  git reset -q --hard HEAD~1
  run "$SD_ACT" push
  [ "$status" -eq 1 ]
  [[ "$output" == *"didn't make them all"* ]] || false
  [ "$(remote feat)" = "$pushed" ]
}

@test "a branch checked out nowhere has no worktree to be dirty: its own pushed commit is dropped (#86)" {
  commit oops
  "$SD_ACT" push >/dev/null
  git switch -q main
  git branch -qf feat feat~1
  run "$SD_ACT" push feat
  [ "$status" -eq 0 ]
  [ "$(remote feat)" = "$(git rev-parse feat)" ]
  [[ "$output" == "force-pushed feat to origin (dropped 1 commit;"* ]] || false
}

@test "a restacked branch's own commit, pushed then reset away, was made here: dropped (#86)" {
  (cd "$TMP/other" && git checkout -q main && echo m >> main.txt && git add main.txt && git commit -qm m && git push -q origin main 2>/dev/null)
  git fetch -q origin && git rebase -q origin/main
  commit oops
  "$SD_ACT" push >/dev/null
  git reset -q --hard HEAD~1
  run "$SD_ACT" push
  [ "$status" -eq 0 ]
  [[ "$output" == "force-pushed feat to origin (dropped 1 commit;"* ]] || false
}

@test "a restack past a colleague's squash-merged commit drops it from the branch: pushed with a lease (#86)" {
  inherit_main
  upstream theirs
  git fetch -q origin && git merge -q --ff-only origin/feat
  commit mine
  "$SD_ACT" push >/dev/null
  (cd "$TMP/other" && git checkout -q main && git pull -q origin main \
     && echo theirs >> other.txt && git add other.txt \
     && git -c user.email=them@example.com -c user.name=Them commit -qm "theirs (#1)" && git push -q origin main 2>/dev/null)
  git fetch -q origin && git rebase -q origin/main
  [ "$(git log --format=%s origin/main..feat)" = "$(printf '%s\n' mine feat)" ]
  run "$SD_ACT" push
  [ "$status" -eq 0 ]
  [ "$(remote feat)" = "$(git rev-parse feat)" ]
  [[ "$output" == "force-pushed feat to origin (rewritten;"* ]] || false
}

@test "taken, reset away, then a new commit: the rewrite path leases over the taken commit, as before #86 (only a pure drop checks who made it)" {
  inherit_main
  upstream theirs
  git fetch -q origin && git merge -q --ff-only origin/feat
  git reset -q --hard HEAD~1
  commit mine
  run "$SD_ACT" push
  [ "$status" -eq 0 ]
  [ "$(remote feat)" = "$(git rev-parse feat)" ]
  [[ "$output" == "force-pushed feat to origin (rewritten;"* ]] || false
}

@test "the remote is only ahead: refused, says how to take its commits" {
  inherit_main
  upstream theirs
  run "$SD_ACT" push
  [ "$status" -eq 1 ]
  [[ "$output" == *"origin/feat has 1 commit you don't"* ]]
  [[ "$output" == *"git merge --ff-only origin/feat"* ]]
  [ "$(tracks feat)" = origin/main ]
}

@test "the lease holds when the remote moves after the fetch" {
  # The hook runs after sd act has fetched and chosen its lease, before the
  # push lands: someone else's push sneaks in between.
  cat > "$(git rev-parse --git-path hooks)/pre-push" <<EOF
#!/usr/bin/env bash
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE
cd "$TMP/other" && git fetch -q origin && git checkout -q -B feat origin/feat \\
  && echo sneaky > sneaky.txt && git add sneaky.txt && git commit -qm sneaky && git push -q origin feat 2>/dev/null
EOF
  chmod +x "$(git rev-parse --git-path hooks)/pre-push"
  inherit_main
  git commit -q --amend -m "feat, reworded"
  run "$SD_ACT" push
  [ "$status" -eq 1 ]
  [[ "$output" == *"origin/feat moved while pushing; nothing pushed"* ]]
  [ "$(git -C "$TMP/other" log -1 --format=%s origin/feat)" = sneaky ]
  [ "$(tracks feat)" = origin/main ]
}

@test "taken over: refused unless --force" {
  commit two
  git config branch.feat.sd-lock chrismo
  run "$SD_ACT" push
  [ "$status" -eq 1 ]
  [[ "$output" == *"taken over"* ]]
  run "$SD_ACT" push --force
  [ "$status" -eq 0 ]
  [ "$(remote feat)" = "$(git rev-parse feat)" ]
}

@test "a named branch is pushed from wherever you are" {
  inherit_main
  commit two
  git switch -q main
  run "$SD_ACT" push feat
  [ "$status" -eq 0 ]
  [ "$(remote feat)" = "$(git rev-parse feat)" ]
  [ "$(tracks feat)" = origin/feat ]
  [ "$(tracks main)" = origin/main ]
}

@test "the remote comes from settings.remote" {
  git remote rename origin up
  commit two
  printf '{"settings":{"remote":"up"}}\n' > "$SHEEPDOG_CONFIG"
  git branch -q --set-upstream-to=up/main feat
  run "$SD_ACT" push
  [ "$status" -eq 0 ]
  [ "$output" = "pushed feat to up (1 commit)" ]
  [ "$(tracks feat)" = up/feat ]
}

# ── the upstream (#39) ───────────────────────────────────────────────────────
# A branch cut from origin/main tracks it; a successful push makes it track origin/<branch>.

@test "upstream: a fast-forward onto a branch tracking origin/main: now tracks origin/feat" {
  inherit_main
  commit two
  run "$SD_ACT" push
  [ "$status" -eq 0 ]
  [ "$output" = "pushed feat to origin (1 commit)" ]
  [ "$(tracks feat)" = origin/feat ]
}

@test "upstream: rewritten with a lease, tracking origin/main: now tracks origin/feat" {
  inherit_main
  git commit -q --amend -m "feat, reworded"
  run "$SD_ACT" push
  [ "$status" -eq 0 ]
  [ "$(remote feat)" = "$(git rev-parse feat)" ]
  [ "$(tracks feat)" = origin/feat ]
}

@test "upstream: nothing to push, tracking origin/main: now tracks origin/feat, HEAD untouched" {
  inherit_main
  head=$(git rev-parse feat)
  run "$SD_ACT" push
  [ "$status" -eq 0 ]
  [ "$output" = "nothing to push on feat" ]
  [ "$(tracks feat)" = origin/feat ]
  [ "$(git rev-parse feat)" = "$head" ]
  [ "$(remote feat)" = "$head" ]
}

@test "upstream: on the remote with no upstream at all: now tracks origin/feat" {
  git branch -q --unset-upstream feat
  commit two
  run "$SD_ACT" push
  [ "$status" -eq 0 ]
  [ "$(tracks feat)" = origin/feat ]
}

@test "upstream: nothing to push and no upstream: now tracks origin/feat" {
  git branch -q --unset-upstream feat
  run "$SD_ACT" push
  [ "$status" -eq 0 ]
  [ "$(tracks feat)" = origin/feat ]
}

@test "upstream: pushing a named branch from another worktree leaves that worktree's branch alone" {
  inherit_main
  commit two
  git worktree add -q -b side "$TMP/side" origin/main 2>/dev/null
  [ "$(tracks side)" = origin/main ]
  cd "$TMP/side"
  run "$SD_ACT" push feat
  [ "$status" -eq 0 ]
  [ "$(tracks feat)" = origin/feat ]
  [ "$(tracks side)" = origin/main ]
}

@test "upstream: the sd parent is not the upstream" {
  inherit_main
  git config branch.feat.sd-parent main
  commit two
  run "$SD_ACT" push
  [ "$status" -eq 0 ]
  [ "$(tracks feat)" = origin/feat ]
  [ "$(git config branch.feat.sd-parent)" = main ]
}
