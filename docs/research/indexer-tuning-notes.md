# Indexer tuning notes

Empirical observations from real-video sessions. The indexers ship with
defaults that work; this doc captures *why* certain knobs are where they
are, and which "obvious" tunings make things worse.

## scenedetect-mcp

**Default threshold: 27.0 (PySceneDetect ContentDetector default)**
**Min shot length: 0.4s**

Tested against the 44-min Samsung Galaxy retrospective: produced 303
shots, distribution median ~5s with a long tail to 170s. Initial
intuition that "303 sounds high" turned out wrong.

The downstream shot-mcp classification confirms the count is right:

- 187 no-face (62%) — B-roll / product shots / screen recordings
- 86 medium (28%) — host talking-head framing
- 29 wide (10%) — establishing
- 1 close-up (<1%) — outlier

This is exactly the shape of a heavily-edited tech YouTube video. The
agent in the live TUI session correctly read this as "rich B-roll
library — great for cutaways."

**Action: no tuning needed.** Threshold 27.0 is reporting genuine cuts.

## topic-mcp

**Default threshold: 0.55**
**Window size: 5 sentences**
**Min segment length: 30s**

Tested against the same 44-min retrospective: produced 33 topics
averaging 80s each — way too fine for editorial chapter navigation.
Investigation found two real bugs that are NOT solved by parameter
tuning:

### Bug 1: cosine-sim distribution makes the absolute threshold useless

Empirical: max cosine similarity across all adjacent 5-sentence windows
in this transcript was **0.416**. Every window pair is below any
reasonable threshold (0.45–0.70). That means `BOUNDARY_THRESHOLD`
doesn't filter at all — it just lets the `WINDOW_SENTENCES` suppression
cap the boundary rate at one per 5 segments.

Spoken-word transcripts have low inter-window coherence by nature; the
threshold-on-absolute-sim approach can't tell "real topic shift" from
"speaker normal cadence."

**Right fix:** local-minimum detection on the sim curve. Boundaries are
valleys relative to neighbors, not absolute lows. This is what original
TextTiling does. **Tracked as #123.**

### Bug 2: merge-into-previous cascade

`_build_topics` merges sub-floor chunks into the *previous* topic
unconditionally. Cascade behavior:

1. Chunks are uniformly ~30-60s (boundary fires every 5 segments × ~6s/segment).
2. Set floor to 60+ → every chunk fails floor → every chunk merges
   into previous → runaway collapse.
3. Result: floor=30 produces 33 topics, floor≥60 produces 1 topic.
   Cliff, not gradient.

Verified by sweeping floor values 30/60/90/120/180/240/300:
all values ≥60 produce 1 topic.

**Right fix:** direction-aware merge. Compare sub-floor chunk to BOTH
neighbors' embeddings, merge with the more-similar one. Breaks the
cascade because a chunk in the middle of a topic merges sideways
(absorbed locally) while a boundary between two distinct topics
doesn't get falsely glued together. **Tracked as #122.**

### Why we're keeping defaults at 0.55 / 30s anyway

Until #122 + #123 land, parameter tuning makes things worse:

- Lowering threshold below 0.55 → never fires (max sim is 0.416)
- Raising threshold → no effect (every position already triggers)
- Raising MIN_SEGMENT_S → cliff to 1 topic
- Lowering MIN_SEGMENT_S below 30s → even more sub-beat noise

The 33-topic baseline is at least navigable. The agent in live
sessions has been ignoring topics anyway and using `find_beat`
(editorial-moments-mcp output) as the chapter primitive — so the
over-segmentation doesn't actively block any workflow today.

## When this changes

Add an entry here whenever an indexer's default is touched. Include:

1. The before/after parameter
2. The real-video evidence that motivated the change
3. The downstream effect (did it help, hurt, or no-op the agent's behavior?)
