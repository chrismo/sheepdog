# Shared by sd act and sd review-local. Source it after lib/config.sh;
# needs bash 5 and super 0.3.0.
#
# harness_run runs the harness adapter, src/harness.rs: the per-harness code,
# apart from the tool-rule wording in implement_run (#79 Design record).
# with_timeout and pin_write are shared helpers.

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

# pin_write <branch> <kind> <sha> <base>: record a review pin and the base
# it was taken against. sd state compares base..pin with the branch's current
# base..HEAD, so a restack (which moves the base) doesn't make a reviewed
# change look changed. Kinds: local, bot, accepted (#66 Re-review policy).
pin_write() {
  case $2 in local|bot|accepted) ;; *) echo "unknown pin: $2 (local, bot, accepted)" >&2; return 1 ;; esac
  git update-ref -m "pin $2" "refs/sheepdog/$1/$2" "$3" \
    && git update-ref -m "pin $2 base" "refs/sheepdog/$1/$2-base" "$4"
}

# remote_default <remote>: the default branch without asking GitHub: what the
# remote says its HEAD is, else refs/remotes/<remote>/HEAD, else main. sd act
# push reads it, and so does sd state when GitHub can't answer, so the two
# agree on what the default branch is (#159).
remote_default() {
  local ref
  ref=$(with_timeout 10 git ls-remote --symref "$1" HEAD 2>/dev/null \
          | sed -n 's|^ref: refs/heads/\(.*\)	HEAD$|\1|p') || true
  if [[ -z $ref ]]; then
    ref=$(git symbolic-ref -q --short "refs/remotes/$1/HEAD" 2>/dev/null || true)
    ref=${ref#"$1"/}
  fi
  echo "${ref:-main}"
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

# commits_not_ours <reflog> <sha>: how many commits of <sha> no reflog entry reaches.
commits_not_ours() {
  sed -n 's/^./^&/p' <<<"$1" | git rev-list --count --stdin "$2"
}

# worktree_for <branch>: the worktree that has refs/heads/<branch> checked out, if any.
worktree_for() {
  git worktree list --porcelain \
    | awk -v ref="refs/heads/$1" '/^worktree / { wt = substr($0, 10) } $0 == "branch " ref { print wt; exit }'
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

# An agent step's standing question (#167): refs/sheepdog/<b>/question, a
# blob {head, step, mark, text}: what the step (implement, review-fix,
# address, ci-fix) asked chrismo, at the HEAD and worktree_mark it left. It
# stands while both are unchanged, until sd act ack --question drops it.
# question_state <branch> <head>: the record as sd state shows it, {step, sha,
# text, at_head}; nothing with no record, status 1 for one that isn't one.
question_state() {
  local b=$1 rec h m wt mark at=false
  rec=$(git cat-file blob "refs/sheepdog/$b/question" 2>/dev/null) || return 0
  rec=$(prc_json 'where kind(this) == "record" and is(head, <string>) and is(step, <string>)
                    and is(mark, <string>) and is(text, <string>)' "$rec" 2>/dev/null) && [[ -n $rec ]] || return 1
  read -r h m < <(prc_line 'values f"{head} {mark}"' "$rec")
  if [[ $h == "$2" ]] && wt=$(worktree_for "$b") && [[ -n $wt ]] && mark=$(worktree_mark "$wt") && [[ $mark == "$m" ]]; then
    at=true
  fi
  prc_json "values {step, sha: head, text, at_head: $at}" "$rec"
}

# made_here <range>: every commit in <range> was authored and committed by
# this repo's user. A commit, amend, cherry-pick or rebase here sets the
# committer (a fast-forward merge, pull or rebase onto the remote keeps the
# original one); the author survives a rebase, so a colleague's commit that a
# restack replayed is still theirs. Only commits made here may be dropped by
# a force-push (#86). No user.email: nothing is.
made_here() {
  local me people
  me=$(git config user.email) && [[ -n $me ]] || return 1
  people=$(git log --format='%ae%n%ce' "$1") || return 1
  [[ -z $(grep -vxF "$me" <<<"$people") ]]
}

# step_guidance <setting> <branch> <base> <default-file>: the guidance part of a
# step's prompt: the file settings.<setting> names; else .sheepdog/<name>.md
# in the <base> commit, <name> being the setting without "Prompt", kebab-cased
# (reviewFixPrompt: .sheepdog/review-fix.md); else <default-file>. A relative path is
# read from the <base> commit, which the branch's agent can't rewrite; an
# absolute one from disk. A named file that is missing is an error, never the
# default. An empty <base> means none can be trusted: no .sheepdog/ lookup,
# and a relative path is refused. Every configurable step prompt goes through
# here.
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

# flag_lines <state-json>: one line per sd state flag (#125), for sd state
# --long and sd act review: kind, file:line (and the lines removed, for a
# test-edit), the line, the commits.
read -r -d '' FLAG_LINES <<'EOF' || true
unnest coalesce(flags, [])
| values f"  ⚑  {kind}  {file}:{line}{kind == "test-edit" ? f" (-{n})" : ""}  {regexp_replace(text, "^\\s+", "")}  {sj(coalesce([unnest commits | values this[0:7]], []), ",")}"
EOF
flag_lines() { prc_line "$FLAG_LINES" "$1"; }

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

# The tools harness_run gives the agent, the only copy (src/harness.rs has
# none), here for the steps that add to them. Claude may read and edit; the
# pipeline commits, pushes and talks to GitHub.
HARNESS_CLAUDE_ALLOW="Read,Edit,Write,Grep,Glob,Bash(git diff *),Bash(git log *),Bash(git show *),Bash(git status *)"
# pi has no per-command bash rules, so no bash at all.
HARNESS_PI_TOOLS="read,edit,write"
# Read-only steps (review, describe) set HARNESS_CLAUDE_ALLOW to this and
# HARNESS_PI_TOOLS to "read" before calling harness_run.
HARNESS_READONLY_ALLOW="Read,Grep,Glob,Bash(git diff *),Bash(git log *),Bash(git show *),Bash(git blame *)"

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

# harness_transcript <harness> <session-id>: the file the harness keeps the
# session's transcript in, if it wrote one.
harness_transcript() { "$SD_BIN" harness transcript "$@"; }

# main_worktree: the repo's main worktree, where sheepdog's shared files live
# and the name usage.sup knows the repo by.
main_worktree() { git worktree list --porcelain | sed -n '1s/^worktree //p'; }

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

# running_during <branch> <step> <cmd...>: <cmd> with running_hold's marker,
# or under the one this process or its parent already holds for <branch>.
# Under set -e a failing <cmd> exits before the release (see running_hold).
running_during() {
  local rc
  [[ ${RUNNING_HELD:-} != "$1" ]] || { "${@:3}"; return; }
  running_hold "$1" "$2" || return 1
  "${@:3}"; rc=$?
  running_release "$1"
  return "$rc"
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

# stacked_on <branch> <remote> <default>: the branch <branch> was made from when
# no one recorded it (a plain git branch from another's tip), found by the
# commits they share that aren't on <default>, local or remote. Prints
# "<name> <base>", base being where they forked. Exit 1: none; 2: a tie, the
# names printed; 3: the ones found have commits of their own past the fork
# too, so git can't say which was made from which (a parent that moved on looks
# like a child), the names printed. Git only, and bounded for sd state:
# candidates are local branches and their remote-tracking refs that hold
# <branch>'s oldest commit past the default, two rev-lists and a merge-base
# each, plus one read of <branch>'s reflog. A branch holding all of <branch>'s
# commits is its child, not its parent, and so is one whose sd-parent names
# <branch>, and one forked where <branch>'s reflog has it arrive other than by
# being made, renamed, reset or fast-forwarded there (a child or a backup). At
# equal shared commits, one with none of its own beats one with some: a
# sibling shares the parent's commits too.
stacked_on() {
  local b=$1 remote=$2 def="refs/remotes/$2/$3" oldest total ref name own theirs shared score fork sha msg
  local best="" best_base="" best_score=0 best_theirs=0
  local -A local_branch=() tie=() reached=()
  local -a not=("^$def")
  [[ $b != "$3" ]] || return 1
  if git show-ref -q --verify "refs/heads/$3"; then not+=("^refs/heads/$3"); fi
  oldest=$(git rev-list --topo-order --reverse "refs/heads/$b" "${not[@]}" 2>/dev/null) || return 1
  oldest=${oldest%%$'\n'*}
  [[ -n $oldest ]] || return 1
  total=$(git rev-list --count "refs/heads/$b" "${not[@]}")
  while read -r name; do local_branch[$name]=1; done < <(git for-each-ref --format='%(refname:short)' refs/heads)
  while read -r sha msg; do
    case $msg in
      "branch: Created from "* | "Branch: renamed "* | "reset: moving to "* | "branch: Reset to "* | *": Fast-forward") ;;
      *) reached[$sha]=1 ;;
    esac
  done < <(git reflog show --format='%H %gs' "refs/heads/$b" -- 2>/dev/null)
  while read -r ref; do
    name=${ref#refs/heads/}; name=${name#refs/remotes/"$remote"/}
    [[ $name != "$b" && $name != "$3" && -n ${local_branch[$name]:-} ]] || continue
    own=$(git rev-list --count "refs/heads/$b" "${not[@]}" "^$ref")
    (( own > 0 )) || continue
    [[ $(git config --get "branch.$name.sd-parent") != "$b" ]] || continue
    theirs=$(git rev-list --count "$ref" "${not[@]}" "^refs/heads/$b")
    fork=$(git merge-base "refs/heads/$b" "$ref")
    [[ -z ${reached[$fork]:-} ]] || continue
    shared=$(( total - own ))
    score=$(( shared * 2 + (theirs == 0) ))
    if (( score > best_score )); then
      best=$name best_base=$fork best_score=$score best_theirs=$theirs tie=()
    elif (( score == best_score )) && [[ $name != "$best" ]]; then
      tie[$name]=1
    fi
  done < <(git for-each-ref --contains "$oldest" --format='%(refname)' refs/heads "refs/remotes/$remote")
  [[ -n $best ]] || return 1
  if (( best_theirs > 0 || ${#tie[@]} )); then
    printf '%s\n' "$best" "${!tie[@]}" | sort | paste -sd' ' -
    (( best_theirs == 0 )) && return 2 || return 3
  fi
  echo "$best $best_base"
}

# stacked_unclear <branch> <names> <default>: why stacked_on exited 3, and the
# git config either way.
stacked_unclear() {
  local name=$2
  [[ $name != *" "* ]] || name="<name>"
  echo "shares commits with ${2// / or }, and each has its own since, so git can't tell which, if either, was made from the other: git config branch.$1.sd-parent $name if $1 was, or git config branch.$1.sd-parent $3 if not"
}
