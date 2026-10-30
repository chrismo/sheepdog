# sheepdog: the design as intended

The yardstick for the 2026-10-29 architecture review. It says what the
design is supposed to be, so the review can check the code against it.
Written by Claude from the written record, not from chrismo; every claim
carries where it came from:

- **[stated]** quoted or paraphrased from AGENTS.md, `docs/alternatives.md`
  or #79 (Design record). Treat as the design.
- **[inferred: ref]** read out of commit bodies, issue text or module header
  comments. Probably right; chrismo should skim these.
- **[assumed]** Claude's best guess with no written evidence. chrismo should
  confirm or strike each one. A finding that rests on one says so.

## 1. Goal

Take the busy work out of getting a branch to a merged PR and keeping it
current while open, so that with many agents running chrismo can see at a
glance where each one stands: up to date with its parent (stack-aware), CI
green on the head SHA, review current (local and/or bot), bot comments
responded to. **[stated: #79]**

The user is one person, chrismo, running several branches in parallel
worktrees on one or two machines, under both Claude and pi.
**[inferred: AGENTS.md "several sessions in parallel worktrees", #79 harness
table, memory on work laptop]**

## 2. Design rules

The five rules every agent builds by. **[stated: AGENTS.md]**

1. **Deterministic wherever possible.** Scripts observe and act at zero token
   cost; an agent runs only for judgment. Token-spending verbs are
   `implement`, `address`, `describe`, `review`, `restack --resolve`, and
   `start`/`ship` when they run implement or describe. Everything else is
   zero tokens.
2. **The pipeline drives; agents are workers.** The pipeline initiates every
   step and hands an agent one bounded task; the agent edits, and sheepdog
   commits, pushes and talks to GitHub. Agents never need to know the
   pipeline exists.
3. **Harness-agnostic.** Claude and pi both work. The per-harness code is
   `src/harness.rs` plus the tool-rule wording a prompt needs
   (`tool_rules`); nothing else may be per-harness.
4. **Reconcile, don't rely on enforcement.** Never assume sheepdog is the
   only writer: each pass compares observed state to desired state and
   closes the gap. Per-step tool denies are cheap extras, never what
   correctness depends on.
5. **Idempotent, keyed on SHA.** Any step can be rerun safely.

Corollaries the record states alongside the rules:

- **A pass never reaches into another worktree.** Stack cascades are pulled:
  a child notices its parent moved on its own next pass. **[stated:
  alternatives, #79]**
- **One push should follow all the local work**, because every push restarts
  CI and makes bot reviews stale. Hence restack and push are separate verbs,
  and the loop's order is restack → agent step(s) → verify → local review
  until clean → one push → wait for CI → bot. **[stated: #79]**
- **Never over others' commits.** `rebase --onto` plus
  `push --force-with-lease=<branch>:<expected-sha>`; ownership of a remote
  SHA is "it appears in this worktree's reflog for the branch". **[stated:
  #79]**
- **Bot threads only.** Human threads are chrismo's. A thread is resolved
  only after a reply; pushback stays open. **[stated: #79]**
- **Log, don't block** an agent's stray push or PR; reconcile around it.
  **[stated: #79]**
- **Done is an explicit outcome**, `done` / `needs-input` / `gave-up`, not
  process exit. **[stated: #79]**
- **Cancelled CI is inconclusive**, not red; checks on a non-HEAD SHA give no
  verdict. **[stated: AGENTS.md O5]**

## 3. The shape: three layers and a watcher

**[stated: AGENTS.md step labels; #79]** The design names three kinds of
atomic action plus notification:

- **Observe** (`sd state`, read-only): O1 PR lookup · O2 parent state · O3
  sync vs parent · O4 local vs remote · O5 CI on HEAD · O6 review freshness
  · O7 threads owed a reply · O8 session.
- **Act** (`sd act`, mechanical): A0 start · A1 fetch · A2 restack · A3 push
  · A4 retarget · A5 trigger bot · A6 rerun flaky CI (unbuilt) · A7 reply /
  resolve · A8 local review (spends tokens) · A9 attach · A10 verify · A11
  autofix.
- **Judgment** (agent steps in the branch's session): J0 implement · J1
  resolve conflicts · J2 fix red CI · J3 triage findings · J4 write reply ·
  J5 re-review needed? (#66, unbuilt) · J6 PR description.
- **Notify:** N2 surface to chrismo (glyph, notification) · N3 seen markers.

Intended relationships between them:

- **Observe is the single source of truth.** `sd state --json` is what `sd
  act` and `sd watch` read; the verbs' precondition checks *are* the state
  machine's transition guards. **[stated: #79 "Build order"; state.rs
  header]**
- **NEXT is derived from state, nothing else.** `src/next.rs` turns the state
  JSON into the ordered list of steps; glyphs are read from NEXT so the tab
  and NEXT can't disagree. **[stated: #79 "Watcher"]**
- **Act verbs check their own preconditions through `sd state`** and refuse
  with reasons; `--force` overrides soft ones. **[stated: #79]**
- **Every judgment step is wrapped by mechanical steps**: the pipeline
  prepares the input, runs the agent for one bounded task, then commits,
  verifies, pushes and posts. The agent never commits, pushes or talks to
  GitHub. **[stated: rule 2; address.rs, implement.rs, ci_fix.rs headers]**
- **The watcher (`sd watch`) is one launchd job across all enrolled
  branches**, fetch first; a pass runs `sd state` per branch as a child
  process it can time out, glyphs it, notifies per policy, and with
  `autoRun` launches a branch's first NEXT step when that step is one a pass
  may run. **[stated: alternatives; pass.rs header; #267 #269]**
- **`sd state` runs out of process from the watcher on purpose**, so a hung
  one can be killed. **[stated: pass.rs header]**

### One watch pass, as intended **[inferred: pass.rs header, #12, #267-#271]**

1. Fetch each enrolled repo once.
2. For each enrolled branch: run `sd state --json` as a child with a
   deadline; diff against the state kept from the last pass; set the tab
   glyph from NEXT; notify what's new per the notification policy.
3. Reconcile running markers: a lease past its deadline is ended; a crashed
   step is counted as failed.
4. With `autoRun`, launch the first NEXT step on branches whose step is
   launchable, under a concurrency cap, pausing on a harness usage limit.
5. Keep per-branch state for the next pass.

### One act verb, as intended **[inferred: step.rs, address.rs, implement.rs headers]**

1. Observe (`sd state --json`), check the verb's preconditions, refuse with
   reasons if unmet.
2. Take the running marker (agent steps) and check the attach lock.
3. Do the mechanical part, or build the prompt and run the harness for one
   step in the branch's session.
4. Parse the outcome; commit the agent's edits; verify; record usage.
5. Post-conditions: push with lease, post replies, pin refs, record
   failures; release the marker.

## 4. State: where it lives and who writes it

**Git-native, no JSON state file.** **[stated: #79 "Storage"]**

| What | Where | Why there | Source |
|---|---|---|---|
| Per-branch records: enrolled, parent, harness, session, lock, task, handled, ci-fix flaky SHA | `git config branch.<b>.sd-*` | removed with the branch by `git branch -D`; shared across worktrees | [stated: #79; ci_fix.rs header] |
| Pins: base, local, bot, accepted, each with a `-base` | `refs/sheepdog/<b>/*` | a ref pins the object against gc; refs are shared across worktrees and stay local (not in default refspecs) | [stated: #79] |
| Failure record, verify-red record | `refs/sheepdog/<b>/failed`, `.../verify-red` (one JSON line each) | same reasons as pins | [stated: failed.rs, verify_red.rs headers] |
| Settings | JSON config files, global plus per-branch overrides only | git config layers only the same key; a full per-branch copy froze the branch | [stated: #79] |
| Running marker / lease | `$SHEEPDOG_STATE_DIR/<branch file>`, one or two lines | read by sd state; the watcher's reconcile ends leases past deadline | [stated: running.rs header] |
| Usage ledger | `$SHEEPDOG_STATE_DIR/usage.sup`, one SUP line per agent step | cost per step and per branch; format fixed for compatibility with the bash era | [stated: usage.rs header] |
| Usage-limit pause | `$SHEEPDOG_STATE_DIR/limit` | one file for every harness and model | [stated: limit.rs header] |
| Watcher per-branch state (last pass's state, seen markers) | `$SHEEPDOG_STATE_DIR` | so a rerun reports only what's new (N3) | [inferred: pass.rs, AGENTS.md N3] |
| Stored PR description for HEAD + target | unknown location | ship finds it | [inferred: describe.rs header; location not stated] |
| Lineage (parent) | `branch.<b>.sd-parent` | must be local: GitHub retargets a child to main when the parent merges | [stated: #79, #22] |

Intended rules about state:

- **Keyed on SHA.** Pins, flaky records, verify-red and stored descriptions
  are tied to a commit, so a new commit leaves them standing on an old one
  and NEXT ignores them. **[stated: rule 5; verify_red.rs header]**
- **Staleness is diff since a pin, never a timestamp**, because rebase
  rewrites dates. **[stated: #79]**
- **Refs never go on their own**; `sd act forget` and the watcher's vacuum
  sweep them. **[stated: #79; watch.rs header]**
- **sheepdog is not the only writer** of any of this. chrismo can push, merge,
  reset, or delete branches by hand; agents can stray; GitHub retargets.
  Every reader reconciles. **[stated: rule 4]**
- **`sd state --json` output is byte-compatible with the bash era**
  (`src/json.rs`). **[stated: state.rs, json.rs headers]** Whether that
  constraint still earns its keep after the bash is gone is an open
  question, not a design statement. **[assumed]**

## 5. Module ownership, as intended

From the module header comments. **[inferred: headers]** This is what each
module says it owns; the review checks whether it does, and only that.

| Module | Owns |
|---|---|
| `main.rs` | verb dispatch, including the `sd-<verb>` link name |
| `state.rs` | Observe: O1-O8 into one JSON document |
| `next.rs` | NEXT: the ordered step list derived from state JSON |
| `act/mod.rs` | `sd act` dispatch and shared verb plumbing |
| `act/step.rs` | what every agent step shares: session, guidance commit, allowed tools, standing question |
| `act/<verb>.rs` | one verb each; `commands.rs` is verify and autofix; `hold.rs` the CI-config hold and the merged/closed PR gate; `bookkeeping.rs` enroll/unenroll/forget/unregister; `github.rs` the GitHub-writing verbs (ack, trigger) |
| `harness.rs` | the only per-harness code: running a step headless, opening the TUI, tool-rule wording |
| `guidance.rs` | the guidance part of a prompt and the shipped fallbacks under `lib/prompts/` |
| `review_local.rs` | the default local reviewer, and the contract any custom one keeps |
| `config.rs` | `sd config`: the settings files, layering, step-to-harness fallback, profiles |
| `git.rs` | every git invocation |
| `proc.rs` | running a command: timeouts, output capture, process-start identity |
| `running.rs`, `failed.rs`, `verify_red.rs`, `limit.rs`, `usage.rs` | one state record each |
| `watch.rs`, `watch/pass.rs` | the watcher: install, glyph, status; the pass, refresh, vacuum |
| `json.rs`, `val.rs`, `time.rs` | compatibility shims from the bash/SuperDB era: byte-exact JSON, null-tolerant field reads, UTC formatting |

Intended boundaries that follow from the rules and headers:

- **Only `harness.rs` knows which harness is running.** **[stated: rule 3]**
- **Only `git.rs` runs git; only `github.rs` and the Observe layer run
  `gh`.** **[assumed: git.rs header says "what the verbs ask of it"; nothing
  states gh is centralized]**
- **`next.rs` reads only the state JSON**, never git, gh or the filesystem.
  **[inferred: next.rs header "from its sd state JSON"]**
- **`state.rs` is read-only**: it writes nothing to git, GitHub or the state
  dir. **[stated: AGENTS.md "Observe (read-only)"]**
- **Verbs don't call each other's internals**; a verb that needs another
  verb's effect (ship → describe, ship → push, ci-fix → verify) calls the
  shared function the other verb is built on. **[assumed]**

## 6. Trust boundaries

| Boundary | Intended rule | Source |
|---|---|---|
| sheepdog ↔ agent | the agent edits the worktree and returns an outcome; it never commits, pushes, opens PRs or posts. Denies are a cheap extra. A stray action is observed and reconciled, logged not blocked. | [stated: rules 2, 4; #79 reconciliation table] |
| sheepdog ↔ GitHub | sheepdog posts replies only on bot threads; retargets, triggers, pushes with lease. Nothing is pushed or posted onto a PR that merged or closed while a step ran. | [stated: #79; hold.rs header] |
| sheepdog ↔ chrismo | chrismo merges, answers standing questions, handles human threads, and can take over a session (attach). While attached, agent steps refuse; while a step runs, attach refuses. | [stated: #79; attach.rs header] |
| branch ↔ its own review | the review prompt is read from the base, not the branch, so a branch can't rewrite its own review; the reviewer is a fresh read-only session. | [stated: review_local.rs header] |
| agent-edited CI config | an agent commit touching a protected path is held, not pushed, until chrismo looks. | [stated: hold.rs header, #124] |
| sheepdog ↔ other sheepdogs | one branch per worktree; a pass never writes to another worktree; shared refs and config are read across worktrees. The running marker and attach lock serialize steps on one branch. | [stated: #79; running.rs, attach.rs headers] |
| cost | unknown spend isn't $0; fail closed. A killed step is charged its real spend. Per-branch budget. | [inferred: #121 comment via alternatives, #276, #148] |

## 7. Invariants the review checks against

Each is a testable statement derived from the above. The review passes
report which hold, which don't, and which can't be determined.

**Rules**

- I1. No `gh`, `git push`, `git commit` or GitHub-writing call is reachable
  from inside an agent step's prompt contract; the pipeline does them after
  the step. [rule 2]
- I2. Grepping for `"claude"` / `"pi"` as harness names hits only
  `harness.rs` (and `tool_rules` wording). [rule 3]
- I3. Every mechanical verb is rerunnable at the same HEAD with no change in
  effect. [rule 5]
- I4. No code path assumes sheepdog made the last write: every pin, marker
  and record is validated against observed state before use. [rule 4]
- I5. Every token-spending verb is one `sd act --help` marks "Spends tokens",
  and no other verb invokes a harness. [rule 1]

**State**

- I6. Every record is keyed on a SHA or validated against HEAD before it
  drives a decision. [§4]
- I7. No timestamp is used to decide staleness. [§4]
- I8. Every record has exactly one writer module and that module's header
  names it. [§5]
- I9. Records removed with a branch (git config) vs. records needing a sweep
  (refs, state-dir files) are each cleaned on their path; nothing is
  orphaned by `forget` or vacuum. [§4]

**Boundaries**

- I10. `next.rs` imports nothing that touches git, gh or the filesystem.
- I11. `state.rs` performs no write.
- I12. All git invocations go through `git.rs`. [assumed]
- I13. All gh invocations go through one place. [assumed]
- I14. The running marker is taken before any agent step's harness call and
  released on every exit path, including signals and timeouts.
- I15. The attach lock is checked before commit and before push inside a
  step that started before attach. [attach.rs header]

**Watcher**

- I16. A pass never blocks on one branch: every child has a deadline.
- I17. Glyph and NEXT come from the same NEXT output.
- I18. The pass launches at most the configured concurrency of agent steps,
  and none while a usage-limit pause stands.

## 8. Open design questions the record already names

Not findings; the design hasn't decided. **[stated: alternatives "Still
open"; question-labeled issues]**

- Fork per step vs. one persistent session per branch (#69).
- Accept warnings at the review cap (#142).
- Ready to merge while behind the base: the merged combination never ran in
  CI (#158).
- NEXT reviews a human's commit while CI is pending on it (#155).
- Diff-check flags under an unattended loop: hold or notify (#146).
- Human threads handed to the agent on request (#72).
- Config architecture: everything in JSON, one schema (#283). This one is
  large enough that the review should treat config as "in flux", not as a
  finding source.
- One editing step for implement / review-fix / address / ci-fix (#133).
  Same: the four verbs' duplication is a known, scheduled consolidation.

## 9. What this page assumes and chrismo should check

Collected from the **[assumed]** tags above:

1. Byte-compatibility of `sd state --json` with the bash era is a
   compatibility shim, not a design goal, now that the bash is gone.
2. `gh` invocations are meant to be centralized the way git's are.
3. Verbs share functions rather than calling each other's entry points.
4. The intended consumer is one person on a few machines; nothing is meant
   to be multi-user.
