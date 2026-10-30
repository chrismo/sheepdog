# As built: open-issue distribution

Facts for the 2026-10-29 architecture review. Source: `gh issue list --state
open --limit 200 --json number,title,body,labels` run in
`/Users/chrismo/modev/sheepdog` on 2026-10-29: **85 open issues**. Labels in
use: enhancement 20, bug 13, low priority 9, question 6; 43 issues carry no
label. Each issue's title and first paragraph were read (longer where the
first paragraph was only a heading). Tags are Claude's reading of the text;
module tags are the `src/` path the issue names, or the one its symptom's
code lives in (then marked *inf*). Step labels are from `as-intended.md` §3.

## 1. Every open issue, tagged, sorted by module

| # | Module | Step | Type | Title (short) |
|---|---|---|---|---|
| 326 | src/next.rs | - | bug | NEXT offers a verb its own gate refuses (address at `src/act/address.rs:105`, answer) |
| 325 | src/next.rs (+ verify_red.rs) | A10 | bug | A failed local verify is invisible to NEXT and never reaches the agent (part 1 landed in #332; `src/next.rs:242`) |
| 304 | src/next.rs | O8 | bug | A running mechanical step shows as `agent:` in NEXT's wait |
| 301 | src/next.rs | J0 | bug | An empty starting commit skips implement; NEXT offers review |
| 300 | src/next.rs | A2/A3 | bug | Restack treadmill: restack outranks push on a busy main |
| 265 | src/next.rs | N2 | bug | Info bot item notified beside a red CI that ci-fix will supersede |
| 248 | src/next.rs (+ act/restack.rs) | A4 | bug | Stack follow-ups: `gh pr ready` ordered before retarget; merged parent's remote branch never deleted |
| 188 | src/next.rs (+ act/answer.rs) | - | bug | `dirty` not held back while attached; `answer` nits (AGENTS.md rule 1 omits `answer`) |
| 183 | src/next.rs (+ act/review.rs) | - | ux | Soak 5 small gaps: repeated output, routine warnings, cmd-less waits, info items pile up |
| 176 | src/act/{ci_fix,address,resolve}.rs *inf* | - | bug | A stale needs-input question outlives the step that passed it (`refs/sheepdog/<b>/question`) |
| 166 | src/next.rs (+ act/ship.rs) | - | bug | Nothing-ahead follow-ups: ship still listed after a reset-away step; own-base check |
| 165 | src/next.rs (+ act/bookkeeping.rs) | - | bug | forget --remove: a lasting refusal still loops NEXT; ignored files can go unlisted |
| 158 | src/next.rs | O3 | design-gap | Ready to merge while behind the base: merged combination never ran in CI (question) |
| 155 | src/next.rs | O5/A8 | design-gap | NEXT reviews a human's commit while CI is pending on it (question) |
| 146 | src/next.rs *inf* | A10 | design-gap | Diff-check flags: hold or notify under an unattended loop (question) |
| 66 | src/next.rs (+ act/review.rs) | J5 | design-gap | Re-review policy: J5, the accepted pin (nothing writes it), PR labels |
| 334 | src/state.rs (`:491` flags) | A10 | bug | A renamed test file's existing skips and suppressions flagged as new |
| 266 | src/state.rs | O4 | design-gap | Shared authoring: steps follow observed facts, not a mode (replaces #255's `sd-mode`) |
| 264 | src/state.rs | O1 | bug | gh-wait has no time limit |
| 250 | src/state.rs (`pick_pr` `:340`) | O1 | feature | Reviewer mode: tend someone else's PR without authoring commits |
| 180 | src/state.rs | O7 | bug | chrismo's reply on a bot thread counts as answered |
| 147 | src/state.rs (`:491`; pattern in config.rs:119) | A10 | bug | Flag skip pattern misses guarded bats skips |
| 75 | src/state.rs | O5 | ux | CI as a glyph on the one-liner (tabled 2026-10-13) |
| 2 | src/state.rs | O1 | bug | A clone of your own fork may not find the PR (untested, bash era) |
| 308 | src/watch/pass.rs | O8 | design-gap | Reconcile's TERM/KILL past a lease deadline has never run live |
| 307 | src/watch/pass.rs | - | ux | The dispatch gate is invisible: no pass line names it |
| 306 | src/watch/pass.rs (+ act/step.rs) | O8 | bug | Crashed step's orphan output never reaches steps.log; `.outcome` outlives a forgotten branch |
| 305 | src/watch/pass.rs | O1 | feature | GraphQL points per pass not recorded; rate_limit alternates between two quota windows |
| 302 | src/watch/pass.rs | N2 | feature | Under autoRun, don't notify a step the pass is about to launch |
| 299 | src/watch/pass.rs | - | feature | One step per branch per pass: a 2-second push waits a whole pass |
| 298 | src/watch/pass.rs | - | design-gap | Agent-step queue isn't fair: late branches wait 8-13 passes |
| 288 | src/watch/pass.rs (`notify` `:328`) | N2 | feature | Notifications that open the branch's Ghostty tab |
| 271 | src/watch/pass.rs | - | research | The loop, slice 5: soak 7 unattended in the sandbox |
| 270 | src/watch/pass.rs (+ running.rs) | - | feature | The loop, slice 4: interlocks for merge / takeover / mechanical step while a step runs |
| 269 | src/watch/pass.rs | - | feature | The loop, slice 3: pass runs agent steps, concurrency cap, quota pause |
| 52 | src/watch/pass.rs | N2 | bug | Audit notifications for actionable NEXT states (missed conflict notify, bash era) |
| 12 | src/watch/pass.rs | - | feature | The loop: watcher dispatches verbs (tracking issue) |
| 322 | src/watch.rs | N2 | feature | Replace Ghostty tab coupling with hooks |
| 317 | src/watch.rs | N2 | ux | `sd watch status` should show who we're waiting on and the PR link |
| 297 | src/watch.rs | - | feature | Per-repo environment for watcher-launched steps (placeholder) |
| 3 | src/watch.rs | N2 | bug | Glyph only shows when the tab title is redrawn (bash era) |
| 245 | mixed: watch.rs, act/mod.rs, next.rs, act/ci_fix.rs | - | bug | Soak 6 small gaps: refresh mid-resolve, foreign-commit wording, `sd --version`, repeated lines |
| 116 | src/act/bookkeeping.rs (+ watch.rs) | - | ux | Silent unenroll followed by cached watch status looks like failure |
| 244 | src/act/bookkeeping.rs | - | bug | forget: no path for an abandoned branch with no PR; branch config file outlives it |
| 323 | src/harness.rs (`PI_TOOLS` `:30`) | J0 | feature | Enable codemode by default for pi agent steps |
| 289 | src/harness.rs *inf* | J0 | feature | Let implementation steps request approval for a specific command |
| 243 | src/harness.rs | - | bug | pi tally: errored calls counted as turns, context 0, null cost |
| 135 | src/harness.rs | - | research | Spike: compaction in `claude -p` / `pi -p` past the fork ceiling |
| 23 | src/harness.rs | - | research | `pi -p --session` keeps context across processes (unverified) |
| 276 | src/usage.rs | - | bug | A killed claude step is charged its real spend (today `cost_usd:null` or the full cap) |
| 150 | src/usage.rs | - | bug | usage.sup records a resumed step's cost as the running total; branchBudget overcounts |
| 290 | src/config.rs | - | feature | Model presets: named harness/model/effort |
| 283 | src/config.rs | - | design-gap | Config architecture: everything in JSON, one schema (in flux per as-intended §8) |
| 251 | src/config.rs | - | feature | Config tool: see/edit every setting with its layer, maybe a TUI |
| 190 | src/config.rs | - | feature | Risk level per branch tuning stops and notifications |
| 292 | src/act/step.rs | - | ux | Announce a step's model and effort when it starts |
| 303 | src/act/step.rs (+ proc.rs) *inf* | O8 | bug | stepTimeout reads `failed (exit 1)`; half-done edits offered as chrismo's |
| 133 | src/act/step.rs | J0/J2/J3 | design-gap | One editing step for implement / review-fix / address / ci-fix |
| 281 | src/act/start.rs *inf* (sd-task) | A0 | design-gap | A branch's task comes from its issue, not git config (questions unsettled) |
| 118 | src/act/start.rs *inf* (sd-task) | A0 | feature | A command to view and update a branch's task |
| 263 | src/act/implement.rs | J0 | feature | No way to hand chrismo's follow-up to the agent after implement |
| 280 | src/review_local.rs | A8 | feature | Review prompt: commit messages in full, not subjects alone |
| 279 | src/review_local.rs | A8 | feature | Local review in several passes from different angles (idea) |
| 240 | src/review_local.rs | A8 | design-gap | Review sessions don't see decisions made after the task |
| 145 | src/review_local.rs (+ act/review.rs) | A8 | bug | Review step says "changes since last review" but reviews the whole branch |
| 126 | src/review_local.rs (lib/prompts/review.md) | A8 | feature | Importance bar and self-check before each finding |
| 151 | src/act/review.rs (+ resolve.rs) | J1/A8 | bug | Restack resolves use up reviewRounds; at the cap NEXT offers --accept on an error |
| 142 | src/act/review.rs | A8 | design-gap | `settings.reviewAccept`: accept warnings at the review cap (idea, undecided) |
| 157 | src/act/describe.rs | J6 | feature | PR description written once at ship and goes stale |
| 149 | src/act/describe.rs | J6 | feature | Item ID per branch: describe ends the body with `Closes <id>` |
| 128 | src/act/describe.rs (lib/prompts/describe.md) | J6 | feature | describe follows the repo's PR template |
| 21 | src/act/describe.rs (+ harness.rs) | J6 | research | `--json-schema` for describe, and a pi equivalent |
| 189 | src/act/ship.rs | J6 | feature | PR structure checks, e.g. `[ISSUE-ID]` suffix on the subject |
| 148 | src/act/ship.rs | - | bug | branchBudget: ship-parent spends on a parent past its budget |
| 156 | src/act/resolve.rs | J1 | bug | restack --resolve drops the agent's summary |
| 152 | src/act/ci_fix.rs | J2 | bug | ci-fix's question about reverting a human's commit goes nowhere |
| 94 | src/act/restack.rs (+ state.rs O2) | A2 | bug | A branch stacked without sheepdog knowing replays the merged parent's commits |
| 47 | src/act/restack.rs, push.rs | A2/A3 | design-gap | Restacked-but-unpushed window: a plain push from outside undoes the restack |
| 72 | src/act/address.rs | A7/J4 | design-gap | Human threads: a verb to hand one to the agent on request (question) |
| 90 | src/act/attach.rs | A9 | research | Spike: resume -p while the session is open interactively |
| 127 | src/guidance.rs (lib/prompts) | J0/J3 | feature | Default prompt wording for the editing steps |
| 231 | src/main.rs (+ config.rs, state.rs, watch, act/mod.rs) | - | feature | Adopt clap for command-line parsing |
| 40 | - (repo-wide) | - | feature | Rename pr-tend → sheepdog (AGENTS.md cites it as done 2026-10-15; still open) |
| 36 | - (SuperDB, retired #235) | - | research | SuperSQL queries as `.spq` files (bash/SuperDB era) |
| 32 | - (docs/pr-workflow/) | - | feature | Consume docs/pr-workflow/ into code and issues, then delete it (`ls docs/pr-workflow`: design.md, prior-art.md, req-review.md, spikes/ still present) |

## 2. Distribution

**By module** (primary tag; `+` modules not counted):

| Module | Count |
|---|---|
| src/next.rs | 15 |
| src/watch/pass.rs | 13 |
| src/state.rs | 8 |
| src/harness.rs | 5 |
| src/review_local.rs | 5 |
| src/act/describe.rs | 4 |
| src/config.rs | 4 |
| src/watch.rs | 4 |
| src/act/step.rs | 3 |
| src/act/bookkeeping.rs, src/act/review.rs, src/act/restack.rs, src/act/ship.rs, src/act/start.rs, src/usage.rs | 2 each |
| src/act/address.rs, attach.rs, ci_fix.rs, implement.rs, resolve.rs, guidance.rs, main.rs, act/{ci_fix,address,resolve} (question record), mixed (#245) | 1 each |
| none (repo-wide, docs, retired SuperDB) | 3 |

**By step label** (first label where two are given):

| Step | Count | Step | Count |
|---|---|---|---|
| - (none / cross-cutting) | 32 | A2 | 3 |
| N2 | 7 | A10 | 4 |
| A8 | 6 | O1 | 4 |
| J0 | 6 | O8 | 4 |
| J6 | 5 | O5, A0, J1 (2 each); O3, O4, O7, A4, A7, A9, J2, J5 (1 each) | |

**By type:**

| Type | Count |
|---|---|
| bug | 31 |
| feature | 28 |
| design-gap | 14 |
| ux | 6 |
| research | 6 |

Compared with GitHub labels: 13 issues carry `bug`; 31 read as bugs by their text, so 18 bug-shaped issues have no label or only `low priority`. Counts checked with awk over the table above.

## 3. Clusters (5+ open issues)

- **src/next.rs (15).** 11 of the 15 are NEXT showing a step that is wrong for the observed state (#183's output nits included): a verb its gate refuses (#326), a verify failure it can't see (#325), a mechanical step labelled agent (#304), review on an empty branch (#301), restack before push (#300), ship after a reset-away step (#166), a looping merged step (#165), a ready-before-retarget order (#248), a notify beside a superseding step (#265), a `dirty` step while attached (#188). The remaining four are design questions about what NEXT should say (#158, #155, #146, #66). Common thread, observed in their text: NEXT's rules and the act verbs' gates are written separately and drift apart (#326 says this outright).
- **src/watch/pass.rs (13).** 9 come from soak 7 (#271) or the loop tracking tree (#12, #269, #270): dispatch fairness (#298), one step per pass (#299), autoRun notifications (#302), the invisible gate (#307), orphan output (#306), untested reconcile kill (#308), quota accounting (#305). Common thread: the autoRun pass's scheduling and what it reports, all filed in the last two weeks against a feature still marked as slices 3-5 of #12.
- **src/state.rs (8).** Two groups: PR lookup edge cases (#2 own fork, #250 teammate's fork, #264 gh-wait, #180 thread last speaker) and the flag diff-check at `src/state.rs:491` (#147, #334). #266 (shared authoring) names rule 4 as its basis.
- **src/harness.rs (5).** Pi-specific: tool lists (#323), tally (#243), session context (#23); plus two spikes (#135 compaction, #289 approval path). The harness is the one module as-intended §5 allows to be per-harness; the issues are consistent with that.
- **src/review_local.rs (5) and step A8 (6 by first label, 8 counting #151 and #155's second label).** All about what the reviewer session sees: commit bodies (#280), earlier decisions (#240), the whole branch each round (#145), review angles (#279), an importance bar (#126); plus #151 and #142 on the review cap. Common thread: the reviewer is a fresh session with only task, commits and diff, so settled things come back as findings (#240's text).
- **N2 (7).** Notification delivery and policy: Ghostty coupling (#322, #288, #3), status output (#317), suppressions under autoRun (#302, #265), a missed notify (#52).
- **J6 (5).** describe/ship output shape: staleness (#157), `Closes <id>` (#149), PR template (#128), structured output (#21), subject suffix (#189).

## 4. Open design decisions

`question`-labeled: #158, #155, #146, #142, #72, #36. Text says undecided / idea / spike / research (excluding incidental matches such as a `## The idea` heading): #142 (undecided), #279 (idea, vague on purpose), #281 (questions not settled), #135 (spike), #90 (spike), #23 (spike follow-up), #36 (research), #21 (spike list), #271 (soak = research run).

| Issue | In as-intended §8? |
|---|---|
| #158 ready while behind base | yes |
| #155 review a human's commit while CI pending | yes |
| #146 diff-check flags hold or notify | yes |
| #142 reviewAccept at the cap | yes |
| #72 human threads handed to agent | yes |
| #283 config architecture | yes |
| #133 one editing step | yes |
| #69 fork per step | listed in §8, but #69 is **not** in the open list (closed; #276's text says fork per step landed in #275) |
| #36 SuperSQL `.spq` files | no; its subject (SuperDB) is retired per #235 |
| #279 multi-angle local review | no |
| #281 task from issue | no |
| #135 compaction spike | no |
| #90 attach-while-interactive spike | no |
| #23 pi session context spike | no |
| #21 describe `--json-schema` | no |
| #190 risk level per branch | no, though its body says it is the dial behind #76, #142, #146 |
| #66 J5 re-review policy | not in §8; §3 lists J5 as "unbuilt" |

## 5. Duplicates and near-duplicates (same module, overlapping text)

| Pair | Overlap observed |
|---|---|
| #150 / #276 | both `src/usage.rs` cost attribution for a resumed or killed step; #150 says the running total overcounts, #276 says a killed fork is `null` or the full cap. #276 cites the `record` comment #150 is about. |
| #281 / #118 / #149 | all three replace or extend `branch.<b>.sd-task`: source it from the issue, add a verb to edit it, add an item ID beside it. #149's text says it "builds after #118". |
| #263 / #118 | #263's workaround is editing `sd-task` by hand and rerunning implement, which is #118's missing verb. |
| #142 / #190 | #190's table lists #142 as one row of the risk dial. |
| #146 / #190 | same: #190 lists #146 as a row. |
| #251 / #283 | #283's text says it "replaces #251's phase 2"; #251 keeps only the TUI phase. |
| #290 / #283 | #290 says it was split out of #74 / #283 item 5. |
| #250 / #266 | #266 says it replaces #250's mode with observed facts; #250 remains open. |
| #152 / #176 | both about an agent's needs-input question not reaching or outliving its step; #326 cites #176 as related. |
| #322 / #288 / #3 | all three about the glyph/notification coupling to the Ghostty tab. |
| #2 / #250 | #250's text states `pick_pr` already keeps PRs from the viewer's own fork (`src/state.rs:339`), which is #2's untested concern. |
| #183 / #245 | both "small gaps" bundles from consecutive soaks; #245 says it is "in the spirit of #183", and both list repeated output lines. |
| #52 / #302 | both about which NEXT states notify; #52 is the bash-era audit, #302 the autoRun policy. |
| #40 / AGENTS.md | not a pair of issues: #40's rename is described as complete in AGENTS.md ("Named pr-tend until 2026-10-15 (#40)") and memory says `sd migrate` was deleted (#235), yet #40 is open. |

## 6. Side observations from the greps (not issues)

- `src/act_help.txt:116` marks `answer` "Spends tokens"; `grep -n answer AGENTS.md` returns nothing, so AGENTS.md rule 1's list (copied into as-intended §2) omits `answer`. #188 records this nit. Relevant to I5.
- `branch.<b>.sd-task` is read by four verbs independently (`src/act/{describe,ci_fix,review,implement}.rs`) with the same `git_out(... "config" "--get" ...)` call, and by `src/state.rs:1516`. Relevant to #133 and to as-intended §5's "verbs share functions" assumption.
- `docs/pr-workflow/` still has `design.md`, `prior-art.md`, `req-review.md`, `spikes/`; #32 is open and AGENTS.md says the directory stays "only until the current push of issues is over".
