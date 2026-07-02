"""Editorial-moments indexer for Montage.

Reads sibling sidecars (whisper transcript + diarization, topic-mcp
segments, audio-energy curves), runs Claude Haiku per topic-segment
with a structured-output schema, and emits typed editorial beats —
hooks, punchlines, CTAs, dead-air, etc.

This is the "brain" indexer per the user's design note. Other
indexers extract *raw signals*; this one converts those signals into
*editorial decisions* the agent can navigate. The dependencies graph
between moments is what makes the index editorial rather than just
metadata.

Cross-indexer dependency: needs whisper + topic sidecars present. If
either is missing we raise (per the topic-mcp pattern landed earlier
this week — don't write an empty success that the SHA-cache then
locks in).

Schema version: "1".
"""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path
from typing import Any

from montage_mcp import IndexAssetRequest, IndexerServer

from ._schema import (
    BRollNeed,
    EditorialMoment,
    EditorialMomentsBody,
    EnergyLevel,
    MomentKind,
)

INDEXER_NAME = "editorial-moments"
INDEXER_VERSION = "0.1.0"
SCHEMA_VERSION = "1"

DEFAULT_MODEL = os.environ.get("EDITORIAL_MOMENTS_MODEL", "claude-haiku-4-5-20251001")
FALLBACK_MODEL = "deterministic-topic-fallback-v1"

server = IndexerServer(
    name=INDEXER_NAME,
    indexer_version=INDEXER_VERSION,
    schema_version=SCHEMA_VERSION,
)


def _find_sibling_sidecar(
    asset_path: str,
    asset_id: str,
    indexer: str,
    project_root: str | None = None,
    index_root: str | None = None,
) -> Path | None:
    """Walk up from `asset_path` to find `<project>/index/<indexer>/<asset_id>.json`.

    Mirrors topic-mcp._find_transcript_path. Stops at the project
    root marker (.montage/ directory).
    """
    if index_root:
        candidate = Path(index_root).absolute() / indexer / f"{asset_id}.json"
        if candidate.exists():
            return candidate
    if project_root:
        candidate = (
            Path(project_root).absolute() / "index" / indexer / f"{asset_id}.json"
        )
        if candidate.exists():
            return candidate
    asset = Path(asset_path).absolute()
    for ancestor in asset.parents:
        candidate = ancestor / "index" / indexer / f"{asset_id}.json"
        if candidate.exists():
            return candidate
        if (ancestor / ".montage").exists():
            return None
    return None


