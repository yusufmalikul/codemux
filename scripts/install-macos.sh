#!/bin/sh
# Build Codemux.app and install it to /Applications,
# so it opens from Finder, Launchpad, or Spotlight (Cmd+Space, type "codemux").
set -eu
cd "$(dirname "$0")/.."

cargo build --release -p codemux

version=$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -1)
out=target/release/bundle
app="$out/Codemux.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/Codemux "$app/Contents/MacOS/Codemux"

# Turn the PNG into an .icns icon file.
iconset="$out/Codemux.iconset"
rm -rf "$iconset" && mkdir -p "$iconset"
for s in 16 32 128 256 512; do
    sips -z $s $s crates/app/assets/icon.png --out "$iconset/icon_${s}x${s}.png" >/dev/null
    d=$((s * 2))
    sips -z $d $d crates/app/assets/icon.png --out "$iconset/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/Codemux.icns"
rm -rf "$iconset"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Codemux</string>
    <key>CFBundleDisplayName</key><string>Codemux</string>
    <key>CFBundleIdentifier</key><string>dev.codemux.app</string>
    <key>CFBundleExecutable</key><string>Codemux</string>
    <key>CFBundleIconFile</key><string>Codemux</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleVersion</key><string>$version</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

# Ad-hoc sign, so macOS lets the app run on Apple Silicon.
codesign --force --sign - "$app"

rm -rf /Applications/Codemux.app
cp -R "$app" /Applications/
echo "Installed /Applications/Codemux.app"
