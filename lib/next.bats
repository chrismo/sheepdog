#!/usr/bin/env bats

# Test suite for lib/next.sh: sd state JSON in, the ordered next steps out.
# Pure policy, so no git or gh: each test edits a "ready to merge" state.
#
# Needs bash 5 first on PATH (the lib is sourced):
#   PATH="/opt/homebrew/opt/bash/bin:$PATH" bats lib/next.bats

setup() {
  source "$BATS_TEST_DIRNAME/config.sh"
  source "$BATS_TEST_DIRNAME/next.sh"
  READY='{"branch": "feat", "head": "h1",
    "pr": {"number": 7, "url": "u", "state": "OPEN", "draft": false, "base": "main", "head_sha": "h1", "mergeable": "MERGEABLE",
           "merge_state": "CLEAN", "review_decision": null, "review_requests": []},
    "parent": {"name": "main", "source": "pr", "merged": false, "pr": null},
    "sync": {"target": "main", "default": "main", "ahead": 1, "behind": 0, "up_to_date": true, "old_base": "b0b0b0b0b0b0b0b0", "conflict": false},
    "push": {"remote_sha": "h1", "ahead": 0, "behind": 0, "diverged": false, "remote_ours": true},
    "fresh": true, "stale": [],
    "ci": {"sha": "h1", "for_head": true, "verdict": "green", "failing": [], "pending": []},
    "review": {"local": {"sha": "h1", "status": "covered"}, "bot": {"sha": null, "status": "none"},
               "accepted": {"sha": null, "status": "none"}},
    "threads": {"owed_bot": 0, "owed_human": 0, "outdated": 0, "blocking": 0, "info": 0, "items": []},
    "session": {"enrolled": true, "harness": null, "id": null, "locked": false},
    "errors": []}'
}

# next <jq-assignment...>: the next steps for READY with the edits applied.
next() {
  local s=$READY a
  for a; do s=$(jq -c "$a" <<<"$s"); done
  printf '%s\n' "$s" | prn_next origin
}
keys() { next "$@" | jq -r '[.[].key] | join(" ")'; }
step() { # step <key> <field> <jq-assignment...>
  local k=$1 f=$2; shift 2
  next "$@" | jq -r --arg k "$k" ".[] | select(.key == \$k) | .$f"
}

@test "ready: the only step is to merge, by chrismo" {
  [ "$(keys)" = ready ]
  [ "$(step ready who)" = you ]
  [ "$(step ready cmd)" = "gh pr merge" ]
}

@test "ready: only when GitHub would merge it (clean, unstable or has hooks)" {
  [ "$(keys '.pr.merge_state = "UNSTABLE"')" = ready ]
  [ "$(keys '.pr.merge_state = "HAS_HOOKS"')" = ready ]
  [ "$(keys '.pr.merge_state = "BLOCKED"')" = blocked ]
  [ "$(keys '.pr.merge_state = "UNKNOWN"')" = merge-wait ]
  [ "$(keys 'del(.pr.merge_state)')" = merge-wait ]
}

@test "review required: wait on the requested reviewers, never ready" {
  local req=('.pr.merge_state = "BLOCKED"' '.pr.review_decision = "REVIEW_REQUIRED"')
  [ "$(keys "${req[@]}" '.pr.review_requests = ["alice", "core-team"]')" = review-wait ]
  [ "$(step review-wait who "${req[@]}")" = wait ]
  [ "$(step review-wait say "${req[@]}" '.pr.review_requests = ["alice", "core-team"]')" \
    = "waiting on review from alice, core-team" ]
  [ "$(step review-wait say "${req[@]}")" = "waiting on a required review" ]
}

@test "review required stays listed behind other steps; ready-like ends don't" {
  [ "$(keys '.ci.verdict = "pending"' '.pr.merge_state = "BLOCKED"' '.pr.review_decision = "REVIEW_REQUIRED"')" \
    = "ci-wait review-wait" ]
  [ "$(keys '.ci.verdict = "pending"' '.pr.merge_state = "BLOCKED"')" = ci-wait ]
  [ "$(keys '.ci.verdict = "pending"' '.pr.merge_state = "UNKNOWN"')" = ci-wait ]
}

@test "stacked on an open parent PR: wait for the parent, never ready" {
  local st=('.pr.base = "p"' '.sync.target = "p"'
            '.parent = {"name": "p", "source": "pr", "merged": false, "pr": {"number": 25, "state": "OPEN", "head_sha": "x"}}')
  [ "$(keys "${st[@]}")" = parent-wait ]
  [ "$(step parent-wait who "${st[@]}")" = wait ]
  [ "$(step parent-wait say "${st[@]}")" = "waiting on p (#25) to merge first" ]
  [ "$(keys "${st[@]}" '.push.ahead = 1')" = "push parent-wait" ]
}

@test "the parent merged but the PR still targets it: retarget points it at the default branch" {
  local st=('.pr.base = "p"' '.parent = {"name": "p", "source": "pr", "merged": true, "pr": {"number": 25, "state": "MERGED", "head_sha": "x"}}')
  [ "$(keys "${st[@]}")" = retarget ]
  [ "$(step retarget who "${st[@]}")" = run ]
  [ "$(step retarget say "${st[@]}")" = "p already merged: point this PR at main" ]
  [ "$(step retarget cmd "${st[@]}")" = "sd act retarget" ]
}

@test "a parent that exists only here: ship it first; no push or open-pr until then (#105)" {
  local st=('.pr = null' '.push = {"remote_sha": null, "ahead": 2, "behind": 0, "diverged": false, "remote_ours": null}'
            '.parent = {"name": "p", "source": "config", "merged": false, "local": true, "on_remote": false, "unpushed": true, "pr": null}'
            '.sync = {"target": "p", "default": "main", "behind": null, "up_to_date": null, "conflict": null, "old_base": null}')
  [ "$(keys "${st[@]}")" = ship-parent ]
  [ "$(step ship-parent who "${st[@]}")" = run ]
  [ "$(step ship-parent say "${st[@]}")" = "ship the parent first: p isn't on origin yet" ]
  [ "$(step ship-parent cmd "${st[@]}")" = "sd act ship p" ]
  [ "$(keys "${st[@]}" '.parent.on_remote = true | .parent.unpushed = false')" = "push open-pr" ]
}

