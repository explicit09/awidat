#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
"$script_dir/check-required-env.sh" \
  APPLE_CERTIFICATE \
  APPLE_CERTIFICATE_PASSWORD \
  KEYCHAIN_PASSWORD

cleanup_work_dir=0
if [[ -n "${RUNNER_TEMP:-}" ]]; then
  work_dir="$(mktemp -d "${RUNNER_TEMP%/}/montage-release.XXXXXX")"
  keychain_path="$work_dir/montage-release.keychain-db"
else
  work_dir="$(mktemp -d)"
  keychain_dir="$(mktemp -d "${TMPDIR:-/tmp}/montage-release-keychain.XXXXXX")"
  keychain_path="$keychain_dir/montage-release.keychain-db"
  cleanup_work_dir=1
fi
cert_path="$work_dir/montage-release-certificate.p12"

restore_keychain_settings=0
previous_default_keychain=""
previous_keychains=()
if [[ -z "${GITHUB_ENV:-}" ]]; then
  restore_keychain_settings=1
  previous_default_keychain="$(security default-keychain | sed 's/^ *"//; s/"$//')"
  while IFS= read -r keychain; do
    previous_keychains+=("$keychain")
  done < <(security list-keychains -d user | sed 's/^ *"//; s/"$//')
fi

cleanup() {
  if [[ "$restore_keychain_settings" -eq 1 ]]; then
    if [[ -n "$previous_default_keychain" ]]; then
      security default-keychain -s "$previous_default_keychain" || true
    fi
    if [[ "${#previous_keychains[@]}" -gt 0 ]]; then
      security list-keychains -d user -s "${previous_keychains[@]}" "$keychain_path" || true
    else
      security list-keychains -d user -s "$keychain_path" || true
    fi
  fi
  rm -f "$cert_path"
  if [[ "$cleanup_work_dir" -eq 1 ]]; then
    rm -rf "$work_dir"
  fi
}
trap cleanup EXIT

printf '%s' "$APPLE_CERTIFICATE" | base64 --decode > "$cert_path"

security create-keychain -p "$KEYCHAIN_PASSWORD" "$keychain_path"
security default-keychain -s "$keychain_path"
security list-keychains -d user -s "$keychain_path"
security set-keychain-settings -lut 21600 "$keychain_path"
security unlock-keychain -p "$KEYCHAIN_PASSWORD" "$keychain_path"
security import "$cert_path" -P "$APPLE_CERTIFICATE_PASSWORD" -T /usr/bin/codesign -t cert -f pkcs12 -k "$keychain_path"
rm -f "$cert_path"
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$KEYCHAIN_PASSWORD" "$keychain_path"

identity="$(
  security find-identity -v -p codesigning "$keychain_path" \
    | awk -F '"' '/Developer ID Application/ { print $2; exit }'
)"

if [[ -z "$identity" ]]; then
  echo "Developer ID Application signing identity not found in imported certificate" >&2
  exit 1
fi

if [[ -n "${GITHUB_ENV:-}" ]]; then
  echo "APPLE_SIGNING_IDENTITY=$identity" >> "$GITHUB_ENV"
  echo "APPLE_KEYCHAIN_PATH=$keychain_path" >> "$GITHUB_ENV"
else
  printf 'export APPLE_SIGNING_IDENTITY=%q\n' "$identity"
  printf 'export APPLE_KEYCHAIN_PATH=%q\n' "$keychain_path"
fi
echo "Developer ID Application signing identity imported"
