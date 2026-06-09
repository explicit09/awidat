#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -ne 1 ]]; then
  echo "usage: $0 PATH_TO_DMG" >&2
  exit 2
fi

dmg_path="$1"
if [[ ! -f "$dmg_path" ]]; then
  echo "DMG does not exist: $dmg_path" >&2
  exit 1
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
"$script_dir/check-required-env.sh" APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID

xcrun notarytool submit "$dmg_path" \
  --apple-id "$APPLE_ID" \
  --password "$APPLE_PASSWORD" \
  --team-id "$APPLE_TEAM_ID" \
  --wait

xcrun stapler staple "$dmg_path"
xcrun stapler validate "$dmg_path"
spctl --assess --type open --context context:primary-signature -v "$dmg_path"
