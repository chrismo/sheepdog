# As built: module structure

Facts gathered 2026-10-29 for the architecture review, against
`docs/arch-review/as-intended.md` §5 (ownership) and §7 (I10-I13). Read-only
on `src/`; nothing here is a fix proposal. Line numbers are as of `main` at
9aa97f0.

Method: `grep` over `src/` for `use crate::`, `use super::`, inline
`crate::x::` / `super::x::` and `act/mod.rs`'s `verb::fn` dispatch, test
modules (`mod tests {`) and `//` comments stripped first. Scripts (not in the
repo): `fnlen.py` (brace-matching function lengths), `pubuse.py` (`pub fn`
per module and other-module users by word match), `deps.py` (edges, fan-in /
fan-out, Tarjan SCCs). `cargo-modules` is not installed. `cargo test` was not
run.

## 1. Dependency graph

Nodes are files (`act/*` and `watch/pass` separate). An edge is any
`use`/path reference to that module in non-test code.

### Edge table (from → to)

| From | To |
|---|---|
| `main` | act/mod, config, harness, review_local, state, watch |
| `state` | config, git, json, **next**, proc, running, time, usage, val, verify_red |
| `next` | json, val |
| `act/mod` | every `act/*` (dispatch), config, git, harness, proc, running, state, val |
| `act/step` | act/commands, act/mod, **act/start**, config, git, harness, json, running, **state**, val |
| `act/address` | act/commands, act/hold, act/implement, act/mod, act/push, act/step, config, git, guidance, harness, proc, running, **state**, val |
| `act/answer` | **act/address, act/ci_fix, act/implement**, act/mod, git, state, val |
| `act/attach` | act/commands, act/mod, config, git, harness, proc, running, time, usage, val |
| `act/bookkeeping` | act/mod, config, git, proc, running, val |
| `act/ci_fix` | act/commands, act/hold, act/implement, act/mod, act/push, act/step, config, git, guidance, harness, proc, val |
| `act/commands` | act/mod, git, proc, verify_red |
| `act/describe` | act/commands, act/implement, act/mod, act/restack, act/review, act/step, config, git, guidance, harness, json, val |
| `act/follow` | act/bookkeeping, act/commands, act/mod, act/review, act/start, git |
| `act/github` | act/mod, config, git, proc, time, val |
| `act/hold` | act/mod, config, git, proc |
| `act/implement` | act/commands, act/hold, act/mod, act/step, config, git, guidance, harness, json, val |
| `act/push` | act/mod, git |
| `act/resolve` | act/mod, act/step, config, git, guidance, harness, proc, val |
| `act/restack` | act/commands, act/hold, act/mod, act/resolve, act/step, git, proc, running, val |
| `act/review` | act/commands, act/mod, config, git, harness, json, limit, proc, running, **state**, val |
| `act/ship` | act/bookkeeping, act/commands, act/describe, act/hold, act/mod, act/push, act/restack, act/review, config, git, proc, val |
| `act/start` | act/bookkeeping, act/mod, config, git, proc |
| `config` | **act/mod** (config.rs:835 `crate::act::plain_words`), git |
| `harness` | config, **failed**, git, json, **limit**, proc, **running**, time, **usage**, val |
| `guidance` | config, git |
| `review_local` | config, git, guidance, harness, json, val |
| `running` | git, proc, time, val |
| `usage` | config, json, val |
| `limit` | config, time |
| `failed`, `verify_red` | git, json, time |
| `git` | proc |
| `val` | json |
| `json`, `proc`, `time`, `lib` | (none) |
| `watch` | config, git, proc, watch/pass |
| `watch/pass` | config, failed, git, harness, json, limit, proc, running, time, val, watch (parent: `die`, `env_or`, `home`, `HELP`) |

### Fan-in / fan-out (file granularity, 39 nodes)

| Module | out | in | Module | out | in |
|---|---|---|---|---|---|
| act/mod | 23 | 20 | git | 1 | **29** |
| act/address | 14 | 2 | config | 2 | **23** |
| act/ci_fix | 12 | 2 | val | 1 | 21 |
| act/describe | 12 | 2 | proc | 0 | 19 |
| act/ship | 12 | 1 | json | 0 | 13 |
| act/review | 11 | 4 | harness | 10 | 12 |
| watch/pass | 11 | 1 | act/commands | 4 | 11 |
| act/attach | 10 | 1 | running | 4 | 10 |
| act/implement | 10 | 5 | time | 0 | 9 |
| act/step | 10 | 7 | act/step | 10 | 7 |
| state | 10 | 6 | guidance | 2 | 6 |
| act/restack | 9 | 3 | state | 10 | 6 |
| act/resolve | 8 | 1 | act/hold | 4 | 5 |
| act/answer | 7 | 1 | act/bookkeeping | 6 | 4 |
| main | 6 | 0 | act/push | 2 | 4 |
| review_local | 6 | 1 | next | 2 | 1 |

