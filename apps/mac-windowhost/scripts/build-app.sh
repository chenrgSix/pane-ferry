#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
if [ "$(uname -s)" != Darwin ]; then
  echo '此脚本必须在 macOS 上运行，需要 Xcode Command Line Tools。' >&2
  exit 1
fi
swift build -c release
BIN_DIR="$(swift build -c release --show-bin-path)"
APP="dist/RemoteAppWindowHost.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN_DIR/RemoteAppWindowHost" "$APP/Contents/MacOS/RemoteAppWindowHost"
WEBRTC_RUNTIME="${REMOTEAPP_WEBRTC_RUNTIME:-../../target/webrtc}"
if [ -f "$WEBRTC_RUNTIME/libremoteapp_webrtc.dylib" ]; then
  mkdir -p "$APP/Contents/Frameworks" "$APP/Contents/Resources/WebRTC"
  cp "$WEBRTC_RUNTIME/libremoteapp_webrtc.dylib" "$APP/Contents/Frameworks/"
  cp -R "$WEBRTC_RUNTIME/licenses" "$APP/Contents/Resources/WebRTC/"
  cp "$WEBRTC_RUNTIME/libdatachannel-revision.txt" "$APP/Contents/Resources/WebRTC/"
  cp "$WEBRTC_RUNTIME/THIRD_PARTY_NOTICES.md" "$APP/Contents/Resources/WebRTC/"
elif [ -n "${REMOTEAPP_WEBRTC_RUNTIME:-}" ]; then
  echo '指定的 WebRTC runtime 缺少动态库；先运行 scripts/build-webrtc.py。' >&2
  exit 1
fi
cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>local.lanremoteapp.windowhost</string>
<key>CFBundleName</key><string>LAN RemoteApp WindowHost</string>
<key>CFBundleExecutable</key><string>RemoteAppWindowHost</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.1.0</string>
<key>CFBundleVersion</key><string>1</string>
<key>LSMinimumSystemVersion</key><string>12.0</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
printf '已生成 %s（本机开发包，未签名/公证）。\n' "$APP"
printf '通过启动器或带 --host --user --app 参数启动 Contents/MacOS/RemoteAppWindowHost。\n'
