#!/usr/bin/env bash
set -euo pipefail
umask 022
cd "$(dirname "$0")/.."
[[ $(uname -s) == Linux ]] || { echo '请在 Ubuntu 上构建 .deb 安装包。' >&2; exit 1; }
for command in cargo dpkg-deb dpkg-shlibdeps python3; do
  command -v "$command" >/dev/null || { echo "缺少构建工具：$command" >&2; exit 1; }
done
cargo build --locked --release -p paneferry-agent -p paneferry-network
version=$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["workspace"]["package"]["version"])')
revision="${PANEFERRY_DEB_REVISION:-1}"
[[ "$revision" =~ ^[1-9][0-9]*$ ]] || { echo 'Debian 修订号必须为正整数。' >&2; exit 1; }
architecture=$(dpkg --print-architecture)
output="${1:-dist}"
mkdir -p "$output"
output=$(cd "$output" && pwd)
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
package="$stage/package"
mkdir -p "$package/DEBIAN" "$package/usr/bin" "$package/usr/lib/paneferry" \
  "$package/usr/lib/systemd/user" "$package/usr/share/applications" \
  "$package/usr/share/icons/hicolor/128x128/apps" "$package/usr/share/doc/paneferry" \
  "$package/usr/share/gnome-shell/extensions/paneferry@local" "$stage/debian"
install -m 755 target/release/paneferry-{agent,network} "$package/usr/lib/paneferry/"
install -m 644 apps/linux-client/*.py "$package/usr/lib/paneferry/"
install -m 644 apps/linux-agent/gnome-extension/* "$package/usr/share/gnome-shell/extensions/paneferry@local/"
install -m 644 apps/launcher/src-tauri/icons/icon.png "$package/usr/share/icons/hicolor/128x128/apps/io.lanremoteapp.launcher.png"
install -m 644 LICENSE "$package/usr/share/doc/paneferry/LICENSE"
install -m 644 LICENSE "$package/usr/share/doc/paneferry/copyright"
python3 - "$package/usr/share/doc/paneferry/usage.md" <<'PY'
from pathlib import Path
import posixpath, re, sys
def online(match):
    target = match[1]
    if '://' in target or target.startswith('#'):
        return match[0]
    return '](' + 'https://github.com/chenrgSix/pane-ferry/blob/main/' + posixpath.normpath('docs/' + target) + ')'
Path(sys.argv[1]).write_text(re.sub(r'\]\(([^)]+)\)', online, Path('docs/usage.md').read_text()))
PY
sed 's|ExecStart=%h/.local/bin/paneferry-agent serve|ExecStart=/usr/lib/paneferry/paneferry-agent serve|' \
  deploy/linux-user-service/remoteapp-agent.service > "$package/usr/lib/systemd/user/remoteapp-agent.service"
cat > "$package/usr/bin/paneferry" <<'SH'
#!/bin/sh
export GDK_BACKEND=x11
exec /usr/bin/python3 -I /usr/lib/paneferry/main.py "$@"
SH
chmod 755 "$package/usr/bin/paneferry"
ln -s ../lib/paneferry/paneferry-agent "$package/usr/bin/paneferry-agent"
ln -s ../lib/paneferry/paneferry-network "$package/usr/bin/paneferry-network"
cat > "$package/usr/share/applications/io.lanremoteapp.launcher.desktop" <<'DESKTOP'
[Desktop Entry]
Type=Application
Name=窗渡 PaneFerry
Comment=共享本机应用，同时使用远端设备的应用
Exec=/usr/bin/paneferry
Icon=io.lanremoteapp.launcher
Terminal=false
Categories=Network;RemoteAccess;
StartupWMClass=PaneFerry
DESKTOP
# Resolve the build host's actual ELF ABI requirements, then add libraries and
# plugins loaded at runtime. No maintainer script touches users or sessions.
cat > "$stage/debian/control" <<EOF
Source: paneferry
Section: net
Priority: optional
Maintainer: chenrgSix <chenrgSix@users.noreply.github.com>

Package: paneferry
Architecture: $architecture
Description: PaneFerry desktop application sharing
EOF
shlibs=$(cd "$stage" && dpkg-shlibdeps -O -e"$package/usr/lib/paneferry/paneferry-agent" -e"$package/usr/lib/paneferry/paneferry-network")
shlibs=${shlibs#shlibs:Depends=}
size=$(du -sk "$package/usr" | awk '{print $1}')
cat > "$package/DEBIAN/control" <<EOF
Package: paneferry
Version: $version-$revision
Architecture: $architecture
Section: net
Priority: optional
Maintainer: chenrgSix <chenrgSix@users.noreply.github.com>
Homepage: https://github.com/chenrgSix/pane-ferry
Installed-Size: $size
Depends: $shlibs, python3 (>= 3.10), python3-gi, python3-gi-cairo, python3-websocket, gir1.2-gtk-3.0, gir1.2-gstreamer-1.0, gir1.2-gst-plugins-base-1.0, gstreamer1.0-plugins-base, gstreamer1.0-plugins-good, gstreamer1.0-plugins-bad, gstreamer1.0-libav, gstreamer1.0-pipewire, xvfb, openbox, xauth, dbus-x11, xdg-dbus-proxy, libei1, libx11-6, xdg-desktop-portal
Recommends: xdg-desktop-portal-gnome, gnome-shell-extension-prefs
Description: PaneFerry graphical application sharing between Mac and Ubuntu
 Pair devices and use remote applications in native windows over QUIC.
 Includes the GTK manager, receiver, user agent and GNOME window bridge.
 Applications remain on the publishing device. GNOME desktop integration
 requires GNOME 50; this preview is built for Ubuntu 26.04.
 .
 Source-available under PolyForm Noncommercial 1.0.0; commercial uses outside
 the license's permissions require separate authorization.
EOF
python3 - "$package" <<'PY'
from pathlib import Path
import hashlib, sys
package = Path(sys.argv[1])
files = sorted(p for p in (package/'usr').rglob('*') if p.is_file() and not p.is_symlink())
(package/'DEBIAN/md5sums').write_text(''.join(
    f'{hashlib.md5(p.read_bytes()).hexdigest()}  {p.relative_to(package)}\n' for p in files))
PY
dpkg-deb --root-owner-group --build "$package" "$output/paneferry_${version}-${revision}_${architecture}.deb"
printf '已生成 %s/paneferry_%s-%s_%s.deb\n' "$output" "$version" "$revision" "$architecture"
