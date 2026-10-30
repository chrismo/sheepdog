# As built: compiler and lint signals

Facts gathered 2026-10-29 at `main` 9aa97f0 for the architecture review.
Toolchain: `rustc 1.90.0 (1159e78c4 2025-09-14)`, `clippy 0.1.90`. No
`clippy.toml` and no `[lints]` table in `Cargo.toml`, so every lint is at its
default level. Raw clippy output is in the session scratchpad, not the repo;
every number below is reproducible from the command shown.

Labels: a plain sentence is an observed fact. **[inference]** marks a
conclusion drawn from facts.

## 1. `cargo build --release`

Command: `cargo build --release 2>&1 | grep -c warning`

Result: exit 0, **0 warnings**.

## 2. Clippy, pedantic + nursery

Command: `cargo clippy --all-targets -- -W clippy::pedantic -W clippy::nursery 2>&1`

- 774 `warning:` lines in all; of those, 554 for `lib`, 612 for `lib test`
  (551 duplicates of the lib ones), the rest spread across the 21 `tests/*.rs`
  targets (10 of each test target's warnings are duplicates of
  `tests/common/mod.rs`).
- Deduplicated by (lint, location): **740 unique warnings**, 604 in `src/`
  and 136 in `tests/`. 111 distinct lint kinds.

### Top 20 kinds (unique sites, lib + tests)

`missing_errors_doc` (19) and `module_name_repetitions` (0 here) are skipped
as noise per the task; `missing_errors_doc` would sit at rank 11.

| # | Lint | Count | Example |
|---|---|---|---|
| 1 | `doc_markdown` (item in docs missing backticks) | 182 | src/act/bookkeeping.rs:68:47 |
| 2 | `must_use_candidate` | 81 | src/act/mod.rs:160:8 |
| 3 | `needless_borrow` (default-level, see §3) | 53 | src/act/mod.rs:327:30 |
| 4 | `format_push_string` | 50 | src/act/address.rs:428:9 |
| 5 | `unreadable_literal` | 41 | src/config.rs:109:26 |
| 6 | `cast_possible_truncation` | 25 | src/act/implement.rs:221:20 |
| 7 | `inefficient_to_string` (`to_string` on `&&str`) | 21 | src/act/address.rs:555:26 |
| 8 | `too_long_first_doc_paragraph` | 20 | src/act/mod.rs:204:1 |
| 9 | `or_fun_call` | 20 | src/act/address.rs:148:55 |
| 10 | `similar_names` | 19 | src/act/ci_fix.rs:243:13 |
| 11 | `many_single_char_names` | 16 | src/act/address.rs:347:16 |
| 12 | `use_self` | 13 | src/act/resolve.rs:70:9 |
| 13 | `too_many_lines` | 13 | src/act/address.rs:347:1 |
| 14 | `float_cmp` | 13 | src/next.rs:273:46 |
| 15 | `needless_pass_by_value` | 11 | src/act/address.rs:537:14 |
| 16 | `option_if_let_else` | 10 | src/act/ship.rs:141:16 |
| 17 | `cast_possible_wrap` | 10 | src/running.rs:216:70 |
| 18 | `format_collect` | 9 | src/act/mod.rs:246:17 |
| 19 | `assigning_clones` | 9 | src/config.rs:1228:25 |
| 20 | `redundant_closure_for_method_calls` | 8 | src/act/address.rs:555:22 |

Also present: `missing_panics_doc` 8, `cast_sign_loss` 8, `useless_let_if_seq`
6, `unused_self` 4, `single_match_else` 4, `needless_raw_string_hashes` 4,
`missing_const_for_fn` 4, `ignored_unit_patterns` 4, `cognitive_complexity` 4.

**[inference]** The top two kinds (263 of 740) are documentation formatting
and `#[must_use]` candidates; the next (`needless_borrow`, 53) is almost
entirely two call sites (§3). The bulk of the pedantic noise is style, not
structure.

### Unique warnings per `src/` module (top 10)

| Module | Warnings | Lines |
|---|---|---|
| src/state.rs | 66 | 1955 |
| src/act/mod.rs | 65 | 481 |
| src/harness.rs | 52 | 886 |
| src/watch/pass.rs | 46 | 1340 |
| src/config.rs | 45 | 1475 |
| src/git.rs | 44 | 911 |
| src/usage.rs | 38 | 289 |
| src/proc.rs | 23 | 519 |
| src/running.rs | 21 | 453 |
| src/json.rs | 20 | 151 |

`src/act/mod.rs` owes 53 of its 65 to the `needless_borrow` cluster in `act()`.

### Called-out lints

**`too_many_lines` (13; limit 100):**

