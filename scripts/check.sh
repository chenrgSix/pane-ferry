#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
command -v cargo >/dev/null || { echo '缺少 Cargo，请先安装 Rust 工具链；脚本不会自动安装。' >&2; exit 1; }
cargo fmt --all -- --check
cargo test --workspace
python3 tests/e2e/desktop_authorization_unit.py
python3 tests/e2e/desktop_monitor_unit.py
python3 tests/e2e/linux_install_safety.py
python3 tests/e2e/linux_runtime_unit.py
python3 tests/e2e/linux_manager_unit.py
python3 tests/e2e/wayland_capture_unit.py
python3 -m py_compile apps/linux-client/*.py
python3 tests/e2e/linux_wire_unit.py
python3 tests/e2e/linux_receiver_boundaries.py
python3 tests/e2e/native_routes_smoke.py
if command -v node >/dev/null; then
  node tests/unit/application_scene.mjs
  node tests/unit/virtual_output_lifecycle.mjs
  node tests/unit/window_presentation.mjs
  node tests/unit/bridge_scheduling.mjs
  python3 tests/e2e/electron_page_scale_unit.py
fi
if command -v npm >/dev/null; then
  (cd apps/launcher && npm ci && npm test && npm run build)
fi
# This is a separate workspace excluded from the root Cargo.toml. Frontend
# checks alone do not compile the launcher commands, storage or process code.
cargo fmt --manifest-path apps/launcher/src-tauri/Cargo.toml -- --check
# Unit tests do not package the generated Mac helpers. A clean checkout must
# compile these tests without manufacturing placeholder release resources.
# Packaging keeps the real resource configuration and verifies every helper.
TAURI_CONFIG='{"bundle":{"resources":[]}}' cargo test --locked --manifest-path apps/launcher/src-tauri/Cargo.toml
if [[ $(uname -s) == Darwin ]]; then
  (cd apps/mac-node && swift build)
  python3 tests/e2e/mac_publisher_boundaries.py
  python3 tests/e2e/mac_encoder_generation.py
  (cd apps/mac-windowhost && swift build)
  python3 tests/e2e/async_video_decode_smoke.py
  python3 tests/e2e/video_presentation_smoke.py
  python3 tests/e2e/transport_async_video_smoke.py
  python3 tests/e2e/windowhost_pipe_smoke.py
  python3 tests/e2e/controller_input_smoke.py
  python3 tests/e2e/window_tree_smoke.py
  python3 tests/e2e/windowhost_termination_smoke.py
  python3 tests/e2e/application_group_smoke.py
  python3 tests/e2e/network_latency_smoke.py
  python3 tests/e2e/clipboard_ordering_smoke.py
  python3 tests/e2e/automatic_reconnect_smoke.py
  python3 tests/e2e/h264_only_smoke.py
  python3 tests/e2e/native_transport_smoke.py
else
  cargo build -p paneferry-agent -p paneferry-network
  if command -v xvfb-run >/dev/null; then
    xvfb-run -a env GTK_IM_MODULE=simple GDK_BACKEND=x11 python3 tests/e2e/linux_receiver_smoke.py
  fi
  echo 'SKIP: Swift/AppKit 需要 macOS SDK，当前平台未编译。'
fi
