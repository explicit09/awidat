# Third-party notices

This file summarizes notable third-party and separately licensed materials
included in the Montage source tree. It is not a replacement for the license
files preserved with each component.

## vendor/codex-rs

`vendor/codex-rs/` is a maintained Montage fork of OpenAI Codex.

- Upstream: `https://github.com/openai/codex`
- License: Apache License 2.0
- Provenance and local fork notes: `vendor/codex-rs/SOURCE`

## vendor/codex-rs/vendor/bubblewrap

`vendor/codex-rs/vendor/bubblewrap/` contains vendored bubblewrap source used
by the Codex fork.

- License text: `vendor/codex-rs/vendor/bubblewrap/COPYING`
- Source availability: the modified source is included in the vendored tree.
- LGPL note: preserve the license text and source availability for Linux builds
  that compile or distribute this component.

## OpenSSL / openssl-sys

The Rust workspace includes `openssl-sys` for platform-specific OpenSSL linkage.

- Dependency declaration: `Cargo.toml`
- Upstream license terms depend on the linked OpenSSL distribution.
- Packaging note: release builds should document the OpenSSL distribution they
  link or bundle on each platform.

## pyannote speaker diarization

The Python whisper MCP can use `pyannote/speaker-diarization-community-1` for
speaker diarization when `HF_TOKEN` is configured and the model terms have been
accepted.

- Model: `pyannote/speaker-diarization-community-1`
- License noted in code: CC-BY-4.0
- References: `python/packages/whisper-mcp/src/whisper_mcp/__init__.py`,
  `python/SMOKE.md`

## Deepgram

The Python whisper MCP can use Deepgram as an optional transcription or
diarization backend when `DEEPGRAM_API_KEY` is configured.

- Provider: `https://deepgram.com/`
- References: `python/packages/whisper-mcp/src/whisper_mcp/__init__.py`

## OpenRouter

Generated-media tools can submit video generation prompts and poll job outputs
through OpenRouter when `OPENROUTER_API_KEY` is configured.

- Provider: `https://openrouter.ai/`
- References: `crates/core/src/generated_media/openrouter.rs`,
  `crates/core/src/tools/start_generated_media_job.rs`

## apps/desktop/src/shell/assets

Podcast demo JPEGs under `apps/desktop/src/shell/assets/` are bundled as shell
UI imagery. Their provenance is not yet documented well enough for consumer
release marketing or tutorial redistribution.

- Asset list and current status: `apps/desktop/src/shell/assets/README.md`

## assets/audio

Synthetic sound effects under `assets/audio/sfx/` are generated from
mathematical oscillators and noise and are dedicated to the public domain under
CC0 1.0 by the Montage project authors.

- License summary: `assets/audio/LICENSE`
- CC0 legal text: `https://creativecommons.org/publicdomain/zero/1.0/legalcode`
