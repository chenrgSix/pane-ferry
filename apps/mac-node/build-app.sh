#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
swift build -c release "$@"
bin="$(swift build -c release --show-bin-path "$@")"
app="dist/PaneFerryNode.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin/PaneFerryNode" "$app/Contents/MacOS/PaneFerryNode"
cp ../../LICENSE "$app/Contents/Resources/LICENSE"
cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>io.lanremoteapp.node</string>
<key>CFBundleName</key><string>窗渡 PaneFerry</string>
<key>CFBundleDisplayName</key><string>窗渡 PaneFerry</string>
<key>CFBundleExecutable</key><string>PaneFerryNode</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleVersion</key><string>1</string>
<key>CFBundleShortVersionString</key><string>0.1.0</string>
<key>LSMinimumSystemVersion</key><string>13.0</string>
<key>LSUIElement</key><true/>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
codesign --force --sign - --identifier io.lanremoteapp.node "$app"
