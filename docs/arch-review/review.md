# Architecture review, 2026-10-29: read this first

Run unattended by Claude at chrismo's request, against main at 9aa97f0 /
ed2e392. What's here:

| Read | For |
|---|---|
| this page | verdicts, the issues filed, what chrismo should confirm |
| `as-intended.md` | the design as reconstructed; every claim tagged [stated] / [inferred] / [assumed] |
| `as-built.md` → `as-built/*.md` | the facts: module graph, external touch points, churn, issue clusters, test shape, lints |
| `findings-pass-{a,b,c}.md` | the three judgment passes with their reasoning |
| `findings.md` | the ledger: 135 rows, each with where it went |

## The verdict in one paragraph

The design is sound and the code follows it in the places that matter: the
agent boundary holds (an agent can't reach GitHub or the remote through
sheepdog's tooling; guidance and protected paths come from the base, not the
branch), every state record that drives a decision is keyed on a SHA or
revalidated against live state before use, the mechanical verbs are
rerunnable, and the watcher is within one timeout of its invariants. What
has drifted is the *shape*: the same knowledge is written in several places
by hand and nothing checks the copies agree. The clearest case is the
transition guards, which NEXT and each act verb both derive from the same
state JSON in different words; eleven open issues and #326 are the two
disagreeing. The same pattern (a per-verb allowlist grown one verb at a
time) explains #270's nine commits, the three copies of the editing-step
skeleton, the per-harness tables in two modules, and the records with no
owning function. None of it needs a rewrite; each is a consolidation along
seams the code already marks.

## Invariant verdicts

From the passes' tables; the invariants are in `as-intended.md` §7.

| Inv | Verdict | Where |
|---|---|---|
| I1 agent can't commit/push/gh | holds; allow list is the real gate | C |
| I2 harness names only in harness.rs | **fails**; consolidate into a type (#345) | C |
| I3 mechanical verbs rerunnable | holds | B |
| I4 reconcile, never trust a record | holds; strongest part of the codebase | B |
| I5 token-spending verbs marked | holds in code; AGENTS.md list was short (fixed) | A, C |
| I6 records keyed on SHA | holds, two deliberate exceptions (`failed`, `usage.sup`) | B |
| I7 no timestamp staleness | holds | B |
| I8 one writer module per record | **struck**, restated as one writer *function* (#342) | B |
| I9 cleanup leaves no orphans | holds, #306 known | B |
| I10 next.rs is pure | holds | A |
| I11 state.rs performs no write | **restated**: may cache what it observed, never a decision (#339) | A, B |
| I12 all git through git.rs | **fails** at 4 sites, one cause; adopt (#337) | A |
| I13 all gh through one place | **fails**; adopt as "one runner, writes in one module" (#336) | A |
| I14 marker released on every exit | holds short of SIGKILL, reconciled downstream | B |
| I15 lock rechecked before commit/push | holds | B |
| I16 every watcher child has a deadline | **fails** for the notifier (#350) | B |
| I17 glyph and NEXT from one source | holds | B |
| I18 concurrency cap and limit pause | holds for watcher-launched steps; hand-run steps uncounted (#350) | B |

## Issues filed

All labeled `arch-review`. Severity is the ledger's. Suggested order: the
first three each make the next cheaper.

| # | Title (short) | Sev | From |
|---|---|---|---|
| #348 | NEXT and the verbs share one gate function per verb | high | A1-A3, A13 |
| #335 | One STEPS table; split `next::steps` along it | med | A4, A9 |
| #341 | A declaration per verb (scope, agent, spends) consumed once in `act::run` | med | B5, A12, C10, C11, C13 |
| #349 | The cost guard fails open | high | B1-B3 |
| #343 | A decisions record the reviewer and review-fix read | high | C1 |
| #344 | One editing-step runner, one typed Outcome; ci-fix's contract into Rust | med | C2-C4, C9 |
| #345 | `enum Harness` with a descriptor per variant | med | C5, C8 |
| #342 | `pins.rs`: one writer function per record | med | B6, B9, C12 |
| #336 | A gh runner with timeout and error classes | med | A5 |
| #339 | `sd-pr` means two things; Observe may cache | med | A8, B4 |
| #340 | Split `state::observe` and `assemble` along the O-labels | med | A10 |
| #346 | Log a stray agent commit or push | med | C6 |
| #347 | `sd harness probe` against installed harnesses | med | C7 |
| #350 | Watcher gaps: notifier timeout, cap, panic test | med | B7, B8, B11 |
| #338 | `rules.rs` leaf breaks the one module cycle | med | A7 |
| #337 | `git::git` takes env; the four escapes come home | low | A11 |
| #351 | Test suite: one fake gh, a recorded fixture, honest state fixtures | med | gather:tests |
| #352 | Lint hygiene | low | gather:lints |

Edits made directly on main during the review: `as-intended.md` corrected
five times as facts came in (where the marker and branch config live, how
verbs observe, I8 and I11 restated, missing open decisions); AGENTS.md rule
1 gained `ci-fix` and `answer`, which `sd act --help` already marked.

## What chrismo should confirm

The yardstick was reconstructed, not dictated. Findings resting on these
are provisional.

1. **I12 and I13 were [assumed].** Both failed, and the passes recommend
   adopting them (as "one runner", not "one module"). If you never wanted
   gh and git centralized, close #336 and #337 and strike the invariants.
2. **"Verbs share functions, not entry points" was [assumed].** Only
   `answer.rs` breaks it (A1 covers it). Confirm or strike.
3. **Byte-compatibility of `sd state --json` with the bash era** was
   assumed to be a shim, not a goal. Eleven unit tests pin it (tests.md
   §4). If it's a shim, those tests and `json.rs`'s comment can say so.
4. **Observe may cache the PR number.** Both passes judged the write fine
   and the rule's wording wrong. #339 carries the AGENTS.md edit; it's your
   rule to change.
5. **Rule 2's wording.** Pass C notes every contract tells the agent "the
   pipeline commits your edits", correctly. "Agents never need to know the
   pipeline exists" means "never drive it"; AGENTS.md could say that.
6. **#133's verdict** (pass C): extract the runner now, decide on merging
   the user-facing verbs later. #344 is written that way.
7. **Duplicates to close** (as-built/issues.md §5): #150 is fixed in
   `step_cost` per pass B and could close against #276; the `sd-task`
   group (#281, #118, #149, #263) reads as one issue; #69 is closed but
   `docs/alternatives.md`'s "Still open" still lists it.

## What this review did not do

- Run anything but reads, `cargo build`, `cargo clippy` and `cargo
  metadata`. No tests were run; the suite's shape was judged from reading.
- Judge the config system (#283 is in flight; treated as "in flux").
- Judge stacks (deprioritized per chrismo).
- Review prose quality of the help texts, prompts or review rules.
- Verify any finding adversarially. Each pass read the code it cites; no
  second agent tried to refute. The file:line references are the check.

## After the review (2026-10-30)

- The incremental plan for the shared gates is the sequencing comment on
  #348: scaffold plus parity test, then push, ci-fix, address, implement,
  review, restack, then #335 and #341 as the last phase. No big bang.
- `prior-art-reconcile.md` reads the decision code of nine tools (Kodiak,
  bors-ng, Renovate, Sortie, Symphony, rust-lang/bors, Prow's Tide, Mergify's
  engine, agent-orchestrator) for a reusable reconcile engine (none) and the
  ideas to pull into #348 (eighteen, two filed: #356 author-set foreign
  modification, #357 address re-offer fingerprint). It also settles gates as
  hand-written named predicates, not a condition language.
