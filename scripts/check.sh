#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
command -v cargo >/dev/null || { echo '缺少 Cargo，请先安装 Rust 工具链；脚本不会自动安装。' >&2; exit 1; }
cargo fmt --all -- --check
cargo test --workspace
python3 tests/e2e/desktop_authorization_unit.py
python3 -m py_compile apps/linux-client/*.py
python3 tests/e2e/ssh_helper_validation.py
python3 tests/e2e/ssh_recovery_smoke.py
if command -v npm >/dev/null; then
  (cd apps/launcher && npm ci && npm test && npm run build)
fi
if [[ $(uname -s) == Darwin ]]; then
  (cd apps/mac-node && swift build)
  python3 tests/e2e/mac_publisher_boundaries.py
  (cd apps/mac-windowhost && swift build)
  python3 tests/e2e/windowhost_pipe_smoke.py
  python3 tests/e2e/controller_input_smoke.py
  python3 tests/e2e/clipboard_ordering_smoke.py
  python3 tests/e2e/automatic_reconnect_smoke.py
  python3 tests/e2e/ssh_password_smoke.py
else
  cargo build -p remoteapp-agent
  python3 tests/e2e/node_routes_smoke.py --agent target/debug/remoteapp-agent
  if command -v xvfb-run >/dev/null; then
    xvfb-run -a env GTK_IM_MODULE=simple GDK_BACKEND=x11 python3 tests/e2e/linux_receiver_smoke.py
  fi
  echo 'SKIP: Swift/AppKit 需要 macOS SDK，当前平台未编译。'
fi
