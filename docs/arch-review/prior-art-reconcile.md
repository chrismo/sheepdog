# Prior art for the reconcile engine

Read on 2026-10-30 for #348 (one gate function per verb, shared by NEXT and
the act verbs) and its sequencing comment, after chrismo asked whether any
other project has a reconcile engine sheepdog could reuse, and what ideas to
pull in. Companion to `docs/alternatives.md`, which compared orchestrators and
stack tools; this page reads the *decision* code of five tools that turn
observed PR state into a next action.

Method: four readers, one per target, read-only. Sortie and Symphony from the
shallow clones in `~/modev/pr-workflow-prior-art/` (Sortie 6e397dc,
2026-10-11; Symphony be10a1b, 2026-09-15). Kodiak, bors-ng and Renovate from
fresh `--depth 1` clones made for this read, into a scratch directory, nothing
run. Every file:line below is to those clones. "Observed" means read in the
code; anything inferred is marked. Nothing but sheepdog was run.

## Answers

**Is there a reconcile engine to reuse?** No. Go, Elixir, Python and
TypeScript, each bound to its own state model, tracker and persistence. More
useful than the no: none of the five has what #348 describes. Every one ends
up with a long, priority-ordered evaluator; the only difference is whether
there is one copy of the guards or two.

| Tool | One evaluator? | Pure? | Guards duplicated? | Verdict shape |
|---|---|---|---|---|
| Kodiak | Yes, `mergeable` (740 lines) | No: calls the GitHub port inline | No | One action per pass; returns nothing |
| Renovate | Yes, `processBranch` (1070 lines) | No | Yes, by re-checking memoized facts | 19-value flat result union, a report-out |
| Sortie | No: 11 ordered per-kind passes | No | Yes, armed at worker exit and re-checked per pass | Per-kind; one 4-way triage gate is pure |
| Symphony | No: one GenServer, split by trigger | Mostly | Partly: one shared predicate, three copies of the branching | 4-way reconcile `cond` |
| bors-ng | No: a GenServer per project | Core fold is pure | No | `patch_preflight`: booleans then one ordered match |

So the long ordered evaluator is intrinsic to the problem. What sheepdog can
choose is to have one, pure, with its order as data, and tests shaped like
Kodiak's.

**What to pull in.** Nine ideas survived the four readers; the first four
belong in the #348 scaffold, two are filed as issues (#356, #357), the rest
are notes on existing issues. The per-tool sections hold the evidence.

| # | Idea | From | Where it lands |
|---|---|---|---|
| 1 | Two-phase gate: compute every predicate into a struct, then one ordered match to the verdict | bors-ng `patch_preflight`; Sortie `reactionTriageGate` | #348 step 1, #335's `Derived` |
| 2 | Tests: a `State` builder with defaults, one field overridden per test, asserting what was not offered too | Kodiak `create_mergeable` + recording fake | #348 parity test; #351 |
| 3 | Re-observe immediately before acting; the gate takes fresh state, not NEXT's | Symphony `revalidate_issue_for_dispatch`; Kodiak refetch per pass | #348 step 2 onward; check every verb uses `state::observe`'s result |
| 4 | `Done` derives from observed state, never a local marker | Kodiak checks `bot_reviews` before approving | #348; invariant I11 as a gate rule |
| 5 | Author-set foreign-modification test over `base..head`, cached by head SHA | Renovate `isBranchModified` | #356 |
| 6 | World-changed abort: non-open PR, reopened PR, stale-lease rejection stop the pass, not one verb | Renovate `REPOSITORY_CHANGED`; bors-ng `:race` | #348 (a `Hard` the table short-circuits on) |
| 7 | Fingerprint richer than SHA for address: sorted unresolved thread IDs | Sortie `buildReviewFingerprint` | #357 (check first) |
| 8 | An explicit verb registry the parity test iterates, so a new verb cannot escape it | Sortie `reactionKindPins` | #341 |
| 9 | CI fold as an exhaustive lattice table, cancelled as a fourth value | bors-ng `State.summary_states` | O5 in #340's split of observe |

