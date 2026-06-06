# twitter_x live evidence

Status: pending.

Record dated evidence here before marking Twitter/X verified in
`docs/social-server/live-evidence-manifest.json`.

Required evidence:
- OAuth sign-in: screenshot or JSON showing `social_accounts` returns the connected Twitter/X test account without tokens.
- Selected account: screenshot or JSON showing the chosen Twitter/X account id in the product surface.
- Metadata validation: title/text, description, tags, media upload capability, and schedule validation output.
- Private or sandbox publish: test-account upload job id and provider response with non-public blast-radius controls.
- Scheduled app-closed firing: hosted cron/tick log proving the due job fired while Awidat was closed.
- Status polling: `social_publish_job` history through terminal state.
- Provider URL: final Twitter/X provider URL.
- Audit history: account/job audit events.
- Negative path: missing scope, reconnect, validation error, media processing failure, or provider requires-action proof.
- Cleanup: deletion/private cleanup proof or documented cleanup limitation.
