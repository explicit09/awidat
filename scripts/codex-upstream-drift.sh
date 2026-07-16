#!/usr/bin/env bash
# Reports commits added to openai/codex (codex-rs/) upstream since the SHA we
# forked from, so a human can decide whether to cherry-pick anything —
# especially security-relevant fixes. This script never merges or modifies
# vendor/codex-rs; it only prints a report. See vendor/codex-rs/SOURCE for the
# fork SHA, cadence, and cherry-pick process.
#
# Usage: scripts/codex-upstream-drift.sh
# Requires: git, standard coreutils. Network access to github.com to fetch
# the upstream repo (shallow, blobless) unless UPSTREAM_CLONE is already set
# to a local clone.
#
# Exit codes:
#   0 - ran successfully, no security-relevant commits flagged
#   1 - ran successfully, security-relevant commits WERE flagged (report above)
#   2 - could not complete the drift check (network/git failure)
set -euo pipefail

REPO_ROOT="$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)"
SOURCE_FILE="$REPO_ROOT/vendor/codex-rs/SOURCE"
UPSTREAM_URL="https://github.com/openai/codex.git"
WORKDIR="$(mktemp -d "${TMPDIR:-/tmp}/codex-upstream-drift.XXXXXX")"
trap 'rm -rf "$WORKDIR"' EXIT

SECURITY_MESSAGE_PATTERN='security|CVE|RUSTSEC|vuln|sanitiz|escape|injection|overflow'
SECURITY_PATH_PATTERN='(^|/)(core/src/safety|core/src/sandbox|sandbox|exec/src/sandbox|landlock|seatbelt)(/|$)'

if [[ ! -f "$SOURCE_FILE" ]]; then
  echo "error: $SOURCE_FILE not found" >&2
  exit 2
fi

FORK_SHA="$(grep -m1 '^Forked at commit:' "$SOURCE_FILE" | sed -E 's/^Forked at commit:[[:space:]]*//')"
if [[ -z "$FORK_SHA" ]]; then
  echo "error: could not parse 'Forked at commit:' line from $SOURCE_FILE" >&2
  exit 2
fi

echo "codex-rs upstream drift report"
echo "generated: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo "fork SHA (from vendor/codex-rs/SOURCE): $FORK_SHA"
echo

if [[ -n "${UPSTREAM_CLONE:-}" ]]; then
  CLONE_DIR="$UPSTREAM_CLONE"
  echo "using existing local clone: $CLONE_DIR"
else
  CLONE_DIR="$WORKDIR/codex"
  echo "fetching $UPSTREAM_URL (shallow) ..."
  if ! git clone --quiet --filter=blob:none --no-checkout "$UPSTREAM_URL" "$CLONE_DIR" 2>&1; then
    echo "error: failed to clone $UPSTREAM_URL — network may be unavailable." >&2
    echo "Set UPSTREAM_CLONE=/path/to/local/codex/clone to run offline against a pre-fetched clone." >&2
    exit 2
  fi
fi

cd "$CLONE_DIR"

# Deepen just enough history to reach the fork SHA, since the initial clone
# above is blobless but still fetches full commit history by default for
# public GitHub repos over HTTPS. If the fork SHA isn't present (e.g. a
# shallow UPSTREAM_CLONE), fail loudly rather than silently reporting nothing.
if ! git cat-file -e "$FORK_SHA^{commit}" 2>/dev/null; then
  echo "fork SHA $FORK_SHA not found in local clone; fetching it explicitly ..."
  if ! git fetch --quiet origin "$FORK_SHA" 2>&1; then
    echo "error: could not fetch fork SHA $FORK_SHA from $UPSTREAM_URL" >&2
    exit 2
  fi
fi

git fetch --quiet origin main 2>&1 || {
  echo "error: could not fetch origin/main from $UPSTREAM_URL" >&2
  exit 2
}

COMMIT_COUNT="$(git rev-list --count "$FORK_SHA..origin/main" -- codex-rs 2>/dev/null || echo 0)"
echo "commits touching codex-rs/ since fork: $COMMIT_COUNT"
echo

if [[ "$COMMIT_COUNT" -eq 0 ]]; then
  echo "No new upstream commits touching codex-rs/ since the recorded fork SHA."
  exit 0
fi

LOG_FILE="$WORKDIR/log.txt"
git log --no-merges --reverse --date=short \
  --pretty=format:'%H|%ad|%s' \
  "$FORK_SHA..origin/main" -- codex-rs > "$LOG_FILE"

echo "=== full commit list (oldest first) ==="
while IFS='|' read -r sha date subject; do
  printf '%s  %s  %s\n' "$date" "${sha:0:12}" "$subject"
done < "$LOG_FILE"
echo

# Flag commits whose subject line looks security-relevant.
FLAGGED_BY_MESSAGE="$WORKDIR/flagged_message.txt"
grep -iE "$SECURITY_MESSAGE_PATTERN" "$LOG_FILE" > "$FLAGGED_BY_MESSAGE" || true

# Flag commits that touch security-sensitive paths, independent of message.
FLAGGED_BY_PATH="$WORKDIR/flagged_path.txt"
: > "$FLAGGED_BY_PATH"
while IFS='|' read -r sha date subject; do
  changed_paths="$(git diff-tree --no-commit-id --name-only -r "$sha" 2>/dev/null || true)"
  if echo "$changed_paths" | grep -qiE "$SECURITY_PATH_PATTERN"; then
    printf '%s|%s|%s\n' "$sha" "$date" "$subject" >> "$FLAGGED_BY_PATH"
  fi
done < "$LOG_FILE"

FLAGGED_TOTAL="$WORKDIR/flagged_all.txt"
cat "$FLAGGED_BY_MESSAGE" "$FLAGGED_BY_PATH" 2>/dev/null | sort -u -t'|' -k1,1 > "$FLAGGED_TOTAL"
FLAGGED_COUNT="$(wc -l < "$FLAGGED_TOTAL" | tr -d ' ')"

echo "=== security-relevant commits (message match: $SECURITY_MESSAGE_PATTERN) ==="
if [[ -s "$FLAGGED_BY_MESSAGE" ]]; then
  while IFS='|' read -r sha date subject; do
    printf '%s  %s  %s\n' "$date" "${sha:0:12}" "$subject"
  done < "$FLAGGED_BY_MESSAGE"
else
  echo "(none)"
fi
echo

echo "=== commits touching security-sensitive paths ($SECURITY_PATH_PATTERN) ==="
if [[ -s "$FLAGGED_BY_PATH" ]]; then
  while IFS='|' read -r sha date subject; do
    printf '%s  %s  %s\n' "$date" "${sha:0:12}" "$subject"
  done < "$FLAGGED_BY_PATH"
else
  echo "(none)"
fi
echo

echo "=== summary ==="
echo "total commits since fork: $COMMIT_COUNT"
echo "flagged as security-relevant: $FLAGGED_COUNT"

if [[ "$FLAGGED_COUNT" -gt 0 ]]; then
  echo
  echo "ACTION NEEDED: review the flagged commits above and decide whether to"
  echo "cherry-pick into vendor/codex-rs/. This script does not merge anything."
  exit 1
fi

exit 0