def _energy_for_window(
    audio_energy_data: dict[str, Any] | None, start_s: float, end_s: float
) -> EnergyLevel:
    """Coarse high|medium|low classification of a time window.

    Compares mean RMS across the window against the asset's overall
    median. Anything ≥ +3 dB above median counts as high; anything
    ≤ -6 dB below counts as low; the rest is medium. No ML; cheap.
    """
    if audio_energy_data is None:
        return EnergyLevel.MEDIUM
    windows = audio_energy_data.get("windows", [])
    if not windows:
        return EnergyLevel.MEDIUM
    win_in_range = [
        w["rms_db"] for w in windows if start_s <= w["start_s"] < end_s
    ]
    if not win_in_range:
        return EnergyLevel.MEDIUM
    all_rms = sorted(w["rms_db"] for w in windows)
    median = all_rms[len(all_rms) // 2]
    mean_in_range = sum(win_in_range) / len(win_in_range)
    delta = mean_in_range - median
    if delta >= 3.0:
        return EnergyLevel.HIGH
    if delta <= -6.0:
        return EnergyLevel.LOW
    return EnergyLevel.MEDIUM


def _build_segment_context(
    transcript: dict[str, Any],
    audio_energy: dict[str, Any] | None,
    topic: dict[str, Any],
) -> str:
    """Render one topic-segment as a compact text block for the LLM.

    Includes the transcript text, speaker tags when available, and the
    coarse energy classification. Capped at ~3000 chars so the LLM
    pass stays cheap.
    """
    body = transcript.get("data", transcript)
    segments = body.get("segments", [])
    start_s = topic["start_s"]
    end_s = topic["end_s"]
    label = topic.get("label", "untitled")

    in_range = [
        s for s in segments if start_s <= float(s.get("start_s", 0)) < end_s
    ]

    energy = _energy_for_window(
        audio_energy.get("data") if audio_energy else None, start_s, end_s
    )

    lines = [
        f"Topic: {label}",
        f"Time range: {start_s:.2f}s..{end_s:.2f}s",
        f"Energy: {energy.value}",
        f"Speakers: {sorted({s.get('speaker_id') for s in in_range if s.get('speaker_id')})}",
        "",
        "Transcript:",
    ]
    chars_left = 2400
    for s in in_range:
        speaker = s.get("speaker_id") or "—"
        text = s.get("text", "").strip()
        line = f"  [{s.get('start_s', 0):.1f}s] {speaker}: {text}"
        if chars_left - len(line) < 0:
            lines.append("  …(truncated)")
            break
        lines.append(line)
        chars_left -= len(line)
    return "\n".join(lines)


def _segments_for_topic(
    transcript: dict[str, Any],
    start_s: float,
    end_s: float,
) -> list[dict[str, Any]]:
    body = transcript.get("data", transcript)
    segments = body.get("segments", [])
    if not isinstance(segments, list):
        return []
    return [
        s
        for s in segments
        if isinstance(s, dict) and start_s <= float(s.get("start_s", 0)) < end_s
    ]


def _topic_text(topic: dict[str, Any], segments: list[dict[str, Any]]) -> str:
    parts = [str(topic.get("label", ""))]
    parts.extend(str(segment.get("text", "")) for segment in segments)
    return " ".join(part.strip() for part in parts if part.strip()).lower()


def _fallback_kind(topic_seq: int, text: str, energy: EnergyLevel) -> MomentKind:
    if topic_seq == 0:
        return MomentKind.HOOK
    if "?" in text or text.startswith(("why ", "how ", "what ")):
        return MomentKind.QUESTION
    if any(token in text for token in ("story", "remember", "when we", "i was", "we were")):
        return MomentKind.STORY
    if energy == EnergyLevel.HIGH:
        return MomentKind.EMOTIONAL_PEAK
    return MomentKind.EXPLANATION


def _fallback_broll_need(kind: MomentKind, text: str) -> BRollNeed:
    visual_terms = (
        "app",
        "dashboard",
        "data",
        "chart",
        "screen",
        "website",
        "workflow",
        "product",
        "company",
        "market",
        "ai",
        "model",
        "phone",
        "camera",
    )
    if any(term in text for term in visual_terms):
        return BRollNeed.HIGH
    if kind in (MomentKind.EXPLANATION, MomentKind.STORY, MomentKind.HOOK):
        return BRollNeed.MEDIUM
    return BRollNeed.NONE


def _fallback_score(kind: MomentKind, energy: EnergyLevel) -> float:
    base = {
        MomentKind.HOOK: 0.78,
        MomentKind.EMOTIONAL_PEAK: 0.72,
        MomentKind.STORY: 0.68,
        MomentKind.EXPLANATION: 0.62,
        MomentKind.QUESTION: 0.58,
    }.get(kind, 0.52)
    if energy == EnergyLevel.HIGH:
        return min(base + 0.08, 0.9)
    if energy == EnergyLevel.LOW:
        return max(base - 0.12, 0.35)
    return base


def _fallback_moments(
    transcript: dict[str, Any],
    audio_energy: dict[str, Any] | None,
    topics: list[dict[str, Any]],
    topic_start_seq: int = 0,
) -> list[EditorialMoment]:
    moments: list[EditorialMoment] = []
    for offset, topic in enumerate(topics):
        topic_seq = topic_start_seq + offset
        start_s = float(topic["start_s"])
        end_s = float(topic["end_s"])
        segments = _segments_for_topic(transcript, start_s, end_s)
        if segments:
            moment_start = float(segments[0].get("start_s", start_s))
            moment_end = float(segments[-1].get("end_s", end_s))
            speaker = segments[0].get("speaker_id")
        else:
            moment_start = start_s
            moment_end = end_s
            speaker = None
        if moment_end <= moment_start:
            moment_end = min(end_s, moment_start + 1.0)
        text = _topic_text(topic, segments)
        energy = _energy_for_window(
            audio_energy.get("data") if audio_energy else None,
            start_s,
            end_s,
        )
        kind = _fallback_kind(topic_seq, text, energy)
        broll_need = _fallback_broll_need(kind, text)
        moments.append(
            EditorialMoment(
                moment_id=f"m_{topic_seq:03d}_00",
                kind=kind,
                start_s=moment_start,
                end_s=moment_end,
                speaker=str(speaker) if speaker else None,
                energy=energy,
                score=_fallback_score(kind, energy),
                broll_need=broll_need,
                cut_in_suggestion="Use the first clean phrase in this topic.",
                cut_out_suggestion="Cut after the last complete thought in this topic.",
                dependencies=[],
                note=(
                    "Deterministic fallback from topic and transcript evidence; "
                    "rerun with a valid Anthropic key for richer editorial labeling."
                ),
            )
        )
    return moments


_SYSTEM_PROMPT = """\
You are an editorial-moments tagger for montage, a video editing
agent.  Given a topic-segment of a recorded conversation, identify
2-6 editorial moments that would be useful for an editor: hooks,
stories, punchlines, setups, questions/answers, CTAs, emotional
peaks, dead-air sections, tangents, explanations.

Return ONLY a JSON object with the exact shape:

{
  "moments": [
    {
      "moment_id": "m_<topic_seq>_<beat_seq>",
      "kind": "hook" | "story" | "punchline" | "setup" | "question" |
              "answer" | "cta" | "emotional_peak" | "dead_air" |
              "tangent" | "explanation",
      "start_s": <number, seconds into source media>,
      "end_s": <number, seconds into source media>,
      "speaker": <whisper speaker_id or null>,
      "energy": "high" | "medium" | "low",
      "score": <number 0..1, editorial importance>,
      "broll_need": "high" | "medium" | "none",
      "cut_in_suggestion": <one short phrase or null>,
      "cut_out_suggestion": <one short phrase or null>,
      "dependencies": [<moment_id strings, possibly empty>],
      "note": <one short sentence of editorial reasoning>
    }
  ]
}

Important:
- start_s/end_s are bound by the topic's time range.
- Hooks score high (>0.8); tangents and dead_air score low (<0.4).
- Mark dependencies when a moment requires earlier setup to land
  standalone — leave empty otherwise.
- Be precise: trim to phrase boundaries, not mid-word.
- Don't invent content; ground every moment in the transcript provided.
"""


def _llm_pass(client: Any, segment_text: str, topic_seq: int) -> list[dict]:
    """Call Claude Haiku with the structured-output prompt; parse JSON.

    Returns a list of moment dicts. Raises on parse failure or empty
    response — the engine treats this as a non-cacheable failure so
    a future re-run will retry.
    """
    user_prompt = (
        f"Topic-segment #{topic_seq}:\n\n{segment_text}\n\n"
        f"Return the JSON object."
    )
    resp = client.messages.create(
        model=DEFAULT_MODEL,
        max_tokens=2048,
        system=_SYSTEM_PROMPT,
        messages=[{"role": "user", "content": user_prompt}],
    )
    raw = resp.content[0].text.strip()
    # Trim ```json fences if the model added them.
    if raw.startswith("```"):
        raw = raw.strip("`")
        if raw.startswith("json\n"):
            raw = raw[len("json\n") :]
        raw = raw.strip()
    parsed = json.loads(raw)
    moments = parsed.get("moments", [])
    if not isinstance(moments, list):
        raise ValueError(f"expected 'moments' to be a list, got {type(moments)}")
    return moments


def _is_auth_failure(error: Exception) -> bool:
    message = str(error).lower()
    return (
        "authentication_error" in message
        or "invalid x-api-key" in message
        or "401 unauthorized" in message
    )


def _resolve_dependencies(moments: list[EditorialMoment]) -> None:
    """Cheap heuristic cross-segment dependency resolution.

    When a punchline/answer follows a setup/question on the same
    speaker within ~120s and the punchline's note references content
    in the setup's note, link them. Mutates `moments` in place.
    """
    setups = [
        m
        for m in moments
        if m.kind in (MomentKind.SETUP, MomentKind.QUESTION, MomentKind.STORY)
    ]
    payoffs = [
        m
        for m in moments
        if m.kind in (MomentKind.PUNCHLINE, MomentKind.ANSWER, MomentKind.EMOTIONAL_PEAK)
    ]
    for p in payoffs:
        # Find the most recent setup before this payoff, same speaker,
        # within 2 minutes. The model already sets explicit deps in
        # its JSON; this just adds the implicit "the setup right
        # before me" link when missing.
        if p.dependencies:
            continue
        candidates = [
            s
            for s in setups
            if s.end_s <= p.start_s
            and (p.start_s - s.end_s) < 120.0
            and (s.speaker is None or p.speaker is None or s.speaker == p.speaker)
        ]
        if candidates:
            best = max(candidates, key=lambda s: s.end_s)
            p.dependencies.append(best.moment_id)


def _normalize_moment_dict(moment: dict[str, Any]) -> dict[str, Any]:
    normalized = dict(moment)
    kind = str(normalized.get("kind", "")).strip().lower().replace("-", "_")
    kind_aliases = {
        "inspirational_cta": MomentKind.CTA.value,
        "call_to_action": MomentKind.CTA.value,
        "call_to_action_cta": MomentKind.CTA.value,
        "emotional": MomentKind.EMOTIONAL_PEAK.value,
        "emotional_moment": MomentKind.EMOTIONAL_PEAK.value,
        "peak": MomentKind.EMOTIONAL_PEAK.value,
        "joke": MomentKind.PUNCHLINE.value,
        "payoff": MomentKind.PUNCHLINE.value,
        "storytelling": MomentKind.STORY.value,
        "background": MomentKind.EXPLANATION.value,
    }
    if kind in kind_aliases:
        normalized["kind"] = kind_aliases[kind]
    elif kind:
        normalized["kind"] = kind

    broll_need = str(normalized.get("broll_need", "")).strip().lower().replace("-", "_")
    broll_aliases = {
        "low": BRollNeed.NONE.value,
        "no": BRollNeed.NONE.value,
        "false": BRollNeed.NONE.value,
        "minimal": BRollNeed.NONE.value,
        "moderate": BRollNeed.MEDIUM.value,
        "maybe": BRollNeed.MEDIUM.value,
        "yes": BRollNeed.MEDIUM.value,
        "strong": BRollNeed.HIGH.value,
        "very_high": BRollNeed.HIGH.value,
    }
    if broll_need in broll_aliases:
        normalized["broll_need"] = broll_aliases[broll_need]
    elif broll_need:
        normalized["broll_need"] = broll_need

    energy = str(normalized.get("energy", "")).strip().lower().replace("-", "_")
    energy_aliases = {
        "quiet": EnergyLevel.LOW.value,
        "calm": EnergyLevel.LOW.value,
        "normal": EnergyLevel.MEDIUM.value,
        "moderate": EnergyLevel.MEDIUM.value,
        "excited": EnergyLevel.HIGH.value,
        "intense": EnergyLevel.HIGH.value,
    }
    if energy in energy_aliases:
        normalized["energy"] = energy_aliases[energy]
    elif energy:
        normalized["energy"] = energy

    return normalized


@server.index_asset
def handle(req: IndexAssetRequest) -> dict[str, Any]:
    transcript_path = _find_sibling_sidecar(
        req.asset_path, req.asset_id, "whisper", req.project_root, req.index_root
    )
    if transcript_path is None:
        raise RuntimeError(
            f"editorial-moments needs the whisper sidecar at "
            f"index/whisper/{req.asset_id}.json. Run `montage index --indexer whisper` "
            f"first, then re-run."
        )
    topic_path = _find_sibling_sidecar(
        req.asset_path, req.asset_id, "topic", req.project_root, req.index_root
    )
    if topic_path is None:
        raise RuntimeError(
            f"editorial-moments needs the topic sidecar at "
            f"index/topic/{req.asset_id}.json. Run `montage index --indexer topic` "
            f"first, then re-run."
        )

    transcript = json.loads(transcript_path.read_text())
    topics_doc = json.loads(topic_path.read_text())
    topics = topics_doc.get("data", {}).get("topics", [])
    if not topics:
        # Same reasoning as topic-mcp: don't cache an empty success.
        raise RuntimeError(
            f"topic sidecar at index/topic/{req.asset_id}.json has no topics; "
            f"editorial-moments has nothing to tag"
        )

    # Audio-energy is optional; falling back to MEDIUM is fine.
    audio_path = _find_sibling_sidecar(
        req.asset_path, req.asset_id, "audio-energy", req.project_root, req.index_root
    )
    audio_energy = (
        json.loads(audio_path.read_text()) if audio_path is not None else None
    )

    client = None
    if os.environ.get("ANTHROPIC_API_KEY"):
        # Lazy import — anthropic SDK isn't tiny.
        import anthropic

        client = anthropic.Anthropic()
    else:
        print(
            "editorial-moments: ANTHROPIC_API_KEY is unset; using deterministic "
            "topic fallback.",
            file=sys.stderr,
        )

    all_moments: list[EditorialMoment] = []
    fallback_topics = 0
    for i, topic in enumerate(topics):
        topic_moments: list[EditorialMoment] = []
        if client is not None:
            ctx = _build_segment_context(transcript, audio_energy, topic)
            try:
                raw_moments = _llm_pass(client, ctx, i)
            except Exception as e:  # noqa: BLE001
                print(
                    f"editorial-moments: topic #{i} failed ({e}); using fallback",
                    file=sys.stderr,
                )
                if _is_auth_failure(e):
                    print(
                        "editorial-moments: Anthropic authentication failed; "
                        "using deterministic fallback for remaining topics.",
                        file=sys.stderr,
                    )
                    client = None
            else:
                # Stamp moment_ids consistently so dependencies are resolvable.
                for j, m in enumerate(raw_moments):
                    m.setdefault("moment_id", f"m_{i:03d}_{j:02d}")
                    try:
                        topic_moments.append(
                            EditorialMoment.model_validate(
                                _normalize_moment_dict(m)
                            )
                        )
                    except Exception as e:  # noqa: BLE001
                        print(
                            f"editorial-moments: schema-invalid moment in topic #{i}: {e}",
                            file=sys.stderr,
                        )
        if not topic_moments:
            topic_moments = _fallback_moments(
                transcript,
                audio_energy,
                [topic],
                topic_start_seq=i,
            )
            fallback_topics += 1
        all_moments.extend(topic_moments)

    if not all_moments:
        raise RuntimeError(
            "editorial-moments produced zero valid moments from both LLM and "
            "deterministic fallback passes. Check transcript/topic sidecars."
        )

    _resolve_dependencies(all_moments)
    labeler_model = DEFAULT_MODEL
    if fallback_topics == len(topics):
        labeler_model = FALLBACK_MODEL
    elif fallback_topics:
        labeler_model = f"{DEFAULT_MODEL}+{FALLBACK_MODEL}"

    body = EditorialMomentsBody(
        moments=all_moments,
        labeler_model=labeler_model,
        topic_segments_processed=len(topics),
    )
    return body.model_dump(mode="json")


def main() -> None:
    server.run()
