#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
[[ $(uname -s) == Linux ]] || { echo '请在 Ubuntu 上构建 Linux 安装包。' >&2; exit 1; }
cargo build --release -p paneferry-agent -p paneferry-network
output="${1:-dist/paneferry-linux}"
mkdir -p "$output/bin" "$output/share/paneferry" "$output/share/applications" "$output/share/icons/hicolor/128x128/apps" "$output/systemd"
mkdir -p "$output/share/gnome-shell/extensions/paneferry@local"
mkdir -p "$output/share/doc/paneferry"
install -m 644 LICENSE "$output/share/doc/paneferry/LICENSE"
install -m 644 apps/linux-agent/gnome-extension/* "$output/share/gnome-shell/extensions/paneferry@local/"
install -m 755 target/release/paneferry-agent "$output/bin/paneferry-agent"
install -m 755 target/release/paneferry-network "$output/bin/paneferry-network"
install -m 644 apps/linux-client/*.py "$output/share/paneferry/"
install -m 644 apps/launcher/src-tauri/icons/icon.png "$output/share/icons/hicolor/128x128/apps/io.lanremoteapp.launcher.png"
install -m 644 deploy/linux-user-service/remoteapp-agent.service "$output/systemd/"
install -m 755 deploy/install-linux.sh "$output/install.sh"
cat > "$output/bin/paneferry" <<'SH'
#!/bin/sh
export GDK_BACKEND=x11
exec /usr/bin/python3 -I "$HOME/.local/share/paneferry/main.py" "$@"
SH
chmod 755 "$output/bin/paneferry"
cat > "$output/share/applications/io.lanremoteapp.launcher.desktop" <<'DESKTOP'
[Desktop Entry]
Type=Application
Name=窗渡 PaneFerry
Comment=共享本机应用，同时使用远端设备的应用
Exec=paneferry
Icon=io.lanremoteapp.launcher
Terminal=false
Categories=Network;RemoteAccess;
StartupWMClass=PaneFerry
DESKTOP
tar -czf "$output.tar.gz" -C "$(dirname "$output")" "$(basename "$output")"
printf '已生成 %s.tar.gz（包含发布与接收组件）。\n' "$output"
