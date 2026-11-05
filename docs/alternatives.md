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
| Built-in PR subscriptions | Same | Each is tied to one vendor and runs in its cloud (Claude Code auto-fix, Cursor subscriptions, Copilot Agent Merge; see Hosted agents below). Claude's can't see merge conflicts; Copilot's refuses stacked PRs |
| Git hooks | Same | Blind to GitHub-side events |
| PreToolUse / Stop hooks to enforce agent discipline | "Pipeline drives" plus reconciliation | Claude-only |
| Agent drives the PR loop | Pipeline drives; agents are workers | Costs tokens; stochastic where it needn't be. Symphony, Gas Town, Open SWE, Amp, the PR skills (shepherd-pr, pstack) and the hosted watch-and-fix loops (Cursor, Claude Code, Copilot) are this camp |
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
| Cycle cap reset per HEAD | Per loop run, reset on success, split by cost (local 3, CI 2, bot 2; cap hit → needs chrismo) | Every fix commits, so a per-HEAD reset would never trip. CI rounds are slow and expensive (Stripe's rule). Revisit with use: autoRun now runs steps, though #12 (the loop) is still open |
| Opting in per step (HumanLayer's one auto-advance flag per transition, all off by default) | `autoRun` runs every step; the pipeline stops where a person is needed (decided 2026-11-05) | The stops are already in the design: chrismo's steps in NEXT (merge, human threads), a step's `needs-input` outcome, and the cycle and budget caps. Per-step flags would make chrismo guess ahead of time which steps will need them |
| Per-step turn cap | Not used | Neither CLI has a turn-cap flag (`--help`, 2026-10-11); `--max-budget-usd` bounds Claude steps |
| OpenHands-style "was it incorporated?" check before replying | Not used, for now | Revisit if fix steps move to cheaper models |
| GitHub native stacks (`gh stack`) | Own restack | Too heavy for local automation; guard only (#17, built) |
| JSON state file | Git-native: branch config, `refs/sheepdog/*`, JSON config files | Refs and repo config are shared across worktrees; a ref pins the object against gc, a SHA in a file would not; the refs stay local; lineage must be local because GitHub retargets a child PR to `main` when the parent merges (#22) |
| Settings in git config | JSON config files (2026-10-13) | Git layers only the same key across scopes; the step-to-harness fallback needed its own resolver whichever store held the values. No git-config fallback, since nobody had any set |
| A branch file holding a full copy of the settings | Overrides only (2026-10-13) | A full copy froze the branch, so later global edits never reached it |
| Claude's `--bg` / `claude attach` | Headless `-p` + resume | Claude-only |
| Blocking an agent's accidental push or PR | Log, don't block; reconcile | A detected push is recorded and surfaced; a repeat offender means the step-contract prompt needs tightening. Per case: a push with no PR yet makes ship's push a no-op; a push with a PR costs one early CI run, since "ready" and the bot trigger need the `local` pin to cover HEAD; a force-push after an amend is ours if the remote SHA is in the reflog, else refused; an agent's `gh pr create` makes ship find-and-fix-up title, body and base; a push to the wrong branch (`HEAD:main`) is not ours to absorb, server-side branch protection covers it |
| Stay in bash, or Go with super embedded as a library | Rust (decided 2026-10-21, #138; ported verb by verb, done 2026-10-22, #193) | SuperSQL is a means, not a goal: the queries reshape one small JSON document each, and most of their text works around SuperDB's missing-field and empty-subquery behavior. Bash was never a chosen language, and the 2026-10-17 spike measured its costs: one `sd state` took about 6 s, 3.4 s of it in 56 super calls, and parsing dominates (PRN_QUERY 638 ms to parse, 1.3 ms to run; the config type-check parsed 7 times per run); upstream main parses only ~5% faster. Process supervision for the loop (#12) needs perl in bash. Go with embedded super would keep the queries and the black-box bats suite, but Rust is the cleaner end state once SuperSQL is a means |

Build order was also a choice: manual first (2026-10-12), every step an
`sd act` verb run by hand, each verb checking its own preconditions with
`sd state`. Reordered for dogfooding (2026-10-13): first what keeps chrismo's
own branches current and reviewed, last what chrismo already does by hand
(start, PR creation).

## Other tools we read

Read for prior art; nothing but sheepdog was run, so treat everything below
as the tools' own claims. Each entry opens with its repo, stars and license
(GitHub, 2026-11-03), then how deep it was read and when:

- **source:** a shallow clone, read in the code. Sortie, av, gh-stack,
  attractor (2026-10-11); Symphony at be10a1b, Roast at 40b87a5, Open SWE at
  c560ff7, Nasiko at 50b0a34, Evener at a4dcbc1, samcdavid/dotfiles at 2e73af1
  (2026-10-16); HumanLayer at f9bd735, humanlayer-pi at f4bd3ed,
  humanlayer-opencode at 9ff1cde (2026-11-05). The clones are in
  `~/modev/pr-workflow-prior-art/`.
- **docs:** the README or product docs only. Archon and Gas Town
  (2026-10-18), Amp (2026-10-17), shepherd-pr (2026-10-21), pstack (README,
  `plugin.json` and 3 of its 23 playbooks, 2026-11-03), the hosted PR loops
  of Cursor, Claude Code, Copilot and Graphite (2026-11-03, by a research
  agent).
- **brief:** repo metadata, search results or second-hand summaries (two
  research rounds, 2026-10-11). Good for "not a fit", not for borrowing
  without a second look.

The decision code of nine tools that turn PR state into a next action (merge
bots, plus Sortie, Symphony and agent-orchestrator) was read from source on
2026-10-30 in [`arch-review/prior-art-reconcile.md`](arch-review/prior-art-reconcile.md),
with its own 20-idea table. This page indexes those tools and doesn't repeat
that page's findings.

### Summary

- **Nothing to adopt wholesale.** Sortie is closest (same `claude -p` adapter,
  orchestrator-driven) but lacks pi, stacks, git-native state and takeover, and
  lets the agent push. Roast matches principles 1–3 and drives Claude and pi,
  but is a run-to-completion engine with no PR lifecycle. Nor is there a
  reconcile engine to reuse: each one found is tied to its own state model
  (prior-art-reconcile.md).
- **Rejected:** agent-drives designs (Symphony, Gas Town, Open SWE, Amp, the PR
  skills, the hosted watch-and-fix loops), GitHub native stacks (#17), LLM-app
  frameworks (wrong layer).
- **Still distinctive:** stack-aware restack with a post-condition, local
  review before the bot, git-native state. Nobody found does all three.

Borrowed, with where it landed:

| Idea | From | In sheepdog |
|---|---|---|
| Cancelled CI is inconclusive, not red | Sortie (ADR-0023) | Reversed by #430: cancelled counts as red (O5) |
| A handed-off set of bot comments | Sortie | Not recorded |
| `-U0` patch-id for the restack post-condition | 2026-10-11 round; source not recorded | Design table above |
| The watcher's reaction model | agent-orchestrator | `sd watch` |
| A deterministic verify before local review | Stripe Minions | `settings.verify` (decided 2026-10-11) |
| Split cycle caps (local 3, CI 2), autofix before the agent | Stripe Minions | Design table above; A11 (#65) |
| Give up after 3 fix-push cycles | spencerpauly `babysitting-pr` | The cycle cap |
| A scope judge given the task | Spotify Honk's LLM judge | `SHEEPDOG_TASK` (decided 2026-10-11) |
| Cheap per-step tool denies for push and PR create, beside reconciliation | Spotify Honk (its git tool can't push) | Design rule 4 (decided 2026-10-11) |
| Fork the session per step | Roast | #69 |
| Undraft waits for required checks | Open SWE | #123 |
| Agent steps that touch CI config hand off, not push | Open SWE | #124 |
| describe follows the repo's PR template | Open SWE | #128 |
| PID-reuse-safe liveness, quota pause, an outcome vocabulary | Evener | Comment on #12 |
| Unknown spend isn't $0, fail closed, transcript size | Nasiko | Comment on #121 |
| A zero-token check for new suppressions and skipped tests | samcdavid/dotfiles | #125 |
| An importance bar and self-check per review finding | samcdavid/dotfiles | #126 |
| Default prompt wording for the editing steps | samcdavid/dotfiles | #127 |
| A red verify matched by patch-id after a hand rebase | pstack (shipping) | #429 (open) |
| Rerun once and check for a stale base before ci-fix | pstack (babysit) | #430 (open) |
| 20 ideas for the gate engine (a two-phase gate, named predicates, Hard vs Soft by substitution, seen markers only after success, …) | The nine tools in prior-art-reconcile.md, and the Harness Playbook | That page's "What to pull in" table: #348, #356, #357 and notes on others |

### Side by side

A neutral side-by-side of what each tool documents: an empty or "not stated"
cell means the sources don't say, not that the feature is missing.

| Tool | Read | Who drives | Where it runs | Harnesses | Stacks | Who pushes / opens the PR |
|---|---|---|---|---|---|---|
| sheepdog | — | Pipeline; agents are bounded workers | Local, worktree per branch | Claude, pi | Yes: `rebase --onto` restack with a patch-id post-condition. Native GitHub stacks only guarded (#17) | Pipeline. Agents don't need to know it exists |
| Sortie | source | Orchestrator | Fresh clone per ticket; Go | Claude, Copilot, OpenCode, Codex, Kiro, Gemini, generic ACP | No | The agent or a hook; read-only on the forge |
| Archon | docs | Pipeline (YAML DAG workflows in the repo) | Local binary; `archon serve` web console; Docker / VPS | Claude, Codex, Pi | Not stated | `archon-pr` opens the PR; merging not stated |
| HumanLayer | source | Server: starts the next phase's session when its flag is on and the phase looks done (an artifact exists, or a Haiku call reads the agent's last message) | Desktop app plus cloud; a daemon per host, worktree per task; an ephemeral daemon in CI | Claude (Agent SDK), Codex (its own loop on the Responses API); Claude Code, OpenCode and Pi only mirrored | No | The agent, in its `describe-pr` phase; people merge from the app's PR panel |
| Roast (Shopify) | source | Pipeline (Ruby DSL of cogs) | Run-to-completion engine driving local agent CLIs | Pi (default), Claude Code; API-backed `chat` cog | No | Not stated |
| agent-orchestrator | source | A Go daemon polls every 30 s and pastes CI failures and review comments into the agent's running session | Desktop app plus daemon | Claude Code through installed hooks; others not recorded | Not recorded; its tests cover suppressing reactions on a stacked child | The agent commits and pushes itself |
| Symphony (openai) | source | Agent, via the repo's `WORKFLOW.md` and skills | Per-issue workspace, Linear polling | Codex | Not stated | Agent, through its `push` and `land` skills |
| Gas Town | docs | Agent (long-lived tmux agents) | Local: per-agent worktrees, a per-rig Refinery merge queue | Claude Code by default; presets for gemini, codex, kiro, cursor, auggie, amp, opencode, copilot, pi, omp, custom | Not mentioned; integration branches instead | `gt done` pushes a branch and creates an MR bead; whether MRs are GitHub PRs is not stated |
| Open SWE | source | Agent (a Deep Agents / LangGraph graph) | Persistent per-thread cloud sandboxes | Its own agent | Not stated | Not stated |
| Amp PR follow-up | docs | Agent, in its thread | Cloud (orbs), GitHub App and webhooks | Amp's own agent | Not stated | Not stated; writes run as the user |
| shepherd-pr | docs | Agent, on the user's request; installing the skill starts nothing | Local (Bash, Python 3 stdlib, authenticated `gh`; macOS or Linux; GitHub.com only) | Guides for Claude Code, Cursor, Codex, Devin CLI, Kimi Code, Gemini CLI, OpenCode, Pi, Hermes Agent and others | Not stated | The agent pushes scoped fixes to an authorized branch; the bundled tools write nothing. Asks before comments, reruns or merges |
| Cursor `/autopilot` and subscriptions | docs | Agent | Cursor cloud agents | Cursor | Bugbot Autofix pushes a stacked PR's fix to a side branch; otherwise not stated | The agent pushes; it doesn't merge |
| Claude Code auto-fix | docs | Agent, subscribed to the PR's GitHub activity | Cloud session; or the desktop app, locally | Claude | Not stated | Claude pushes and replies as the user; "Merge when ready" turns on GitHub auto-merge |
| Copilot Agent Merge | docs | Agent | GitHub Actions-based environment | Copilot | Refuses stacked PRs | Not stated; combines with auto-merge |
| pstack (Cursor) | docs | Agent, following a playbook under `/loop`; a `watch-pr` script polls | Local chat, or one Cursor cloud agent per PR | Cursor; Claude Code through a generated adapter (Cursor-only steps skipped); a model per role | Yes: babysit works the bottom PR and never restacks; shipping lands bottom-up and retargets | The agent pushes (`--force-with-lease`). Shipping merges only PRs a separate verifier agent passed |

### The agent drives the loop (opposite of design rule 2)

**Symphony** · [openai/symphony](https://github.com/openai/symphony) ·
27.9k★ · Apache-2.0 · source, 2026-10-16. Created 2026-02-26; v0.0.1
2026-07-18, v0.0.3 2026-09-15. SPEC.md plus an Elixir reference
implementation; no durable database by design, so restart recovery comes from
the tracker and filesystem, and the tracker state is the human lock. The
reference `WORKFLOW.md` has the agent sweep PR comments, reviews and checks
before `Human Review`, treating every actionable comment as blocking until
addressed or pushed back, and the agent replies inline per comment (accept,
clarify or push back). Its `land` skill watches checks, merges `origin/main`
to resolve conflicts, fixes failures and squash-merges when green. Local
review is a plan self-review in the workpad.

- **Verdict:** the agent-drives camp. Worth rereading: SPEC §7–8, reconcile
  and retry.

**Gas Town** · [gastownhall/gastown](https://github.com/gastownhall/gastown)
(plus beads and gascity) · 18.7k★ · MIT · docs, 2026-10-18. Its
`docs/design/factory-worker-api.md` lists 28+ fragile tmux and
screen-scraping touch points. State is Beads (git-backed issue tracking) plus
git-worktree hooks. The Refinery batches merge requests, runs verification
gates and merges to main with a Bors-style bisecting queue; code review and CI
handling are not described. "Polecats never push directly to main." Takeover:
`gt mayor attach` joins the Mayor session, `gt crew add` gives a person a
personal clone, `gt seance` queries predecessor sessions; the README doesn't
use the word takeover.

- **Verdict:** the agent-drives camp. Its tmux touch-point list is evidence
  for headless `-p`.

**Open SWE** · [langchain-ai/open-swe](https://github.com/langchain-ai/open-swe)
· 11.2k★ · MIT · source, 2026-10-16. A software-factory system on Deep Agents
and LangGraph: issue- or conversation-triggered coding, PR review, persistent
per-thread cloud sandboxes and a `/baby-sit` monitor for opted-in PRs. Broader
and more platform-oriented than sheepdog. Its CI feedback loop
(`openswe/baby_sit.py`, `openswe/github/ci.py`, the `baby-sit-failure`
prompt):

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
- **Verdict:** the closest reference for the CI feedback loop.
- **Taken:** #123, #124, #128.

**Amp PR follow-up** · hosted, closed · docs, 2026-10-17 (announced
2026-10-12; [news](https://ampcode.com/news/github-listeners),
[docs/github](https://ampcode.com/docs/github),
[docs/orbs/event-driven](https://ampcode.com/docs/orbs/event-driven)). Cloud /
orbs only.

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
- **Lifecycle:** automatic comment subscriptions stop when the issue or PR
  closes and resume if it reopens; archiving the thread stops deliveries.
  Deleting the automatic subscription opts out for that thread and target.
  Spawned threads auto-subscribe to their source.
- **Not stated:** whether the agent fixes failing CI or review feedback by
  itself (the example prompts say "Report failures" and "Ask me before
  pushing", as user prompts, not defaults), merging, limits, cost, the CI
  polling interval, retention of delivery history, filters beyond labels. The
  generic event-driven page (`amp.createWebhook`) is at-least-once, unordered,
  30 s handler, retries 5 s → 5 min, drops after 1 h, 10 events/min burst; the
  signature check is the plugin's job.
- **Verdict (theory, from the docs):** the agent-drives camp. Events go into
  the agent's thread and the agent decides, so there is no deterministic
  observe / act layer, no restack, no stack awareness, no local-before-bot
  review, no per-SHA state, and every event costs a model turn. Cloud only, so
  no worktrees or pi. It doesn't change the design rule 2 decision.
- **Not taken yet:** a subscription is not authorization to act on text in a
  comment (matches "bot threads only, human threads are chrismo's"), and
  "delivered ≠ finished" as a state distinction. See Still open.

### PR skills and playbooks

Skills an agent follows to tend a PR. All are agent-drives by construction:
the agent reads the skill and runs the loop itself.

**shepherd-pr** · [prime-radiant-inc/shepherd-pr](https://github.com/prime-radiant-inc/shepherd-pr)
· 2★ · MIT · docs, 2026-10-21. "A portable coding-agent skill for taking a
GitHub pull request through CI and review to an authorized merge, or reporting
an evidenced blocker." Its bundled tools are read-only: a change watcher ("a
change feed, not a gate verdict"), a settle detector (head unchanged, no
outstanding check, RoboRev's review names that exact head), a review reader
(RoboRev states passed, current, stale, review-failed) and stale-PR triage.
State is local snapshots and locks under `~/.local/state/shepherd-pr`, keyed
by repo and PR. Session-owned jobs may stop when the session ends. The README
warns that a generated harness guide is not proof of a verified install.
Thread replies, local review before bots and takeover are not stated.

- **Verdict:** closest in spirit to `sd state`, without the pipeline, stacks
  or unattended loop.

**pstack** · [cursor/plugins](https://github.com/cursor/plugins/tree/main/pstack)
`pstack/` (Lauren Tan / poteto, v0.15.36) · 15.2k★ for the monorepo · MIT ·
docs, 2026-11-03. A Cursor plugin of skills for engineering method as a
whole: about 55 skills, 23 `poteto-mode` playbooks and 24 one-rule
principles. The PR part is the `babysit`, `shipping` and `autopilot-full` /
`-stack` playbooks; the rest (bug fix, perf, feature, writing) has no
counterpart in sheepdog. Read: the README, `plugin.json`, and the babysit,
shipping and autopilot-full playbooks.

- **Babysit:** one babysitter per stack, working only the lowest unmerged PR.
  It goes conflicts, then threads, then CI, batching fixes into one push wave.
  It never restacks, retargets or force-pushes: it reports "needs a rebase" to
  the PR's owner. A bundled `watch-pr` script polls GitHub and returns a
  verdict (`READY`, `WAITING`, `ADVANCE`, `COMPLETE`); the agent acts on each
  one.
- **CI:** classified before any retry. A flake gets one fresh build, and an
  identical second failure means it was never a flake. A failure in code the
  diff never touches means a stale base (`git merge-base --is-ancestor`), and
  gets reported, not retried. Only a failure in the diff's own code gets a
  commit.
- **Bot findings:** each claim is checked against the code; real ones are
  fixed red-first in the lowest PR that owns the code. From the third bot pass
  it leans toward dismissing, except anything touching security, auth,
  billing, data or migrations, which is escalated.
- **Shipping:** a verifier agent that didn't write the code runs the change as
  a user would and posts `PASS` / `PASS+NOTES` / `FAIL`. Green CI and a bot
  approval don't count as verdicts. Only the contiguous run of verified PRs
  from the bottom lands. A verdict is keyed on `git patch-id`, so it survives
  a rebase that doesn't change the patch.
- **Autopilot-full:** one cloud-agent owner per PR, each off trunk (never
  stacked); a root agent audits hourly under `/loop` and replaces agents with
  no side effects past a stuck-after time.
- **Verdict (theory, from the docs):** the agent-drives camp. A deterministic
  watcher waits, but every verdict costs a model turn, and the agent runs `gh`
  itself, replies included. Much of it independently matches sheepdog's rules:
  idempotency, conflicts before threads before CI, bot findings checked
  against the code. Its one-babysitter rule is enforcement where sheepdog
  reconciles (rule 4). Review pins here already compare patch-ids
  (`rewritten-same`).
- **Taken:** #429, #430. Not yet: the independent verifier (Still open).

**Babysitter skills** (mblode `pr-babysitter`, dlc `babysit`, spencerpauly
`babysitting-pr`) · brief, 2026-10-11. Ideas only; none is stack-aware.

- **Taken:** spencerpauly's "give up after 3 fix-push cycles" became the cycle
  cap.

**samcdavid/dotfiles `claude/skills`** ·
[samcdavid/dotfiles](https://github.com/samcdavid/dotfiles) · 5★ · no
license · source, 2026-10-16. Short skills. On 2026-10-14 the author deleted
their own "multi-stage pipeline, runner agents, verifier tiers" in favor of
them; the shallow clone doesn't show why.

- **Taken:** #125 (a zero-token diff check for new suppressions, test skips
  and edited existing tests), #126 (local review: an importance bar and a
  self-check before each finding), #127 (default prompt wording for the
  editing steps).

### Pipeline and workflow engines

**Sortie** · [sortie-ai/sortie](https://github.com/sortie-ai/sortie) · 216★ ·
Apache-2.0 · source, 2026-10-11. Created 2026-03-17, commits daily, one
person or a small team. Its reaction contracts (architecture docs 12–16 and
19, the ADRs) map onto O5–O7 / J2–J4: documented contracts for CI, review,
auto-merge, bot review and merge conflict, with cancelled CI as inconclusive.
A "self-review" phase (verify commands plus an agent review turn) runs before
completion is accepted. It never replies to or resolves threads, and has no
takeover, only a parking label. Symphony-derived but it pulled control back to
the orchestrator (ADR-0007); its Claude adapter is `claude -p --output-format
stream-json --session-id` then `--resume`, the same as ours. ADRs: cancelled
CI is inconclusive (0023), the retry budget resets per HEAD (0024),
self-review before completion (0021), a bot-comment fingerprint and handed-off
set. The agent or a hook pushes and opens the PR, then writes
`.sortie/scm.json` (SHA, PR number) for the orchestrator to read. Trackers:
GitHub, GitLab, Gitea, Linear, Jira, plus a read-only JSON-file tracker for
local tasks. Possibly heavy, like gh-stack: 29 architecture docs and 34 ADRs.

- **Verdict:** a design reference, not a base. Adopting means giving up pi,
  giving up (or adding) stacks, accepting SQLite and letting the agent push
  (its `WORKFLOW.md` puts `git push` in a hook).
- **Taken:** the handed-off comment set; cancelled CI as inconclusive,
  since reversed (#430: cancelled counts as red and goes to ci-fix).

**Archon** · [coleam00/Archon](https://github.com/coleam00/Archon) · 23.8k★ ·
MIT · docs, 2026-10-18. An earlier read called it Claude-centric; the README
lists Claude, Codex and Pi. Its nodes are `bash:` and `prompt:`, with
`loop: until:`, human gates and `fresh_context`. Triggered from the web UI,
CLI, Telegram, Slack, Discord or GitHub webhooks; worktree per run or reuse;
SQLite or PostgreSQL; workflows are YAML under `.archon/workflows/`.
`archon-deliver`: implement → draft PR → review → fix findings → validate →
CI → ready for review (how a CI failure is handled is not stated);
`archon-review` reviews through parallel specialist lenses; workflows can
pause for human approval; runs persist and resume.

- **Verdict:** closest to design rules 1–2 in philosophy, but it has a web UI
  and no stack support stated.

**HumanLayer** · [humanlayer/humanlayer](https://github.com/humanlayer/humanlayer)
· 12.8k★ (2026-11-05) · Apache-2.0 · source at f9bd735, 2026-11-05. Created
2024-08-05. A team product: a macOS desktop app and HumanLayer Cloud (Postgres
synced to clients through Electric), with a Bun daemon on each host that runs
the sessions. It's a Turborepo of about 5,900 files. A task groups sessions
and synced markdown artifacts that teammates comment on in the cloud UI. The
workflows are phased skill chains, each phase a fresh session that writes a
numbered artifact under `.humanlayer/tasks/<slug>/`. RPI goes research
questions → research → design discussion → structure outline → implement →
`describe-pr`; PRD-Oriented, Oneshot and Freeform are the other workflows.

- **Who drives:** the server. When a session ends,
  `apps/riptide-api/src/orpc/daemon/sessions/check-auto-advance.ts` starts
  the next phase's session if four conditions hold: the task's flag for that
  transition is on (all default off), the session carries the phase's label,
  the phase is done, and no session for the next phase exists yet. For the
  research phases, "done" means an artifact with the right frontmatter
  `type` exists. For later phases, a Haiku call
  (`packages/ai/src/agents/extract-next-step/`) reads the agent's last
  message and pulls out the next-step prompt the agent suggested. Steps that
  would skip a human review (design → outline, outline → implement) never
  auto-advance.
- **Harnesses:** Claude runs through the Claude Agent SDK. Codex runs on
  HumanLayer's own agent loop (`@humanlayer/fold-*` and `@humanlayer/codelayer`,
  from npm; their source isn't in this repo) against the Responses API, not
  the Codex CLI. Claude Code, OpenCode and Pi get plugins that mirror a
  session the user started into the cloud and accept web messages and stop.
  For pi, the README says "the web app can't start a pi session".
- **[humanlayer-pi](https://github.com/humanlayer/humanlayer-pi) and
  [humanlayer-opencode](https://github.com/humanlayer/humanlayer-opencode)**
  (39★ and 14★, no license file, last commit 2026-10-05; source read the
  same day): the installable builds of `apps/riptide-pi-extension` and
  `apps/riptide-opencode-plugin`. They hold a bundled JS file and a copy of
  the RPI skills, and a publish script overwrites each repo. Both alpha.
  They work only inside a session the user started: they send its events,
  task files and diff to the cloud, take web messages, stop and (OpenCode)
  tool approvals, and add comment tools. "The web app cannot start an
  opencode session" either. Their `describe-pr` skill has the agent push
  and run `gh pr create` / `gh pr edit` itself.
- **PRs:** the workflow ends when the PR opens. `describe-pr` is an agent
  phase: it commits, pushes if the branch has no upstream, opens or updates
  the PR and writes the body. Then "Continue with normal pull request
  review." A GitHub App's webhooks (`check_run`, `pull_request`, reviews,
  review comments, review threads) refresh a PR panel showing checks (both
  `CheckRun` and `StatusContext`), the unresolved-thread count, the review
  decision and the merge state. The panel's writes (merge, update branch,
  auto-merge, draft or ready, close) run as the App. Merge and update branch
  send an `expectedHeadSha`, so GitHub refuses them if the branch moved.
  No webhook starts an agent: there's no CI fix, thread reply, restack or
  stack support.
- **Attribution:** a `HumanLayer-Session` commit trailer ties a commit to its
  session. A merged PR counts as a session's output only in the `committed`
  tier, not `observed` (the branch was only seen checked out).
- **Automations:** `humanlayer automation run` runs one session in CI with a
  daemon that lives only for the job. The docs' recurring-agent patterns are
  for the user to build in Actions, not shipped: skip the run while an agent
  PR is open, and a `/iterate` PR comment reruns the agent on the branch.
- **Verdict:** a team workspace for the work before the PR. It's sheepdog's
  J0 and J6 grown into a planning chain with a human review between phases,
  and it has nothing after the PR opens. The server starts each phase (rule
  2), but the agent pushes and opens the PR. Its "done" signal for later
  phases is a model reading the agent's prose, which the outcome row in the
  design table rejects. Pi is only mirrored, so "nothing above runs pi"
  (Keep building or switch) still holds.
- **Not taken:** one auto-advance flag per transition, all off by default,
  with no flag for a step that would skip a human review. sheepdog runs
  every step and stops where a person is needed instead (design table
  above).

**Roast** · [Shopify/roast](https://github.com/Shopify/roast) · 1.3k★ · MIT ·
source, 2026-10-16. A run-to-completion engine with no watcher, no PR
lifecycle (nothing under `examples/` or `tutorial/` mentions pull requests)
and no stacks. Each agent result carries a session id, and sessions are
forked by default (Claude `--fork-session`, pi `--fork`).

- **Verdict:** matches design rules 1–3 and drives Claude and pi; a reference
  for step and session mechanics.
- **Taken:** session forking (#69).

**agent-orchestrator (AO)** ·
[OrchestratorInc/agent-orchestrator](https://github.com/OrchestratorInc/agent-orchestrator)
· 14.1k★ · Apache-2.0 · brief, 2026-10-11; the daemon's source, 2026-10-30,
in prior-art-reconcile.md. The only tool found with an automatic CI / review
→ agent loop, but a desktop app plus daemon, telemetry on by default, repo
moved 3× (ComposioHQ → AgentWrapper → OrchestratorInc), 827 open issues at
the time, and it owns agent launch. It pastes each reaction into the agent's
already-running TUI and tells finished from asking through harness hooks.

- **Verdict:** not adopted (design table above). The paste model is the
  opposite of design rule 2 and its main source of guard complexity (nine
  suppression outcomes).
- **Taken:** its reaction model inspired the watcher; ideas 16–18 in
  prior-art-reconcile.md (seen markers advance only after success, a CI dedup
  signature with the log tail, report every applicable condition).

**attractor** · [strongdm/attractor](https://github.com/strongdm/attractor) ·
1.3k★ · Apache-2.0 · source, 2026-10-11. A DOT-graph pipeline; specs only, no
code. JSON checkpoint per node, goal gates, per-edge context fidelity (default
`compact` = fresh session). Its philosophy is "no human code review".

**continuous-claude** ·
[AnandChowdhary/continuous-claude](https://github.com/AnandChowdhary/continuous-claude)
· 1.4k★ · MIT · brief. A bash Ralph loop, one PR per iteration;
`gh pr checks` → merge or discard.

**Ralph** · [snarktank/ralph](https://github.com/snarktank/ralph) · 22.1k★ ·
MIT · brief (ghuntley's version, 1.8k★ on 2026-10-11). Bash: a fresh process
every loop; state in files and git.

**Spec Kit, BMAD, Kiro** · [github/spec-kit](https://github.com/github/spec-kit)
146.6k★, [bmad-code-org/BMAD-METHOD](https://github.com/bmad-code-org/BMAD-METHOD)
55.0k★, Kiro closed · brief. Upstream of sheepdog (spec → tasks) with no CI or
review loop; could feed `sd act start`.

**LangGraph, CrewAI, Dify** · brief. Wrong layer: they build agents from raw
model API calls, so they'd replace Claude / pi rather than drive them (breaks
design rule 3) and add a Python / server runtime. CrewAI's role-play is the
opposite of design rule 1.

### Merge bots and reconcile engines

Kodiak ([chdsbd/kodiak](https://github.com/chdsbd/kodiak)), bors-ng
([bors-ng/bors-ng](https://github.com/bors-ng/bors-ng)), Renovate
([renovatebot/renovate](https://github.com/renovatebot/renovate)),
rust-lang/bors ([rust-lang/bors](https://github.com/rust-lang/bors)), Prow's
Tide ([kubernetes-sigs/prow](https://github.com/kubernetes-sigs/prow)
`pkg/tide`) and Mergify's engine (Mergifyio/mergify-engine at tag 5.0.0) ·
source, 2026-10-30, all in
[prior-art-reconcile.md](arch-review/prior-art-reconcile.md). Not agent
tools: each turns observed PR state into one next action, the job of
`sd state`'s NEXT and the act verbs' gates.

- **Verdict:** no engine to reuse; every one is a long priority-ordered
  evaluator tied to its own state model. Tide and Mergify carry comments
  admitting their two copies of the guards disagree, which is what #348
  (one gate function per verb) avoids.
- **Taken:** most of that page's 20 ideas (Borrowed above).

**The Harness Playbook** (stencil.so, Can Bölük, 2026-09-02) · docs,
2026-10-30, in prior-art-reconcile.md. The design of one agent harness, not a
PR pipeline. "The sandbox should execute, not decide" is design rule 2.

- **Taken:** ideas 19 (unknown is not false) and 20 (parity over generated
  states).

### Hosted agents and built-in PR features

Closed or cloud products. The 2026-11-03 entries were read from docs,
changelogs and forum staff replies by a research agent; quotes are not
spot-checked, and most docs pages carry no date.

By 2026-11-03 Cursor, Claude Code and Copilot each ship an unattended "watch
the PR and fix it" loop. All three are agent-driven, cloud-hosted and tied to
one vendor's harness, and each has a gap sheepdog covers: Copilot refuses
stacked PRs, Claude can't see merge conflicts, and Cursor's CI fix covers
only PRs its agent created.

**Cursor `/autopilot`, PR subscriptions and Bugbot Autofix** · docs,
2026-11-03.

- **`/autopilot`** (renamed from `/babysit`, per a support reply 2026-08-04;
  [forum](https://forum.cursor.com/t/babysit-disappeared/167381)): hands the
  PR to a cloud agent that gets it "mergeable, CI green, and comments triaged,
  then report and stop" (staff, 2026-08-27,
  [forum](https://forum.cursor.com/t/autopilot-command-stops-when-pr-becomes-mergeable/169586)).
  It doesn't merge.
- **Subscriptions** ([changelog 2026-08-19](https://cursor.com/changelog/08-19-26),
  [capabilities](https://cursor.com/docs/cloud-agent/capabilities)): cloud
  agents subscribe to PRs they create and fix CI and bot comments. Only PRs
  the agent created, GitHub Actions only; stops after a human push or 10
  CI-failure follow-ups; a subscription lasts at most 180 days; Teams and
  Enterprise only; `@cursor autofix off` turns it off.
- **Bugbot Autofix** ([docs](https://cursor.com/docs/bugbot)): a finding
  spawns a cloud agent. Off, New Branch (recommended) or Commit to Existing
  Branch, 3 attempts per PR. On a stacked PR (base not the default branch) it
  pushes to a new branch, applied with `@cursor push <sha>`. The 2026-10-11
  note (one comment at a time; reply instead of guessing on design questions;
  resolve only if fully addressed, the same rules as J3 / A7) isn't in the
  current docs.
- **Automations** ([3.8, 2026-06-18](https://cursor.com/changelog/06-18-26)):
  GitHub triggers (a workflow run finished, inline review comments, a thread
  resolved) and templates for auto-fixing review comments.

**Claude Code auto-fix** · docs, 2026-11-03.

- **Cloud** ([claude-code-on-the-web](https://code.claude.com/docs/en/claude-code-on-the-web)):
  started with `/autofix-pr`, the ⋮ menu or a prompt; needs the Claude GitHub
  App. Claude subscribes to the PR's activity; on a CI failure or review
  comment it "investigates and pushes a fix if one is clear", asks about
  ambiguous or architectural comments, and replies under the user's account,
  labeled as Claude Code. It can't see merge conflicts (GitHub sends no
  webhook for them). Runs until turned off; caps not stated.
- **Desktop app** ([docs](https://code.claude.com/docs/en/desktop)): Auto-fix
  for failing checks, and in a local session also review comments from
  owners, members, collaborators or GitHub Apps; "Merge when ready" turns on
  GitHub auto-merge. Needs `gh`.
- **claude-code-action** · [anthropics/claude-code-action](https://github.com/anthropics/claude-code-action)
  · 9.8k★ · MIT. A GitHub Actions run started by an `@claude` mention or a
  workflow event, capped by `--max-turns`; it doesn't watch PRs, and bot
  actors are blocked unless allowlisted. Its
  `examples/ci-failure-auto-fix.yml` (brief, 2026-10-11): a script gathers
  failed logs and the agent fixes on a separate branch, a name prefix as the
  loop guard.
- **Claude Code Projects beta** · brief, 2026-10-11. Cloud-only at launch per
  reports; not usable for local worktrees and stacks.

**GitHub Copilot Agent Merge** · docs, 2026-11-03
([changelog 2026-10-28](https://github.blog/changelog/2026-10-28-agent-merge-watches-and-fixes-pull-requests-in-the-cloud/),
[docs](https://docs.github.com/en/copilot/how-tos/copilot-on-github/use-copilot-agents/watch-and-fix-pull-requests)).
"Watch and fix" in the merge box: Copilot fixes failing Actions workflows and
merge conflicts, can be on by default for PRs you create, and combines with
auto-merge. Runs in an Actions-based environment. "Does not support stacked
pull requests or pull requests opened from a fork." Review comments are
handled only by the Copilot app, "not supported in the cloud"
([docs](https://docs.github.com/en/copilot/how-tos/github-copilot-app/managing-issues-and-pull-requests)).
Earlier: one-click fixes for failing Actions
([2026-05-18](https://github.blog/changelog/2026-05-18-one-click-fixes-for-failing-actions-with-copilot-cloud-agent/)).

**Graphite Agents** · docs, 2026-11-03 ([agents](https://graphite.com/docs/agents),
[chat](https://graphite.com/docs/graphite-chat)). Run on Cursor Cloud Agents
(paid Cursor plan), started by hand, commit to the PR branch. Graphite Chat
fixes failing checks on request. No unattended loop found.

**Other ticket → agent → PR products** (Codex cloud, Devin, Jules) · brief,
2026-10-11. Closed or cloud; user-triggered; none is local + worktree + pi +
stack-aware. symphony-ts, an unofficial Apache-2.0 TS port of Symphony, was
not checked deeply.

- **Verdict (theory):** nothing here changes design rule 2; their limits are
  the useful part. Cursor stops after 10 CI-fix rounds and 3 Bugbot attempts
  per PR (compare sheepdog's caps), stops on a human push, and Claude asks
  instead of guessing on design questions (J3).

### Harnesses, runtimes and platforms

**OpenHands** · [OpenHands/OpenHands](https://github.com/OpenHands/OpenHands)
· 92.6k★ · MIT · brief. A harness (a peer of Claude / pi), not an
orchestrator. Its resolver runs a post-step LLM check per thread
("incorporated?") and generates a reply.

- **Verdict:** only the verify-before-reply idea applies, parked until fix
  steps move to cheaper models (design table above).

**Evener** · [prime-radiant-inc/evener](https://github.com/prime-radiant-inc/evener)
· 190★ · MIT · source, 2026-10-16. A local coding-agent runtime with a hub /
UI, sessions, delegates, managed worktrees and durable background jobs; not a
PR lifecycle manager. Its worktree API is far more than sheepdog needs, but
its safety rules are a checklist (`docs/developing-evener/worktrees.md`,
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
- **Taken:** a comment on #12 (the loop): PID-reuse-safe liveness, quota
  limits pause the loop, an outcome vocabulary.

**Nasiko** · [Nasiko-Labs/nasiko](https://github.com/Nasiko-Labs/nasiko) ·
11.7k★ · license not detected by GitHub · source, 2026-10-16. An adjacent
platform: an agent runtime and control plane that discovers and routes
existing coding harnesses, accounts for model usage (much deeper than first
read), and deploys A2A agents. Its coding-agent example puts filesystem and
shell behind a `Sandbox` trait with a local implementation; the remote phase
is marked not started, so it isn't shipped. No end-to-end PR / CI shepherd.

- **Verdict:** a modest reference if execution backends ever need to vary.
- **Taken:** a comment on #121 (cost guards): unknown spend isn't $0, fail
  closed, transcript context size.

**Terminal runtimes, dashboards and desktop apps** · brief, 2026-10-11, none
with a PR loop: herdr (47.9k★, terminal runtime), vibe-kanban (28.6k★, maker
shut down), superset (15.1k★ on 2026-10-11, PR feedback is manual), omnigent
(11.2k★, young meta-harness), claude-squad (8.7k★), emdash (6.0k★ on
2026-10-11, shows PR / CI, the user acts). Sculptor, container-use,
Nimbalyst, Conductor and Terragon are desktop apps, isolation, or shut down;
not pipelines.

### Reported in-house platforms

Stripe Minions, Ramp Inspect, Spotify Honk, Uber, Harvey · brief, from blog
and talk summaries read 2026-10-11; unverified, so spot-check quotes before
citing. Mostly cloud sandboxes behind Slack at company scale; the overlap is
step design and readiness checks.

- Converged shape (Wallfacer): a sandbox per task, chat or ticket intake,
  closed feedback loops, PR out, human merges.
- **Stripe "Blueprints":** code-defined workflows mixing deterministic nodes
  (lint, push) with agent nodes (implement, fix CI). Lint loops locally before
  push. CI is capped at 2 rounds, with autofixes applied before the agent
  node, then back to the human, since full CI is too expensive to iterate on.
- **Spotify Honk:** the agent only replaces the transformation script; agent
  tools are restricted (the git tool can't push, a bash allowlist); the old
  loop was capped at 10 turns per session × 3 retries. Verifiers are chosen by
  repo contents (pom.xml → Maven), callable by the agent as MCP and run by the
  harness through a Claude stop hook before any PR; failure means no PR. An
  LLM judge gets the diff and the original prompt and vetoes about a quarter
  of sessions, top reason scope creep. CI after PR-open is future work.
- Ramp opens PRs with the requester's token (no self-approval). Uber's uReview
  is deterministic generate → filter → validate → dedupe around the LLM.
- None of the sources describe an agent addressing PR comments, so J3 / J4 has
  no outside evidence either way.
- **Taken:** the verify gate, split CI cap and autofix-first (Stripe); the
  scope judge and per-step tool denies (Honk). Decided 2026-10-11; a per-step
  turn cap was rejected (design table above).

### Stack tools and rebase strategies

**gh-stack** · [github/gh-stack](https://github.com/github/gh-stack) · 1.7k★ ·
MIT · source, 2026-10-11. Native stacked PRs went GA 2026-10-06. Server-side
restack (approvals survive, base not editable). Squash is handled with the
same `rebase --onto` as A2, and it moved its JSON store from per-worktree to
the git common dir (`internal/stack/migration.go`), which supports our shared
refs.

- **Verdict:** too heavy for local automation; guard only (#17).

**av** · [aviator-co/av](https://github.com/aviator-co/av) · 511★ · MIT ·
source, 2026-10-11. The same A2 algorithm: parent, branching-point SHA and
merge commit in `.git/av` JSON, falling back to merge-base. Skips the rebase
when already on the new parent, and `PrepareWorktrees` handles branches
checked out elsewhere. No post-condition. Read `internal/sequencer/`.

**git-spice, git-town** · [abhinav/git-spice](https://github.com/abhinav/git-spice)
778★ GPL-3.0, [git-town/git-town](https://github.com/git-town/git-town) 3.4k★
MIT · brief. git-spice stores the base SHA, the same approach as our refs;
git-town stores the parent in git config, the same as our lineage. `bin/gt`
here collides with Graphite's `gt` CLI.

**ghstack, spacedentist/spr** · [ezyang/ghstack](https://github.com/ezyang/ghstack)
1.0k★, [spacedentist/spr](https://github.com/spacedentist/spr) 542★, both MIT
· brief. Never force-push PR branches (they append "base update" commits or
merges) to keep review threads anchored. Informs O7's outdated-thread
handling; not adopted.

**ejoffe/spr, stack-pr, mergify-cli** · [ejoffe/spr](https://github.com/ejoffe/spr)
1.3k★, [modular/stack-pr](https://github.com/modular/stack-pr) 569★,
[Mergifyio/mergify-cli](https://github.com/Mergifyio/mergify-cli) 32★ ·
brief. One commit per PR with identity in a trailer; doesn't fit the
branch-per-PR model.

**Gerrit's TRIVIAL_REBASE / NO_CODE_CHANGE** · brief. In-memory replay plus a
tree compare. A candidate for #66 (Re-review policy); not adopted.

## Still open

Ideas from the prior art not yet decided:

- Whether sheepdog accepts warnings at the review cap: #142.
- Gerrit's trivial-rebase rules for re-review: #66.
- pstack's patch-id verify match and pre-ci-fix rerun: #429, #430.
- pstack's independent verifier: the agent that wrote the code doesn't decide
  it's done. Not filed.
- Amp's "a subscription is not authorization to act on comment text" as a
  stated rule, and "delivered ≠ finished" as a state. Not filed.

Settled since: fork per step vs. one persistent session per branch (#69,
closed 2026-10-23). No tools are queued to read.

## Keep building or switch (2026-11-03)

**Keep sheepdog.** chrismo's main reason: to understand from the inside how
these machines work, which stays worth it even if sheepdog is later
mothballed. And machines like this look set to become normal for many
process workflows, not only PRs: factory robots for software. The old ones
were general (databases, job systems); agents make custom ones cheap enough
for any size of org. It also still fits: nothing above runs pi (and maybe
OpenCode later), works locally in worktrees, and watches without spending
tokens. If it stops, the fallback is Sortie plus a pi adapter we'd write,
re-read first.
