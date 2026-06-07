# instagram live evidence

Status: pending.

Record dated evidence here before marking Instagram verified in
`docs/social-server/live-evidence-manifest.json`.

Required evidence:
- OAuth sign-in: screenshot or JSON showing `social_accounts` returns the connected Instagram professional test account without tokens.
- Selected account: screenshot or JSON showing the chosen Instagram account id in the product surface.
- Metadata validation: caption, tags, provider-fetchable artifact URL, and schedule validation output.
- Metadata edit: proof that a saved metadata change revalidates the bound target through `social_update_target` before firing.
- Private or sandbox publish: sandbox/private media container job id and provider response.
- Scheduled app-closed firing: hosted cron/tick log proving the due job fired while Montage was closed.
- Status polling: `social_publish_job` history through terminal state.
- Provider URL: final Instagram provider URL or sandbox equivalent.
- Audit history: account/job audit events.
- Negative path: missing scope, reconnect, validation error, container processing failure, or provider requires-action proof.
- Cleanup: deletion/private cleanup proof or documented cleanup limitation.