@test "the parent closed without merging: chrismo decides; no restack, never ready" {
  local st=('.pr.base = "p"' '.sync.target = "p"' '.sync.up_to_date = false' '.sync.behind = 2'
            '.parent = {"name": "p", "source": "config", "merged": false, "pr": {"number": 25, "state": "CLOSED", "head_sha": "x"}}')
  [ "$(keys "${st[@]}")" = parent-closed ]
  [ "$(step parent-closed who "${st[@]}")" = you ]
  [ "$(step parent-closed say "${st[@]}")" = "p (#25) was closed without merging: decide what this PR targets" ]
  [ "$(keys "${st[@]}" '.push.ahead = 1')" = "push parent-closed" ]
}

@test "a base branch with no PR of its own (a release branch) can still be ready" {
  [ "$(keys '.pr.base = "release"' '.sync.target = "release"' '.parent = {"name": "release", "source": "pr", "merged": false, "pr": null}')" = ready ]
}

@test "GitHub's BEHIND (required up-to-date branches): restack onto the default branch, a real step" {
  # Observed in sheepdog-sandbox 2026-10-14 (findings/6-behind.md): BEHIND needs a
  # required status check plus "require branches to be up to date", and then
  # wins over a required review. So it is a restack to run, not a blocked state.
  [ "$(keys '.pr.merge_state = "BEHIND"' '.sync.up_to_date = false' '.sync.behind = 3')" = behind ]
  [ "$(step behind who '.pr.merge_state = "BEHIND"' '.sync.up_to_date = false' '.sync.behind = 3')" = run ]
  [ "$(step behind say '.pr.merge_state = "BEHIND"' '.sync.up_to_date = false' '.sync.behind = 3')" \
    = "restack onto main (3 behind): GitHub requires branches to be up to date" ]
  [ "$(step behind cmd '.pr.merge_state = "BEHIND"' '.sync.up_to_date = false' '.sync.behind = 3')" = "sd act restack" ]
  # a required review on top still waits behind the restack
  [ "$(keys '.pr.merge_state = "BEHIND"' '.sync.up_to_date = false' '.sync.behind = 3' '.pr.review_decision = "REVIEW_REQUIRED"')" = "behind review-wait" ]
}

@test "blocked for another reason: chrismo looks on GitHub" {
  [ "$(step blocked who '.pr.merge_state = "BLOCKED"' '.pr.review_decision = "APPROVED"')" = you ]
  [ "$(step blocked say '.pr.merge_state = "BLOCKED"')" = "GitHub says the merge is blocked: see why" ]
  [ "$(keys '.pr.merge_state = "BEHIND"')" = behind ]
  [ "$(step blocked cmd '.pr.merge_state = "BLOCKED"')" = "gh pr view --web" ]
}

@test "every step has key, who, say and cmd" {
  run next '.push.ahead = 2' '.threads.owed_bot = 1' '.ci.verdict = "pending"'
  [ "$status" -eq 0 ]
  [ "$(jq '[.[] | keys == ["cmd", "key", "say", "who"]] | all' <<<"$output")" = true ]
  [ "$(jq '[.[].who | IN("you", "agent", "run", "wait")] | all' <<<"$output")" = true ]
}

@test "merged: only the cleanup step, whatever else is true" {
  [ "$(keys '.pr.state = "MERGED"' '.push.ahead = 3' '.ci.verdict = "red"' '.errors = ["x"]')" = merged ]
  [ "$(step merged cmd '.pr.state = "MERGED"')" = "sd act forget" ]
  [ "$(step merged say '.pr.state = "MERGED"')" = "merged: drop the branch's pins and enrollment, then remove this worktree" ]
}

@test "closed: only the closed step" {
  [ "$(keys '.pr.state = "CLOSED"' '.threads.owed_bot = 2')" = closed ]
}

@test "errors come first and point at --json" {
  [ "$(keys '.errors = ["gh: boom"]' '.push.ahead = 1')" = "errors push" ]
  [ "$(step errors say '.errors = ["a", "b"]')" = "2 errors: the steps below may be wrong" ]
  [ "$(step errors cmd '.errors = ["a"]')" = "sd state --json" ]
}

@test "stale refs: fetch, mechanically" {
  [ "$(step stale who '.fresh = false')" = run ]
  [ "$(step stale cmd '.fresh = false')" = "git fetch --prune origin" ]
  [ "$(step stale say '.fresh = false')" = "fetch: GitHub is ahead of your remote refs, so the steps below may be wrong" ]
}

@test "a push that isn't ours: chrismo looks, and no push is suggested" {
  [ "$(keys '.push.remote_ours = false' '.push.behind = 2' '.push.theirs = 2' '.push.ahead = 1' '.push.diverged = true')" = foreign ]
  [ "$(step foreign cmd '.push.remote_ours = false' '.push.behind = 2' '.push.theirs = 2')" = "git log HEAD..origin/feat" ]
  [ "$(step foreign say '.push.remote_ours = false' '.push.behind = 2' '.push.theirs = 2')" = "origin/feat has 2 commits this copy has never had" ]
  [ "$(step foreign say '.push.remote_ours = false' '.push.behind = 1' '.push.theirs = 1')" = "origin/feat has 1 commit this copy has never had" ]
}

@test "a push that isn't ours: our own pre-amend commits under it aren't counted" {
  [ "$(step foreign say '.push.remote_ours = false' '.push.behind = 2' '.push.theirs = 1')" = "origin/feat has 1 commit this copy has never had" ]
}

@test "attached: the session is open interactively; agent steps wait" {
  run next '.session.locked = true' '.threads.owed_bot = 2'
  [ "$(jq -r '.[0].key' <<<"$output")" = attached ]
  [ "$(jq -r '.[0].who' <<<"$output")" = wait ]
  [ "$(jq -r '.[0].cmd' <<<"$output")" = null ]
  [ "$(jq -r '.[1].key' <<<"$output")" = bot ]
}

