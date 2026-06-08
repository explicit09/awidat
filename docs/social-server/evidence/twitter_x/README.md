# twitter_x live evidence

Status: pending.

Record dated evidence here before marking Twitter/X verified in
`docs/social-server/live-evidence-manifest.json`.

Required evidence:
- OAuth sign-in: screenshot or JSON showing `social_accounts` returns the connected Twitter/X test account without tokens.
- Selected account: screenshot or JSON showing the chosen Twitter/X account id in the product surface.
- Metadata validation: post text, no ignored privacy/description/tag fields, media upload capability, and schedule validation output.
- Metadata edit: proof that a saved metadata change revalidates the bound target through `social_update_target` before firing.
- Private or sandbox publish: test-account upload job id and provider response with non-public blast-radius controls.
- Scheduled app-closed firing: hosted cron/tick log and `social_fire_due_job`-equivalent status proof showing the due job fired while Montage was closed.
- Status polling: `social_publish_job` history while scheduled plus `social_poll_publish_job` history while provider processing is active.
- Recovery controls: `social_reschedule_job`, `social_retry_job`, and `social_cancel_job` evidence where the job state supports them.
- Provider URL: final Twitter/X provider URL.
- Audit history: account/job audit events.
- Negative path: missing scope, reconnect, validation error, oversized media rejection, media processing failure, or provider requires-action proof.
- Cleanup: deletion/private cleanup proof or documented cleanup limitation.
