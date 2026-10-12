#!/bin/sh
# Downloads Sparkle (the update framework) once into target/sparkle,
# checked against a pinned checksum, and prints the folder it is in:
# Sparkle.framework, bin/sign_update, bin/generate_keys and LICENSE.
set -eu
cd "$(dirname "$0")/.."

version=2.10.0
sha256=c2bf58aa8387266ac179357b1415d6f2635f044da8be41042af32425dae6da0c

base=target/sparkle
dir="$base/$version"
# A cached copy (CI restores target/) may have lost files; fetch it again.
if [ ! -d "$dir/Sparkle.framework" ] || [ ! -f "$dir/LICENSE" ] || [ ! -x "$dir/bin/sign_update" ]; then
    rm -rf "$dir"
    mkdir -p "$base"
    archive="$base/Sparkle-$version.tar.xz"
    curl -sSfL -o "$archive" \
        "https://github.com/sparkle-project/Sparkle/releases/download/$version/Sparkle-$version.tar.xz"
    if [ "$(shasum -a 256 "$archive" | cut -d' ' -f1)" != "$sha256" ]; then
        rm -f "$archive"
        echo "error: Sparkle $version does not match its pinned checksum" >&2
        exit 1
    fi
    rm -rf "$dir.partial"
    mkdir -p "$dir.partial"
    tar -xf "$archive" -C "$dir.partial"
    mv "$dir.partial" "$dir"
fi
echo "$PWD/$dir"
