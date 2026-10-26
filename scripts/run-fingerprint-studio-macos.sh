#!/bin/sh
# Build a local app bundle so macOS identifies the GPUI app by name.
set -eu
cd "$(dirname "$0")/.."
build_only=false
if [ "${1:-}" = "--build-only" ]; then
    build_only=true
    shift
fi
if [ ! -f apps/fingerprint-studio/assets/FingerprintStudio.icns ]; then
    ./scripts/build-fingerprint-studio-icon.py
fi
cargo build -p fingerprint-studio
bundle='dist/Fingerprint Studio.app'
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"
# Replace the inode rather than overwriting a running, signed executable.
cp target/debug/fingerprint-studio "$bundle/Contents/MacOS/fingerprint-studio.new"
mv -f "$bundle/Contents/MacOS/fingerprint-studio.new" "$bundle/Contents/MacOS/fingerprint-studio"
cp apps/fingerprint-studio/assets/FingerprintStudio.icns "$bundle/Contents/Resources/FingerprintStudio.icns"
cat > "$bundle/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>fingerprint-studio</string>
<key>CFBundleIdentifier</key><string>org.uareu.fingerprint-studio</string>
<key>CFBundleName</key><string>Fingerprint Studio</string>
<key>CFBundleDisplayName</key><string>Fingerprint Studio</string>
<key>CFBundleIconFile</key><string>FingerprintStudio.icns</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.1.0</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
codesign --force --deep --sign - "$bundle"
if [ "$build_only" = true ]; then
    printf 'Built %s\n' "$bundle"
    exit 0
fi
exec "$bundle/Contents/MacOS/fingerprint-studio" "$@"
