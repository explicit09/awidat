"""Typed schema for editorial moments.

The headline data structure of the awidat indexer corpus. Every other
indexer emits *raw signals* (transcript, energy, scenes, topics);
this one emits *editorial decisions* — the candidate moves the agent
should consider when editing.

The shape mirrors the user's design note:

    {
      "moment_id": "m_0412",
      "kind": "story",
      "start_s": 252.4,
      "end_s": 278.9,
      "speaker": "host",
      "energy": "high",
      "score": 0.84,
      "broll_need": "medium",
      "cut_in_suggestion": "after the filler at 252.4s",
      "cut_out_suggestion": "after the laugh at 278.9s",
      "dependencies": ["m_0411"],
      "note": "first explicit pitch line; works as a 30s standalone"
    }

The `dependencies` field is what makes this an *editorial* index, not
just a metadata one — it tells the agent "this punchline can't be cut
standalone; it depends on the m_0411 setup." That graph is exactly
the editorial structure video doesn't get for free.

Schema version: "1".
"""

from __future__ import annotations

from enum import Enum
from typing import Optional

from pydantic import BaseModel, Field


class MomentKind(str, Enum):
    """Categories of editorial beat the agent might want to cut on.

    Tuned for podcast / interview / pitch footage. v1.5 may add
    visual-shaped kinds (cutaway, b-roll-cue, etc.) once we have a
    vision indexer to inform them.
    """

    HOOK = "hook"
    STORY = "story"
    PUNCHLINE = "punchline"
    SETUP = "setup"
    QUESTION = "question"
    ANSWER = "answer"
    CTA = "cta"
    EMOTIONAL_PEAK = "emotional_peak"
    DEAD_AIR = "dead_air"
    TANGENT = "tangent"
    EXPLANATION = "explanation"


class EnergyLevel(str, Enum):
    HIGH = "high"
    MEDIUM = "medium"
    LOW = "low"


class BRollNeed(str, Enum):
    HIGH = "high"
    MEDIUM = "medium"
    NONE = "none"


class EditorialMoment(BaseModel):
    """One typed editorial beat in the asset's source-media timeline.

    All time fields are seconds *into the source media*, not into the
    timeline. Same convention as the EDL ops — anchors survive edits
    by being content-relative, not timeline-relative.
    """

    moment_id: str = Field(
        ..., description="Stable identifier for this moment (e.g. 'm_0412')."
    )
    kind: MomentKind = Field(..., description="Editorial category.")
    start_s: float = Field(..., ge=0.0, description="Source-media start (seconds).")
    end_s: float = Field(..., gt=0.0, description="Source-media end (seconds).")
    speaker: Optional[str] = Field(
        None, description="Whisper speaker id, when diarization ran."
    )
    energy: EnergyLevel = Field(
        EnergyLevel.MEDIUM,
        description="Coarse arousal classification of the segment.",
    )
    score: float = Field(
        ...,
        ge=0.0,
        le=1.0,
        description=(
            "Editorial importance, 0..1. Hooks score high; tangents and "
            "dead-air score low. Used by the agent to rank candidates "
            "when picking what to cut to."
        ),
    )
    broll_need: BRollNeed = Field(
        BRollNeed.NONE,
        description=(
            "Whether this moment would benefit from a b-roll insert. "
            "high=visually dull; medium=could use it; none=carry the "
            "talking head."
        ),
    )
    cut_in_suggestion: Optional[str] = Field(
        None,
        description=(
            "Free-form note on where to cut INTO this moment cleanly "
            "(e.g. 'after the filler at 252.4s', 'on the laugh')."
        ),
    )
    cut_out_suggestion: Optional[str] = Field(
        None,
        description=(
            "Free-form note on where to cut OUT of this moment cleanly."
        ),
    )
    dependencies: list[str] = Field(
        default_factory=list,
        description=(
            "moment_ids this beat needs as setup. The agent must keep "
            "or restore them when cutting this moment standalone."
        ),
    )
    note: Optional[str] = Field(
        None,
        description=(
            "Model's own one-sentence reasoning for why this moment is "
            "editorially interesting. Free-form prose."
        ),
    )


class EditorialMomentsBody(BaseModel):
    """Sidecar `data:` payload."""

    moments: list[EditorialMoment]
    labeler_model: str = Field(
        ..., description="Model id used for the LLM-pass (e.g. 'claude-haiku-4-5')."
    )
    topic_segments_processed: int = Field(
        ..., ge=0, description="How many topic-segments were fed to the LLM."
    )
