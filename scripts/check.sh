#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
command -v cargo >/dev/null || { echo '缺少 Cargo，请先安装 Rust 工具链；脚本不会自动安装。' >&2; exit 1; }
cargo fmt --all -- --check
cargo test --workspace
if command -v npm >/dev/null; then
  (cd apps/launcher && npm ci && npm run build)
fi
if [[ $(uname -s) == Darwin ]]; then
  (cd apps/mac-windowhost && swift build)
  python3 tests/e2e/windowhost_pipe_smoke.py
else
  echo 'SKIP: Swift/AppKit 需要 macOS SDK，当前平台未编译。'
fi
