# Alternatives considered

What was weighed and not chosen, so the "why not" is in one place. This file
is the record: it began as a summary of #79 (Design record) and #80 (Prior art
record) and now stands on its own, so the reasoning doesn't need those issues.
Issues stay as the inbox for things to look at later and for work still to do.

Two parts: our own design choices that were rejected, and other tools we read.

## Rejected design alternatives


| Instead of | We chose | Why not the alternative |
|---|---|---|
| `/loop` per session as the watcher | One launchd job across all enrolled branches, fetch first | Tokens per idle wake; dies with the session |
| Built-in PR subscriptions | Same | Claude-only, untested |
| Git hooks | Same | Blind to GitHub-side events |
| PreToolUse / Stop hooks to enforce agent discipline | "Pipeline drives" plus reconciliation | Claude-only |
| Agent drives the PR loop | Pipeline drives; agents are workers | Costs tokens; stochastic where it needn't be (Symphony, Gas Town are this camp) |
| Adopt agent-orchestrator | Self-built | Evaluated and not adopted; round 2 found nothing to adopt wholesale |
| Cross-worktree writes for stack cascades | Cascades are pulled: a child notices its parent moved on its own next pass | A pass never reaches into another worktree |
| Process exit as the done signal | Explicit outcome (`done` / `needs-input` / `gave-up`) via `--json-schema` on Claude, a parsed fenced-json block on pi | Exit can't tell finished from asking |
| Plain rebase, or any `--force` | `rebase --onto` + `push --force-with-lease=<branch>:<expected-sha>` | Squash merges defeat a plain rebase (patch-id skip can't match P1..Pn against one squash commit); never overwrite others' commits |
| Default patch-id for the restack post-condition | `git diff -U0 \| git patch-id --stable` | The default includes context lines; an upstream edit inside our diff's context gave different IDs (git 2.53, 2026-10-11) |
| Extra ownership record for force-push safety | Reflog: a remote head in `git reflog show <branch>` is ours | Reflog is deterministic and needs no record; per-worktree fits one branch per worktree. "Not ours" means "this copy never had them", not authorship (#24) |
| Restack and push as one verb | Separate verbs | Every push restarts CI and makes bot reviews stale; one push should follow all the local work |
| PR bot as the reviewer | Configurable local review script | Preferred whenever it suffices and able to replace the bot; `/marge` not required at work. Findings go to the branch's own agent as the next step, since it already knows why the code looks the way it does; isolation is the reviewer's job (separate process, fresh context, read-only tools) and the agent never invokes it |
| Agent replying on every thread | Bot threads only | Human threads are chrismo's. A7 resolves a thread only after replying; pushback stays open (Cursor's rule) |
| Timestamps for bot staleness | Pin HEAD at trigger; staleness = diff since the pin | Rebase rewrites committer dates; timestamps only answer "has the bot replied since the trigger" |
| Cycle cap reset per HEAD | Per loop run, reset on success, split by cost (local 3, CI 2, bot 2; cap hit → needs chrismo) | Every fix commits, so a per-HEAD reset would never trip. CI rounds are slow and expensive (Stripe's rule). Revisit after use; the loop isn't built (#12) |
| Per-step turn cap | Not used | Neither CLI has a turn-cap flag (`--help`, 2026-10-11); `--max-budget-usd` bounds Claude steps |
| OpenHands-style "was it incorporated?" check before replying | Not used, for now | Revisit if fix steps move to cheaper models |
| GitHub native stacks (`gh stack`) | Own restack | Too heavy for local automation; guard only, #17 |
| JSON state file | Git-native: branch config, `refs/sheepdog/*`, JSON config files | Refs and repo config are shared across worktrees; a ref pins the object against gc, a SHA in a file would not; the refs stay local; lineage must be local because GitHub retargets a child PR to `main` when the parent merges (#22) |
| Settings in git config | JSON config files (2026-10-13) | Git layers only the same key across scopes; the step-to-harness fallback needed its own resolver whichever store held the values. No git-config fallback, since nobody had any set |
| A branch file holding a full copy of the settings | Overrides only (2026-10-13) | A full copy froze the branch, so later global edits never reached it |
| Claude's `--bg` / `claude attach` | Headless `-p` + resume | Claude-only |
| Blocking an agent's accidental push or PR | Log, don't block; reconcile | A detected push is recorded and surfaced; a repeat offender means the step-contract prompt needs tightening. Per case: a push with no PR yet makes ship's push a no-op; a push with a PR costs one early CI run, since "ready" and the bot trigger need the `local` pin to cover HEAD; a force-push after an amend is ours if the remote SHA is in the reflog, else refused; an agent's `gh pr create` makes ship find-and-fix-up title, body and base; a push to the wrong branch (`HEAD:main`) is not ours to absorb, server-side branch protection covers it |
| Stay in bash, or Go with super embedded as a library | Rust, native, side by side on main, verb by verb (decided 2026-10-21, #138, plan #193) | SuperSQL is a means, not a goal: the queries reshape one small JSON document each, and most of their text works around SuperDB's missing-field and empty-subquery behavior. Bash was never a chosen language, and the 2026-10-17 spike measured its costs: one `sd state` took about 6 s, 3.4 s of it in 56 super calls, and parsing dominates (PRN_QUERY 638 ms to parse, 1.3 ms to run; the config type-check parsed 7 times per run); upstream main parses only ~5% faster. Process supervision for the loop (#12) needs perl in bash. Go with embedded super would keep the queries and the black-box bats suite, but Rust is the cleaner end state once SuperSQL is a means |

Build order was also a choice: manual first (2026-10-12), every step an
`sd act` verb run by hand, each verb checking its own preconditions with
`sd state`. Reordered for dogfooding (2026-10-13): first what keeps chrismo's
own branches current and reviewed, last what chrismo already does by hand
(start, PR creation).

## Other tools we read

Read for prior art, not run, except where an entry says otherwise. The table is
a neutral side-by-side of what each tool documents: an empty or "not stated"
cell means the sources don't say, not that the feature is missing. The entries
below hold the rest of what was read. Sources: #79 and #80 (2026-10-11), the
Amp docs (2026-10-17), shallow clones in `~/modev/pr-workflow-prior-art/`
(Symphony at be10a1b, 2026-09-15; Roast at 40b87a5, 2026-10-06; Open SWE at
c560ff7, Nasiko at 50b0a34, Evener at a4dcbc1 and samcdavid/dotfiles at
2e73af1, 2026-10-16), READMEs for Archon and Gas Town (2026-10-18) and for
shepherd-pr (2026-10-21). Nothing but sheepdog was run, so treat the rest as
claims.

| Tool | Who drives | Where it runs | Harnesses | Stacks | Who pushes / opens the PR |
|---|---|---|---|---|---|
| sheepdog | Pipeline; agents are bounded workers | Local, worktree per branch | Claude, pi | Yes: `rebase --onto` restack with a patch-id post-condition. Native GitHub stacks only guarded (#17) | Pipeline. Agents don't need to know it exists |
| Sortie | Orchestrator | Fresh clone per ticket; Go | Claude, Copilot, OpenCode, Codex, Kiro, Gemini, generic ACP | No | The agent or a hook; read-only on the forge |
| Archon | Pipeline (YAML DAG workflows in the repo) | Local binary; `archon serve` web console; Docker / VPS | Claude, Codex, Pi | Not stated | `archon-pr` opens the PR; merging not stated |
| Roast (Shopify) | Pipeline (Ruby DSL of cogs) | Run-to-completion engine driving local agent CLIs | Pi (default), Claude Code; API-backed `chat` cog | No (per #80) | Not stated |
| Symphony (openai) | Agent, via the repo's `WORKFLOW.md` and skills | Per-issue workspace, Linear polling | Codex | Not stated | Agent, through its `push` and `land` skills |
| Gas Town | Agent (long-lived tmux agents) | Local: per-agent worktrees, a per-rig Refinery merge queue | Claude Code by default; presets for gemini, codex, kiro, cursor, auggie, amp, opencode, copilot, pi, omp, custom | Not mentioned; integration branches (#80) | `gt done` pushes a branch and creates an MR bead; whether MRs are GitHub PRs is not stated |
| Open SWE | Agent (a Deep Agents / LangGraph graph) | Persistent per-thread cloud sandboxes | Its own agent | Not stated | Not stated |
| shepherd-pr | Agent, on the user's request; installing the skill starts nothing | Local (Bash, Python 3 stdlib, authenticated `gh`; macOS or Linux; GitHub.com only) | Guides for Claude Code, Cursor, Codex, Devin CLI, Kimi Code, Gemini CLI, OpenCode, Pi, Hermes Agent and others | Not stated | The agent pushes scoped fixes to an authorized branch; the bundled tools write nothing. Asks before comments, reruns or merges |
| Amp PR follow-up | Agent, in its thread | Cloud (orbs), GitHub App and webhooks | Amp's own agent | Not stated | Not stated; writes run as the user |

### The agent drives the loop (opposite of design rule 2)

**Symphony (openai/symphony).** 27.6k★, v0.0.3 2026-09-15. SPEC.md plus an
Elixir reference implementation; no durable database by design, so restart
recovery comes from the tracker and filesystem, and the tracker state is the
human lock. The reference `WORKFLOW.md` has the agent sweep PR comments,
reviews and checks before `Human Review`, treating every actionable comment as
blocking until addressed or pushed back, and the agent replies inline per
comment (accept, clarify or push back). Its `land` skill watches checks,
merges `origin/main` to resolve conflicts, fixes failures and squash-merges
when green. Local review is a plan self-review in the workpad.

**Gas Town.** 18.3k★. State is Beads (git-backed issue tracking) plus
git-worktree hooks. The Refinery batches merge requests, runs verification
gates and merges to main with a Bors-style bisecting queue; code review and CI
handling are not described. "Polecats never push directly to main." Takeover:
`gt mayor attach` joins the Mayor session, `gt crew add` gives a person a
personal clone, `gt seance` queries predecessor sessions; the README doesn't
use the word takeover.

**Amp PR follow-up** (docs read 2026-10-17, announced 2026-10-12;
[news](https://ampcode.com/news/github-listeners),
[docs/github](https://ampcode.com/docs/github),
[docs/orbs/event-driven](https://ampcode.com/docs/orbs/event-driven)). A
hosted product, cloud / orbs only.

- **Subscribing:** asked for in the receiving thread (repo, PR number, handling
  instructions). When GitHub subscription tools are available, Amp is
  instructed to subscribe before reporting a PR complete, unless the user opts
  out.
- **Events:** PR comments, submitted reviews and inline review comments; issue
  comments and closed / reopened; CI as a check rollup for an exact commit, a
  named Actions workflow, or a named check or commit status.
- **Delivery:** events continue the subscribed thread. Amp fetches the linked
  content, summarizes it and follows the thread's current task; "CI messages
  steer an active agent." "Delivered" means the thread accepted the event, not
  that the agent finished.
- **Spawners:** a new thread and orb per newly opened issue or PR, filtered
  only by a label present at open. Comments and CI can't spawn.
- **Trust:** subscriptions "do not grant permission to push changes, deploy, or
  carry out instructions someone posts in a GitHub comment." Writes run as the
  user, so branch protection and org permissions apply.
- **Not stated:** whether the agent fixes failing CI or review feedback by
  itself, merging, limits, cost, the CI polling interval, filters beyond
  labels.

**shepherd-pr** (prime-radiant-inc, README read 2026-10-21). "A portable
coding-agent skill for taking a GitHub pull request through CI and review to
an authorized merge, or reporting an evidenced blocker." Its bundled tools are
read-only: a change watcher ("a change feed, not a gate verdict"), a settle
detector (head unchanged, no outstanding check, RoboRev's review names that
exact head), a review reader (RoboRev states passed, current, stale,
review-failed) and stale-PR triage. State is local snapshots and locks under
`~/.local/state/shepherd-pr`, keyed by repo and PR. Session-owned jobs may stop
when the session ends. The README warns that a generated harness guide is not
proof of a verified install. Closest in spirit to `sd state`, without the
pipeline, stacks or unattended loop. Thread replies, local review before bots
and takeover are not stated. 2★, 0 forks, MIT, 21 commits, no releases.

**Open SWE (langchain-ai/open-swe).** A software-factory system on Deep Agents
and LangGraph: issue- or conversation-triggered coding, PR review, persistent
per-thread cloud sandboxes and a `/baby-sit` monitor for opted-in PRs. Broader
and more platform-oriented than sheepdog. The closest reference for the CI
feedback loop (`openswe/baby_sit.py`, `openswe/github/ci.py`, the
`baby-sit-failure` prompt), read from source:

- Retry state and each dispatch fingerprint are pinned to the exact head SHA;
  the count resets when the head changes and a delayed retry naming a stale SHA
  is rejected.
- Reruns need evidence of a transient failure and are capped at three per head;
  the agent does the rerun itself, so the server's cap is on recording them.
- CI outcomes are modeled as pending, success, failure, or terminal but
  unrecognized / blocked, not "every non-green is fixable".
- Before readiness it verifies that required checks have actually reported,
  through classic branch protection and rulesets.
- Dispatch fingerprints persist and watch evaluation is serialized with a
  lease-like lock, so concurrent webhooks don't start duplicate agent runs.

Taken from it: #123 (undraft waits for required checks), #124 (agent steps
that touch CI config commit and hand off, they don't push), #128 (describe
follows the repo's PR template).

### Pipeline and workflow engines

**Sortie (sortie-ai/sortie).** A design reference, not a base. Adopting means
giving up pi, giving up (or adding) stacks, accepting SQLite and letting the
agent push (its `WORKFLOW.md` puts `git push` in a hook). Its reaction
contracts (architecture docs 12–16 and 19, the ADRs) map onto O5–O7 / J2–J4:
documented contracts for CI, review, auto-merge, bot review and merge
conflict, with cancelled CI as inconclusive. A "self-review" phase (verify
commands plus an agent review turn) runs before completion is accepted. It
never replies to or resolves threads, and has no takeover, only a parking
label. 198★, created 2026-03-17.

**Archon.** Closest to principles 1–2 in philosophy, but it has a web UI and
no stack support stated. #80 called it Claude-centric; the README (2026-10-18)
lists Claude, Codex and Pi. Triggered from the web UI, CLI, Telegram, Slack,
Discord or GitHub webhooks; worktree per run or reuse; SQLite or PostgreSQL;
workflows are YAML under `.archon/workflows/`. `archon-deliver`: implement →
draft PR → review → fix findings → validate → CI → ready for review (how a CI
failure is handled is not stated); `archon-review` reviews through parallel
specialist lenses; workflows can pause for human approval; runs persist and
resume. 23.7k★, MIT.

**Roast (Shopify).** Matches principles 1–3 and drives Claude and pi, but is a
run-to-completion engine with no watcher, no PR lifecycle (nothing under
`examples/` or `tutorial/` mentions pull requests) and no stacks. A reference
for step and session mechanics: each agent result carries a session id, and
sessions are forked by default (Claude `--fork-session`, pi `--fork`).
1.25k★, MIT.

**agent-orchestrator (AO).** Not adopted: a desktop app plus daemon, telemetry
on by default, repo moved 3×, 827 open issues, and it owns agent launch. Its
reaction model inspired the watcher.

**Attractor, continuous-claude, Ralph.** Pipeline or loop engines; not
adopted.

**LangGraph, CrewAI, Dify.** Wrong layer: they build agents from raw model API
calls, so they'd replace Claude / pi rather than drive them (breaks principle
3) and add a Python / server runtime. CrewAI's role-play is the opposite of
principle 1.

### Harnesses, runtimes and platforms

**OpenHands.** A harness (peer of Claude / pi), not an orchestrator. Only its
resolver's verify-before-reply idea applies, and it's parked until fix steps
move to cheaper models.

**Evener (prime-radiant-inc/evener).** A local coding-agent runtime with a
hub / UI, sessions, delegates, managed worktrees and durable background jobs;
not a PR lifecycle manager. Its worktree API is far more than sheepdog needs,
but its safety rules are a checklist (`docs/developing-evener/worktrees.md`,
`docs/job-control.md`):

- Git worktree locks guard active lanes; a crash leaves the lock as fail-safe
  residue and recovery needs the owner identity and an explicit unlock.
- Cleanup is conservative: dirty or unmerged worktrees are kept and `prune`
  says why it skipped one. Branch deletion needs evidence the work is captured
  elsewhere, not Git's context-dependent "merged".
- Its cherry-based check can prove a single-commit squash; a multi-commit
  squash can't be proven by ancestry, so those lanes are left for explicit
  attestation.
- Worktree summaries show branch, lock, dirtiness, age and commits ahead.
- Restart reconciliation reports lost processes as `runtime_lost` rather than
  implying they resumed.

Not imported wholesale. Taken: a comment on #12 (the loop): PID-reuse-safe
liveness, quota limits pause the loop, an outcome vocabulary.

**Nasiko (Nasiko-Labs/nasiko).** An adjacent platform: an agent runtime and
control plane that discovers and routes existing coding harnesses, accounts
for model usage (much deeper than first read), and deploys A2A agents. Its
coding-agent example puts filesystem and shell behind a `Sandbox` trait with a
local implementation; the remote phase is marked not started, so it isn't
shipped. No end-to-end PR / CI shepherd. A modest reference if execution
backends ever need to vary. Taken: a comment on #121 (cost guards): unknown
spend isn't $0, fail closed, transcript context size.

**Claude Code Projects beta.** Cloud-only at launch per reports; not usable
for local worktrees and stacks.

**Other ticket → agent → PR daemons** (symphony-ts, Copilot coding agent, Codex
cloud, Cursor background agents, Devin). None is local + worktree + pi +
stack-aware.

**herdr, vibe-kanban, claude-squad, superset, omnigent, emdash.** Terminal
runtimes or dashboards with no PR loop, or shut down.

### Stack tools and rebase strategies

**GitHub native stacks (gh-stack).** Not adopted, #17: server-side restack, but
too heavy for local automation. Guard only.

**ezyang/ghstack, spacedentist/spr.** Never force-push PR branches (they append
"base update" commits or merges) to keep review threads anchored. Informs
O7's outdated-thread handling; not adopted.

**ejoffe/spr, modular/stack-pr, mergify-cli.** One commit per PR with identity
in a trailer; doesn't fit the branch-per-PR model.

**Gerrit's TRIVIAL_REBASE / NO_CODE_CHANGE** (in-memory replay plus tree
compare). A candidate for #66 (Re-review policy); not adopted.

### Skill collections

**samcdavid/dotfiles `claude/skills`.** Short skills. On 2026-10-14 the author
deleted their own "multi-stage pipeline, runner agents, verifier tiers" in
favor of them; the shallow clone doesn't show why. Taken: #125 (a zero-token
diff check for new suppressions, test skips and edited existing tests), #126
(local review: an importance bar and a self-check before each finding), #127
(default prompt wording for the editing steps).

### Candidate changes from the prior art (decided 2026-10-11)

Adopted 1–4, rejected 5:

1. Deterministic verify before A8 (`settings.verify`): adopted.
2. Split caps (local 3, remote CI 2, autofix before J2): adopted.
3. Scope judge via `SHEEPDOG_TASK`: adopted.
4. Cheap per-step tool deny alongside reconciliation: adopted.
5. Per-step turn cap next to `--max-budget-usd`: **rejected**, no turn-cap flag in either CLI.

## Still open

- Fork per step vs. one persistent session per branch: #69.
- Whether sheepdog accepts warnings at the review cap: #142.

No tools are queued to read; the earlier list (#37: Open SWE, Nasiko, Evener) is
done and recorded above.
