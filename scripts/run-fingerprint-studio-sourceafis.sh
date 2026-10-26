#!/bin/sh
# Experimental SourceAFIS setup using HID's nominal 500 dpi specification.
# The decoder's effective output scale has not been calibrated.
set -eu
cd "$(dirname "$0")/.."
build_only=false
if [ "${1:-}" = "--build-only" ]; then
    build_only=true
    shift
fi
if [ "$#" -ne 0 ]; then
    printf 'Usage: %s [--build-only]\n' "$0" >&2
    exit 2
fi
if [ ! -f apps/fingerprint-studio/assets/FingerprintStudio.icns ]; then
    ./scripts/build-fingerprint-studio-icon.py
fi
dotnet restore examples/engine/UareuEngine.csproj --locked-mode
dotnet build examples/engine/UareuEngine.csproj -c Release --no-restore
engine="$PWD/examples/engine/bin/Release/net10.0/UareuEngine"
"$engine" --self-test
printf '%s\n' 'Experimental matching: assuming nominal 500 PPI; decoder scale is unverified.'
cargo build -p fingerprint-studio
bundle='dist/Fingerprint Studio SourceAFIS.app'
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"
cp apps/fingerprint-studio/assets/FingerprintStudio.icns "$bundle/Contents/Resources/FingerprintStudio.icns"
# Replace the executable inode so a running older app is never overwritten.
cp target/debug/fingerprint-studio "$bundle/Contents/MacOS/fingerprint-studio.new"
mv -f "$bundle/Contents/MacOS/fingerprint-studio.new" "$bundle/Contents/MacOS/fingerprint-studio"
cat > "$bundle/Contents/MacOS/launch-sourceafis" <<'LAUNCH'
#!/bin/sh
set -eu
app_dir=$(CDPATH= cd -- "$(dirname "$0")" && pwd)
repo_dir=$(CDPATH= cd -- "$app_dir/../../../.." && pwd)
exec "$app_dir/fingerprint-studio" \
  --engine "$repo_dir/examples/engine/bin/Release/net10.0/UareuEngine" \
  --experimental-ppi 500 --threshold 40
LAUNCH
chmod +x "$bundle/Contents/MacOS/launch-sourceafis"
cat > "$bundle/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>launch-sourceafis</string>
<key>CFBundleIdentifier</key><string>org.uareu.fingerprint-studio.sourceafis</string>
<key>CFBundleName</key><string>Fingerprint Studio SourceAFIS</string>
<key>CFBundleDisplayName</key><string>Fingerprint Studio SourceAFIS</string>
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
exec "$bundle/Contents/MacOS/launch-sourceafis"
