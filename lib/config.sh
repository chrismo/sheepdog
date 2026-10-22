# SuperDB queries and config reads for the bash verbs, which are all Rust now
# (#203): ci/super-stress, lib/sheepdog.sh and tests/state.rs still source
# it. Needs bash 5 and super 0.3.0. SuperDB's edges:
# AGENTS.md, "SuperDB notes".
#
# Every function returns 1 on an error (message on stderr) and never exits:
# callers run them as `x=$(prc_...) || die`, where errexit is off inside, so
# each step checks its own status. prc_get returns 3 for a path with no value
# and no default.
#
# super reads nothing from a here-string or a < redirect (super 0.3.0), so input
# is always piped.

PRC_SD_CONFIG="$(cd "$(dirname "${BASH_SOURCE[0]}")/../libexec" && pwd)/sd-config"
PRC_SD_STATE="${PRC_SD_CONFIG%/*}/sd-state"

prc_err() { echo "sd config: $*" >&2; return 1; }

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

# prc_get <path> <branch-or-empty> <no-branch: true|false> [json]
prc_get() {
  local -a opts=()
  [[ $3 != true ]] || opts+=(--no-branch)
  [[ ${4:-} != json ]] || opts+=(--json)
  "$PRC_SD_CONFIG" get "${opts[@]}" -- "$1" ${2:+"$2"}
}

# prc_step <step> <branch-or-empty> <forced-harness-or-empty>
prc_step() { "$PRC_SD_CONFIG" step --harness "$3" -- "$1" ${2:+"$2"}; }

# prn_next <remote>: sd state JSON on stdin; prints NEXT's steps as a JSON
# array, first is next (src/next.rs, as sd state runs them).
prn_next() { "$PRC_SD_STATE" --next "$1"; }

prc_json_string() { # <string>
  local s=$1 t=${1//[$'\n\t\r']/}
  [[ ! $t =~ [[:cntrl:]] ]] || { prc_err "control characters aren't supported in a value"; return; }
  s=${s//\\/\\\\} s=${s//\"/\\\"} s=${s//$'\n'/\\n} s=${s//$'\t'/\\t} s=${s//$'\r'/\\r}
  printf '"%s"' "$s"
}
