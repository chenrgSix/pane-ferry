#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
[[ $(uname -s) == Linux ]] || { echo '需要在 Linux 上构建启动器。' >&2; exit 1; }
for command in cargo npm cmake pkg-config; do
  command -v "$command" >/dev/null || { echo "缺少构建工具：$command" >&2; exit 1; }
done
pkg-config --exists gtk+-3.0 webkit2gtk-4.1 || {
  echo '请安装 libgtk-3-dev、libwebkit2gtk-4.1-dev、libappindicator3-dev 和 librsvg2-dev。' >&2; exit 1;
}
cargo build --locked --manifest-path ../../Cargo.toml --release -p paneferry-agent -p paneferry-network
mkdir -p src-tauri/resources/linux-client
rm -f src-tauri/resources/linux-client/main.py
install -m 755 ../../target/release/paneferry-{agent,network} src-tauri/resources/
for file in ../linux-client/*.py; do
  # The retired GTK manager is not part of the product UI anymore.
  [[ $(basename "$file") == main.py ]] || install -m 644 "$file" src-tauri/resources/linux-client/
done
npm ci
npm run tauri -- build --no-bundle --config src-tauri/tauri.linux.conf.json
