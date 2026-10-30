# As built: the test suite as a design artifact

Facts gathered 2026-10-29 for the architecture review, by reading `tests/` and
`src/` (no test was run). Yardstick: `docs/arch-review/as-intended.md`. Each
section ends with the commands that produced its numbers; "inference" marks a
conclusion drawn from them.

Totals: `tests/*.rs` + `tests/common/mod.rs` = 25,504 lines, 1,516 `#[test]`
fns; `src/` unit tests = 270 `#[test]` fns in 32 of 39 modules.

## 1. Per test file

| File | Lines | Tests | Lines/test | Preamble (lines before first `#[test]`) |
|---|---|---|---|---|
| tests/state.rs | 4463 | 250 | 17.9 | 446 |
| tests/watch.rs | 3587 | 210 | 17.1 | 293 |
| tests/config.rs | 3023 | 224 | 13.5 | 135 |
| tests/act_start.rs | 1779 | 111 | 16.0 | 260 |
| tests/act_address.rs | 1473 | 91 | 16.2 | 266 |
| tests/next.rs | 1359 | 126 | 10.8 | 135 |
| tests/act_restack.rs | 1346 | 64 | 21.0 | 201 |
| tests/act_ci_fix.rs | 1030 | 52 | 19.8 | 236 |
| tests/harness.rs | 932 | 45 | 20.7 | 193 |
| tests/act_ship.rs | 925 | 42 | 22.0 | 264 |
| tests/act_attach.rs | 896 | 46 | 19.5 | 182 |
| tests/act_commands.rs | 719 | 47 | 15.3 | 63 |
| tests/act_push.rs | 634 | 35 | 18.1 | 100 |
| tests/act_bookkeeping.rs | 620 | 39 | 15.9 | 113 |
| tests/act_review.rs | 524 | 31 | 16.9 | 106 |
| tests/act_github.rs | 522 | 28 | 18.6 | 76 |
| tests/common/mod.rs | 415 | 0 | - | (shared harness) |
| tests/review_local.rs | 388 | 20 | 19.4 | 132 |
| tests/act_follow.rs | 339 | 20 | 16.9 | 53 |
| tests/install.rs | 279 | 16 | 17.4 | 77 |
| tests/cli.rs | 150 | 12 | 12.5 | 34 |
| tests/act.rs | 89 | 6 | 14.8 | 22 |
| tests/links.rs | 12 | 1 | 12.0 | 6 |

Most setup per assertion (lines/test, preamble): `act_ship.rs` (22.0, 264),
`act_restack.rs` (21.0, 201), `harness.rs` (20.7, 193), `act_ci_fix.rs`
(19.8, 236), `act_attach.rs` (19.5, 182). `state.rs` has the largest
preamble (446 lines: fake gh, 30 `impl Sd` helpers, 10 fixture builders).
`next.rs` is the leanest (10.8): pure policy over a jq-edited `READY` JSON,
no process spawned but `jq`.

Inference: the preamble is per-file, not shared. Each act suite defines its
own fake `gh` (section 2) and its own fake `claude`/`pi`; `tests/common`
holds only the temp-dir harness and bats-shaped assertion helpers.

Commands: `for f in tests/*.rs; do wc -l; grep -c '#\[test\]'; grep -n -m1 '#\[test\]'; done`.

## 2. tests/common and the fake gh

`tests/common/mod.rs` (415 lines) models no `gh` at all. It provides the temp
dir (`bin/sd-<verb>` link, `fakebin/` first on PATH, HOME, global git config),
`Sd::fake(name, script)` to drop any bash script on PATH, bats-shaped `Run`
(`output` = stdout+stderr interleaved, `lines()`, `line(n)`), `jq()`,
`assert_has/lacks/match`, a running-marker writer and a lock string. The only
gh-related constant is `GH_FAILS` (mod.rs:18): a gh that logs its args to
stderr and exits 1.

Every suite that needs gh writes its own fake (14 `fake("gh", ...)` sites in
13 files, `grep -c 'fake("gh"' tests/*.rs`). What each models:

| Suite | Fake gh models | Unmodelled calls return |
|---|---|---|
| state.rs:26-83 | `api repos/*/rules/branches/*` and `*/branches/*` from `rules.json`/`branch.json`; GraphQL by `branch=` or `number=` field: serves `$GH_FIXTURES/<branch>.json` reshaped by jq into `data.repository.pullRequest`, injects `rateLimit`; `-F` numeric branch rejected as real gh would; `GH_FAKE_FAIL*` knobs | `api repos/*` other paths: "unexpected api call" exit 2; no `branch=`/`number=`: exit 2; unknown number: real gh's "Could not resolve" text, exit 1; unknown branch: empty `pullRequests.nodes` |
| act_address.rs:20-43 | `pr view` (state from `$FX/pr.state`, default OPEN); GraphQL ops by regex on the query text (`nodes(ids:...)`, `resource(url`); applies `--jq`; logs `{op,args}` to `gh.log` | any other op: `{"data":{}}` piped through `--jq` |
| act_ci_fix.rs:30-41 | `api .../commits/*/status`, `.../check-runs` from `status.json`/`checks.json`; `run view` from `gha.log`; `pr view` | "unexpected call" exit 1 |
| act_ship.rs:19-43 | `pr create` (prints a URL), `pr edit`; GraphQL by number/branch from `$GH/*.json` | unknown branch: empty nodes |
| act_restack.rs:21-36 | `pr edit`; GraphQL by number/branch | unknown branch: empty nodes |
| act_github.rs:224-250 | `api repos/*/issues/*/comments` (returns `{id, created_at}` through `--jq`); GraphQL by number/branch | unknown branch: empty nodes |
| act_follow.rs:18-31 | GraphQL by `number=` with reviews from `$GH/reviews` | empty nodes |
| act_start.rs:26-28, act_attach.rs:26-27 | logs args; always answers "no PR" | - |
| act_bookkeeping.rs:32-, watch.rs:873-881 | one canned PR document, or rate-limit failure | - |

The GraphQL fixtures are hand-written: `tests/state.rs:138-160 pr_fixture`
builds a node from a jq literal with 14 fields and the suite edits it with jq
assignments; `tests/act_follow.rs:24` builds another by hand. Real `gh`'s
query is `src/state.rs:24-53` (`QUERY`) and `:55-62` (`DISCOVER`).

