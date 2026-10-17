# Harness and settings for the PR pipeline. Sourced by libexec/sd-config, sd act,
# sd watch and sd state; needs bash 5 and super 0.3.0. Layers and paths: sd config --help;
# SuperDB's edges: AGENTS.md, "SuperDB notes".
#
# Every function returns 1 on an error (message on stderr) and never exits:
# callers run them as `x=$(prc_...) || die`, where errexit is off inside, so
# each step checks its own status. prc_get returns 3 for a path with no value
# and no default.
#
# super reads nothing from a here-string or a < redirect (super 0.3.0), so input
# is always piped.

PRC_DEFAULTS='{"harness": "claude",
  "settings": {"remote": "origin", "stepTimeout": 1800, "stepBudget": 5, "branchBudget": 25, "forkLimit": 100000, "reviewRounds": 3, "botTimeout": 3600, "interval": 300, "vacuumAfter": 604800, "rateLimitFloor": 250, "replyPrefix": "🤖 ",
               "verify": "", "autofix": "", "localReview": "", "botCommand": "", "notify": "",
               "reviewPrompt": "", "implementPrompt": "", "reviewFixPrompt": "", "describePrompt": "",
               "addressPrompt": "", "resolvePrompt": "", "ciFixPrompt": "",
               "botLogin": [], "botIgnore": [], "botIgnoreBody": [], "implementAllow": [],
               "flags": {"suppressions": ["shellcheck\\s+disable", "eslint-disable", "#\\s*noqa", "@ts-ignore", "@ts-expect-error",
                                          "#\\s*type:\\s*ignore", "//\\s*nolint", "rubocop:disable", "#!?\\[allow\\("],
                         "skips": ["(^|[;&|])\\s*skip(\\s|$)","\\.skip\\(", "\\bxit\\(", "\\bxdescribe\\(", "\\bt\\.Skip(f|Now)?\\(",
                                   "@pytest\\.mark\\.skip", "^\\s*pending(\\s|\\(|$)"],
                         "testFiles": ["*_test.*", "*.test.*", "*.bats", "test/", "spec/", "tests/"]}}}'
PRC_SKELETON='{"harness": "claude", "harnesses": {"claude": {}, "pi": {}}, "steps": {}, "settings": {}}'

# A step is a sd act verb that runs an agent: add new ones here. The global-only
# settings are the watcher's, which reads no branch file.
PRC_HARNESSES=(claude pi)
PRC_STEPS=(address ci-fix describe implement review)
PRC_GLOBAL_ONLY=(remote interval notify vacuumAfter rateLimitFloor)
PRC_SETTINGS='stepTimeout?:uint64,stepBudget?:float64,branchBudget?:float64,forkLimit?:uint64,reviewRounds?:uint64,replyPrefix?:string,verify?:string,autofix?:string,localReview?:string,botCommand?:string,botTimeout?:uint64,
              reviewPrompt?:string,implementPrompt?:string,reviewFixPrompt?:string,describePrompt?:string,
              addressPrompt?:string,resolvePrompt?:string,ciFixPrompt?:string,
              botLogin?:[string],botIgnore?:[string],botIgnoreBody?:[string],implementAllow?:[string],protectedPaths?:[string],
              flags?:{suppressions?:[string],skips?:[string],testFiles?:[string]}'
