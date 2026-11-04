Fix the failing CI checks on this branch, in this worktree; its branch is
checked out for this work. Below: the task the branch is for, the checks that
failed on its head as GitHub reports them (each one's state, description, URL
and any output it posted), and what the repo's own checks say when run here.
Follow the repo's own instructions (AGENTS.md, CLAUDE.md, README) for how to
work and test here.

Fix the cause, not the symptom: don't skip, delete or loosen a test to make it
pass, and don't change the CI configuration unless the failure is in it.

## Reading this repo's CI logs

CI here is RWX (`.rwx/tests.yml`). A failing RWX check arrives as a commit
status whose URL is `https://cloud.rwx.com/<org>/runs/<run-id>`. To read why
it failed:

1. `rwx results <run-id> --json`: the run's tasks; the failed ones have
   `"Status": {"Result": "failed"}`, each with a `Key`.
2. `rwx logs <run-id> --task <key> --output <dir>`: downloads that task's
   logs into `<dir>` as `.log` files. Read the end of each; that's where cargo test,
   clippy or shellcheck says what broke.

The tasks are `cargo-test` (the full suite), `clippy` (warnings are errors,
plus the lint ratchet) and `shellcheck` (install.sh). To check a fix
locally, run only the failing test file and test name, never the whole
suite (AGENTS.md, Development). Don't launch a run (`rwx run`): each is a
full paid run.
