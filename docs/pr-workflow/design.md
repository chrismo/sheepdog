# PR workflow — design (draft, 2026-10-11)

> **Consumed 2026-10-15, kept for now (chrismo).** Every section of this file,
> `req-review.md` and `prior-art.md` was checked against the code and moved to
> its home that day: built behavior is documented by each command's `--help`
> and its bats tests, unbuilt work and open decisions are issues #65–#80 plus
> comments on #12, #14, #17, #36, #44, #49, #50 and #63, and the settled
> reasoning is #79 (Design record) and #80 (Prior art record). The design
> rules and the SuperDB notes are in `AGENTS.md`. PR #81 has the
> section-by-section pointers. The file stays until the current push of
> issues is over; treat the issues as current where the two disagree.

Goal: take the busy work out of getting a branch to a merged PR and keeping it
**current** while it's open, so that with many agents running chrismo can see at
a glance where each one stands:

1. up to date with its parent (stack-aware)
2. CI green on the head SHA
3. review current (local and/or bot)
4. bot comments responded to

## Principles

1. **Deterministic wherever possible.** Scripts observe and act at zero token
   cost; an agent is invoked only for judgment.
2. **The pipeline drives; agents are workers.** The pipeline initiates every
   step — including implementation — and invokes an agent for one bounded task
   at a time. Agents never need to know the pipeline exists.
3. **Harness-agnostic — essential.** Everything works with Claude or pi. The only
   per-harness code is a small adapter (below).
4. **Reconcile, don't rely on enforcement.** The pipeline never assumes it's the
   only writer. Each pass compares observed state to desired state and closes
   the gap; an agent's stray push or PR is just another observed state. Cheap
   per-step denies are used where a harness offers them, but correctness never
   depends on them.
5. **Idempotent, keyed on SHA.** Any step can be rerun safely.

## Decisions so far

