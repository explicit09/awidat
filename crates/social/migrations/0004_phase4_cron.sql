-- Phase 4: activate the pg_cron schedules that drive the awidat-social service.
--
-- Three jobs call the deployed Rust service over pg_net (net.http_post):
--   1. awidat-publish-tick     (every minute)  -> POST /internal/tick
--   2. awidat-poll-processing  (every minute)  -> POST /internal/cron/poll-processing
--   3. awidat-refresh-tokens   (every 5 min)   -> POST /internal/cron/refresh-tokens
--
-- The service base URL and the shared secret are read from Supabase Vault
-- (vault.decrypted_secrets) rather than embedded as literals, so rotating the
-- secret does not require a migration. Create them once per project (Dashboard
-- → Settings → Vault, or SQL) BEFORE applying this migration:
--
--   SELECT vault.create_secret('https://<app>.fly.dev', 'awidat_service_base_url');
--   SELECT vault.create_secret('<SERVICE_SHARED_SECRET>', 'awidat_service_secret');
--
-- ── Crash-safety invariant (read before adding any "stuck job" cleanup) ───────
-- Every publish job is a durable Postgres row. claim_due_publish_jobs only
-- re-selects status='scheduled' (and uses FOR UPDATE SKIP LOCKED so overlapping
-- ticks never double-claim). A worker crash mid-tick therefore leaves a job in
-- one of two safe states:
--   * still 'scheduled'              -> re-picked next minute;
--   * 'uploading' / 'processing'    -> advanced by the cancel-race re-read and
--                                       the poll-processing sweep.
-- A retryable provider error reschedules the job to a future scheduled_for with
-- a backoff (Phase 4 domain logic), so it, too, is just re-picked later. Do NOT
-- add a fragile timeout-based requeue of 'uploading' rows without understanding
-- this — it would risk double-posting. (A bounded reaper is a deferred, optional
-- hardening tracked in the Phase 4 plan's open risks.)

-- Idempotent (re)registration: unschedule first so re-applying this migration
-- doesn't error on a duplicate job name.
DO $$
DECLARE
    base_url    text;
    secret      text;
    auth_header text;
BEGIN
    SELECT decrypted_secret INTO base_url
        FROM vault.decrypted_secrets WHERE name = 'awidat_service_base_url';
    SELECT decrypted_secret INTO secret
        FROM vault.decrypted_secrets WHERE name = 'awidat_service_secret';

    IF base_url IS NULL OR secret IS NULL THEN
        RAISE EXCEPTION
            'Vault secrets awidat_service_base_url / awidat_service_secret must exist before applying 0004_phase4_cron.sql';
    END IF;

    auth_header := 'Bearer ' || secret;

    -- Remove any prior registrations (no-op if absent).
    PERFORM cron.unschedule('awidat-publish-tick')    WHERE EXISTS (SELECT 1 FROM cron.job WHERE jobname = 'awidat-publish-tick');
    PERFORM cron.unschedule('awidat-poll-processing') WHERE EXISTS (SELECT 1 FROM cron.job WHERE jobname = 'awidat-poll-processing');
    PERFORM cron.unschedule('awidat-refresh-tokens')  WHERE EXISTS (SELECT 1 FROM cron.job WHERE jobname = 'awidat-refresh-tokens');

    -- 1. Minute-tick firing: claim due jobs and fire them.
    PERFORM cron.schedule(
        'awidat-publish-tick',
        '* * * * *',
        format(
            $job$
            SELECT net.http_post(
                url     := %L,
                headers := jsonb_build_object('Content-Type', 'application/json', 'Authorization', %L),
                body    := '{}'::jsonb
            );
            $job$,
            base_url || '/internal/tick',
            auth_header
        )
    );

    -- 2. Poll-processing: advance Processing jobs to Published/Failed.
    PERFORM cron.schedule(
        'awidat-poll-processing',
        '* * * * *',
        format(
            $job$
            SELECT net.http_post(
                url     := %L,
                headers := jsonb_build_object('Content-Type', 'application/json', 'Authorization', %L),
                body    := '{}'::jsonb
            );
            $job$,
            base_url || '/internal/cron/poll-processing',
            auth_header
        )
    );

    -- 3. Token-refresh sweep: keep access tokens fresh ahead of fire-time.
    PERFORM cron.schedule(
        'awidat-refresh-tokens',
        '*/5 * * * *',
        format(
            $job$
            SELECT net.http_post(
                url     := %L,
                headers := jsonb_build_object('Content-Type', 'application/json', 'Authorization', %L),
                body    := '{}'::jsonb
            );
            $job$,
            base_url || '/internal/cron/refresh-tokens',
            auth_header
        )
    );
END
$$;

-- ── Down migration (manual) ──────────────────────────────────────────────────
-- To reverse this migration:
--   SELECT cron.unschedule('awidat-publish-tick');
--   SELECT cron.unschedule('awidat-poll-processing');
--   SELECT cron.unschedule('awidat-refresh-tokens');
--
-- Inspect runs:
--   SELECT * FROM cron.job;
--   SELECT * FROM cron.job_run_details ORDER BY start_time DESC LIMIT 20;
--   SELECT * FROM net._http_response ORDER BY created DESC LIMIT 20;