@test "human items: chrismo replies, via the --long list" {
  [ "$(step human who '.threads.owed_human = 2')" = you ]
  [ "$(step human say '.threads.owed_human = 2')" = "reply to 2 human items" ]
  [ "$(step human say '.threads.owed_human = 1')" = "reply to 1 human item" ]
  [ "$(step human cmd '.threads.owed_human = 1')" = "sd state --long" ]
}

# A stacked branch: its parent is another branch, not the default.
STACKED=('.parent.name = "base"' '.sync.target = "base"' '.sync.up_to_date = false' '.sync.behind = 3')

@test "behind a stacked parent: sd act restack" {
  [ "$(step behind who "${STACKED[@]}")" = run ]
  [ "$(step behind say "${STACKED[@]}")" = "restack onto base (3 behind)" ]
  [ "$(step behind cmd "${STACKED[@]}")" = "sd act restack" ]
}

@test "a tree restack that conflicts is chrismo's: --resolve can't do it" {
  [ "$(step conflict who "${STACKED[@]}" '.sync.conflict = true' '.sync.by_tree = true')" = you ]
  [ "$(step conflict say "${STACKED[@]}" '.sync.conflict = true' '.sync.by_tree = true')" = "restack onto base by tree conflicts, which --resolve can't do: resolve it by hand (sd act restack says how)" ]
  [ "$(step conflict cmd "${STACKED[@]}" '.sync.conflict = true' '.sync.by_tree = true')" = "sd act restack" ]
}

@test "behind, by tree: the step says the commits become one" {
  [ "$(step behind say "${STACKED[@]}" '.sync.by_tree = true')" = "restack onto base by tree (3 behind): its commits become one" ]
  [ "$(step behind cmd "${STACKED[@]}" '.sync.by_tree = true')" = "sd act restack" ]
}

@test "a conflicting restack hands the agent the resolution" {
  [ "$(step conflict cmd "${STACKED[@]}" '.sync.conflict = true')" = "sd act restack --resolve" ]
  [ "$(step conflict who "${STACKED[@]}" '.sync.conflict = true')" = agent ]
}

# A plain rebase there would replay a rewritten parent's old commits (#97).
@test "behind with no old base: chrismo's, pointing at sd state's errors, no rebase" {
  [ "$(step behind who "${STACKED[@]}" '.sync.old_base = null')" = you ]
  [ "$(step behind say "${STACKED[@]}" '.sync.old_base = null')" = "restack onto base (3 behind): no base to restack from; see sd state's errors" ]
  [ "$(step behind cmd "${STACKED[@]}" '.sync.old_base = null')" = "sd state --json" ]
}

@test "only behind the default branch: no step; main moves too often to chase" {
  [ "$(keys '.sync.up_to_date = false' '.sync.behind = 40')" = ready ]
}

@test "behind the default branch with a conflict: the agent resolves it" {
  [ "$(keys '.sync.up_to_date = false' '.sync.behind = 40' '.sync.conflict = true')" = conflict ]
  [ "$(step conflict cmd '.sync.up_to_date = false' '.sync.conflict = true')" = "sd act restack --resolve" ]
}

@test "parent merged, retargeted to the default branch: restack" {
  [ "$(keys '.parent.name = "base"' '.parent.merged = true' '.sync.up_to_date = false' '.sync.behind = 2')" = behind ]
}

@test "a conflicting restack is the agent's, instead of a plain restack" {
  [ "$(keys "${STACKED[@]}" '.sync.conflict = true')" = conflict ]
  [ "$(step conflict who "${STACKED[@]}" '.sync.conflict = true')" = agent ]
}

@test "red CI: the agent fixes it; names the failing checks" {
  [ "$(step ci-red who '.ci.verdict = "red"' '.ci.failing = ["lint", "test"]')" = agent ]
  [ "$(step ci-red say '.ci.verdict = "red"' '.ci.failing = ["lint", "test"]')" = "fix CI: lint, test" ]
  [ "$(step ci-red cmd '.ci.verdict = "red"' '.ci.failing = ["lint", "test"]')" = "sd act ci-fix" ]
}

@test "red CI with unpushed commits, or on an old head: push first, or wait; no fix step" {
  [ "$(keys '.ci.verdict = "red"' '.ci.failing = ["test"]' '.push.ahead = 1')" = push ]
  [ "$(keys '.ci.verdict = "red"' '.ci.failing = ["test"]' '.ci.for_head = false')" = ci-wait ]
}

@test "blocked CI: chrismo's, since no agent fixes a check that didn't start or wants approval" {
  local b=('.ci.verdict = "blocked"' '.ci.blocked = ["wf", "deploy"]')
  [ "$(step ci-blocked who "${b[@]}")" = you ]
  [ "$(step ci-blocked say "${b[@]}")" \
    = "CI blocked: wf, deploy didn't start or want approval; an agent can't fix that" ]
  [ "$(step ci-blocked cmd "${b[@]}")" = "gh pr checks" ]
  [ "$(keys "${b[@]}" '.pr.draft = true' '.pr.merge_state = "DRAFT"')" = ci-blocked ]
  [ "$(keys "${b[@]}" '.push.ahead = 1')" = push ]
}

@test "bot items: sd act address" {
  [ "$(step bot who '.threads.owed_bot = 3')" = agent ]
  [ "$(step bot say '.threads.owed_bot = 3')" = "address 3 bot items" ]
  [ "$(step bot cmd '.threads.owed_bot = 3')" = "sd act address" ]
}

@test "bot items that are only info: chrismo reads them, then acks" {
  [ "$(keys '.threads.owed_bot = 2' '.threads.info = 2')" = read ]
  [ "$(step read who '.threads.owed_bot = 2' '.threads.info = 2')" = you ]
  [ "$(step read say '.threads.owed_bot = 2' '.threads.info = 2')" = "read 2 bot items in sd state --long, then ack" ]
  [ "$(step read cmd '.threads.owed_bot = 2' '.threads.info = 2')" = "sd act ack" ]
}

