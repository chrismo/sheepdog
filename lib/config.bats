#!/usr/bin/env bats

bats_require_minimum_version 1.5.0

# Test suite for lib/config.sh, which the bash verbs source: its prc_get /
# prc_step wrappers around libexec/sd-config and prn_next around
# libexec/sd-state, its SuperDB queries, and the pinned super they run. sd
# config's and sd state's own tests are Rust: tests/config.rs, tests/state.rs.
# Build first: cargo build --release.

SD_CONFIG="$BATS_TEST_DIRNAME/../libexec/sd-config"
LIBEXEC="$BATS_TEST_DIRNAME/../libexec"

setup() {
  TMP="$(cd "$BATS_TEST_TMPDIR" && pwd -P)"
  export GIT_CONFIG_GLOBAL="$TMP/gitconfig-global"
  export GIT_CONFIG_SYSTEM=/dev/null
  printf '[user]\n\temail = t@example.com\n\tname = Tester\n[init]\n\tdefaultBranch = main\n' > "$GIT_CONFIG_GLOBAL"
  export SHEEPDOG_CONFIG="$TMP/global.json"
  git init -q "$TMP/repo"
  cd "$TMP/repo"
  git commit -q --allow-empty -m init
  git switch -qc feat
}

write_global() { printf '%s\n' "$1" > "$SHEEPDOG_CONFIG"; }

GLOBAL='{"harness":"claude",
  "harnesses":{"claude":{"model":"strong","effort":"high"},
               "pi":{"provider":"openrouter","model":"cheap","effort":"medium"}},
  "steps":{"address":{"harness":"pi","pi":{"model":"cheaper","effort":null}},
           "review":{"claude":{"effort":"max"}}}}'

@test "is a link to the Rust binary, which runs as the verb it's named for" {
  [ "$(readlink "$SD_CONFIG")" = ../target/release/sd ]
  [ -x "$SD_CONFIG" ]
}

@test "sd-state: is a link to the Rust binary, which runs as the verb it's named for" {
  [ "$(readlink "$LIBEXEC/sd-state")" = ../target/release/sd ]
  [ -x "$LIBEXEC/sd-state" ]
}

@test "sd-watch: is a link to the Rust binary, which runs as the verb it's named for" {
  [ "$(readlink "$LIBEXEC/sd-watch")" = ../target/release/sd ]
  [ -x "$LIBEXEC/sd-watch" ]
}

# ── the super binary (lib/config.sh's queries): pinned asdf install, not the shim

# super_query — one query through lib/config.sh, as the bash verbs run them.
# sd config itself is Rust and runs no super.
super_query() {
  bash5 -c 'source "$1" && prc_json "values 1" "{}"' _ "$BATS_TEST_DIRNAME/../lib/config.sh"
}

# fake_super <dir> <label> — a super that logs its label, then runs the real one.
fake_super() {
  mkdir -p "$1"
  printf '#!/usr/bin/env bash\nprintf "%s\\n" >> "$SUPER_LOG"\nexec "$REAL_SUPER" "$@"\n' "$2" > "$1/super"
  chmod +x "$1/super"
}

