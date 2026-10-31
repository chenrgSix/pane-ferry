#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ "$(uname -s)" != Darwin ]]; then echo "需要 macOS 和 Xcode Command Line Tools。" >&2; exit 1; fi
command -v cmake >/dev/null || { echo '音频 Opus 编译需要 CMake，请先运行 brew install cmake。' >&2; exit 1; }
../mac-windowhost/scripts/build-app.sh
../mac-node/build-app.sh
mkdir -p src-tauri/resources
ditto ../mac-windowhost/dist/PaneFerryWindowHost.app src-tauri/resources/PaneFerryWindowHost.app
ditto ../mac-node/dist/PaneFerryNode.app src-tauri/resources/PaneFerryNode.app
cargo build --manifest-path ../../Cargo.toml --release -p paneferry-network
install -m 755 ../../target/release/paneferry-network src-tauri/resources/paneferry-network
npm ci
npm run tauri -- build
bundle="src-tauri/target/release/bundle/macos/窗渡 PaneFerry.app"
# Tauri may leave the launcher executable linker-signed without sealing the
# containing bundle. Seal all embedded helpers before this development build is
# installed; an unsealed bundle fails macOS strict verification.
codesign --force --deep --sign - "$bundle"
codesign --verify --deep --strict "$bundle"
# Helpers live in Resources; verify their app seals explicitly as well.
codesign --verify --deep --strict "$bundle/Contents/Resources/PaneFerryWindowHost.app"
codesign --verify --deep --strict "$bundle/Contents/Resources/PaneFerryNode.app"