@test "past settings.branchBudget: one you step in place of the agent steps; run steps still run (#121)" {
  local over='.spend = {"usd": 25.456, "budget": 25}'
  [ "$(keys "$over" '.ci.verdict = "red"' '.ci.failing = ["test"]' '.threads.owed_bot = 2')" = budget ]
  [ "$(step budget who "$over" '.threads.owed_bot = 2')" = you ]
  [ "$(step budget say "$over" '.threads.owed_bot = 2')" \
    = 'spent $25.46 on this branch, past settings.branchBudget ($25.00): raise it, or take over' ]
  [ "$(step budget cmd "$over" '.threads.owed_bot = 2')" = "sd act attach" ]
  # a restack ahead of the agent's steps, and a push after them, stay
  [ "$(keys "$over" '.pr.merge_state = "BEHIND"' '.sync.up_to_date = false' '.sync.behind = 3' '.threads.owed_bot = 2')" = "behind budget" ]
  [ "$(keys "$over" '.threads.owed_bot = 1' '.push.ahead = 1' '.review.local.status = "uncovered"')" = "budget push" ]
  # nothing that spends: no budget step
  [ "$(keys "$over")" = ready ]
}

@test "past settings.branchBudget: open-pr is chrismo's too, since ship may describe (#121)" {
  local nopr=('.pr = null' '.push = {"remote_sha": null, "ahead": 2, "behind": 0, "diverged": false, "remote_ours": null}')
  [ "$(keys "${nopr[@]}")" = "push open-pr" ]
  [ "$(keys '.spend = {"usd": 30, "budget": 25}' "${nopr[@]}")" = "push budget" ]
  # the parent's ship spends on the parent, against its own budget
  [ "$(keys '.spend = {"usd": 30, "budget": 25}' "${nopr[@]}" \
            '.parent = {"name": "p", "source": "config", "merged": false, "local": true, "on_remote": false, "unpushed": true, "pr": null}' \
            '.sync = {"target": "p", "default": "main", "behind": null, "up_to_date": null, "conflict": null, "old_base": null}')" = ship-parent ]
}

@test "spend unknown (usage.sup unreadable) with a budget: held as if over it (#121)" {
  local unknown='.spend = {"usd": null, "budget": 25}'
  [ "$(keys "$unknown" '.threads.owed_bot = 2')" = budget ]
  [ "$(step budget say "$unknown" '.threads.owed_bot = 2')" \
    = "can't tell what this branch has spent (sd state's errors say why), so nothing spends: fix the usage record, or take over" ]
  # no budget, nothing to hold
  [ "$(keys '.spend = {"usd": null, "budget": 0}' '.threads.owed_bot = 2')" = bot ]
}

@test "within settings.branchBudget, or with none (0): the agent steps as usual" {
  [ "$(keys '.spend = {"usd": 24.9, "budget": 25}' '.threads.owed_bot = 2')" = bot ]
  [ "$(keys '.spend = {"usd": 99, "budget": 0}' '.threads.owed_bot = 2')" = bot ]
  [ "$(keys '.spend = null' '.threads.owed_bot = 2')" = bot ]
}

@test "blocking and info bot items together: address takes them all" {
  [ "$(keys '.threads.owed_bot = 3' '.threads.info = 1')" = bot ]
  [ "$(step bot say '.threads.owed_bot = 3' '.threads.info = 1')" = "address 3 bot items" ]
}

@test "unpushed commits: sd act push" {
  [ "$(step push say '.push.ahead = 2')" = "push 2 commits" ]
  [ "$(step push cmd '.push.ahead = 2')" = "sd act push" ]
}

@test "an agent's commit changed CI config: chrismo reviews, then pushes; no push step, no ci-fix (#124)" {
  local held='.push.held = {"sha": "h2", "paths": [".github/workflows/test.yml", ".rwx/tests.yml"]}'
  [ "$(keys '.push.ahead = 1' "$held")" = ci-config ]
  [ "$(step ci-config who '.push.ahead = 1' "$held")" = you ]
  [ "$(step ci-config say '.push.ahead = 1' "$held")" \
    = "the agent changed CI config (.github/workflows/test.yml, .rwx/tests.yml): review, then sd act push" ]
  [ "$(step ci-config cmd '.push.ahead = 1' "$held")" = "sd act push" ]
  [ "$(keys '.push.ahead = 1' '.ci.verdict = "red"' '.ci.failing = ["test"]' "$held")" = ci-config ]
  [ "$(keys '.pr = null' '.push = {"remote_sha": null, "ahead": 1, "behind": 0}' "$held")" = ci-config ]   # ship would refuse
  [ "$(keys '.push.ahead = 1' '.push.held = null')" = push ]
}

@test "CI config held where a push would be wrong (undone restack, foreign, parent unpushed): still shown, no sd act push offered (#124)" {
  local held='.push.held = {"sha": "h2", "paths": [".rwx/tests.yml"]}'
  [ "$(keys '.sync.undone = 1' '.sync.up_to_date = false' '.push.ahead = 1' "$held")" = "restack-undone ci-config" ]
  [ "$(step ci-config cmd '.sync.undone = 1' '.sync.up_to_date = false' '.push.ahead = 1' "$held")" = null ]
  [ "$(step ci-config cmd '.push.remote_ours = false' '.push.behind = 1' '.push.theirs = 1' '.push.ahead = 1' "$held")" = null ]
  [ "$(step ci-config cmd '.parent.unpushed = true' '.push.ahead = 1' "$held")" = null ]
  [ "$(step ci-config say '.sync.undone = 1' "$held")" = "the agent changed CI config (.rwx/tests.yml): review it; push once the steps above are done" ]
}

