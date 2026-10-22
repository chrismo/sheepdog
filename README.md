# <img src="img/sam-sheepdog-face.png" alt="Sam Sheepdog" width="110" align="middle"> sheepdog

Takes the busy work out of getting a branch to a merged PR and keeping it
current while it's open: up to date with its parent (stack-aware), CI green on
the head SHA, review current, bot comments answered. Scripts observe and act;
an agent (Claude or pi) is invoked only for a bounded judgment step.

One command, `sd`, with a verb (`sd help` lists them):

| Command      | What it does                                              |
|--------------|-----------------------------------------------------------|
| `sd state`   | Status of a branch's PR, and what to do next              |
| `sd act`     | Start a branch, then take its next step (address, push…)  |
| `sd watch`   | Periodic pass over your branches; optional launchd job    |
| `sd config`  | Show and validate settings                                |

Design notes, consumed into issues and kept for now:
[`docs/pr-workflow/design.md`](docs/pr-workflow/design.md). Decisions are
recorded in the Decision issues on the
[project board](https://github.com/users/chrismo/projects/6); the rules agents
build by are in `AGENTS.md`.

## Install

```bash
./install.sh        # builds sd, links it, hooks git to rebuild it, installs its launchd watcher
```

The installer builds `sd` (`cargo build --release`), links `bin/sd`, a link
to the build, into `~/.local/bin` (`$LOCAL_BIN` to change the link
directory), writes post-merge and post-rewrite hooks into the checkout's git
hooks directory that rebuild `sd` after a pull or a rebase (a hook already
there that isn't sheepdog's is left alone, and the installer says so) and, on
macOS, starts the watcher, which checks enrolled
branches every five minutes by default. Re-run it to update both.

Upgrading from a checkout where `bin/sd` was still a bash script: run
`./install.sh` once after the pull that brings this in. Before then nothing
rebuilds `sd`, and the watcher's old build looks for verbs that are gone. Use
`sd watch uninstall` to stop the background job, or `sd watch install` to
start it again (with the same `$LOCAL_BIN`, if you changed it). On other platforms, only the CLI is linked; the background
watcher requires macOS (launchd). Anything at `$LOCAL_BIN/sd` that isn't a
symlink blocks installation before any links or jobs are changed.

Needs on `PATH`: `bash5` (a link to bash 5+; the commands are
`#!/usr/bin/env bash5`), `cargo` to build (https://rustup.rs), `git`, `gh`,
and `jq`. `claude` and/or `pi` for the agent steps.

## Agent credentials

Claude steps run with `ANTHROPIC_API_KEY` unset, so `claude -p` uses the
claude.ai login (an exported key would override it, and a key in a shell rc is
often not the one to bill). To bill the key instead, set
`harnesses.claude.useApiKey` to `true` (`sd config set --global --json
harnesses.claude.useApiKey true`). pi can't use the claude.ai login, so it
keeps the key, or runs on the provider its block names.

## Agent steps

`sd act implement`, `address` and `describe` each run one headless agent step
(`claude -p` or `pi -p`) in the branch's session, as does `restack --resolve`
under the `implement` step; `start` runs implement unless `--no-implement`,
and `ship` runs describe unless HEAD already has a stored description.
`sd act review`'s default reviewer runs in a fresh read-only session every
time, so it never shares the author's context; a custom reviewer only has to
keep the contract below, so isolation is up to it. `sd config step <step>`
shows the harness, model and effort a step uses. The agent edits; sheepdog
commits, pushes and talks to GitHub.

Each step's prompt is the step's guidance, then a fixed contract (what the
agent may do, what it returns). A repo replaces a step's guidance by committing
`.sheepdog/<step>.md`: `implement`, `review-fix`, `review`, `describe`,
`address`, `resolve` or `ci-fix`. sheepdog reads it from the branch's base
commit, so a branch can't rewrite its own guidance. `settings.<step>Prompt`
(e.g. `implementPrompt`, `reviewFixPrompt`) names another file and wins over
`.sheepdog/`: a relative path is read from the base commit too, an absolute
one from disk. When no base commit can be trusted (no base pin, no default
branch to fork from, or the branch is the default branch), the `.sheepdog/` file is skipped for the shipped
default, and a relative path is an error. The shipped defaults are in
`lib/prompts/`.

A custom local reviewer (`settings.localReview`) keeps the contract that
`sd review-local --help` prints: `SHEEPDOG_BASE`, `SHEEPDOG_HEAD` and
`SHEEPDOG_TASK` in, a JSON list of findings out.

## Interfaces with other tools

- `sd watch` writes a glyph per worktree to `~/.claude/tab-status/<worktree>.pr`
  (`$STATUS_DIR`), which agent-rig's `tab-status` reads. Two watched
  same-named worktrees on one branch get none, since tab-status couldn't tell
  whose it is.
- `sd watch glyph [<path>]` prints the glyph for the worktree holding `<path>`
  (default `.`) while it's still on the branch checked, from
  `$SHEEPDOG_STATE_DIR/glyphs`. It is exact by path and fast (no GitHub), for
  a shell prompt or a tab title.
- The launchd job is labelled `com.sheepdog.watch`.

## Tests

[bats](https://github.com/bats-core/bats-core). See `AGENTS.md` for how to run
them and the bash-version caveat.
