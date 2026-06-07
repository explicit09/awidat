# montage-social-server — Phase 1 runbook

This runbook covers the exact CLI commands to stand up the Phase 1 infrastructure.
Follow steps in order; each USER step requires a browser or an account you own.

---

## 0. Prerequisites

| Tool | Min version | Install |
|---|---|---|
| `supabase` CLI | latest | `brew install supabase/tap/supabase` |
| `fly` CLI | latest | `brew install flyctl` |
| `psql` | any | `brew install libpq` |
| Rust toolchain | 1.82+ | `rustup update` |

---

## 1. Create the Supabase project

1. Go to https://app.supabase.com → New project.
2. Choose an org, pick a name (`montage`), select a region (e.g. `us-east-1`), and
   set a strong DB password.  Save the password — you need it below.
3. After the project initialises, note:
   - **Project ref** (in the URL: `https://app.supabase.com/project/<ref>`)
   - **Project URL**: `https://<ref>.supabase.co`
   - **Service role key**: Settings → API → service_role
   - **Anon key**: Settings → API → anon (not needed until Phase 7)
   - **DB password**: what you just set

---

## 2. Enable pg_cron and pg_net

In the Supabase dashboard:

1. Database → Extensions → search `pg_cron` → Enable.
2. Database → Extensions → search `pg_net` → Enable (usually pre-enabled).

Or from psql:
```sql
ALTER DATABASE postgres WITH pg_cron;
CREATE EXTENSION IF NOT EXISTS pg_net;
```

---

## 3. Apply migrations

```bash
# Link your local checkout to the Supabase project.
supabase link --project-ref <ref>

# Push all migrations in crates/social/migrations/ to Supabase.
supabase db push
```

Verify:
```sql
-- Run in Supabase SQL editor.
SELECT table_name FROM information_schema.tables
WHERE table_schema = 'public'
ORDER BY table_name;
-- Expected: account_publish_defaults, campaign_variant_targets,
--           connected_accounts, oauth_connections, oauth_token_secrets,
--           publish_job_events, publish_jobs, workspace_member_roles
```

---

## 4. Create the Supabase Storage bucket (D4)

In the dashboard: Storage → New bucket → name it `artifacts`, set to **private**.

Or via the API:
```bash
curl -X POST "https://<ref>.supabase.co/storage/v1/bucket" \
  -H "Authorization: Bearer <service_role_key>" \
  -H "Content-Type: application/json" \
  -d '{"id":"artifacts","name":"artifacts","public":false}'
```

---

## 5. Create the Fly.io app

```bash
# Authenticate.
fly auth login

# Create the app (one-time; name must be globally unique).
fly apps create montage-social

# Set deployment secrets — never committed to the repo.
fly secrets set --app montage-social \
  DATABASE_URL="postgresql://postgres.<ref>:<DB_PASSWORD>@aws-0-us-east-1.pooler.supabase.com:6543/postgres?options=-c%20search_path%3Dmontage_social,public" \
  SERVICE_SHARED_SECRET="$(openssl rand -hex 32)" \
  SUPABASE_URL="https://<ref>.supabase.co" \
  SUPABASE_SERVICE_KEY="<service_role_key>" \
  SUPABASE_JWT_SECRET="<project_jwt_secret>" \
  STORAGE_BUCKET="artifacts"
```

The `DATABASE_URL` above uses the **Supavisor session-pooler** (port 6543, not 5432).
Find it in Supabase → Settings → Database → Connection string → Session pooler.

### Multi-tenant: the `montage_social` schema (IMPORTANT)

Montage shares a single Supabase project (`technologia-builder-network`) with other
products to keep costs down (one DB / auth / compute). Montage's tables live in a
dedicated **`montage_social` Postgres schema**, isolated from the host app's
`public` tables. Because `PgSocialStore` issues unqualified table names, the
`DATABASE_URL` **must** pin the connection's search_path to that schema:

```
?options=-c%20search_path%3Dmontage_social,public
```

(`%20` = space, `%3D` = `=`.) Do NOT `ALTER ROLE ... SET search_path` — the
`postgres`/service role is shared with the host app, which needs `public`. The
per-connection `options` param scopes the search_path to the Montage server only.

