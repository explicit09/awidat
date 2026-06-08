# Open-Source Readiness Baseline

Date: 2026-06-08
Branch: codex/open-source-readiness

## Evidence

```text
vendor/codex-rs/SOURCE:21:  stamp Codex client version as "0.128.0" instead of Montage's inherited
vendor/codex-rs/SOURCE:23:  `Codex Desktop/0.128.0 ... (codex_exec; 0.128.0)` and
vendor/codex-rs/SOURCE:24:  `/codex/models?client_version=0.128.0`; sending `0.1.0` in the suffix
vendor/codex-rs/SOURCE:29:  omit newer websocket client metadata that the installed 0.128.0 CLI does not
vendor/codex-rs/SOURCE:34:  stamp analytics `runtime.codex_rs_version` as "0.128.0"; the installed CLI
vendor/codex-rs/SOURCE:41:  after the User-Agent was fixed. The installed CLI sends `version: 0.128.0`,
docs/social-server/README.md:124:ref `vgkocfbtkzmpklruqmsx`, region `us-east-1`):
docs/social-server/README.md:125:- `SUPABASE_URL=https://vgkocfbtkzmpklruqmsx.supabase.co`
docs/social-server/README.md:232:| `SOCIAL_TOKEN_AEAD_KEY` | Phase 2 | 2 | 64 hex chars = 32-byte ChaCha20-Poly1305 key |
docs/social-server/README.md:275:Store the output as `SOCIAL_TOKEN_AEAD_KEY`.  Pick a short key ID, e.g. `"k1"`.
docs/social-server/README.md:283:  SOCIAL_TOKEN_AEAD_KEY="<64_hex_chars_from_step_1>" \
docs/social-server/README.md:318:- [ ] `SOCIAL_TOKEN_AEAD_KEY` generated and set as Fly secret
crates/social-server/run-local.sh:53:export SOCIAL_TOKEN_AEAD_KEY="${SOCIAL_TOKEN_AEAD_KEY:-c2e3e7c14a025d829f0411ce456f1ca01b3f546218bf21892b3dd7cc0d9efedd}"
crates/social-server/run-local.sh:76:# SUPABASE_URL=https://vgkocfbtkzmpklruqmsx.supabase.co  — for Storage signed URLs
crates/auth/src/lib.rs:16://! ChatGPT sign-in reuses codex's first-party OAuth client id. OpenAI has
vendor/codex-rs/login/src/auth/default_client.rs:141:    let build_version = "0.128.0";
```
