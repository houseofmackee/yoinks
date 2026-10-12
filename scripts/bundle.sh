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

version=$(scripts/version.sh)
bundle_version=$(scripts/version.sh --bundle)
app="target/$profile/Yoinks.app"

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "target/$profile/yoinks" "$app/Contents/MacOS/yoinks"
# Regenerate with: swift scripts/make-icon.swift
cp macos/AppIcon.icns "$app/Contents/Resources/AppIcon.icns"
sed -e "s/@VERSION@/$version/g" -e "s/@BUNDLE_VERSION@/$bundle_version/g" \
    macos/Info.plist > "$app/Contents/Info.plist"

# The Share extension ("Yoinks" in Share menus), built from
# swift/ShareExtension.swift unless YOINKS_NO_SHARE is set or there is no
# Swift compiler. Extensions run sandboxed.
share="$app/Contents/PlugIns/YoinksShare.appex"
if [ -z "${YOINKS_NO_SHARE:-}" ] && xcrun --find swiftc >/dev/null 2>&1; then
    mkdir -p "$share/Contents/MacOS"
    sed -e "s/@VERSION@/$version/g" -e "s/@BUNDLE_VERSION@/$bundle_version/g" \
        macos/ShareExtension-Info.plist > "$share/Contents/Info.plist"
    if [ "$profile" = release ]; then optimize=-O; else optimize=-Onone; fi
    xcrun swiftc "$optimize" -parse-as-library -application-extension \
        -module-name YoinksShare -target arm64-apple-macos13.0 \
        -sdk "$(xcrun --show-sdk-path --sdk macosx)" \
        -Xlinker -e -Xlinker _NSExtensionMain \
        -o "$share/Contents/MacOS/YoinksShare" swift/ShareExtension.swift
else
    echo "note: no Share extension (set YOINKS_NO_SHARE, or no Swift compiler)" >&2
fi

# Shortcuts actions: build.rs compiled swift/Intents.swift into the binary
# and left its file lists in target/<profile>/intents; Xcode's metadata
# processor turns them into Metadata.appintents, which is how macOS finds
# the actions. Without Xcode the app works, minus the actions.
intents="target/$profile/intents"
if [ -f "$intents/sources.txt" ] && xcrun --find appintentsmetadataprocessor >/dev/null 2>&1; then
    swiftc=$(xcrun --find swiftc)
    toolchain=$(dirname "$(dirname "$(dirname "$swiftc")")")
    xcode_build=$(xcodebuild -version | sed -n 's/^Build version //p')
    if ! log=$(xcrun appintentsmetadataprocessor \
        --output "$app/Contents/Resources" \
        --toolchain-dir "$toolchain" \
        --module-name YoinksIntents \
        --sdk-root "$(xcrun --show-sdk-path --sdk macosx)" \
        --xcode-version "${xcode_build:-0}" \
        --platform-family macOS \
        --deployment-target 13.0 \
        --target-triple arm64-apple-macos13.0 \
        --source-file-list "$intents/sources.txt" \
        --swift-const-vals-list "$intents/constvalues.txt" \
        --quiet-warnings 2>&1); then
        echo "warning: could not generate the Shortcuts actions' metadata:" >&2
        echo "$log" >&2
    fi
else
    echo "note: no Shortcuts actions (needs Xcode's appintentsmetadataprocessor)" >&2
fi

# Release builds embed Sparkle for updates (YOINKS_NO_SPARKLE=1 skips it),
# without the XPC services only sandboxed apps use. The feed is the latest
# release's appcast.xml (scripts/dist.sh writes it), and both the feed and
# the update must be signed with the key whose public half is below; the
# private half is the release workflow's SPARKLE_PRIVATE_KEY secret.
sparkle_public_key="q+IRljJ/ThhIoCIoBDRPGNlGlj8C+7oSAHkUjf5rYRc="
if [ "$profile" = release ] && [ -z "${YOINKS_NO_SPARKLE:-}" ]; then
    sparkle=$(scripts/sparkle.sh)
    framework="$app/Contents/Frameworks/Sparkle.framework"
    mkdir -p "$app/Contents/Frameworks"
    ditto "$sparkle/Sparkle.framework" "$framework"
    rm -rf "$framework/Versions/B/XPCServices" "$framework/XPCServices"
    cp "$sparkle/LICENSE" "$app/Contents/Resources/Sparkle-LICENSE.txt"
    plist="$app/Contents/Info.plist"
    plutil -insert SUFeedURL -string \
        "https://github.com/houseofmackee/yoinks/releases/latest/download/appcast.xml" "$plist"
    plutil -insert SUPublicEDKey -string "$sparkle_public_key" "$plist"
    plutil -insert SURequireSignedFeed -bool YES "$plist"
    plutil -insert SUVerifyUpdateBeforeExtraction -bool YES "$plist"
fi

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
    # Nested code first, inside out, with the same identity: the Share
    # extension (sandboxed) and Sparkle's executables.
    if [ -d "$share" ]; then
        codesign --force --options runtime "$timestamp" --sign "$SIGN_IDENTITY" \
            --entitlements macos/ShareExtension.entitlements "$share"
    fi
    framework="$app/Contents/Frameworks/Sparkle.framework"
    if [ -d "$framework" ]; then
        for part in "$framework/Versions/B/Autoupdate" "$framework/Versions/B/Updater.app" "$framework"; do
            codesign --force --options runtime "$timestamp" --sign "$SIGN_IDENTITY" "$part"
        done
    fi
    codesign --force --options runtime "$timestamp" --sign "$SIGN_IDENTITY" "$app"
else
    if [ -d "$share" ]; then
        codesign --force --sign "$SIGN_IDENTITY" --entitlements macos/ShareExtension.entitlements "$share"
    fi
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
