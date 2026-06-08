# youtube live evidence

Status: pending.

Record dated evidence here before marking YouTube verified in
`docs/social-server/live-evidence-manifest.json`.

Evidence file naming:
- Store dated artifacts in this folder with a `YYYY-MM-DD-` prefix.
- Keep screenshots, logs, JSON excerpts, and cleanup notes grouped by the same
  date prefix so the manifest update is auditable.

Redaction:
- Do not commit tokens, refresh tokens, client secrets, service keys, bearer
  headers, cookies, or private OAuth callback query values.
- Redact account emails and non-test personal identifiers unless they are
  already public provider handles needed to prove the selected account.

Required evidence:
- OAuth sign-in: screenshot or JSON showing `social_accounts` returns the connected YouTube test channel without tokens.
- Selected account: screenshot or JSON showing the chosen YouTube account id in the product surface.
- Metadata validation: title, description, tags, thumbnail, privacy, and schedule validation output.
- Metadata edit: proof that a saved metadata change revalidates the bound target through `social_update_target` before firing.
- Private or sandbox publish: private or unlisted upload job id and provider response.
- Scheduled app-closed firing: hosted cron/tick log and `social_fire_due_job`-equivalent status proof showing the due job fired while Montage was closed.
- Status polling: `social_publish_job` history while scheduled plus `social_poll_publish_job` history while provider processing is active.
- Recovery controls: `social_reschedule_job`, `social_retry_job`, and `social_cancel_job` evidence where the job state supports them.
- Provider URL: final private/unlisted YouTube URL.
- Audit history: account/job audit events.
- Negative path: missing scope, reconnect, validation error, or provider requires-action proof.
- Cleanup: deletion/private cleanup proof or documented cleanup limitation.
