# tiktok live evidence

Status: pending.

Record dated evidence here before marking TikTok verified in
`docs/social-server/live-evidence-manifest.json`.

Required evidence:
- OAuth sign-in: screenshot or JSON showing `social_accounts` returns the connected TikTok test account without tokens.
- Selected account: screenshot or JSON showing the chosen TikTok account id in the product surface.
- Metadata validation: caption/description, tags, privacy/public eligibility, and schedule validation output.
- Private or sandbox publish: sandbox/private upload job id and provider response.
- Scheduled app-closed firing: hosted cron/tick log proving the due job fired while Awidat was closed.
- Status polling: `social_publish_job` history through terminal state.
- Provider URL: final TikTok provider URL or sandbox equivalent.
- Audit history: account/job audit events.
- Negative path: missing scope, reconnect, validation error, public eligibility, or provider requires-action proof.
- Cleanup: deletion/private cleanup proof or documented cleanup limitation.
