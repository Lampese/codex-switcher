#!/usr/bin/env sh
set -eu

if ! command -v cargo >/dev/null 2>&1; then
  if [ -f "$HOME/.cargo/env" ]; then
    # rustup adds PATH updates here for POSIX shells.
    . "$HOME/.cargo/env"
  fi
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "Error: cargo not found. Install Rust via rustup: https://rustup.rs" >&2
  exit 1
fi

exec tauri "$@"
