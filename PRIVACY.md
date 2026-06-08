# Privacy and Data Egress

Montage is a developer-preview, local-first video editing project. Project files,
media paths, transcripts, indexes, generated plans, and render artifacts are
stored locally unless you configure an external service or publishing workflow.

This document describes the main places data can leave your machine. Review the
code and provider terms before using Montage with confidential media.

## Local Project Data

Montage projects live on your machine. Imported media, transcript outputs,
indexes, timeline state, agent notes, and render artifacts remain local by
default. The desktop app and CLI read and write those files directly.

## OpenAI and ChatGPT

Agent sessions can send prompts, project context, transcript excerpts, tool
results, and other media-derived metadata to OpenAI when you use OpenAI API-key
auth or a configured ChatGPT OAuth flow. API-key auth is the supported public
default for third-party use. ChatGPT OAuth requires an explicitly configured,
sanctioned OAuth client id.

## Anthropic

Some agent-backed commands and Python indexers use Anthropic models when
`ANTHROPIC_API_KEY` is configured. Those workflows can send prompts, transcript
text, edit briefs, and media-derived metadata to Anthropic.

## OpenRouter

Generated-media workflows can use OpenRouter when `OPENROUTER_API_KEY` is
configured. Those requests can send text prompts, visual descriptions, and job
metadata to OpenRouter and the selected model provider.

## Deepgram

Whisper-related transcription workflows can use Deepgram when
`DEEPGRAM_API_KEY` is configured. In that mode, raw audio or audio-derived data
may be sent to Deepgram for transcription or diarization.

## Hugging Face and pyannote

Speaker diarization can use gated Hugging Face models such as pyannote when
`HF_TOKEN` is configured and the model terms have been accepted. Model downloads,
authentication, and related requests may contact Hugging Face infrastructure.

## YouTube and Social Providers

The social publishing server can send rendered media, titles, descriptions,
tags, thumbnails, schedules, account identifiers, and publishing metadata to
YouTube or other configured social providers. OAuth tokens for those providers
are encrypted at rest when the social server is configured correctly.

## Secrets

Local secrets should be stored in `.env.local` files that are ignored by git, OS
keychain-backed stores, GitHub Actions secrets, or deployment secret managers.
Do not commit API keys, OAuth secrets, database URLs, bearer tokens, or token
encryption keys.

## Contact

Until Montage has a dedicated public support channel, report security and
privacy issues through the process in `SECURITY.md`.
