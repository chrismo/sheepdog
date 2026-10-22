#!/usr/bin/env bats

bats_require_minimum_version 1.5.0

# Test suite for bin/sd, the one command on PATH: `sd <verb>` runs
# libexec/sd-<verb>, the way git runs git-<verb>.

SD="$BATS_TEST_DIRNAME/sd"
LIBEXEC="$(cd "$BATS_TEST_DIRNAME/../libexec" && pwd)"

setup() {
  TMP="$(cd "$BATS_TEST_TMPDIR" && pwd -P)"
}

# fake_layout — a copy of bin/sd next to a libexec/ holding one probe verb, so
# dispatch can be checked without running a real verb. Its Rust build is up to
# date; the fake cargo on PATH logs to $CARGO_CALLS and "builds" by creating
# target/release/sd, unless $CARGO_EXIT says it failed.
fake_layout() {
  mkdir -p "$TMP/x/bin" "$TMP/x/libexec" "$TMP/x/src" "$TMP/x/target/release" "$TMP/fakebin"
  cp "$SD" "$TMP/x/bin/sd"
  touch -t 202601010000 "$TMP/x/src/main.rs" "$TMP/x/Cargo.toml" "$TMP/x/Cargo.lock"
  printf '#!/bin/sh\n' > "$TMP/x/target/release/sd"
  chmod +x "$TMP/x/target/release/sd"
  export CARGO_CALLS="$TMP/cargo-calls"
  cat > "$TMP/fakebin/cargo" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$CARGO_CALLS"
(( ${CARGO_EXIT:-0} == 0 )) || exit "$CARGO_EXIT"
printf '#!/bin/sh\n' > "${5%/Cargo.toml}/target/release/sd"
chmod +x "${5%/Cargo.toml}/target/release/sd"
EOF
  chmod +x "$TMP/fakebin/cargo"
  PATH="$TMP/fakebin:$PATH"
  cat > "$TMP/x/libexec/sd-probe" <<'EOF'
#!/usr/bin/env bash
printf 'arg:%s\n' "$@"
printf 'path:%s\n' "${PATH%%:*}"
exit "${PROBE_EXIT:-0}"
EOF
  chmod +x "$TMP/x/libexec/sd-probe"
}

@test "runs under bash 5 via the bash5 shebang" {
  run head -1 "$SD"
  [ "$output" = "#!/usr/bin/env bash5" ]
}

@test "no verb: usage on stderr, nonzero" {
  run --separate-stderr "$SD"
  [ "$status" -ne 0 ]
  [ -z "$output" ]
  [[ "$stderr" == *"Usage: sd <verb>"* ]]
}

@test "help, -h and --help: usage on stdout, zero" {
  local h
  for h in help -h --help; do
    run --separate-stderr "$SD" "$h"
    [ "$status" -eq 0 ]
    [[ "$output" == *"Usage: sd <verb>"* ]] || false
  done
}

@test "usage names every verb in libexec" {
  run "$SD" --help
  local f
  for f in "$LIBEXEC"/sd-*; do
    [[ $f != *.bats ]] || continue
    [[ "$output" == *"  ${f##*/sd-} "* ]] || false
  done
}

@test "unknown verb fails" {
  run "$SD" frobnicate
  [ "$status" -ne 0 ]
  [[ "$output" == *"sd: unknown verb: frobnicate"* ]]
}

@test "a verb that is a path is refused" {
  fake_layout
  run "$TMP/x/bin/sd" ../libexec/sd-probe
  [ "$status" -ne 0 ]
  [[ "$output" == *"unknown verb"* ]]
}

@test "a test file is not a verb" {
  fake_layout
  touch "$TMP/x/libexec/sd-probe.bats"
  run "$TMP/x/bin/sd" probe.bats
  [ "$status" -ne 0 ]
  [[ "$output" == *"unknown verb"* ]]
}

@test "dispatches to libexec/sd-<verb> with the remaining args" {
  fake_layout
  run "$TMP/x/bin/sd" probe one 'two words'
  [ "$status" -eq 0 ]
  [ "${lines[0]}" = "arg:one" ]
  [ "${lines[1]}" = "arg:two words" ]
}

@test "passes the verb's exit status through" {
  fake_layout
  PROBE_EXIT=3 run "$TMP/x/bin/sd" probe
  [ "$status" -eq 3 ]
}

@test "puts libexec first on PATH, so verbs find each other" {
  fake_layout
  run "$TMP/x/bin/sd" probe
  [ "${lines[1]}" = "path:$TMP/x/libexec" ]
}

@test "run through a symlink, it still finds its libexec" {
  fake_layout
  mkdir -p "$TMP/local-bin"
  ln -s "$TMP/x/bin/sd" "$TMP/local-bin/sd"
  run "$TMP/local-bin/sd" probe hi
  [ "$status" -eq 0 ]
  [ "${lines[0]}" = "arg:hi" ]
}

@test "the real verbs are reachable" {
  run "$SD" config --help
  [ "$status" -eq 0 ]
  [[ "$output" == *"Usage"* ]]
}

# ── the Rust build: rebuilt when missing or older than its source (#193) ────

@test "an up-to-date build isn't rebuilt" {
  fake_layout
  run "$TMP/x/bin/sd" probe
  [ "$status" -eq 0 ]
  [ ! -e "$CARGO_CALLS" ]
}

@test "a missing build is built before the verb runs" {
  fake_layout
  rm "$TMP/x/target/release/sd"
  run --separate-stderr "$TMP/x/bin/sd" probe hi
  [ "$status" -eq 0 ]
  [ "$(cat "$CARGO_CALLS")" = "build --release --quiet --manifest-path $TMP/x/Cargo.toml" ]
  [ "${lines[0]}" = "arg:hi" ]
}

@test "a source, Cargo.toml or Cargo.lock newer than the build is rebuilt, once" {
  fake_layout
  local f
  for f in src/main.rs Cargo.toml Cargo.lock; do
    rm -f "$CARGO_CALLS"
    touch -t 202601010000 "$TMP/x/target/release/sd"
    touch "$TMP/x/$f"
    run "$TMP/x/bin/sd" probe
    [ "$status" -eq 0 ]
    [ "$(wc -l < "$CARGO_CALLS" | tr -d ' ')" = 1 ] || { echo "$f not rebuilt"; return 1; }
    run "$TMP/x/bin/sd" probe
    [ "$(wc -l < "$CARGO_CALLS" | tr -d ' ')" = 1 ] || { echo "$f rebuilt twice"; return 1; }
  done
}

@test "a failed build stops before the verb, naming install.sh" {
  fake_layout
  rm "$TMP/x/target/release/sd"
  CARGO_EXIT=101 run "$TMP/x/bin/sd" probe
  [ "$status" -eq 1 ]
  [[ "$output" != *"arg:"* ]]
  [[ "$output" == *"couldn't build"*"$TMP/x/install.sh"* ]]
}

@test "no cargo: stops the same way" {
  fake_layout
  rm "$TMP/x/target/release/sd"
  CARGO="$TMP/no-such-cargo" run "$TMP/x/bin/sd" probe
  [ "$status" -eq 1 ]
  [[ "$output" == *"couldn't build"*"$TMP/x/install.sh"* ]]
}

@test "help needs no build" {
  fake_layout
  rm "$TMP/x/target/release/sd"
  CARGO_EXIT=101 run "$TMP/x/bin/sd" --help
  [ "$status" -eq 0 ]
  [ ! -e "$CARGO_CALLS" ]
}
