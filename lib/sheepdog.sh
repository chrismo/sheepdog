# Shared by sd act, sd state and sd watch. Source it after lib/config.sh;
# needs bash 5 and super 0.3.0.
#
# harness_run is the harness adapter: the per-harness code, apart from the
# tool-rule wording in implement_run (#79 Design record). with_timeout and
# pin_write are shared helpers.

# with_timeout <seconds> <cmd...>: kills the command's whole process group
# when time runs out (its children would otherwise hold our output pipe open).
with_timeout() {
  perl -e '
    my $t = shift;
    my $pid = fork // die "fork: $!\n";
    if (!$pid) { setpgrp(0, 0); exec @ARGV or die "exec $ARGV[0]: $!\n" }
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

# commits_not_ours <reflog> <sha>: how many commits of <sha> no reflog entry reaches.
commits_not_ours() {
  sed -n 's/^./^&/p' <<<"$1" | git rev-list --count --stdin "$2"
}

# worktree_for <branch>: the worktree that has refs/heads/<branch> checked out, if any.
worktree_for() {
  git worktree list --porcelain \
    | awk -v ref="refs/heads/$1" '/^worktree / { wt = substr($0, 10) } $0 == "branch " ref { print wt; exit }'
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

# Claude may read and edit; the pipeline commits, pushes and talks to GitHub.
HARNESS_CLAUDE_ALLOW="Read,Edit,Write,Grep,Glob,Bash(git diff *),Bash(git log *),Bash(git show *),Bash(git status *)"
HARNESS_CLAUDE_DENY="Bash(git commit *),Bash(git push *),Bash(gh *)"
# pi has no per-command bash rules, so no bash at all.
HARNESS_PI_TOOLS="read,edit,write"
# Read-only steps (review, describe) set HARNESS_CLAUDE_ALLOW to this and
# HARNESS_PI_TOOLS to "read" before calling harness_run.
HARNESS_READONLY_ALLOW="Read,Grep,Glob,Bash(git diff *),Bash(git log *),Bash(git show *),Bash(git blame *)"

# harness_object: the JSON object on stdin, or status 1 (not JSON, not an
# object, nothing at all).
harness_object() {
  local r
  r=$(prc_json 'where kind(this) == "record"' 2>/dev/null) && [[ -n $r ]] || return 1
  printf '%s\n' "$r"
}

# harness_result <claude --output-format json output>: the step's result
# object: structured_output, else the result text parsed as JSON; nothing on
# an error result (status 1).
harness_result() {
  prc_json 'where not coalesce(is_error, false)
    | values coalesce(structured_output, parse_sup(coalesce(result, "")), null)' "$1" 2>/dev/null | harness_object
}

harness_branch() { [[ -n ${HARNESS_BRANCH:-} ]] && echo "$HARNESS_BRANCH" || git branch --show-current 2>/dev/null; }

# main_worktree: the repo's main worktree, where sheepdog's shared files live
# and the name usage.sup knows the repo by.
main_worktree() { git worktree list --porcelain | sed -n '1s/^worktree //p'; }

usage_file() { echo "${SHEEPDOG_STATE_DIR:-$HOME/.local/state/sheepdog}/usage.sup"; }

# harness_transcript <harness> <session-id>: the file the harness keeps the
# session's transcript in, if it wrote one.
harness_transcript() {
  case $1 in
    claude) find "${CLAUDE_CONFIG_DIR:-$HOME/.claude}/projects" -maxdepth 2 -name "$2.jsonl" 2>/dev/null | head -1 ;;
    pi) find "${PI_CODING_AGENT_SESSION_DIR:-$HOME/.pi/agent/sessions}" -maxdepth 2 -name "*_$2.jsonl" 2>/dev/null | head -1 ;;
  esac
}

