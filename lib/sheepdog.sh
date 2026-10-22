# What sd act's bash verbs shared. No verb sources it since the last of them
# went to Rust (#203); lib/sheepdog.bats still tests what's left, and both go
# together. Source it after lib/config.sh; needs bash 5 and super 0.3.0.
#
# harness_run runs the harness adapter, src/harness.rs (#79 Design record).

SD_BIN="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/target/release/sd"

# with_timeout <seconds> <cmd...>: kills the command's whole process group
# when time runs out (its children would otherwise hold our output pipe open).
# In its own group, a Ctrl-C or a kill of ours doesn't reach the command, so
# a HUP, INT or TERM is passed on, and waited for, before it ends us too: an
# agent left running would edit after its running marker is gone (#170).
with_timeout() {
  perl -e '
    my $t = shift;
    my $pid = fork // die "fork: $!\n";
    if (!$pid) { setpgrp(0, 0); exec @ARGV or die "exec $ARGV[0]: $!\n" }
    for my $s (qw(HUP INT TERM)) {
      $SIG{$s} = sub { kill "TERM", -$pid; waitpid $pid, 0; $SIG{$s} = "DEFAULT"; kill $s, $$ };
    }
    $SIG{ALRM} = sub { kill "TERM", -$pid; print STDERR "timed out after ${t}s\n"; exit 124 };
    alarm $t;
    waitpid $pid, 0;
    exit($? & 127 ? 128 + ($? & 127) : $? >> 8);
  ' "$@"
}

# repo_remote: the remote sheepdog fetches, pushes and asks GitHub about:
# settings.remote, from the repo file inside a repo, else the global file.
repo_remote() { prc_get settings.remote "" true; }

