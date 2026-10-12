#!/bin/sh
# Builds Yoinks.app (Apple Silicon) around the cargo binary and registers it
# with macOS so the yoinks:// scheme and the "Add to Yoinks" service work.
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
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "target/$profile/yoinks" "$app/Contents/MacOS/yoinks"
# Regenerate with: swift scripts/make-icon.swift
cp macos/AppIcon.icns "$app/Contents/Resources/AppIcon.icns"
sed "s/@VERSION@/$version/g" macos/Info.plist > "$app/Contents/Info.plist"

# Debug builds are signed ad hoc. Release builds are signed, in order of
# preference, with a valid "Developer ID Application" identity (of team
# YOINKS_TEAM_ID, if set), which scripts/dist.sh can notarize, or with the
# self-signed "Yoinks Release" certificate. Either keeps macOS permissions
# across updates; ad-hoc signatures change with every build. SIGN_IDENTITY
# overrides this ("-" for ad hoc).
release_identity="Yoinks Release"
if [ -z "${SIGN_IDENTITY:-}" ]; then
    SIGN_IDENTITY=-
    if [ "$profile" = release ]; then
        developer_id=$(security find-identity -v -p codesigning |
            sed -n 's/.*"\(Developer ID Application: .*\)"$/\1/p' |
            grep -F "${YOINKS_TEAM_ID:+($YOINKS_TEAM_ID)}" | head -n 1 || true)
        if [ -n "$developer_id" ]; then
            SIGN_IDENTITY=$developer_id
        elif security find-identity -p codesigning | grep -q "\"$release_identity\""; then
            SIGN_IDENTITY=$release_identity
        else
            echo "warning: no Developer ID or \"$release_identity\" in the keychain; signing ad hoc" >&2
        fi
    fi
fi
if [ "$profile" = release ]; then
    # Hardened runtime, and a secure timestamp for a Developer ID: both are
    # required for notarization.
    case "$SIGN_IDENTITY" in
        "Developer ID"*) timestamp=--timestamp ;;
        *) timestamp=--timestamp=none ;;
    esac
    codesign --force --options runtime "$timestamp" --sign "$SIGN_IDENTITY" "$app"
else
    codesign --force --sign "$SIGN_IDENTITY" "$app"
fi
codesign --verify --strict "$app"

# Register with Launch Services so yoinks:// and the service work right away
# (skipped on CI with YOINKS_SKIP_REGISTER=1).
if [ -z "${YOINKS_SKIP_REGISTER:-}" ]; then
    lsregister=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
    "$lsregister" -f "$app"
    /System/Library/CoreServices/pbs -update
fi

echo "$app"
