# As built: external-system touch points

Facts gathered 2026-10-29 at commit 9aa97f0, read-only, by grepping `src/`
(`grep -rn '"gh"' src/`, `grep -rn 'Command::new\|proc::' src/`,
`grep -rn '"git"' src/`, `grep -rn 'update-ref\|"config", &format' src/`,
`grep -rn 'fs::write\|fs::remove\|OpenOptions' src/`,
`grep -rn 'running::\|harness::' src/`) and reading each hit in context.
Line numbers are from that commit. Test-module hits are marked (test).
Conclusions are marked **[inference]**; everything else is what the code says.

## 1. `gh` invocations (I13)

All run through `proc::run` or `proc::timeout` (src/proc.rs), never a shared
`gh` wrapper. Eleven call sites in seven modules. Each builds its own argv.

| file:line | what | r/w | wrapper |
|---|---|---|---|
| src/state.rs:224 | `gh api graphql -F owner -F repo …` (PR lookup, discovery: O1/O5/O6/O7) | read | `Gh::call` (state.rs only) |
| src/state.rs:1082 | `gh api <path>` (required checks: rulesets, branch protection) | read | local closure |
| src/act/ci_fix.rs:54 | `gh api repos/{owner}/{repo}/commits/<sha>/status`, `…/check-runs` (via `gh_api`, lines 95, 106) | read | `gh_api` (ci_fix.rs only) |
| src/act/ci_fix.rs:116 | `gh run view <run> --job <job> --log-failed` | read | none |
| src/act/hold.rs:165 | `gh pr view <pr> --json state --jq .state` (merged/closed gate) | read | none |
| src/act/address.rs:124 | `gh api graphql -f …` (owed-thread text, reply/resolve mutations) | read + **write** | `gql` (address.rs only) |
| src/act/github.rs:191 | `gh api repos/<repo>/issues/<n>/comments --hostname <h> -f body=…` (A5 trigger comment) | **write** | none |
| src/act/restack.rs:356 | `gh pr edit <n> --base <default>` (A4 retarget) | **write** | none |
| src/act/ship.rs:160 | `gh pr create --draft --base … --head … --title … --body …` | **write** | none |
| src/act/ship.rs:175 | `gh pr edit <n> --title --body [--base]` | **write** | none |
| src/watch/pass.rs:528 | `gh api rate_limit --jq …` | read | none |
| src/watch/pass.rs:818 | `gh api repos/<o>/<r>/pulls/<n>` (what became of a gone branch's PR) | read | `Command::new("gh")` directly, with `GH_REPO` env |

Repo targeting: `sd act` sets `GH_REPO` once in `act::run` (src/act/mod.rs:301-303,
from `git::gh_repo`); `pass.rs:820` sets it per call; `state.rs` passes
`-F owner={owner} -F repo={repo}` and relies on `gh`'s own default-repo
resolution in `self.dir`. **[inference]** I13 ("one place") does not hold:
there are three private wrappers (`Gh::call`, `gh_api`, `gql`) and eight bare
calls. as-intended.md §5 says "only `github.rs` and the Observe layer run
`gh`" **[assumed]**; as built, `ci_fix.rs`, `hold.rs`, `address.rs`,
`restack.rs`, `ship.rs` and `pass.rs` also do, and four of the five GitHub
*writes* are outside `github.rs`.

## 2. `git` invocations outside `src/git.rs` (I12)

`git.rs` exports `git`, `git_input`, `git_timed`, `git_out`, `git_ok`, `rev`,
`show_ref`, `pin_write`, etc. (src/git.rs:12-33, 113). Every module that
needs git imports these (`grep -rn 'git::' src/` hits 29 files). Direct
`Command::new("git")` outside git.rs:

| file:line | what | why not git.rs |
|---|---|---|
| src/act/start.rs:118 | `git_on_stderr`: git with its stderr passed through (`on_stderr`) | needs stderr inherited, which `proc::run` pipes |
| src/act/restack.rs:300 | `git -C <wt> reset --quiet --hard <new>` with `GIT_REFLOG_ACTION=…` | needs an env var; `git::git` takes none |
| src/act/resolve.rs:137 | `git -C <wt> read-tree/add -A/write-tree` with `GIT_INDEX_FILE=<scratch>` | needs an env var |
| src/act/resolve.rs:232 | `git -C <wt> rebase --continue` with `GIT_EDITOR=true` | needs an env var |
| src/guidance.rs:124 (test) | test fixture's own git runner | under `#[cfg(test)]` at line 84 |
| src/config.rs:1461 (test) | test fixture | under `#[cfg(test)]` at line 1307 |

Also: `git.rs` itself calls `Command::new("git")` at 25 (env `GIT_TERMINAL_PROMPT`)
and 184-198 (`--no-optional-locks`, stderr nulled). **[inference]** The four
production escapes share one cause: `git::git` has no way to set an
environment variable. I12 holds for argv/dir but not for env-carrying calls.

## 3. Writes outside the process: state records and their writers/readers (I8, I9)

### 3a. Where state actually lives

as-intended.md §4 puts the running marker under `$SHEEPDOG_STATE_DIR`. As
built it is under the repo's git common dir:

| path | contents | writer | reader | removed by |
|---|---|---|---|---|
| `$GIT_COMMON_DIR/sheepdog/<b>.running` | running marker / lease (running.rs:14-17) | running.rs:241 (`marked`) | state.rs:1503 via running::state; pass.rs:99-112, 666-692, 754, 782 | running.rs:154 (release), running.rs:52 (sd state drops a dead no-deadline marker), pass.rs:112 (watcher) |
| `$GIT_COMMON_DIR/sheepdog/<b>.outcome` | how a launched step ended | pass.rs:1051 (`sd watch step`) | pass.rs:785 | pass.rs:748, 786 |
| `$GIT_COMMON_DIR/sheepdog/<b>.json` | per-branch config layer (config.rs:463-465) | config.rs:1068 `write_file` (sd config set) | config.rs layering | bookkeeping.rs:125 (unenroll, branch gone), config.rs:492 `sweep_branch_files` (pass.rs:915) |
| `$GIT_COMMON_DIR/sheepdog/<b>.describe.json` | stored PR description (the "unknown location" in §4) | describe.rs:110 | ship.rs:140 | bookkeeping.rs:125, 207; config.rs:492 |
| `$GIT_COMMON_DIR/sheepdog.json` | repo config layer (config.rs:447) | config.rs:1068 | config.rs | nothing |
| `~/.config/sheepdog/config.json` (`$SHEEPDOG_CONFIG`) | global config | config.rs:1068, 524 (rename-on-read rewrites the file), 1162 (init) | config.rs | nothing |
| `$SHEEPDOG_STATE_DIR/usage.sup` | usage ledger, append-only | usage.rs:207-212 (from harness.rs:230) | state.rs:106, harness.rs:204 | nothing |
| `$SHEEPDOG_STATE_DIR/limit` | usage-limit pause | limit.rs:31 (from harness.rs:252, review.rs:129) | limit.rs:42 (pass.rs:725, 1045; review.rs:128) | limit.rs:47 (on read, once past reset) |
| `$SHEEPDOG_STATE_DIR/glyphs` | glyph index | pass.rs:996 (`write_index`), 1114 | watch.rs:62, pass.rs:972, 1111 | nothing |
| `$SHEEPDOG_STATE_DIR/last-pass` | pass summary | pass.rs:990 | watch.rs:230 | nothing |
| `$SHEEPDOG_STATE_DIR/steps.log` | launched steps' stdout/stderr | pass.rs:734 (append) | none in src | nothing |
| `$SHEEPDOG_STATE_DIR/branches/<repo>-<sha1[..8]>/<b>.json` | last pass's state per branch (N3) | pass.rs:372 | pass.rs:365, 876, 1165 | pass.rs:887 (`report_gone`) |
| `$STATUS_DIR/<worktree>.pr` (default `~/.claude/tab-status`) | tab glyph | pass.rs:470 | external (tab-status) | pass.rs:496, 985 |
| `~/Library/LaunchAgents/com.sheepdog.watch.plist` | launchd job | watch.rs:169-178 | launchd | watch.rs:207 |
| global git config `sheepdog.repo` | registered repos | bookkeeping.rs:61 (enroll), 239 (unregister) | pass.rs:277 | bookkeeping.rs:239 |

### 3b. `refs/sheepdog/<b>/*`

| ref | writers (module:line) | readers | deleted by |
|---|---|---|---|
| `base` | start.rs:221, bookkeeping.rs:115, restack.rs:257 | state.rs:1198, 1240 | forget (bookkeeping.rs:204-205), vacuum `sweep_pins` (pass.rs:311-323) |
| `local`, `local-base` | review.rs:145 via git::pin_write | state.rs:1047-1058 | same |
| `bot`, `bot-base` | github.rs:201 via git::pin_write | state.rs | same |
| `accepted`, `accepted-base` | review.rs:54 via git::pin_write | state.rs | same |
| `findings` | review.rs:158 (round record), **implement.rs:315** (answers) | state.rs:1423 | review.rs:62, 146; forget; sweep |
| `question` | step.rs:239 | state.rs:152, answer.rs:44 | step.rs:295, **github.rs:76** (ack --question); forget; sweep |
| `failed` | failed.rs:46 (called from harness.rs:277, pass.rs:802, 1042) | state.rs:173 | failed.rs:52 (harness.rs:267); forget; sweep; NEXT tells chrismo to `git update-ref -d` (next.rs:237) |
| `verify-red` | verify_red.rs:26 (from commands.rs:177) | state.rs:187, commands.rs:168 | verify_red.rs:35 (commands.rs:167); forget; sweep |
| `pr-head`, `pr-fetched` | follow.rs:120, 102 | state.rs (followed PR), follow.rs:98 | follow.rs:105, 115; forget; sweep |

`sweep_pins` deletes every `refs/sheepdog/<b>/*` whose branch is gone
(pass.rs:311-323), so refs are not orphaned by branch deletion once a pass
runs. `forget` deletes all of a branch's refs (bookkeeping.rs:204-205).

### 3c. `branch.<b>.sd-*` git config

| key | writers | readers | cleared by |
|---|---|---|---|
| `sheepdog` (enrolled) | bookkeeping.rs:57 | pass.rs:290, 873, 1081; state.rs:1572 | bookkeeping.rs:122; **pass.rs:605** (vacuum unenroll), **pass.rs:923** (no local branch) |
| `sd-task` | start.rs:205 | state.rs:1516; review.rs:118; ci_fix.rs:188; describe.rs:72 | `git branch -D` only |
| `sd-parent` | start.rs:208, bookkeeping.rs:108, restack.rs:133 (`parent_to_default`) | state.rs:1155, git.rs:344 | branch -D |
| `sd-pr` | follow.rs:66, **state.rs:298 (unset), state.rs:304 (set)** | state.rs:269, 322 | branch -D |
| `sd-mode` | follow.rs:66 | state.rs:1139 | branch -D |
| `sd-session`, `sd-harness` | step.rs:140-141 (`step_store`); start.rs:214 (`sd-harness`) | step.rs:63-66; attach.rs:116-117; state.rs:1573-1575 | branch -D |
| `sd-lock` | attach.rs:134, 144-145, 181; **mod.rs:178** (`lock_sweep` unsets a stale one) | push.rs:10; mod.rs `attached_check`; address.rs:90; bookkeeping.rs:165; state.rs:1576 | attach.rs:181, mod.rs:178 |
| `sd-handled` | address.rs:329, github.rs:99 | state.rs:1504 | branch -D |
| `sd-ci-fix` | ci_fix.rs:39 | ci_fix.rs:174 | branch -D |
| `sd-bot-triggered` | github.rs:202 | state.rs:1122; review.rs:56 | branch -D |
| `sd-accepted-after` | review.rs:57, 59 | state.rs:1123 | review.rs:59 |
| `sd-held` | hold.rs:125; hold.rs:89 and push.rs:21 (unset) | state.rs:1376; hold.rs | push.rs:21, hold.rs:89 |

**[inference]** I8 (one writer module per record) holds for `local`/`bot`/`accepted`
pins (all via `git::pin_write`), `failed`, `verify-red`, `limit`, `usage.sup`,
`sd-held`, `sd-ci-fix`, `sd-bot-triggered`. It does not hold for: `base` (3
modules), `findings` (review.rs + implement.rs), `question` (step.rs +
github.rs delete), `sd-parent` (3), `sd-pr` (follow.rs + state.rs), `sd-lock`
(attach.rs + mod.rs), `sd-handled` (address.rs + github.rs), `sheepdog`
(bookkeeping.rs + pass.rs), `sd-harness` (step.rs + start.rs). Of the "one
state record each" modules named in §5, none has a header naming its writer
as the sole one; `running.rs`'s marker is also removed by `pass.rs:112`.

**[inference]** I9: `forget` removes config (`sheepdog` only; the rest goes
with `branch -D` under `--remove`), all refs, the describe file and the
branch config file (via unenroll when the branch is gone). It does not touch
`$SHEEPDOG_STATE_DIR/branches/<repo>/<b>.json` (the watcher's `report_gone`
removes that on the next pass, pass.rs:887), the `.running`/`.outcome` files,
`$STATUS_DIR/<wt>.pr`, or `usage.sup` lines. Without `--remove`, `forget`
leaves every `branch.<b>.sd-*` key except `sheepdog` in place (bookkeeping.rs:203).

### 3d. Writes from `sd state` (I11)

`sd state` is "read-only" (AGENTS.md). Observed writes on its path:
- src/state.rs:298 `git config --unset branch.<b>.sd-pr` and :304 `git config branch.<b>.sd-pr <pick>` when discovery picks a different PR (comment at :302 acknowledges the race).
- src/running.rs:52 `fs::remove_file` of a dead, deadline-less marker, reached from state.rs:1503.
- `gh api graphql` with a `rateLimit` query: a read that spends GitHub quota (state.rs:224).
No write to `refs/sheepdog/*`, the state dir or GitHub was found in state.rs.

## 4. Harness names and harness calls (I2, I5)

### 4a. `"claude"` / `"pi"` as harness names outside harness.rs (`grep -rn '"claude"\|"pi"' src/`)

| file:line | use |
|---|---|
| src/config.rs:144 `HARNESSES`, :159-163 `HARNESS_BLOCKS`, :837, :973 (default `"claude"`), :1158 (init skeleton) | schema and default harness |
| src/act/attach.rs:54 | a stored session with no `sd-harness` belongs to `"claude"` |
| src/usage.rs:99, 111, 161-170 | per-harness cost arithmetic (`harness != "claude"`) |
| src/failed.rs:26, src/act/step.rs:78 | doc comments only |
| src/watch/pass.rs:432 | `STATUS_DIR` default `~/.claude/tab-status` (Claude Code's tab-status, not a harness name) |
| src/act_help.txt:224-227, 284, 322; src/watch_help.txt:166 | help text |
| tests in limit.rs, failed.rs, attach.rs, github.rs, usage.rs, state.rs | fixtures |

**[inference]** I2 does not hold as stated: `config.rs` (schema + default),
`attach.rs:54` (default) and `usage.rs` (cost shape) are per-harness.
`tool_rules` wording is in harness.rs:35 as the rule allows.

### 4b. Every process that spawns a harness

| file:line | spawns | from |
|---|---|---|
| src/harness.rs:407 `claude_command` / :179 `pi` | headless step (`claude -p …`, `pi -p …`) | `run_step` (harness.rs:103) via `agent` |
| src/harness.rs:362, 368 `tui` | interactive `claude --resume`/`pi --session-id` | attach.rs:130 → proc::foreground (attach.rs:155) |

### 4c. Who reaches `harness::run_step` (`grep -rn 'harness::run_step' src/`)

| caller | verb(s) | marked "Spends tokens" in act_help.txt |
|---|---|---|
| src/act/implement.rs:273 | implement; start (child `sd act implement`, start.rs:127); answer (answer.rs:48) | implement yes (:66); start "unless --no-implement" (:54); answer yes (:74) |
| src/act/address.rs:451 | address; answer | yes (:102) |
| src/act/ci_fix.rs:261 | ci-fix; answer | yes (:116) |
| src/act/resolve.rs:210 | restack --resolve | yes (:317) |
| src/act/describe.rs:103 | describe; ship (ship.rs calls describe_run) | yes (:203, :216) |
| src/review_local.rs:110 | `sd review-local`, run as a child by `sd act review` (review.rs:110) or by chrismo | review yes (:171); `sd review-local` is its own top-level verb (main.rs:30) |
| src/harness.rs:633 | `sd harness run …` (top-level verb, main.rs:29) | not an `sd act` verb; no marker when `HARNESS_BRANCH` is unset (harness.rs:108-110) |

`attach` spawns a harness TUI (harness.rs:359) and its help entry
(act_help.txt:223-232) carries no "Spends tokens". AGENTS.md rule 1 lists
implement, address, describe, review, restack --resolve, start/ship; the help
additionally marks `answer` and `ci-fix`. No zero-token verb was found calling
`run_step` or `tui`.

## 5. Process-level: `Command::new` / `proc::` outside proc.rs, by program

| program | sites |
|---|---|
| `git` | git.rs (all), plus §2's four production escapes |
| `gh` | §1's eleven sites |
| `claude`, `pi` | harness.rs:179, 362, 368, 407 |
| `sd` itself (`proc::sd_itself`) | start.rs:127 (`sd act implement`), review.rs:110 (`sd review-local`), pass.rs:735 (`sd watch step`), pass.rs:539 and 1034 (`sd state --json`, `sd act <verb>`; program overridable by `SHEEPDOG_STATE_PROGRAM`/`SHEEPDOG_ACT_PROGRAM`), watch.rs:149 (plist ProgramArguments) |
| `sh -c <user command>` | commands.rs:158 (settings.verify), :217 (settings.autofix), review.rs:114 (settings.localReview), start.rs:96 (`wt.setup`), pass.rs:340 (settings.notify) |
| `$GIT_COMMON_DIR/worktree-setup` | start.rs:89 |
| `rwx results`, `rwx logs` | ci_fix.rs:69, 72 |
| `osascript` | pass.rs:335 (default notifier) |
| `launchctl`, `plutil`, `id -u` | watch.rs:131, 174, 185, 135 |
| `ps -o lstart=`, `kill -0`, `kill -TERM/-KILL -- -<pgid>` | running.rs:27, 118; mod.rs:167; proc.rs:314; pass.rs:87-88 (libc::kill) |
| `hostname` | mod.rs:154 |
| `shasum`, `date`, `sleep`, `sh -c exit`, `true` (test) | pass.rs:1306, 1334; running.rs tests; mod.rs:442 |

Timeouts: `proc::timeout`/`with_timeout` put the child in its own process
group and TERM then KILL it at the deadline (proc.rs:218-342). `proc::run`
has no timeout (proc.rs:29-52); every `gh` call except pass.rs:528 and :823
uses it, as do all `git::git` calls (git.rs:19) except `git_timed`.

## 6. Running marker: taken and released (I14)

Taken in one place: `running::marked` (running.rs:229-251) writes the file
inside `proc::deferring_signals`, pushes it onto `HELD`, and registers an
`atexit` handler once. Entry points: `during` (harness.rs:111, every
`run_step` with a branch), `holding` (mod.rs:319 for WAIT_FOR_STEP verbs
restack/push/ship/autofix/verify/fetch; step.rs:25; review.rs:91;
restack.rs:230; address.rs:528), `leasing` (pass.rs:1032, `sd watch step`).
A nested call with `RUNNING_HELD=<b>` set takes nothing (running.rs:222-224),
so a child `sd act` under `sd watch step` and `run_step` under `holding` share
the outer marker.

Removal paths observed:
1. Normal return: running.rs:247-248 `release` (only if the file still holds this process's line).
2. `std::process::exit` anywhere (30 sites, `grep -rn 'process::exit' src`): `at_exit` (running.rs:165-170) drains `HELD`, then re-raises a deferred signal.
3. HUP/INT/TERM during a step: deferred by `proc::deferring_signals`, the child group is ended (proc.rs:294-296, 315-323), then path 1 runs and `die_of` re-raises (proc.rs:249-253).
4. Watcher: `release`/`stop` remove a leased marker whose pid is dead or past deadline (pass.rs:95-113, 792-797).
5. `sd state`: a dead, no-deadline marker is removed on read (running.rs:51-53).

Paths that do NOT reach a removal by the holder itself:
- SIGKILL of the holder (watcher `stop` after `KILL_AFTER`, pass.rs:103; `launch`'s 5 s SIGKILL, pass.rs:758; an external `kill -9`): no atexit. Left for path 4 (leased) or path 5 (hand-run).
- Signals outside `PASSED_ON` {1, 2, 15} (proc.rs:142): QUIT, ABRT, SEGV etc. terminate without atexit. No handler installed for them.
- `proc::foreground` (attach.rs:155) makes HUP/INT/TERM no-ops for the rest of the process (proc.rs:185-192); attach holds no marker, so no leak there, but a later `holding` in the same process would not see a deferred signal. **[inference]** No such sequence exists in the dispatch today.
- Panic: Cargo.toml sets no `panic = "abort"`, so a panic unwinds to the runtime's exit, which runs atexit handlers. **[inference, not tested here]**
- `marked` when `fs::write` fails returns `None` without a marker (running.rs:241-244): nothing to leak; the verb exits 1 (step.rs:26, mod.rs:320).

`free` is not a lock (running.rs:124-127, comment): two steps starting at
once can both write the marker; the second overwrites the first's line, and
the first's `release` then skips removal (running.rs:153), leaving the
second's marker in place. **[inference]** This is the documented
"not a lock" behaviour, not a leak.
