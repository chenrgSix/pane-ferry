#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
if [ "$(uname -s)" != Darwin ]; then
  echo '此脚本必须在 macOS 上运行，需要 Xcode Command Line Tools。' >&2
  exit 1
fi
swift build -c release
BIN_DIR="$(swift build -c release --show-bin-path)"
APP="dist/PaneFerryWindowHost.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN_DIR/PaneFerryWindowHost" "$APP/Contents/MacOS/PaneFerryWindowHost"
cp ../../LICENSE "$APP/Contents/Resources/LICENSE"
cp ../launcher/src-tauri/icons/icon.png "$APP/Contents/Resources/PaneFerry.png"
cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>local.lanremoteapp.windowhost</string>
<key>CFBundleName</key><string>窗渡 PaneFerry WindowHost</string>
<key>CFBundleExecutable</key><string>PaneFerryWindowHost</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.1.1</string>
<key>CFBundleVersion</key><string>0.1.1</string>
<key>LSMinimumSystemVersion</key><string>12.0</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
codesign --force --sign - --identifier local.lanremoteapp.windowhost "$APP"
codesign --verify --deep --strict "$APP"
printf '已生成 %s（本机临时签名开发包，未公证）。\n' "$APP"
printf '通过启动器或带 --native-peer --native-state --native-helper --app 参数启动 Contents/MacOS/PaneFerryWindowHost。\n'
