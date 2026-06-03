-- Phase 1: publishing domain schema mirroring SqliteSocialStore.
--
-- Design choices (load-bearing — do not change without updating PgSocialStore):
--   TEXT columns that hold serialised Rust enums use `text`, not Postgres enums,
--   so adding a new variant never requires ALTER TYPE.
--   Epoch timestamp columns (updated_at, scheduled_for, created_at) use `bigint`
--   (Rust i64 ms/s epochs) — NOT timestamptz.  Converting them would silently
--   break claim_due_publish_jobs comparisons and the FSM round-trip.
--   payload_json stays `text`, not `jsonb`, to preserve exact serde-string
--   equality that the existing test suite asserts
--   (e.g. sqlite_account_listing_does_not_include_encrypted_token_material).

CREATE TABLE IF NOT EXISTS oauth_connections (
    id          text PRIMARY KEY,
    payload_json text NOT NULL
);

CREATE TABLE IF NOT EXISTS connected_accounts (
    id                  text    PRIMARY KEY,
    owner_json          text    NOT NULL,
    provider            text    NOT NULL,
    provider_account_id text    NOT NULL,
    status              text    NOT NULL,
    payload_json        text    NOT NULL,
    updated_at          bigint  NOT NULL
);

-- Mirrors the SQLite UNIQUE INDEX: prevents the same provider account from
-- being connected twice to the same owner (DuplicateConnectedAccount error).
CREATE UNIQUE INDEX IF NOT EXISTS connected_accounts_owner_provider_account
    ON connected_accounts (owner_json, provider, provider_account_id);

CREATE TABLE IF NOT EXISTS oauth_token_secrets (
    connected_account_id text PRIMARY KEY,
    payload_json         text NOT NULL
);

CREATE TABLE IF NOT EXISTS campaign_variant_targets (
    id                   text    PRIMARY KEY,
    campaign_id          text    NOT NULL,
    variant_id           text    NOT NULL,
    connected_account_id text    NOT NULL,
    provider             text    NOT NULL,
    validation_state     text    NOT NULL,
    scheduled_for        bigint  NOT NULL,
    payload_json         text    NOT NULL,
    updated_at           bigint  NOT NULL
);

CREATE INDEX IF NOT EXISTS campaign_variant_targets_variant
    ON campaign_variant_targets (campaign_id, variant_id);

CREATE TABLE IF NOT EXISTS publish_jobs (
    id                   text    PRIMARY KEY,
    campaign_id          text    NOT NULL,
    variant_id           text    NOT NULL,
    connected_account_id text    NOT NULL,
    provider             text    NOT NULL,
    idempotency_key      text    NOT NULL,
    scheduled_for        bigint  NOT NULL,
    status               text    NOT NULL,
    payload_json         text    NOT NULL,
    updated_at           bigint  NOT NULL
);

-- Prevents two jobs from targeting the same (artifact, account) pair.
CREATE UNIQUE INDEX IF NOT EXISTS publish_jobs_idempotency_key
    ON publish_jobs (idempotency_key);

-- Supports claim_due_publish_jobs: WHERE status = 'scheduled' AND scheduled_for <= $now
-- ORDER BY scheduled_for, id — the composite index makes this a sequential
-- index scan that stops at the limit rather than a full table scan.
CREATE INDEX IF NOT EXISTS publish_jobs_due
    ON publish_jobs (status, scheduled_for, id);

CREATE TABLE IF NOT EXISTS publish_job_events (
    id             text    PRIMARY KEY,
    publish_job_id text    NOT NULL,
    event_type     text    NOT NULL,
    actor_type     text    NOT NULL,
    payload_json   text    NOT NULL,
    created_at     bigint  NOT NULL
);

-- Events fetched in insertion order per job.
CREATE INDEX IF NOT EXISTS publish_job_events_job
    ON publish_job_events (publish_job_id, created_at, id);

CREATE TABLE IF NOT EXISTS account_publish_defaults (
    connected_account_id text    PRIMARY KEY,
    payload_json         text    NOT NULL,
    updated_at           bigint  NOT NULL
);

-- payload_json shape mirrors WorkspaceMemberRole serde output (D3: Phase 1 owns
-- this table; Phase 7 only adds RLS + by-user queries against this shape).
CREATE TABLE IF NOT EXISTS workspace_member_roles (
    workspace_id text NOT NULL,
    user_id      text NOT NULL,
    payload_json text NOT NULL,
    PRIMARY KEY (workspace_id, user_id)
);