Skipped, with reasons: a flat result enum returned from one long function
(Renovate's is a dashboard report, not a guard the action reuses: the
two-copies shape #348 leaves); an evaluator that calls the API port itself
(Kodiak: the port buys testability that `fn gate(&State)` gets free);
in-memory retry backoff and attempt counters (Symphony, Sortie: lost on
restart, and sheepdog keeps no state outside git); golden op-replay fixtures
(Sortie: a single-shot CLI over one JSON record is already table-driven);
Sortie's fixed-order per-kind passes with ordering comments (the priority
table makes that order data); a normalized PR-body hash before editing
(sheepdog doesn't maintain PR bodies).

## Kodiak (chdsbd/kodiak, Python)

A GitHub App that merges PRs when their conditions hold. The closest thing
found to a single evaluator.

**Evaluator.** `async def mergeable(...)`, `bot/kodiak/evaluation.py:636-1376`,
740 lines. Inputs are about 20 keyword arguments, each a frozen pydantic model
fetched up front: `PullRequest`, `BranchProtectionRule`, lists of reviews,
contexts, check runs and commits, config, repository, plus mode flags
`merging` and `is_active_merge` and a retry budget. The snapshot is the
argument list, not a struct. Output is `None`. Ordering is one long chain of
`if ...: <act>; return`, explicitly prioritized (the comment at 761-762 says
config errors come first): config errors, disable label, paywall, GitHub
merge queue, UNKNOWN mergeability (trigger a recompute and requeue),
auto-approve, branch update, missing label, conflict, blocking labels and
title, draft, merged, closed, BLOCKED/BEHIND diagnosis, do-not-merge, then
merge or enqueue. Exactly one action path runs per pass.

**Purity.** Not pure. It takes `api: PRAPI`, a Protocol of about 15
side-effect verbs (`evaluation.py:285-321`: dequeue, requeue, set_status,
merge, update_branch, create_comment, add_label, ...) and calls them inline.
The separation is at the port, not at an action value. The real port is
`PRV2` (`bot/kodiak/pull_request.py:196-472`); `evaluate_pr`
(`pull_request.py:89-190`) refetches the whole snapshot with `get_pr` and
re-runs `mergeable` on `PollForever`, `RetryForSkippableChecks` and
`ApiCallException`, passing the retry budget back in as an input.

**Idempotency against other writers.**

- Status is a GitHub check run keyed on `head_sha`
  (`bot/kodiak/queries/__init__.py:1486-1502`); re-posting the same message
  is harmless, so Kodiak never dedupes it. Every status is a function of the
  current snapshot, re-derived each pass.
- Approval: it checks observed `bot_reviews` for an existing APPROVED by its
  own login before approving (`evaluation.py:875-884`). "Already done" is
  read from observed state.
- Conflict comment: posted only while the automerge label is present, and
  the label is removed in the same step (`evaluation.py:946-963`); the label
  is the seen marker, so a rerun finds no label and posts nothing.
- Merge queue: a Redis sorted set with `zadd nx=True`, so re-enqueue never
  moves a PR's position (`bot/kodiak/queue.py:708-714`); `is_active_merge` is
  a Redis marker compared by value (`queue.py:471-474`) so the webhook-driven
  pass doesn't clobber statuses from the merge-loop pass
  (`evaluation.py:670-674`).
- Stale SHA: the snapshot is refetched at the start of every pass and the
  evaluator never reads cached state. Inferred: this plus idempotent GitHub
  operations is the whole story; there is no fingerprint.

**Tests.** `bot/kodiak/test_evaluation.py` (83 tests) plus
`bot/kodiak/tests/evaluation/*.py` (54 more). `create_mergeable()`
(`test_evaluation.py:360-417`) wraps `mergeable` with a default for every
argument from `create_pull_request()`, `create_branch_protection()`,
`create_config()` and friends (231-325). Each test overrides one or two fields
and asserts on `MockPrApi` (177-222), which records every call per verb:
`api.merge.called is False`, `api.set_status.calls[0]["msg"]`. Each test
also asserts the negative (update_branch, merge, queue_for_merge not called).
No property tests.

**For sheepdog.** Ideas 2 and 4 above. The test shape maps directly onto a
gate test: build a `State` from defaults, override one field, assert the
`Gate` and that NEXT offers nothing else. If the builder is generated from
the `State` type that `state.rs` emits, the fixture-key drift in #351 cannot
recur. The "evaluator calls the port" design is skipped: it is the
conflation #348 removes.

## bors-ng (bors-ng/bors-ng, Elixir)

A merge queue: batches approved PRs onto a staging branch, waits for CI,
merges or bisects.

**Evaluator.** None. Reconciliation is a GenServer per project
(`lib/worker/batcher.ex`) with two entry paths that converge: the webhook
`{:status, {commit, identifier, state, url}}` (`batcher.ex:179-196`) looks
the batch up by SHA and updates one Status row; the timer `{:poll, _}`
(230-244, then `poll_` 286-305) reloads every incomplete batch from the
database and calls `poll_running_batch` (665-676), which refetches commit
statuses from GitHub. Both end in `maybe_complete_batch` (678-706).

The pure core is `Batcher.State.summary_states/1`
(`lib/worker/batcher/state.ex:20-35`): a 16-cell lattice fold over per-check
states `:waiting | :running | :ok | :error` into one batch verdict. The
closest thing to a gate is `patch_preflight/3` (`batcher.ex:963-1028`): it
computes seven booleans, then one `case` over the tuple returns
`{:ok, max} | :waiting | {:error, reason}`, ordered by pattern priority.

**Purity.** `State` and `Batcher.Message.generate_status` /
`generate_message` (`lib/worker/batcher/message.ex`) are pure; everything
else calls `GitHub.*` inline. GitHub is a GenServer behind
`GitHub.ServerMock` (`lib/github/github/server_mock.ex`) in tests.

**Idempotency.** Batch and status rows are keyed on the staging commit SHA
(`Batch.get_assoc_by_commit`, `lib/database/batch.ex:135`); a status webhook
for an unknown SHA is dropped (`[] -> :ok`, `batcher.ex:194`). Before
merging, `pr.head_sha != patch.commit -> :race` (`batcher.ex:426-427`).
Timing is persisted, not in-process: `last_polled` and `timeout_at` columns
with `next_poll_is_past` / `timeout_is_past` (`batch.ex:142-157`).
Double-review is caught by state (`Patch.all(:awaiting_review)` nil gives
`:already_running_review`, `batcher.ex:138-146`). Comments are not deduped;
a single-writer registry (`check_self`, 1261-1268) is the guard. Observed:
comment dedupe relies on process exclusivity, not observed state.

**Tests.** `test/batcher/batcher_test.exs` (54), `state_test.exs` (4, an
exhaustive lattice table), `message_test.exs` (23), `attemptor_test.exs`
(14). Pattern: `ServerMock.put_state(%{...})` a whole fake GitHub map, insert
database rows, call `Batcher.handle_info({:poll, :once}, id)` or
`handle_cast` directly, then assert the full `ServerMock.get_state()` map by
equality. The clock is driven by setting `last_polled: 0`.

**For sheepdog.** Idea 1 (`patch_preflight`'s shape is the gate shape: all
predicates first, then one ordered match, so "which refusal wins" is explicit
and the same for every verb), idea 9 (the lattice fold with an exhaustive
table test fits O5's CI fold, with cancelled-is-inconclusive as a fourth
value), and idea 6 (`:race` on a moved head). Persisting timing in state so
tests set the clock by editing state is worth considering for the bot-reply
wait in O6.

## Renovate (renovatebot/renovate, TypeScript)

Reconciles hundreds of dependency-update branches and PRs against desired
state every run. The mature part is its handling of other writers.

**`processBranch`.** One 1070-line function of ordered early returns, not a
table: `lib/workers/repository/update/branch/index.ts:158-1231`. Roughly 30
`return { branchExists, prNo?, result }` sites plus a 100-line catch block
mapping error strings to results (946-1045). Order: branch and PR existence
(165-240), pending/approval/limits (242-312), stop-updating label (314-336),
foreign modification (370-427), schedule (429-456), minimum release age and
confidence (460-590), rebase decision (595-647), compute and commit
(650-835), status and automerge (876-945), `ensurePr` (1047-1210).

`BranchResult` is a string union of 19 values (`lib/workers/types.ts:141-159`):
already-existed, automerged, done, error, needs-approval, needs-pr-approval,
not-scheduled, no-work, pending, pr-created, pr-edited, pr-limit-reached,
commit-per-run-limit-reached, commit-hourly-limit-reached,
branch-limit-reached, rebase, update-not-scheduled,
minimum-group-size-not-met. The return also carries `prBlockedBy`, `prNo`,
`commitSha`, `updatesVerified` (149-156). The caller `writeUpdates`
(`lib/workers/repository/process/write.ts:146-254`) loops branches, calls
`syncBranchState` then `processBranch`, copies the result onto the branch
object, writes the new SHA into the branch cache, and short-circuits the
whole run on `automerged` because the base moved (222-228). The other
consumer is the Dependency Dashboard, which buckets branches by result
(`dependency-dashboard.ts:505-522`). So the enum is a report-out, not a
dispatch table.

**Foreign-modification detection.** `isBranchModified`,
`lib/util/git/index.ts:983-1090`: `git log origin/<base>..origin/<branch>`
with `%ae` and `%ce`, collect every author and committer email, remove
`gitAuthorEmail`, the configured `ignoredAuthors` (literal, regex or glob)
and `platformIgnoredAuthors` (platform-supplied bot emails, 248, 427). Any
remaining email means modified. A pure author-set test: no commit count, no
SHA comparison. Three-level memo: in-process `config.branchIsModified[branch]`
(992), then the repo cache keyed on branch name and current branch SHA
(`modified-cache.ts:5-18`, a hit only if `branch.sha === branchSha`), then
git. A successful push by Renovate resets the in-process flag (1605).

What it refuses when modified (`update/branch/index.ts:383-427`): with an
open PR it posts or keeps a sticky "Edited/Blocked" comment
(`handle-existing.ts:57-95`) and returns `pr-edited` with no rebase, no
commit and no PR edit, unless the user asked for a rebase via the PR checkbox,
a label or the dashboard. With no open PR but a modified branch it looks for a
closed PR and overrides the branch only if that PR's recorded SHA equals the
branch SHA (407-424). In `shouldReuseExistingBranch` (`reuse.ts:45-54`) a
behind-base branch that is modified is kept as is rather than rebased. A
user-changed target branch counts as modification (`index.ts:141-147`, 383).
Conflicted and modified: logs, does nothing (`reuse.ts:76-78`).

Race protection: `pushCommit` uses `--force-with-lease`
(`lib/util/git/index.ts:1548`); a `[rejected] (stale info)` becomes
`REPOSITORY_CHANGED` (1611), which `processBranch` rethrows (957-960) and
aborts the whole repo run. A PR flipping to non-open mid-run throws the same
(377-382), as does a closed PR found reopened (`check-existing.ts:44-48`).

**Idempotency.** `prepareCommit` returns null, no push, when
`git diff HEAD origin/<branch>` is empty unless `force`
(`lib/util/git/index.ts:1510-1516`); empty commits are dropped (1500-1504).
`force` is set when the user requested a rebase, the branch is new, or the
branch is conflicted (`update/branch/index.ts:779-784`). Upstream of git,
`cacheFingerprintMatch === 'matched'` with `reuseExistingBranch` skips the
whole update computation (650-652); the fingerprint hashes the filtered
upgrade config plus manager hashes (`write.ts:35-80`, 190-205). PR body:
`hashBody` strips the debug comment, Reviewable block, emojis, whitespace and
`#`, then sha256 (`lib/modules/platform/pr-body.ts:25-36`); `ensurePr` skips
`updatePr` when target branch, stripped title, body hash and labels all match
(`update/pr/index.ts:421-448`).

Cache: `BranchCache` (`lib/util/cache/repository/types.ts:70-134`) is keyed
by branch name and stores `sha`, `baseBranch`, `baseBranchSha`,
`isBehindBase`, `isConflicted`, `isModified`, `commitFingerprint`, `prCache`.
`syncBranchState` (`write.ts:84-144`) invalidates: a base name change drops
`isModified`; a base SHA change drops `isBehindBase` and `isConflicted`; a
branch SHA change drops all four. Each getter re-checks the SHAs it depends
on (`behind-base-branch-cache.ts:16-24`, `conflicts-cache.ts:12-20`).

**Tests.** `index.spec.ts` is 4015 lines, 121 tests in 5 describes. Every
collaborator module is mocked (56-75); `beforeEach` builds one config from
the production defaults builder (140-166) and each test overrides fields or
mock returns, then asserts the exact return object. No fixture files.
`reuse.spec.ts` is 30 tests of the same style.

Inferred, not observed: `processBranch`'s guards and the per-step functions
re-check some of the same facts (`isBranchModified` is called at `index.ts:371`
and again at `reuse.ts:49,69`); no parity test was found. The same
duplication sheepdog is removing, mitigated by memoization.

**For sheepdog.** Idea 5 (#356): the author-set test is cheap, independent of
which clone pushed, and catches what the reflog cannot. Idea 6: a `Hard`
"world changed" verdict that stops the pass for the branch. The ordering of
Renovate's early returns (existence, pending and limits, foreign
modification, schedule, rebase decision, commit) is a reasonable sanity check
on the priority order of sheepdog's verb table. `syncBranchState`'s
invalidation matrix (which cached fact depends on head SHA, base SHA or base
name) is the cleanest extractable idea for a per-fact cache if O3's
behind/conflicted ever needs memoizing across passes.

## Sortie (sortie-ai/sortie, Go)

An orchestrator driving coding agents against tracker tickets, reacting to
CI, review, bot review, merge conflict and auto-merge events. The nearest
design relative of sheepdog, and it has #348's problem.

**Observed state.** No single assembled record. Runtime state is one
in-memory `State` struct (`internal/orchestrator/state.go:812`) of maps keyed
by issue id: `Running`, `RetryAttempts`, `Claimed`, `Parked`,
`PendingReactions` (key `issueID:kind`, `state.go:451`), `ReactionAttempts`,
`ReactionHandedOffComments`. A `PendingReaction` (`state.go:459`) carries
identity, timers and backoff counters plus kind-specific data (`CIReactionData`,
`state.go:534`: PR number, owner, repo, branch, SHA, from `scm.json`). Forge
facts are fetched per pass, not stored: `SCMAdapter.GetMergeability` returns
`domain.PRMergeStatus` (`internal/domain/scm.go:74`: ReviewDecision,
CIConclusion, Draft, Mergeability, HeadSHA, BaseBranch, Merged,
MergeCommitSHA), plus `FetchPendingReviews`, `GetReviewDecision`,
`GetCIStatus`. Durable dedupe lives in SQLite (`reaction_fingerprints`,
`internal/persistence/reaction_fingerprints.go:21-145`).

**Deciding the next action.** Not one evaluator. `ReconcileRunningIssues`
(`reconcile.go:227-293`) is a fixed-order sequence of about 11 per-kind
passes (overdue retries, stalled, tracker state, CI, review, bot review,
merge conflicts, auto-merge, label review, label fix, merge completion); the
comments admit the ordering dependencies ("Runs LAST so the CI and review
passes have already updated"). Each pass loops over `PendingReactions` with
inline fetch-then-branch logic, e.g. `reconcileCIStatus`
(`ci_reconcile.go:58-333`) and `reconcileAutoMerge`
(`auto_merge_reconcile.go:37-280`, a chain of draft / mergeability unknown /
not clean / review not approved / CI pending / CI not green / head empty,
each deferring). Decision and IO are interleaved. The only pure verdict
functions are `classifyHeadChange` (`head_change.go:40`, an attribution enum)
and `reactionTriageGate` (`reaction_triage.go:512`, returning
`triageProceed / Wait / Handled / Escalate`, 412-436). Docs 07 describe an
issue state machine (Unclaimed, Claimed, Running, RetryQueued, Released); the
code has those as map-membership checks, not an enum.

**Preconditions.** Ad hoc ifs, duplicated at two points: worker exit arms a
reaction only if adapters exist and `scm.json` has PR identity
(`exit.go:915-990`, repeated per kind at 940, 984, 1023, ...); each reconcile
pass then re-checks adapters and config at its top (`ci_reconcile.go:59`,
`auto_merge_reconcile.go:38-43`); `recovery.go:307-394` re-seeds the same
entries at startup. The only registry is `reactionKindPins` (`state.go:421`),
a map kind to bool for "pins the workspace against sweep"; it declares no
guards. So Sortie has the two-copies shape #348 names, mitigated by
convention.

**Idempotency, dedupe, retry.** Per kind: fingerprint upsert, then read
`(stored, dispatched)`; same fingerprint and dispatched means skip
(`review_reconcile.go:155-172`; the auto-merge fingerprint is head SHA plus
review decision, `auto_merge_reconcile.go:312`). CI's fingerprint is the head
SHA and the read precedes the write so a head change is detectable
(`ci_reconcile.go:179-230`); `applyCIEpochTransition` (`ci_reconcile.go:345`)
resets backoff and the once-per-head escalation flag unconditionally but
resets the retry budget `ReactionAttempts` only when `classifyHeadChange`
says the change was not ours (no running, retry or claim, and zero completed
worker runs since `HeadRecordedAt`, `head_change.go:40-65`), matching
ADR-0024. Budget: `handleCIFailure` increments `ReactionAttempts[rkey]`, and
over `MaxRetries` escalates once per head (`ci_reconcile.go:410-419`). Review
dedupe is a persisted handed-off comment-ID set plus a reported set plus the
running turn's IDs; a set with no new ID never redispatches
(`settleHandedOffCommentSet`, `carriesNewComment`,
`review_reconcile.go:496-566`). Cross-kind exclusion is a single retry slot
per issue (`retrySlotIncumbent`, `dispatch.go:497`).

**Tests.** Per-pass unit tests calling the pass directly with a hand-built
`State` and hand-rolled fakes (`mockSCMAdapter`, `mockCIProvider`,
`ciReconcileStore`, about 25 fake types), e.g. `ci_reconcile_test.go:356-470`
(63 tests in that file). Table-driven `tests := []struct` appears in 37 files,
mostly for small helpers. One golden-fixture harness replays scripted steps
and diffs tracker and notify operations against JSON
(`golden_notify_test.go:1-60`, `testdata/golden_notify/*.json`). No parity
test across kinds.

Observed versus claimed: the state-machine description (docs 07) and the
re-arm table (ADR-0024) are only partly mirrored by the code; auto-merge
re-derives every pass rather than tracking epochs as the ADR says.

**For sheepdog.** Idea 1 (`reactionTriageGate`'s four-way verdict consumed by
the caller's switch is sheepdog's `Gate`, kept pure), idea 8 (make the verb
table the registry the parity test iterates), idea 7 (#357). Worth
considering: reset a retry budget only on positive evidence the head change
was not ours (`classifyHeadChange`); sheepdog's O4 reflog already attributes
pushes, so wire that attribution into any budget reset rather than resetting
on any SHA move. The cleanest extractable idea is the fingerprint protocol:
one durable `(subject fingerprint, dispatched)` per (branch, verb), where a
fingerprint change re-arms and a matching dispatched row refuses; sheepdog's
SHA pins are the special case.

## Symphony (openai/symphony, SPEC.md plus Elixir reference)

A ticket-to-agent dispatcher with a reconcile loop and no durable database.
Its spec is the clearest statement of reconcile-as-design among the five.

**The loop (spec).** §8.1 tick: reconcile running issues first, then validate
config, fetch candidates by `active_states`, sort (§8.2), dispatch while slots
remain. Desired state derives entirely from the tracker: an issue in an active
state with the required labels and `dispatchable=true` should have exactly
one worker; terminal means no worker and the workspace removed; non-active or
unroutable means no worker, workspace kept (§8.5 Part B, §14.4). Gap-closing
is one `cond` per running issue (§16.3) plus dispatch for eligible unclaimed
issues. Duplicate work is avoided only in memory: `claimed` / `running` checks
are REQUIRED before launch (§7.4) and the orchestrator is the single mutator
(§7). No durable database by design (§14.3): after a restart, timers and
sessions are gone and the service recovers by terminal-workspace cleanup,
fresh polling and re-dispatch. The tracker is the only cross-restart lock;
the agent moves the issue to a handoff state like `Human Review` through
tracker tools (§1, §11.5), which takes it out of `active_states`.

**The code.** Not one pure evaluator. One GenServer,
`elixir/lib/symphony_elixir/orchestrator.ex`, with the decision split by
trigger. Dispatch: `maybe_dispatch` (256-308), `choose_issues` (781),
`should_dispatch_issue?` (816-831: claimed, running, blocked, slots) over
`candidate_issue?` (853-870: fields present, routable, active, not terminal).
Reconcile per running issue: `reconcile_issue_state` (421-443: terminal
terminates and cleans up; unroutable terminates; active refreshes the
snapshot; else terminate). A near-copy for blocked issues:
`reconcile_blocked_issue_state` (456-476). Retry: `handle_retry_issue_lookup`
(1113-1136) and `handle_active_retry` (1181-1221) re-check
`retry_candidate_issue?` (equal to `candidate_issue?`) and slots. So one guard
concept (active, routable, not terminal) is reused through `candidate_issue?`,
but the surrounding terminate / release / dispatch branching is written three
times (running, blocked, retry): #348's drift risk in a smaller form. Every
dispatch re-fetches the single issue right before launch
(`revalidate_issue_for_dispatch`, 1006-1024, called from `dispatch_issue`,
907).

**Retry, stall, takeover.** §8.4: a normal exit gets a 1 s continuation
retry; a failure gets `min(10000 * 2^(attempt-1), max_retry_backoff_ms)`
(`failure_retry_delay`, 1240-1243, power capped at 10). A retry firing
re-fetches by id and releases the claim if the issue is missing, terminal or
non-active (§8.4, 1113-1136); slot starvation requeues (1206). Stale timers
are defeated by a per-entry `retry_token` ref (1040, 1075-1091). Stall (§8.5
Part A): no agent event for `stall_timeout_ms` kills and retries with backoff
(`restart_stalled_issue`, 608-640). Beyond the spec, the code has a `blocked`
map: a stall after the agent requested operator input parks the issue, claim
kept, no retry (`block_issue_from_entry`, 757-779), released only when the
tracker shows terminal, non-active or unroutable (456-476). Human takeover is
entirely through the tracker: change the state or remove the required label
(§14.4) and the next tick stops the worker.

**Tests.** Example-based ExUnit only, no property tests. The orchestrator
exposes `*_for_test` seams (361-409: `reconcile_issue_states_for_test`,
`should_dispatch_issue_for_test`, `handle_retry_issue_lookup_for_test`,
`revalidate_issue_for_dispatch_for_test`); tests build a `%State{}` literal,
pass an `%Issue{}` with the tracker's new view, and assert on the returned
state (`core_test.exs:850-895`, 928-1000). Lifecycle tests drive the real
GenServer with `:sys.replace_state` and fake `:DOWN` messages (1062-1100) or a
memory tracker and a FIFO-blocked hook for the restart-no-overlap case (351).
§17.4 is the spec's own test list. No parity test between dispatch and
reconcile; parity holds only because both call `candidate_issue?`.

**For sheepdog.** Idea 3 (re-observe before acting: the gate takes fresh
state) and the four-way reconcile outcome (terminal, unroutable, active,
other) as a cross-check on the `Gate` variants. Symphony's "tracker is the
only lock" is sheepdog's "git is the only state", so its no-local-claim-table
stance confirms the SHA pins are enough. Its retry tokens (a ref per scheduled
retry so a stale wake-up is ignored) are worth considering if `sd watch` ever
schedules per-branch retries. Its "blocked, waiting on a human" parking state
matches rule 4 but lives in memory and dies on restart; sheepdog's version
would have to be derived from observed state.

## Not read

rust-lang/bors (Rust; the one codebase whose idioms would transfer, not just
its ideas), Prow's Tide (Go; one `takeAction` per sync over a pool struct),
Mergify's archived engine (Python; actions gated by declarative conditions
with a reason per verdict), agent-orchestrator (the one agent tool with an
automatic CI/review-to-agent loop; only its README was read for
`alternatives.md`). The four reports above converged enough that only bors
seemed worth a further read, for Rust idioms, and that is not done.
