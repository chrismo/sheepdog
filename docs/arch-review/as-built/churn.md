# As built: history churn

Facts from `git log` on 2026-10-29 at `9aa97f0`, for the architecture review
(`docs/arch-review/as-intended.md`). Nothing was checked out; every number
below comes from the command shown with it. Conclusions are marked
**[inference]**.

## 0. The windows

| Window | Commits | How counted |
|---|---|---|
| All | 217 | `git rev-list --count HEAD` (2026-10-11 `23caa7e` .. 2026-10-29 `9aa97f0`) |
| Pre-port (author date < 2026-10-22) | 146 | 217 - 71 |
| Port day (author date = 2026-10-22) | 32 | `git log --format=%ad --date=short \| grep -c 2026-10-22` |
| Post-port (committer date after the port day) | 39 | `git rev-list --count HEAD --since=2026-10-22` |

Caveat on the task's "since 2026-10-22": git fills a bare `--since` date
with the current time of day, so as run here the window starts between
`84bc6f6` (2026-10-22 17:18, the last port-day commit) and `0678b1a`
(2026-10-23 07:31). It therefore **excludes** the 32 port-day commits. That
is the split the review wants: "post-port" below means 2026-10-23 onward,
39 commits; "port day" is the 32 commits of 2026-10-22 (the #201-#204
slices). Post-port commits per day: 10-23: 15 · 10-25: 3 · 10-26: 4 ·
10-27: 4 · 10-28: 6 · 10-29: 7.

## 1. Per-file commit counts, src/ and tests/

`git log --format= --name-only -- src tests | grep -v '^$' | sort | uniq -c | sort -rn | head -25`,
and the same with `--since=2026-10-22`.

| All time (217) | | Post-port (39) | |
|---|---|---|---|
| 23 | src/act/mod.rs | 15 | src/watch_help.txt |
| 19 | src/watch_help.txt | 14 | src/act_help.txt |
| 19 | src/state.rs | 10 | src/watch/pass.rs |
| 19 | src/config.rs | 10 | src/config.rs |
| 16 | tests/state.rs | 9 | tests/watch.rs |
| 16 | src/harness.rs | 9 | tests/state.rs |
| 16 | src/act_help.txt | 9 | tests/act_start.rs |
| 14 | tests/config.rs | 9 | src/state.rs |
| 13 | src/watch/pass.rs | 9 | src/state_help.txt |
| 13 | src/state_help.txt | 8 | tests/next.rs |
| 12 | tests/watch.rs | 8 | src/next.rs |
| 12 | tests/act_start.rs | 8 | src/harness.rs |
| 12 | src/next.rs | 7 | tests/config.rs |
| 11 | src/running.rs | 7 | src/act/mod.rs |
| 11 | src/act/step.rs | 6 | tests/harness.rs |
| 10 | src/proc.rs | 6 | src/running.rs |
| 10 | src/lib.rs | 5 | tests/act_ci_fix.rs |
| 10 | src/git.rs | 5 | src/act/step.rs |
| 9 | tests/next.rs | 5 | src/act/attach.rs |
| 9 | src/watch.rs | 4 | tests/act_restack.rs |
| 9 | src/act/start.rs | 4 | tests/act_commands.rs |
| 9 | src/act/commands.rs | 4 | tests/act_address.rs |
| 8 | tests/harness.rs | 4 | src/failed.rs |
| 8 | tests/common/mod.rs | 4 | src/act/start.rs |
| 8 | src/act/review.rs | 4 | src/act/review.rs |

Observed: `src/` has only existed since 2026-10-21 (`8e8e20a`, #196), so
"all time" for Rust files is 2026-10-21..29; the top 25 by author date
>= 2026-10-22 (port day included) is led by watch_help.txt 19, state.rs 18,
tests/state.rs 16, act_help.txt 16, act/mod.rs 15. The three `*_help.txt`
files are `include_str!`'d help text (`src/state.rs:20`,
`src/watch.rs:1`, `src/review_local.rs:21`); they are 176, 345 and 175
lines (`wc -l`). Of the 38 post-port commits touching `src/` or `tests/`,
23 also edit at least one `*_help.txt`.

**[inference]** The most-edited files in the Rust era are prose, not code:
help text is where the vocabulary of NEXT steps, glyphs and notifications
is written down, and three files carry overlapping parts of it (see §5).

## 2. Files touched in more than a third of post-port commits

Threshold 39/3 = 13. `git log --since=2026-10-22 --format= --name-only | sort | uniq -c | sort -rn | awk '$1 > 13'`

| Commits | File |
|---|---|
| 15 | src/watch_help.txt |
| 14 | src/act_help.txt |

