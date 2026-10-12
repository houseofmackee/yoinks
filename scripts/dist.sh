#!/bin/sh
# Builds everything a GitHub release needs, in target/dist:
#
#   Yoinks-<version>.dmg  from scripts/make-dmg.sh
#   Yoinks-<version>.zip  the same Yoinks.app, zipped with ditto, which keeps
#                         the signature intact
#   SHA256SUMS.txt        checksums of both
#   appcast.xml           Sparkle's update feed for this release, signed,
#                         when YOINKS_SPARKLE_KEY (the path to the EdDSA
#                         private key) is set
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
version=$(scripts/version.sh)
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

# Stapling and validating fetch the ticket from Apple, and CI runners
# sometimes lose that connection: try a few times.
retry() {
    attempt=1
    until "$@"; do
        if [ "$attempt" -ge 5 ]; then
            return 1
        fi
        echo "retrying in 15 s: $*" >&2
        sleep 15
        attempt=$((attempt + 1))
    done
}

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
    retry xcrun stapler staple "$app"
fi

YOINKS_DMG_NO_BUILD=1 scripts/make-dmg.sh >/dev/null
cp "target/release/$dmg" "$dist/$dmg"
if [ -n "$identity" ]; then
    codesign --force --timestamp --sign "$identity" "$dist/$dmg"
fi
if [ -n "$notarize" ]; then
    notarize "$dist/$dmg"
    retry xcrun stapler staple "$dist/$dmg"
    # Fail rather than publish anything Gatekeeper would block.
    retry xcrun stapler validate "$dist/$dmg"
    spctl --assess --type execute "$app"
fi

ditto -c -k --sequesterRsrc --keepParent "$app" "$dist/$zip"
(cd "$dist" && shasum -a 256 "$dmg" "$zip" > SHA256SUMS.txt)
cat "$dist/SHA256SUMS.txt"

# Sparkle downloads the zip; the feed lists only this release, with its
# signature and this version's CHANGELOG.md section as the notes shown in
# the update window, and is itself signed (the app requires a signed
# feed).
# YOINKS_UPDATE_BASE_URL serves a test update from elsewhere.
if [ -n "${YOINKS_SPARKLE_KEY:-}" ]; then
    sign_update="$(scripts/sparkle.sh)/bin/sign_update"
    signature=$("$sign_update" --ed-key-file "$YOINKS_SPARKLE_KEY" -p "$dist/$zip")
    length=$(stat -f %z "$dist/$zip")
    date=$(LC_ALL=C date -u "+%a, %d %b %Y %H:%M:%S +0000")
    repository=https://github.com/houseofmackee/yoinks
    base=${YOINKS_UPDATE_BASE_URL:-$repository/releases/download/v$version}
    if ! notes=$(python3 -I scripts/release-notes.py --html "$version"); then
        echo "warning: CHANGELOG.md has no section for $version; the update window only links to the release" >&2
        notes="<p>Yoinks $version is available. See the <a href=\"$repository/releases/tag/v$version\">release notes</a> for what changed.</p>"
    fi
    cat > "$dist/appcast.xml" <<APPCAST
<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
  <channel>
    <title>Yoinks</title>
    <link>$repository</link>
    <item>
      <title>Yoinks $version</title>
      <pubDate>$date</pubDate>
      <sparkle:version>$(scripts/version.sh --bundle)</sparkle:version>
      <sparkle:shortVersionString>$version</sparkle:shortVersionString>
      <sparkle:minimumSystemVersion>13.0</sparkle:minimumSystemVersion>
      <sparkle:fullReleaseNotesLink>$repository/releases/tag/v$version</sparkle:fullReleaseNotesLink>
      <description><![CDATA[$notes]]></description>
      <enclosure url="$base/$zip" length="$length" type="application/octet-stream" sparkle:edSignature="$signature"/>
    </item>
  </channel>
</rss>
APPCAST
    "$sign_update" --ed-key-file "$YOINKS_SPARKLE_KEY" "$dist/appcast.xml"
    echo "$dist/appcast.xml"
fi
