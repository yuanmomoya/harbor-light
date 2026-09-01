#!/bin/zsh
set -euo pipefail
cd "$(dirname "$0")/.."
if ! command -v cargo >/dev/null; then
  echo "需要安装 Rust：https://rustup.rs" >&2
  exit 1
fi
if ! xcode-select -p >/dev/null 2>&1; then
  echo "需要 Xcode Command Line Tools：xcode-select --install" >&2
  exit 1
fi
cargo build --release
BIN="${CARGO_TARGET_DIR:-./target}/release/harbor-light"
exec "$BIN" install "$@"
