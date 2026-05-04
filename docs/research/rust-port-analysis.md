# Rust Port Analysis for the 10 MCP Indexers

**Status:** research, not a decision. Goal: be honest about what porting each indexer would cost and buy us, so "let's just rewrite it in Rust" becomes a per-indexer call instead of an ideology.

**TL;DR:** A full port is not worth it today. Two indexers (`frame-quality-mcp`, `audio-energy-mcp`) are free wins. Two more (`clip-mcp`, `whisper-mcp`) are plausible via candle/whisper-rs but carry real quality risk. The rest range from "regression guaranteed" to "literally pointless." Pragmatic path: replace the trivial ones, keep the hard ones in Python with `uv` while the Python ML ecosystem keeps shipping SOTA models first.

---

## Per-indexer assessment

### 1. whisper-mcp — the big one

- **Current deps:** `whisperx` → `faster-whisper` (CTranslate2 backend) → `pyannote.audio` diarization → `torch`. Most of our dist size and cold-start.
- **Candidates:**
  - **`whisper-rs`** (whisper.cpp bindings, ~0.13.x): mature, fast, GGML quantized weights work well. No diarization. Transcript quality is whisper.cpp quality — a small but real step below CTranslate2 in our experience (timestamp drift, hallucinations on silence).
  - **`candle-transformers`** has Whisper. Works, but slower than whisper.cpp on CPU and the Metal/CUDA story is rougher than torch's. Not the way to ship.
  - **Diarization gap:** no production-grade Rust pyannote. Options: (a) ONNX-export pyannote segmentation+embedding and run via `tract`/`ort` plus your own clustering, or (b) keep diarization in Python and only port ASR. (a) is a research project.
- **Effort: very hard.** ASR alone is moderate; ASR + diarization + word-level alignment (whisperx gives this free via wav2vec2) is multi-week minimum.
- **Quality risk: high.** Realistic outcome is measurable WER regression and worse diarization. Python is genuinely SOTA here; Rust isn't.

### 2. clip-mcp

- **Current deps:** `open_clip_torch` ViT-B-32 OpenAI weights, `torch`, `Pillow`.
- **Candidates:**
  - **`candle-transformers`**: has CLIP. ViT-B-32 OpenAI weights load. Embedding parity is close but not bit-identical — cosine sim within ~1e-4, fine for retrieval, possibly a problem cross-comparing against a Python-generated index.
  - **`fastembed-rs`**: wraps ONNX via `ort`. Ships CLIP variants. Lowest-friction — pre-converted weights, batched inference.
  - **`tract`**: pure-Rust ONNX, no C++ dep. Slower than `ort` but cleaner deployment (true single-binary). Worth benchmarking.
- **Effort: moderate.** Preprocessing (resize/center-crop/ImageNet norm) via `image` + `ndarray` is straightforward; text tokenizer in `tokenizers`.
- **Quality risk: low–moderate.** Embeddings retrieve fine; the real risk is index-format incompatibility if Python-indexed and Rust-indexed corpora ever coexist.

### 3. face-mcp

- **Current deps:** `face_recognition` (dlib HOG detector + ResNet 128-dim embedder), `scikit-learn` DBSCAN.
- **Candidates:**
  - **`dlib-rs`**: thin, under-maintained binding; keeps the dlib C++ dep, defeating the single-binary goal.
  - **ONNX route**: ArcFace or FaceNet via `ort`/`tract`, detector via RetinaFace or YuNet. The actually-good answer. Modern ArcFace beats dlib's 2017 ResNet, so quality goes *up*.
  - **DBSCAN**: trivial via `linfa-clustering`, or 80 lines by hand.
- **Effort: hard.** No single piece is hard, but the embedding space changes — every face cluster needs reindexing and recognition thresholds need revalidation.
- **Quality risk: moderate, mostly upside.** Real risk is the migration window where old and new embeddings coexist and clustering breaks across the boundary.

### 4. shot-mcp

- **Current deps:** `opencv-python` (Farneback dense optical flow) + sidecar JSON.
- **Candidates:**
  - **`opencv-rust`**: real bindings to libopencv. Keeps the OpenCV C++ dep — opposite of the single-binary goal.
  - **Pure Rust:** no production-grade Farneback in pure Rust. Port it (~500 lines, bug farm) or switch algorithms (Lucas-Kanade exists in scattered crates).
- **Effort: hard** pure-Rust, **moderate** via `opencv-rust`.
- **Quality risk: moderate.** A reimplemented Farneback won't be bit-identical to cv2's, and our shot-cut thresholds are tuned against cv2's output. Expect to retune.

### 5. frame-quality-mcp

- **Current deps:** `cv2.Laplacian`, luma stats, `numpy`. Pure pixel arithmetic.
- **Candidates:** **`image` + `ndarray`**. Laplacian is a 3x3 convolution, luma is `0.299R + 0.587G + 0.114B`, variance is a one-liner.
- **Effort: trivial.** A weekend including MCP wiring.
- **Quality risk: none.** Reproducible to float epsilon.

### 6. gaze-mcp

