#!/bin/sh
# Builds a release Yoinks.app and packs it into a drag-to-install disk image:
# target/release/Yoinks-<version>.dmg (Yoinks.app next to an Applications
# shortcut).
#
# The image is not signed or notarized yet (see PLAN.md, Phase 10), so
# Gatekeeper asks before the first launch.
set -eu
cd "$(dirname "$0")/.."

scripts/bundle.sh release >/dev/null
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
dmg="target/release/Yoinks-$version.dmg"

staging=$(mktemp -d)
trap 'rm -rf "$staging"' EXIT
cp -R target/release/Yoinks.app "$staging/"
ln -s /Applications "$staging/Applications"

rm -f "$dmg"
hdiutil create -quiet -volname "Yoinks $version" -srcfolder "$staging" \
    -fs APFS -format UDZO -ov "$dmg"
echo "$dmg"
