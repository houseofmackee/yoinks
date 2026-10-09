#!/bin/sh
# Builds Yoinks.app around the cargo binary and registers it with macOS so
# the yoinks:// scheme and the "Add to Yoinks" service work.
#
# Usage: scripts/bundle.sh [debug|release]
set -eu

profile=${1:-debug}
cd "$(dirname "$0")/.."

case "$profile" in
    release) cargo build --release ;;
    debug) cargo build ;;
    *) echo "usage: $0 [debug|release]" >&2; exit 2 ;;
esac

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
app="target/$profile/Yoinks.app"

rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp "target/$profile/yoinks" "$app/Contents/MacOS/yoinks"
sed "s/@VERSION@/$version/g" macos/Info.plist > "$app/Contents/Info.plist"

# Ad-hoc signature for local use. Distribution builds are signed with a
# Developer ID and notarized (see PLAN.md, Phase 10).
codesign --force --sign - "$app"

lsregister=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
"$lsregister" -f "$app"
/System/Library/CoreServices/pbs -update

echo "$app"
