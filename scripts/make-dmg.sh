#!/bin/sh
# Builds a release Yoinks.app and packs it into a drag-to-install disk image:
# target/release/Yoinks-<version>.dmg (Yoinks.app next to an Applications
# shortcut).
#
# YOINKS_DMG_NO_BUILD=1 packs the existing target/release/Yoinks.app as is
# (scripts/dist.sh uses it after notarizing the app). Signing and
# notarizing the image is up to scripts/dist.sh.
set -eu
cd "$(dirname "$0")/.."

[ -n "${YOINKS_DMG_NO_BUILD:-}" ] || scripts/bundle.sh release >/dev/null
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
