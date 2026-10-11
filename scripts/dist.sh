#!/bin/sh
# Builds everything a GitHub release needs, in target/dist:
#
#   Yoinks-<version>.dmg  from scripts/make-dmg.sh
#   Yoinks-<version>.zip  the same Yoinks.app, zipped with ditto, which keeps
#                         the signature intact
#   SHA256SUMS.txt        checksums of both
#
# The release workflow (.github/workflows/release.yml) runs this with
# SIGN_IDENTITY="Yoinks Release". Run locally, it signs the same way as
# `scripts/bundle.sh release` (the release certificate if present, else ad
# hoc).
set -eu
cd "$(dirname "$0")/.."

scripts/make-dmg.sh >/dev/null
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
dmg="Yoinks-$version.dmg"
zip="Yoinks-$version.zip"
dist=target/dist

rm -rf "$dist"
mkdir -p "$dist"
cp "target/release/$dmg" "$dist/$dmg"
ditto -c -k --sequesterRsrc --keepParent target/release/Yoinks.app "$dist/$zip"
(cd "$dist" && shasum -a 256 "$dmg" "$zip" > SHA256SUMS.txt)
cat "$dist/SHA256SUMS.txt"
