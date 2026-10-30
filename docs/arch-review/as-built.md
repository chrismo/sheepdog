# sheepdog: as built, 2026-10-29

Facts gathered at commit 9aa97f0 by six parallel readers, one file each
under `as-built/`. Each file says what it ran. This page is the index and
the headlines; the raw findings are rows 1-96 of `findings.md`. Compare
against `as-intended.md`.

| File | Measured | Headline |
|---|---|---|
| [deps.md](as-built/deps.md) | module graph, SCCs, function sizes, pub-fn consumers | One edge, `config.rs:835 → act::plain_words`, joins act, config, guidance, harness, limit, state and usage into one strongly connected component. `next::steps` is 613 lines (70% of its file); `state::observe` 513. I10 holds; I11 does not (`state.rs` writes `branch.<b>.sd-pr`). |
| [externals.md](as-built/externals.md) | every gh, git, filesystem, harness and process touch point | gh at 11 sites in 7 modules; 4 of 5 GitHub writes are outside `github.rs` (I13 fails). git escapes `git.rs` at 4 sites, each needing an env var the wrapper can't set (I12 fails literally). Several records have 2-3 writer modules (I8 fails). Harness names appear in config, attach and usage (I2 fails). Marker release covers return, exit and HUP/INT/TERM; SIGKILL relies on reconcile. The running marker, branch config layer and stored description live under `$GIT_COMMON_DIR/sheepdog/`, not `$SHEEPDOG_STATE_DIR`. |
| [churn.md](as-built/churn.md) | per-file commit counts, issue clusters, co-change pairs | Post-port the most-edited files are the three help texts (watch 15/39, act 14/39), and 23 of 38 code commits also edit one. The one fix-of-the-fix cluster is #270 (9 commits, each adding a marker or lock interlock to one more verb). Co-change: `next.rs`+`state.rs` 9 of 12, `state.rs`+`pass.rs` 5 of 10, `act/step.rs` with every editing verb. |
| [issues.md](as-built/issues.md) | 85 open issues tagged by module, step, type | `next.rs` 15 and `watch/pass.rs` 13 hold a third of the issues. Eleven next.rs issues are NEXT offering a step wrong for the state; #326 says NEXT's rules and the verbs' gates are written separately and drift. 13 duplicate pairs/groups, largest around `sd-task` (#281 #118 #149 #263) and cost attribution (#150 #276). 9 open design decisions were missing from the yardstick (now added). |
| [tests.md](as-built/tests.md) | 1,516 acceptance tests in 22 files, 270 unit tests, read not run | A title-for-title port of the bats suite. 13 files each write their own fake gh; no recorded real-gh fixture, no check that the fakes match `state.rs`'s QUERY. Watch and next suites run against hand-written state JSON missing keys `state.rs` emits. Byte-exact whole-output asserts dominate `tests/config.rs` (136). Invariants I2, I5, I8, I10-I13 have no test; I4, I14, I16-I18 are well covered. |
| [lints.md](as-built/lints.md) | build warnings, clippy default and pedantic, unwraps, deps | Build clean. 58 default clippy warnings (53 are `needless_borrow` at two sites). Pedantic: `next::steps` cognitive complexity 90, `state::observe` 76; 13 functions over 100 lines; the three `*_run` verb bodies open with the same three lines. 29 non-test unwraps, 18 on literal regexes, none on external data. 4 direct deps, 21 transitive. |

## What the facts say together

Observed, not yet judged (the review passes judge):

1. **The Observe → NEXT → Act chain has two copies of its guards.** NEXT
   decides what to offer; each verb re-decides whether to run. Eleven open
   issues are the two disagreeing, and the co-change of `next.rs` with
   `state.rs` is the cost of keeping them aligned by hand.
2. **The shared-step layer was retrofitted verb by verb.** #270's nine
   commits and `act/step.rs`'s co-change with every editing verb show the
   running marker, attach lock and failure record being added to one verb
   at a time rather than once.
3. **Three invariants the yardstick assumed were never design goals.**
   gh centralization (I13), git centralization (I12) and one-writer-per-record
   (I8) all fail, and nothing in the record ever stated them. They're
   candidates for either adopting or striking, not bugs.
4. **The suite pins the bash era's shape, not GitHub's.** Hand-written
   fixtures per file, no contact with real gh output, and hand-written state
   JSON feeding the watcher and NEXT tests. Strong on behavior, blind to
   drift at both external boundaries.
5. **Two functions are the architecture.** `next::steps` and
   `state::observe` together are over 1,100 lines and hold nearly every
   rule in sections 3 and 4 of the yardstick.
