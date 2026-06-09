#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
"$script_dir/check-required-env.sh" \
  APPLE_CERTIFICATE \
  APPLE_CERTIFICATE_PASSWORD \
  KEYCHAIN_PASSWORD

work_dir="${RUNNER_TEMP:-$(mktemp -d)}"
cert_path="$work_dir/montage-release-certificate.p12"
keychain_path="$work_dir/montage-release.keychain-db"

printf '%s' "$APPLE_CERTIFICATE" | base64 --decode > "$cert_path"

security create-keychain -p "$KEYCHAIN_PASSWORD" "$keychain_path"
security set-keychain-settings -lut 21600 "$keychain_path"
security unlock-keychain -p "$KEYCHAIN_PASSWORD" "$keychain_path"
security import "$cert_path" -P "$APPLE_CERTIFICATE_PASSWORD" -A -t cert -f pkcs12 -k "$keychain_path"
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$KEYCHAIN_PASSWORD" "$keychain_path"

identity="$(
  security find-identity -v -p codesigning "$keychain_path" \
    | awk -F '"' '/Developer ID Application/ { print $2; exit }'
)"

if [[ -z "$identity" ]]; then
  echo "Developer ID Application signing identity not found in imported certificate" >&2
  exit 1
fi

echo "APPLE_SIGNING_IDENTITY=$identity" >> "${GITHUB_ENV:-/dev/null}"
echo "APPLE_KEYCHAIN_PATH=$keychain_path" >> "${GITHUB_ENV:-/dev/null}"
echo "Developer ID Application signing identity imported"
