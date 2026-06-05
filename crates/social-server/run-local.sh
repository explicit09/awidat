#!/usr/bin/env bash
# Run the awidat-social server on localhost against the live Supabase DB.
#
# This is for LOCAL TESTING — no Fly, no deploy. The server binds 127.0.0.1:3000;
# point the desktop at AWIDAT_SOCIAL_SERVER_URL=http://127.0.0.1:3000.
#
# SOCIAL_FIRING_ENABLED defaults to "false": the localhost server does NOT fire
# jobs unless explicitly enabled. With firing off, boot skips cron/extension
# migrations, so it will NOT touch or re-enable deployed cron schedules.
#
# When SOCIAL_FIRING_ENABLED=true, this script runs a local-only cron loop that
# calls the same /internal/* worker routes pg_cron calls in deployment. That makes
# desktop local dev behave like the deployed scheduler without applying cron SQL.
#
# DATABASE_URL must use the Supabase SESSION pooler (port 5432), NOT transaction
# mode (6543): the sync `postgres` crate uses named prepared statements, which
# collide under transaction pooling (42P05 "prepared statement s0 already
# exists"). Verified working end-to-end on 5432. See .env.local.
#
# Fill in the two <PLACEHOLDER>s, or copy this to .env.local (gitignored) and
# source it. Secrets must never be committed.
set -euo pipefail
cd "$(dirname "$0")"

# ── Secrets (gitignored) ─────────────────────────────────────────────────────
# Put DATABASE_URL (with the Supabase DB password, URL-encoded) in .env.local —
# that file is gitignored so the password never lands in git. This committed
# script holds NO secret. See .env.local for the exact connection-string shape;
# the search_path=awidat_social option is MANDATORY (PgSocialStore uses
# unqualified table names).
if [[ -f .env.local ]]; then
  # shellcheck disable=SC1091
  source .env.local
fi
if [[ -z "${DATABASE_URL:-}" ]]; then
  echo "DATABASE_URL is unset. Put it in crates/social-server/.env.local (gitignored)." >&2
  exit 1
fi

# All values below use ${VAR:-default} so anything exported in .env.local wins
# (put the Google OAuth secrets there — never in this committed file).

# Bearer the desktop sends to /social/* (dev single-user mode). Must match the
# desktop's AWIDAT_SOCIAL_AUTH_TOKEN. Any non-empty value works for local dev.
export DESKTOP_AUTH_TOKEN="${DESKTOP_AUTH_TOKEN:-local-dev-token}"

# Bearer for the /internal/* worker routes (only needed if you curl them).
export SERVICE_SHARED_SECRET="${SERVICE_SHARED_SECRET:-local-internal-token}"

# AEAD key for token-at-rest encryption (64 hex chars = 32 bytes). Generated for
# local use; rotate for any real environment. Generate a fresh one with:
#   openssl rand -hex 32
export SOCIAL_TOKEN_AEAD_KEY="${SOCIAL_TOKEN_AEAD_KEY:-c2e3e7c14a025d829f0411ce456f1ca01b3f546218bf21892b3dd7cc0d9efedd}"
export SOCIAL_TOKEN_KEY_ID="${SOCIAL_TOKEN_KEY_ID:-local-k1}"

# ── Google/YouTube OAuth (from .env.local; required for the Connect flow) ─────
# Create a Google Cloud OAuth client (Web application) with redirect URI
# http://127.0.0.1:3000/oauth/callback/youtube and put the id/secret in
# .env.local as GOOGLE_CLIENT_ID / GOOGLE_CLIENT_SECRET. Until set, Connect
# returns 503 "Google OAuth not configured".
export GOOGLE_CLIENT_ID="${GOOGLE_CLIENT_ID:-}"
export GOOGLE_CLIENT_SECRET="${GOOGLE_CLIENT_SECRET:-}"
export OAUTH_REDIRECT_BASE="${OAUTH_REDIRECT_BASE:-http://127.0.0.1:3000}"

# ── Local-test posture ──────────────────────────────────────────────────────
export SOCIAL_FIRING_ENABLED="${SOCIAL_FIRING_ENABLED:-false}"   # no autonomous firing; skips cron migration on boot
export AWIDAT_SOCIAL_SKIP_INFRA_MIGRATIONS="${AWIDAT_SOCIAL_SKIP_INFRA_MIGRATIONS:-true}"
# Local direct-publish tests should honor the visibility selected in the desktop.
# The server binary itself still defaults this to true for deployed/pre-audit use.
export YOUTUBE_FORCE_PRIVATE="${YOUTUBE_FORCE_PRIVATE:-false}"
export BIND_ADDR="${BIND_ADDR:-127.0.0.1:3000}"
export ARTIFACT_BASE_DIR="${ARTIFACT_BASE_DIR:-$PWD/.artifacts-local}"
mkdir -p "$ARTIFACT_BASE_DIR"

# ── Optional (for Storage uploads / Phase 7 auth; leave empty for now) ───────
# SUPABASE_URL=https://vgkocfbtkzmpklruqmsx.supabase.co  — for Storage signed URLs
# SUPABASE_SERVICE_KEY=...                               — for Storage signed URLs
# SUPABASE_JWT_SECRET=...                                — Phase 7 per-user auth

if [[ "$SOCIAL_FIRING_ENABLED" != "true" ]]; then
  echo "Starting awidat-social on $BIND_ADDR (firing disabled; cron migration skipped)..."
  exec cargo run -p awidat-social-server
fi

LOCAL_CRON_INTERVAL_SECS="${AWIDAT_SOCIAL_LOCAL_CRON_INTERVAL_SECS:-15}"
echo "Starting awidat-social on $BIND_ADDR (local firing loop enabled; cron migration skipped)..."
cargo run -p awidat-social-server &
server_pid=$!

cleanup() {
  kill "$server_pid" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

for _ in {1..120}; do
  if curl -fsS "http://$BIND_ADDR/health" >/dev/null 2>&1; then
    break
  fi
  if ! kill -0 "$server_pid" 2>/dev/null; then
    wait "$server_pid"
    exit $?
  fi
  sleep 1
done

if ! kill -0 "$server_pid" 2>/dev/null; then
  wait "$server_pid"
  exit $?
fi

echo "Local social cron loop active every ${LOCAL_CRON_INTERVAL_SECS}s."
while kill -0 "$server_pid" 2>/dev/null; do
  curl -fsS -X POST \
    -H "Authorization: Bearer $SERVICE_SHARED_SECRET" \
    "http://$BIND_ADDR/internal/tick" >/dev/null || true
  curl -fsS -X POST \
    -H "Authorization: Bearer $SERVICE_SHARED_SECRET" \
    "http://$BIND_ADDR/internal/cron/poll-processing" >/dev/null || true
  sleep "$LOCAL_CRON_INTERVAL_SECS"
done

wait "$server_pid"

# ── Smoke test (in another terminal) ────────────────────────────────────────
#   curl -s http://127.0.0.1:3000/health
#   curl -s http://127.0.0.1:3000/providers
#   # user route (dev bearer):
#   curl -s -H "Authorization: Bearer local-dev-token" http://127.0.0.1:3000/social/accounts
#   # manual worker tick (internal bearer):
#   curl -s -X POST -H "Authorization: Bearer local-internal-token" http://127.0.0.1:3000/internal/tick
