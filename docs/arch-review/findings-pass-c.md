# Pass C: the agent boundary

Judged 2026-10-29 at ed2e392 against `as-intended.md` §2 rules 2 and 3, §3
"every judgment step is wrapped", §6, and the J-steps. Read in full:
`src/harness.rs`, `src/act/{step,implement,address,ci_fix,resolve,answer,review,describe,attach,push,ship,hold}.rs`,
`src/guidance.rs`, `src/review_local.rs`, `src/usage.rs`, every file under
`lib/prompts/`, `lib/review-rules.md`, and `as-built/{externals,issues}.md`.
Nothing was run.

## Verdicts

| Invariant | Verdict | Why |
|---|---|---|
| I1 (agent can't commit, push or call gh) | **holds**, as a cheap extra | Claude: `--allowedTools` is the real gate (`harness.rs:29`, read/edit plus four read-only git patterns plus `agent_rules`), `--disallowedTools` (`harness.rs:21`) is redundant on top. pi: `--tools read,edit,write`, no shell at all. The "log" half of rule 4's "log, don't block" is not built: see C6. |
| I2 (harness names only in harness.rs) | **fails; consolidate, don't strike** | `config.rs:144-163` holds a second copy of each harness's settings table; `step.rs:86` and `attach.rs:54` each default an unowned session to claude; `usage.rs:161-170` parses claude's result object. No `Harness` type exists: `harness.rs` matches on the string in four places (161, 360, 380, 510). See C5. |
| §3 "every judgment step is wrapped by mechanical steps" | **holds** | All six `run_step` callers are verbs that prepare input, run one step, then commit, verify, push and post. The wrapper is copy-pasted three times (C2). |
| I5 (token-spending verbs marked, nothing else reaches a harness) | **holds in code, drifts in AGENTS.md** | No zero-token verb reaches `run_step` or `tui`. AGENTS.md rule 1 omits `ci-fix` and `answer`, which `act_help.txt` marks (C11). |
| §6 sheepdog ↔ agent trust | **holds**, with one gap | Guidance and protected paths read from the base commit (`guidance.rs:31`, `hold.rs:46`); untrusted thread authors make an item chrismo's (`address.rs:133-160`); replies are scrubbed (`address.rs:212`); CI config edits are held (`hold.rs:120`). Gap: C6. |
| §6 branch ↔ its own review | **holds, but discards what it needs** | Fresh read-only session, prompt from the base (`review_local.rs:85`). The pipeline destroys the three decision records the reviewer would need (C1). |

## Findings

| # | Pass | Module | Step | Sev | Rests on | Finding | Status |
|---|---|---|---|---|---|---|---|
| C1 | C | src/act/review.rs, src/act/step.rs, src/review_local.rs | A8/J3 | high | stated (#79 local review; #240) | The pipeline throws away every decision made after the task, then runs a reviewer that can only see the task: `review.rs:47-50` writes a new findings record that "drops the earlier findings' answers" (the declines with their reasons); `step.rs:293-296` deletes the question ref when `sd act answer` runs and the answer text goes only into the fork's prompt, nowhere on record; resolved bot threads (address's `conversation`, `address.rs:152`) are posted to GitHub and never read back. #240's three soak cases and about 70% of one branch's spend are this. Recommend an append-only `refs/sheepdog/<b>/decisions` (one JSON line per declined/accepted finding, answered question, resolved bot thread, with head and text) written by the pipeline at each of those points, and a "Decisions on record" section appended to the review prompt (`review_local.rs:91`) and the review-fix prompt (`implement.rs:242`). This keeps #79's isolation argument intact: the reviewer still never sees the authoring session, only facts the pipeline chose. | raw |
| C2 | C | src/act/implement.rs, src/act/address.rs, src/act/ci_fix.rs | J0/J2/J3 | med | stated (rule 2) | The editing-step skeleton is written three times: dirty-worktree check, `state_json`, gate, `question_gate`, `step_session`, `guidance_base`+`step_guidance`, `agent_rules`+`tool_rules`, the verify line, the `Step {…}` literal, `run_step`, `hold_since`, `step_store`, `question_answered`, the outcome match, `commit_edits`, `hold_since` again, `verify_autofix` (implement.rs:183-334, address.rs:347-515, ci_fix.rs:137-317; `as-built/lints.md` has the three opening identically). `resolve.rs:151-241` is a fourth variant. #270's nine commits each added one interlock to one of these copies. Verdict on #133: extract the runner now (`step::run_editing_step(spec) -> Outcome` taking a prompt payload, a schema and two closures, "before commit" and "after verify"), independent of whether the user-facing verbs merge; merging them is a product decision with real costs (prompt size, one vocabulary) that can wait. #133 step 1 (per-item review-fix answers) is already done: `implement.rs:52`. | raw |
| C3 | C | src/act/{implement,address,ci_fix,resolve}.rs, src/harness.rs | J0-J3 | med | stated (#79 "done signal") | The outcome vocabulary is not one: implement and address return `done/needs-input/gave-up`; ci-fix returns `fixed/flaky/needs-input/gave-up` (`ci_fix.rs:26`); resolve returns `done/gave-up/already-on-target` (`resolve.rs:38`) and has no needs-input by design. Items use `fixed/declined/ask` in review-fix (`implement.rs:59`) but `fixed/reply/pushback/ask` in address (`address.rs:40`) for the same act of not changing code, though `implement.rs:50` says review-fix uses "the vocabulary #133 gives every source". The step-level outcome is parsed in four separate `match` blocks (implement.rs:284, address.rs:463, ci_fix.rs:272, resolve.rs:107) with the unknown-outcome fallback copied each time. Recommend `run_step` return a typed `Outcome { Done, NeedsInput(text), GaveUp(text), Other(String) }` parsed once in harness.rs or step.rs, with each verb matching only its own `Other` values, and one item vocabulary (`fixed/declined/ask`, plus `flaky` for CI) as #133 proposes. | raw |
| C4 | C | src/act/ci_fix.rs, lib/prompts/ci-fix.md | J2 | med | stated (rule 2; #79 "done signal") | ci-fix has no contract constant: the "do not commit, push, or run gh" rule and the `fixed/flaky/needs-input/gave-up` vocabulary live only in `lib/prompts/ci-fix.md`, which is the *guidance*, overridable per repo by `settings.ciFixPrompt` or `.sheepdog/ci-fix.md` (`ci_fix.rs:219`). Every other editing verb keeps its contract in Rust next to its schema (`IMPLEMENT_CONTRACT` implement.rs:23, `ADDRESS_CONTRACT` address.rs:46, `CONTRACT` resolve.rs:25). A repo that sets its own ci-fix guidance silently loses the outcome contract the pipeline parses. Move the last two paragraphs of `ci-fix.md` into a `CI_FIX_CONTRACT` appended after the guidance. | raw |
| C5 | C | src/harness.rs, src/config.rs, src/act/step.rs, src/act/attach.rs, src/usage.rs | - | med | stated (rule 3, I2) | There is no `Harness` type; the harness is a `&str` matched in four places in harness.rs and the per-harness knowledge is spread over five modules: `config.rs:147-158` lists each harness's settings fields (claude: model/effort/args/useApiKey; pi: provider/model/effort/args) while `harness.rs:379-384` maps the same fields to flags, so adding a harness or a field edits both; `step.rs:86` and `attach.rs:54` each hard-code "a session with no stored harness is claude's"; `usage.rs:161-176` knows claude's result object (`total_cost_usd` is a running total, `usage.input_tokens`, `num_turns`) and pi's absence of one; `harness.rs:74-75` makes every caller pass both `claude_allow` and `pi_tools`. Recommend one `enum Harness { Claude, Pi }` in harness.rs with a descriptor per variant (settings schema, flag table, editing and read-only tool sets, deny list, has native schema, transcript locator, cost model, legacy-default flag), `config.rs` deriving `CLAUDE`/`PI` from it, `Step` taking a `Tools::{Editing(rules), ReadOnly}` that the harness maps, and `usage::record` taking a harness-neutral `Spend` the adapter fills. | raw |
| C6 | C | src/act/{implement,address,ci_fix}.rs, src/state.rs, src/next.rs, src/act/hold.rs | A3/J0-J3 | med | stated (#79 "Log, don't block") | "A detected agent push is recorded and surfaced; a repeat offender means the step-contract prompt needs tightening" is not built. An agent commit is detected (`agent_head != before`, implement.rs:280, 289) but logged only on ci-fix's flaky path (`ci_fix.rs:280-281`); an agent push lands the agent's SHA in this worktree's reflog, so `state.rs:1348-1352` reports `remote_ours: true` and NEXT shows a normally pushed branch (the `foreign` step at `next.rs:153` fires only for SHAs the reflog lacks). The one detection that does exist is `hold.rs:134-139`, and only for protected CI paths, and it dies rather than logs. The reconciliation half works (`push_branch` "nothing to push", ship's find-and-fix-up `ship.rs:169-181`). Recommend: after `run_step`, compare `before`, `agent_head` and the remote-tracking ref; print and record a `stray` field on the step's `usage.sup` row (`{commit: bool, push: bool}`) so `sd state`'s spend summary can count offenders per branch and harness. | raw |
| C7 | C | src/harness.rs, src/usage.rs, src/act/attach.rs | - | med | inferred (harness.rs header) | sheepdog depends on harness internals that are not contracts: transcript locations and name patterns (`harness.rs:505-520`: `~/.claude/projects/**/<sid>.jsonl`, `~/.pi/agent/sessions/**/_<sid>.jsonl`), JSONL line shapes (`tally_of` 553-584: `type:assistant`, `isSidechain`, `message.usage.*`, model `<synthetic>`; pi's `message.role`, `usage.cacheRead/cacheWrite/cost.total/totalTokens`), the limit entry's `apiErrorParams.rate_limit_info.resetsAt` (297-303), the result object's `api_error`, `structured_output`, `session_id`, `total_cost_usd` running-total semantics (`usage.rs:82-92`), and the flag set (`--json-schema`, `--fork-session`, `--permission-mode acceptEdits`, `--max-budget-usd`; pi `--fork`, `--tools`, `--thinking`). The dependency is mostly confined to harness.rs (usage.rs is the leak, see C5), but it is pinned only by hand-written unit fixtures that encode what was true at one version (#79's table says "Claude Code 2.1.296, pi 0.99.1, from --help 2026-10-11"). Nothing checks the installed harness. Recommend a `sd harness probe` (run a trivial `-p` step per installed harness, assert the result object, transcript file and tally shape) that chrismo runs after a harness upgrade, the way agent-rig's `claude-contract.bats` does; and record the verified version in the harness.rs header. | raw |
| C8 | C | src/harness.rs | J0-J3 | low | stated (rule 3) | `tool_rules` tells pi "You can't run commands here beyond read-only git" (`harness.rs:40`) but pi's tools are `read,edit,write` (`harness.rs:30`): no shell, so no git at all. The prompt promises a capability the agent lacks. pi also can never run `settings.verify` itself, so for pi the "The repo's checks, which the pipeline runs after you" line (implement.rs:252) is its only word on checks; the wording should say so. Make the sentence per harness inside `tool_rules`. | raw |
| C9 | C | src/act/{implement,ci_fix,address}.rs, src/act/step.rs | J0/J2/J3 | low | stated (#79 "done signal") | needs-input means different things per verb and the contracts don't say: implement and ci-fix stop, leaving edits uncommitted (implement.rs:297-306, ci_fix.rs:289-292); address commits, verifies, pushes and posts the non-ask replies, then records the asks as the question (address.rs:468-514). Both are defensible (address items are independent), but `ADDRESS_CONTRACT` (address.rs:46-58) says only `"needs-input" (anything you marked ask)`. State the publish-the-rest behaviour in the contract, or align. | raw |
| C10 | C | src/act/resolve.rs, src/config.rs | J1 | low | stated (rule 1) | `restack --resolve` runs under implement's step config (`resolve.rs:158`: `step_session(b, "implement", …)`) but records itself as step `resolve` in usage.sup and the running marker (`resolve.rs:205`), and `config.rs:145` `STEPS` has no `resolve`, so its model and effort can't be tuned apart from implement's while its cost is reported apart. Either add `resolve` to `STEPS` or name the step `implement` in the record and say in the header that resolve is implement mid-rebase. | raw |
| C11 | C | AGENTS.md, src/act_help.txt | - | low | stated (rule 1) | AGENTS.md rule 1 lists the token-spending verbs as implement, address, describe, review, restack --resolve, start and ship; `act_help.txt` also marks `ci-fix` (:116) and `answer` (:74), both of which reach `run_step`. The rule's list is the one agents read first. Add ci-fix and answer to AGENTS.md. | raw |
| C12 | C | src/act/step.rs, src/act/github.rs, src/act/review.rs, src/act/implement.rs | J3/J4 | low | stated (I8) | The two agent-facing records each have two writers with no shared function: `refs/sheepdog/<b>/question` is deleted by `step.rs:295` (answered) and `github.rs:76` (ack --question); `refs/sheepdog/<b>/findings` is written by `review.rs:158` (a round) and `implement.rs:315` (the answers), each building the blob and running `hash-object`/`update-ref` itself. With C1's decisions record this becomes three. Give each record one module with `write`/`clear` (as `failed.rs` and `verify_red.rs` already do) and call it from both sites. | raw |
| C13 | C | src/harness.rs | - | low | stated (I14) | `sd harness run` without `HARNESS_BRANCH` runs a step with no running marker (`harness.rs:104-110`): a test seam reachable by hand, and the one way to spend tokens on a branch's worktree while NEXT thinks nothing is running. Refuse without `HARNESS_BRANCH` outside debug builds, or derive the branch from the worktree as `run_step` already does when the arg is empty. | raw |

Findings resting on an **[assumed]** claim: none. C5 and C7 rest on the
[inferred] module-header ownership; the rest on [stated] rules.

## Design-level observations

**The boundary itself is sound.** Rule 2 is implemented the way #79 describes
it, and the places that matter are careful: guidance and protected paths come
from the base commit so a branch can't rewrite its own instructions, an item
touched by an untrusted author is handed to chrismo rather than the agent,
replies are scrubbed of /commands and @mentions before posting, CI-config
edits are held for a human push, and a PR that merged or closed under a step
gets nothing pushed or posted. The deny lists are what the design says they
should be, cheap extras on top of an allow list and a reconciling pipeline.
Nothing in this pass found an agent able to reach GitHub or the remote
through sheepdog's own tooling.

**The boundary was built one verb at a time, and the seams show.** Three
editing verbs carry the same thirty-line skeleton, four verbs each parse the
outcome, and #270's nine commits were the cost of adding one interlock to each
copy in turn. #133 reads this as "one editing step"; the evidence supports a
narrower, cheaper move first: one internal runner that all four verbs call,
with the verb-specific parts (payload, schema, what to do before commit and
after verify) passed in. Whether the user-facing verbs then merge is a
separate question about prompt size and one vocabulary, and can be decided
after the runner exists.

**The reviewer's isolation is right; what it's shown is wrong.** #79's
argument is that the reviewer must not share the author's conversation. The
code honours that, and then goes further than the argument requires: it
discards the declines, the answers and the resolved threads too. Those are
facts the pipeline produced, not the author's reasoning, and withholding them
is why the same finding comes back three times and chrismo ends up pressing
`--accept`. A decisions record written by the pipeline and read by both the
reviewer and the fix round is the smallest change that fixes #240, #145 and
most of #279's motivation without touching the isolation.

**Rule 3 needs a type, not more discipline.** Harness-agnosticism is held
together by string matches in harness.rs and a second copy of each harness's
field table in config.rs. It has worked for two harnesses. A third, or a
change to pi's tool flags, would need edits in five modules with nothing to
say which. An `enum Harness` with a descriptor per variant moves the question
"what is per-harness?" from a grep to a type, and lets `Step` stop carrying
one field per harness.

**What sheepdog knows about the harnesses was true on 2026-10-11.** Transcript
paths, JSONL shapes, the limit entry's field path and the running-total
semantics of `total_cost_usd` are all internals, verified against one version
each and pinned by fixtures that can't tell when they go stale. agent-rig
already solved this for its own Claude Code dependencies with a contract
suite run on version change; sheepdog should borrow the pattern.

**One wording note on rule 2.** "Agents never need to know the pipeline
exists" is contradicted, correctly, by every contract: "the pipeline commits
your edits with your commit_message". The agents do need to know a pipeline
will commit, push and post for them, or they'll try to. The rule means
"agents never *drive* the pipeline"; AGENTS.md could say that.