# gh_repo_of <url>: host/owner/repo, the form GH_REPO takes, from a remote's
# URL (scp-style, ssh://, https://); empty for a local path or file:// URL.
# src/git.rs's gh_repo_of is the same for sd state.
gh_repo_of() {
  local u=${1%/}
  u=${u%.git}
  if [[ $u == file:* ]]; then
    return 0
  elif [[ $u =~ ^[a-z+]+://([^@/]+@)?([^/:]+)(:[0-9]+)?/([^/]+)/([^/]+)$ ]]; then
    echo "${BASH_REMATCH[2]}/${BASH_REMATCH[4]}/${BASH_REMATCH[5]}"
  elif [[ $u =~ ^([^@/:]+@)?([^/:]+):([^/]+)/([^/]+)$ ]]; then
    echo "${BASH_REMATCH[2]}/${BASH_REMATCH[3]}/${BASH_REMATCH[4]}"
  fi
}

# gh_repo <remote>: export GH_REPO for <remote>'s URL; unset, gh picks among
# the remotes, and in a fork's clone that's upstream. A URL with no repo in
# it leaves GH_REPO be.
gh_repo() {
  local url repo
  url=$(git remote get-url "$1" 2>/dev/null) || return 0
  repo=$(gh_repo_of "$url")
  if [[ -n $repo ]]; then export GH_REPO=$repo; fi
}

# worktree_mark <worktree>: a hash of what's uncommitted there, staged or not,
# untracked files' contents included; writes nothing (no objects, no index
# refresh). The same mark: nothing in the worktree changed. Only regular
# files' contents are hashed (hash-object can't open a nested repo's
# directory, and --stdin-paths reads one name per line); status still
# lists the rest.
worktree_mark() {
  local f
  { git -C "$1" --no-optional-locks status --porcelain -z --untracked-files=all \
    && git -C "$1" --no-optional-locks diff --binary --no-color --no-ext-diff --no-textconv HEAD \
    && git -C "$1" ls-files -o -z --exclude-standard \
      | while IFS= read -r -d '' f; do
          if [[ -f $1/$f && $f != *$'\n'* ]]; then printf '%s\n' "$f"; fi
        done \
      | git -C "$1" hash-object --stdin-paths; } \
    | git -C "$1" hash-object --stdin
}

# step_guidance <setting> <branch> <base> <default-file>: the guidance part of a
# step's prompt: the file settings.<setting> names; else .sheepdog/<name>.md
# in the <base> commit, <name> being the setting without "Prompt", kebab-cased
# (reviewFixPrompt: .sheepdog/review-fix.md); else <default-file>. A relative path is
# read from the <base> commit, which the branch's agent can't rewrite; an
# absolute one from disk. A named file that is missing is an error, never the
# default. An empty <base> means none can be trusted: no .sheepdog/ lookup,
# and a relative path is refused. src/guidance.rs is the copy sd runs.
step_guidance() {
  local path top conv
  path=$(prc_get "settings.$1" "$2" false) || return 1
  # From the top, so ./ means the worktree's top wherever the step runs.
  top=$(git rev-parse --show-toplevel) || return 1
  if [[ -z $path && -z $3 ]]; then
    cat "$4"
  elif [[ -z $path ]]; then
    conv=".sheepdog/$(sed -E 's/Prompt$//; s/([A-Z])/-\1/g' <<<"$1" | tr 'A-Z' 'a-z').md"
    if [[ $(git -C "$top" cat-file -t "$3:$conv" 2>/dev/null) == blob ]]; then
      git -C "$top" cat-file blob "$3:$conv"
    else
      cat "$4"
    fi
  elif [[ $path == /* ]]; then
    [[ -f $path ]] || { echo "settings.$1: no such file: $path" >&2; return 1; }
    cat "$path"
  elif [[ -z $3 ]]; then
    echo "settings.$1: no base commit to read $path from; use an absolute path" >&2; return 1
  else
    [[ $(git -C "$top" cat-file -t "$3:$path" 2>/dev/null) == blob ]] \
      || { echo "settings.$1: no such file in ${3:0:7}: $path" >&2; return 1; }
    git -C "$top" cat-file blob "$3:$path"
  fi
}

# CI config: an agent step's commit that touches one of these isn't pushed
# until chrismo has looked (#124). A path covers itself and what's under it.
PROTECTED_PATHS_DEFAULT=(.github/workflows/ .github/actions/ .rwx/ .captain/ .circleci/ .gitlab-ci.yml .buildkite/)

# protected_paths <branch> <base>: the protected paths, one per line:
# settings.protectedPaths when set (an empty list protects nothing); else
# .sheepdog/protected-paths in the <base> commit, which the branch's agent
# can't rewrite (one per line, # comments); else PROTECTED_PATHS_DEFAULT. An
# empty <base> skips the file, as step_guidance does. Unless the setting is
# set, .sheepdog/protected-paths is itself protected: an agent emptying it
# would unprotect every branch after it merges.
protected_paths() {
  local out rc=0
  out=$(prc_get settings.protectedPaths "$1" false) || rc=$?
  if (( rc == 0 )); then
    [[ -z $out ]] || printf '%s\n' "$out"
    return 0
  elif (( rc != 3 )); then
    return 1
  elif [[ -n $2 && $(git cat-file -t "$2:.sheepdog/protected-paths" 2>/dev/null) == blob ]]; then
    git cat-file blob "$2:.sheepdog/protected-paths" | sed -E 's/#.*//; s/^[[:space:]]+//; s/[[:space:]]+$//; /^$/d'
  else
    printf '%s\n' "${PROTECTED_PATHS_DEFAULT[@]}"
  fi
  echo .sheepdog/protected-paths
}

# protected_hits <paths> <files>: the <files> (one per line) a protected path covers.
protected_hits() {
  local f p
  while IFS= read -r f; do
    [[ -n $f ]] || continue
    while IFS= read -r p; do
      [[ -n $p ]] || continue
      if [[ $f == "${p%/}" || $f == "${p%/}"/* ]]; then echo "$f"; break; fi
    done <<<"$1"
  done <<<"$2"
}

# The tools harness_run gives the agent: src/harness.rs's CLAUDE_ALLOW and
# PI_TOOLS. Claude may read and edit; the pipeline commits, pushes and talks
# to GitHub.
HARNESS_CLAUDE_ALLOW="Read,Edit,Write,Grep,Glob,Bash(git diff *),Bash(git log *),Bash(git show *),Bash(git status *)"
# pi has no per-command bash rules, so no bash at all.
HARNESS_PI_TOOLS="read,edit,write"

# harness_run <harness> <session-id> <new|resume> <timeout> <prompt> <json-schema> [<settings-json>]
# Runs one headless agent step in $PWD; prints the step's JSON result.
# settings-json is prc_step's output. $HARNESS_STEP names the step in
# usage.sup when the caller sets it, and $HARNESS_BRANCH its branch, else the
# worktree's. src/harness.rs has the rest: the API key, the step budget, the
# usage record and the running marker.
harness_run() {
  HARNESS_STEP=${HARNESS_STEP:-} HARNESS_BRANCH=${HARNESS_BRANCH:-} HARNESS_CLAUDE_ALLOW=$HARNESS_CLAUDE_ALLOW \
    HARNESS_PI_TOOLS=$HARNESS_PI_TOOLS "$SD_BIN" harness run "$@"
}

# harness_tally <harness> <session-id>: what the session's transcript says, as
# one JSON object; {} with no transcript. context is its last turn's tokens.
harness_tally() { "$SD_BIN" harness tally "$@"; }

# running_file <branch>: the marker kept while an agent step runs on
# <branch>, one line: "<pid> <step> <start time> <the pid's proc_start>".
# harness_run's own is src/running.rs's, the same file.
running_file() {
  local dir
  dir=$(git rev-parse --path-format=absolute --git-common-dir) || return 1
  echo "$dir/sheepdog/${1//\//__}.running"
}

# proc_start <pid>: when <pid> started, by ps, else nothing. It tells a
# marker's process from a later one given the same pid.
proc_start() { LC_ALL=C TZ=UTC ps -o lstart= -p "$1" 2>/dev/null | awk '{ $1 = $1; print }' || true; }

# running_hold <branch> <step>: <branch>'s marker, for this (sub)shell, for a
# step whose commit and verify come after its harness_run; running_release
# removes it. No trap: a step that dies takes its pid with it, and sd state
# clears the marker. RUNNING_HELD is exported, so the step's own children
# (sd-review-local in implement's review rounds) run under its marker.
running_hold() {
  local f pid=$BASHPID
  running_free "$1" || return 1
  f=$(running_file "$1") && mkdir -p "${f%/*}" || return 1
  echo "$pid $2 $(date -u +%Y-%m-%dT%H:%M:%SZ) $(proc_start "$pid")" >"$f" || return 1
  export RUNNING_HELD=$1
}

running_release() {
  local f pid rest
  f=$(running_file "$1") || return 1
  if [[ -s $f ]] && read -r pid rest <"$f" && [[ $pid == "$BASHPID" ]]; then rm -f "$f"; fi
  RUNNING_HELD=
}

# running_free <branch>: fails, saying so, while another live process's agent
# step holds <branch>'s marker, so a second pass doesn't start a second agent
# on the worktree, or take the first one's marker. Not a lock: two steps
# starting at once can both pass.
running_free() {
  local state pid step since rest
  state=$(running_state "$1") || return 1
  [[ $state != null ]] || return 0
  read -r pid step since rest <"$(running_file "$1")" || return 0
  [[ $pid != "$BASHPID" ]] || return 0
  echo "$step is running on $1 (pid $pid, since $since): not starting another agent step" >&2
  return 1
}

# running_state <branch>: the agent step running on <branch>, as JSON
# {step, pid, since}, else null. A marker whose pid is gone (its step was
# killed past the trap), or now another process's, is removed, not trusted
# (#79 design rule 4).
running_state() {
  local f pid step since start
  f=$(running_file "$1") || return 1
  [[ -s $f ]] && read -r pid step since start <"$f" || { echo null; return 0; }
  if [[ $pid =~ ^[0-9]+$ ]] && kill -0 "$pid" 2>/dev/null && [[ $(proc_start "$pid") == "$start" ]]; then
    printf '{"step": %s, "pid": %s, "since": %s}\n' "$(prc_json_string "$step")" "$pid" "$(prc_json_string "$since")"
  else
    rm -f "$f"
    echo null
  fi
}
