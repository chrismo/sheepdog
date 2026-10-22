#!/usr/bin/env bats

# The fixture sd records watcher installation without touching launchd.
OLD_COMMANDS=(pr-state pr-act pr-watch pr-config pr-review-local)

setup() {
  TEST_DIR="$(mktemp -d "${TMPDIR:-/tmp}/sheepdog-install.XXXXXX")"
  TEST_DIR="$(cd "$TEST_DIR" && pwd -P)"
  export LOCAL_BIN="$TEST_DIR/local-bin"
  export WATCH_CALLS="$TEST_DIR/watch-calls"
  INSTALLER="$TEST_DIR/repo/install.sh"
  mkdir -p "$TEST_DIR/repo/bin" "$TEST_DIR/fakebin"
  cat > "$TEST_DIR/fakebin/uname" <<'EOF'
#!/usr/bin/env bash5
printf '%s\n' "${TEST_OS:-Darwin}"
EOF
  chmod +x "$TEST_DIR/fakebin/uname"
  export CARGO_CALLS="$TEST_DIR/cargo-calls"
  cat > "$TEST_DIR/fakebin/cargo" <<'EOF'
#!/usr/bin/env bash5
printf '%s\n' "$*" >> "$CARGO_CALLS"
exit "${CARGO_EXIT:-0}"
EOF
  chmod +x "$TEST_DIR/fakebin/cargo"
  export PATH="$TEST_DIR/fakebin:$PATH"
  cp "$BATS_TEST_DIRNAME/install.sh" "$INSTALLER"
  cat > "$TEST_DIR/repo/bin/sd" <<'EOF'
#!/usr/bin/env bash5
[[ -L $LOCAL_BIN/sd ]] || exit 99
printf '%s\n' "$*" >> "$WATCH_CALLS"
exit "${WATCH_EXIT:-0}"
EOF
  chmod +x "$TEST_DIR/repo/bin/sd"
}

teardown() {
  rm -rf "$TEST_DIR"
}

@test "links sd into LOCAL_BIN" {
  run "$INSTALLER"
  [ "$status" -eq 0 ]
  [ -L "$LOCAL_BIN/sd" ]
  [ "$(readlink "$LOCAL_BIN/sd")" = "$TEST_DIR/repo/bin/sd" ]
}

@test "builds the Rust verbs in the repo before linking (#193)" {
  run "$INSTALLER"
  [ "$status" -eq 0 ]
  [ "$(cat "$CARGO_CALLS")" = "build --release --quiet --manifest-path $TEST_DIR/repo/Cargo.toml" ]
  [ -L "$LOCAL_BIN/sd" ]
}

@test "a failed build links nothing and installs no watcher" {
  export CARGO_EXIT=101
  run "$INSTALLER"
  [ "$status" -eq 1 ]
  [ ! -e "$LOCAL_BIN/sd" ]
  [ ! -e "$WATCH_CALLS" ]
  [[ "$output" == *"cargo build failed"* ]]
}

@test "no cargo: says what's needed and links nothing" {
  export CARGO="$TEST_DIR/no-such-cargo"
  run "$INSTALLER"
  [ "$status" -eq 1 ]
  [ ! -e "$LOCAL_BIN/sd" ]
  [[ "$output" == *"needs cargo"*"rustup.rs"* ]]
}

@test "links nothing else: the verbs are reached through sd" {
  run "$INSTALLER"
  [ "$status" -eq 0 ]
  [ "$(ls "$LOCAL_BIN")" = sd ]
}

@test "replaces a link left by another checkout" {
  mkdir -p "$LOCAL_BIN"
  ln -s /somewhere/else/bin/sd "$LOCAL_BIN/sd"
  run "$INSTALLER"
  [ "$status" -eq 0 ]
  [ "$(readlink "$LOCAL_BIN/sd")" = "$TEST_DIR/repo/bin/sd" ]
}

