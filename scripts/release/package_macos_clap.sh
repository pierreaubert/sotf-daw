#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 2 ] || [ ! -f "$1" ] || [[ "$2" != *.clap ]]; then
    echo "usage: package_macos_clap.sh <libplugin.dylib> <destination.clap>" >&2
    exit 2
fi

library=$1
bundle=$2
base=$(basename "$bundle" .clap)
if [[ "$library" != *.dylib ]] || [[ ! -s "$library" ]] || [[ ! "$base" =~ ^[A-Za-z0-9_]+$ ]]; then
    echo "CLAP library must be a dylib and bundle stem must be ASCII alphanumeric/underscore" >&2
    exit 2
fi

identifier=${base//_/-}
mkdir -p "$bundle/Contents/MacOS"
cp "$library" "$bundle/Contents/MacOS/$base"
cmp -s "$library" "$bundle/Contents/MacOS/$base"
printf '<?xml version="1.0" encoding="UTF-8"?>\n<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n<plist version="1.0">\n<dict>\n\t<key>CFBundleExecutable</key>\n\t<string>%s</string>\n\t<key>CFBundleIdentifier</key>\n\t<string>org.spinorama.sotf.%s.clap</string>\n\t<key>CFBundleName</key>\n\t<string>%s</string>\n\t<key>CFBundlePackageType</key>\n\t<string>BNDL</string>\n\t<key>CFBundleVersion</key>\n\t<string>1</string>\n</dict>\n</plist>\n' "$base" "$identifier" "$base" > "$bundle/Contents/Info.plist"
