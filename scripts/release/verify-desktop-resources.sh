#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -ne 1 ]]; then
  echo "usage: $0 PATH_TO_MONTAGE_APP" >&2
  exit 2
fi

app_path="$1"
resources_dir="$app_path/Contents/Resources"

if [[ ! -d "$resources_dir" ]]; then
  echo "missing app resources directory: $resources_dir" >&2
  exit 1
fi

find_resource_dir() {
  local name="$1"
  local marker="$2"
  local direct="$resources_dir/$name"
  local up="$resources_dir/_up_/_up_/_up_/$name"

  for candidate in "$direct" "$up"; do
    if [[ -e "$candidate/$marker" ]]; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done

  echo "missing bundled $name resource with marker $marker under $resources_dir" >&2
  exit 1
}

python_root="$(find_resource_dir python packages/montage-mcp/pyproject.toml)"
skills_root="$(find_resource_dir skills .bundled-marker)"

required_skills=(
  auto-cutter
  podcast-editor
  podcast-episode-producer
  podcast-hook
  short-form
  talking-head-vertical
  viral-clip-extractor
)

for skill in "${required_skills[@]}"; do
  if [[ ! -f "$skills_root/$skill/SKILL.md" ]]; then
    echo "missing bundled skill: $skills_root/$skill/SKILL.md" >&2
    exit 1
  fi
done

echo "desktop resources verified: python=$python_root skills=$skills_root"
