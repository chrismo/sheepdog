# Pass B: state and idempotency

Judged 2026-10-29 at ed2e392 against `as-intended.md` §3 (watcher), §4,
rules 4 and 5, invariants I3, I4, I6, I7, I8, I9, I14-I18. Facts are from
`as-built/externals.md` and `as-built/churn.md` plus reading `running.rs`,
`failed.rs`, `verify_red.rs`, `limit.rs`, `usage.rs`, `act/{bookkeeping,
hold,step,attach,push,github,restack,follow}.rs`, `act/mod.rs`,
`watch/pass.rs`, `state.rs`'s record readers and `next.rs`'s budget gate.
Nothing was run.

## Verdicts

| Inv | Verdict | Why |
|---|---|---|
| I3 rerun-safe | **holds** | fetch, restack (`Gate::Done` when up to date), push (`(0,0)` prints "nothing to push"), retarget (`Done` when already on default), trigger (`Done` when the bot pin covers HEAD, `--force` to repeat), ack (state omits handled items), enroll (lineage recorded once; `sheepdog=true` idempotent) all no-op on a rerun at the same HEAD. |
| I4 reconcile, never trust | **holds** | Every record is validated before it drives a decision: pins vs HEAD and base (`state.rs:1047`), `sd-pr` vs head and state then rediscovery (`:270-304`), hold cleared when its commit is on the remote (`hold.rs:76-90`), marker vs pid and process start (`running.rs:113-118`), describe vs HEAD and target, question vs HEAD and worktree mark (`state.rs:152-165`), flaky vs red SHA (`ci_fix.rs:174`). The watcher's `notified` list is trusted as written, by design (N3), and its loss only re-notifies. |
| I6 keyed on SHA | **holds**, two deliberate exceptions | `failed` counts consecutive failures and resets on success (alternatives: "not per HEAD"); `usage.sup` is a ledger keyed on repo and branch *name* (finding B3). |
| I7 no timestamp staleness | **holds** | Time gates found: usage-limit reset (`limit.rs:44`), lease deadline (`pass.rs:795`), vacuum age (`pass.rs:603`), bot-silent after `botTimeout`, and bot-replied-since-trigger by GitHub's clock (`state.rs:942`, #63). None decides whether a review covers HEAD. |
| I8 one writer module | **strike**, replace | Fails for `base`, `sd-parent`, `findings`, `question`, `sd-lock`, `sd-handled`, `sheepdog`, the marker. Each extra writer is a legitimate lifecycle point (start / enroll / restack for lineage; review / implement for findings; step / ack for question). The real gap is that each re-implements the write (B6). Replace with "one writer *function* per record, in the record's module". |
| I9 cleanup, no orphans | **holds**, one known gap | `forget` removes refs, describe file, branch-config file; the next pass removes the watcher's branch state (`report_gone`) and sweeps refs of deleted branches. `.outcome` outlives a forgotten branch: #306, known. `usage.sup` lines are never removed (B3). |
| I14 marker released on every exit | **holds** short of SIGKILL and signals outside HUP/INT/TERM | Normal return, `process::exit` (atexit), HUP/INT/TERM all release (`running.rs:148-170`). SIGKILL of a leased step is reconciled (`pass.rs:792-797`); of a hand-run step, removed on the next `sd state` read (`running.rs:51-53`). A panic releasing via atexit is inferred from Rust's unwind-then-exit, not observed (B8). |
| I15 lock rechecked before commit and push | **holds** | `commit_edits` (`step.rs:125`), autofix commit and revert (`commands.rs:224,239`), address and ci-fix before push (`address.rs:509`, `ci_fix.rs:130`), resolve at every stop (`resolve.rs:213`). |
| I16 every child has a deadline | **fails** on one path | `sd state`, fetch, `gh api rate_limit`, `ask_gone` and the launched step are all timed. `notify()` (`pass.rs:328-349`) runs osascript or `settings.notify` with none (B7). |
| I17 glyph and NEXT from one source | **holds** | `classify()` reads `next[].key` and nothing else (`pass.rs:157-182`). |
| I18 concurrency cap and limit pause | **holds** for watcher-launched steps | `held()` counts live leased agent markers across registered repos and checks `limit::holding()` (`pass.rs:712-729`). Hand-run agent steps have no deadline and aren't counted (B11). One `limit` file for all harnesses (B10). |

## Findings

