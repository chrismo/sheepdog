# Alternatives considered

Pulled from two issues on 2026-10-17: #79 (Design record) and #80 (Prior art
record). They stay the source for the full reasoning; this file collects only
what was weighed and not chosen, so the "why not" is in one place. Open
decisions have their own issues and are listed at the end.

## Rejected design alternatives (#79)

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

Build order was also a choice: manual first (2026-10-12), every step an
`sd act` verb run by hand, each verb checking its own preconditions with
`sd state`. Reordered for dogfooding (2026-10-13): first what keeps chrismo's
own branches current and reviewed, last what chrismo already does by hand
(start, PR creation).

## Prior art: rejected or not adopted (#80)

| Alternative | Verdict |
|---|---|
| Sortie (sortie-ai/sortie) | Design reference, not a base. Adopting means giving up pi, giving up (or adding) stacks, accepting SQLite, and letting the agent push. Its reaction contracts (architecture docs 12–16 and 19, the ADRs) map onto O5–O7 / J2–J4 |
| Roast (Shopify) | Matches principles 1–3 and drives Claude and pi, but is a run-to-completion engine with no watcher, no PR lifecycle, no stacks. Reference for step and session mechanics |
| Symphony (openai/symphony), Gas Town | Agent-drives designs; the opposite of principle 2 |
| Amp PR follow-up (docs read 2026-10-17, not run) | Agent-drives: GitHub events (PR comments, reviews, CI) are routed into the agent's thread and the agent decides. Cloud / orbs only. See below and #80 |
| GitHub native stacks (gh-stack) | Not adopted, #17. Server-side restack, but too heavy |
| agent-orchestrator (AO) | Desktop app + daemon, telemetry on by default, repo moved 3×, 827 open issues, owns agent launch. Its reaction model inspired the watcher |
| Archon, Attractor, continuous-claude, Ralph | Pipeline or loop engines; Archon is the closest to principles 1–2 in philosophy but it has a web UI and no stack support stated. #80 called it Claude-centric; its README (2026-10-18) lists Claude, Codex and Pi |
| LangGraph, CrewAI, Dify | Wrong layer: they build agents from raw model API calls, so they'd replace Claude / pi rather than drive them (breaks principle 3) and add a Python / server runtime; CrewAI's role-play is the opposite of principle 1 |
| OpenHands | A harness (peer of Claude / pi), not an orchestrator. Only its resolver's verify-before-reply idea applies |
| Gerrit's TRIVIAL_REBASE / NO_CODE_CHANGE (in-memory replay + tree compare) | Candidate for #66 (Re-review policy), not adopted |
| ezyang/ghstack, spacedentist/spr | Never force-push PR branches (append "base update" commits or merges) to keep review threads anchored; informs O7's outdated-thread handling but not adopted |
| ejoffe/spr, modular/stack-pr, mergify-cli | One commit per PR, identity in a trailer; doesn't fit the branch-per-PR model |
| Claude Code Projects beta | Cloud-only at launch per reports; not usable for local worktrees and stacks |
| Other ticket → agent → PR daemons (symphony-ts, Copilot coding agent, Codex cloud, Cursor background agents, Devin) | None is local + worktree + pi + stack-aware |
| herdr, vibe-kanban, claude-squad, superset, omnigent, emdash | Terminal runtimes or dashboards with no PR loop, or shut down |

### Candidate changes from the prior art (decided 2026-10-11)

Adopted 1–4, rejected 5:

1. Deterministic verify before A8 (`settings.verify`): adopted.
2. Split caps (local 3, remote CI 2, autofix before J2): adopted.
3. Scope judge via `SHEEPDOG_TASK`: adopted.
4. Cheap per-step tool deny alongside reconciliation: adopted.
5. Per-step turn cap next to `--max-budget-usd`: **rejected**, no turn-cap flag in either CLI.

### Amp PR follow-up (#80, docs read 2026-10-17)