@test "a regular file blocks installation without being overwritten or installing the watcher" {
  mkdir -p "$LOCAL_BIN"
  echo mine > "$LOCAL_BIN/sd"
  ln -s /somewhere/pr-watch "$LOCAL_BIN/pr-watch"
  run "$INSTALLER"
  [ "$status" -ne 0 ]
  [ ! -e "$WATCH_CALLS" ]
  [ ! -L "$LOCAL_BIN/sd" ]
  [ "$(cat "$LOCAL_BIN/sd")" = mine ]
  [ "$(readlink "$LOCAL_BIN/pr-watch")" = /somewhere/pr-watch ]
  [[ "$output" == *"sd"*"not a symlink"* ]]
}

@test "installs the watcher by default after linking sd" {
  run "$INSTALLER"
  [ "$status" -eq 0 ]
  [ "$(cat "$WATCH_CALLS")" = "watch install" ]
}

@test "reinstalling sd reinstalls the watcher" {
  "$INSTALLER"
  run "$INSTALLER"
  [ "$status" -eq 0 ]
  [ "$(wc -l < "$WATCH_CALLS" | tr -d ' ')" -eq 2 ]
}

@test "the watcher is installed for the link the installer made" {
  unset LOCAL_BIN
  export HOME="$TEST_DIR/home"
  run "$INSTALLER"
  [ "$status" -eq 0 ]
  [ -L "$HOME/.local/bin/sd" ]
  [ "$(cat "$WATCH_CALLS")" = "watch install" ]
}

@test "a relative LOCAL_BIN is linked and handed on as an absolute path" {
  cd "$TEST_DIR"
  export LOCAL_BIN=local-bin WATCH_EXIT=1
  run "$INSTALLER"
  [ "$status" -eq 1 ]
  [ -L "$TEST_DIR/local-bin/sd" ]
  [[ "$output" == *"retry with: LOCAL_BIN=$TEST_DIR/local-bin $TEST_DIR/local-bin/sd watch install" ]]
}

@test "a watcher installation failure fails the installer" {
  export WATCH_EXIT=42
  run "$INSTALLER"
  [ "$status" -eq 42 ]
  [ "$(cat "$WATCH_CALLS")" = "watch install" ]
  [[ "$output" == *"linked"*"watcher installation failed"*"LOCAL_BIN=$LOCAL_BIN $LOCAL_BIN/sd watch install"* ]]
}

@test "the retry hint for the default LOCAL_BIN is the plain command" {
  unset LOCAL_BIN
  export HOME="$TEST_DIR/home" WATCH_EXIT=1
  run "$INSTALLER"
  [ "$status" -eq 1 ]
  [[ "$output" == *"retry with: $HOME/.local/bin/sd watch install" ]]
}

@test "on Linux links sd and explains that the background watcher requires macOS" {
  export TEST_OS=Linux
  run "$INSTALLER"
  [ "$status" -eq 0 ]
  [ -L "$LOCAL_BIN/sd" ]
  [ ! -e "$WATCH_CALLS" ]
  [[ "$output" == *"watcher requires macOS"* ]]
}

@test "removes pr-* links from a pr-tend install" {
  mkdir -p "$LOCAL_BIN"
  local cmd
  for cmd in "${OLD_COMMANDS[@]}"; do
    ln -s "/somewhere/pr-tend/bin/$cmd" "$LOCAL_BIN/$cmd"
  done
  run "$INSTALLER"
  [ "$status" -eq 0 ]
  for cmd in "${OLD_COMMANDS[@]}"; do
    [ ! -L "$LOCAL_BIN/$cmd" ]
  done
  [[ "$output" == *"Removed pr-act"* ]]
}

@test "leaves a regular file with an old command's name alone" {
  mkdir -p "$LOCAL_BIN"
  echo mine > "$LOCAL_BIN/pr-state"
  run "$INSTALLER"
  [ "$status" -eq 0 ]
  [ "$(cat "$LOCAL_BIN/pr-state")" = mine ]
}