No `.rs` file crosses the threshold; the top code files are
`src/watch/pass.rs` and `src/config.rs` at 10 each (26%).

## 3. Issue numbers in 3+ commit subjects

`git log --format=%s | grep -oE '#[0-9]+' | sort | uniq -c | awk '$1>=3'`,
subjects via a perl pass over `git log --format='%h %ad %s'`. PR numbers
(the trailing `(#N)` from squash merges) appear once each and so never
reach 3. Titles from `gh issue view`.

| Issue | Commits | State | Title (gh) |
|---|---|---|---|
| #203 | 13 | closed | Port the agent steps to Rust |
| #270 | 9 | open | The loop, slice 4: interlocks for a merge, a takeover or a mechanical step while a step runs |
| #202 | 6 | closed | Port sd act's zero-token verbs to Rust |
| #204 | 5 | closed | Finish the Rust port: sd is the binary, and bash, super and bats retire |
| #283 | 4 | open | Config architecture: every setting in JSON, one schema |
| #201 | 3 | closed | Port sd watch to Rust, ahead of the loop (#12) |

#201-#204 (27 commits, all dated 2026-10-22) are the port's tracking issues;
each commit ports or moves one verb or test suite ("Port sd act X to Rust",
"Move Y bats suite to Rust integration tests"). **[inference]** Planned
slices, not fixes of fixes.

#270's nine commits, 2026-10-27..29, each add one interlock:

```
f8d3021 10-29 Mechanical verbs run by hand hold the running marker while they run
9dc911b 10-29 sd act review holds the running marker over its verify and reviewer
a164882 10-28 sd act attach resumes a failed or killed editing step's session when it left edits
3e3d48c 10-28 A step sd ended because sd got a signal isn't counted in the failure record
738cc6c 10-28 A pass ends a launched step whose PR merged or closed, without counting it as a failure
89e37c5 10-28 An agent step checks the attach lock again before it commits and pushes
c45d13c 10-27 restack, push, ship, autofix, verify and fetch refuse while another process's step holds the running marker
a6f44d6 10-27 address and ci-fix don't push onto a PR that merged mid-step; forget --remove waits
cb194ac 10-27 sd act attach refuses while an agent step runs, and keeps a session a step moved
```

Observed: four of the nine (`f8d3021`, `9dc911b`, `c45d13c`, `cb194ac`)
each widen the set of verbs that take or respect the running marker; two
(`3e3d48c`, `738cc6c`) each correct what the failure record counts. These
are the I14/I15 area. **[inference]** The marker and failure-record rules
were introduced per verb rather than once at the shared layer, so each
verb (and the pass) needed its own commit; this is the one "fix of the fix"
cluster the subjects show.

#283's four commits (10-25..26): autoRun replaces dispatch (`5a6a644`),
branch-file cleanup (`ae6a79d`), typed `config set` (`dbd5c3d`), renamed-key
migration on read (`1bd1c97`). as-intended §8 lists #283 as "in flux".

## 4. Subjects with again / revert / really / actually / still / also / follow-up

`git log --format='%h %ad %s' | grep -iE '\b(again|revert|really|actually|still|also|follow-up|followup)\b'`