| Function | Lines | Location |
|---|---|---|
| `next::steps` | 600 | src/next.rs:133 |
| `state::observe` | 448 | src/state.rs:1110 |
| `state::assemble` | 230 | src/state.rs:789 |
| `ci_fix::ci_fix_run` | 172 | src/act/ci_fix.rs:137 |
| `address::address_run` | 155 | src/act/address.rs:347 |
| `implement::implement_run` | 140 | src/act/implement.rs:183 |
| `config::explain` | 131 | src/config.rs:778 |
| `restack::restack` | 121 | src/act/restack.rs:153 |
| `state::flags_json` | 115 | src/state.rs:490 |
| `harness::agent` | 115 | src/harness.rs:154 |
| `ship::ship` | 113 | src/act/ship.rs:65 |
| `pass::<Pass>::check` | 108 | src/watch/pass.rs:537 |
| `config::cli` | 105 | src/config.rs:1197 |

**`cognitive_complexity` (4; limit 25):** `next::steps` 90
(src/next.rs:133), `state::observe` 76 (src/state.rs:1110),
`state::assemble` 27 (src/state.rs:789), `ci_fix::ci_fix_run` 27
(src/act/ci_fix.rs:137).

**[inference]** `next::steps` and `state::observe` are the two functions the
as-intended page names as the whole of NEXT and the whole of Observe (§3
"Observe is the single source of truth", "NEXT is derived from state"); they
are also the two largest and most complex functions in the binary. The
three `*_run` functions (address, ci_fix, implement) start with the same
three lines (`answering` / `status(wt)` / `die`), consistent with the
scheduled consolidation in #133 (as-intended §8).

**`too_many_arguments`:** 0 warnings. One `#[allow(clippy::too_many_arguments)]`
at src/state.rs:696 on `owed_item` (9 parameters) suppresses the only
candidate found.

**`large_enum_variant`:** 0.

**`needless_pass_by_value` (11):** 6 are in `#[cfg(test)]` modules
(src/act/address.rs:537, :714; src/act/describe.rs:135;
src/act/implement.rs:345; src/act/ship.rs:189; src/config.rs:1311). The 5 in
non-test code: src/act/hold.rs:172 (`pr_refusal(.., read: Result<String,
String>)`), src/act/restack.rs:139 and src/act/review.rs:20 (two private
`fn must(o: proc::Out)` helpers), src/next.rs:84 (`step(.., cmd:
Option<String>)`), src/state.rs:1035 (`config_failed(lines: Vec<String>)`).

**`float_cmp` (13):** the 4 in non-test `src/` are all `n(..) == 1.0`
pluralization checks (src/next.rs:273, :793; src/state.rs:1699, :1769); the
other 9 are in `usage.rs`'s and `tests/harness.rs`'s test code.
**[inference]** none is on a decision path.

## 3. Clippy, default lints

Command: `cargo clippy --all-targets 2>&1`

58 `warning:` lines; 54 for `lib` (all 54 duplicated in `lib test`), 1 for
`tests/act_start.rs`, plus 3 summary lines. Every warning in full:

| Lint | Count | Where |
|---|---|---|
| `needless_borrow` ("creates a reference which is immediately dereferenced") | 42 | src/act/mod.rs:327-348, in `fn act(verb, b: &str, remote: &str, a: &Args)`: every `&b`, `&remote`, `&a` passed to a verb |
| `needless_borrow` | 11 | src/act/review.rs:137:22, 139:90, 139:104, 140:34, 145:36, 145:43, 148:84, 148:98, 155:67, 156:22, 156:29 (`&base`, `&head`, `&findings_ref`, `&s`) |
| `double_ended_iterator_last` ("called `Iterator::last` on a `DoubleEndedIterator`") | 1 | src/act/attach.rs:75:13, `let r = last.last()?;` over a `filter_map` of `text.lines()` |
| `unused_mut` ("variable does not need to be mutable") | 1 | tests/act_start.rs:1605:9, `let mut job = common::spawn(...)` |

**[inference]** The 53 `needless_borrow` hits come from two functions whose
parameters are already `&str`; they are mechanical (`cargo clippy --fix`
offers all 53). The `attach.rs:75` one walks the whole usage ledger to find
the last matching record where `next_back()` would stop at the first match
from the end; the ledger grows one line per agent step (as-intended §4).

## 4. Dead-code allows

Command: `grep -rn -E 'allow\((dead_code|unused[a-z_]*)' src/` and
`grep -rn -E '#!?\[allow' src/`

- `#[allow(dead_code)]`: **none**.
- `#[allow(unused*)]`: **none**.
- The only `#[allow(...)]` in `src/` is `#[allow(clippy::too_many_arguments)]`
  at src/state.rs:696.
- No `#![allow]` crate-level attributes; `src/lib.rs` is 18 `pub mod` lines.

## 5. `unwrap()` / `expect()` in `src/`

Every `src/*.rs` with a `#[cfg(test)]` has exactly one, placed after all
non-test code (checked: first `cfg(test)` line vs. file length for all 32
files), so "before the first `#[cfg(test)]`" is a sound non-test filter.

Command: `awk '/^#\[cfg\(test\)\]/{t=1} t==0 && /\.(unwrap|expect)\(/' <file>`

