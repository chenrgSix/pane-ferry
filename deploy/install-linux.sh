#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
defer_restart=false
case "${1:-}" in
  --defer-restart) defer_restart=true ;;
  '') ;;
  *) echo '用法：./install.sh [--defer-restart]' >&2; exit 2 ;;
esac
# The old service owns independent application processes. Never stop them as an
# incidental upgrade; an active service requires an explicitly deferred restart.
if ! "$defer_restart" && systemctl --user is-active --quiet remoteapp-agent.service; then
  echo '服务正在运行。请用 ./install.sh --defer-restart 安装并保留当前应用，保存工作后再手动重启服务。' >&2
  exit 2
fi
python3 - <<'PY'
import gi
for name, version in [('Gtk','3.0'),('Gst','1.0'),('GstApp','1.0')]:
    gi.require_version(name,version)
from gi.repository import Gst
Gst.init(None)
assert any(Gst.ElementFactory.find(n) for n in ('avdec_h264','openh264dec')), '需要 H.264 解码组件'
PY
mkdir -p "$HOME/.local/bin" "$HOME/.local/share/paneferry" "$HOME/.local/share/applications" "$HOME/.local/share/icons/hicolor/128x128/apps" "$HOME/.config/systemd/user"
for binary in bin/*; do
  name=$(basename "$binary")
  install -m 755 "$binary" "$HOME/.local/bin/.$name.new"
  mv -f "$HOME/.local/bin/.$name.new" "$HOME/.local/bin/$name"
done
install -m 644 share/paneferry/*.py "$HOME/.local/share/paneferry/"
install -m 644 share/applications/*.desktop "$HOME/.local/share/applications/"
python3 - <<'PY'
from pathlib import Path
entry=Path.home()/'.local/share/applications/io.lanremoteapp.launcher.desktop'
executable=str(Path.home()/'.local/bin/paneferry')
for character in ('\\','"','`','$'):
    executable=executable.replace(character,'\\'+character)
entry.write_text(entry.read_text().replace('Exec=paneferry\n','Exec="'+executable+'"\n'))
PY
install -m 644 share/icons/hicolor/128x128/apps/* "$HOME/.local/share/icons/hicolor/128x128/apps/"
install -m 644 systemd/remoteapp-agent.service "$HOME/.config/systemd/user/"
extension_root="${XDG_DATA_HOME:-$HOME/.local/share}/gnome-shell/extensions/paneferry@local"
mkdir -p "$extension_root"
install -m 644 share/gnome-shell/extensions/paneferry@local/* "$extension_root/"
if command -v gnome-shell >/dev/null && [[ $(gnome-shell --version) == *' 50.'* ]]; then
  python3 - <<'PY'
import ast, subprocess
try:
    raw=subprocess.check_output(['gsettings','get','org.gnome.shell','enabled-extensions'],text=True).strip()
    enabled=[] if raw=='@as []' else ast.literal_eval(raw)
    if 'paneferry@local' not in enabled:
        subprocess.run(['gsettings','set','org.gnome.shell','enabled-extensions',repr([*enabled,'paneferry@local'])],check=True)
except (OSError, ValueError, SyntaxError, subprocess.SubprocessError):
    print('请在 GNOME 扩展管理器启用“窗渡 PaneFerry”。')
PY
  gnome-extensions enable paneferry@local 2>/dev/null || printf 'GNOME 首次识别扩展需下次登录；当前桌面和应用保持运行。\n'
else
  printf '当前 Wayland 窗口桥接支持 GNOME 50；其他桌面继续使用 X11 路径。\n'
fi
systemctl --user daemon-reload
if "$defer_restart"; then
  printf '文件已更新，当前服务和应用保持运行。保存工作后运行 systemctl --user restart remoteapp-agent.service 以使用新版。\n'
  exit 0
fi
# Replace older systemd-run transient installations of the same product unit.
# A stopped transient unit must be unloaded before enabling its persistent file.
systemctl --user stop remoteapp-agent.service || true
systemctl --user reset-failed remoteapp-agent.service 2>/dev/null || true
systemctl --user daemon-reload
systemctl --user enable remoteapp-agent.service
systemctl --user start remoteapp-agent.service
printf '安装完成。在应用菜单打开 窗渡 PaneFerry，或运行 ~/.local/bin/paneferry。\n'