pin_setup() {
  export REAL_SUPER SUPER_LOG="$TMP/super-calls" ASDF_DATA_DIR="$TMP/asdf"
  # A real 0.3.0 binary, not the asdf shim: the shim breaks once HOME or
  # ASDF_DATA_DIR move.
  REAL_SUPER=
  local c
  for c in /opt/homebrew/bin/super "$HOME"/.asdf/installs/superdb/*/bin/super; do
    [[ -x $c && $("$c" -version 2>/dev/null) == "Version: v0.3.0" ]] || continue
    REAL_SUPER=$c
    break
  done
  [[ -n $REAL_SUPER ]] || skip "no real super 0.3.0 binary on this machine"
  fake_super "$TMP/decoy" shim
  PATH="$TMP/decoy:$PATH"
  write_global "$GLOBAL"
}

@test "super: the pinned asdf install is used, not the PATH shim" {
  pin_setup
  fake_super "$ASDF_DATA_DIR/installs/superdb/9.9.9/bin" pinned
  ASDF_SUPERDB_VERSION=9.9.9 run super_query
  [ "$status" -eq 0 ]
  [ "$(sort -u "$SUPER_LOG")" = pinned ]
}

@test "super: the version comes from ~/.tool-versions when the env var is unset" {
  pin_setup
  fake_super "$ASDF_DATA_DIR/installs/superdb/9.9.9/bin" pinned
  mkdir -p "$TMP/home"
  printf 'nodejs 1.0.0\nsuperdb 9.9.9\n' > "$TMP/home/.tool-versions"
  HOME="$TMP/home" run super_query
  [ "$status" -eq 0 ]
  [ "$(sort -u "$SUPER_LOG")" = pinned ]
}

@test "super: the env var wins over ~/.tool-versions" {
  pin_setup
  fake_super "$ASDF_DATA_DIR/installs/superdb/9.9.9/bin" pinned
  fake_super "$ASDF_DATA_DIR/installs/superdb/1.1.1/bin" wrong
  mkdir -p "$TMP/home"
  printf 'superdb 1.1.1\n' > "$TMP/home/.tool-versions"
  HOME="$TMP/home" ASDF_SUPERDB_VERSION=9.9.9 run super_query
  [ "$(sort -u "$SUPER_LOG")" = pinned ]
}

@test "super: no installed pin falls back to super on PATH" {
  pin_setup
  ASDF_SUPERDB_VERSION=9.9.9 run super_query
  [ "$status" -eq 0 ]
  [ "$(sort -u "$SUPER_LOG")" = shim ]
}

# old_super <dir> <version> — a super that reports <version>, otherwise the real one.
old_super() {
  mkdir -p "$1"
  printf '#!/usr/bin/env bash\n[[ $1 == -version ]] && { echo "Version: v%s"; exit 0; }\nexec "$REAL_SUPER" "$@"\n' "$2" > "$1/super"
  chmod +x "$1/super"
}

@test "super: any version but 0.3.0 is refused, naming the one found" {
  pin_setup
  old_super "$ASDF_DATA_DIR/installs/superdb/0.2.0/bin" 0.2.0
  ASDF_SUPERDB_VERSION=0.2.0 run super_query
  [ "$status" -eq 1 ]
  [ "$output" = "sd config: needs super 0.3.0, but $ASDF_DATA_DIR/installs/superdb/0.2.0/bin/super is v0.2.0" ]
}

@test "super: one that won't run is refused" {
  pin_setup
  mkdir -p "$TMP/broken"
  printf '#!/usr/bin/env bash\nexit 1\n' > "$TMP/broken/super"
  chmod +x "$TMP/broken/super"
  mkdir -p "$TMP/home"
  HOME="$TMP/home" PATH="$TMP/broken:$PATH" run super_query
  [ "$status" -eq 1 ]
  [ "$output" = "sd config: needs super 0.3.0, but super didn't run" ]
}

@test "super: every bash command checks the version before reading config" {
  pin_setup
  old_super "$ASDF_DATA_DIR/installs/superdb/0.2.0/bin" 0.2.0
  mkdir -p "$TMP/home"
  local cmd
  for cmd in "sd-act address"; do
    HOME="$TMP/home" SHEEPDOG_STATE_DIR="$TMP/state" STATUS_DIR="$TMP/status" ASDF_SUPERDB_VERSION=0.2.0 \
      run "$LIBEXEC"/$cmd
    [ "$status" -eq 1 ]
    [[ "$output" == *"needs super 0.3.0, but "*" is v0.2.0"* ]]
  done
  [ ! -e "$TMP/state" ]
}

@test "super: the version is checked once per command, not per query" {
  pin_setup
  fake_super "$ASDF_DATA_DIR/installs/superdb/0.3.0/bin" pinned
  printf '#!/usr/bin/env bash\nprintf "%%s\\n" "$1" >> "$SUPER_LOG"\nexec "$REAL_SUPER" "$@"\n' \
    > "$ASDF_DATA_DIR/installs/superdb/0.3.0/bin/super"
  ASDF_SUPERDB_VERSION=0.3.0 run bash5 -c 'source "$1" && prc_super_init &&
    for q in 1 2 3 4; do prc_json "values $q" "{}"; done' _ "$BATS_TEST_DIRNAME/../lib/config.sh"
  [ "$status" -eq 0 ]
  [ "$(grep -c -- -version "$SUPER_LOG")" -eq 1 ]
  [ "$(wc -l < "$SUPER_LOG")" -gt 3 ]
}

@test "super: help needs no super" {
  pin_setup
  old_super "$ASDF_DATA_DIR/installs/superdb/0.2.0/bin" 0.2.0
  local cmd
  for cmd in sd-config sd-state sd-act; do
    ASDF_SUPERDB_VERSION=0.2.0 run "$LIBEXEC/$cmd" --help
    [ "$status" -eq 0 ]
  done
}

@test "super: no pin at all falls back to super on PATH" {
  pin_setup
  mkdir -p "$TMP/home"
  HOME="$TMP/home" run super_query
  [ "$status" -eq 0 ]
  [ "$(sort -u "$SUPER_LOG")" = shim ]
}

# ── lib/config.sh: sourced by sd act, sd watch and sd state ─────────────

# Callers use `x=$(prc_... ) || die`, where bash turns errexit off inside the
# function, so every failure has to come back as a status. bats can't fail a
# test while errexit is off, so these collect statuses and assert after set -e.
@test "lib: errors come back as status 1 with errexit off" {
  source "$BATS_TEST_DIRNAME/../lib/config.sh"
  local s=()
  set +e
  printf '{not json\n' > "$SHEEPDOG_CONFIG"
  out=$(prc_get settings.verify feat false); s+=($?)
  out=$(prc_step describe feat ""); s+=($?)
  rm "$SHEEPDOG_CONFIG"
  cd "$TMP"
  out=$(prc_step describe somebranch ""); s+=($?)
  out=$(prc_get 'a..b' "" true); s+=($?)
  set -e
  [ "${s[*]}" = "1 1 1 1" ]
}

@test "lib: unset is status 3, a value is status 0" {
  source "$BATS_TEST_DIRNAME/../lib/config.sh"
  local s=() remote bots
  set +e
  out=$(prc_get settings.foo feat false); s+=($?)
  remote=$(prc_get settings.remote feat false); s+=($?)
  bots=$(prc_get settings.botLogin feat false json); s+=($?)
  set -e
  [ "${s[*]}" = "3 0 0" ]
  [ "$remote" = origin ]
  [ "$bots" = '[]' ]
}

@test "lib: prc_get with no branch reads the repo file inside a repo, the global file outside (#194)" {
  source "$BATS_TEST_DIRNAME/../lib/config.sh"
  write_global '{"settings":{"remote":"upstream"}}'
  "$SD_CONFIG" set --repo settings.remote fork
  [ "$(prc_get settings.remote "" true)" = fork ]
  [ "$(cd "$TMP" && prc_get settings.remote "" true)" = upstream ]
}

@test "lib: prc_json runs a query over piped JSON and prints each result as one JSON line" {
  source "$BATS_TEST_DIRNAME/../lib/config.sh"
  [ "$(printf '{"a":[1,2],"b":"x"}\n' | prc_json 'values {n: len(a), b}, b')" = $'{"n":2,"b":"x"}\n"x"' ]
}

@test "lib: prc_line prints strings raw, one per line, and an empty string as an empty line" {
  source "$BATS_TEST_DIRNAME/../lib/config.sh"
  [ "$(printf '{"s":"a b","e":"","n":3}\n' | prc_line 'values s, e, n' | wc -l | tr -d ' ')" = 3 ]
  [ "$(printf '{"s":"a b","e":"","n":3}\n' | prc_line 'values s')" = "a b" ]
  [ "$(printf '{"s":"a b","e":"","n":3}\n' | prc_line 'values n')" = 3 ]
}

@test "lib: prc_json and prc_line take the document as a second argument instead of stdin" {
  source "$BATS_TEST_DIRNAME/../lib/config.sh"
  [ "$(prc_json 'values {n: len(a)}' '{"a":[1,2]}')" = '{"n":2}' ]
  [ "$(prc_line 'values s' '{"s":"raw text"}')" = "raw text" ]
  [ "$(prc_line 'values s' '{"s":"raw text"}' < /dev/null)" = "raw text" ]
}

@test "lib: prc_val prints one value of a document; prc_strings lists lines, or [] for none" {
  source "$BATS_TEST_DIRNAME/../lib/config.sh"
  [ "$(prc_val '{"a":{"b":"x y"}}' a.b)" = "x y" ]
  [ "$(prc_val '{"a":null}' 'coalesce(a, "none")')" = none ]
  [ "$(printf 'one\ntwo 2\n' | prc_strings)" = '["one","two 2"]' ]
  [ "$(printf '' | prc_strings)" = '[]' ]
}

@test "lib: prc_json and prc_line fail on input that isn't JSON" {
  source "$BATS_TEST_DIRNAME/../lib/config.sh"
  local s=()
  set +e
  out=$(printf 'nope\n' | prc_json 'values this' 2>/dev/null); s+=($?)
  out=$(printf 'nope\n' | prc_line 'values this' 2>/dev/null); s+=($?)
  set -e
  [ "${s[*]}" = "1 1" ]
}

@test "lib: sourcing it runs nothing and leaves the shell options alone" {
  run bash -c 'set +e; source "$1"; echo "opts=$-"' _ "$BATS_TEST_DIRNAME/../lib/config.sh"
  [ "$status" -eq 0 ]
  [[ "$output" == opts=* ]]
  [[ "${output#opts=}" != *e* ]]
}