@test "a restack undone from outside sheepdog: redo it by hand, then sd act push (#47)" {
  [ "$(keys '.sync.undone = 1' '.sync.up_to_date = false' '.push.ahead = 1' '.push.behind = 0')" = restack-undone ]
  [ "$(step restack-undone who '.sync.undone = 1')" = you ]
  [ "$(step restack-undone say '.sync.undone = 1')" = "the restack was undone (1 of main's commits is a copy on feat; a plain git push after a restack?): redo it, and don't sd act push before that, which would publish the copies" ]
  [ "$(step restack-undone say '.sync.undone = 2')" = "the restack was undone (2 of main's commits are copies on feat; a plain git push after a restack?): redo it, and don't sd act push before that, which would publish the copies" ]
  [ "$(step restack-undone cmd '.sync.undone = 1' '.sync.old_base = null')" = "git rebase origin/main && sd act push" ]
  [ "$(step restack-undone cmd '.sync.undone = 1' '.sync.old_base = "0123456789abcdef0123"')" = "git rebase --onto origin/main 0123456789ab && sd act push" ]
  [ "$(keys '.sync.undone = 0')" = ready ]
  [ "$(keys '.sync.undone = 1' '.pr.merge_state = "BEHIND"')" = restack-undone ]
}

# On a stacked parent a plain rebase would replay a rewritten parent's old commits (#97).
@test "a restack undone on a stacked branch with no old base: no rebase offered, pin by hand (#97)" {
  local s=('.sync.undone = 1' '.sync.target = "base"' '.parent.name = "base"' '.sync.old_base = null')
  [ "$(step restack-undone who "${s[@]}")" = you ]
  [[ "$(step restack-undone say "${s[@]}")" == *"no base to redo it from on base: pin one by hand (git update-ref refs/sheepdog/feat/base <the commit feat left base at>), then sd act restack" ]] || false
  [ "$(step restack-undone cmd "${s[@]}")" = null ]
  local m=('.sync.undone = 1' '.parent.name = "base"' '.parent.merged = true' '.sync.old_base = null')
  [ "$(step restack-undone cmd "${m[@]}")" = null ]
  [[ "$(step restack-undone say "${m[@]}")" == *"no base to redo it from on main: pin one by hand (git update-ref refs/sheepdog/feat/base <the commit feat left base at>), then sd act restack" ]] || false
}

@test "the remote has commits this copy reset away (ours): force-push, which leases (#86)" {
  [ "$(step push say '.push.ahead = 0' '.push.behind = 1' '.push.remote_ours = true' '.push.remote_made = true')" = "force-push: drop 1 commit the remote has, reset away here" ]
  [ "$(step push cmd '.push.ahead = 0' '.push.behind = 2' '.push.remote_ours = true' '.push.remote_made = true')" = "sd act push" ]
  [ "$(keys '.push.ahead = 0' '.push.behind = 1' '.push.remote_ours = true' '.push.remote_made = null')" = ready ]
}

@test "commits taken from the remote and reset away: take them back, no drop (#86)" {
  local st=('.push.ahead = 0' '.push.behind = 1' '.push.remote_ours = true' '.push.remote_made = false')
  [ "$(keys "${st[@]}")" = take ]
  [ "$(step take who "${st[@]}")" = you ]
  [ "$(step take say "${st[@]}")" = "origin/feat has 1 commit this branch took but didn't make, reset away here: take it back (git merge --ff-only origin/feat), or drop it by hand" ]
  [ "$(step take say "${st[@]}" '.push.behind = 2')" = "origin/feat has 2 commits this branch took but didn't make, reset away here: take them back (git merge --ff-only origin/feat), or drop them by hand" ]
  [ "$(keys "${st[@]}" '.pr.draft = true' '.pr.merge_state = "DRAFT"')" = take ]
}

@test "the same reset with changes still in the worktree: chrismo commits or drops them, no push step (#86)" {
  [ "$(keys '.push.ahead = 0' '.push.behind = 1' '.push.remote_ours = true' '.push.remote_made = true' '.push.dirty = true')" = reset-dirty ]
  [ "$(step reset-dirty who '.push.ahead = 0' '.push.behind = 1' '.push.remote_ours = true' '.push.remote_made = true' '.push.dirty = true')" = you ]
  [ "$(step reset-dirty say '.push.ahead = 0' '.push.behind = 1' '.push.remote_ours = true' '.push.remote_made = true' '.push.dirty = true')" = "origin/feat has 1 commit reset away here, and the worktree has uncommitted changes: commit them (then sd act push replaces the remote commit), or remove them (git reset --hard; git clean for untracked files)" ]
  [[ "$(step reset-dirty say '.push.ahead = 0' '.push.behind = 2' '.push.remote_ours = true' '.push.remote_made = true' '.push.dirty = true')" == "origin/feat has 2 commits reset away here, and the worktree has uncommitted changes: commit them (then sd act push replaces the remote commits),"* ]]
  [ "$(keys '.push.ahead = 0' '.push.behind = 1' '.push.remote_ours = true' '.push.remote_made = true' '.push.dirty = false')" = push ]
  [ "$(keys '.push.ahead = 0' '.push.behind = 1' '.push.remote_ours = true' '.push.remote_made = true' '.push.dirty = null')" = push ]
}

@test "take or reset-dirty holds the pipeline: no bot trigger or CI wait after it, since nothing reaches GitHub until chrismo acts (#86)" {
  local remote_ahead=('.push.ahead = 0' '.push.behind = 1' '.push.remote_ours = true'
                      '.ci.sha = "r1"' '.ci.for_head = false' '.bot = {command: "/review-bot"}')
  [ "$(keys "${remote_ahead[@]}" '.push.remote_made = false')" = take ]
  [ "$(keys "${remote_ahead[@]}" '.push.remote_made = true' '.push.dirty = true')" = reset-dirty ]
  [ "$(keys "${remote_ahead[@]}" '.push.remote_made = true' '.push.dirty = false')" = "push" ]
}

@test "rewritten history of our own: sd act push, which leases" {
  [ "$(step push say '.push.ahead = 2' '.push.behind = 2' '.push.diverged = true' '.sync.ahead = 2')" = "force-push 2 commits (rewritten); a plain git push is rejected, and pulling would undo the rewrite" ]
  [ "$(step push cmd '.push.ahead = 2' '.push.behind = 2' '.push.diverged = true')" = "sd act push" ]
}

