#!/usr/bin/env bash
# Build sd, link it into ~/.local/bin ($LOCAL_BIN), write the git hooks that
# rebuild it after a pull or a rebase, and install its launchd watcher. Safe
# to re-run. Plain bash: it runs under macOS's 3.2.
set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")" && pwd)"

cargo=${CARGO:-cargo}
cargo_path=$(command -v "$cargo") || { echo "! Cannot install sd: needs cargo to build it (https://rustup.rs)" >&2; exit 1; }
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

# The hooks run the cargo found above, so a git with another PATH (a GUI
# client, a job) still finds it.
mark="# sheepdog's install.sh wrote this hook, and rewrites it on a re-run."
if hooks=$(git -C "$REPO_DIR" rev-parse --path-format=absolute --git-path hooks 2>/dev/null); then
  mkdir -p "$hooks"
  for name in post-merge post-rewrite; do
    hook="$hooks/$name"
    if [[ -e $hook ]] && ! grep -qxF "$mark" "$hook"; then
      echo "! $hook isn't sheepdog's, so it's left alone: sd won't be rebuilt on $name" >&2
      continue
    fi
    printf '#!/usr/bin/env bash\n%s\n%q build --release --quiet --manifest-path %q \\\n  || { echo %q >&2; exit 1; }\n' \
      "$mark" "$cargo_path" "$REPO_DIR/Cargo.toml" "sd: couldn't rebuild sd; run $REPO_DIR/install.sh" > "$hook"
    chmod +x "$hook"
    echo "✓ Wrote $hook, which rebuilds sd"
  done
else
  echo "! $REPO_DIR is not a git checkout: nothing will rebuild sd; re-run ./install.sh after updating it" >&2
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
