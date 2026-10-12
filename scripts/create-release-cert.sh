#!/bin/sh
# Creates the self-signed release signing identity, imports it into the
# login keychain, and exports it to target/release-cert for a backup.
#
# Releases after 0.2.0-beta.1 are signed with a Developer ID instead, and
# the release workflow no longer uses this certificate. It remains for
# release builds on a Mac without an Apple developer account
# (scripts/bundle.sh falls back to it).
#
# Run once per project, ever: every release must be signed with the same
# certificate, because macOS ties permissions (Accessibility, Screen
# Recording) to it. A new certificate makes every user grant them again.
set -eu
cd "$(dirname "$0")/.."

NAME="Yoinks Release"
FILE=Yoinks-Release

if security find-identity -p codesigning | grep -q "\"$NAME\""; then
    echo "error: \"$NAME\" is already in the keychain. A new one would change every" >&2
    echo "build's signature. To get its .p12 for a backup, export it from" >&2
    echo "Keychain Access (My Certificates > right-click \"$NAME\" > Export) instead." >&2
    exit 1
fi

OUT=target/release-cert
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$OUT"

cat > "$WORK/cert.cnf" <<EOF
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = $NAME
[ext]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
EOF

# macOS's LibreSSL writes PKCS#12 files that `security import` accepts
# (OpenSSL 3 defaults to algorithms it rejects). Twenty years: every release
# must be signed by this same certificate.
OPENSSL=/usr/bin/openssl
PASSWORD=$($OPENSSL rand -hex 16)
$OPENSSL req -x509 -newkey rsa:2048 -nodes -days 7300 -config "$WORK/cert.cnf" \
    -keyout "$WORK/key.pem" -out "$WORK/cert.pem" 2>/dev/null
$OPENSSL pkcs12 -export -inkey "$WORK/key.pem" -in "$WORK/cert.pem" \
    -out "$WORK/id.p12" -name "$NAME" -passout "pass:$PASSWORD"
security import "$WORK/id.p12" -k "$HOME/Library/Keychains/login.keychain-db" \
    -P "$PASSWORD" -T /usr/bin/codesign

cp "$WORK/id.p12" "$OUT/$FILE.p12"
printf %s "$PASSWORD" > "$OUT/password.txt"

cat <<EOF
Created "$NAME" in the login keychain and exported it to $OUT:

  $FILE.p12      the certificate and private key
  password.txt   its password

1. Back up $FILE.p12 and its password somewhere safe (a password
   manager). Losing them means a new certificate, which changes the
   signature of builds made with it.
2. Delete $OUT afterwards.
EOF
