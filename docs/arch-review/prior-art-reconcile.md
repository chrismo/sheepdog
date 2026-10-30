# Prior art for the reconcile engine

Read on 2026-10-30 for #348 (one gate function per verb, shared by NEXT and
the act verbs) and its sequencing comment, after chrismo asked whether any
other project has a reconcile engine sheepdog could reuse, and what ideas to
pull in. Companion to `docs/alternatives.md`, which compared orchestrators and
stack tools; this page reads the *decision* code of five tools that turn
observed PR state into a next action.

Method: two rounds of readers, one per target, read-only. Round one (five
tools): Sortie and Symphony from the shallow clones in
`~/modev/pr-workflow-prior-art/` (Sortie 6e397dc, 2026-10-11; Symphony
be10a1b, 2026-09-15); Kodiak, bors-ng and Renovate from fresh `--depth 1`
clones. Round two (four more, same day, after chrismo asked for them):
rust-lang/bors at 68c45b6, Prow's Tide (sparse clone of `pkg/tide`),
Mergify's engine at tag 5.0.0 (2022-06-20, the last tagged source; HEAD is an
issue tracker now) and agent-orchestrator. All clones into a scratch
directory, nothing run. Every file:line below is to those clones. "Observed"
means read in the code; anything inferred is marked. Nothing but sheepdog was
run.

## Answers

**Is there a reconcile engine to reuse?** No. Go, Elixir, Python, TypeScript
and, for bors, Rust bound to sqlx, Postgres and octocrab; each tied to its
own state model, tracker and persistence. More useful than the no: none of
the nine has what #348 describes. Every one ends up with a long,
priority-ordered evaluator; the only difference is whether there is one copy
of the guards or two. Two of the nine (Tide, Mergify) carry comments admitting
the two copies disagree.

| Tool | One evaluator? | Pure? | Guards duplicated? | Verdict shape |
|---|---|---|---|---|
| Kodiak | Yes, `mergeable` (740 lines) | No: calls the GitHub port inline | No | One action per pass; returns nothing |
| Renovate | Yes, `processBranch` (1070 lines) | No | Yes, by re-checking memoized facts | 19-value flat result union, a report-out |
| Sortie | No: 11 ordered per-kind passes | No | Yes, armed at worker exit and re-checked per pass | Per-kind; one 4-way triage gate is pure |
| Symphony | No: one GenServer, split by trigger | Mostly | Partly: one shared predicate, three copies of the branching | 4-way reconcile `cond` |
| bors-ng | No: a GenServer per project | Core fold is pure | No | `patch_preflight`: booleans then one ordered match |
| rust-lang/bors | Per concern; the queue is a sort key plus one match | `queue_status`, the sort and the CI fold are pure; the rest interleaved | No, but a pre-act sanity check repairs the DB | Derived enum with borrowed payloads; `Result<(), SanityCheckError>` |
| Tide | Yes for the pool (`takeAction`); a second controller for the status reason | No: merges and triggers inline | Yes: pool predicates vs status predicates, by two controllers, admitted in a comment | Action enum (Wait, Trigger, TriggerBatch, Merge, MergeBatch, PoolBlocked) |
| Mergify | Yes: one engine over declarative conditions | Conditions pure; each heavy action keeps an imperative gate in `run` | Yes: conditions vs the action's `run` checks, with a FIXME to merge them | Checklist of named conditions with match marks |
| agent-orchestrator | Yes, but it queues every applicable reaction, no priority | No | No | `[]pendingNudge`; a separate derived notification lane |

So the long ordered evaluator is intrinsic to the problem. What sheepdog can
choose is to have one, pure, with its order as data, and tests shaped like
Kodiak's.

**Gates as functions or as data?** Round two was asked this. Mergify is the
one tool that made gates data (a condition language users write in YAML) and
its engine still needed an imperative gate inside every heavy action
(`merge_report`, `queue.run`), with a FIXME saying the maintainers meant to
move those into conditions and never did. Its reason output is just the
condition text. The engine-and-data design cost about 1,700 lines (parser,
filter, conditions) plus 74 unit tests before the first rule, and it keeps two
attribute tables in sync by hand. For ~15 verbs and one maintainer the
verdict is: hand-written Rust predicates, structured as a flat named list per
verb, not free-form if/else, so the name is the reason and the checklist
rendering is free; keep the Hard/Soft substitution trick (below); skip the
grammar. bors adds the Rust idioms that make this safe (ideas 10, 11, 13).

