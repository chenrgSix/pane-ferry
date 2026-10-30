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
  # Report every independent AppKit regression in one run; any failure still
  # fails this gate. Slow builds must not hide the remaining failures.
  mac_failed=0
  for smoke in chroma_atlas_smoke async_video_decode_smoke video_presentation_smoke transport_async_video_smoke windowhost_pipe_smoke controller_input_smoke window_tree_smoke windowhost_termination_smoke application_group_smoke network_latency_smoke clipboard_ordering_smoke automatic_reconnect_smoke h264_only_smoke native_transport_smoke; do
    if ! python3 "tests/e2e/${smoke}.py"; then mac_failed=1; fi
  done
  if [[ $mac_failed != 0 ]]; then exit 1; fi
else
  cargo build -p paneferry-agent -p paneferry-network
  if command -v xvfb-run >/dev/null; then
    xvfb-run -a env GTK_IM_MODULE=simple GDK_BACKEND=x11 python3 tests/e2e/linux_receiver_smoke.py
  fi
  echo 'SKIP: Swift/AppKit 需要 macOS SDK，当前平台未编译。'
fi
