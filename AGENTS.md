# sheepdog

The PR pipeline: `sd state`, `sd act`, `sd watch`, `sd config`. `sd` is one
Rust binary, `target/release/sd` built from `src/` (#193, #204); `src/main.rs`
dispatches its verbs, and run through a link named `sd-<verb>` it runs as that
verb. `bin/sd` is a committed link to the build. Split out of agent-rig on
2026-10-13 with its history; commits before that date are agent-rig commits
filtered down to these paths. Named pr-tend until 2026-10-15 (#40).

Edit this repo, never `~/.local/bin/sd`: it's a link to `bin/sd` made by
`./install.sh`, which also writes the post-merge and post-rewrite git hooks
that rebuild `target/release/sd` after a pull or a rebase.

## Design rules

The alternatives weighed against these, and the tools compared, are in
`docs/alternatives.md`; #79 (Design record, closed) holds the rest of the
reasoning. These are the rules agents build by:

1. **Deterministic wherever possible.** Scripts observe and act at zero token
   cost; an agent runs only for judgment. The verbs that run one are
   `implement`, `address`, `describe`, `review`, `restack --resolve`, and
   `start` and `ship` when they run implement or describe; `sd act --help`
   marks them "Spends tokens". Every other verb is zero tokens.
2. **The pipeline drives; agents are workers.** The pipeline initiates every
   step and hands an agent one bounded task; the agent edits, and sheepdog
   commits, pushes and talks to GitHub. Agents never need to know the
   pipeline exists.
3. **Harness-agnostic.** Claude and pi both work. The per-harness code is
   `src/harness.rs`, the tool-rule wording a step's prompt needs included
   (`tool_rules`); nothing else may.
4. **Reconcile, don't rely on enforcement.** Never assume sheepdog is the only
   writer: each pass compares observed state to desired state and closes the
   gap. Per-step tool denies are cheap extras, never what correctness depends
   on.
5. **Idempotent, keyed on SHA.** Any step can be rerun safely.

### Step labels

Step labels in comments and test headers are the design's atomic actions:

- **Observe** (`sd state`, read-only): **O1** PR lookup (number, base, head SHA,
  draft, mergeable; native-stack membership, #17) · **O2** parent state (from
  config; open / merged / closed) · **O3** sync vs parent (behind? would a
  restack conflict?) · **O4** local vs remote (unpushed? remote moved? whose
  SHA, via reflog) · **O5** CI on the head SHA (cancelled is inconclusive, not
  red; checks on a non-HEAD SHA give no verdict) · **O6** review freshness
  (HEAD covered by `local` / `bot` / `accepted`? bot replied since trigger,
  #63) · **O7** threads owed a reply (unresolved threads and review bodies
  where the last speaker isn't us; bot vs human; outdated-but-unresolved) ·
  **O8** session (harness, session id, lock, step in flight, #12).
- **Act** (`sd act`, mechanical): **A0** start · **A1** fetch · **A2** restack ·
  **A3** push · **A4** retarget · **A5** trigger the bot · **A6** rerun flaky
  CI (not built, #12) · **A7** post reply / resolve thread · **A8** local
  review (spends tokens) · **A9** attach (#14) · **A10** verify · **A11** autofix (#65).
- **Judgment** (agent steps in the branch's session): **J0** implement · **J1**
  resolve restack conflicts · **J2** diagnose / fix red CI (#49) · **J3** triage
  findings: change code, reply only, push back, or ask · **J4** write the reply
  (bot threads only; the pipeline posts it) · **J5** does the change since the
  last review need one? none / local / remote (#66) · **J6** write the PR
  description.
- **Notify:** **N2** surface to chrismo (tab glyph, notification) · **N3**
  "seen" markers so a rerun reports only what's new. (N1, messaging a live
  session, was dropped: the pipeline resumes the session itself.)

## Workflow

sheepdog tends its own PRs, so code changes go through them:

- Branch, then `sd act push`, `gh pr create`, `sd act enroll`. Follow
  `sd state`'s NEXT from there.
- Dependent work stacks on the parent's branch (`gh pr create --base
  <parent>`). Enroll while the PR still targets the parent: enroll records
  the lineage that GitHub's retarget on merge erases. Keep it current with
  `sd act restack && sd act push`.
- chrismo merges: squash, delete the branch.
- Docs-only and issue-tracking edits may go straight to `main`.
- `docs/pr-workflow/` was consumed into issues (#32) and stays only until the
  current push of issues is over: don't rely on it, and put notes on built
  work in the command's header comment, not there.

### Or run sheepdog against the sandbox

Dogfooding on this repo's own PRs is the default, not a rule.
[`chrismo/sheepdog-sandbox`](https://github.com/chrismo/sheepdog-sandbox)
(checkout `../sheepdog-sandbox`) is a throwaway Go repo with RWX CI that sheepdog
can run against when running it here isn't advisable:

- a spike that needs real merges, force-pushes or branch-protection changes
- a CI state on demand: its `ci/mode` makes CI pass, fail, flake, crawl or hang
- a sheepdog build you don't trust yet near real PRs

Pick whichever fits the task; one piece of work can use both. The sandbox's
AGENTS.md has its rules and the setup that keeps sheepdog's state there away
from chrismo's real config, glyphs and launchd job.

## Development

This is a TDD repo: write a failing test first, then make it pass. Every test
is a cargo test. Unit tests sit beside the code in `src/`; a verb's acceptance
tests are in `tests/<verb>.rs` (the built binary run through a link named
`sd-<verb>`, by `tests/common`'s harness, #199); `sd`'s own dispatch is
`tests/cli.rs`, and `install.sh` is `tests/install.rs`. The tests need `git`,
`jq` (tests/next.rs and the fake `gh`), `bash` (the fixtures' fakes) and
`perl` (tests/act_restack.rs's fixtures) on `PATH`.

**`cargo test` builds its own debug binary.** It runs the Rust unit tests and
`tests/` against it. `bin/sd` runs the release build: after a change under
`src/`, `cargo build --release` before trying `bin/sd` by hand.

**The full suite runs on the PR's CI, not locally and not by `rwx run`.**
Locally, run only the tests you're writing or changing during red-green: one
test file (`--test <file>`, no `.rs`) and a filter on the test name, never the
whole suite:

```bash
cargo test --test act_review no_usable_round   # tests/act_review.rs
cargo test --lib tally                         # unit tests in src/
```

When the change is ready, commit and `sd act push`; the PR's run is the
full-suite check (see CI below). Several sessions often work in parallel
worktrees here, and a local full run takes every core.

**Don't launch runs with `rwx run`.** Each one is a full paid run, and it
doesn't stand in for the PR's run: a later commit changes the files, and RWX
runs everything again. Over 2026-10-18 to -21, `rwx run` was 43 of the 56 full
runs. Push when the work is ready for a full check, not after every commit.

The macOS-only `sd watch` plist tests return early on RWX: cargo test has no
skip, so each starts with `if no_plutil() { return; }` (`tests/watch.rs`) and
passes on Linux without checking anything. After changing the plist, run them
locally: `cargo test --test watch install`.

**`install.sh` is plain `bash`** and must run under macOS's 3.2 (#204), so
keep its constructs 3.2-safe. `tests/install.rs` runs it by its
`#!/usr/bin/env bash` shebang, so under whichever `bash` is first on `PATH`
(often Homebrew's 5); after changing it, also run it by hand under `/bin/bash`.

`rwx` may fail inside Claude Code's sandbox: an x509/TLS error from it there
means the sandbox, so run it outside.

## CI (RWX)

`.rwx/tests.yml` runs `cargo test --locked` on Ubuntu for PRs and pushes to
`main`, as one task, `cargo-test`. Its filter lists the files the tests read
from the repo (`src/`, `tests/`, `lib/prompts/`, `lib/review-rules.md`,
`install.sh`, `bin/sd`, the Cargo files): a test that starts reading another
one needs it added there, or a change to that file won't rerun the task. A
push to a branch with an open PR starts a run; `sd state` reports its result.
To read a run:

```bash
rwx runs list --branch <branch>
rwx logs --task cargo-test <run-id>
```

`rwx run` (a run from local edits, no push) is for chrismo to ask for; see
"Don't launch runs with `rwx run`" above. `rwx lint .rwx/tests.yml` is free.