| Commit | Subject |
|---|---|
| 89e37c5 2026-10-28 | An agent step checks the attach lock **again** before it commits and pushes (#270) (#312) |
| f0c2dce 2026-10-16 | sd act restack: keep sd-parent while the PR **still** targets the merged parent (#91) |

Two matches in 217 subjects; no `revert`. Observed: in both, the word
describes the behaviour being built (a second lock check; a PR that still
targets its parent), not a redo of an earlier commit.

## 5. Co-change pairs of src/ files (same commit, 5+ times)

Computed from `git log --format='%x00%H' --name-only -- src` with an awk
pass over each commit's `src/` names. 65 commits touch `src/` all time, 37
post-port; 44 files under `src/`. Per-file denominators from §1.

| Pairs | Files | Denominators |
|---|---|---|
| 11 | state_help.txt + state.rs | state.rs 19 |
| 10 | watch_help.txt + watch/pass.rs | pass.rs 13 |
| 9 | state.rs + watch_help.txt | state.rs 19 |
| 9 | next.rs + state.rs | next.rs 12, state.rs 19 |
| 8 | state_help.txt + watch_help.txt | |
| 7 | act/mod.rs + act/step.rs | mod.rs 23, step.rs 11 |
| 7 | act_help.txt + watch_help.txt | |
| 6 | state.rs + watch/pass.rs | pass.rs 13 |
| 6 | next.rs + watch_help.txt | next.rs 12 |
| 6 | next.rs + state_help.txt | next.rs 12 |
| 6 | lib.rs + state.rs; lib.rs + main.rs | lib.rs 10 |
| 6 | act/mod.rs + proc.rs; act/mod.rs + git.rs | proc.rs 10, git.rs 10 |
| 6 | act/implement.rs + act/step.rs | implement.rs 7, step.rs 11 |
| 6 | act_help.txt + each of watch/pass.rs, state.rs, state_help.txt, harness.rs | |
| 5 | state_help.txt + watch/pass.rs; proc.rs + running.rs; harness.rs + proc.rs; git.rs + state.rs | running.rs 11 |
| 5 | act/step.rs + each of state.rs, harness.rs, act/resolve.rs, act/hold.rs, act/commands.rs | resolve.rs 5, hold.rs 7, commands.rs 9 |
| 5 | act/mod.rs + each of state.rs, running.rs, harness.rs, act/hold.rs, act/commands.rs | |
| 5 | act_help.txt + each of next.rs, act/step.rs, act/resolve.rs | |

Post-port only (3+): watch_help.txt + pass.rs 9 · state_help.txt + state.rs 8
· state.rs + watch_help.txt 7 · state_help.txt + watch_help.txt 7 ·
act_help.txt + watch_help.txt 7 · act_help.txt + state.rs 6 · act_help.txt
+ state_help.txt 6 · state.rs + pass.rs 5 · state_help.txt + pass.rs 5 ·
next.rs + each of watch_help.txt, state.rs, state_help.txt 5 · act_help.txt
+ each of pass.rs, next.rs 5 · harness.rs + watch_help.txt 4 · resolve.rs +
step.rs 4 · act_help.txt + each of harness.rs, step.rs, resolve.rs 4 ·
next.rs + pass.rs 3.

Observed, code-to-code:

- `next.rs` changed with `state.rs` in 9 of its 12 commits (5 of 8
  post-port). as-intended §3 says NEXT is derived from the state JSON, so a
  new state field needs a NEXT rule; **[inference]** this pair is the
  design, not drift.
- `state.rs` changed with `watch/pass.rs` in 6 commits (5 of pass.rs's 10
  post-port). The pass is meant to read `sd state --json` as a child
  (as-intended §3). Which of the six changed a shared field vs. a shared
  type was not checked here.
- `act/step.rs` (the shared agent-step layer) co-changes with `act/mod.rs`
  7, `implement.rs` 6 (of 7), `resolve.rs` 5 (of 5), `hold.rs` 5 (of 7),
  `commands.rs` 5 (of 9). as-intended §8 names the four editing verbs'
  duplication as a scheduled consolidation (#133).
- `lib.rs` co-changes with `main.rs` and `state.rs` 6 each: module
  registration. `act/mod.rs` with `proc.rs` and `git.rs` 6 each.

Observed, code-to-prose: the nine commits touching both `state.rs` and
`watch_help.txt` (`git log -- src/state.rs src/watch_help.txt`, filtered to
commits with both) are `f7534c4 f532cd2 d8bff7c 4cc9dbb 110db8c 6e9c938
5dbc792 70bfdcd 97d65ba`; `watch_help.txt:40-85` is the list of NEXT steps
notified and not notified. `state_help.txt` and `watch_help.txt` co-change
8 times. **[inference]** The NEXT-step vocabulary is documented in three
help files and each new state or step edits two or three of them.

## 6. Five largest post-port commits by lines changed

`git log --since=2026-10-22 --format='%x00%h %ad %s' --shortstat`, summed +/-.

| Lines | Files | +/- | Commit |
|---|---|---|---|
| 1012 | 13 | +997/-15 | d8bff7c 2026-10-23 Reviewer mode: sd act follow, fetch, and a followed PR's NEXT (#250) (#255) |
| 608 | 2 | +558/-50 | 8ed3378 2026-10-26 Config profiles: a named set of overrides a branch selects (#74) (#291) |
| 537 | 15 | +481/-56 | d583826 2026-10-25 sd watch runs agent steps, capped and paused on a usage limit (#269) (#282) |
| 535 | 6 | +482/-53 | 8a2e315 2026-10-23 A pass launches its step detached and reconciles the lease each pass (#268) (#277) |
| 409 | 8 | +381/-28 | f532cd2 2026-10-23 sd watch dispatches a branch's first mechanical step (#267) (#274) |

Next three: `89e37c5` 400/15 (#270 attach-lock recheck), `f7534c4` 385/16
(#325 verify-red), `f13b975` 364/15 (#69 fork per step). Observed: three of
the top five are the watcher's autoRun series (#267 #268 #269) and the
largest is reviewer mode (#250); the second-largest puts 558 lines into two files (`src/config.rs`,
`tests/config.rs`), the #283 area.
