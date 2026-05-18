# Caption Transcript Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the first Awidat-native foundation for smarter short-form captions and scan-friendly transcript phrase views.

**Architecture:** Add a small reusable Python helper under the bundled short-form skill. The existing caption script consumes the helper, and a new transcript packing script emits markdown from the same phrase grouping logic.

**Tech Stack:** Python 3 standard library, bundled Awidat skills, stdlib `unittest`.

---

### Task 1: Phrase Grouping Core

**Files:**
- Create: `skills/short-form/scripts/transcript_phrases.py`
- Test: `skills/short-form/scripts/caption_plan_test.py`

- [x] **Step 1: Write failing tests**

Add tests that call `transcript_phrases.group_words_into_phrases` and verify words are split on speaker changes, punctuation, silence gaps, and max word count.

- [x] **Step 2: Run failing tests**

Run: `python3 skills/short-form/scripts/caption_plan_test.py`

Expected: FAIL because `transcript_phrases.py` does not exist yet.

- [x] **Step 3: Implement phrase grouping**

Create `transcript_phrases.py` with `normalize_words`, `group_words_into_phrases`, and `render_packed_markdown`.

- [x] **Step 4: Run tests**

Run: `python3 skills/short-form/scripts/caption_plan_test.py`

Expected: PASS for the phrase grouping tests.

### Task 2: Caption Script Integration

**Files:**
- Modify: `skills/short-form/scripts/caption_plan.py`
- Test: `skills/short-form/scripts/caption_plan_test.py`

- [x] **Step 1: Write failing tests**

Add tests that call `caption_plan.build_caption_phrases` and verify output uses grouped transcript phrases, applies hot styling to overlapping phrases, and keeps mobile caption metadata.

- [x] **Step 2: Run failing tests**

Run: `python3 skills/short-form/scripts/caption_plan_test.py`

Expected: FAIL because `build_caption_phrases` is not implemented.

- [x] **Step 3: Update caption script**

Make `caption_plan.py` use the shared helper instead of fixed-size chunking in `main`.

- [x] **Step 4: Run tests**

Run: `python3 skills/short-form/scripts/caption_plan_test.py`

Expected: PASS.

### Task 3: Packed Transcript CLI

**Files:**
- Create: `skills/short-form/scripts/pack_transcript.py`
- Test: `skills/short-form/scripts/caption_plan_test.py`

- [x] **Step 1: Write failing tests**

Add tests that call `pack_transcript.build_packed_transcript` and verify the markdown includes source headings and `[start-end] speaker text` rows.

- [x] **Step 2: Run failing tests**

Run: `python3 skills/short-form/scripts/caption_plan_test.py`

Expected: FAIL because `pack_transcript.py` does not exist yet.

- [x] **Step 3: Implement packed transcript CLI**

Create `pack_transcript.py` with `--transcript`, repeated `--transcript`, and optional `--source-label` support.

- [x] **Step 4: Run tests**

Run: `python3 skills/short-form/scripts/caption_plan_test.py`

Expected: PASS.

### Task 4: Skill Documentation

**Files:**
- Modify: `skills/short-form/SKILL.md`

- [x] **Step 1: Document usage**

Update the caption pass to describe phrase grouping and the new packed transcript helper.

- [x] **Step 2: Verify**

Run: `python3 skills/short-form/scripts/caption_plan_test.py`

Expected: PASS.
