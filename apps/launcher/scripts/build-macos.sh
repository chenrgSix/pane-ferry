#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ "$(uname -s)" != Darwin ]]; then echo "需要 macOS 和 Xcode Command Line Tools。" >&2; exit 1; fi
../mac-windowhost/scripts/build-app.sh
mkdir -p src-tauri/resources
ditto ../mac-windowhost/dist/RemoteAppWindowHost.app src-tauri/resources/RemoteAppWindowHost.app
npm ci
npm run tauri -- build
