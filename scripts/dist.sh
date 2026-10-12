#!/bin/sh
# Builds everything a GitHub release needs, in target/dist:
#
#   Yoinks-<version>.dmg  from scripts/make-dmg.sh
#   Yoinks-<version>.zip  the same Yoinks.app, zipped with ditto, which keeps
#                         the signature intact
#   SHA256SUMS.txt        checksums of both
#
# The app is signed as by `scripts/bundle.sh release`. When that is a
# Developer ID and an App Store Connect API key is given, the app and the
# DMG are also notarized and stapled, so Gatekeeper opens them without
# asking:
#
#   YOINKS_NOTARY_KEY         path to the key (.p8, Developer role)
#   YOINKS_NOTARY_KEY_ID      its key ID
#   YOINKS_NOTARY_ISSUER      the issuer ID
#
# The release workflow (.github/workflows/release.yml) sets these.
set -eu
cd "$(dirname "$0")/.."

app=target/release/Yoinks.app
scripts/bundle.sh release >/dev/null
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
dmg="Yoinks-$version.dmg"
zip="Yoinks-$version.zip"
dist=target/dist

rm -rf "$dist"
mkdir -p "$dist"

identity=$(codesign -dvv "$app" 2>&1 | sed -n 's/^Authority=\(Developer ID Application: .*\)$/\1/p')
notarize=
if [ -n "$identity" ]; then
    if [ -n "${YOINKS_NOTARY_KEY:-}" ] && [ -n "${YOINKS_NOTARY_KEY_ID:-}" ] && [ -n "${YOINKS_NOTARY_ISSUER:-}" ]; then
        notarize=1
    else
        echo "note: signed with a Developer ID but not notarized; set YOINKS_NOTARY_KEY, YOINKS_NOTARY_KEY_ID and YOINKS_NOTARY_ISSUER" >&2
    fi
fi

# Submits a zip or DMG to Apple and waits; on rejection prints Apple's log
# and fails.
notarize() {
    echo "notarizing $1"
    result=$(xcrun notarytool submit "$1" --wait --output-format json \
        --key "$YOINKS_NOTARY_KEY" --key-id "$YOINKS_NOTARY_KEY_ID" --issuer "$YOINKS_NOTARY_ISSUER")
    echo "$result"
    status=$(printf '%s' "$result" | plutil -extract status raw -o - - 2>/dev/null || true)
    if [ "$status" != Accepted ]; then
        id=$(printf '%s' "$result" | plutil -extract id raw -o - - 2>/dev/null || true)
        if [ -n "$id" ]; then
            xcrun notarytool log "$id" \
                --key "$YOINKS_NOTARY_KEY" --key-id "$YOINKS_NOTARY_KEY_ID" --issuer "$YOINKS_NOTARY_ISSUER" >&2 || true
        fi
        echo "error: notarization of $1 failed ($status)" >&2
        exit 1
    fi
}

if [ -n "$notarize" ]; then
    # The app first, so the zip and the DMG both hold a stapled app.
    ditto -c -k --keepParent "$app" "$dist/notarize.zip"
    notarize "$dist/notarize.zip"
    rm "$dist/notarize.zip"
    xcrun stapler staple "$app"
fi

YOINKS_DMG_NO_BUILD=1 scripts/make-dmg.sh >/dev/null
cp "target/release/$dmg" "$dist/$dmg"
if [ -n "$identity" ]; then
    codesign --force --timestamp --sign "$identity" "$dist/$dmg"
fi
if [ -n "$notarize" ]; then
    notarize "$dist/$dmg"
    xcrun stapler staple "$dist/$dmg"
    xcrun stapler validate "$dist/$dmg"
    spctl --assess --type execute "$app"
fi

ditto -c -k --sequesterRsrc --keepParent "$app" "$dist/$zip"
(cd "$dist" && shasum -a 256 "$dmg" "$zip" > SHA256SUMS.txt)
cat "$dist/SHA256SUMS.txt"