- **Current deps:** `face_recognition` 5-point landmarks → trig heuristic.
- **Candidates:** the landmark detector is the only ML piece. If `face-mcp` ports, gaze inherits the detector and the heuristic translates to Rust in ~50 lines. Standalone, ship a small ONNX landmark model (e.g. PFLD) via `ort`/`tract`.
- **Effort: moderate**, only because of the detector.
- **Quality risk: low** if migrated alongside face-mcp so the landmark provider stays consistent.

### 7. scenedetect-mcp

- **Current deps:** `PySceneDetect` ContentDetector (HSV histogram diff with adaptive thresholds).
- **Candidates:** no direct Rust equivalent. The algorithm is simple — decode frames (via `ffmpeg-next` or piping ffmpeg), HSV, histogram, weighted delta — but you're reimplementing PySceneDetect's scoring, not calling a library. `ffmpeg-next` keeps the ffmpeg C dep, but we already require ffmpeg at runtime, so no net loss.
- **Effort: moderate.** 1–2 weeks to match PySceneDetect defaults closely enough that downstream cut lists stay stable.
- **Quality risk: moderate.** Same retuning problem as shot-mcp: cuts drift, fixtures regenerate.

### 8. audio-energy-mcp

- **Current deps:** `soundfile` + `numpy`, ffmpeg pipe for decode.
- **Candidates:** **`symphonia`** (broad codec support, pure Rust, actively maintained) is the right pick. `hound` if WAV-only. Energy is RMS over windows, trivial.
- **Effort: trivial.** Cleanest port of the 10.
- **Quality risk: none.**

### 9. topic-mcp

- **Current deps:** `sentence-transformers` all-MiniLM-L6-v2, plus clustering (KMeans/HDBSCAN).
- **Candidates:** **`fastembed-rs`** explicitly supports MiniLM-L6-v2 via ONNX — drop-in. **`rust-bert`** works but heavier and slower-moving. **`candle-transformers`** has BERT but more wiring. Clustering: `linfa-clustering` for KMeans; HDBSCAN is sparse in Rust (a couple of crates of varying quality) — plan to port or reimplement if we depend on it.
- **Effort: moderate.** Embedding swap is easy; clustering is the wildcard.
- **Quality risk: low** for embeddings (ONNX MiniLM is byte-equivalent within fp16 noise), **moderate** for clustering if we swap algorithms.

### 10. editorial-moments-mcp

- **Current deps:** `anthropic` SDK, `pydantic`. An HTTP client and a JSON validator.
- **Candidates:** none worth listing. Porting buys literally nothing — no native code replaced, no model being run, no startup cost eliminated. It's already pure orchestration.
- **Effort: trivial but pointless.**
- **Quality risk: none** (same LLM either way).
- **Recommendation: do not port.** This is the backstop that proves "Rust everywhere" is the wrong frame.

---

## Recommended port order

If and when this becomes worth doing, in order of value-per-effort:

1. **`audio-energy-mcp`** — trivial, zero risk, cleanest single-binary win. Use it as the proof-of-concept that the Rust MCP scaffolding works end-to-end.
2. **`frame-quality-mcp`** — also trivial, no model, validates the `image`/`ndarray` pixel-pushing path.
3. **`scenedetect-mcp`** — moderate effort, removes a Python-only dep, tolerable retuning cost.
4. **`shot-mcp`** — only via `opencv-rust` (keeps libopencv) unless we're willing to reimplement Farneback. Defer until 1–3 are landed.
5. **`topic-mcp`** — gated on whether `fastembed-rs` covers our model and whether our clustering is ports-friendly.
6. **`clip-mcp`** — gated on a hard freeze of which CLIP variant we use, because mixing Python-indexed and Rust-indexed embeddings is a footgun.
7. **`face-mcp` + `gaze-mcp`** — port together, because gaze depends on face's landmark provider. Treat the embedding-space change as a forced reindex.
8. **`whisper-mcp`** — last, and only if `whisper-rs` quality has caught up to faster-whisper *and* we have a credible diarization story (likely an ONNX pyannote pipeline). Today, no.
9. **`editorial-moments-mcp`** — **never port.** This stays Python (or moves to TS) forever. It's pure orchestration; Rust adds zero.

### Stays-Python backstops

- **`editorial-moments-mcp`**: see above. Porting is busywork.
- **`whisper-mcp`**: until the Rust ASR + diarization stack reaches parity, porting this is the most likely way to ship a quality regression to users. The 30 MB binary dream is not worth a worse transcript.

### Honest framing

The Python+`uv` distribution model isn't a temporary hack — it's a reasonable steady state. The ML ecosystem ships in Python first, and "Rust port" usually means "ONNX export + `ort`/`tract` + reimplemented preprocessing" — real engineering, not a transpile. The single-binary goal is also partial: a fully-Rust awidat still ships `ffmpeg` alongside, so the real question is "30 MB + ffmpeg" vs. "300 MB Python tree + ffmpeg."

If we do this, do it incrementally, start with the trivial wins, and treat each port as an independent product decision with its own quality bar — not a march toward an ideology.
