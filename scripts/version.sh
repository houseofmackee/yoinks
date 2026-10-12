#!/bin/sh
# Prints the version from Cargo.toml (0.2.0-beta.2), or with --bundle the
# form used for CFBundleVersion and Sparkle's sparkle:version (0.2.0beta2).
# Sparkle ignores "-beta.2" when comparing, so it would treat 0.2.0-beta.1,
# 0.2.0-beta.2 and 0.2.0 as the same version; it orders 0.2.0alpha1 <
# 0.2.0beta1 < 0.2.0rc1 < 0.2.0 correctly.
set -eu
cd "$(dirname "$0")/.."

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
if [ "${1:-}" = --bundle ]; then
    printf '%s\n' "$version" | sed -E 's/-([A-Za-z]+)\.?([0-9]+)$/\1\2/'
else
    printf '%s\n' "$version"
fi
