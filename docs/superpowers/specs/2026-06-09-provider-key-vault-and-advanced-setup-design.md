# Provider Key Vault and Advanced Setup Design

Date: 2026-06-09

## Decision

Montage stays local-first for now. We will not introduce Montage-managed credits or a hosted billing model in this phase.

The app will support bring-your-own provider keys, but this setup belongs under an Advanced settings surface instead of the normal onboarding path. Provider keys will be stored in one Montage keychain-backed vault item rather than one macOS Keychain item per provider key.

## Goals

- Stop repeated macOS Keychain prompts caused by separate per-provider keychain reads.
- Give desktop users an in-app way to add, test, replace, and remove provider keys.
- Keep provider setup optional and hidden from average users until they need a feature.
- Preserve env vars as a developer override.
- Avoid building a managed-credit business model in this phase.

## Non-Goals

- No hosted Montage credits, subscriptions, billing, or provider proxy service.
- No automatic silent migration that triggers several Keychain prompts.
- No display of full stored secrets after save.
- No removal of env-var support.

## Architecture

### Single Vault

Create one keychain-backed vault item:

- Service: `montage`
- Account: `secrets_vault`

The vault value is versioned JSON:

```json
{
  "version": 1,
  "providers": {
    "deepgram_api_key": {
      "value": "secret",
      "updated_at": "2026-06-09T00:00:00Z"
    },
    "openrouter_api_key": {
      "value": "secret",
      "updated_at": "2026-06-09T00:00:00Z"
    }
  }
}
```

The runtime reads this vault once per process and caches it in memory. All provider secret lookups use this order:

1. Env var override.
2. In-memory vault cache.
3. Explicit legacy fallback only when requested by a migration/import action.

### Legacy Keychain Entries

Existing per-key entries under `montage` and legacy `awidat` remain readable only through a deliberate import flow. Montage should not eagerly probe all legacy entries at launch because that recreates the multi-prompt problem.

The import flow is user-initiated from Settings:

- "Import existing keychain secrets"
- Explain that macOS may ask for access to each old key one time.
- Copy found values into the vault.
- Offer to delete old per-key entries after import, but default to keeping them until the user confirms.

## Settings UX

Add `Settings -> Advanced -> Provider Keys`.

Each provider row shows:

- Provider name: Deepgram, OpenRouter, Hugging Face, Anthropic, Pexels, X/Twitter, etc.
- Status: `Not set`, `Configured`, `Needs attention`.
- Actions: `Add`, `Replace`, `Test`, `Remove`.
- Short capability note, for example "Enables transcription" or "Enables generated media."

When a key is added:

- User pastes the key into a password-style input.
- Montage stores it in the vault.
- The input clears immediately.
- The full value is never rendered again.

When a key is missing:

- Feature surfaces show friendly copy, not env-var names.
- Example: "Deepgram is needed for transcription. Add a key in Advanced settings."
- The call to action opens the matching provider row.

## Error Handling

- Vault read denied: show "Montage could not access its secure provider-key vault" with retry and setup options.
- Invalid key on test: mark the row `Needs attention` and show provider-specific failure text.
- Missing key: do not crash; show the feature-specific setup prompt.
- Corrupt vault JSON: preserve the raw keychain value, show a repair prompt, and do not overwrite automatically.

## Security

- Secrets never appear in logs, telemetry, transcripts, screenshots, or copied debug bundles.
- Settings can show only redacted previews such as `sk-...abcd`.
- Exporting keys is out of scope.
- Deleting a key removes it from the vault and clears the in-memory cache entry.
- Env vars remain process-local overrides and are never written into the vault automatically.

## Testing

- Unit tests for vault JSON parse, write, update, delete, and corrupt data handling.
- Unit tests for lookup precedence: env var wins over vault; vault wins over missing legacy.
- Desktop command tests for provider status, save, remove, and redaction.
- UI tests for the Advanced provider-key list and missing-key prompt routing.
- Regression test: debug startup does not probe multiple keychain entries by default.

## Rollout

1. Keep the debug startup keychain prefetch disabled by default.
2. Add the vault module and lookup API.
3. Add desktop commands for provider status/save/test/remove.
4. Add Advanced provider-key settings UI.
5. Route missing-key feature failures to friendly setup prompts.
6. Add explicit legacy import as a separate final step.

## Open Choice

After import, old per-key entries should not be deleted automatically. The first implementation should offer deletion as an explicit user action only.
