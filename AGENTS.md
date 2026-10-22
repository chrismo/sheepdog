# sheepdog

The PR pipeline: `sd state`, `sd act`, `sd watch`, `sd config`. `bin/sd`
dispatches each verb to `libexec/sd-<verb>`, a bash script or, for a verb
ported to Rust (#193), a link to the one binary `target/release/sd` built from
`src/`, which runs as the verb it's named for. Split out of agent-rig on
2026-10-13 with its history; commits before that date are agent-rig commits
filtered down to these paths. Named pr-tend until 2026-10-15 (#40).

Edit this repo, never `~/.local/bin/sd` — it's a symlink made by `./install.sh`.

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

This is a TDD repo: write a failing test first, then make it pass. A ported
verb's acceptance tests are Rust, in `tests/<verb>.rs` (the built binary run
through a link named `sd-<verb>`, by `tests/common`'s harness, #199); the bash
verbs' are bats.

**Build before bats.** The ported verbs' links point at the release binary, and
lib/config.sh's `prc_get`, `prc_step` and `prn_next` run it, so every bats suite
needs `cargo build --release` after a change under `src/`. `cargo test` runs the
Rust unit tests and `tests/`, against its own debug build.

**The full suite runs on the PR's CI, not locally and not by `rwx run`.**
Locally, run only the tests you're writing or changing during red-green,
picked by name with `-f`, not whole files and never the suite:

```bash
bats -f 'findings record with no usable round' libexec/sd-act-review.bats
```

When the change is ready, commit and `sd act push`; the PR's run is the
full-suite check (see CI below). Several sessions often work in parallel
worktrees here, and a local full run takes every core.

**Don't launch runs with `rwx run`.** Each one is a full paid run, and it
doesn't stand in for the PR's run: a later commit changes the files, and RWX
runs everything again. Over 2026-10-18 to -21, `rwx run` was 43 of the 56 full
runs. Push when the work is ready for a full check, not after every commit.

The macOS-only `sd watch` plist tests return early on RWX (`no_plutil` in
`tests/watch.rs`). After changing the plist, run them locally:
`cargo test --test watch install`.

**Check which bash bats is using before trusting a green run.** `bats` is
`#!/usr/bin/env bash`, so test bodies run under whichever bash is first on
`PATH`. Under bash 3.2 a `[[ ]]` or `(( ))` that is not the last statement of a
test body does not fail the test under `set -e`, so the suite passes no matter
what it claims. Bash 5 fails it correctly.

```bash
bash --version | head -1
PATH="/opt/homebrew/opt/bash/bin:$PATH" bats ...   # if 3.2 comes first
```

**No `unnest … into` in a SuperDB query.** super 0.3.0 panicked
intermittently on that one construct, and a test in `ci/super-stress.bats`
fails if any script uses it. CI no longer stress-tests queries (2026-10-21:
the Rust port is retiring SuperDB). `ci/super-stress` still runs a query 200
times over a fixture for anyone who wants it locally; no script has a named
query left for its built-in list (#203). SuperDB's edges are under "SuperDB
notes" below.

**Every script is `#!/usr/bin/env bash5`, `install.sh` included.** Test them
by running them, not sourcing, so the shebang picks the bash. agent-rig's
`install.sh` makes the `~/.local/bin/bash5` link; without it a script fails
with `env: bash5: No such file or directory`.

The suites pass inside Claude Code's sandbox as well as outside it (424/424 in
104 s, 2026-10-13). `rwx` may not: an x509/TLS error from it in the sandbox
means the sandbox, so run it outside.

## CI (RWX)

`.rwx/tests.yml` runs every suite on Ubuntu for PRs and pushes to `main`, through
Captain (`.captain/config.yml`) via `ci/test`. A push to a branch with an open
PR starts a run; `sd state` reports its result. To read a run:

```bash
rwx runs list --branch <branch>
rwx logs --task test <run-id>
```

`rwx run` (a run from local edits, no push) is for chrismo to ask for; see
"Don't launch runs with `rwx run`" above.

One 16-CPU task, not Captain shards: `bats --jobs` balances tests across CPUs
better than Captain balances 11 files across runners (37 s vs 77 s, 2026-10-14).

Linux has no `plutil`, so the `sd watch` tests that validate the launchd
plist return early there (`no_plutil`). They still run on macOS.

## SuperDB notes (super 0.3.0, observed 2026-10-13 and -14)

Every query goes through `prc_json` / `prc_line` / `prc_val` in
`lib/config.sh`, with `PRC_FNS` (`on`, `n`, `nul`, `sj`) in scope. Check
SuperSQL with the superdb MCP, not shell probes.

- **Input is always piped.** `super` reads nothing from a here-string or a
  `<` redirect.
- **0.3.0 only.** Every command runs `super -version` once at start and refuses
  anything else; the config types' optional fields (`{a?:string}`) don't parse
  on older versions.
- **A missing field is an error value**, not null: it prints as
  `error("missing")` and poisons comparisons, so anything that may be absent
  is `coalesce`d; `on(x)`, `n(x)` and `nul(x)` are the shorthands.
- **`has()` is unreliable on fused types:** elements of a literal array of
  differing records, and records from a JSON array of differing shapes, all
  answer true. Test presence with `coalesce(x, null) is not null`.
- **`grep(pattern, s)` with a pattern from data is a substring match**, not a
  regex; only a literal pattern is a regex. Dynamic regexes go through
  `regexp(re, s)`: `[]` on no match, an error value for an invalid regex
  (Go RE2 syntax, no lookaround).
- **`join([], sep)` is an error** ("array of string arg required"), hence
  `sj()`.
- **An empty array subquery is null:** `[unnest x | where false]` yields null,
  so it is always `coalesce([...], [])`.
- **A subquery sees only its own stream:** function parameters and outer
  fields are invisible after the first `|`; carry them in the unnested record
  (`unnest {ctx, x: items}`, exactly two fields with the array second).
- **No `unnest … into ( … )` lateral subqueries:** the construct panicked
  ("context canceled") in 12 of 200 runs; plain `unnest` and `aggregate … by`
  had 0 failures. `count {r: this, j}` numbers a stream (1-based). Inside a
  subquery, `aggregate` and `fork` were avoided so output order never depends
  on the scheduler.
- **`unflatten` needs its keys grouped by prefix**, so merged keys come out
  alphabetical.
- **No JSON encoder** (`sd config get` prints with a second call) and
  **`-f line` prints strings raw**, one per line: an empty string is an empty
  line, and `$( )` strips trailing newlines, so a run that needs to count lines
  ends with a marker.
- **Startup is about 17 ms per call** (jq: 4 ms), about 50 ms per config file
  read, so queries are batched where a loop would have called jq per item.
