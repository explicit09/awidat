# Consumer Release Proof

Date: 2026-06-10

## Branch

- Branch: `codex/consumer-readiness-salvage`
- Source branch used only for salvage: `codex/consumer-release-readiness`
- Baseline: current `origin/main`

## Salvaged In This Branch

- First-launch welcome state now records timestamped data-flow consent with
  `montage:welcome:consent`.
- Legacy agent composer and secondary `start_turn` entry points now gate
  unauthenticated use and open auth setup instead of sending raw agent requests.
- OpenRouter generated-media jobs now include explicit cost-confirmation text in
  approval keys.
- OpenRouter generated-media records can carry configured estimated cost and
  provider-reported actual cost.
- Generated-media UI now shows OpenRouter estimate/actual cost labels or
  `cost unknown`.

## Verification Receipt

Verified on this branch:

- `node --experimental-strip-types tests/welcome.test.ts`: PASS
- `node --experimental-strip-types tests/composer-auth-gate.test.ts`: PASS
- `node --experimental-strip-types tests/generated-media-cost.test.ts`: PASS
- `cargo test -p montage-core openrouter -- --nocapture`: PASS, 9 tests
- `cargo fmt --all -- --check`: PASS
- `pnpm --dir apps/desktop build`: PASS

## Still Not Proven

- Signed/notarized production macOS artifact.
- Windows artifact and Authenticode signing.
- Intel macOS artifact coverage.
- Updater path.
- Production crash reporting.
- Packaged Python model-weight first-launch behavior.
- Full packaged-app smoke proving Python indexers run from bundled resources.