Announced 2026-10-12 ([news](https://ampcode.com/news/github-listeners));
details from [docs/github](https://ampcode.com/docs/github) and
[docs/orbs/event-driven](https://ampcode.com/docs/orbs/event-driven). Nothing
was run.

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

## Feature comparison

A neutral side-by-side, not a scorecard: the columns are what each tool
documents, so an empty cell means the sources don't say, not that the feature
is missing. Sources are #79 and #80 (2026-10-11), the Amp docs (2026-10-17) and
this repo's AGENTS.md. Symphony and Roast were read from the shallow clones in
`~/modev/pr-workflow-prior-art/` (Symphony at be10a1b, 2026-09-15; Roast at
40b87a5, 2026-10-06), Archon and Gas Town from their READMEs on 2026-10-18.
Nothing but sheepdog was run, so treat the rest as claims.

| Tool | Who drives | Where it runs | Harnesses | Stacks | State | CI / review reactions | Who pushes / opens the PR |
|---|---|---|---|---|---|---|---|
| sheepdog | Pipeline; agents are bounded workers | Local, worktree per branch | Claude, pi | Yes: `rebase --onto` restack with a patch-id post-condition. Native GitHub stacks only guarded (#17) | Git refs + git config + JSON config files | `sd state` observes CI on the head SHA, review freshness and threads owed a reply; `sd act` verbs act. The unattended loop is not built (#12) | Pipeline. Agents don't need to know it exists |
| Sortie | Orchestrator | Fresh clone per ticket; Go | Claude, Copilot, OpenCode, Codex, Kiro, Gemini, generic ACP | No | SQLite | Documented contracts for CI, review, auto-merge, bot review, merge conflict; cancelled CI = inconclusive | The agent or a hook (its `WORKFLOW.md` puts `git push` in a hook). Read-only on the forge: never replies or resolves |
| Symphony (openai) | Agent: the repo's `WORKFLOW.md` prompt and skills drive PR, CI and review | Per-issue workspace, Linear polling; SPEC.md plus an Elixir reference implementation | Codex (the spec targets `codex app-server`) | Not stated | No durable database by design; restart recovery from the tracker and filesystem. Tracker state is the human lock | The reference `WORKFLOW.md` has the agent sweep PR comments, reviews and checks before `Human Review`, treating every actionable comment as blocking until addressed or pushed back. Its `land` skill watches checks, merges `origin/main` to resolve conflicts, fixes failures, and squash-merges when green | Agent, through its `push` and `land` skills |
| Gas Town | Agent (long-lived tmux agents) | Local: per-agent git worktrees, a per-rig Refinery merge queue | Claude Code by default; presets for gemini, codex, kiro, cursor, auggie, amp, opencode, copilot, pi, omp, and custom commands | Not mentioned in the README. #80: integration branches instead of stacks | Beads (git-backed issue tracking) plus git-worktree hooks | The Refinery batches merge requests, runs verification gates and merges to main with a Bors-style bisecting queue. Code review and CI handling are not described | `gt done` pushes a branch and creates an MR bead; "polecats never push directly to main". Whether MRs are GitHub PRs is not stated |
| Archon | Pipeline (YAML DAG workflows in the repo) | Local binary, Homebrew or source; `archon serve` web console; Docker / VPS covered. Worktree per run or reuse. Triggered from web UI, CLI, Telegram, Slack, Discord, GitHub webhooks | Claude, Codex, Pi | Not stated | SQLite or PostgreSQL (14 core tables); workflows are YAML under `.archon/workflows/` | `archon-deliver`: implement → draft PR → review → fix findings → validate → CI → ready for review. How a CI failure is handled is not stated. `archon-review` reviews a PR or diff through parallel specialist lenses, and a finished PR with review comments can be returned to | `archon-pr` opens the PR. Merging is not stated |
| Roast (Shopify) | Pipeline (Ruby DSL of cogs: `cmd`, `agent`, `ruby`, `chat`, `repeat`, `map`, `call`) | Run-to-completion engine that drives local agent CLIs | Pi (default) and Claude Code for the `agent` cog; the `chat` cog calls OpenAI, Anthropic, Perplexity, Gemini and Bedrock APIs | No (per #80) | Not a store: each agent result carries a session id | No watcher and no PR lifecycle; nothing under `examples/` or `tutorial/` mentions pull requests | Not stated |
| Amp PR follow-up | Agent, in its thread | Cloud (orbs), GitHub App and webhooks | Amp's own agent | Not stated | Threads and subscriptions on Amp's side | Subscribes to PR comments, reviews and CI for the latest commit; fix and merge behavior not stated | Not stated; subscriptions grant no permission to push, writes run as the user |

| Tool | Local review before the bot | Replies to / resolves threads | Takeover of a running session | Maturity per #80 |
|---|---|---|---|---|
| sheepdog | Yes: configurable `settings.review`, findings go to the branch's own agent | Bot threads only, after replying; human threads left to the user (A7) | Lock plus interactive resume of the persistent session | Single user, this repo |
| Sortie | "Self-review" phase (verify commands plus an agent review turn) before completion is accepted | No | None; parking label | 198★, created 2026-03-17 |
| Symphony (openai) | Plan self-review in the workpad, then the PR feedback sweep and green checks before `Human Review` | Yes, by the agent: per comment it accepts, clarifies or pushes back, replying inline on review comments | Not stated; the `Human Review` ticket state holds the human lock | 27.6k★; v0.0.3 2026-09-15 |
| Gas Town | Refinery verification gates before the merge; a human review step is not described | Not stated | `gt mayor attach` joins the Mayor session; `gt crew add` gives a person a personal clone; `gt seance` queries predecessor sessions. The README doesn't use the word takeover | 18.3k★ |
| Archon | `archon-deliver` reviews and fixes findings before the PR is marked ready; workflows can pause for human approval | Not stated | Runs and sessions persist and can be resumed; handing a live session to someone else is not stated | 23.7k★, MIT |
| Roast (Shopify) | Not applicable: no PR lifecycle | Not applicable | Sessions can be resumed by id and are forked by default (Claude `--fork-session`, pi `--fork`) | 1.25k★, MIT |
| Amp PR follow-up | Not stated | Not stated | Not stated | Hosted product; announced 2026-10-12 |

## Still open

- Fork per step vs. one persistent session per branch: #69.
- Language: stay in bash, or Go (super embedded) or Rust/Go native: #138.
- Whether sheepdog accepts warnings at the review cap: #142.
- More prior art (Open SWE, Nasiko, Evener): #37.