| Topic | Decision |
|---|---|
| Scope | One branch at a time. A pass never reaches into another worktree. |
| Initiator | The pipeline: `sd act start "<task>"` (chrismo) to begin; tickets later |
| Agent invocation | Headless, one step per process, persistent session per branch (`-p` + resume) |
| Done signal | The agent process exiting (or chrismo exiting a takeover). [req-review #5](req-review.md#5-done--process-exit-cant-tell-finished-from-asking) |
| Takeover | `sd act attach` → lock + interactive resume of the same session |
| Rebase policy | `rebase --onto` + `push --force-with-lease=<branch>:<expected-sha>`; plain push when nothing was rewritten |
| Stacks | First-class. Lineage stored locally; restack is always `rebase --onto`. |
| Local review | Configurable script run by the pipeline; preferred over the PR bot whenever it suffices, and can replace it (`/marge` not required at work) |
| Bot | PR comment slash command, configurable per repo (`/marge` at work; home TBD) |
| Bot staleness | Pin HEAD when triggering; staleness = diff since the pin; rebase-only changes don't count |
| Re-review needed? | Judgment (J5), steered by PR label. Outcomes: none / local / remote |
| Agent replies | **Bot threads only.** Human threads are surfaced to chrismo. |
| Accidental agent push | **Denied where cheap, reconciled always.** Claude steps run with `--disallowedTools "Bash(git push:*)" "Bash(gh pr create:*)"`. pi's `--tools` is whole-tool only (can't deny `git push` and keep bash), so pi relies on reconciliation. Detected pushes are logged so the step prompt can be tightened. |
| Watcher | **One launchd job** across all enrolled branches |
| PR description | A resume step ("write the PR description"); `--json-schema` on Claude |
| Cycle caps | **Per loop run, reset on success; split by cost.** Local loops (verify + local review): 3 attempts. Remote CI rounds: 2 (a CI round is slow and expensive — Stripe's rule). Bot rounds: 2. Cap hit → needs-chrismo. Reaching clean / green resets that loop's count. Not per HEAD — every fix commits, so a per-HEAD reset would never trip. Revisit after use. |
| Session per step | **Resume for every step in v1.** Record per-step token usage (`--output-format json`); switch late steps (CI fix, bot address) to fresh-session + findings if resumes get expensive. *Theory:* prompt cache will have expired after a CI / bot wait, so a long resumed session is re-read at full price. |
| Deterministic verify | **Yes.** `settings.verify` (lint / build / test) runs before the local model review, at zero tokens; failure output (trimmed) goes to the agent's fix step and the model reviewer never sees code that doesn't build. The step contract tells the agent the command exists, so J0 can run it before exiting. |
| Autofix before CI fix | **Yes.** `settings.autofix` (formatters, `lint --fix`) runs first on red CI or failed verify; only what remains goes to J2. |
| Scope check | **Yes.** The task text is recorded at start (`branch.<name>.sd-task`) and passed to the local reviewer as `SHEEPDOG_TASK`, so it can flag work outside the task (Spotify's top judge veto). |
| Per-step turn cap | **No.** Neither CLI shows a turn-cap flag (`--help`, 2026-10-11); `--max-budget-usd` bounds Claude steps. |
| Verify before reply | **No, for now.** Revisit if we move fix steps to cheaper models (OpenHands-style per-thread "was it incorporated?" check). |
| Handed-off feedback | Multi-valued `branch.<name>.sd-handled <comment-id>` (git config `--add`). Mostly a guard for review bodies and in-flight steps; threads already dedup via "last speaker is us" (O7). |
| Step prompts | Lookup chain, nearest wins: `<scope>/.sheepdog/<step>.md` walking up to the repo root → sheepdog defaults. `scope` = `branch.<name>.sd-scope`, set by `sd act start --scope <app-dir>` (monorepo: per app); default repo root. |
| Build vs adopt | Self-built. AO evaluated (below), not adopted; round 2 (2026-10-11) found nothing to adopt wholesale — see "Prior art, round 2". |
| Who drives | **Pipeline drives, reaffirmed** after round 2. Symphony / Gas Town (agent drives the PR loop from prompts) are the opposite camp; Sortie / Archon match us. Keeps tokens down; stochastic only where judgment is needed. |
| GitHub native stacks (`gh stack`) | **Not used by the pipeline.** Too heavy for local automation; read only for git tricks. Guard only: chrismo uses `gh stack` at work, so a branch can be in a native stack anyway — then O1 sees the `stack` field and the pipeline skips A2 / A4 (GitHub restacks server-side and refuses base edits) rather than race it. Not built: [#17]. |

## Harness adapter — the only per-agent code

Both harnesses let the caller **choose the session ID up front**, so the
pipeline mints it and never has to discover it.

| Operation | Claude | pi |
|---|---|---|
| First step | `claude -p --session-id $U "<prompt>"` | `pi -p --session-id $U "<prompt>"` |
| Next step | `claude -p --resume $U "<prompt>"` | `pi -p --session $U "<prompt>"` |
| Done | process exits | process exits |
| Take over | `claude --resume $U` | `pi --session $U` |
| Watch | `claude-pod` | `claude-pod --pi` |
| Deny push / PR | `--disallowedTools "Bash(git push:*)" "Bash(gh pr create:*)"` | — (whole-tool `--tools` only) |

Facts (from `--help`, Claude Code 2.1.296, pi 0.99.1): `-p` is **one user
turn, many steps** — the agent loops through tool calls until it ends its turn,
then exits. It can't ask a question mid-run; if it needs input, it ends and
prints the question. Permission prompts go to the host or are auto-denied
(`--permission-prompts none`).

`harness_run` holds the harness's stderr (claude can put protocol JSON there)
and shows it only when the step fails, so nothing streams while a step runs.
On failure it adds the harness's own account: claude's error text or subtype,
or the end of pi's reply (#42). Each step says on stderr when it starts: which
harness, which session, what it is doing.

**Auth and billing** (docs read 2026-10-11 by the claude-code-guide agent;
`claude auth status` here = claude.ai, Max):

- `claude -p` on a Max login is officially allowed. Support article 15036540:
  Jun 15 update — `claude -p` "still draw[s] from your subscription limits";
  Oct 7 update — Max now includes monthly API credits that "cover ... `claude
  -p`", and "you can still use ... `claude -p` ... with your subscription
  limits". Which pool a given call draws from is not stated. An in-product
  notice chrismo saw ("headless is API usage only") was not found in docs.
- **An exported `ANTHROPIC_API_KEY` overrides the claude.ai login** (observed:
  "claude.ai connectors are disabled because ANTHROPIC_API_KEY ... takes
  precedence"). chrismo's `~/.zshrc` exports a key with no credit → every
  headless call failed until unset. The adapter runs Claude as
  `env -u ANTHROPIC_API_KEY claude -p ...` unless config says to use the key.
- Never `--bare`: it ignores OAuth and requires an API key.
- **pi cannot use the Claude subscription.** Legal & compliance page: Anthropic
  "does not permit third-party developers to ... route requests through Free,
  Pro, or Max plan credentials". pi steps need their own API key or a
  non-Anthropic provider (fits pi's role: cheaper models).
- Measure: record per-step usage from `--output-format json`
  (`total_cost_usd` is computed for every call and is **not** evidence of how
  it was billed).

Not used: Claude's `--bg` / `claude attach` (Claude-only). `--max-budget-usd`
(Claude) is a per-step cost cap worth using.

## Lifecycle — state machine

```
start → implement* → check → (fail? → [autofix] → fix* → check)          [cap 3]
      → describe* → ship → ci → (red? → autofix → fix-ci* → check → ship)  [cap 2]
      → bot? → (findings? → address* → check → ship) → ready → merged      [cap 2]

check = verify (zero tokens) then local-review (model) — review only runs on
        a tree that verifies
                                                     * = agent step (resume $U)
```

Every arrow is a script decision. Every `*` is one headless agent step in the
branch's session; the process exit advances the machine. Any failure, a cap
hit, or an escalation → **needs-chrismo** (N2).

### Steps 1–4, concretely

1–2. Built 2026-10-14: `sd act start` and `sd act implement` (`sd act
   --help`; decisions in [#14](https://github.com/chrismo/sheepdog/issues/14)).
3. **Pipeline** checks: exit 0, tree clean, commits ahead of base? (Dirty → one
   resume: "uncommitted changes left"; otherwise → needs-chrismo.) Runs
   `settings.verify`; failure → autofix, then resume `$U` with the trimmed
   output. Verify passes → local review script. Findings → resume `$U` with the
   findings JSON → agent fixes and commits → exit → check again. Cap 3 cycles.
   Clean → pin `local`.
4. **Pipeline** runs the describe step (resume `$U`, structured output), then
   ships: push, `gh pr create` (or reconcile an existing PR, below).

### Happy path, the rest of the way

5. Watcher: CI pending → nothing. Then green, bot not yet triggered.
6. First remote review: **proposed deterministic rule** — after the first ship,
   no label → trigger the bot (A5), pin `bot`. (`review:light` → skip; local
   suffices.)
7. Bot replies; O7 shows an unresolved bot thread → address step (resume `$U`
   with normalized findings): agent fixes + commits → local review → ship →
   pipeline posts the agent's reply (J4) and resolves the thread (A7).
8. CI green; `bot` pin is behind HEAD → J5 step: small fix inside the flagged
   range, local clean → no re-review → pin **`accepted`** (see O6).
9. All four checks hold → tab glyph green → **chrismo** merges.
10. Watcher sees merged → unenroll, delete `refs/sheepdog/<branch>/*`.

### Takeover

`sd act attach`: take the lock (`branch.<name>.sd-lock`), open the interactive
resume of `$U`. While locked the pipeline runs no agent steps (mechanical
observation continues). chrismo exits → release lock → pipeline resumes at the
local-review check. **chrismo exiting is a done signal too**, so pairing on an
implementation fits the same loop.

## Reconciliation — accommodating an accidental agent push

| Agent did | Pipeline sees | Pipeline does |
|---|---|---|
| `git push`, no PR yet | remote = local HEAD | ship's push is a no-op; `gh pr create` proceeds |
| `git push`, PR exists | remote has commits the local review hasn't covered | nothing breaks: "ready" and the bot trigger both require the `local` pin to cover HEAD. Cost: one early CI run |
| force-push after an amend | remote ≠ expected SHA; our lease would fail | **remote SHA in this branch's reflog?** yes → ours; push with lease = that SHA. No → someone else pushed → escalate. [req-review #9](req-review.md#9-someone-elses-fast-forward-is-treated-as-a-foreign-push) |
| `gh pr create` | PR already exists | ship becomes "find and fix up": `gh pr edit` description **and base** (an agent in a stack will likely target `main`, not its parent). Base edit skipped for a native-stack PR (O1). |
| pushed to the wrong branch (`HEAD:main`) | — | not ours to absorb; that's server-side branch protection |

**Ownership via reflog:** `git reflog show <branch>` lists every SHA this branch
has been at locally. Remote head in that list → we put it there → safe to
overwrite. Deterministic, no extra record. Reflogs are per-worktree, which is
fine: one branch, one worktree.

**Log, don't block:** a detected agent push is recorded and surfaced (N2). A
repeat offender means the step-contract prompt needs tightening.

## Storage — git-native, no JSON file

| What | Where |
|---|---|
| Parent branch name | `git config branch.<name>.sd-parent` |
| Enrolled | `git config branch.<name>.sheepdog true` |
| Harness | `git config branch.<name>.sd-harness claude\|pi` |
| Session ID | `git config branch.<name>.sd-session <uuid>` |
| Takeover lock | `git config branch.<name>.sd-lock` |
| Restack base SHA | `refs/sheepdog/<branch>/base` |
| SHA last locally reviewed clean | `refs/sheepdog/<branch>/local` |
| SHA bot was triggered on | `refs/sheepdog/<branch>/bot` |
| SHA accepted as reviewed enough (J5 "no") | `refs/sheepdog/<branch>/accepted` |
| Bot command / login | `settings.botCommand`, `settings.botLogin` (config files) |
| Local reviewer script | `settings.localReview` (config files) |
| Local review guidance | `settings.reviewPrompt` (config files): a path; relative ones read from the base commit |
| Verify / autofix commands | `settings.verify`, `settings.autofix` (config files) (repo-level for a monorepo app: scope lookup below) |
| Task text | `git config branch.<name>.sd-task` |
| Comment IDs handed to the agent | `git config --add branch.<name>.sd-handled <id>` |
| Prompt scope (monorepo app dir) | `git config branch.<name>.sd-scope <path>` |

- Refs (outside `refs/worktree/`) and repo config are **shared across
  worktrees**, so a child's pass can read its parent's records.
- A ref **pins the object** against gc after rebases / force-pushes; a SHA in a
  JSON file would not.
- `refs/sheepdog/*` is not in the default push/fetch refspecs — stays local.
- Lineage must be local because GitHub auto-retargets a child PR to `main` when
  the parent merges with its branch deleted (observed 2026-10-14, #25 → #26),
  erasing the parent name from the PR. (GitHub's native stacked PRs, GA
  2026-10-06 per agent-read docs, do keep lineage server-side — but we don't
  use them; see Decisions.)

**Cleanup** (observed, scratch repo, 2026-10-11):

| Event | `branch.<name>.*` config | `refs/sheepdog/<name>/*` |
|---|---|---|
| `git worktree remove` | kept | kept |
| `git branch -D` | **removed by git** | kept |

Config cleans itself up when the branch is deleted; refs never do (they're
repo-wide, not per-worktree). So `sd act forget` deletes them (NEXT after a
merge, built 2026-10-14), and every watcher pass sweeps `refs/sheepdog/<b>/*`
whose branch no longer exists (abandoned branches).

**Bootstrap:** `sd act start` records everything at creation. For
a branch made any other way, `sd act enroll` stores a stacked PR's parent and
base pin while the PR's base still names the parent; otherwise `sd state`
derives what it can (A2 fallback chain) and stays read-only.

### Harness, model, effort and provider per step

Modeled on Sortie's per-rule settings blocks (`docs/workflow-reference.md`,
"Rule settings blocks", read 2026-10-12): a step may override model and effort,
and the harness itself. Built as JSON blocks (`harnesses.<h>`, `steps.<step>`,
`steps.<step>.<h>`); see "Config files" below. A git-config draft of the same
keys was dropped. Git only layers the *same* key across scopes, so the
step-to-harness fallback needed our own resolver whichever store held the
values.

`<step>` is a verb name (`address`, `review`, `ci-fix`, `describe`, ...).

Resolution, per key:
- The step-and-harness block replaces the harness block's value whole.
- A key that isn't written is inherited.
- `null` clears the inherited key, so the runtime's own default applies.

The harness command and timeouts can't be overridden per step;
`settings.stepTimeout` is global.

`sd config step` warns (stderr, built 2026-10-14) when the step block sets
`model` but not `effort` and the harness block supplies one, because level
names depend on the model (Sortie's `agent.effort.inherited`). Writing
`effort`, even as `null`, silences it.

Each adapter maps these keys to its CLI flags (checked against `--help`,
2026-10-12): claude `--model`, `--effort`; pi `--provider` (needs `--model`),
`--model`, `--thinking` for effort. `args` stays as the escape hatch, split on
whitespace (no argument can contain a space) and appended **after** the typed
flags and the sandbox flags, so it can also override `--allowedTools`,
`--disallowedTools` (claude) or `--tools` (pi). Whether a repeated flag's last
value wins is up to each CLI and untested here.

To verify: whether a resumed session (`sd-session`) accepts a different model
or effort than it started with; Sortie says a change between attempts doesn't
end its session, which is untested here.

### Config files: XDG global + branch-local copy (built 2026-10-13)

The settings live in JSON instead of dotted git-config keys:

- **Global:** `$SHEEPDOG_CONFIG`, else `${XDG_CONFIG_HOME:-~/.config}/sheepdog/config.json`,
  one per machine (no system layer; single user).
- **Branch:** `<git common dir>/sheepdog/<branch>.json` (`%` and `/` encoded),
  shared by every worktree of the repo.

Shape:

```json
{"harness": "claude",
 "harnesses": {"claude": {"model": "...", "effort": "high"},
               "pi": {"provider": "...", "model": "...", "effort": "medium"}},
 "steps": {"address": {"harness": "pi", "pi": {"model": "...", "effort": null}}},
 "settings": {"verify": "make test", "botLogin": ["marge"]}}
```

**One merge, two readers.** `lib/config.sh` deep-merges the branch file over
the global file in SuperDB. A leaf in the higher layer replaces every leaf at,
above or below its path in the lower one, lists are leaves, and `null` is kept
through the merge. Then:

- `sd config get <path>` (`prc_get`) reads any dotted path, the same paths
  `set` and `unset` take, over the **built-in defaults** (`PRC_DEFAULTS`).
  A `null` in a file removes the key from the files, so the default applies.
  - **Defaults:** `harness: claude`; settings `remote: origin`,
    `stepTimeout: 1800`, `interval: 300`, `replyPrefix: "🤖 "`, and `verify`,
    `notify`, `botLogin`, `botIgnore`, `botIgnoreBody`, `implementAllow` empty.
  - **Output:** a string prints raw, a list one element per line, an object as
    JSON; `--json` prints JSON for all of them.
  - **Exit codes:** 3 means no value and no default; 1 is always an error.
- `sd config step <step>` (`prc_step`) is what one step runs with.
  - **Harness:** `--harness`, else the step's `harness`, else the top-level
    `harness`, else the default.
  - **Settings:** that harness's block, with the step's block for that harness
    laid over it. A step-level `null` clears the harness block's key, so the
    nulls have to survive the merge; that's why the merge keeps them.

Callers can't tell which layer a value came from. `sd act`, `sd watch` and
`sd state` source the lib and only call `prc_get` and `prc_step`. The lib's
functions never exit and check every step (`|| return`), because callers run
them as `x=$(prc_...) || die`, where errexit is off. The CLI calls them the same
way, so its tests cover that path. The watcher has no branch, so it reads with
`--no-branch`. `sd act address` stops on a config error. `sd watch` reports the
error on the repo's line in `sd watch status` and refuses to `install`.
`verify` must be one command; a list is refused before the agent runs.

**Typed files (built 2026-10-13).** Every file is checked against a SuperDB
type when it's read, and `set` / `unset` check the result before writing it.
Each problem is one line naming the file and the dotted path:

```
sd config: ~/.config/sheepdog/config.json: settings.verfy: unknown key
sd config: ~/.config/sheepdog/config.json: settings.stepTimeout: cannot cast to uint64: 30m
sd config: <git dir>/sheepdog/feat.json: settings.remote: global or repo file only
```

- **What's allowed:** `harness` is `claude` or `pi`. `harnesses.claude` takes
  `model`, `effort` (low … max) and `args`; `harnesses.pi` takes `provider`,
  `model`, `effort` (off … max) and `args`. `steps.<step>` takes `harness` and a
  block per harness. Settings are typed (strings, `uint64`, string lists).
  `null` is allowed anywhere, to clear an inherited key.
- **Step names are a closed list** in `PRC_STEPS`: `address` plus the planned
  agent steps `ci-fix`, `describe`, `implement`, `review`. A new verb that runs
  an agent has to be added there; `sd config step <unknown>` is refused.
- **Global-only settings:** `remote`, `interval` and `notify` are the watcher's,
  and the watcher reads no branch file. A branch file setting one is refused,
  so sd watch, sd state and sd act can't disagree about them.
- **How:** `cast` fills a typed record but silently drops keys the type lacks,
  so the check also compares the input's leaf paths with the cast's.
- **Limits:** a string number such as `"1800"` is accepted as 1800, and
  `1800.5` is cut to 1800. An unknown key holding an empty block (`"x": {}`)
  has no leaves to compare, so it isn't reported; it also sets nothing.
- **Cost:** about 50 ms per file per read (2026-10-13: `prc_get` 184 → 282 ms
  with both files present).

**No git config fallback.** `pr-tend.<key>` and `pr-tend.<harness>.args` (now
`sheepdog.*`) aren't read any more; nobody had any set (checked 2026-10-13). Per-branch *state* stays
in git config: enrollment, `sd-session`, `sd-harness`, `sd-handled`, and so on,
plus the `sheepdog.repo` registry.

**Writing values:**
- `sd config init --global` creates the global file as a skeleton of empty
  blocks.
- `sd config set [--global | --branch <b>] [--json] <path> <value>` writes one
  value at a dotted path, to the current branch's file by default.
  - Values are strings unless `--json` (numbers, lists, `null` to clear an
    inherited key). `--json` takes exactly one JSON value.
  - Each parent on the path must be missing, `null` or an object.
  - Writes go through SuperDB `put`, so the file's empty blocks and other keys
    stay.
- `sd config unset <path>` removes a key so it inherits again.
- `set` on a branch with no file creates one holding only that key.

**A branch file holds only overrides** (2026-10-13; it was a full copy).
`sd config init` writes empty blocks to fill in, and `set` creates the file
when needed. Per-PR differences are common: some PRs are cheap and easy, others
thorough and need a frontier model. (No command flags are designed for this
yet.) A full copy froze the branch: later edits to the global file, such as a
new `botIgnoreBody`, never reached it. Branch files made by the old `init` are
still full copies; `sd config init --force` replaces one with empty blocks.

**SuperDB notes (0.3.0, observed 2026-10-13):**
- **No here-strings or redirects:** `super` reads nothing from `<<<` or from
  a `<` redirect of a file, so input is always piped.
- **0.3.0 only:** every pr-* command runs `super -version` once at start and
  refuses anything else ("needs super 0.3.0, but … is v0.2.0"). 0.3.0 is the
  only version these notes were observed on. The config types' optional fields
  (`{a?:string}`) don't parse on 0.1.0 or the 2025 dev pins at all.
- **No lateral subqueries:** `unnest … into ( … aggregate … )` panicked
  ("context canceled") in 12 of 200 runs. The merge uses plain `unnest` and
  `aggregate … by` instead, with 0 failures in 200 runs, and 0 in 450 calls of
  the finished lib.
- **Key order:** `unflatten` needs keys grouped by prefix, so the merged keys
  come out alphabetical.
- **No JSON encoder:** SuperDB has no to-JSON function, so `get` uses a second
  call to print raw or JSON.

**SuperDB notes from the jq conversion (0.3.0, observed 2026-10-14, #19):**
`sd state`, `sd act`, `sd watch` and `sd review-local` no longer run `jq`;
every query goes through `prc_json` / `prc_line` / `prc_val` in
`lib/config.sh`, with `PRC_FNS` (`on`, `n`, `nul`, `sj`) in scope.
`ci/super-stress` runs each named query 200 times on `ci/fixtures/` and fails
on a panic or a differing output (13 filters, 0 failures at conversion time).
- **A missing field is an error value**, not null: it prints as
  `error("missing")` / `{"error":"missing"}` and poisons comparisons, so
  anything that may be absent is `coalesce`d. `on(x)`, `n(x)` and `nul(x)` are
  the shorthands.
- **`has()` is unreliable on fused types:** elements of a literal array of
  differing records, and records from a JSON array of differing shapes, all
  answer `has` true. Test presence with `coalesce(x, null) is not null`.
- **`grep(pattern, s)` with a pattern from data is a substring match**, not a
  regex; only a literal pattern is a regex. Dynamic regexes (`botIgnoreBody`)
  go through `regexp(re, s)`, which returns `[]` on no match and an error
  value for an invalid regex (that is the startup check). Go RE2 syntax, not
  jq's Oniguruma: no lookaround.
- **`join([], sep)` is an error** ("array of string arg required"), hence
  `sj()`.
- **An empty array subquery is null:** `[unnest x | where false]` yields null,
  so it is always `coalesce([...], [])`.
- **A subquery sees only its own stream:** function parameters and outer
  fields are invisible after the first `|`; carry them in the unnested
  record (`unnest {ctx, x: items}`), which must have exactly two fields with
  the array second.
- **`count {r: this, j}`** numbers a stream (1-based); `aggregate` inside a
  subquery was avoided, as was `fork`, so output order never depends on the
  scheduler.
- **`-f line` prints strings raw**, one per line, which replaces `jq -r`; an
  empty string is an empty line and `$( )` strips trailing newlines, so a run
  that needs to count lines ends with a marker (`harness_run`).
- **Startup is ~17 ms per call** (jq: ~4 ms), so queries are batched where a
  loop would have called jq per item.

## Atomic actions

### Observe — deterministic, read-only (`sd state`)

| # | Action | Answers |
|---|---|---|
| O1 | PR lookup | number, base, head SHA, draft/open, mergeable; in a native GitHub stack? (→ hands off) |
| O2 | Parent state | parent from config; parent PR open / merged |
| O3 | Sync vs parent | behind? would restack conflict? |
| O4 | Local vs remote | unpushed commits? remote moved? whose SHA (reflog)? |
| O5 | CI on head SHA | pending / green / red, which checks. **Cancelled = inconclusive, not red**; checks on a non-HEAD SHA give no verdict (Sortie ADR-0023) |
| O6 | Review freshness | bot replied since trigger? Is HEAD covered by `local` / `bot` / `accepted`? |
| O7 | Threads owed a reply | unresolved threads / review bodies where last speaker isn't us; split bot vs human; **outdated-but-unresolved** (anchors lost to a force-push) handled explicitly |
| O8 | Session | harness, session ID, locked (taken over)? step in flight? |

### Act — deterministic, mechanical (`sd act <verb>`)

| # | Action | Notes |
|---|---|---|
| A0 | start | worktree, lineage, base pin, enroll, mint session |
| A1 | fetch | |
| A2 | restack | algorithm below; stops on conflict |
| A3 | push | plain if fast-forward; else `--force-with-lease=<branch>:<sha>` with the SHA from O4, only when that SHA is ours. Never over others' commits, `--force` or not (chrismo, 2026-10-13) |
| A4 | retarget / fix base | `gh pr edit --base` to match `sd-parent` (or `main` once the parent merged) |
| A5 | trigger bot | post `sheepdog.botCommand`; pin `bot`; skip if already pinned at HEAD |
| A6 | rerun flaky CI | optional; needs an allowlist |
| A7 | post reply / resolve thread | posts the agent's J4 text; resolves only after replying |
| A8 | local review | run `sheepdog.localReview`; findings → next agent step; clean → pin `local`. **The one Act verb that spends tokens** (the default reviewer is a model call) — exception to principle 1's zero-token rule |
| A9 | attach | lock + interactive resume (takeover) |
| A10 | verify | run `settings.verify`; trim output to errors; zero tokens |
| A11 | autofix | run `settings.autofix`; commit the result if the tree changed |

### Judgment — agent steps (resume `$U` with a step-contract prompt)

| # | Step |
|---|---|
| J0 | implement the task |
| J1 | resolve restack conflicts |
| J2 | diagnose / fix red CI |
| J3 | triage findings (bot or local): change code, reply only, push back, or ask chrismo |
| J4 | write the reply (bot threads only) — the pipeline posts it |
| J5 | does the change since the last review need one? none / local / remote |
| J6 | write the PR description (structured output) |

### Notify — deterministic

| # | Action |
|---|---|
| N2 | surface to chrismo: tab-title glyph; push notification for needs-chrismo (human threads, escalations, caps hit, detected agent pushes) |
| N3 | "seen" markers so a rerun reports only what's new |

(N1, messaging a live session, is no longer needed for the main loop — the
pipeline resumes the session itself. See "pi support in claude-peer" below.)

## A2 — restack algorithm

Squash merges defeat a plain rebase (patch-id skip can't match P1..Pn against
one squash commit S), so there is **one code path for every case**:

```
git rebase --onto <new-base> <old-base-sha> <branch>
```

- parent still open → `<new-base>` = `origin/<parent>`
- parent merged (squash, merge, or rebase-merge) → `<new-base>` = `origin/main`

`<old-base-sha>` resolution, each candidate checked with
`git merge-base --is-ancestor <sha> HEAD`:

1. `refs/sheepdog/<branch>/base`
2. merged parent PR's `headRefOid` (GitHub)
3. `git merge-base --fork-point origin/<parent>`
4. escalate (J1 / chrismo)

**Post-condition** — the branch's own change must be unchanged by the move:

```
before = git diff <old-base-sha> <old-HEAD> | git patch-id --stable
after  = git diff <new-base>     HEAD       | git patch-id --stable
```

Equal → write new `base` pin, done. Not equal → reset to old HEAD, escalate.
Only for a clean rebase: a conflict resolved by the agent (J1, `sd act restack
--resolve`, built 2026-10-14) changes the diff by definition, so a resolved
rebase skips the check, pins the base, runs verify, and goes to local review.
Each stop is one agent step in the branch's session with edit tools only; the
pipeline checks for leftover markers, adds and continues; 3 stops is the cap.

**Observed (subagent probe, git 2.53, 2026-10-11):** default patch-id includes
context lines, so an upstream edit inside our diff context (line 8 vs our line
10) gave different IDs → needless escalation. `git diff -U0 | git patch-id
--stable` gave equal IDs; `range-diff` also flagged it (`!`, context only).
→ **Use `-U0`.** A wrong old-base that replays commits still in the merged
parent PR is refused before the rebase (RESTACK_GATE's `from_parent`, #44, by
SHA: a parent rebased before it merged leaves copies it misses). Whether `-U0`
alone catches a wrong old-base is still untested.

Stack cascades are **pulled**: a child notices its parent moved on its own next
pass (O2/O3). No cross-worktree writes.

## O6 / J5 — review staleness and re-review

Shared helper with A2: "same change modulo base?" via `git diff -U0 | git patch-id --stable` (context-free; see A2 observation).

HEAD counts as **reviewed** when covered by the `local`, `bot`, or `accepted`
pin (whichever the label requires). An open PR with none of them, drafts
included, is unreviewed: its review step comes before waiting on a parent PR or
a required review (#41).

| Case | Check | Meaning |
|---|---|---|
| pin = HEAD | — | covered |
| pin is ancestor of HEAD | `rev-list --count pin..HEAD` | N commits uncovered → J5 |
| rewritten by rebase | `-U0` patch-id of pin-base..pin vs base..HEAD | equal → covered; else `range-diff` → J5. Each pin's `<kind>-base` ref is the base it was taken against (`pin_write`, built 2026-10-14): the one `base` ref moves on restack, and after a squash-merged parent `merge-base` brings the parent's commits in too |

Timestamps only answer "has the bot replied since the trigger" — never
staleness, because rebase rewrites committer dates.

**Candidate for the "rewritten by rebase" row — Gerrit's TRIVIAL_REBASE**
(`ChangeKindCacheImpl.java`): replay the old change onto the new base and
compare trees:
`git merge-tree --write-tree --merge-base=<old-base> <new-base> <pin>` ==
`HEAD^{tree}` → covered. Stronger than patch-id for O6. (Weak as the A2
post-condition, since after a clean rebase it's ~always true.)

**Caveat — human approvals:** GitHub's "dismiss stale approvals" fires when the
**merge base** changes, not the diff. On repos requiring approval, a restack can
drop "ready" even though our pins say covered. Docs-read, untested.

**J5 inputs** (assembled by script so every decision sees the same evidence):
diff since the pin, the bot's comments, per-hunk "in a range the bot commented
on". J5 "no re-review" → pin `accepted`, so the next pass doesn't ask again.

**Importance via PR label:**

| Label | Iterations | Before merge |
|---|---|---|
| `review:light` | local | local is enough — remote never needed |
| *(none)* | local | first ship → remote (proposed rule); afterwards J5 decides |
| `review:critical` | local | remote on the final SHA, always |

## Local review — the configurable script (A8)

Purpose: catch what the PR bot would catch **before** it costs a PR
interaction — fewer bot round-trips, less PR comment noise, faster loop. Runs
only after `settings.verify` passes (A10), so the model never reviews code that
doesn't build.

```
sd config set [--global] settings.localReview <command>
```

Any executable. What it does inside (prompt, model, emulating the backend bot
or not) is out of scope; sheepdog defines only the contract, and ships a
default `libexec/sd-review-local` (built 2026-10-14: the guidance, then the
reporting rules in `lib/review-rules.md`, the task, commits and diff, through
`harness_run` in `lib/sheepdog.sh` with read-only tools and a new session every
run; harness, model and effort from `sd config step review`). It says on stderr
when it starts: harness, session, range (#42).

**The guidance is configurable (#38):** `settings.reviewPrompt` names a file
that replaces `lib/review-prompt.md`, the framing and what to look for. The
reporting rules stay fixed, since the findings schema and the pins depend on
them. Empty is the shipped default; a missing file, or a path that isn't a file,
fails the review. A relative path is read from `SHEEPDOG_BASE`, never from the
worktree: the authoring agent can edit the worktree, so guidance read there
would let a branch weaken its own review. A change to the guidance therefore
applies to later branches, not to the PR that makes it. `./` is the worktree's
top. `step_guidance` in `lib/sheepdog.sh` holds that rule, for #50 to reuse per
step. `sd act review` gives the default reviewer 30 s over `stepTimeout`, so the
harness's own timeout fires first. `sd act review` runs it after `sd act verify`
passes, over `base..HEAD`; an empty list pins `local` (with its base),
findings are printed as JSON, and a failed reviewer pins nothing.

**Contract:**

- input: env `SHEEPDOG_BASE`, `SHEEPDOG_HEAD` (SHAs), `SHEEPDOG_TASK` (the task
  text, so out-of-scope changes can be flagged); cwd = the worktree
- output: JSON findings on stdout — `[{file, line, severity, body}]`
- exit 0 = review completed (findings may be empty); non-zero = reviewer failed
  (escalate; never treated as clean)

**Who runs it, who receives it:** the pipeline runs it after an agent step
exits (so the tree isn't moving — no frozen worktree needed) and hands the
findings to the branch's agent as the next resume step. The agent triages them
in its own context; it already knows why the code looks the way it does.

**Isolation** is the script's job: a separate process (fresh context, never the
authoring conversation) with read-only tools (no `gh`, no edits). The agent
can't steer it — it never even invokes it.

**One findings shape for both reviewers.** Bot comments (O7) are normalized to
the same `{file, line, severity, body}` shape, so the address step handles local
and remote findings identically.

## Ordering — a push invalidates everything downstream

Every push resets CI (O5) and bot freshness (O6). So within any loop:

1. restack (A2)
2. agent step(s); batch all code changes
3. verify, then local review, until clean (no CI run, no PR interaction)
4. push once (A3), fix base if needed (A4)
5. wait for CI green
6. remote bot (A5) only if the label / J5 calls for it

## Watcher — one launchd job

Every few minutes, for each enrolled branch:

1. `sd state` (zero tokens)
2. compute the next transition of the state machine
3. mechanical transitions → run them (A1–A7, A10, A11); A8 runs only after an
   agent step exits or a takeover ends, never on an idle pass.
   [req-review #3](req-review.md#3-lock--in-flight-gates-only-agent-steps-not-mechanical-ones)
4. agent transitions → run the resume step, **unless locked** (taken over) or a
   step is already in flight for this branch
5. needs-chrismo → N2

**Built so far — observe-only (`libexec/sd-watch`, 2026-10-11).** Steps 2–4 are
not built; a pass only observes and reports:

- Enrollment: `sd act enroll [<branch>]` sets `branch.<name>.sheepdog` and adds
  the main worktree to global `sheepdog.repo` (multi-valued), the watcher's list
  of repos. `sd act unregister [<path>]` takes a repo off that list.
- **Fetch first** (decided): one `git fetch --prune` per repo, then `sd state`
  per enrolled branch. `sd state` stays read-only, with no `--fetch` option.
- Glyph per worktree in `~/.claude/tab-status/<worktree>.pr` as
  `<branch>\t<glyph>`; `tab-status` appends it to the title only while the
  worktree is still on that branch.
  - **The glyph is read from sd state's `.next` steps** (`lib/next.sh`), not
    from the raw fields, so the tab and `NEXT` can't disagree (2026-10-13).
  - ❌ needs chrismo: any `ci-red`, `conflict`, `human` or `foreign` step.
  - ⚠️ unknown: an `errors` or `stale` step, or no steps at all.
  - ⏳ waiting: every step is a `wait` (CI running, a required review, GitHub
    still computing mergeability).
  - ✅ ready: `ready` is the only step.
  - 🏁 merged · 🚫 closed.
  - 🔧 work for later: anything else (restack onto a stacked parent, push, bot
    items, info items to read and ack, open a PR, draft, review not covered,
    cancelled CI, GitHub blocking the merge for another reason).
- Notifications (N2, macOS by default, `settings.notify` overrides) only for a
  step that wasn't there last pass (N3), keyed by step and worded by its `say`:
  the ❌ steps, merged, ready to merge.
- `sd watch status` prints the last pass. `sd watch install` / `uninstall`
  manage the launchd job, opt-in per machine; interval `settings.interval`,
  default 300 s.
- O7 in `sd state` (2026-10-12): besides inline threads, bot **review bodies**
  (non-blank) and bot **top-level PR comments** are owed items, typed
  `thread` / `review` / `comment` in `.threads.items`, with per-type counts in
  `.threads.by_type`; all count toward `owed_bot`. A review/comment stays owed
  until its node id is in multi-valued `branch.<name>.sd-handled` (`sd act
  address` adds it). Human top-level comments are not tracked.
  `settings.botLogin` is a list and matches with or without `[bot]`.
- Marks in `sd state` (2026-10-13). Every owed item is marked `!` (blocks) or
  `i` (to read), and the one-liner shows `!:<n> i:<n>`.
  - **`!`:** unresolved threads, a bot review in state `CHANGES_REQUESTED`, and
    a person's latest `APPROVED` / `CHANGES_REQUESTED` / `DISMISSED` review
    when it requests changes. A later `COMMENTED` review doesn't clear it,
    matching GitHub. Verdicts come from `reviews(last: 50)` only, so on a PR
    with more than 50 reviews an older request for changes can drop out of
    view and stop blocking (limit, not handled).
  - **`i`:** bot review bodies and PR comments not yet in `sd-handled`.
  - **Why the split:** those have no "resolved" state, so they were counted as
    owed alongside real work. The verdict a review bot writes in prose (Merge
    Simpson always submits as `COMMENT`) isn't parsed.
  - **Output:** `--json` items carry `mark`, `author`, `url`, and an `excerpt`
    (first line with text, tags stripped, 100 characters). A `stored` block
    has the `sd-handled` ids and `refs/sheepdog/<b>/*`. `--long` adds one line
    per item.

Rejected alternatives: `/loop` per session (tokens per idle wake, dies with the
session); built-in PR subscriptions (Claude-only, untested); git hooks (blind to
GitHub-side events); PreToolUse / Stop hooks to enforce agent discipline
(Claude-only; replaced by "pipeline drives" + reconciliation).

## pi support in `claude-peer` — no longer on the critical path

The pipeline resumes sessions itself, so it doesn't need to message a live
session. `claude-peer` pi support is still useful for one case: nudging a
session chrismo has taken over ("CI went red while you're in here"). Plan if
wanted:

- `pi/extensions/peer-socket.ts`: on `session_start` listen on
  `/tmp/cc-socks/pi-<pid>.sock`, write a registry entry (`kind: "pi"`) to a
  **separate dir** (e.g. `~/.pi/agent/peers/`, not Claude's unsupported
  `~/.claude/sessions/`); on each line call
  `pi.sendUserMessage(text, { deliverAs: "followUp" })`; clean up on exit.
- `claude-peer` scans the second registry dir; send path unchanged.
- Verified in pi 0.99.1 docs/types; not yet exercised.

## Packaging

| Piece | Kind | Tests |
|---|---|---|
| `libexec/sd-state` | O1–O8 → JSON + one-line summary | bats, fake `gh` + git fixtures |
| `libexec/sd-act <verb>` | A0–A9 | bats; real git fixtures for A2 (squash case!) and reconciliation (reflog ownership) |
| `lib/sheepdog.sh` | `harness_run` (adapter: claude / pi) + `with_timeout` | bats, stub `claude` / `pi` |
| step-contract prompts | one per J step, versioned in the repo | — |
| `libexec/sd-review-local` | default reviewer script | bats for the contract, stub `claude` |
| `sd act verify` / `autofix` | run configured commands, trim output | bats, stub commands |
| `libexec/sd-watch` + launchd plist | state machine driver | bats |
| tab-title glyph | N2 | — |

## Suggested build order

**Manual first (decided 2026-10-12).** Every step is a `sd act` verb chrismo
runs by hand; the watcher only starts dispatching them once each one behaves.
Each verb checks its own preconditions with `sd state` and refuses with the
reasons (`--force` overrides the soft ones), so those checks *are* the state
machine's transition guards: the loop later calls the verb and the verb
decides. Verbs are written headless from the start (structured outcome, no
prompts, exit codes) so the watcher can call them unchanged.

**Reordered 2026-10-13 for dogfooding.** The pipeline's first real work is
building itself, so the order favours what keeps chrismo's own branches current
and reviewed, and leaves for last what he already does by hand: starting work
and creating PRs. Theory, not yet tested: it is drawn from the verbs' value, not
from running them.

1. `sd state` — done
2. `sd watch` observe-only + glyph + notifications — done
3. `sd act address` (bot feedback) + harness adapter — done, see below
4. **Bootstrap spike.** Enroll an existing branch with no `sd act start` and
   check that `sd state` derives the missing records (A2 fallback chain). Every
   later step assumes it works; if it doesn't, fix it first. [#7]
5. `sd act restack` + `push` incl. reconciliation — done 2026-10-13, see below
6. **Retarget**, pulled out of the old step 6: after a parent merges, restack
   onto `main` and change the PR's base. It is stack maintenance, so it sits
   next to restack. Done 2026-10-14: `sd act retarget` edits the base (GitHub
   usually already did, on the parent's branch delete) and rewrites
   `sd-parent` to it; a restack that lands on the default branch does too,
   unless the PR still targets the merged parent (then retarget does it, #82). A
   parent closed without merging is a `parent-closed` step for chrismo, and
   restack and retarget refuse. [#8]
7. `sd act verify` — the zero-token half of the old step 5. Gates every later
   step. Done 2026-10-14: `settings.verify` in the branch's worktree under
   `settings.stepTimeout`; fail → nonzero, output trimmed to its last 40 lines;
   a tree change counts as failure. `sd act address` runs the same code. [#9]
8. `sd act ship` + `describe` — push, `gh pr create`, and the description step.
   After the other manual verbs because creating a PR by hand is the cheapest
   thing chrismo does. Done 2026-10-14: describe is read-only in the branch's
   session and stores `{title, body}` for HEAD; ship reuses it, rechecks HEAD,
   pushes, opens a **draft** PR on the parent (or fixes up an open PR's title,
   body and base) and enrolls. NEXT marks the draft ready only once CI is green
   and the review covers HEAD. [#10]
9. Local review (`libexec/sd-review-local`) — the model half of the old step 5. With
   no bot, it is the only source of review findings. Done 2026-10-14, with
   `sd act review`; handing findings to the branch's agent is the loop's. [#11]
10. The loop: watcher dispatches verbs (below, "Process supervision"). After
    4–9 because a verb has to behave by hand first. [#12]
11. Bot trigger — waits until there is a bot to trigger. Done 2026-10-14:
    `sd act trigger` posts `settings.botCommand` on the PR and pins `bot` at
    HEAD (with its base); NEXT's `bot-trigger` step runs it once HEAD is
    pushed and the local review covers it. The sandbox has a toy bot
    (`.github/workflows/review-bot.yml`) that answers `/review-bot` with a
    review. [#13]
12. `sd act start` + implement; `sd act attach` — last; chrismo starts the work
    by hand while dogfooding. Start and implement done 2026-10-14; attach
    remains. [#14]

Two decisions that had to come before steps 5 and 6 are settled: the name is
`pr-tend` (sheepdog since 2026-10-15), and the pipeline moved out of agent-rig into its own repo
(2026-10-13).

## `sd act address` — built 2026-10-12

`sd act address [--force] [--no-publish] [--harness claude|pi] [<branch>]`:

1. Find the worktree that has the branch checked out (refuse if none or
   dirty), fetch, run `sd state --json`.
2. Gate. Hard (never forced): no PR, PR not open, GitHub unreadable. Soft
   (`--force`): taken over, a restack that would conflict, sync unknown,
   unpushed, remote behind or diverged. Warn: CI still running, behind the
   target (a busy repo's branch is never caught up, so behind alone never
   blocks; [req-review #10](req-review.md)). Nothing owed → exit 0.
3. Fetch the owed bot items' text with one `nodes(ids:)` query; normalize to
   `{id, type, path, line, outdated, conversation}`.
4. Agent step: resume `branch.<b>.sd-session` or mint one (stored with
   `sd-harness` after a successful run), run under `settings.stepTimeout`
   (1800 s). Returns `{outcome, commit_message, summary, items: [{id, action,
   reply}]}`, action ∈ fixed / reply / pushback / ask. Claude: `--json-schema`,
   result in `structured_output`, edit tools only (commit, push, `gh` denied).
   pi: `--tools read,edit,write`, result parsed from a final ```json block.
5. Pipeline commits the edits (`gave-up` → nothing committed or posted), runs
   `settings.verify` (fail → commit kept, nothing pushed), prints the plan.
   `--no-publish` stops here; the agent has already run.
6. Push, then: threads get a reply (prefix `settings.replyPrefix`, default
   "🤖 "); fixed / reply resolve it, pushback stays open. Review bodies and PR
   comments get one combined PR comment and are added to
   `branch.<b>.sd-handled`. The comment links to each item by URL rather than
   quoting it: on 2026-10-13 a quoted Merge Simpson status comment carried its
   avatar URL (`…/merge-simpson.png`), whose `/merge-simpson` matched that
   workflow's `contains(body, ...)` trigger and started a new review. `ask` items (and any the agent skipped) are
   printed under "needs chrismo" and left alone.

**Trust:** a thread is handed over only if every comment in it is from a bot
(GitHub `Bot` type or `settings.botLogin`) or the viewer. Anyone else's text
could steer edits that `settings.verify` then runs and the push publishes, so
such threads go to "needs chrismo" untouched. The agent never runs shell
commands beyond read-only git (Claude) or none at all (pi); `verify` runs repo
code on agent edits, unsandboxed — `--no-publish` lets chrismo look first.

Not in v1: local review before the push, cycle cap, bot re-trigger, the
`accepted` pin, lease / lock. `botIgnore` (logins) keeps bots such as
`github-actions` from ever being owed; `botIgnoreBody` (regexes, matched against
bot review and PR comment bodies in `sd state`) drops single posts such as a
review bot's "on the job" status without ignoring the account. Both are applied
before the agent sees anything, so ignoring is deterministic, not an
instruction. Inline threads are matched by author only (their bodies aren't
fetched by `sd state`).

The agent's reply *text* is scrubbed before posting (#70, 2026-10-16): a
`/command` or `@mention` gets a lookalike glyph, see `scrub` in
`src/act/address.rs`.

## `sd act restack` and `sd act push` — built 2026-10-13

Separate verbs: restack never pushes, because every push restarts CI and makes
bot reviews stale ("Ordering"), so one push should follow all the local work.
By hand: `sd act restack && sd act push`. NEXT's `behind` and `push` steps
name them.

`sd act restack [--force] [<branch>]` (A2):

1. Find the branch's worktree (refuse if none or dirty), fetch with `--prune`,
   run `sd state --json`.
2. Gate, from sd state. Hard: the parent's merged state unknown (GitHub
   unreadable, parent not the default branch), no old base or target ref
   (sd state's errors are quoted), conflict unknown, a predicted conflict (the
   message gives the `git rebase --onto` to run by hand: J1 isn't built).
   Soft (`--force`): taken over. Already on the target → exit 0.
3. `git rebase --onto <remote>/<target> <old-base>` in that worktree. A
   conflict partway through (possible when the whole-branch merge is clean,
   e.g. a commit that is later reverted) → `rebase --abort`, back to the old
   HEAD.
4. Post-condition: `git diff -U0 | git patch-id --stable` of old-base..old-HEAD
   must equal new-base..new-HEAD. Not equal (e.g. the target already has part
   of the change, so the rebase dropped a commit) → reset to the old HEAD and
   print the `git range-diff` to look at.
5. Move `refs/sheepdog/<b>/base` to the new base.

Tested against real git and the real sd state (gh stubbed): a moved default
branch, a stacked parent that moved, a squash-merged parent (old base from the
merged PR's `headRefOid`), and each refusal and rollback above.

`sd act push [--force] [<branch>]` (A3), git only, no GitHub:

- Fetch `--prune`, then compare with `<remote>/<b>`. Not there → `push -u`.
  Ahead only → plain push. Remote only ahead (a GitHub "Commit suggestion",
  a fixer bot) → refused with `git merge --ff-only <remote>/<b>` to take them
  ([req-review #9](req-review.md#9-someone-elses-fast-forward-is-treated-as-a-foreign-push)).
- Diverged and the remote head is in the branch's reflog → `--force-with-lease`
  on that head. A push that moved the remote in between is rejected ("moved
  while pushing"; tested with a `pre-push` hook that pushes first).
- Diverged and the remote has commits that were never ours → refused, even
  with `--force`. chrismo: "this should only ever be force with lease". The
  count excludes our own pre-amend commits (`rev-list --stdin` with the reflog
  negated).
- `--force` only gets past a takeover lock.

`sd act address` publishes through the same code, so its push follows the same
rules: before, a plain `git push` failed on any rewritten branch.

Not built yet: J1, conflict resolution by an agent ([#15]); per-pin bases
([#16]); the native `gh stack` guard ([#17]); retarget, including rewriting
`sd-parent` after the parent merges ([#8]). sd state's `foreign` step counts
the remote commits push would refuse over (`push.theirs`), not `push.behind`, so
our own pre-amend commits under someone else's push aren't counted.

## Process supervision — plan for the loop (not built, [#12])

From Symphony SPEC §7–8 and Sortie (`orchestrator/reconcile.go`, `slot.go`,
`procutil/pgid_unix.go`, ADR-0002): a tick reconciles running work first, then
dispatches into free slots; exits are events with distinct reasons; state is
durable (Sortie's correction to Symphony's in-memory state). Our launchd pass is
short-lived, so it can't own children like their daemons do. Plan:

- The pass never waits on an agent: it launches `sd act <verb> <branch>`
  detached.
- The verb is the supervisor of its own step: new process group, wall-clock
  deadline, writes an outcome record on exit.
- A lease file per running step (pid, pgid, verb, start, deadline, harness) is
  the claim — the missing "step in flight" record (req-review #4).
- Each pass reconciles leases before dispatching: live → skip the branch; past
  deadline → kill the group, "timed out"; pid gone with no outcome → "crashed".
- Global cap on concurrent steps, counted from live leases (start at 2).
- To verify first: whether launchd kills a finished job's leftover process
  group unless `AbandonProcessGroup` is set (from memory, unverified).

## To verify (spikes before building on them)

- ~~`--resume -p` keeps context across processes~~ observed for Claude (fork
  spike, below); `pi -p --session` still unverified [#23]
- resuming a session that's open interactively elsewhere — what breaks? (the
  lock exists to prevent it; confirm it's needed) [#14]
- ~~`--json-schema` for the describe step~~ verified for Claude on the sandbox
  (ship smoke test, 2026-10-14); pi's fenced-json fallback still needs a real
  model run (its default provider's quota was exhausted) [#21]
- GitHub auto-retarget on parent merge [#22]
- ~~patch-id false-escalation rate in A2~~ confirmed real; switch to `-U0` (above)
- **Session ID stability:** Sortie's Claude adapter treats the minted
  `--session-id` as provisional and adopts whatever ID the runtime reports.
  Fork spike: plain `claude -p --resume U` kept U. One short run — keep a
  cheap guard (compare `session_id` in `--output-format json` to the stored ID;
  mismatch → log + adopt).
- **Fork per step instead of plain resume?** Roast resumes with
  `claude -p --resume <id> --fork-session` / `pi -p --fork <id>` by default and
  records the new ID each step emits. The parent session is never mutated, so a
  failed or bad step can be rerun from the same starting point (principle 5),
  and takeover could fork too. Cost: session lineage to track (one ID per step
  vs one per branch). Spike: does a forked step keep full context, and how does
  `sd-session` become "latest good fork"?
  **Claude half observed (2.1.296, haiku, 2026-10-11)** — script
  `spikes/fork-claude.sh`:
  - plain `-p --resume U` keeps session ID U (Sortie's ID-replacement concern
    not observed for plain resume)
  - `--resume U --fork-session` → new random ID; fork has full context
  - `--resume U --fork-session --session-id F` → **ID is F** — the pipeline can
    still mint fork IDs up front, nothing to discover
  - parent U is **unmutated** by forks (resuming U afterwards recalls only the
    pre-fork turns)
  **pi half observed (pi 0.99.1, cloudflare-ai-gateway
  `workers-ai/@cf/zai-org/glm-5.3-flash`, 2026-10-11)** — script
  `spikes/fork-pi.sh` (provider/model via `PI_SPIKE_PROVIDER` /
  `PI_SPIKE_MODEL`). Same four results as Claude:
  - plain `--session U` keeps ID U
  - `--fork U` → new ID (pi's is a UUIDv7) with full context
  - `--fork U --session-id F` → **ID is F**
  - parent U unmutated by forks
  Both harnesses therefore support "mint fork ID up front, parent stays
  clean" with the same shape: Claude `--resume U --fork-session --session-id
  F`, pi `--fork U --session-id F`.

## Known issues

- **FIXED 2026-10-13: NEXT and sd watch's ✅ said "ready to merge" when GitHub
  blocked the merge** (found on a work PR: `reviewDecision:
  REVIEW_REQUIRED`, `mergeStateStatus: BLOCKED`). `sd state` asked only for
  `mergeable` ("no conflicts"), and `ready` was what was left when no other step
  applied.
  - `sd state` now reports `.pr.merge_state`, `.pr.review_decision` and
    `.pr.review_requests` (logins and team slugs).
  - `ready` only when `merge_state` is `CLEAN`, `UNSTABLE` or `HAS_HOOKS`.
    Otherwise the last step is:
    - review required → `wait  waiting on review from <who>`, listed even
      behind other steps. A "nudge them" step after N hours could come later.
    - `UNKNOWN` (or missing) → `wait  GitHub hasn't decided whether it can
      merge yet`
    - `BEHIND` → `run  restack onto main (N behind): GitHub requires branches
      to be up to date → sd act restack` (sandbox spike 2026-10-14: `BEHIND`
      needs "require up to date" plus a required check, and wins over
      `BLOCKED`)
    - anything else (`BLOCKED`, `DIRTY`, …) → `you  GitHub says …: see why →
      gh pr view --web`
    The last two, like `ready`, only show when nothing else does: a pending or
    red CI already explains a `BLOCKED`.
  - Stacked PRs (base isn't the default branch, 2026-10-14): never `ready`
    while the parent's PR is open (`wait  waiting on <parent> (#N) to merge
    first`, listed behind other steps), and after it merged but the PR still
    targets it, `you  <base> already merged: point this PR at main → gh pr
    edit --base main`. A base with no PR of its own (a release branch) is
    unaffected.
  - sd watch: ⏳ when every step is a wait; `blocked` is 🔧. Checked read-only
    on cli/cli#14640 (`REVIEW_REQUIRED` / `BLOCKED`, two users requested).

Open bugs are [issues labeled `bug`](https://github.com/chrismo/sheepdog/issues?q=is%3Aissue+is%3Aopen+label%3Abug);
all work is on the [sheepdog project board](https://github.com/users/chrismo/projects/6).

## Open

- **Scope:** this grew from "keep my PR current" into a small software factory.
  Ship tending first and layer initiation on, or build the full lifecycle?
- First initiator beyond `sd act start` — Linear tickets via `claude-slot` /
  `ticket-sort`?
- First-remote-review rule for unlabeled PRs (proposed above; unconfirmed)
- Home bot command (TBD; config default handles it)
- Watcher interval (default 300 s; revisit after use)
- N2 channel beyond the tab glyph (PushNotification? `say`?)
- Agent-reply marking, if/when agents reply on human threads (deferred)
- **Fork per step** — spike done, both harnesses fork identically; now just a decision
- **Named harness profiles?** Reusable profiles (harness, provider, model,
  effort, args) that steps reference by name, in the config files. Not built;
  today a step names a harness and its block.
- **CI as a glyph on the one-liner** (tabled 2026-10-13): `✅` / `🔴` instead of
  `CI green` / `CI red`, for agent status lines.
- **`ack` as a human stop, once the loop runs on its own** (raised 2026-10-13).
  `sd act ack` (built 2026-10-13) marks `i` items read with no reply, and NEXT
  hands info-only bot items to chrismo (`read … then ack`). On important PRs that
  stop must stay: an approving bot review can still carry a suggestion worth a
  human read. On vibe-friendly tasks it may not be needed: the watcher could
  ack on its own, or an agent could triage `i` items. Revisit with a per-PR
  signal, likely the same label that steers J5.

## Prior art — summary

Full research (two rounds, 2026-10-11) is in `prior-art.md`;
clones in `~/modev/pr-workflow-prior-art/` (sortie, av, gh-stack, attractor, symphony, roast).

- **Nothing to adopt wholesale.** Sortie (closest: same `claude -p` adapter,
  orchestrator-driven) lacks pi, stacks, git-native state, takeover, and lets
  the agent push. Roast (Shopify) matches principles 1–3 and drives Claude +
  pi, but is a run-to-completion engine with no PR lifecycle.
- **Borrowed:** cancelled CI = inconclusive and the handed-off-comment set
  (Sortie); verify gate, split CI cap, autofix-first (Stripe Minions); scope
  check (Spotify Honk's judge); `-U0` patch-id and the Gerrit tree-compare
  candidate; session forking (Roast, pending).
- **Rejected:** agent-drives designs (Symphony, Gas Town); GitHub native stacks
  (`gh stack`, too heavy); LLM-app frameworks (LangGraph, CrewAI, Dify — wrong
  layer).
- **Still distinctive:** stack-aware restack with a post-condition, local review
  before the bot, git-native state.

[#6]: https://github.com/chrismo/sheepdog/issues/6
[#7]: https://github.com/chrismo/sheepdog/issues/7
[#8]: https://github.com/chrismo/sheepdog/issues/8
[#9]: https://github.com/chrismo/sheepdog/issues/9
[#10]: https://github.com/chrismo/sheepdog/issues/10
[#11]: https://github.com/chrismo/sheepdog/issues/11
[#12]: https://github.com/chrismo/sheepdog/issues/12
[#13]: https://github.com/chrismo/sheepdog/issues/13
[#14]: https://github.com/chrismo/sheepdog/issues/14
[#15]: https://github.com/chrismo/sheepdog/issues/15
[#16]: https://github.com/chrismo/sheepdog/issues/16
[#17]: https://github.com/chrismo/sheepdog/issues/17
[#18]: https://github.com/chrismo/sheepdog/issues/18
[#21]: https://github.com/chrismo/sheepdog/issues/21
[#22]: https://github.com/chrismo/sheepdog/issues/22
[#23]: https://github.com/chrismo/sheepdog/issues/23