**No check that the fake matches real gh output shape.** `grep -rn -i -E
'recorded|captured from|real gh|schema' tests/` finds no recorded fixture, no
JSON schema and no comparison against `QUERY`'s field list; `grep -n QUERY
tests/state.rs` finds nothing. The fakes serve whatever fields the fixture
literal has; a field `QUERY` asks for but the fixture omits reads as null
through `src/val.rs` (header: "a missing field reads as null").

The same holds one layer up: `tests/watch.rs:76` runs the pass against a fake
`sd-state` (`SHEEPDOG_STATE_PROGRAM`, a debug-build seam, `src/watch/pass.rs:538`)
fed from a hand-written `READY` state (`tests/watch.rs:40-66`), and
`tests/next.rs:11-24` has its own `READY`. Neither `READY` carries the
`verify`, `flags`, `spend`, `question`, `review.findings`, `parent.on_remote`
or `parent.unpushed` keys that `src/state.rs:989-1025` emits (checked with
`grep -c` per key, all 0). `tests/state.rs` does assert on the real
`.next` (57 `.next` references), which is the one place real state meets
real NEXT. The act suites also bypass `sd state`: `SHEEPDOG_STATE_FIXTURES`
(`src/act/mod.rs:231-235`, debug builds only) makes `state_json` read
`<branch>.json` from disk (act_address.rs:110, act_ci_fix.rs:104,
act_github.rs:24).

## 3. Byte-exact vs field assertions

Counted by pattern: "whole" = `assert_eq!` on `r.output`, `sd.out(..)`,
`sd.ok(..)` or `r.lines()`; "field" = `jq(`, `.field(`, `assert_has`,
`assert_match`, `assert_lacks`, `.contains(`. Command:
`grep -c -E 'assert_eq!\(\s*(r\.output|sd\.out|sd\.ok|r\.lines)' tests/*.rs`
and the field patterns likewise. Heuristic, not exact.

| File | whole-output `assert_eq!` | field/substring asserts | total `assert_eq!` |
|---|---|---|---|
| config.rs | 136 | 51 | 442 |
| watch.rs | 30 | 141 | 257 |
| act_bookkeeping.rs | 21 | 24 | 66 |
| act_push.rs | 18 | 17 | 100 |
| act_start.rs | 16 | 107 | 154 |
| act_restack.rs | 12 | 65 | 162 |
| act_github.rs | 11 | 26 | 49 |
| state.rs | 6 (plus 437 `.field()` + 51 `.field_c()` jq reads) | 105 | 538 |
| next.rs | 2 (but 441 `assert_eq!` on `keys()`/`step()`/`lines_for()` strings) | 1 | 441 |
| others | 0-6 each | - | - |

Where byte-exact dominates:

- `tests/config.rs`: 136 whole-output asserts, mostly `sd.out(&["get", key])
  == "value"` (a one-value command, so exact is the natural assertion) plus
  full error lines like `config.rs:2732 "sd config: <path>: profile: no such
  profile gone (defined: billing)"`.
- `tests/act_push.rs` (18 of 35 tests) and `tests/act_bookkeeping.rs`: exact
  verb output lines, e.g. `act_push.rs:149 "pushed feat to origin (1 commit)"`.
- `tests/watch.rs` status/vacuum lines, e.g. `watch.rs:1725 "repo  feat
  unenrolled: merged for 2m (sd act enroll feat to watch it again)"`, with
  two-space column alignment pinned.
- `tests/next.rs`: 441 `assert_eq!` on the NEXT key list, a step's field, or
  the rendered NEXT line (`lines_for`), e.g. `state.rs:4015` pins a full NEXT
  line with glyph, who, say and cmd columns.

`tests/state.rs` is the counter-example: 488 jq field reads against
`--json`, 6 whole-output asserts (error lines). No test asserts the whole
`sd state --json` document byte for byte (`grep -n -E 'byte|whole'
tests/state.rs` finds none); the byte-compatibility claim in
`src/json.rs:1-2` is pinned only by `src/json.rs`'s three unit tests
(section 4).

## 4. Tests that encode a workaround or bash-era behavior

Test names and comments carrying `bash`, `byte for byte`, `super`, `compat`,
`legacy` (`grep -rn -E 'fn [a-z0-9_]*(byte|bash|super|compat|legacy)' src tests`;
`grep -rn 'Was lib/.*bats' src tests`):

| Where | Pins |
|---|---|
| src/json.rs:117 `numbers_print_as_super_prints_them` | super 0.3.0 `-j` number formatting (`1e+21`, `-0`, `0.000001`) |
| src/json.rs:129 `strings_escape_as_super_escapes_them` | super's string escaping (` `, `\u007f` raw) |
| src/json.rs:137 `floats_print_as_super_prints_them_in_sup_and_f_strings` | SUP float text (`5.`, `9223372036854700032.`, `1e-07`) |
| src/usage.rs:222 `record_writes_what_the_bash_usage_q_wrote` | usage.sup line format, SUP syntax, from retired `ci/fixtures/usage.json` |
| src/state.rs:1900 `spend_sums_floats_as_super_did_and_prints_a_whole_sum_without_a_point` | spend rendering |
| src/state.rs:1906 `is_test_matches_names_paths_and_directories_as_bash_globs` | `[[ $p == $g ]]` glob semantics |
| src/watch.rs:262 `plist_is_byte_for_byte_what_the_bash_wrote` | launchd plist text |
| src/watch/pass.rs:1285 `class_record_is_in_the_order_the_bash_wrote_it` | per-branch watcher record key order |
| src/act/github.rs:254 `ack_items_what_is_as_super_printed_it` | ack output, "retired" ACK_Q query |
| src/act/mod.rs:467 `plural_is_bashs` | pluralization |
| src/proc.rs:406 `with_timeout_kills_the_whole_group_and_says_so_as_the_bash_did` | timeout message |
| src/act/commands.rs:289,300 | bats "Executed N instead of expected M" TAP parsing (a real bats behavior, not a port artifact) |

Header-level: `src/json.rs:1-2`, `src/state.rs:2`, `src/usage.rs:2` and
`src/val.rs:1-3` state the byte-for-byte / SuperDB-shaped contract as the
module's reason to exist. 16 unit tests carry `// Was lib/sheepdog.bats:
"<title>"` comments (harness.rs 3, guidance.rs 4, running.rs 2, state.rs 2,
hold.rs 4, config.rs 1), and every `tests/*.rs` header says "each test keeps
the bats test's title above it"; `tests/common/mod.rs:46-64` reproduces bats'
`run`/`lines`/`sed -n Np` semantics. `tests/next.rs:66` and `:130` describe
helpers as `bash's "${a[@]}"` and `as bats's $output has them`.

Inference: the acceptance suite is a port of the bats suite kept
title-for-title; the bash-era pins proper are the 11 unit tests above, nearly
all in `json.rs`, `usage.rs`, `watch.rs` and `pass.rs`. No acceptance test
pins `sd state --json` as a whole document.

## 5. Coverage gaps

**src modules with no `#[cfg(test)]`** (`grep -c '#\[test\]' src/**/*.rs`):
`next.rs` (876 lines; covered by tests/next.rs calling `sheepdog::next::steps`
directly), `act/follow.rs` (tests/act_follow.rs), `review_local.rs`
(tests/review_local.rs), `verify_red.rs` (45 lines; exercised through
act_commands.rs/act_ship.rs/state.rs/next.rs, 36 mentions), `val.rs` (38
lines, none), `main.rs` (tests/cli.rs), `lib.rs`. Thinnest unit coverage
among modules with tests: `act/start.rs` (1 test / 249 lines), `act/push.rs`
(2 / 169), `limit.rs` (2 / 81), `failed.rs` (2 / 75).

**sd act verbs vs tests/<verb>.rs.** The 22 dispatched verbs
(`src/act/mod.rs:327-348`) all have a home, but not by one-file-per-verb:
`act_bookkeeping.rs` = enroll/unenroll/forget/unregister; `act_commands.rs` =
verify/autofix; `act_github.rs` = ack/trigger; `act_follow.rs` = follow/fetch;
`act_restack.rs` = restack/retarget/resolve; `act_ship.rs` = ship/describe;
`act_start.rs` = start/implement; `answer` has no file: its 8 tests live in
act_address.rs (4) and act_ci_fix.rs (4). `autofix` has 6 tests by name across
three files. `tests/act.rs:49-72` checks every dispatched verb has a usage line
by regex over `src/act/mod.rs`. **No test asserts which verbs `--help` marks
"Spends tokens"** (`grep -rn 'Spends tokens' tests/` is empty).

**Invariants (as-intended section 7)** against test names (grep of `^fn` in
tests and src):

| Inv. | A test that would catch a violation | Observed |
|---|---|---|
| I1 agent can't commit/push/gh | `tests/act_start.rs:868 implement_no_free_shell_..._commit_push_and_gh_denied` asserts the deny list; `act_address.rs:583-585`, `review_local.rs:174` likewise | covers the claude deny-list wording only (a "cheap extra" per rule 4); nothing tests that a stray agent commit/push is reconciled |
| I2 harness names only in harness.rs | none | `grep -rn '"claude"\|"pi"' src` outside harness.rs hits config.rs (10), usage.rs (6), limit.rs tests; no test guards it |
| I3 rerun same HEAD, same effect | `act_ship.rs:881 ship_twice_...`, `act_start.rs:424 start_twice_...`, `act_start.rs:850 implement_..._a_second_run_forks_it`, `act_ci_fix.rs:647 flaky_..._same_red_sha_is_refused_next_time`, `act_address.rs:689`/`act_start.rs:1475` rerun-refuses-at-zero-tokens | per-verb, for start/ship/implement/ci-fix/address; no rerun test found by name for push, restack, retarget, trigger, enroll, verify |
| I4 no "sheepdog was last writer" assumption | `act_push.rs:196-407` (12 tests on someone else's commits, resets, fast-forwards), `state.rs` 33 moved/stale/by-hand names, `watch.rs` 21 | strongest-covered invariant |
| I5 token verbs marked, no other verb runs a harness | none | no test reads the "Spends tokens" marks; no test asserts a mechanical verb never runs `claude`/`pi` |
| I6 records keyed on SHA / validated vs HEAD | `act_ci_fix.rs:647` (red SHA), `act_review.rs:291`, `act_commands.rs` verify-red (15 mentions), `state.rs:3915` dead marker cleared | per-record |
| I7 no timestamp decides staleness | `act_review.rs:347 accept_records_the_bot_trigger_it_followed_so_next_can_order_them_without_comparing_clocks_181` | one test, one record; `watch.rs:1410-1573` "clock" tests are about vacuum durations, not staleness |
| I8 one writer per record | none | writers of `refs/sheepdog/*` appear in 13 src files (`grep refs/sheepdog src`), no test |
| I9 forget/vacuum leave nothing orphaned | `act_bookkeeping.rs:274-564` (18 forget/unenroll tests), `watch.rs:405 a_pass_sweeps_refs_sheepdog_b_whose_branch_no_longer_exists`, `:420` | covers refs, config file, description; no test names the state-dir running marker or watcher per-branch file under forget |
| I10 next.rs touches no git/gh/fs | none (structural); `src/next.rs:70-71` imports only `val` and `serde_json` | holds by reading; `tests/next.rs:2` header states it |
| I11 state.rs writes nothing | none | `grep` of write-ish calls in src/state.rs finds only an error-message string (`:1315`) |
| I12 all git via git.rs | none | `Command::new("git")` also in `act/restack.rs:300`, `act/start.rs:118`, `act/resolve.rs:137,232`, `guidance.rs:124` |
| I13 all gh via one place | none | `proc::run("gh", ...)` in state.rs, ci_fix.rs, restack.rs, address.rs, hold.rs, ship.rs, github.rs, watch/pass.rs (8 modules) |
| I14 marker taken before harness, released on every exit | `harness.rs:576,599,610,634,658,674`; `attach.rs:361,371,396`; `watch.rs:3156,3403` | signal paths tested for harness run and attach |
| I15 attach lock rechecked before commit and push | `act_address.rs:1058,1076,1093,1105`; `act_ci_fix.rs:454-546`; `act_restack.rs:1148,1167` | covered for address, ci-fix, resolve; not by name for implement or review |
| I16 pass never blocks on one branch | `watch.rs:739 a_hung_sd_state_times_out_instead_of_stalling_the_pass`, `:3156`, `:3403` | covered |
| I17 glyph from NEXT | `watch.rs:607 glyph_follows_sd_states_next_steps_not_its_raw_fields` | covered |
| I18 concurrency cap and limit pause | `watch.rs:3573 ..._counts_toward_the_cap_269`, `:3535 a_usage_limit_holds_agent_steps_...`, `:3420` | covered |

## 6. Ignored or skipped tests

`#[ignore]`: none (`grep -rn '#\[ignore' tests src`). `#[cfg(target_os)]`
on tests: none. Early-return skips: 17 call sites of `if no_plutil() {
return; }` in `tests/watch.rs` (`fn no_plutil() -> bool { !cfg!(target_os =
"macos") }`, watch.rs:2553), with the reason in the comment at :2551 ("where
bats skipped them: cargo test has no skip") and in AGENTS.md: the launchd
plist tests pass on Linux CI without checking anything. The 16 tests:
`install_writes_a_valid_plist_and_bootstraps_it`,
`install_retries_bootstrap_while_the_old_job_is_unloading`,
`install_bootstrap_retry_is_bounded_and_keeps_the_error`,
`install_does_not_retry_other_bootstrap_errors`,
`install_the_plist_is_readable_by_its_owner_only_whatever_the_umask`,
`install_the_job_abandons_its_process_group_..._12`, `install_the_job_runs_sd_watch`,
`install_with_no_sd_in_local_bin_the_job_runs_this_sd_itself`,
`install_the_jobs_path_is_the_one_install_ran_with_each_dir_once`,
`install_the_default_local_bin_is_not_added_to_the_jobs_path`,
`install_the_job_runs_the_sd_in_local_bin_which_leads_its_path_once`,
`install_a_relative_local_bin_is_made_absolute_...`,
`uninstall_boots_out_and_removes_the_plist`,
`install_a_non_numeric_interval_is_refused_and_the_old_job_is_kept`,
`install_paths_are_xml_escaped`, `config_install_reads_interval_from_the_file`
(watch.rs:2598-2907). No other skip pattern found (`grep -rn -E 'skipping|eprintln!\("skip'`).