@test "rewritten after a restack: count the branch's own commits, not what came in from the target" {
  # After a restack onto a squash-merged parent, remote..HEAD also counts the
  # squash commit (#31). sync.ahead is <target>..HEAD: just the branch's own.
  [ "$(step push say '.push.ahead = 3' '.push.behind = 2' '.push.diverged = true' '.sync.ahead = 2')" = "force-push 2 commits (rewritten); a plain git push is rejected, and pulling would undo the rewrite" ]
  [ "$(step push say '.push.ahead = 3' '.push.behind = 2' '.push.diverged = true' '.sync.ahead = null')" = "force-push 3 commits (rewritten); a plain git push is rejected, and pulling would undo the rewrite" ]
}

@test "never pushed: sd act push" {
  [ "$(step push say '.push.remote_sha = null' '.push.ahead = 1' '.pr = null')" = "push: not on origin yet" ]
  [ "$(step push cmd '.push.remote_sha = null' '.push.ahead = 1' '.pr = null')" = "sd act push" ]
}

@test "no PR: push first, then open one" {
  [ "$(keys '.pr = null' '.push.remote_sha = null' '.push.ahead = 1')" = "push open-pr" ]
  [ "$(step open-pr cmd '.pr = null')" = "sd act ship" ]
}

@test "no PR because gh failed: don't suggest opening one" {
  [ "$(keys '.pr = null' '.threads.blocking = null' '.errors = ["gh: boom"]')" = errors ]
}

@test "draft: ready for review once CI is green and the review covers HEAD, and only then" {
  [ "$(keys '.pr.draft = true' '.pr.merge_state = "DRAFT"')" = draft ]
  [ "$(step draft who '.pr.draft = true')" = run ]
  [ "$(step draft cmd '.pr.draft = true')" = "gh pr ready" ]
  [ "$(step draft say '.pr.draft = true')" = "CI green and reviewed: mark the PR ready for review" ]
  [ "$(keys '.pr.draft = true' '.ci.verdict = "none"')" = draft ]
  [ "$(keys '.pr.draft = true' '.ci.verdict = "none"' '.ci.required = null')" = "" ]
  [ "$(keys '.pr.draft = true' '.ci.verdict = "none"' '.ci.required = []')" = draft ]
  [ "$(step draft say '.pr.draft = true' '.ci.verdict = "none"')" = "reviewed, no CI checks: mark the PR ready for review" ]
  [ "$(keys '.pr.draft = true' '.pr.merge_state = "DRAFT"' '.ci.verdict = "pending"')" = ci-wait ]
  [ "$(keys '.pr.draft = true' '.pr.merge_state = "DRAFT"' '.review.local = {sha: "x", status: "uncovered", uncovered: 1}')" = review ]
  [ "$(keys '.pr.draft = true' '.pr.merge_state = "DRAFT"' '.push.ahead = 1')" = push ]
  [ "$(keys '.pr.draft = true' '.pr.merge_state = "DRAFT"' '.push.behind = 1' '.push.remote_ours = true' '.push.remote_made = true')" = push ]
}

@test "no PR: ship opens one" {
  [ "$(step open-pr cmd '.pr = null' '.push.remote_sha = null')" = "sd act ship" ]
  [ "$(step open-pr who '.pr = null' '.push.remote_sha = null')" = run ]
}

@test "review behind HEAD: local review by the agent" {
  [ "$(step review who '.review.local = {sha: "x", status: "uncovered", uncovered: 2}')" = agent ]
  [ "$(step review cmd '.review.local = {sha: "x", status: "uncovered", uncovered: 2}')" = "sd act review" ]
  [ "$(keys '.review.local = {sha: "x", status: "rewritten-changed"}')" = review ]
  [ "$(keys '.review.local = {sha: "x", status: "unknown"}')" = review ]
}

@test "review findings at HEAD: the agent fixes them, not another review" {
  local st=('.review.local = {sha: "x", status: "uncovered", uncovered: 2}'
            '.review.findings = {sha: "h1", round: 1, n: 2, at_head: true, declined: false, cap: 3, items: []}')
  [ "$(keys "${st[@]}")" = review-fix ]
  [ "$(step review-fix who "${st[@]}")" = agent ]
  [ "$(step review-fix cmd "${st[@]}")" = "sd act implement" ]
  [ "$(step review-fix say "${st[@]}")" = "fix 2 review findings (round 1/3)" ]
  # findings on an older head: the new commits get reviewed
  [ "$(keys "${st[@]}" '.review.findings.at_head = false')" = review ]
}

@test "review findings at the cap, or declined by the agent: chrismo fixes or accepts" {
  local st=('.review.local = {sha: "x", status: "uncovered", uncovered: 2}'
            '.review.findings = {sha: "h1", round: 3, n: 1, at_head: true, declined: false, cap: 3, items: []}')
  [ "$(keys "${st[@]}")" = review-decide ]
  [ "$(step review-decide who "${st[@]}")" = you ]
  [ "$(step review-decide cmd "${st[@]}")" = "sd act review --accept" ]
  [ "$(step review-decide say "${st[@]}")" \
    = "1 review finding after 3 rounds: fix it, or accept HEAD as reviewed (sd state --long lists them)" ]
  [ "$(keys "${st[@]}" '.review.findings.round = 1' '.review.findings.declined = true')" = review-decide ]
  [ "$(step review-decide say "${st[@]}" '.review.findings.round = 1' '.review.findings.declined = true')" \
    = "the agent declined 1 review finding: fix it, or accept HEAD as reviewed (sd state --long lists them)" ]
}

@test "review findings on an older head, before any PR or pin: the next review is due" {
  [ "$(keys '.pr = null' '.review.local = {sha: null, status: "none"}' \
           '.review.findings = {sha: "x", round: 1, n: 1, at_head: false, declined: false, cap: 3, items: []}')" = "open-pr review" ]
}

@test "review findings an accepted pin covers: no review step" {
  [ "$(keys '.review.local = {sha: "x", status: "uncovered", uncovered: 2}' \
           '.review.findings = {sha: "h1", round: 3, n: 1, at_head: true, declined: false, cap: 3, items: []}' \
           '.review.accepted = {sha: "h1", status: "covered"}')" = ready ]
}

