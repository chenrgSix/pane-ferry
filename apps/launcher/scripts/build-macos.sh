#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ "$(uname -s)" != Darwin ]]; then echo "需要 macOS 和 Xcode Command Line Tools。" >&2; exit 1; fi
swift build --package-path ../mac-windowhost -c release
mkdir -p src-tauri/resources
cp ../mac-windowhost/.build/release/RemoteAppWindowHost src-tauri/resources/RemoteAppWindowHost
chmod 755 src-tauri/resources/RemoteAppWindowHost
npm ci
npm run tauri -- build
