#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
python3 - <<'PY'
import gi
for name, version in [('Gtk','3.0'),('Gst','1.0'),('GstApp','1.0')]:
    gi.require_version(name,version)
from gi.repository import Gst
Gst.init(None)
assert any(Gst.ElementFactory.find(n) for n in ('avdec_h264','openh264dec')), '需要 H.264 解码组件'
PY
mkdir -p "$HOME/.local/bin" "$HOME/.local/share/paneferry" "$HOME/.local/share/applications" "$HOME/.local/share/icons/hicolor/128x128/apps" "$HOME/.config/systemd/user"
install -m 755 bin/* "$HOME/.local/bin/"
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
systemctl --user daemon-reload
# Replace older systemd-run transient installations of the same product unit.
# A stopped transient unit must be unloaded before enabling its persistent file.
systemctl --user stop remoteapp-agent.service || true
systemctl --user reset-failed remoteapp-agent.service 2>/dev/null || true
systemctl --user daemon-reload
systemctl --user enable remoteapp-agent.service
systemctl --user start remoteapp-agent.service
printf '安装完成。在应用菜单打开 窗渡 PaneFerry，或运行 ~/.local/bin/paneferry。\n'