@test "bot configured and HEAD pushed and locally reviewed but not bot-reviewed: trigger it" {
  local st=('.bot = {command: "/review-bot"}' '.review.bot = {sha: null, status: "none"}')
  [ "$(keys "${st[@]}")" = bot-trigger ]
  [ "$(step bot-trigger who "${st[@]}")" = run ]
  [ "$(step bot-trigger cmd "${st[@]}")" = "sd act trigger" ]
  [ "$(step bot-trigger say "${st[@]}")" = "trigger the review bot (/review-bot)" ]
  # unpushed, or the local review behind: those first, no trigger yet
  [ "$(keys "${st[@]}" '.push.ahead = 1')" = push ]
  [ "$(keys "${st[@]}" '.review.local = {sha: "x", status: "uncovered", uncovered: 1}')" = review ]
  # the bot already covers HEAD, or no bot configured: no step
  [ "$(keys "${st[@]}" '.review.bot = {sha: "h1", status: "covered"}')" = ready ]
  [ "$(keys '.review.bot = {sha: null, status: "none"}')" = ready ]
  # a draft waits for the bot too
  [ "$(keys "${st[@]}" '.pr.draft = true' '.pr.merge_state = "DRAFT"')" = bot-trigger ]
}

@test "bot triggered, no reply yet: wait on the bot; not ready, not draft-ready" {
  local st=('.bot = {command: "/review-bot", timeout: 3600}' '.review.bot = {sha: "h1", status: "covered", triggered: "t", replied: false, waited: 60}')
  [ "$(keys "${st[@]}")" = bot-wait ]
  [ "$(step bot-wait who "${st[@]}")" = wait ]
  [ "$(step bot-wait say "${st[@]}")" = "waiting on the review bot (/review-bot)" ]
  [ "$(step bot-wait cmd "${st[@]}")" = null ]
  [ "$(keys "${st[@]}" '.pr.draft = true' '.pr.merge_state = "DRAFT"')" = bot-wait ]
}

@test "bot waiting and CI cancelled: the rerun (a run step) comes before the wait" {
  [ "$(keys '.bot = {command: "/review-bot", timeout: 3600}' '.review.bot = {sha: "h1", status: "covered", triggered: "t", replied: false, waited: 60}' \
            '.ci.verdict = "inconclusive"')" = "ci-rerun bot-wait" ]
}

@test "bot replied since the trigger: ready" {
  [ "$(keys '.bot = {command: "/review-bot", timeout: 3600}' '.review.bot = {sha: "h1", status: "covered", triggered: "t", replied: true, waited: 60}')" = ready ]
}

@test "bot silent past settings.botTimeout: chrismo looks, or triggers again" {
  local st=('.bot = {command: "/review-bot", timeout: 3600}' '.review.bot = {sha: "h1", status: "covered", triggered: "t", replied: false, waited: 4000}')
  [ "$(keys "${st[@]}")" = bot-silent ]
  [ "$(step bot-silent who "${st[@]}")" = you ]
  [ "$(step bot-silent say "${st[@]}")" = "the review bot hasn't answered in 66 min: check it, trigger it again, or accept HEAD as reviewed (sd act review --accept)" ]
  [ "$(step bot-silent cmd "${st[@]}")" = "sd act trigger --force" ]
}

@test "bot silent, but HEAD accepted: no wait on the bot; ready" {
  [ "$(keys '.bot = {command: "/review-bot", timeout: 3600}' '.review.bot = {sha: "h1", status: "covered", triggered: "t", replied: false, waited: 4000}' \
            '.review.accepted = {sha: "h1", status: "covered"}')" = ready ]
}

@test "a bot pin without a trigger time counts as reviewed, as before" {
  [ "$(keys '.bot = {command: "/review-bot", timeout: 3600}' '.review.bot = {sha: "h1", status: "covered", triggered: null, replied: null, waited: null}')" = ready ]
}

@test "review: a pin that covers HEAD means no review step" {
  [ "$(keys '.review.local = {sha: "x", status: "uncovered", uncovered: 2}' \
           '.review.accepted = {sha: "h1", status: "covered"}')" = ready ]
  [ "$(keys '.review.local = {sha: "x", status: "rewritten-same"}')" = ready ]
  [ "$(keys '.review.bot = {sha: "x", status: "uncovered", uncovered: 2}')" = ready ]
}

# ── reviews before the parent ───────────────────────────────────────────────

UNREVIEWED='.review.local = {sha: null, status: "none"}'
ON_OPEN_PARENT=('.pr.base = "p"' '.sync.target = "p"'
                '.parent = {"name": "p", "source": "pr", "merged": false, "pr": {"number": 25, "state": "OPEN", "head_sha": "x"}}')

@test "never reviewed: an open PR gets the review step, drafts too" {
  [ "$(keys "$UNREVIEWED")" = review ]
  [ "$(step review cmd "$UNREVIEWED")" = "sd act review" ]
  [ "$(keys "$UNREVIEWED" '.pr.draft = true' '.pr.merge_state = "DRAFT"')" = review ]
}

@test "never reviewed with no PR yet: ship first, no review step" {
  [ "$(keys "$UNREVIEWED" '.pr = null')" = open-pr ]
}

@test "never reviewed, but accepted or bot coverage counts" {
  [ "$(keys "$UNREVIEWED" '.review.accepted = {sha: "h1", status: "covered"}')" = ready ]
  [ "$(keys "$UNREVIEWED" '.review.bot = {sha: "h1", status: "covered"}')" = ready ]
}

@test "unreviewed stacked PR: review before waiting on the parent" {
  [ "$(keys "$UNREVIEWED" "${ON_OPEN_PARENT[@]}")" = "review parent-wait" ]
  # no bot trigger until the local review covers HEAD
  [ "$(keys "$UNREVIEWED" "${ON_OPEN_PARENT[@]}" '.bot = {command: "/review-bot"}')" = "review parent-wait" ]
}