Full table: `deps.py` output, section FAN.

### Cycles

Tarjan SCCs over the edge table:

1. **One SCC of 25 files** at file granularity: every `act/*` file plus
   `config`, `guidance`, `harness`, `limit`, `state`, `usage`. Collapsing
   `act/*` to one node, the SCC is `act ↔ config ↔ guidance ↔ harness ↔ limit
   ↔ state ↔ usage`. The single edge that closes it is
   `config → act`: `src/config.rs:835` calls `crate::act::plain_words`,
   re-exported at `src/act/mod.rs:21` from `src/act/step.rs:165`. Without
   that edge, `config` has fan-out 1 (`git`) and the graph is acyclic apart
   from (2). **[inference]** `plain_words` is a pure string check on the
   verify command; its home in `act/step.rs` is what makes config depend on
   act.
2. `watch ↔ watch/pass`: parent/child (`pass.rs:10 use super::{die, env_or,
   home, HELP}`; `watch.rs:236-241` dispatches to `pass::`). Same shape as
   `act/mod ↔ act/*`; ordinary Rust parent-module structure, not a design
   cycle.

No cycle touches `next`, `git`, `proc`, `json`, `val`, `time`, `running`,
`failed`, `verify_red`, `review_local`, `watch`.

## 2. Imports vs. stated ownership (as-intended §5)

### I10: `next.rs` imports nothing touching git, gh or the filesystem — HOLDS

`src/next.rs:70` `use crate::val::{self, at, sj, text}`; `src/next.rs:845`
`crate::json::float`. Those are its only crate references
(`grep -n 'crate::' src/next.rs`). `val.rs` imports only `serde_json` and
`crate::json` (`val.rs:5,19-20`); `json.rs` imports only `serde_json`
(`json.rs:5`). No `std::fs`, `std::process` or `Command` in `next.rs`
(`grep -n 'std::\|Command\|fs::' src/next.rs` hits only a
`std::cmp::Ordering` at :207).

### I11: `state.rs` performs no write — DOES NOT HOLD (one write, by design)

`src/state.rs:259-316` `lookup_pr`: when the stored PR number is missing,
gone, closed or on another head, discovery picks one and:

- `state.rs:298` `git(self.dir, &["config", "--unset", "branch.{b}.sd-pr"])`
- `state.rs:304` `git(self.dir, &["config", "branch.{b}.sd-pr", &pick])`

Its doc comment (`state.rs:263-267`) says so: "discovery picks one and
stores its number (unset for none), for a local branch only". Everything
else observed in `state.rs` is a read: `gh api graphql` queries
(`state.rs:222-224`, `:1082` REST GET for required checks), `git ls-remote`
(`:1607`, network read), `fs::read` (`:106`), `git config --get`
(`:269, :322, :1115, :1504`). No `update-ref`, `push`, `commit`, `fs::write`
(`grep -n 'fs::write\|update-ref\|"push"\|"commit"' src/state.rs`: no code
hits; `:1315` is advice text in an error string). `branch.<b>.sd-pr` has one other
writer, `act/follow.rs:64` (`grep -rn 'sd-pr' src/act`); the record has two
writer modules, and neither module's header names the other.

### Other imports outside the module's stated ownership

| Module | Stated owner of | Imports / calls beyond that | Where |
|---|---|---|---|
| `config.rs` | settings files, layering, profiles | `crate::act::plain_words` | config.rs:835 (closes the big cycle) |
| `harness.rs` | per-harness code: headless step, TUI, tool rules | writes four state records: `running::during` :111, `usage::append` :230, `limit::write` :252, `failed::clear/record` :267-277 | harness.rs:9 imports `failed, limit, running, usage` |
| `state.rs` | Observe O1-O8 | git config write (above); exports helpers `unbot` (:620) and `flag_lines` (:1791) consumed by `act/address.rs:15` and `act/review.rs:15`; `question_state` (:151) by `act/step.rs:261`, `act/answer.rs:10` | state.rs |
| `act/step.rs` | what every agent step shares | imports a verb: `super::start::start_default` | step.rs:6, :155 |
| `act/answer.rs` | one verb | calls three other verbs' entry points: `implement::implement`, `address::address`, `ci_fix::ci_fix` | answer.rs:48-50 |
| `act/mod.rs` | dispatch and shared plumbing | `state::observe` in-process rather than a `sd state --json` child; `lock_stale`/`lock_sweep` (attach lock) | mod.rs:248; :160-180 |
| `act/describe.rs` | one verb | `super::restack::state`, `super::review::short`, `super::implement::blank` | describe.rs:6-8 |
| `watch/pass.rs` | the pass, refresh, vacuum | runs `gh api` directly (`Command::new("gh")` :818, `proc::with_timeout(.."gh"..)` :528); writes `failed::record_in` :15 | pass.rs |
| `running.rs` | the running marker | `git::common_dir` to name the marker file | running.rs:15, :102 |