| # | Pass | Module | Step | Sev | Rests on | Finding | Status |
|---|---|---|---|---|---|---|---|
| B1 | B | src/state.rs, src/usage.rs, src/next.rs | - | high | inferred: §6 cost row (#121 comment, #276) | The cost guard does not fail closed per step: `usage::record` writes `cost_usd:null` for a step sd stopped on a signal and for a killed step with no cap (usage.rs:155-169), `spend_in` sums only numeric `cost_usd` and silently skips null (state.rs:118-131), and NEXT's `budget` hold fires only when the whole file is unreadable (next.rs:209-215). An unknown spend is counted as $0. #276 is this gap for signals; the no-cap kill is the same path. Recommend: `spend_in` returns `{usd, unknown: n}`; NEXT holds on `unknown > 0` with a message naming the rows, until #276's transcript-based real spend lands. | raw |
| B2 | B | src/next.rs, src/act/step.rs | J0-J6 | med | stated: #79 "each verb checks its own preconditions; those checks are the transition guards" | `settings.branchBudget` is enforced only in NEXT (next.rs:209-228); no verb reads it (`grep budget src/act` is empty). A hand-run `sd act implement`, `sd watch step` invoked directly, or any `--force` spends past the cap, and `ship-parent` is unguarded by design note (#148). Recommend a `spend_gate(b)` in `step_session` (step.rs:66) that refuses an agent step past the budget, so the guard lives where the other step gates do and NEXT only reports it. | raw |
| B3 | B | src/usage.rs, src/state.rs, src/act/bookkeeping.rs | - | med | stated | `usage.sup` is keyed on `(repo, branch name)` and never pruned: `spend_in` (state.rs:118-131) sums every row ever written for the name, `forget` removes nothing from it, so a reused branch name inherits the old branch's spend and can start at the budget. Recommend recording an enroll (or start) timestamp per branch and summing rows after it, or writing a `{reset}` row from `forget`; either keeps the ledger append-only. | raw |
| B4 | B | src/state.rs | O1 | med | stated: AGENTS.md "Observe (read-only)"; I11 | `lookup_pr` writes `branch.<b>.sd-pr` (state.rs:298, 304) when discovery picks a different PR, and its own comment at :302 accepts a lost race with another writer. The write is a discovery cache, keyed and revalidated at :270-277, so it is benign; but it makes `sd state` a writer, the pass runs it on every branch every minute, and nothing in the record permits it. Recommend one of: amend AGENTS.md rule for Observe to "may cache discovered identity (the PR number), never a decision", and name the write in `state.rs`'s header; or move the store to `enroll`/`ship`/`start` and let `sd state` discover without storing. The first is cheap and honest; pick it unless a second cache appears. | raw |
| B5 | B | src/act/mod.rs, src/act/step.rs, src/act/review.rs, src/act/restack.rs, src/act/address.rs, src/harness.rs | A0-A11, J0-J6 | med | inferred: churn.md #270 series | The verb-to-interlock mapping is an allowlist in five places: `WAIT_FOR_STEP` (mod.rs:302-309: restack, push, ship, autofix, verify, fetch take `idle` + `holding`), `step::in_worktree` (step.rs:17-29: the editing agent steps), review.rs:91, restack.rs:230 (resolve), address.rs:528, and `harness::run_step`'s own `during` (harness.rs:111). `retarget`, `trigger`, `ack`, `enroll`, `describe` hold nothing; `attach` and `forget --remove` call `idle` themselves. The nine #270 commits each added one entry. Recommend a declaration per verb, `struct Verb { name, scope: None | Worktree | Remote, agent: bool, spends: bool }`, consumed once in `act::run`: it takes `idle`/`holding`/`attached_check` from the declaration, and `tests/act.rs` asserts every dispatched verb has one. Omission then fails a test instead of waiting for a soak. This is the single "step scope" guard the question asks about; `running.rs` already has the primitives. | raw |
| B6 | B | src/git.rs, src/act/start.rs, src/act/bookkeeping.rs, src/act/restack.rs, src/act/step.rs, src/act/github.rs, src/watch/pass.rs | A0, A2, A7 | med | stated: I8, restated as one writer function | Records with no single writer function: `refs/sheepdog/<b>/base` is written by three raw `update-ref` calls with three different `-m` messages (start.rs:221, bookkeeping.rs:115, restack.rs:257); `branch.<b>.sd-parent` by three inline `git config` calls; `question` is deleted by `step::question_answered` (step.rs:295) and again inline by `ack --question` (github.rs:76); the running marker is released by `running::release` (line compare, running.rs:148) and re-implemented by `pass::release` (pid compare, pass.rs:109-114). `git::pin_write` (git.rs:113) already does this right for local/bot/accepted. Recommend a `pins.rs` (or widen `pin_write`) owning base, local, bot, accepted, findings, question, failed, verify-red: one write, one delete, one read per kind, and `failed.rs`/`verify_red.rs` fold in. Then I8 as "one writer function" is testable by grep. | raw |
| B7 | B | src/watch/pass.rs | N2 | med | stated: I16 | `notify()` (pass.rs:328-349) runs `osascript` or the user's `settings.notify` via `Command::status()` with no timeout, once per new reason per branch, inside the pass. A hung notifier (a stuck notification daemon, a script that prompts) blocks every branch after it and the pass never reaches `write_index`. Recommend `proc::with_timeout(self.timeout, …)` and count a timeout as a failed notification (the existing retry-once path at :573-578 then applies). | raw |
| B8 | B | src/running.rs, src/proc.rs | - | low | stated: I14 | Marker release on a panic is inferred (unwind to the runtime's exit runs atexit) and untested; signals outside {HUP, INT, TERM} (QUIT, ABRT, SEGV) have no handler (proc.rs:142) and leave the marker for reconcile or the next `sd state`. Both are covered downstream, so low. Recommend one unit test: `during_at` with a closure that panics, assert the file is gone after `catch_unwind`; and a one-line note in `running.rs`'s header that KILL and uncaught signals are the watcher's and `sd state`'s to clean. | raw |
| B9 | B | src/watch/pass.rs, src/act/bookkeeping.rs | N3 | low | stated | The pass unenrolls inline with `git config --unset branch.<b>.sheepdog` (pass.rs:605 vacuum, :923 no local branch) instead of `bookkeeping::unenroll`, so the branch-config and describe files `unenroll` removes are left to `sweep_branch_files` (:915): two code paths for one state change, which agree today only because the sweep runs first. Recommend calling `unenroll` (it already handles the gone-branch case). | raw |
| B10 | B | src/limit.rs, src/watch/pass.rs | - | low | stated: limit.rs header | The usage-limit pause is one file for every harness and model (limit.rs:3-4): a Claude limit stops the pass launching pi steps too (pass.rs:725-728), and vice versa. Documented, and harmless while one harness is in use; a design choice to revisit when pi runs alongside Claude. Recommend keying the file on harness when that day comes, nothing now. | raw |
| B11 | B | src/watch/pass.rs | - | low | stated: I18 | `held()` counts only leased agent markers (`l.deadline.is_some()`, pass.rs:719), so a hand-run `sd act implement` or `sd act address` (marker with no deadline) doesn't count toward `settings.agentSteps`: real concurrency can exceed the cap by the number of hand-run steps. Recommend counting every live agent marker; the deadline filter belongs to reconcile, not the cap. | raw |
| B12 | B | src/watch/pass.rs | N3 | low | stated: rule 4 | The watcher's per-branch state file (`$SHEEPDOG_STATE_DIR/branches/<repo>/<b>.json`) is a second store: `notified`, `failed`, the stale clock and the last state snapshot, written unlocked, and a pass and a refresh can interleave (comment at pass.rs:619-621). It is consistent with rule 4 (loss or a lost race only re-notifies) and the snapshot is used only for a gone branch's PR number. No change recommended; note it in `as-intended.md` §4 as the one record that is a cache, not truth. | raw |
| B13 | B | src/act/bookkeeping.rs, src/running.rs | A0 | low | stated: I9 | `forget` (without `--remove`) leaves every `branch.<b>.sd-*` key but `sheepdog`, by design, and leaves `<b>.running`/`<b>.outcome` under `$GIT_COMMON_DIR/sheepdog/` whatever the flags; #306 covers `.outcome`. Recommend `forget` remove both files when no live marker holds the branch (it already checks `idle`). | known: #306 |

## Design-level observations

**The state layer is sound; its edges are hand-rolled.** Every record that
decides anything is keyed on a SHA or validated against live state before
use, and I found no reader that trusts a record it didn't check. That is
rule 4 working as written, and it is the strongest part of this review's
scope. What is missing is the layer *under* the records: there is no module
that owns "a pin" the way `running.rs` owns the marker. `git::pin_write`
is three kinds of pin done right; `base`, `findings`, `question`, `failed`
and `verify-red` are each written by raw `update-ref` at the call site,
sometimes from two or three modules. The ownership failures I8 reports are
not design drift, they're the absence of that one module. Building it
(`pins.rs`, B6) is a day's work and turns an invariant nobody can check
into one a grep checks.

**The #270 pattern is the shape of the next bug.** Nine commits in three
days each added one interlock to one verb, and the result is a correct but
enumerated system: six verbs in a const, four in a helper, two more calling
the primitive themselves, and five verbs holding nothing. Each is right
today. The next verb (there's one a week) will miss the list, and the soak
will find it. B5's declaration table is the fix: make a verb's scope a
fact it states about itself, consumed once in `act::run`, with a test that
every dispatched verb states one. The running-marker primitives are
already good enough to be the only thing the table needs.

**Cost accounting is the one place the design says "fail closed" and the
code says "skip".** `spend_in` skips a null cost; `record` writes null for
exactly the steps most likely to have spent and died (signal, no cap).
NEXT's budget hold is wired to the wrong signal (an unreadable file, not an
unknown row), and no verb consults the budget at all. #150 is fixed in
`step_cost` and should close; #276 is the real remainder. Until the
transcript-based real spend exists, the honest interim is to make an
unknown row hold the branch (B1) and to put the gate where the other step
gates are (B2). Keying the ledger on branch name (B3) is a quieter wrong
number that will bite on the first reused branch name.

**Observe writes one thing, and the design should say so.** `sd state`
storing the PR number it discovered is a sensible cache and a clear
violation of "read-only" as written. The fix is a sentence in AGENTS.md,
not a code move; but it has to be a sentence, because "Observe is
read-only" is the kind of rule an agent will otherwise enforce by moving
the write somewhere worse.

**The watcher is close to its invariants.** Children have deadlines except
the notifier; the cap and the pause work for what the watcher launched;
glyphs come from NEXT. The two gaps (B7, B11) are one-line fixes. The
per-branch state file is a legitimate second store, fail-safe on loss, and
should be named as such in the yardstick so no one tries to eliminate it.