For the **current shared project** (`technologia-builder-network`,
ref `vgkocfbtkzmpklruqmsx`, region `us-east-1`):
- `SUPABASE_URL=https://vgkocfbtkzmpklruqmsx.supabase.co`
- Schema `montage_social` + all 9 tables + `pg_cron`/`pg_net` are already applied.
- RLS is enabled deny-all on every `montage_social` table; the service-role
  connection bypasses it (the server is the trusted authorization point).
- `SUPABASE_JWT_SECRET`: Supabase → Settings → API → JWT Settings → JWT Secret
  (HS256). Enables Phase 7 per-user auth on `/social/*`; omit it to keep the
  single-user dev bearer.

---

## 6. Edit fly.toml

Open `crates/social-server/fly.toml` and set `primary_region` to the Fly region
closest to your Supabase project region:

| Supabase region | Fly region |
|---|---|
| us-east-1 | iad |
| ap-southeast-1 | sin |
| eu-west-1 | lhr |

---

## 7. Deploy

```bash
fly deploy --config crates/social-server/fly.toml
fly status --app montage-social
```

Verify:
```bash
curl https://montage-social.fly.dev/health
# Expected: {"status":"ok","service":"montage-social-server"}
```

---

## 8. Smoke-test the Supabase → service network path (Step 8)

Run this in the Supabase SQL editor to prove `pg_net` can reach the service:

```sql
SELECT net.http_post(
    url     := 'https://montage-social.fly.dev/internal/tick',
    headers := jsonb_build_object(
                   'Content-Type',  'application/json',
                   'Authorization', 'Bearer <SERVICE_SHARED_SECRET>'
               ),
    body    := '{}'::jsonb
);

-- Wait a few seconds, then check:
SELECT * FROM net._http_response ORDER BY created DESC LIMIT 5;
-- The most recent row should have status_code = 200.
```

Also check the service log:
```bash
fly logs --app montage-social
# Expected line: "tick processed"
```

For the deployed product, `SOCIAL_FIRING_ENABLED` must be `true` and migration
`0004_phase4_cron.sql` must be applied with the Vault secrets in place. A 200
response from `/internal/tick` while firing is disabled proves only networking,
not that scheduled posts will fire while the desktop app is closed.

Confirm the scheduler jobs are registered:

```sql
SELECT jobname, schedule, active
FROM cron.job
WHERE jobname IN (
  'montage-publish-tick',
  'montage-poll-processing',
  'montage-refresh-tokens'
)
ORDER BY jobname;
```

Before calling social publishing ready for users, complete the provider-level
live verification contract in
[`live-verification.html`](./live-verification.html). The contract requires
private/sandbox publishes for YouTube, TikTok, Instagram, and Twitter/X,
including OAuth sign-in, selected account evidence, scheduled app-closed firing,
provider URL/status proof, audit history, negative paths, and cleanup. Track
the current machine-readable evidence status in
[`live-evidence-manifest.json`](./live-evidence-manifest.json); keep
`allProvidersVerified` false until every provider's live evidence is complete.

---

## 9. Environment variable contract

| Variable | Required | Phase | Notes |
|---|---|---|---|
| `DATABASE_URL` | Yes | 1 | Supavisor session-pooler URL |
| `SERVICE_SHARED_SECRET` | Yes | 1 | Random hex; also stored in Supabase for `pg_net` |
| `BIND_ADDR` | No | 1 | Default `0.0.0.0:3000` |
| `SOCIAL_FIRING_ENABLED` | Yes | 4 | Must be `true` in deployment so pg_cron fires due publish jobs |
| `TIKTOK_PUBLIC_POSTING_ENABLED` | No | 6 | Default `false`; keep false until TikTok app audit clears public/friends posting |
| `SUPABASE_URL` | Yes | 1 | `https://<ref>.supabase.co` |
| `SUPABASE_SERVICE_KEY` | Yes | 1 | service_role key |
| `STORAGE_BUCKET` | No | 1 | Default `artifacts` |
| `GOOGLE_CLIENT_ID` | Phase 2 | 2 | Google OAuth app client ID |
| `GOOGLE_CLIENT_SECRET` | Phase 2 | 2 | Google OAuth client secret (server-only, never in desktop) |
| `SOCIAL_TOKEN_AEAD_KEY` | Phase 2 | 2 | 64 hex chars = 32-byte ChaCha20-Poly1305 key |
| `SOCIAL_TOKEN_KEY_ID` | Phase 2 | 2 | Key identifier stored alongside every token (e.g. "k1") |
| `OAUTH_REDIRECT_BASE` | Phase 2 | 2 | Base URL for OAuth redirect URIs |

