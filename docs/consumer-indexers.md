# Consumer Indexer Strategy

Consumer builds should bundle `uv` plus the in-repo Python workspace as desktop
app resources. `uv` is required runtime infrastructure, not an optional
developer convenience.

Current `main` already declares the `uv` sidecar and `python` resource in the
Tauri bundle and resolves them before falling back to developer install paths.
That is the correct foundation to keep: do not port the older
`codex/consumer-release-readiness` Tauri config over it.

Runtime expectations:

- Resolve `uv` from the bundled sidecar first, then from developer install
  paths.
- Resolve the Python workspace from `MONTAGE_PYTHON_ROOT`, then packaged app
  resource locations, then developer checkout paths.
- Keep default Python indexers registered for consumer builds.
- Surface setup-required states when bundled resources or model prerequisites
  are missing.
- Do not replace local indexers with hosted indexing for the first consumer
  release.

Remaining release work:

- Prove the packaged app can run the Python indexers from bundled resources.
- Decide and document the first-launch model-weight strategy for heavyweight
  indexers.
- Keep provider-key requirements visible for indexers that use off-device
  services such as Deepgram or Anthropic.