@test "stacked, locally reviewed, bot configured but not run: trigger it before waiting on the parent" {
  [ "$(keys "${ON_OPEN_PARENT[@]}" '.bot = {command: "/review-bot"}')" = "bot-trigger parent-wait" ]
  # no bot configured: none required
  [ "$(keys "${ON_OPEN_PARENT[@]}")" = parent-wait ]
}

@test "stacked with outstanding feedback: address it before waiting on the parent" {
  [ "$(keys "${ON_OPEN_PARENT[@]}" '.threads.owed_bot = 1')" = "bot parent-wait" ]
  [ "$(keys "${ON_OPEN_PARENT[@]}" '.threads.owed_bot = 1' '.threads.info = 1')" = "read parent-wait" ]
  [ "$(keys "${ON_OPEN_PARENT[@]}" '.threads.owed_human = 1')" = "human parent-wait" ]
}

@test "stacked with a required human review: wait on the review before the parent" {
  [ "$(keys "${ON_OPEN_PARENT[@]}" '.pr.merge_state = "BLOCKED"' '.pr.review_decision = "REVIEW_REQUIRED"')" \
    = "review-wait parent-wait" ]
}

@test "stacked and fully reviewed: the open parent is all that's left" {
  [ "$(keys "${ON_OPEN_PARENT[@]}" '.bot = {command: "/review-bot"}' '.review.bot = {sha: "h1", status: "covered"}')" \
    = parent-wait ]
  [ "$(keys "$UNREVIEWED" "${ON_OPEN_PARENT[@]}" '.review.accepted = {sha: "h1", status: "covered"}')" = parent-wait ]
}

@test "inconclusive CI: rerun it" {
  [ "$(step ci-rerun who '.ci.verdict = "inconclusive"')" = run ]
}

@test "CI pending or not yet on HEAD: wait" {
  [ "$(step ci-wait who '.ci.verdict = "pending"')" = wait ]
  [ "$(keys '.ci.verdict = "pending"')" = ci-wait ]
  [ "$(keys '.ci.for_head = false')" = ci-wait ]
}

@test "CI waiting only on required checks to report: says which" {
  [ "$(step ci-wait say '.ci.verdict = "pending"' '.ci.missing = ["test"]')" \
    = "waiting on required check test to report" ]
  [ "$(step ci-wait say '.ci.verdict = "pending"' '.ci.missing = ["test", "ext"]')" \
    = "waiting on required checks test, ext to report" ]
  # A draft's missing check may never come (a workflow that skips drafts, or
  # filtered to other branches): the wait says how out, by hand.
  [ "$(step ci-wait say '.pr.draft = true' '.ci.verdict = "pending"' '.ci.missing = ["test"]')" \
    = "waiting on required check test to report; if it never runs on a draft, undraft by hand (gh pr ready)" ]
  [ "$(step ci-wait say '.ci.verdict = "pending"' '.ci.missing = ["test"]' '.ci.pending = ["lint"]')" = "CI running" ]
}

@test "unpushed commits: push, not wait for CI that can't run yet" {
  [ "$(keys '.push.ahead = 1' '.ci.for_head = false')" = push ]
  [ "$(keys '.push.ahead = 1' '.ci.verdict = "pending"')" = push ]
}

@test "no PR: no CI wait" {
  [ "$(keys '.pr = null' '.ci.for_head = null' '.ci.verdict = "none"')" = open-pr ]
}

@test "order follows the loop's Ordering (#79 Design record): restack, agent fixes, push, then waiting" {
  # ci-red yields to push while commits are unpushed (they may be the fix), so
  # it shows up only once the push is done.
  [ "$(keys '.fresh = false' '.threads.owed_human = 1' "${STACKED[@]}" \
           '.ci.verdict = "red"' '.threads.owed_bot = 1' '.push.ahead = 1' '.pr.draft = true' \
           '.review.local = {sha: "x", status: "uncovered", uncovered: 1}')" \
    = "stale human behind bot push review" ]
  [ "$(keys '.fresh = false' '.threads.owed_human = 1' "${STACKED[@]}" \
           '.ci.verdict = "red"' '.threads.owed_bot = 1' '.pr.draft = true' \
           '.review.local = {sha: "x", status: "uncovered", uncovered: 1}')" \
    = "stale human behind ci-red bot review" ]
}

@test "a state that isn't a record fails" {
  run bash -c 'source "$1/config.sh"; source "$1/next.sh"; echo "[]" | prn_next origin' _ "$BATS_TEST_DIRNAME"
  [ "$status" -ne 0 ]
}

@test "the remote and branch reach commands as data" {
  [ "$(printf '%s\n' "$READY" | jq -c '.push.remote_ours = false | .push.behind = 1 | .push.theirs = 1 | .branch = "a\"b"' \
        | prn_next 'up"x' | jq -r '.[] | select(.key == "foreign") | .cmd')" = 'git log HEAD..up"x/a"b' ]
}

# ── prn_lines: the NEXT line(s) ──────────────────────────────────────────────

lines_for() { # lines_for <one|all> <jq-assignment...>
  local mode=$1; shift
  next "$@" | prn_lines "$mode"
}

@test "lines: one NEXT line with who, say, command and a count of the rest" {
  run lines_for one '.push.ahead = 1' '.threads.owed_bot = 1'
  [ "$status" -eq 0 ]
  [ "${#lines[@]}" -eq 1 ]
  [ "$output" = "NEXT  agent  address 1 bot item  → sd act address  (+1 more)" ]
}

@test "lines: a step with no command has no arrow" {
  run lines_for one '.pr.state = "CLOSED"'
  [ "$output" = "NEXT  you    PR closed without merging" ]
}

@test "lines: all lists every step, the rest as then" {
  run lines_for all '.push.ahead = 1' '.threads.owed_bot = 1'
  [ "${#lines[@]}" -eq 2 ]
  [ "${lines[0]}" = "NEXT  agent  address 1 bot item  → sd act address" ]
  [ "${lines[1]}" = "then  run    push 1 commit  → sd act push" ]
}