Cross-verb shared functions (per the §5 **[assumed]** "shared function, not
entry point" rule) observed: `ship → describe::describe_run, push::push_branch,
restack::parent_to_default, bookkeeping::enroll/record_lineage`
(ship.rs:5-11); `address/ci_fix → push::push_branch, implement::blank,
commands::verify_autofix` (address.rs:5-8, ci_fix.rs:7-10); `follow →
start::destination/start_hook/git_on_stderr` (follow.rs:15). Only
`answer.rs:48-50` calls another verb's top-level entry.

### I12 / I13 (not asked for in depth; counted while here)

- `gh` is run from 8 modules: `state.rs` (:224, :1082), `act/ci_fix.rs`
  (:54, :116), `act/restack.rs:356` (`pr edit --base`), `act/address.rs:124`
  (graphql: `mutation` strings at :301, :308, :321 post replies, resolve
  threads and add comments), `act/hold.rs:165` (`pr view`), `act/ship.rs:160,175`
  (`pr create`, `pr edit`), `act/github.rs:191` (`api -f`), `watch/pass.rs:528,818`.
  Command: `grep -rn 'run("gh"\|"gh",\|Command::new("gh")' src`.
- `git` is run outside `git.rs` by `Command::new("git")` at
  `act/restack.rs:300` (`reset --hard` with `GIT_REFLOG_ACTION`),
  `act/start.rs:118` (stderr passthrough), `act/resolve.rs:137,232`
  (`GIT_INDEX_FILE`, `GIT_EDITOR`). Each sets an env var or stream mode
  `git.rs`'s wrappers don't take **[inference]**. `guidance.rs:124` and
  `config.rs:1461` are in test modules.

## 3. Size outliers

### 15 longest functions (non-test; brace-matched by `fnlen.py`)

| Lines | Function | Where |
|---|---|---|
| 613 | `steps` | src/next.rs:133 |
| 513 | `observe` | src/state.rs:1110 |
| 304 | `scrub` | src/act/address.rs:212 |
| 239 | `assemble` | src/state.rs:789 |
| 181 | `ci_fix_run` | src/act/ci_fix.rs:137 |
| 169 | `address_run` | src/act/address.rs:347 |
| 152 | `implement_run` | src/act/implement.rs:183 |
| 139 | `explain` | src/config.rs:778 |
| 130 | `restack` | src/act/restack.rs:153 |
| 127 | `flags_json` | src/state.rs:490 |
| 127 | `agent` | src/harness.rs:154 |
| 120 | `check` | src/watch/pass.rs:537 |
| 118 | `ship` | src/act/ship.rs:65 |
| 109 | `cli` | src/config.rs:1197 |
| 99 | `start` | src/act/start.rs:139 |

Next five: `summary` state.rs:1626 (97), `pass` pass.rs:899 (93), `reviewer`
next.rs:748 (93), `resolve_conflicts` resolve.rs:151 (91), `stacked_on`
git.rs:290 (85).

### Files over 800 lines

| File | Lines | What the length is |
|---|---|---|
| `src/state.rs` | 1955 | three big fns: `observe` 513 + `assemble` 239 + `flags_json` 127 = 45% of the file; a 30-line GraphQL `QUERY` const (:23-55); 49 non-test fns; tests only :1858-1955 |
| `src/config.rs` | 1475 | many small: 69 non-test fns totalling 947 lines, longest 139 (`explain`); ~360 lines of consts, structs and doc comments; tests :1308-1475 |
| `src/watch/pass.rs` | 1340 | many mid-size: 54 non-test fns totalling 955 lines, top three 120/93/66; tests :1191-1340 |
| `src/git.rs` | 911 | 58% tests: `pub(crate) mod tests` starts at :386 and holds the `Repo` fixture other modules' tests import (`act/hold.rs:187`, `state.rs:1952`); 33 `pub fn` in the 385 code lines |
| `src/next.rs` | 876 | one big fn: `steps` is 613 lines, 70% of the file; 14 fns; no `mod tests` (acceptance tests are `tests/next.rs`) |
| `src/harness.rs` | 886 | `agent` 127, `run` 56; 29 non-test fns totalling 466 lines; tests :654-886 |

## 4. `pub fn` inventory

Counted with `grep -c '^\s*pub\(([a-z]*)\)\? fn '` per file; "1 other" = the
name appears in exactly one other module's non-test code (word match by
`pubuse.py`, so a shared name such as `run`, `state` or `status` can
over-count users; under-counting is not possible). `lib.rs` makes every
module `pub`; `pub(crate)`/`pub(super)` appears 30 times in all of `src/`
(`grep -rn 'pub(crate)\|pub(super)' src | wc -l`).

| Module | pub fn | used by 1 other | used by 0 others |
|---|---|---|---|
| git.rs | 33 | 1 | 2 (`parse_worktrees` :137, `gh_repo_of` :237; both used inside git.rs) |
| act/step.rs | 17 | 2 | 0 |
| act/mod.rs | 14 | 1 | 2 (`lock_stale` :160, `lock_sweep` :171; used inside mod.rs) |
| config.rs | 12 | 4 | 0 |
| proc.rs | 12 | 6 | 0 |
| running.rs | 10 | 3 | 1 (`proc_start` :23; used by `tests/act_ci_fix.rs`, `tests/harness.rs`) |
| harness.rs | 9 | 3 | 0 |
| act/commands.rs | 9 | 1 | 0 |
| usage.rs | 7 | 2 | 0 |
| val.rs, json.rs | 6, 6 | 0, 3 | 0 |
| act/bookkeeping.rs, act/start.rs | 6, 6 | 2, 5 | 0 |
| state.rs, watch/pass.rs | 5, 5 | 3, 3 | 0 |
| verify_red.rs, failed.rs, act/restack.rs, act/hold.rs, act/follow.rs | 4 each | 0, 1, 1, 1, 1 | 0 |
| time.rs, guidance.rs, act/describe.rs | 3 each | 0, 0, 1 | 0 |
| limit.rs, next.rs, act/review.rs, act/push.rs, act/github.rs, act/implement.rs | 2 each | 0 | 0 |
| 9 one-fn modules (review_local, watch, act/{attach,ci_fix,answer,resolve,address,ship}) | 1 each | resolve: 1 | 0 |
| **Total** | **218** | **46** | **5** |

Single-consumer `pub fn`s by consumer (candidates for `pub(super)` or a move;
list from `pubuse.py`):

- consumed only by `watch/pass.rs`: `config::sweep_branch_files` :472,
  `failed::record_in` :38, `git::branch_in` :53, `proc::with_timeout` :62,
  `running::{outcome_file :18, leases :99, leasing :204}`.
- consumed only by `harness.rs`: `json::go_g` :91, `usage::{step_cost :81,
  session_total :93}`.
- consumed only by `act/attach.rs`: `harness::{tui :354, session_exists
  :399}`, `proc::foreground` :180.
- consumed only by `act/ship.rs`: `bookkeeping::record_lineage` :88,
  `describe::describe_run` :61, `restack::parent_to_default` :126.
- consumed only by `act/restack.rs`: `hold::hold_after` :96,
  `resolve::resolve_conflicts` :145, `step::resolve_gave_up` :219.
- consumed only by `act/follow.rs`: `start::{destination :61, start_hook
  :81, git_on_stderr :115}`.
- consumed only by `act/mod.rs`: `state::observe` :1105,
  `config::get_global_in` :690, `bookkeeping::unregister` :224,
  `follow::follow` :53.
- others: `commands::verify_autofix_to` → ci_fix; `mod::attached_during` →
  resolve; `start::start_default` → step; `step::fork_of` → resolve;
  `config::branch_config_file` → bookkeeping; `config::set_branch_profile` →
  start; `harness::tally` → step; `json::{sup_string :83, sup_float :102}` →
  usage; `proc::with_timeout_merged` → commands; `proc::timeout_passing_stderr`
  → review; `proc::{die_of_caught :232, deferring_signals :241}` → running;
  `state::unbot` → address; `state::flag_lines` → review;
  `pass::{refresh, vacuum, vacuum_preview}` → watch.

One false positive in that run: `start::git_or_die` (:107, `pub(super)`)
matched `review_local.rs:46`, which is a different private fn of the same
name; it has no other-module user.

## 5. Facts answering open items in as-intended

- §4 "Stored PR description: unknown location": it is
  `$GIT_COMMON_DIR/sheepdog/<branch with / as __>.describe.json`
  (`config.rs:470-472` `describe_file`, written at `act/describe.rs:110`,
  removed by `act/bookkeeping.rs:125,207` and `config::sweep_branch_files`).
- §3 "`sd state --json` is what `sd act` reads": `sd act` reads it
  in-process via `state::observe` (`act/mod.rs:229-250`, with a debug-only
  fixture path); the watcher runs it as a child (`watch/pass.rs:538`).
