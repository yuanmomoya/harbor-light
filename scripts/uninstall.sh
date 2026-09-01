#!/bin/zsh
set -euo pipefail
cd "$(dirname "$0")/.."
BIN="${CARGO_TARGET_DIR:-./target}/release/harbor-light"
if [[ -x "$BIN" ]]; then
  exec "$BIN" uninstall "$@"
fi
if command -v cargo >/dev/null; then
  exec cargo run --release -- uninstall "$@"
fi
echo "找不到 harbor-light，请在本仓库运行：make uninstall" >&2
exit 1
