#!/bin/sh
# Optional approved-app wrapper for Firefox, including its confined Snap build.
# Run only inside a RemoteApp-owned X11 session. No desktop settings are changed.
set -eu
display_number=${DISPLAY#:}
case "$display_number" in ''|*[!0-9]*) echo 'A local numeric DISPLAY is required.' >&2; exit 2 ;; esac
test -r "${XAUTHORITY:-}" || { echo 'A readable Xauthority is required.' >&2; exit 2; }
if test -d /snap/firefox/current; then
  umask 077
  firefox_auth_dir="$HOME/snap/firefox/common/lan-remoteapp"
  mkdir -p "$firefox_auth_dir"
  chmod 700 "$firefox_auth_dir"
  firefox_auth_file="$firefox_auth_dir/Xauthority-$display_number"
  /usr/bin/xauth -f "$XAUTHORITY" extract "$firefox_auth_file" "$DISPLAY"
  chmod 600 "$firefox_auth_file"
  export XAUTHORITY="$firefox_auth_file"
  # snap-confine needs an application scope rather than the Agent's service
  # cgroup. --scope stays in the foreground and retains the process group.
  export DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$(id -u)/bus"
  export DISABLE_WAYLAND=1 GDK_BACKEND=x11 MOZ_ENABLE_WAYLAND=0
  exec /usr/bin/systemd-run --user --scope --collect --quiet /usr/bin/firefox "$@"
fi
export DISABLE_WAYLAND=1 GDK_BACKEND=x11 MOZ_ENABLE_WAYLAND=0
exec /usr/bin/firefox "$@"
