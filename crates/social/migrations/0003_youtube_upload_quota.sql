-- Tracks daily YouTube Data API upload count per Google Cloud project.
-- Enforced in the server worker before calling execute_claimed_upload_job.
-- count resets naturally by day (keyed on the epoch-day integer).
CREATE TABLE IF NOT EXISTS provider_upload_quota (
    project_key TEXT    NOT NULL,
    day_epoch   BIGINT  NOT NULL,  -- floor(unix_seconds / 86400)
    count       INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (project_key, day_epoch)
);