# harness_tally <harness> <session-id>: what the session's transcript says, as
# one JSON object; {} with no transcript. context is its last turn's tokens,
# the whole context then: what a resume or fork inherits (#121). For claude,
# also entries, its assistant lines: more after a step than before means the
# step ran turns. For pi, which prints no usage, also cost, input,
# cache_create, cache_read, output and turns, summed over the session. A grep
# for the role first: a transcript runs to megabytes. claude's top-level type
# is the record's; a nested one is a subagent's progress, and a quoted one a
# tool result's text. A "<synthetic>" model's entry is claude's note of an
# API error (prompt too long), all zeros: no turn, and no context. Two passes each: an aggregate's collect has no order to
# take the last from.
harness_tally() {
  local f r ctx sel
  f=$(harness_transcript "$1" "$2")
  if [[ -n $f ]]; then
    case $1 in
      claude)
        sel='where type == "assistant" and not on(isSidechain) and not nul(message.usage)
               and coalesce(message.model, "") != "<synthetic>"'
        ctx=$(grep -F '"type":"assistant"' "$f" | prc_line "$sel | tail 1
          | values n(message.usage.input_tokens) + n(message.usage.cache_creation_input_tokens)
                   + n(message.usage.cache_read_input_tokens) + n(message.usage.output_tokens)" 2>/dev/null) || ctx=
        r=$(grep -F '"type":"assistant"' "$f" | prc_json "$sel | aggregate entries:=count()
          | values {...this, context: ${ctx:-null}}" 2>/dev/null) || r= ;;
      pi)
        ctx=$(grep -F '"role":"assistant"' "$f" | prc_line 'where on(message.role == "assistant") and not nul(message.usage)
          | tail 1 | values n(message.usage.totalTokens)' 2>/dev/null) || ctx=
        r=$(grep -F '"role":"assistant"' "$f" | prc_json 'where on(message.role == "assistant") and not nul(message.usage)
          | aggregate cost:=sum(n(message.usage.cost.total)), input:=sum(n(message.usage.input)),
                      cache_create:=sum(n(message.usage.cacheWrite)), cache_read:=sum(n(message.usage.cacheRead)),
                      output:=sum(n(message.usage.output)), turns:=count()
          | values {...this, context: '"${ctx:-null}"'}' 2>/dev/null) || r= ;;
    esac
  fi
  echo "${r:-"{}"}"
}

# The usage record, over {t, repo, branch, step, harness, session, mode,
# status, ms, cap: the step's --max-budget-usd ("0": none known), o: claude's
# result object, a and b: harness_tally after and before}. t is passed in, not
# now(), so the output is the same every run. A claude step with no result
# (killed by stepTimeout, or crashed) that still ran turns is charged its cap,
# the most it could spend, and marked estimated; with no cap its cost is
# unknown (null), and still marked.
read -r -d '' USAGE_Q <<'EOF' || true
fn nn(s): s == "" ? null : s
values {time: t::time, repo: nn(repo), branch: nn(branch), step: nn(step), harness, session, mode, status,
  context: coalesce(a.context, null), cap: cap::float64,
  unseen: harness == "claude" and nul(o) and n(a.entries) > n(b.entries),
  u: harness == "claude"
     ? (nul(o) ? null : {cost: coalesce(o.total_cost_usd, 0), input: n(o.usage.input_tokens),
                         cache_create: n(o.usage.cache_creation_input_tokens), cache_read: n(o.usage.cache_read_input_tokens),
                         output: n(o.usage.output_tokens), duration_ms: n(o.duration_ms), turns: n(o.num_turns)})
     : (nul(a.turns) ? null : {cost: a.cost - n(b.cost), input: a.input - n(b.input),
                               cache_create: a.cache_create - n(b.cache_create), cache_read: a.cache_read - n(b.cache_read),
                               output: a.output - n(b.output), duration_ms: ms, turns: a.turns - n(b.turns)})}
| values {time, repo, branch, step, harness, session, mode, status: status::int64,
          cost_usd: not nul(u) ? u.cost::float64 : unseen and cap > 0 ? cap : null,
          estimated: unseen,
          input: nul(u) ? null : u.input::int64, cache_create: nul(u) ? null : u.cache_create::int64,
          cache_read: nul(u) ? null : u.cache_read::int64, output: nul(u) ? null : u.output::int64,
          duration_ms: nul(u) ? null : u.duration_ms::int64, turns: nul(u) ? null : u.turns::int64,
          context: nul(context) ? null : context::int64}
EOF

# harness_usage <harness> <session-id> <mode> <status> <output> <tally-before> <ms> <cap>:
# one record per step, a line of Super (SUP) appended to usage.sup in
# $SHEEPDOG_STATE_DIR: time, repo, branch, step, harness, session, mode, status,
# cost_usd, estimated (USAGE_Q), the tokens (input, cache_create, cache_read,
# output), duration_ms, turns, and context (harness_tally). claude's numbers
# are from its JSON output; pi's, which prints none, are what its transcript
# gained during the step; null when there are none. The cost is the harness's
# estimate, not how it was billed. A usage.tsv from before #121 is left as it
# was.
harness_usage() {
  local file rec obj=null after strs
  file=$(usage_file)
  if [[ $1 == claude ]]; then obj=$(printf '%s\n' "$5" | harness_object) || obj=null; fi
  after=$(harness_tally "$1" "$2")
  strs=$(printf '"t": "%s", "repo": %s, "branch": %s, "step": %s, "harness": %s, "session": %s, "mode": %s' \
           "$(date -u +%FT%TZ)" "$(prc_json_string "$(main_worktree 2>/dev/null)")" "$(prc_json_string "$(harness_branch)")" \
           "$(prc_json_string "${HARNESS_STEP:-}")" "$(prc_json_string "$1")" "$(prc_json_string "$2")" "$(prc_json_string "$3")") \
    || return 0
  rec=$(printf '{%s, "status": %s, "ms": %s, "cap": "%s", "o": %s, "a": %s, "b": %s}\n' "$strs" "$4" "$7" "${8:-0}" "$obj" "$after" "$6" \
          | prc_super -i json -s -c "$PRC_FNS"$'\n'"$USAGE_Q" - 2>/dev/null) && [[ -n $rec ]] || return 0
  mkdir -p "${file%/*}" 2>/dev/null || return 0
  printf '%s\n' "$rec" >> "$file"
}

# branch_spend <branch>: what this repo's <branch> has cost so far, in USD, by
# usage.sup, as a JSON number; 0 with no record. The file is every repo's and
# branch's, so a line that isn't a whole record (an append cut off, a stray
# edit) is skipped with a warning rather than failing every branch's sd state;
# one shaped like a record that still doesn't parse fails it. A filter, not
# parse_sup per line: on super 0.3.0 that misreported valid lines around a bad
# one, and -e=false still stops at a SUP syntax error.
branch_spend() {
  local file repo b out skipped
  file=$(usage_file)
  [[ -s $file ]] || { echo 0; return 0; }
  repo=$(prc_json_string "$(main_worktree)") && b=$(prc_json_string "$1") || return 1
  skipped=$(grep -cvE '^\{time:.*\}$' "$file") || true
  (( skipped == 0 )) || echo "warning: $skipped line(s) of $file aren't usage records; their cost isn't counted" >&2
  out=$({ grep -E '^\{time:.*\}$' "$file" || true; } \
          | prc_super -i sup -j -c "where repo == $repo and branch == $b | aggregate usd:=sum(cost_usd) | values coalesce(usd, 0)" - 2>/dev/null) \
    || return 1
  echo "${out:-0}"
}

# harness_run <harness> <session-id> <new|resume> <timeout> <prompt> <json-schema> [<settings-json>]
# Runs one headless agent step in $PWD; prints the step's JSON result.
# settings-json is prc_step's output: model, effort, provider, args, and for
# claude useApiKey. Claude runs with ANTHROPIC_API_KEY unset unless that is
# true: an exported key overrides the claude.ai login (#68), and the one in a
# shell rc is often not the one to bill. pi can't use the login, so it keeps
# the key. settings.stepBudget (USD, read here for the worktree's branch)
# caps a claude step through --max-budget-usd; 0 is no cap. pi has no such
# flag: settings.branchBudget (sd state) is its only guard. Every step
# appends a record to $SHEEPDOG_STATE_DIR/usage.sup (harness_usage), with
# $HARNESS_STEP as the step name when the caller sets it. The branch is
# $HARNESS_BRANCH, else the worktree's: a step that runs mid-rebase (restack
# --resolve) has a detached HEAD, so callers set it.
# The harness's stderr (claude can put protocol JSON there) shows only when
# the step fails, and then all at once: nothing streams while it runs.
harness_run() {
  local h=$1 sid=$2 mode=$3 timeout=$4 prompt=$5 schema=$6 settings=${7:-'{}'} out err rc said budget cap=""
  local model effort provider use_key args fields before t0 cap_usd=0 args_cap=false i
  local -a typed=() extra f env=()
  # args last, so a newline in it shifts nothing; the end marker survives $( )
  # stripping the trailing newlines of empty fields.
  fields=$(prc_line 'where kind(this) == "record"
    | values coalesce(model, ""), coalesce(effort, ""), coalesce(provider, ""), coalesce(useApiKey, false), coalesce(args, ""), "<end>"' "$settings") || return 1
  mapfile -t f <<<"$fields"
  [[ ${#f[@]} -ge 6 && ${f[-1]} == "<end>" ]] || return 1
  model=${f[0]} effort=${f[1]} provider=${f[2]} use_key=${f[3]} args=$(printf '%s\n' "${f[@]:4:${#f[@]}-5}")
  read -ra extra <<<"$args"
  # An interrupted step leaves this file behind (no trap: harness_run runs in
  # its caller's shell, whose traps it would replace).
  err=$(mktemp) || return 1
  case $h in
    claude)
      [[ $use_key == true ]] || env=(env -u ANTHROPIC_API_KEY)
      if [[ -n $model ]]; then typed+=(--model "$model"); fi
      if [[ -n $effort ]]; then typed+=(--effort "$effort"); fi
      budget=$(prc_get settings.stepBudget "$(harness_branch)" false) || { echo "sd config failed" >&2; rm -f "$err"; return 1; }
      [[ $budget != -* ]] || { echo "settings.stepBudget must be 0 or more: $budget" >&2; rm -f "$err"; return 1; }
      # A cap in args is the more specific one, so it replaces stepBudget's.
      for i in "${!extra[@]}"; do
        case ${extra[i]} in
          --max-budget-usd) cap_usd=${extra[i+1]:-0} args_cap=true ;;
          --max-budget-usd=*) cap_usd=${extra[i]#*=} args_cap=true ;;
        esac
      done
      [[ $budget =~ ^0*(\.0*)?$ || $args_cap == true ]] \
        || { typed+=(--max-budget-usd "$budget"); cap=" (settings.stepBudget $budget USD)" cap_usd=$budget; } ;;
    pi)
      if [[ -n $provider ]]; then typed+=(--provider "$provider"); fi
      if [[ -n $model ]]; then typed+=(--model "$model"); fi
      if [[ -n $effort ]]; then typed+=(--thinking "$effort"); fi ;;
  esac
  before=$(harness_tally "$h" "$sid")
  t0=${EPOCHREALTIME//[.,]/}
  case $h in
    claude)
      local -a sess=(--session-id "$sid")
      [[ $mode == resume ]] && sess=(--resume "$sid")
      # The prompt goes first: --allowedTools and --disallowedTools are variadic
      # and would swallow a trailing one.
      out=$(with_timeout "$timeout" "${env[@]}" claude -p "$prompt" "${sess[@]}" --output-format json --json-schema "$schema" \
              --permission-mode acceptEdits --allowedTools "$HARNESS_CLAUDE_ALLOW" \
              --disallowedTools "$HARNESS_CLAUDE_DENY" "${typed[@]}" "${extra[@]}" </dev/null 2>"$err") \
        && harness_result "$out"
      ;;
    pi)
      # pi has no schema flag: the prompt asks for a final ```json block.
      out=$(with_timeout "$timeout" pi -p --session-id "$sid" --tools "$HARNESS_PI_TOOLS" "${typed[@]}" "${extra[@]}" \
              "$prompt"$'\n\nEnd your reply with the result as one ```json fenced block matching this JSON Schema:\n'"$schema" \
              </dev/null 2>"$err") \
        && awk '/^```json/ { buf = ""; on = 1; next } /^```/ { if (on) last = buf; on = 0; next }
                on { buf = buf $0 "\n" } END { printf "%s", last }' <<<"$out" | harness_object
      ;;
    *) echo "unknown harness: $h (claude or pi)" >&2; rm -f "$err"; return 1 ;;
  esac
  rc=$?
  harness_usage "$h" "$sid" "$mode" "$rc" "$out" "$before" $(( (${EPOCHREALTIME//[.,]/} - t0) / 1000 )) "$cap_usd"
  if (( rc != 0 )); then
    tail -n 40 "$err" >&2
    # The end of the harness's own account of the failure: the budget hit,
    # else claude's result text, else an error result's subtype (e.g.
    # error_max_turns), else stdout that isn't JSON; pi's reply.
    said=$out
    if [[ $h == claude ]]; then
      said=$(prc_line 'where kind(this) == "record"
        | values on(subtype == "error_max_budget_usd")
            ? f"hit the step budget'"$cap"': the step cost {coalesce(total_cost_usd, 0)} USD"
            : coalesce(nullif(result, ""), coalesce(is_error, false) ? subtype : null, "")
        | where this != ""' "$out" 2>/dev/null) || said=$out
    fi
    [[ -z $said ]] || tail -n 20 <<<"$said" >&2
  fi
  rm -f "$err"
  return "$rc"
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
