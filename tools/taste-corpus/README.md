# taste-corpus: professional editorial ground truth

Pipeline for the taste gate (docs/taste-gate-plan-2026-07-25.md):
recover a professional editor's keep/cut/speed decisions by aligning a
raw recording's transcript against its published edit.

- `align.py` — the aligner CLI. TF-IDF over 15s windows, monotonic
  banded matching with global re-lock (handles pre-show discards longer
  than the band and mid-show structural jumps). Emits decision-list
  JSON per `decision_list_schema.json`. Exit 2 + warning when coverage
  < 50% (wrong pair / heavy reordering / ASR mismatch).
- `decision_list_schema.json` — the contract consumed by montage-eval's
  agreement scorer (Phase B).
- `test_align.py` — synthetic pairs with planted cuts/speed-ups; run
  `python3 -m unittest discover tools/taste-corpus`.

Validated against the 2026-07-06 study's real pairs: BRINK recovers the
study's exact fingerprint (13 speed-ups, 24-min pre-show discard, 100%
coverage); DRONE aligns at 93% with 17 sped runs.

House discipline: decision lists carry a `house` field and scores must
never mix houses — taste polarity does not transfer (study Finding 13).
