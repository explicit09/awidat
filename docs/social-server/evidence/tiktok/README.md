# tiktok live evidence

Status: pending.

Record dated evidence here before marking TikTok verified in
`docs/social-server/live-evidence-manifest.json`.

Required evidence:
- OAuth sign-in: screenshot or JSON showing `social_accounts` returns the connected TikTok test account without tokens.
- Selected account: screenshot or JSON showing the chosen TikTok account id in the product surface.
- Metadata validation: caption/title, privacy/public eligibility, interaction toggles, no ignored description/tag/thumbnail fields, and schedule validation output.
- Metadata edit: proof that a saved metadata change revalidates the bound target through `social_update_target` before firing.
- Private or sandbox publish: sandbox/private upload job id and provider response.
- Scheduled app-closed firing: hosted cron/tick log and `social_fire_due_job`-equivalent status proof showing the due job fired while Montage was closed.
- Status polling: `social_publish_job` history while scheduled plus `social_poll_publish_job` history while provider processing is active.
- Recovery controls: `social_reschedule_job`, `social_retry_job`, and `social_cancel_job` evidence where the job state supports them.
- Provider URL: final TikTok provider URL or sandbox equivalent.
- Audit history: account/job audit events.
- Negative path: missing scope, reconnect, validation error, oversized media rejection, public eligibility, or provider requires-action proof.
- Cleanup: deletion/private cleanup proof or documented cleanup limitation.