| Module | Non-test | All (incl. tests) | What the non-test ones are |
|---|---|---|---|
| src/proc.rs | 5 | 19 | `.expect("piped")` on `child.stdin/stdout.take()` (:38, :277); `wait_with_output().expect(..)` (:44); mutex `lock().expect("no reader panics")` (:84, :94) |
| src/config.rs | 5 | 12 | `as_object_mut().expect("parent checked..")` (:400, :408, :1160); `serde_json::to_string_pretty(..).expect("a Value serializes")` (:702, :1075) |
| src/act/address.rs | 3 | 12 | two fixed-pattern `Regex::new(..).expect(..)` (:216, :218); `to_string_pretty` (:422) |
| src/usage.rs | 2 | 2 | fixed-pattern regexes (:25, :33) |
| src/state.rs | 2 | 5 | fixed-pattern regex helper (:83); `state["stale"].as_array_mut().expect("stale is a list")` (:1615) |
| src/harness.rs | 2 | 11 | `/dev/urandom` read `.expect("/dev/urandom reads")` (:47); fixed-pattern regex (:337) |
| src/git.rs | 2 | 21 | two fixed-pattern regexes with bare `.unwrap()` (:249, :250) |
| src/act/github.rs | 2 | 4 | fixed-pattern regexes (:149, :156) |
| src/act/ci_fix.rs | 2 | 2 | fixed-pattern regexes (:92, :93) |
| src/watch/pass.rs | 1 | 2 | fixed-pattern regex, bare `.unwrap()` (:816) |
| src/time.rs | 1 | 1 | fixed-pattern regex (:10) |
| src/review_local.rs | 1 | 1 | fixed-pattern regex (:76) |
| src/act/step.rs | 1 | 2 | fixed-pattern regex (:166) |
| src/running.rs | 0 | 30 | all 30 are in its test module |
| others (next.rs, main.rs, failed.rs, limit.rs, verify_red.rs, val.rs, json.rs, watch.rs, act/{start,ship,restack,push,follow,commands,attach,answer,hold,resolve,review,describe,bookkeeping,implement}.rs) | 0 | 0-10 | |

Totals: **29 non-test** unwrap/expect sites in `src/`, 180 including tests.

Of the 29: 18 are `Regex::new` on a literal pattern (15 `.expect("a fixed
pattern compiles")`, 3 bare `.unwrap()` at git.rs:249-250 and pass.rs:816);
3 are `serde_json::to_string_pretty` on a `Value`; 4 are `as_object_mut` /
`as_array_mut` after a shape check; 2 are mutex locks; 3 are child-process
pipe handles; 1 is `/dev/urandom`.

User-hit paths: proc.rs, git.rs, state.rs, harness.rs, address.rs,
github.rs, ci_fix.rs, step.rs, review_local.rs, time.rs, usage.rs and
pass.rs are all run by verbs (`sd state`, `sd act *`, `sd watch`);
config.rs by `sd config`. **[inference]** None of the 29 unwraps a value that
comes from user input, a file, git or GitHub: each is either a compile-time
constant, a just-constructed `Value`, or an OS handle sheepdog itself set
up. The `running.rs` count (30) is entirely test code.

## 6. Dependencies

Command: `cargo metadata --format-version 1 | jq ...`

- **Direct: 4** (`libc ^0.2`, `regex ^1`, `serde_json ^1` with
  `preserve_order`, `sha1_smol ^1`), all normal, none optional. No dev- or
  build-dependencies.
- **Total: 21** packages besides `sheepdog` (22 nodes in `resolve`).
  `regex` pulls 4 (aho-corasick, memchr, regex-automata, regex-syntax);
  `serde_json` pulls 13 (serde, serde_core, serde_derive, syn, quote,
  proc-macro2, unicode-ident, indexmap, equivalent, hashbrown, foldhash,
  itoa, zmij); `libc` and `sha1_smol` pull nothing.

Modules using each (`grep -rl '<dep>::' src/`):

| Dep | Modules | Used for |
|---|---|---|
| `serde_json` | 25 of 38 files | the state/config/NEXT `Value` plumbing |
| `regex` | 12 files | fixed patterns (see §5) |
| `libc` | **src/watch/pass.rs only** | `kill(-pid)`, `SIGTERM`/`SIGKILL` (:87-:103, :758), `setsid` (:743), `localtime_r`/`strftime` for the last-pass line (:230-:235) |
| `sha1_smol` | **src/watch/pass.rs only** | per-repo state-dir name, sha1 of the repo path (:247) |

Both single-module deps are in `watch/pass.rs`; `Cargo.toml`'s comments
name the reason for each (local time; shasum-compatible dir name).

## Observations against the yardstick

- as-intended §5 says `next.rs` owns "the ordered step list derived from
  state JSON" and `state.rs` owns O1-O8. The lint data shows each is
  dominated by one function (`steps` 600 lines / complexity 90; `observe`
  448 lines / complexity 76). That is a size fact, not a boundary breach;
  whether it is a finding is for the module-ownership pass.
- Nothing here bears directly on I1-I18; the compiler has no view of the
  design rules. The one lint touching a state record is
  `double_ended_iterator_last` on the usage ledger scan (attach.rs:75).
