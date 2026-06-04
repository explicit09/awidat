#!/usr/bin/env bash
# Run the awidat-social server on localhost against the live Supabase DB.
#
# This is for LOCAL TESTING — no Fly, no deploy. The server binds 127.0.0.1:3000;
# point the desktop at AWIDAT_SOCIAL_SERVER_URL=http://127.0.0.1:3000.
#
# SOCIAL_FIRING_ENABLED stays "false": the localhost server does NOT fire jobs on
# a schedule (that's the deployed environment's pg_cron job). With firing off, the
# boot migration step skips the cron/extension migrations, so it will NOT touch or
# re-enable the deployed cron schedules. To exercise the worker by hand, POST
# /internal/tick yourself (see the curl at the bottom).
#
# Fill in the two <PLACEHOLDER>s, or copy this to .env.local (gitignored) and
# source it. Secrets must never be committed.
set -euo pipefail

# ── Required ────────────────────────────────────────────────────────────────
# Supabase session-pooler URL. Get DB_PASSWORD from Supabase → Settings →
# Database. The search_path option is MANDATORY (PgSocialStore uses unqualified
# table names → they must resolve to the awidat_social schema).
export DATABASE_URL="postgresql://postgres.vgkocfbtkzmpklruqmsx:<DB_PASSWORD>@aws-0-us-east-1.pooler.supabase.com:6543/postgres?options=-c%20search_path%3Dawidat_social,public"

# Bearer the desktop sends to /social/* (dev single-user mode). Must match the
# desktop's AWIDAT_SOCIAL_AUTH_TOKEN. Any non-empty value works for local dev.
export DESKTOP_AUTH_TOKEN="local-dev-token"

# Bearer for the /internal/* worker routes (only needed if you curl them).
export SERVICE_SHARED_SECRET="local-internal-token"

# AEAD key for token-at-rest encryption (64 hex chars = 32 bytes). Generated for
# local use; rotate for any real environment. Generate a fresh one with:
#   openssl rand -hex 32
export SOCIAL_TOKEN_AEAD_KEY="c2e3e7c14a025d829f0411ce456f1ca01b3f546218bf21892b3dd7cc0d9efedd"
export SOCIAL_TOKEN_KEY_ID="local-k1"

# ── Local-test posture ──────────────────────────────────────────────────────
export SOCIAL_FIRING_ENABLED="false"   # no autonomous firing; skips cron migration on boot
export BIND_ADDR="127.0.0.1:3000"
export ARTIFACT_BASE_DIR="${ARTIFACT_BASE_DIR:-$PWD/.artifacts-local}"
mkdir -p "$ARTIFACT_BASE_DIR"

# ── Optional (leave empty for local route testing) ──────────────────────────
# GOOGLE_CLIENT_ID / GOOGLE_CLIENT_SECRET — real YouTube OAuth (for the OAuth flow)
# SUPABASE_URL=https://vgkocfbtkzmpklruqmsx.supabase.co  — for Storage signed URLs
# SUPABASE_SERVICE_KEY=...                               — for Storage signed URLs
# SUPABASE_JWT_SECRET=...                                — Phase 7 per-user auth
# OAUTH_REDIRECT_BASE=http://127.0.0.1:3000

echo "Starting awidat-social on $BIND_ADDR (firing disabled; cron migration skipped)..."
exec cargo run -p awidat-social-server

# ── Smoke test (in another terminal) ────────────────────────────────────────
#   curl -s http://127.0.0.1:3000/health
#   curl -s http://127.0.0.1:3000/providers
#   # user route (dev bearer):
#   curl -s -H "Authorization: Bearer local-dev-token" http://127.0.0.1:3000/social/accounts
#   # manual worker tick (internal bearer):
#   curl -s -X POST -H "Authorization: Bearer local-internal-token" http://127.0.0.1:3000/internal/tick
