#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
[[ $(uname -s) == Linux ]] || { echo '请在 Ubuntu 上构建 Linux 安装包。' >&2; exit 1; }
cargo build --release -p remoteapp-agent
output="${1:-dist/lan-remoteapp-linux}"
mkdir -p "$output/bin" "$output/share/lan-remoteapp" "$output/share/applications" "$output/share/icons/hicolor/128x128/apps" "$output/systemd"
install -m 755 target/release/remoteapp-agent "$output/bin/remoteapp-agent"
install -m 644 apps/linux-client/*.py "$output/share/lan-remoteapp/"
install -m 644 apps/launcher/src-tauri/icons/icon.png "$output/share/icons/hicolor/128x128/apps/io.lanremoteapp.launcher.png"
install -m 644 deploy/linux-user-service/remoteapp-agent.service "$output/systemd/"
install -m 755 deploy/install-linux.sh "$output/install.sh"
cat > "$output/bin/lan-remoteapp" <<'SH'
#!/bin/sh
export GDK_BACKEND=x11
exec /usr/bin/python3 -I "$HOME/.local/share/lan-remoteapp/main.py" "$@"
SH
chmod 755 "$output/bin/lan-remoteapp"
cat > "$output/share/applications/io.lanremoteapp.launcher.desktop" <<'DESKTOP'
[Desktop Entry]
Type=Application
Name=LAN RemoteApp
Comment=共享本机应用，同时使用远端设备的应用
Exec=lan-remoteapp
Icon=io.lanremoteapp.launcher
Terminal=false
Categories=Network;RemoteAccess;
StartupWMClass=LAN RemoteApp
DESKTOP
tar -czf "$output.tar.gz" -C "$(dirname "$output")" "$(basename "$output")"
printf '已生成 %s.tar.gz（包含发布与接收组件）。\n' "$output"
