#!/usr/bin/env bash
# Build sd's Rust verbs, link sd into ~/.local/bin ($LOCAL_BIN) and install
# its launchd watcher. Safe to re-run. Plain bash: it runs under macOS's 3.2.
set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")" && pwd)"

cargo=${CARGO:-cargo}
command -v "$cargo" >/dev/null || { echo "! Cannot install sd: needs cargo to build it (https://rustup.rs)" >&2; exit 1; }
"$cargo" build --release --quiet --manifest-path "$REPO_DIR/Cargo.toml" \
  || { echo "! Cannot install sd: cargo build failed" >&2; exit 1; }
echo "✓ Built sd's Rust verbs"

LOCAL_BIN="${LOCAL_BIN:-$HOME/.local/bin}"
mkdir -p "$LOCAL_BIN"
LOCAL_BIN="$(cd "$LOCAL_BIN" && pwd)"
export LOCAL_BIN

dest="$LOCAL_BIN/sd"
if [[ -e $dest && ! -L $dest ]]; then
  echo "! Cannot install sd: $dest exists and is not a symlink" >&2
  exit 1
else
  ln -sfn "$REPO_DIR/bin/sd" "$dest"
  echo "✓ Linked sd -> $LOCAL_BIN/"
fi

if [[ $(uname -s) == Darwin ]]; then
  "$dest" watch install || {
    status=$?
    retry=$(printf '%q watch install' "$dest")
    [[ $LOCAL_BIN == "$HOME/.local/bin" ]] || retry="LOCAL_BIN=$(printf %q "$LOCAL_BIN") $retry"
    echo "! sd is linked, but watcher installation failed; retry with: $retry" >&2
    exit "$status"
  }
else
  echo "! Background watcher requires macOS; sd is linked"
fi