**What to pull in.** Nine ideas survived round one and nine more round two;
the first four of each belong in the #348 scaffold, two are filed as issues
(#356, #357), the rest are notes on existing issues. The per-tool sections
hold the evidence.

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
| 10 | A gate is a flat list of named predicates; the name is the reason; render as a checklist with match marks and a "clears when" line | Mergify `conditions.py` rendering; bors `check_pr_approval_validity -> Option<Comment>` | #348 step 1; #335's help text |
| 11 | A `Go` token the verb's act fn takes by value, so a verb cannot run without asking its gate; `#[must_use]` on `Gate` | bors `ExclusiveLockProof` | #348 step 1: parity by construction, beside the test |
| 12 | Hard vs Soft by substitution: re-evaluate the gate with the CI conditions replaced by true; if it still fails, it is Hard | Mergify `checks_status.py` | #348 step 1 |
| 13 | Destructure `State` with no `..` inside the gate module, so a new State field fails to compile until a gate claims or ignores it | bors `pr_needs_update_in_db` | #348 step 1 |
| 14 | Weight each refusal so the reported reason is the most severe, not the first in source order | Tide `requirementDiff` | #348 / #335 |
| 15 | A pre-act sanity check that returns typed errors the caller maps to repairs, not refusals | bors `sanity_check_pr` + `SanityCheckError` | #348 step 2 onward |
| 16 | Seen markers advance only after the step succeeded, so a crashed step re-fires next pass | agent-orchestrator two-phase hash ack | #350 / #342: a check on when pins are written |
| 17 | The dedup signature for a CI failure includes the log tail and check URL, not just the SHA, so a flaky rerun with new output is new and an identical rerun is not | agent-orchestrator `sendOnce` signatures | #344 ci-fix outcome; O5 |
| 18 | Report every applicable condition in `sd state`, run one step in NEXT | agent-orchestrator `[]pendingNudge` (its earlier first-match version suppressed review feedback under red CI) | #335: the "Notified" block |

Skipped, with reasons: a flat result enum returned from one long function
(Renovate's is a dashboard report, not a guard the action reuses: the
two-copies shape #348 leaves); an evaluator that calls the API port itself
(Kodiak: the port buys testability that `fn gate(&State)` gets free);
in-memory retry backoff and attempt counters (Symphony, Sortie: lost on
restart, and sheepdog keeps no state outside git); golden op-replay fixtures
(Sortie: a single-shot CLI over one JSON record is already table-driven);
Sortie's fixed-order per-kind passes with ordering comments (the priority
table makes that order data); a normalized PR-body hash before editing
(sheepdog doesn't maintain PR bodies); a parsed condition grammar (Mergify:
needed because users write YAML; sheepdog has one author and Rust enums);
run/cancel action pairs (Mergify: most cancels are a constant); Tide's
separate status goroutine (an API-budget device for thousands of PRs);
advisory locks and per-concern channels (bors: a daemon's needs, sheepdog has
O8 and runs to completion); pasting into a live TUI with harness hooks for
finished-vs-asking detection (agent-orchestrator: the opposite of rule 2, and
its main source of guard complexity, nine suppression outcomes).

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

## rust-lang/bors (Rust)

The Rust project's new merge bot: PR state in Postgres, GitHub webhooks plus a
periodic refresh, comment commands, a merge queue. 38k lines. The one
codebase in sheepdog's language, so its idioms matter as much as its
decisions.

**State model** (`src/database/mod.rs`). `PullRequestModel` (478-519) is the
persisted row: `status: PullRequestStatus` (Open, Draft, Closed, Merged),
`approval_status: ApprovalStatus { NotApproved, TentativelyApproved(ApprovalInfo),
Approved(ApprovalInfo) }` (231) where `ApprovalInfo { approver, sha }` (195)
pins approval to a SHA, `try_build` and `auto_build: Option<BuildModel>`, and
a private `mergeable_state` with `mergeable_state_is_stale`; the only reader,
`mergeable_status()` (553), folds stale into `Unknown`, so callers cannot see
stale data. No transition table: transitions are database writes. The state
the queue reasons over is derived: `QueueStatus<'a>` (211-229), computed by
`queue_status()` (562-589), a pure nested match over status, approval and
build status into seven variants that *borrow* their evidence
(`Pending(&ApprovalInfo, &BuildModel)`). `BuildStatus` (331) has exhaustive
`is_failure` / `is_failure_or_cancel` (346-360); Cancelled is explicitly "as if
no build", sheepdog's O5 rule. `pr_needs_update_in_db` (980-1043)
destructures both structs field by field with no `..`, so adding a field is a
compile error until someone decides whether it matters.

**Deciding.** Merge queue, `src/bors/merge_queue.rs` `process_repository`
(133-253): load all non-closed PRs, `sort_queue_prs`
(`src/utils/sort_queue.rs:6-30`, a `cmp().then_with()` chain over small
`fn -> u32` priority functions: queue status first, then mergeable, priority,
rollup, age), then walk the sorted list matching `queue_status()`. Each arm
ends in `AutoBuildStartOutcome { BuildStarted, ContinueToNextPr, PauseQueue }`
(344). A priority-ordered table plus one match, #348's shape, except the table
is a sort key rather than a verb list. Right before acting, `sanity_check_pr`
(607-639) re-fetches GitHub and returns `Result<(), SanityCheckError>` (586:
`WrongStatus`, `NotMergeable`, `ApprovedShaMismatch { approved, actual }`);
the caller (355-560) maps each variant to a *repair* (fix the DB status
silently; unapprove and comment; skip). The approval queue has a second,
synchronous, pure sanity check (`src/bors/approval_queue.rs:129-142`).
Refresh, `src/bors/handlers/refresh.rs` `sync_pull_requests_state`: GitHub
list and DB list into two `BTreeMap`s, diff, upsert only if
`pr_needs_update_in_db`; DB-open-but-GitHub-missing PRs are re-fetched one by
one. Each refresh concern is its own `BorsGlobalEvent` variant
(`src/bors/event.rs`) with its own mpsc queue and `*Sender` newtype
(`process.rs:105-140`). Decisions are interleaved with DB and GitHub calls;
only `queue_status`, `sort_queue_prs`, `get_pr_ci_status`
(`approval.rs:26-93`: dedupe runs per workflow id keeping the latest, any
Failure gives Failed, any Pending gives Pending with `started_at`, else
Success) and the sync sanity check are pure.

**Convergence and idempotency.** Every PR webhook handler opens with
`db.upsert_pull_request(payload)` (`pr_events.rs:28, 61, 168`), so a webhook
is just another observation and refresh upserts the same row; both funnel
into the same queues. Approval stores the SHA; unapprove-on-push is one atomic
`UPDATE ... WHERE id = $1 AND (approved_by IS NULL OR approved_sha != $2)`
(`src/database/operations.rs:562-587`), with a comment in `handlers/mod.rs`
`invalidate_pr` (1061 on) explaining it exists for the out-of-order push
webhook after `r+ squash`. Builds are looked up by (repo, branch, commit SHA)
(`operations.rs:771`). Multi-instance: a Postgres advisory lock
`ensure_not_concurrent` (`client.rs:56-99`) hands the closure an
`ExclusiveLockProof { _proof: () }` (571); `start_auto_build` requires it as a
parameter, so the type system proves the lock is held. Mergeability
staleness is a 95-line module doc (`mergeability_queue.rs:1-95`) listing
named race conditions and which are deliberately ignored.

**Human override.** Push, base-branch edit or close call `invalidate_pr`:
unapprove if the SHA changed, cancel the auto build with
`AutoBuildCancelReason { PushToPR, Unapproval, Close, Cancel }`
(`workflow.rs:335`), close containing rollups. Explicit commands `cancel`,
`try cancel`, `r-`, `retry`. A human merging or closing on GitHub is absorbed
by refresh or by the merge-queue sanity check with no comment. The queue
deliberately still tries approved PRs the DB thinks unmergeable, "in case our
DB data is stale" (`merge_queue.rs` near 190).

**Idioms and tests.** Enums with payloads everywhere; `StartAutoBuildError`
is `#[must_use]` (572); `InvalidationOutcome { unapproved, closed }` is a
struct of bools returned up; `ExclusiveOperationOutcome<R> { Performed(R),
Skipped }`. Commands: a `BorsCommand` enum (`command/mod.rs:133`) and one big
match in `handle_comment` (`handlers/mod.rs:439` on); each command re-checks
permission and `check_pr_approval_validity -> Option<Comment>`
(`review.rs:219-258`), where the refusal *is* the user-facing comment. Tests:
332 `#[sqlx::test]` end-to-end on a fresh Postgres each plus 153 plain
`#[test]`. `run_test(pool, async |ctx: &mut BorsTester| ...)`
(`tests/mod.rs:116-190`) boots the real router against a wiremock GitHub,
drives it with synthesized webhooks (`send_webhook`, 1315), asserts through
`PullRequestProxy.expect_*` over what the mock recorded bors doing.
Teardown `finish` (1370) drains queues and `assert_empty_queues()`: an
unconsumed comment fails the test. 214 inline `insta::assert_snapshot!(x,
@"...")`, no `.snap` files. `TestSyncMarker` statics (`bors/mod.rs:170-194`)
are `#[cfg(test)]` mark points inside production code that tests await.
`modify_pr_in_gh` mutates the mock without a webhook, giving missed-webhook
tests (`merge_queue.rs:1819-1876`).

**Reusable.** Nothing liftable as a crate: sqlx, Postgres, octocrab pinned
`=0.54.2`. Worth noting: `insta` inline snapshots, `arc-swap` for hot-reloaded
config. `TestSyncMarker` (`utils/sync.rs`, about 80 lines) and the
`sort_queue.rs` shape are small and generic. Inferred: the daemon and queue
machinery do not fit a CLI.

**For sheepdog.** Idea 11 (a zero-size `Go` token the act fn takes by value,
as `ExclusiveLockProof`: a verb cannot run without having asked its gate,
parity by construction beside the registry test), idea 13 (no-`..`
destructuring of `State` inside the gate module), idea 15 (the pre-act
sanity check that returns typed errors the caller maps to repairs), idea 10's
second half (`Gate::Hard(Reason)` whose `Display` is both NEXT's line and the
verb's stderr, one wording source). Consider `QueueStatus<'a>`'s borrowed
payloads: `Gate::Go` carrying references to the evidence the verb needs (SHA,
thread ids) so verb and NEXT act on one observation without cloning. Consider
inline `insta` snapshots for NEXT text in `tests/next.rs`, with
`cargo insta review` instead of hand-editing expected strings (#351). Skip
advisory locks and per-concern channels.

## Tide (kubernetes-sigs/prow, `pkg/tide`, Go)

Prow's merge-pool controller. Each sync it queries GitHub for PRs matching
configured queries, builds a pool per org, repo and branch, and takes one
action per pool. It also maintains a status context on each PR saying why
the PR is not in the pool. Sparse clone of `pkg/tide`; line numbers are to
that checkout.

**Sync loop** (`tide.go:601-693`). `Sync()`: `provider.Query()` (GitHub
GraphQL search, `github.go:103-180`; PRs the index still reports open but
`pr.Merged` are dropped, 163-168), then `provider.blockers()`, then
`dividePool` (`tide.go:2016-2059`: one `subpool` per org/repo/branch, the base
SHA fetched once per subpool, ProwJobs listed by an index keyed on
org/repo/branch/baseSHA), `filterSubpools` (735-760), hand the filtered pool
to the status controller (647-657), then `syncSubpool` per subpool in
parallel. `subpool` (1978-1996) is a snapshot: prs, pjs (jobs at that base
SHA), per-PR context policy, presubmits. Never refreshed mid-sync;
`takeAction` consumes it by value. Action enum (103-114): Wait, Trigger,
TriggerBatch, Merge, MergeBatch, PoolBlocked; `recordableActions` (117-123)
excludes Wait and Blocked from history. `Pool` (126-151) is the output
record: Success, Pending and Missing PRs, BatchPending, Action, Target,
Blockers, Error.

The gate is two layers: `filterPR` (825-863) is `mergeAllowed`
(`github.go:742-787`, returns a reason string or empty) plus "no non-pending,
non-Prow-owned unsuccessful context"; then `accumulate` (1150-1208) folds each
PR's ProwJob states into success, pending or missing by "worst of the best per
required context", and `accumulateBatch` (1016-1112) does the same for batch
jobs, discarding any batch whose Pulls no longer match current head SHAs
(1041-1050).

**`takeAction`** (1615-1660). Inputs: the subpool plus the five accumulated
lists and the missing serial tests. Order: MergeBatch if a batch passed; else
Merge the highest-priority passing PR, but only if no batch is pending (the
comment: don't invalidate the running batch); else Wait if no presubmits are
configured; else TriggerBatch if more than one PR and no batch pending; else
Trigger one serial retest if there are missings and nothing pending or
passing; else Wait. Not pure: it calls `provider.mergePRs` and `c.trigger`
inline and returns (Action, targets, err). `syncSubpool` (1859-1942) wraps it:
PoolBlocked short-circuits before `takeAction`, records history, emits
metrics. `pickHighestPriorityPR` (982-1010) re-checks `isPassingTests` from
live head contexts right before merging, so the accumulated successes list is
not trusted alone.

**Status context** (`status.go`). A separate goroutine
(`statusController.run`, 588-600) consumes the sync loop's pool snapshot.
`expectedStatus` (277-370): first `mergeAllowed`, the same function `filterPR`
uses, yielding an Error state with that reason; then if the PR is not in the
pool: blocker issues, else `requirementDiff` (130-273) against every
configured query for the repo, picking the query with the smallest weighted
diff (branch 2000, author 1000, milestone 100, blocked 100, review 50, each
label or context 1) and reporting only the first-set description. If in the
pool but required jobs haven't passed at this base SHA: "Retesting".
`setStatuses` (436-521) reads the existing `tide` context on the head commit
and posts only when state or description differs (481), and skips PRs in
`dontUpdateStatus` (the set the merger fills before forcing the context to
success, `github.go:405-406`, `tide.go:1395-1409`).

Same predicates as the pool? Partially. `mergeAllowed` is shared. The
query-match diff and the CI predicate are not: `requirementDiff` re-derives
label and context mismatches independently of the search query that selected
pool membership, and the comment at `status.go:313-316` admits the two
controllers can disagree ("hasFulfilledQuery is a weird state") and papers
over it. This is #348's drift, in a project with far more maintainers.

**Concurrency and idempotency.** Head moved: batches are validated against
current head SHAs each sync (1041-1050); `accumulate` ignores jobs whose
`Pulls[0].SHA` isn't the head (1172-1174); GitHub's `ModifiedHeadError`
during a merge means "stop this PR, keep the batch going" (1516-1521). Batch
already running: `nonFailedBatchForJobAndRefsExists` (1601-1613) checks a
cluster index keyed on job name, base and pulls before creating a job, so
re-triggering is a no-op; `takeAction` also refuses merges while a batch is
pending. Already merged: the search dedupe above; merge errors are classified
(`tryMerge`, 1497-1555), `UnmergablePRBaseChangedError` retried with backoff,
auth and config errors aborting the rest of the batch. In-flight record: none
locally; inferred, the ProwJob objects on the cluster are the pending record,
queried fresh each sync. History (`history/history.go:150-174`) is write-only
audit, never read for decisions.

**Tests.** Table-driven throughout with a hand-rolled fake GitHub client
(`tide_test.go:797-870`, counts merges, records statuses) and a fake
controller-runtime manager for ProwJobs. `testTakeAction` (1743-2410): 22
cases, inputs are the pre-accumulated lists plus pre-existing jobs and merge
errors; asserts action, merge count, jobs created, batch count and
`dontUpdateStatus` size. `TestFilterSubpool` (2723-3170): 13 cases.
`TestExpectedStatus` (`status_test.go:44-872`): 40 cases; `TestRequirementDiff`:
52; `TestSetStatuses` (1901-2058): 8 cases on "post only when changed". No
test asserts the status reason agrees with the pool decision:
`TestExpectedStatus` takes `inPool` as an input boolean with a stub pool map
(826-828), and `TestSync` (`tide_test.go:2489-2721`) asserts Action only.

**For sheepdog.** The parity test is the thing Tide never built; its two
controllers disagree and a comment says so. Idea 14: weight each refusal so
NEXT reports the most severe reason, not the first in source order. Consider
a per-pass `dontUpdateStatus` set (`status.go:103-107`): once a verb has
mutated remote state in this pass, the observer in the same pass must not
reconcile that field back; relevant because `sd act` observes in-process.
Consider keying in-flight work on (job, base SHA, head SHAs) and looking it
up before creating (1601-1613), matching "idempotent, keyed on SHA". Skip the
separate status goroutine and the last-second re-check (covered by
re-observe-before-acting). Cleanest extractable idea: the weighted
`requirementDiff`, where each failed predicate adds a weight and only the
first-set description survives, so closest-match selection and the one-line
reason fall out of one pass.

## Mergify engine (Mergifyio/mergify-engine, Python, tag 5.0.0)

A GitHub App whose users write `pull_request_rules` in YAML: each rule a list
of `conditions` in a small expression language over PR attributes and a set
of `actions`. The one tool that made gates data. HEAD of the repo is now an
issue tracker; tag 5.0.0 (2022-06-20) is the last tagged source and is what
was read.

**Condition language.** One hand-rolled parser, `rules/parser.py:261-461`:
optional `-` or `¬` (negate) or `+`, optional `#` (count), an attribute matched
longest-first from a fixed table (49-107, about 55 attributes), an operator
from a per-type allowlist (`=`, `!=`, `<`, `>`, `<=`, `>=`, `~=` regex;
aliases `:`, `==`, `≠`), then a typed value (text, branch, login or team,
number, time, schedule, timestamp or relative). Bool attributes take no
operator (`-draft`, 314-319). Per-type modifier rules: `#` not on bools or
dates (113-125). Output is a tiny tree `{op: (attr, value)}` wrapped in
`{"-": ...}` (168-180); `and` / `or` groups nest at most four deep
(`rules/__init__.py:424-446`).

Evaluation: `Filter` compiles the tree into closures once
(`filter.py:211-313`). A leaf does `getattr(snapshot, attr)`, awaits if a
coroutine; `#attr` tries a precomputed `#attr` first, then `len(attr)`
(177-209). A binary op is `any(op(a, ref) for a in values for ref in refs)`
(`all` for `!=`), so list attributes get set semantics for free (246-258,
316-335). `value_expanders` turn `@org/team` into member logins at eval time
(`rules/live_resolvers.py:103-109`); a failed team lookup becomes
`evaluation_error` on that condition, not a crash (`conditions.py:101-110`).

Human-readable reason: there is no reason generator. Each `RuleCondition`
stores `match` after one evaluation (single-use, `conditions.py:102-104`) and
the summary renders a checklist, `- [X] \`label=wip\`` plus optional
description and a warning on error (215-252). The condition string is its own
explanation; "what clears it" is left to the reader. Evaluation is
three-valued only for CI: `IncompleteChecksFilter` returns true, false or
IncompleteCheck (`filter.py:515-694`), and `checks_status.py:33-96` folds a
rule to SUCCESS, FAILURE or PENDING by first re-evaluating with every
check condition replaced by `number>0` to see whether something non-CI fails.
That is Mergify's Hard versus Soft, done by substitution.

**Rule evaluation.** Per PR, every rule gets its action's
`get_conditions_requirements` appended (`rules/__init__.py:311-341`): merge
adds branch-protection and depends-on conditions (`actions/merge.py:144-166`),
rebase adds `-closed` and `#commits-behind>0 or -linear-history`
(`actions/rebase.py:80-106`). So an action's prerequisites are expressed in
the same language and show in the same checklist, tagged with a description
like "rebase requirement". Then `GenericRulesEvaluator.create` sorts rules
into matching, faulty and ignored; "ignored" means re-evaluate with every
non-base attribute replaced by `number>0` and hide the rule if head, base or
author still fail (241-258). Ordering: `actions_runner.run_actions` sorts
rules so non-matching (cancel) run before matching (run)
(`engine/actions_runner.py:336-338`); within a rule, dict order. Conflicts: a
`DISALLOW_RERUN_ON_OTHER_RULES` flag makes a second merge or rebase in the
same pass report "Another X action already ran" (346-349, 400-408).

Beyond its rule's conditions, an action still has an imperative gate in
`run`: `merge_report` checks draft, merged, closed, dirty and unknown
mergeable state before calling the API (`actions/merge_base.py:275-341`);
`queue.run` checks config compatibility and branch protection
(`actions/queue.py:195-251`). The comment at `merge.py:65-67` ("FIXME: enforce
-merged/-closed in conditions requirements") shows the maintainers wanted
those imperative checks moved into conditions and had not finished.

**Snapshot** (`context.py`). The raw PR JSON plus a per-Context
`ContextCaches` of `SingleCache` slots: review threads, reviews, check runs,
statuses, is_behind, files, commits, commits-behind count (868-898).
Attribute names resolve through `PullRequest.__getattr__` into
`Context._get_consolidated_data`, one big if/elif over about 55 names
(1237-1514, 2046-2047), each fetching lazily on first touch. Nothing is cached
across events except Redis side caches (team members, permissions, branch
protection). `ATTRIBUTES` and `LIST_ATTRIBUTES` (1995-2044) are a second copy
of the parser's table; observed: two lists, no test tying them together was
found.

**Idempotency.** State lives in the Summary check run: a base64 YAML line of
`{check_name: conclusion}` (`actions_runner.py:247-293`) and a Redis pointer
from head SHA to summary, copied forward on synchronize
(`engine/__init__.py:201-247`). Per action: the previous conclusion is
compared with the expected set for run (SUCCESS, FAILURE) or cancel (NEUTRAL,
CANCELLED); skip with "Already in expected state" unless `ALWAYS_RUN`, or
`SUCCESS_IS_FINAL_STATE`, or a user refresh after FAILURE (373-398). Flags per
action class (`actions/__init__.py:45-67`): label and rebase are `ALWAYS_RUN`
and idempotent by diffing (`label.py:49-63`); merge is `SUCCESS_IS_FINAL_STATE`.
`cancel` is mostly a constant CANCELLED report (`merge.py:121-124`); only
`queue.cancel` does real work (`queue.py:371-408`). `NearDatetimeFilter`
re-evaluates the same tree with datetime semantics to compute when a time
condition will next flip and schedules a refresh (`filter.py:478-508`,
`delayed_refresh.py:68-100`).

**Tests.** Unit: 301. `test_filter.py` (39) drives `BinaryFilter` with a
FakePR dict (`tests/unit/rules/test_filter.py:32-38`); `test_parser.py` (2,
heavily parametrized); `test_rules.py` (33) builds a mocked client answering
by URL (886-950); `test_get_rule_checks_status.py` (13) uses an attrs dict.
Functional: 228 tests against 754 recorded cassette files
(`zfixtures/cassettes`, replay unless recording; `tests/functional/base.py:56-101`).

**For sheepdog.** Idea 10 (a flat list of named conditions rendered as a
checklist with match marks; pair each with a short "clears when" string), idea
12 (Hard/Soft by substitution). Consider a per-verb expected-conclusion record
keyed on SHA to skip re-running (373-398), though SHA pins already cover most
of it. Consider the ignored-versus-potential split (241-258) for NEXT: hide
verbs whose fixed preconditions (no PR, not on a stack) fail, list the rest
with what is missing. Skip the grammar and the run/cancel pairs. Cleanest
extractable idea: action prerequisites written in the same language as the
rules and rendered in the same checklist (`merge.py:144-166`,
`rebase.py:80-106`).

## agent-orchestrator (OrchestratorInc, Go daemon)

The desktop app's daemon is public and is Go, not TypeScript:
`backend/internal/observe/scm/observer.go` (poll) and
`backend/internal/lifecycle/reactions.go` (react). The one agent tool with an
automatic CI-and-review-to-agent loop; `alternatives.md` had read only its
README.

**Observe and persist.** Polling, no webhooks: `Observer.Poll` every 30 s
(`observer.go:28-52`; review threads on a slower 2 min cadence, an
unconditional refetch after 5 min, GraphQL batches of 25). The repo list is
guarded by ETag probes with an incremental cursor per repo. The record is
`ports.SCMObservation` (`backend/internal/ports/scm_observations.go:55-87`):
PR metadata (node id, HeadSHA, draft, merged, closed), CI (`Summary` unknown,
pending, passing or failing; `HeadSHA`; `FailedFingerprint`; `FailedChecks`
each with a 20-line `LogTail`), Review (decision, reviews, threads and
comments with stable ids, `IsBot`), Mergeability, and `Changed{Metadata, CI,
Review}`. Persisted in SQLite (`pr`, `pr_checks`, `pr_reviews`,
`pr_review_threads`, `pr_comment`) keyed by PR URL with a provider-id alias
layer (`docs/scm-observer.md`). Each row carries three semantic hashes
(`MetadataHash`, `CIHash`, `ReviewHash`; `observer.go:2158-2181`); unchanged
hashes mean no write and no lifecycle call (654-657).

Two-phase acknowledgement: persist facts with the OLD hashes, call lifecycle,
then on success re-persist with the NEW hashes (660-702). A crash or lifecycle
failure between the two re-delivers the same observation next poll. The repo
ETag and cursor advance only when every PR in the repo acked (725-768).

**Deciding.** One evaluator over the record, `Manager.ApplyPRObservation`
(`reactions.go:70-288`), but not a priority chooser: it queues every
applicable nudge independently (CI failing, each unresolved actionable
comment, each changes-requested review, merge conflict) into `[]pendingNudge`
and sends them all (158-166, 274-278). A comment at 160-164 says an earlier
version returned early and CI suppressed review feedback; the queue was built
to remove that coupling (test `TestPRObservation_CIFailingAndReviewBothNudge`,
`manager_test.go:2350`). Red CI and review comments both get pasted into the
same session in the same pass, no ranking. Two agents on one PR: the worker
session owning the branch gets all fix nudges; a separate reviewer agent can
be launched over the same worktree by `autoreview`
(`backend/internal/autoreview/coordinator.go:86-162`), gated on the worker
being idle past a threshold and no run for the current HeadSHA. A reviewer and
a fixer can coexist, never two fixers. The merge-conflict nudge is skipped
when the PR is stacked on an open sibling (`reactions.go:251-268, 412-434`).

**Not reacting twice.** `sendOnce` (929-995): `seen[key] == sig` suppresses.
Keys: `ci:<url>`, `comment:<url>:<commentID>` (per comment, not per thread,
326-338), `review:<url>:<reviewID>`, `merge-conflict:<url>`. Signatures: CI is
the join of name, commit hash, status, URL and raw log tail (768-774); comment
is sorted thread id plus comment id; review is its state; conflict is the
constant "conflicting". The dedup map is persisted as JSON in
`pr.last_nudge_signature` (41-47, 997-1045) and lazily loaded per PR after a
restart. Order is send, then in-memory, then persist, accepting one duplicate
nudge after a crash rather than a lost one (981-986). A new head after the
agent's fix: the CI signature includes the commit hash, so the same check
failing on the new SHA re-nudges; the review signature does not include the
SHA, so a comment nudges once per comment id regardless of pushes. Re-arm: a
definitively clear mergeability (`mergeable` or `unstable`, never `unknown` or
`blocked`) deletes only the merge-conflict key (344-410), with a `pendingRearm`
flag to retry a failed durable write. On the observer side, CI log tails are
fetched only when `FailedFingerprint` (sha256 of head SHA plus failing check
name, status, conclusion, URL and run id;
`adapters/scm/github/observer_provider.go:605-614`) changed against the stored
`CIHash` prefix (`observer.go:1651-1688`).

**Bounding the loop.** CI and merge-conflict nudges: `maxAttempts 0` means
unbounded, bounded only by the signature (per head SHA plus log tail). Review
nudges: `reviewMaxNudge = 3` per comment or review key (`reactions.go:18`),
attempts persisted. No cost cap found; inferred, `pricing/` is display only.
Auto-review: three failed auto runs per head SHA (`coordinator.go:22,
184-210`). Human notification is not "gave up"; it is a separate derived
lane: `needs_input`, `ready_to_merge`, `pr_merged`, `pr_closed_unmerged`,
`review_changes_requested` (`domain/notification.go:13-23`), emitted from
`ApplySCMObservation` (`reactions.go:440-519`). Ready-to-merge is one pure
rule, `MergeReadiness.ReadyToMerge`, shared by the live path and startup
reconciliation (`domain/pr.go:248-264`). Each open-PR lifecycle call has a
30 s timeout (`observer.go:52, 687-690`).

**Handing work to the agent.** No prompt file and no new session: it pastes
a message into the already-running TUI or chat session
(`formatCIFailureMessage` 776-810, `formatReviewCommentsMessage` 857-890,
changes-requested 837-855): failed check name, URL, sanitized log tail in a
code fence; or per comment file:line, author, body, URL, thread id; plus "Fix
and push again" or "Use the thread ID to resolve each thread". The agent
commits and pushes itself (the only daemon-side `git push` is in the
importer). `ao pr resolve-comments` and `ao pr merge` CLIs (`cli/pr.go:40,
64`) re-fetch state before acting. Finished versus asked a question comes
from installed harness hooks (`adapters/agent/claudecode/hooks.go:44-55`):
stop means idle, permission-request means waiting for input, tool hooks mean
active (`adapters/agent/activitystate/activitystate.go:32-47`). The delivery
gate re-reads session state at the paste (`sessionguard/guard.go:148-156,
319-356`): refuse when waiting for input or blocked. A suppressed send is not
stamped delivered, so it re-fires once the agent is workable (911-927).

**Tests.** About 60 reaction tests in `lifecycle/manager_test.go:2143-3950`
(both-nudge, dedup survives restart, re-arm cases, exited and needs-input
gates, stacked-child suppression, messenger failure retry). Observer: 88 tests
in `observe/scm/observer_test.go`, including
`TestPoll_LifecycleFailureHoldsBackHashesForDurableRetry` (2090) and
`TestPoll_FailingCIFetchesLogTailOnlyWhenFingerprintChanged` (1392). The
review planner is pure and tested (`review/planner.go:38`).

**For sheepdog.** Idea 16 (seen markers advance only after the step
succeeded: the cleanest extractable idea, as the rule for N3 markers and the
pins), idea 17 (the CI dedup signature includes the log tail and check URL),
idea 18 (report every applicable condition in `sd state`; NEXT still runs one
step). Consider a delivery gate that re-reads session state immediately
before launching, with a distinct "suppressed, not delivered" outcome that
leaves the marker unset (O8). Consider the deliberate asymmetry of unbounded
CI nudges versus three attempts per review comment id. Skip the hook-and-paste
model: sheepdog launches bounded headless steps, and the paste model is the
opposite of rule 2 and was AO's main source of guard complexity (nine
suppression outcomes in `sessionguard`).