PRC_TYPES='type hclaude = {model?:string,effort?:enum(low,medium,high,xhigh,max),args?:string,useApiKey?:bool}
type hpi = {provider?:string,model?:string,effort?:enum(off,minimal,low,medium,high,xhigh,max),args?:string}
type harness = enum(claude,pi)
type step = {harness?:harness,claude?:hclaude,pi?:hpi}
type steps = {'"$(printf '"%s"?:step,' "${PRC_STEPS[@]}" | sed 's/,$//')"'}
type branch_settings = {'"$PRC_SETTINGS"'}
type global_settings = {remote?:string,interval?:uint64,notify?:string,vacuumAfter?:uint64,rateLimitFloor?:uint64,'"$PRC_SETTINGS"'}'

# One line per problem: a key the layer's type doesn't have, or a value that
# doesn't cast. cast drops unknown keys silently, hence the diff of the input's
# leaves against the cast's. An empty block ({}) has no leaves, and flatten()
# and fields() both drop it, so the third branch reads those paths off the
# type instead (typeof keeps them): an empty block is unknown unless it is,
# or is a prefix of, a path in the cast's type, which cast fills in for a
# known block. Only the unknown ones are emitted, so a known path's cast
# error (harness: {}) still shows. The replace runs over the type's text,
# so a key named like "a:{}" would be misnamed in the message.
PRC_CHECK='fn mt(v): parse_sup(replace(typeof(v)::string, ":{}", ":null"))
fn paths(v): coalesce([unnest fields(v) | values join(this, ".")], [])
values {r: this, cp: paths(mt(cast(this, <config>)))}
| fork
    ( unnest flatten(r) | values {k: join(key, "."), ok: false, err: ""} )
    ( unnest flatten(cast(r, <config>))
      | values {k: join(key, "."), ok: not is_error(value),
                err: is_error(value) ? f"{under(value).message}: {under(value).on}" : ""} )
    ( unnest {cp, k: coalesce([unnest {ls: paths(typeof(r)), p: paths(mt(r))} | where not (p in ls) | values p], [])}
      | where len(coalesce([unnest {k, c: cp} | where c == k or c[0:len(k)+1] == f"{k}." | values c], [])) == 0
      | values {k, ok: false, err: ""} )
| aggregate ok:=or(ok), errs:=collect(err) by k
| where not ok | sort k
| values {k, err: join(errs, "")}
| values err != "" ? f"{k}: {err}" : k in GLOBAL_ONLY_ ? f"{k}: global file only" : f"{k}: unknown key"'

# Deep merge of {i, v} layers: a leaf from a higher i replaces every leaf at,
# above or below its path in lower layers. Lists are leaves; nulls are kept.
# unflatten needs its keys grouped, hence the sort: keys come out alphabetical.
# No `unnest ... into`: its subquery panicked ("context canceled") in 12 of 200
# runs on super 0.3.0.
PRC_MERGE='unnest {i, kv: flatten(v)} | values {i, k: kv.key, v: kv.value} WHERE_
| aggregate es:=collect(this)
| unnest {es, e: es}
| unnest {e, f: es}
| values {e, s: f.i > e.i and (f.k == e.k[0:len(f.k)] or e.k == f.k[0:len(e.k)])}
| aggregate shadowed:=or(s) by e
| where not shadowed
| sort e.k
| aggregate kv:=collect({key: e.k, value: e.v})
| values unflatten(coalesce(kv, []))'

prc_err() { echo "sd config: $*" >&2; return 1; }

prc_global_file() { echo "${SHEEPDOG_CONFIG:-${XDG_CONFIG_HOME:-$HOME/.config}/sheepdog/config.json}"; }

prc_branch_file() { # <branch>
  local dir name=${1//[%]/%25}
  dir=$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null) || { prc_err "not a git repository"; return; }
  echo "$dir/sheepdog/${name//\//%2F}.json"
}

prc_branch_arg() { # <branch-or-empty>
  local b=$1
  [[ -n $b ]] || b=$(git branch --show-current 2>/dev/null) || b=
  [[ -n $b ]] || { prc_err "detached HEAD; name a branch"; return; }
  echo "$b"
}

# Duplicated from a work repo's bin/lib/superdb.sh. The asdf shim re-resolves on every
# call (~14x slower), so call the pinned install directly; fall back to `super`
# on PATH when no pinned install exists.
prc_superdb_binary() {
  local version=${ASDF_SUPERDB_VERSION:-} bin
  [[ -n $version ]] || version=$(awk '$1 == "superdb" { print $2 }' "$HOME/.tool-versions" 2>/dev/null) || version=
  bin="${ASDF_DATA_DIR:-$HOME/.asdf}/installs/superdb/$version/bin/super"
  if [[ -n $version && -x $bin ]]; then printf '%s' "$bin"; else printf 'super'; fi
}

# Requires 0.3.0, the only version this has run on. Sets PRC_SUPER_BIN, so call
# it outside $( ); otherwise every prc_super call repeats the check.
prc_super_init() {
  local bin v
  bin=$(prc_superdb_binary)
  v=$("$bin" -version 2>/dev/null) || v=
  [[ -n $v ]] || { prc_err "needs super 0.3.0, but $bin didn't run"; return; }
  [[ $v == "Version: v0.3.0" ]] || { prc_err "needs super 0.3.0, but $bin is ${v#Version: }"; return; }
  PRC_SUPER_BIN=$bin
}

prc_super() {
  [[ -n ${PRC_SUPER_BIN:-} ]] || prc_super_init || return
  "$PRC_SUPER_BIN" "$@"
}

# Functions every query may use. A missing field is an error in SuperDB, and
# a null or error compares to nothing, so on() and n() make them false and 0,
# nul() is "null or absent", and sj() joins an empty list (join can't).
PRC_FNS='fn on(x): coalesce(x, false)
fn n(x): coalesce(x, 0)
fn nul(x): coalesce(x, null) is null
fn sj(a, sep): len(a) == 0 ? "" : join(a, sep)'

# The two shapes every pr-* command uses: prc_json <query> [json] and
# prc_line <query> [json] run the JSON document (the argument, else stdin)
# through <query> with PRC_FNS in scope. prc_json prints each result as one
# line of JSON; prc_line prints strings raw (unquoted, one per line; a null
# prints "null", a missing field error("missing"), so coalesce what may be
# absent). super reads nothing from a here-string, so the document is piped.
prc_json() { prc_doc "$@" | prc_super -i json -j -c "$PRC_FNS"$'\n'"$1" -; }
prc_line() { prc_doc "$@" | prc_super -i json -f line -c "$PRC_FNS"$'\n'"$1" -; }
prc_doc() { if (( $# > 1 )); then printf '%s\n' "$2"; else cat; fi; }

# prc_val <json> <expr>: one value of a JSON document, printed raw.
prc_val() { prc_line "values $2" "$1"; }

# prc_strings: the lines on stdin as a JSON list of strings; [] for none.
prc_strings() { prc_super -i line -j -c 'aggregate a:=collect(this) | values coalesce(a, [])' -; }

# prc_problems <global|branch> <name>: a layer's JSON on stdin; reports each
# problem as "<name>: <problem>" and returns 1 if there are any.
prc_problems() {
  local out line only
  only=$(printf '"settings.%s",' "${PRC_GLOBAL_ONLY[@]}")
  out=$(prc_super -i json -f line -c "$PRC_TYPES
    type config = {harness?:harness,harnesses?:{claude?:hclaude,pi?:hpi},steps?:steps,settings?:${1}_settings}
    ${PRC_CHECK/GLOBAL_ONLY_/[${only%,}]}" -) || { prc_err "couldn't check $2"; return; }
  [[ -n $out ]] || return 0
  while IFS= read -r line; do prc_err "$2: $line"; done <<<"$out"
  return 1
}

# prc_layer <file> [global|branch]: its JSON, or {} when it doesn't exist.
# Checked against that layer's type when one is named.
prc_layer() {
  local kinds
  [[ -e $1 ]] || { echo '{}'; return 0; }
  kinds=$(prc_super -i json -f line -c 'values kind(this)' "$1" 2>/dev/null) || kinds=
  [[ $kinds == record ]] || { prc_err "not a JSON object: $1"; return; }
  if [[ -n ${2:-} ]]; then cat "$1" | prc_problems "$2" "$1" || return; fi
  cat "$1"
}

# prc_path_expr <dotted.path>: this["a"]["b"]. The names are checked first, so
# they can't carry query syntax.
prc_path_expr() {
  local seg expr=this
  local -a segs
  [[ $1 =~ ^[A-Za-z0-9_-]+(\.[A-Za-z0-9_-]+)*$ ]] \
    || { prc_err "invalid path: '$1' (dotted names, e.g. harnesses.claude.model)"; return; }
  IFS=. read -ra segs <<<"$1"
  for seg in "${segs[@]}"; do expr+="[\"$seg\"]"; done
  echo "$expr"
}

# prc_files <branch-or-empty> <no-branch: true|false>: the global file with the
# branch file merged over it, nulls kept. An empty <branch> means the current
# one, or none when detached or outside a repo; a named one needs a repo.
prc_files() {
  local b=$1 gf g bf bj='{}'
  if [[ $2 == true ]]; then
    b=
  elif [[ -z $b ]]; then
    b=$(git branch --show-current 2>/dev/null) || b=
  fi
  gf=$(prc_global_file)
  g=$(prc_layer "$gf" global) || return
  if [[ -n $b ]]; then
    bf=$(prc_branch_file "$b") || return
    bj=$(prc_layer "$bf" branch) || return
  fi
  printf '{"i":1,"v":%s}\n{"i":2,"v":%s}\n' "$g" "$bj" | prc_super -i json -j -c "${PRC_MERGE/WHERE_/}" -
}

# prc_paths <branch-or-empty> <no-branch: true|false>: "global  <path>" and
# "branch  <path>", with "  (missing)" after a file that doesn't exist.
prc_paths() {
  local b=$1 f why=
  prc_path_line global "$(prc_global_file)"
  [[ $2 != true ]] || return 0
  if [[ -z $b ]]; then
    if ! git rev-parse --git-dir >/dev/null 2>&1; then
      why="not a git repository"
    else
      b=$(git branch --show-current) || return
      [[ -n $b ]] || why="detached HEAD"
    fi
  fi
  if [[ -n $why ]]; then echo "branch  (none: $why)"; return 0; fi
  f=$(prc_branch_file "$b") || return
  prc_path_line branch "$f"
}

prc_path_line() { if [[ -e $2 ]]; then echo "$1  $2"; else echo "$1  $2  (missing)"; fi; }

# prc_get <path> <branch-or-empty> <no-branch: true|false> [json]: one value.
# A string prints raw, a list one element per line, an object as JSON; json
# prints any of them as JSON. A null in a file falls through to the built-in
# default.
prc_get() {
  local expr files kind q op=values
  local -a fmt=(-f line)
  expr=$(prc_path_expr "$1") || return
  files=$(prc_files "$2" "$3") || return
  q="${PRC_MERGE/WHERE_/| where v is not null}"
  kind=$(printf '{"i":0,"v":%s}\n{"i":1,"v":%s}\n' "$PRC_DEFAULTS" "$files" \
    | prc_super -i json -f line -c "$q | values is_error($expr) ? \"missing\" : kind($expr)" -) || return
  [[ $kind != missing ]] || return 3
  if [[ ${4:-} == json || $kind == record ]]; then
    fmt=(-j)
  elif [[ $kind == array ]]; then
    op=unnest
  fi
  printf '{"i":0,"v":%s}\n{"i":1,"v":%s}\n' "$PRC_DEFAULTS" "$files" | prc_super -i json "${fmt[@]}" -c "$q | $op $expr" -
}

# prc_show <branch-or-empty> <no-branch: true|false>: the whole merged config,
# defaults included, as pretty JSON.
prc_show() {
  local files
  files=$(prc_files "$1" "$2") || return
  printf '{"i":0,"v":%s}\n{"i":1,"v":%s}\n' "$PRC_DEFAULTS" "$files" \
    | prc_super -i json -J -c "${PRC_MERGE/WHERE_/| where v is not null}" -
}

# prc_step <step> <branch-or-empty> <forced-harness-or-empty>: what <step> runs
# with, as one JSON object: the step's harness block with the step's block for
# that harness over it. A null in the step block clears the harness block's key.
prc_step() {
  local step=$1 forced=$3 hexpr files
  [[ " ${PRC_STEPS[*]} " == *" $step "* ]] || { prc_err "unknown step: $step (${PRC_STEPS[*]})"; return; }
  hexpr="coalesce(f.steps[\"$step\"].harness, f.harness, d.harness)"
  if [[ -n $forced ]]; then
    [[ " ${PRC_HARNESSES[*]} " == *" $forced "* ]] || { prc_err "unknown harness: $forced (${PRC_HARNESSES[*]})"; return; }
    hexpr="\"$forced\""
  fi
  files=$(prc_files "$2" false) || return
  prc_step_warn "$step" "$hexpr" "$files" || return
  printf '%s\n%s\n' "$PRC_DEFAULTS" "$files" | prc_super -i json -j -c "
    aggregate c:=collect(this) | values {d: c[0], f: c[1]}
    | values {f, h: $hexpr}
    | values {harness: h, ...coalesce(f.harnesses[h], {}), ...coalesce(f.steps[\"$step\"][h], {})}
    | unnest flatten(this) | where value is not null
    | aggregate kv:=collect(this) | values unflatten(kv)" -
}

# prc_step_warn <step> <harness-expr> <files>: effort level names depend on
# the model, so a step block that sets model and leaves effort to the harness
# block may name a level its model doesn't have. Warns on stderr; never fails
# the step. A step block with effort written, even null, is on purpose.
prc_step_warn() {
  local step=$1 hexpr=$2 warn
  warn=$(printf '%s\n%s\n' "$PRC_DEFAULTS" "$3" | prc_super -i json -f line -c "
    aggregate c:=collect(this) | values {d: c[0], f: c[1]}
    | values {f, h: $hexpr}
    | values {h, sb: f.steps[\"$step\"][h], hb: f.harnesses[h]}
    | where coalesce(sb.model, null) is not null and missing(sb.effort) and coalesce(hb.effort, null) is not null
    | values f\"sd config: steps.$step.{h} sets model but inherits effort {hb.effort} from harnesses.{h}; effort levels depend on the model, so set steps.$step.{h}.effort (null clears it)\"" -) || return
  [[ -z $warn ]] || echo "$warn" >&2
}

# prc_target_file <global: true|false> <branch-or-empty>: the file set/unset write.
prc_target_file() {
  local b
  if [[ $1 == true ]]; then prc_global_file; return 0; fi
  b=$(prc_branch_arg "$2") || return
  git rev-parse -q --verify "refs/heads/$b" >/dev/null || { prc_err "no such branch: $b"; return; }
  prc_branch_file "$b"
}

prc_json_string() { # <string>
  local s=$1 t=${1//[$'\n\t\r']/}
  [[ ! $t =~ [[:cntrl:]] ]] || { prc_err "control characters aren't supported in a value"; return; }
  s=${s//\\/\\\\} s=${s//\"/\\\"} s=${s//$'\n'/\\n} s=${s//$'\t'/\\t} s=${s//$'\r'/\\r}
  printf '"%s"' "$s"
}

# prc_write <file> <super query over {doc, v}> <value-json>: rewrite <file>
# through the query. Leaves the file alone on failure.
prc_write() {
  local f=$1 cur='{}' tmp layer=branch
  [[ $f != "$(prc_global_file)" ]] || layer=global
  if [[ -e $f ]]; then cur=$(prc_layer "$f") || return; fi
  mkdir -p "$(dirname "$f")" || return
  tmp=$(mktemp "$f.XXXXXX") || return
  if ! printf '{"doc":%s,"v":%s}\n' "$cur" "$3" | prc_super -i json -J -c "$2" - > "$tmp"; then
    rm -f "$tmp"
    prc_err "couldn't update $f"
    return
  fi
  cat "$tmp" | prc_problems "$layer" "$f" || { rm -f "$tmp"; return 1; }
  mv "$tmp" "$f"
}

prc_set() { # <file> <path> <value> <json: true|false>
  local expr vj kinds parent=this seg ok=true cur
  local -a segs
  expr=$(prc_path_expr "$2") || return
  if [[ $4 == true ]]; then
    kinds=$(printf '%s' "$3" | prc_super -i json -f line -c 'values kind(this)' - 2>/dev/null) || kinds=
    [[ -n $kinds && $kinds != *$'\n'* ]] || { prc_err "not one JSON value: $3"; return; }
    vj=$3
  else
    vj=$(prc_json_string "$3") || return
  fi
  IFS=. read -ra segs <<<"$2"
  for seg in "${segs[@]:0:${#segs[@]}-1}"; do
    parent+="[\"$seg\"]"
    ok+=" and (is_error($parent) or $parent is null or kind($parent) == \"record\")"
  done
  cur=$(prc_layer "$1") || return
  ok=$(printf '%s\n' "$cur" | prc_super -i json -f line -c "values $ok" -) || return
  [[ $ok == true ]] || { prc_err "couldn't set $2 in $1: a parent isn't an object"; return; }
  prc_write "$1" "put this[\"doc\"]${expr#this}:=v | values doc" "$vj"
}

prc_unset() { # <file> <path>
  local expr
  expr=$(prc_path_expr "$2") || return
  prc_write "$1" "drop this[\"doc\"]${expr#this} | values doc" null
}

prc_init_global() { # <force>
  local dest
  dest=$(prc_global_file)
  [[ $1 == true || ! -e $dest ]] || { prc_err "$dest exists; --force overwrites it"; return; }
  mkdir -p "$(dirname "$dest")" || return
  printf '%s\n' "$PRC_SKELETON" | prc_super -i json -J -c 'values this' - > "$dest" || return
  echo "wrote $dest"
}

# prc_init <force> <branch>: a branch file of empty blocks; it holds only overrides.
prc_init() {
  local force=$1 b=$2 dest
  git rev-parse -q --verify "refs/heads/$b" >/dev/null || { prc_err "no such branch: $b"; return; }
  dest=$(prc_branch_file "$b") || return
  [[ $force == true || ! -e $dest ]] || { prc_err "$dest exists; --force overwrites it"; return; }
  mkdir -p "$(dirname "$dest")" || return
  printf '%s\n' "$PRC_SKELETON" | prc_super -i json -J -c 'drop harness' - > "$dest" || return
  echo "wrote $dest"
}