---

## Railway alternative

If you prefer Railway over Fly.io:

1. Create a Railway project and connect the repo.
2. Set the same environment variables in the Railway dashboard.
3. Railway auto-detects the Dockerfile at `crates/social-server/Dockerfile`.
4. The `fly.toml` is Fly-specific and ignored by Railway.

The service is stateless (state lives in Supabase); either host works without
code changes.

---

## Phase 1 done checklist

- [ ] Supabase project created, pg_cron + pg_net enabled
- [ ] Migrations applied — 8 tables visible in the dashboard
- [ ] Supabase Storage bucket `artifacts` created
- [ ] Fly.io app deployed, `/health` returns 200 from the public internet
- [ ] Step 8 smoke test: 200 in `net._http_response` and service log shows `tick processed`
- [ ] Platform app-reviews started in parallel (YouTube TOS, TikTok video.publish,
      Instagram instagram_content_publish) — on the critical path for Phases 3/6

Next: Phase 2 — server-side OAuth exchange + AEAD token storage.

---

## Phase 2 setup

### 1. Generate the AEAD key

```bash
# 32 random bytes → 64 hex chars.
openssl rand -hex 32
```

Store the output as `SOCIAL_TOKEN_AEAD_KEY`.  Pick a short key ID, e.g. `"k1"`.

### 2. Set Phase 2 secrets

```bash
fly secrets set --app montage-social \
  GOOGLE_CLIENT_ID="<client_id_from_google_console>" \
  GOOGLE_CLIENT_SECRET="<client_secret_from_google_console>" \
  SOCIAL_TOKEN_AEAD_KEY="<64_hex_chars_from_step_1>" \
  SOCIAL_TOKEN_KEY_ID="k1" \
  OAUTH_REDIRECT_BASE="https://montage-social.fly.dev"
```

`GOOGLE_CLIENT_SECRET` stays on the server.  The desktop app never sees it.

### 3. Configure the Google Cloud OAuth consent screen

1. Google Cloud Console → APIs & Services → Credentials → Create OAuth 2.0 Client ID.
2. Authorized redirect URIs: `https://montage-social.fly.dev/oauth/callback/youtube`
3. Enable the YouTube Data API v3 in the project.
4. In the OAuth consent screen, add the scopes:
   - `https://www.googleapis.com/auth/youtube.upload`
   - `https://www.googleapis.com/auth/youtube.readonly`

### 4. OAuth flow (desktop initiates, server completes)

```
Desktop                          social-server                     Google
──────                           ─────────────                     ──────
POST /oauth/begin/youtube  ──►  save OAuthConnection
                           ◄──  {authorization_url}
                           ──►  open URL in browser
                                                              ──►  grant
GET /oauth/callback/youtube  ◄──                              ◄──  redirect
                                POST oauth2.googleapis.com/token
                                GET  youtube/v3/channels?mine=true
                                encrypt tokens (ChaCha20-Poly1305)
                                save ConnectedAccount + TokenSecret
                           ◄──  {status: "ok", account_id}
```

### Phase 2 done checklist

- [ ] `SOCIAL_TOKEN_AEAD_KEY` generated and set as Fly secret
- [ ] `GOOGLE_CLIENT_ID` + `GOOGLE_CLIENT_SECRET` set as Fly secrets
- [ ] `OAUTH_REDIRECT_BASE` set as Fly secret
- [ ] Redirect URI registered in Google Cloud Console
- [ ] YouTube Data API v3 enabled
- [ ] End-to-end OAuth flow tested with a real Google account
