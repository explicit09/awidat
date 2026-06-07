#!/usr/bin/env python3
"""Plan graph-native look regions and generated LUTs from color indexes.

This is the bridge between color analysis and renderable Montage EDL:

1. Read the project OTIO timeline.
2. Read one or more color-analysis sidecars.
3. Intersect color scenes with timeline clips.
4. Choose objective color correction and a generated LUT candidate.
5. Write project-relative .cube LUTs under luts/generated/.
6. Emit EDL that uses Set Color Correction, Apply LUT, and optional splits.

The script is deterministic and intentionally conservative. It does not
replace frame/creative review; it creates a reviewable plan the agent can
apply, render, and verify.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable

# Python 3.11+ ships tomllib; on older interpreters fall back to the
# `tomli` shim if available. We need the catalog at every run so
# import-time failure is a clear stop signal rather than a silent
# fallback to undocumented hardcoded defaults.
try:
    import tomllib  # type: ignore[unused-ignore]
except ModuleNotFoundError:  # pragma: no cover
    import tomli as tomllib  # type: ignore[no-redef]


FIELDS = [
    "exposure_ev",
    "contrast",
    "saturation",
    "temperature",
    "tint",
    "shadows",
    "highlights",
]
SUPPORTED_INTERPOLATION = "tetrahedral"

# Looks catalog lives next to this script as `../looks.toml` so the
# skill is self-contained. Layout matches what the Rust agent layer
# reads at runtime (no separate copy).
CATALOG_PATH = Path(__file__).resolve().parent.parent / "looks.toml"


@dataclass(frozen=True)
class CatalogLook:
    id: str
    display_name: str
    description: str
    default_input_space: str
    default_output_space: str
    default_size: int
    recommended_strength_min: float
    recommended_strength_max: float
    tags: tuple[str, ...]


def load_catalog(path: Path = CATALOG_PATH) -> dict[str, CatalogLook]:
    """Read looks.toml from disk and index it by `id`.

    Raises FileNotFoundError or KeyError if the catalog is missing
    required fields — that's intentional: the planner should fail
    fast when its agent-facing manifest is out of sync rather than
    silently writing LUTs for looks the agent thinks don't exist.
    """
    with path.open("rb") as handle:
        data = tomllib.load(handle)
    looks: dict[str, CatalogLook] = {}
    for entry in data.get("look", []):
        look = CatalogLook(
            id=entry["id"],
            display_name=entry["display_name"],
            description=entry["description"],
            default_input_space=entry["default_input_space"],
            default_output_space=entry["default_output_space"],
            default_size=int(entry["default_size"]),
            recommended_strength_min=float(entry["recommended_strength_min"]),
            recommended_strength_max=float(entry["recommended_strength_max"]),
            tags=tuple(entry.get("tags", [])),
        )
        looks[look.id] = look
    return looks


CATALOG = load_catalog()


def catalog_default_size() -> int:
    """Fall-back cube size when the agent omits ``--lut-size``.

    Uses the catalog's natural_balance default (the safest neutral
    look) so re-runs are reproducible without consulting CLI args.
    """
    natural = CATALOG.get("natural_balance")
    return natural.default_size if natural else 17


DEFAULT_LUT_SIZE = catalog_default_size()


@dataclass(frozen=True)
class ClipRef:
    name: str
    anchor: str
    asset_id: str
    source_start_s: float
    source_end_s: float
    track_start_s: float
    track_end_s: float
    # Stable color-space id from montage-effects::KNOWN_COLOR_SPACES.
    # Defaults to `rec709_g24` when the clip has no explicit
    # `montage.color_pipeline.clip_input_space` — that's the implicit
    # contract for Rec.709-delivered footage that was correct
    # without color management.
    clip_input_space: str = "rec709_g24"


@dataclass
class RegionPlan:
    """Editable region row. `lut_path` is filled in by
    [`resolve_cached_lut_paths`] after content-hashing — it points
    at `luts/cache/<sha256>.cube` so identical looks (same id, same
    size, same transform) share a single file across runs.

    `look_strength` (Stage 7) is the midpoint of the catalog's
    recommended range for that look; the planner emits it on the
    EDL `Apply LUT` op so render mixes the LUT with the source
    instead of slamming on at 100%. `None` means "let
    montage.lut default to full strength" (legacy behavior).
    """

    clip: ClipRef
    region_index: int
    anchor: str
    source_start_s: float
    source_end_s: float
    timeline_start_s: float
    timeline_end_s: float
    scene_id: str
    issue_tags: list[str]
    policy: dict[str, Any]
    correction: dict[str, Any]
    consistency_group: str
    look_id: str
    lut_path: str | None
    score: float
    rationale: str
    sample_times_s: list[float]
    source_sample_times_s: list[float]
    look_strength: float | None = None


def load_json(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def load_body(path: Path) -> dict[str, Any]:
    raw = load_json(path)
    return raw.get("data", raw)


def safe_slug(value: str) -> str:
    slug = re.sub(r"[^A-Za-z0-9._-]+", "-", value.strip()).strip("-").lower()
    return slug or "look"


def fmt_num(value: float) -> str:
    text = f"{value:.6f}".rstrip("0").rstrip(".")
    return "0" if text == "-0" else text


def clip_uuid(clip: dict[str, Any]) -> str | None:
    montage = clip.get("metadata", {}).get("montage", {})
    value = montage.get("clip_uuid")
    if isinstance(value, str) and value:
        return value
    extra = montage.get("extra", {})
    value = extra.get("clip_uuid")
    return value if isinstance(value, str) and value else None


def external_asset(clip: dict[str, Any]) -> str | None:
    ref = clip.get("media_reference", {})
    if ref.get("OTIO_SCHEMA") != "ExternalReference.1":
        return None
    value = ref.get("target_url")
    return value if isinstance(value, str) and value else None


def read_clip_input_space(clip: dict[str, Any]) -> str:
    """Pull `montage.color_pipeline.clip_input_space` off `clip` so the
    planner knows what space the source pixels live in. Falls back
    to `rec709_g24` — the implicit assumption for footage the
    project never tagged.

    Only `montage.color_pipeline` is consulted, not the legacy
    `montage.lut` effect, because `clip_input_space` is a property
    of the source, not of any particular grade.
    """
    for effect in clip.get("effects", []) or []:
        if effect.get("effect_name") != "montage.color_pipeline":
            continue
        metadata = effect.get("metadata") or {}
        value = metadata.get("clip_input_space")
        if isinstance(value, str) and value:
            return value
    return "rec709_g24"


def range_seconds(clip: dict[str, Any]) -> tuple[float, float] | None:
    source_range = clip.get("source_range")
    if not isinstance(source_range, dict):
        return None
    start = source_range.get("start_time", {})
    duration = source_range.get("duration", {})
    start_value = float(start.get("value", 0.0))
    start_rate = float(start.get("rate", 1.0) or 1.0)
    duration_value = float(duration.get("value", 0.0))
    duration_rate = float(duration.get("rate", 1.0) or 1.0)
    start_s = start_value / start_rate
    duration_s = duration_value / duration_rate
    if duration_s <= 0.0:
        return None
    return start_s, start_s + duration_s


def timeline_clips(project: Path) -> list[ClipRef]:
    timeline = load_json(project / "project.otio.json")
    clips: list[ClipRef] = []
    for stack_child in timeline.get("tracks", {}).get("children", []):
        if stack_child.get("OTIO_SCHEMA") != "Track.1":
            continue
        if stack_child.get("kind") != "Video":
            continue
        if stack_child.get("name") == "Titles":
            continue
        cursor = 0.0
        for child in stack_child.get("children", []):
            schema = child.get("OTIO_SCHEMA")
            if schema == "Gap.1":
                rng = range_seconds(child)
                if rng:
                    cursor += rng[1] - rng[0]
                continue
            if schema != "Clip.2":
                continue
            asset_id = external_asset(child)
            rng = range_seconds(child)
            if not asset_id or not rng:
                continue
            duration = rng[1] - rng[0]
            name = child.get("name") or asset_id
            anchor = clip_uuid(child) or name
            clip_input_space = read_clip_input_space(child)
            clips.append(
                ClipRef(
                    name=name,
                    anchor=anchor,
                    asset_id=asset_id,
                    source_start_s=rng[0],
                    source_end_s=rng[1],
                    track_start_s=cursor,
                    track_end_s=cursor + duration,
                    clip_input_space=clip_input_space,
                )
            )
            cursor += duration
    return clips


def color_index_asset(path: Path, body: dict[str, Any]) -> str:
    raw = load_json(path)
    value = raw.get("asset_id") or body.get("asset_id")
    if isinstance(value, str) and value:
        return value
    # Fallback for sidecar paths like index/color-analysis/raw/foo.mp4.json.
    parts = path.parts
    if "color-analysis" in parts:
        idx = parts.index("color-analysis")
        return str(Path(*parts[idx + 1 :])).removesuffix(".json")
    return path.stem


def load_color_indexes(paths: list[Path]) -> dict[str, dict[str, Any]]:
    indexes: dict[str, dict[str, Any]] = {}
    for path in paths:
        body = load_body(path)
        indexes[color_index_asset(path, body)] = body
    return indexes


def scenes_for(body: dict[str, Any], clip: ClipRef) -> list[dict[str, Any]]:
    scenes = body.get("scenes") or []
    out = []
    for scene in scenes:
        start = float(scene.get("start_s", 0.0))
        end = float(scene.get("end_s", start))
        overlap_start = max(start, clip.source_start_s)
        overlap_end = min(end, clip.source_end_s)
        if overlap_end - overlap_start >= 0.25:
            s = dict(scene)
            s["_overlap_start_s"] = overlap_start
            s["_overlap_end_s"] = overlap_end
            out.append(s)
    if out:
        return out
    summary = dict(body.get("summary") or {})
    summary.setdefault("scene_id", "summary")
    summary["_overlap_start_s"] = clip.source_start_s
    summary["_overlap_end_s"] = clip.source_end_s
    return [summary]


def normalize_correction(correction: dict[str, Any]) -> dict[str, float]:
    out: dict[str, float] = {}
    for field in FIELDS:
        if field in correction and correction[field] is not None:
            try:
                out[field] = float(correction[field])
            except (TypeError, ValueError):
                pass
    return out


def candidate_scores(tags: list[str], policy: dict[str, Any], style: str) -> dict[str, float]:
    scores = {
        "none": 0.42,
        "natural_balance": 0.50,
        "contrast_pop": 0.48,
        "warm_gold": 0.44,
        "cool_blue": 0.44,
        "teal_orange": 0.38,
        "bleach_bypass": 0.32,
        "shadow_lift": 0.42,
        "highlight_soften": 0.42,
    }

    tag_set = set(tags)
    if "already_balanced" in tag_set:
        scores["none"] += 0.35
    if "low_contrast" in tag_set:
        scores["contrast_pop"] += 0.30
        scores["bleach_bypass"] -= 0.20
    if "underexposed" in tag_set or "crushed_shadows" in tag_set:
        scores["shadow_lift"] += 0.34
        scores["teal_orange"] -= 0.12
    if "overexposed" in tag_set or "clipped_highlights" in tag_set:
        scores["highlight_soften"] += 0.32
        scores["warm_gold"] -= 0.12
    if "warm_cast" in tag_set:
        scores["cool_blue"] += 0.24
        scores["warm_gold"] -= 0.18
    if "cool_cast" in tag_set:
        scores["warm_gold"] += 0.24
        scores["cool_blue"] -= 0.18
    if "green_cast" in tag_set:
        scores["natural_balance"] += 0.24
    if "magenta_cast" in tag_set:
        scores["natural_balance"] += 0.20

    if style == "cinematic":
        scores["teal_orange"] += 0.22
        scores["contrast_pop"] += 0.08
        scores["none"] -= 0.12
    elif style == "warm":
        scores["warm_gold"] += 0.22
    elif style == "cool":
        scores["cool_blue"] += 0.22
    elif style == "punchy":
        scores["contrast_pop"] += 0.22

    if policy.get("recommended_action") == "review_only":
        # Risky frames should not get an aggressive creative look by default.
        scores["natural_balance"] += 0.08
        scores["teal_orange"] -= 0.10
        scores["bleach_bypass"] -= 0.16

    return {key: max(0.0, min(1.0, value)) for key, value in scores.items()}


def choose_look(
    tags: list[str],
    policy: dict[str, Any],
    style: str,
    clip_input_space: str = "rec709_g24",
) -> tuple[str, float]:
    """Score every catalog look and pick the highest survivor.

    Stage 7: looks whose catalog `default_input_space` doesn't match
    `clip_input_space` are dropped from contention. The `"none"`
    fallback is always eligible — when no compatible look exists
    the planner emits no LUT op for that region, which is the
    correct behavior for log-encoded footage with no bundled
    shaper.
    """
    scores = candidate_scores(tags, policy, style)
    eligible: dict[str, float] = {"none": scores.get("none", 0.0)}
    for look_id, score in scores.items():
        if look_id == "none":
            continue
        entry = CATALOG.get(look_id)
        # Unknown ids slip through the drift check, so just keep
        # them eligible — better an unexpected look than silently
        # blackholed planning.
        if entry is None or entry.default_input_space == clip_input_space:
            eligible[look_id] = score
    look_id, score = max(eligible.items(), key=lambda item: item[1])
    return look_id, score


def strength_for(look_id: str) -> float | None:
    """Recommended `montage.lut.strength` for a look — midpoint of
    the catalog range, clamped to `[0.0, 1.0]`. Returns `None` when
    the look isn't in the catalog (caller skips emitting strength,
    so the legacy "full strength" default applies).
    """
    entry = CATALOG.get(look_id)
    if entry is None:
        return None
    mid = (entry.recommended_strength_min + entry.recommended_strength_max) / 2.0
    return max(0.0, min(1.0, mid))


def consistency_group(asset_id: str, tags: list[str], look_id: str) -> str:
    stable_tags = sorted(tag for tag in tags if tag != "unsafe_to_auto_correct")
    if not stable_tags:
        stable_tags = ["balanced"]
    return f"{safe_slug(asset_id)}:{look_id}:{'-'.join(stable_tags)}"


def sample_times(start_s: float, end_s: float) -> list[float]:
    if end_s <= start_s:
        return [start_s]
    mid = start_s + ((end_s - start_s) / 2.0)
    return sorted({round(start_s + 0.1, 3), round(mid, 3), round(max(start_s, end_s - 0.1), 3)})


def make_regions(clips: list[ClipRef], indexes: dict[str, dict[str, Any]], style: str) -> list[RegionPlan]:
    regions: list[RegionPlan] = []
    for clip in clips:
        body = indexes.get(clip.asset_id)
        if body is None:
            continue
        clip_scenes = scenes_for(body, clip)
        for idx, scene in enumerate(clip_scenes):
            issue_tags = [str(tag) for tag in scene.get("issue_tags", [])]
            policy = dict(scene.get("policy") or {})
            correction = normalize_correction(scene.get("recommended_correction") or {})
            look_id, score = choose_look(
                issue_tags,
                policy,
                style,
                clip_input_space=clip.clip_input_space,
            )
            if look_id == "none":
                lut_path = None
                look_strength = None
            else:
                lut_path = f"luts/generated/{style}-{look_id}.cube"
                look_strength = strength_for(look_id)
            group = consistency_group(clip.asset_id, issue_tags, look_id)
            source_start = float(scene["_overlap_start_s"])
            source_end = float(scene["_overlap_end_s"])
            offset = source_start - clip.source_start_s
            timeline_start = clip.track_start_s + offset
            timeline_end = timeline_start + (source_end - source_start)
            rationale = rationale_for(
                look_id, issue_tags, policy, style, clip.clip_input_space
            )
            regions.append(
                RegionPlan(
                    clip=clip,
                    region_index=idx,
                    anchor=clip.anchor,
                    source_start_s=source_start,
                    source_end_s=source_end,
                    timeline_start_s=timeline_start,
                    timeline_end_s=timeline_end,
                    scene_id=str(scene.get("scene_id", f"region_{idx + 1}")),
                    issue_tags=issue_tags,
                    policy=policy,
                    correction=correction,
                    consistency_group=group,
                    look_id=look_id,
                    lut_path=lut_path,
                    score=score,
                    rationale=rationale,
                    sample_times_s=sample_times(timeline_start, timeline_end),
                    source_sample_times_s=sample_times(source_start, source_end),
                    look_strength=look_strength,
                )
            )
    return regions


def rationale_for(
    look_id: str,
    tags: list[str],
    policy: dict[str, Any],
    style: str,
    clip_input_space: str = "rec709_g24",
) -> str:
    action = policy.get("recommended_action", "review_only")
    tag_text = ", ".join(tags) or "no issue tags"
    if look_id == "none":
        compatible = sum(
            1
            for look in CATALOG.values()
            if look.default_input_space == clip_input_space
        )
        if compatible == 0:
            return (
                f"no compatible look in catalog for clip_input_space={clip_input_space}; "
                f"policy={action}; tags={tag_text}"
            )
        return f"leave unchanged; policy={action}; tags={tag_text}"
    return (
        f"{look_id} selected for style={style}; "
        f"clip_input_space={clip_input_space}; policy={action}; tags={tag_text}"
    )


def piece_anchors(clip: ClipRef, count: int) -> list[str]:
    anchors = [clip.anchor]
    name = clip.name
    for _ in range(1, count):
        name = f"{name}-b"
        anchors.append(name)
    return anchors


def build_edl(regions: list[RegionPlan], emit_splits: bool) -> str:
    lines = ["*** Begin EDL"]
    by_clip: dict[str, list[RegionPlan]] = {}
    for region in regions:
        by_clip.setdefault(region.clip.anchor, []).append(region)

    for clip_regions in by_clip.values():
        clip_regions.sort(key=lambda r: (r.source_start_s, r.source_end_s))
        clip = clip_regions[0].clip
        anchors = piece_anchors(clip, len(clip_regions))
        if emit_splits and len(clip_regions) > 1:
            for idx, region in enumerate(clip_regions[1:], start=1):
                lines.extend(
                    [
                        "*** Split Clip",
                        f"@@ anchor: clip_uuid={anchors[idx - 1]}",
                        f"+ at_s: {fmt_num(region.source_start_s)}",
                    ]
                )
        for idx, region in enumerate(clip_regions):
            anchor = anchors[idx] if emit_splits and len(clip_regions) > 1 else region.anchor
            if region.correction and region.policy.get("recommended_action") == "auto_correct":
                lines.extend(["*** Set Color Correction", f"@@ anchor: clip_uuid={anchor}"])
                for field in FIELDS:
                    if field in region.correction:
                        lines.append(f"+ {field}: {fmt_num(region.correction[field])}")
            if region.lut_path:
                lines.extend(
                    [
                        "*** Apply LUT",
                        f"@@ anchor: clip_uuid={anchor}",
                        f"+ lut_path: {region.lut_path}",
                        f"+ interpolation: {SUPPORTED_INTERPOLATION}",
                    ]
                )
                # Emit the catalog-recommended strength so render
                # mixes the LUT with the source. Skipped when
                # `look_strength` is None (legacy "full strength").
                if region.look_strength is not None:
                    lines.append(f"+ strength: {fmt_num(region.look_strength)}")
    lines.append("*** End EDL")
    return "\n".join(lines) + "\n"


def transform_for(look_id: str) -> Callable[[float, float, float], tuple[float, float, float]]:
    def clamp(v: float) -> float:
        return max(0.0, min(1.0, v))

    def contrast(v: float, amount: float) -> float:
        return clamp((v - 0.5) * amount + 0.5)

    def natural(r: float, g: float, b: float) -> tuple[float, float, float]:
        return clamp(r * 1.01), clamp(g * 1.0), clamp(b * 0.99)

    def contrast_pop(r: float, g: float, b: float) -> tuple[float, float, float]:
        return contrast(r, 1.14), contrast(g, 1.12), contrast(b, 1.12)

    def warm_gold(r: float, g: float, b: float) -> tuple[float, float, float]:
        return clamp(r * 1.10 + 0.025), clamp(g * 1.03 + 0.010), clamp(b * 0.88)

    def cool_blue(r: float, g: float, b: float) -> tuple[float, float, float]:
        return clamp(r * 0.90), clamp(g * 1.01 + 0.008), clamp(b * 1.10 + 0.025)

    def teal_orange(r: float, g: float, b: float) -> tuple[float, float, float]:
        y = 0.2126 * r + 0.7152 * g + 0.0722 * b
        shadow = 1.0 - y
        return clamp(r * 1.02 + 0.10 * y), clamp(g * 0.98 + 0.07 * shadow), clamp(b * 0.90 + 0.13 * shadow)

    def bleach_bypass(r: float, g: float, b: float) -> tuple[float, float, float]:
        y = 0.2126 * r + 0.7152 * g + 0.0722 * b
        rr = y * 0.72 + r * 0.28
        gg = y * 0.72 + g * 0.28
        bb = y * 0.72 + b * 0.28
        return contrast(rr, 1.25), contrast(gg, 1.25), contrast(bb, 1.25)

    def shadow_lift(r: float, g: float, b: float) -> tuple[float, float, float]:
        return tuple(clamp(math.sqrt(max(0.0, v)) * 0.92 + 0.025) for v in (r, g, b))  # type: ignore[return-value]

    def highlight_soften(r: float, g: float, b: float) -> tuple[float, float, float]:
        return tuple(clamp(1.0 - ((1.0 - v) ** 1.18)) for v in (r, g, b))  # type: ignore[return-value]

    transforms = {
        "natural_balance": natural,
        "contrast_pop": contrast_pop,
        "warm_gold": warm_gold,
        "cool_blue": cool_blue,
        "teal_orange": teal_orange,
        "bleach_bypass": bleach_bypass,
        "shadow_lift": shadow_lift,
        "highlight_soften": highlight_soften,
    }
    # Every transform implemented here must also exist in the
    # catalog (and vice versa). Drift would mean the agent picks
    # looks the planner can't generate or vice versa, so fail at
    # planner startup rather than silently falling back to
    # `natural`.
    catalog_ids = set(CATALOG.keys())
    impl_ids = set(transforms.keys())
    missing_impl = catalog_ids - impl_ids
    missing_catalog = impl_ids - catalog_ids
    if missing_impl or missing_catalog:
        message = (
            "look catalog drift: "
            f"impls missing for catalog ids {sorted(missing_impl)}, "
            f"catalog missing for impl ids {sorted(missing_catalog)}"
        )
        raise RuntimeError(message)
    if look_id not in transforms:
        # Unknown id is a planner bug — log loudly so the agent
        # surfaces it instead of silently producing a natural LUT
        # under the wrong filename.
        print(
            f"warning: look_id {look_id!r} not in catalog; falling back to natural_balance",
            file=sys.stderr,
        )
    return transforms.get(look_id, natural)


def build_cube_text(look_id: str, size: int) -> str:
    """Render a `.cube` file as a string. Used for both content
    hashing and on-disk writing so the hash sees exactly what the
    write would have produced.
    """
    transform = transform_for(look_id)
    parts = [
        f'TITLE "{look_id}"',
        f"LUT_3D_SIZE {size}",
        "DOMAIN_MIN 0.0 0.0 0.0",
        "DOMAIN_MAX 1.0 1.0 1.0",
    ]
    for bi in range(size):
        b = bi / (size - 1)
        for gi in range(size):
            g = gi / (size - 1)
            for ri in range(size):
                r = ri / (size - 1)
                rr, gg, bb = transform(r, g, b)
                parts.append(f"{rr:.6f} {gg:.6f} {bb:.6f}")
    return "\n".join(parts) + "\n"


def lut_cube_size(look_id: str, override: int | None) -> int:
    """Final cube size for a look — CLI override wins, otherwise
    the catalog's per-look default, otherwise the module default.
    """
    if override is not None:
        return override
    if look_id in CATALOG:
        return CATALOG[look_id].default_size
    return DEFAULT_LUT_SIZE


def cache_path_for(content: str) -> str:
    """Project-relative cache path keyed by SHA-256 of the file's
    final bytes. Same content → same path → reused across regions
    and across runs.
    """
    digest = hashlib.sha256(content.encode("utf-8")).hexdigest()
    return f"luts/cache/{digest}.cube"


def resolve_cached_lut_paths(
    project: Path,
    regions: list[RegionPlan],
    size_override: int | None,
) -> list[str]:
    """Generate (or reuse) one cache file per unique
    `(look_id, size)` pair and stamp `region.lut_path` with the
    resolved cache-relative path. Returns the list of cache paths
    that were *written* this run (existing files are not relisted)
    so the planner can surface a deterministic regeneration count.
    """
    written: list[str] = []
    # Group regions by the (look_id, cube_size) tuple so we only
    # rebuild each unique LUT once. Within a group every region
    # ends up pointing at the same cache file.
    content_cache: dict[tuple[str, int], tuple[str, str]] = {}
    for region in regions:
        if region.look_id == "none":
            region.lut_path = None
            continue
        size = lut_cube_size(region.look_id, size_override)
        key = (region.look_id, size)
        if key not in content_cache:
            content = build_cube_text(region.look_id, size)
            relpath = cache_path_for(content)
            content_cache[key] = (content, relpath)
        content, relpath = content_cache[key]
        out = project / relpath
        if not out.exists():
            out.parent.mkdir(parents=True, exist_ok=True)
            out.write_text(content, encoding="utf-8")
            written.append(str(out))
        region.lut_path = relpath
    return written


# Retained as a thin shim so external callers (and the prior tool
# entry point) keep working. Prefer `resolve_cached_lut_paths`.
def generate_luts(
    project: Path,
    regions: list[RegionPlan],
    size: int | None,
) -> list[str]:
    return resolve_cached_lut_paths(project, regions, size)


def plan_json(regions: list[RegionPlan], written_luts: list[str], edl_path: Path | None) -> dict[str, Any]:
    return {
        "version": 1,
        "status": "planned",
        "edl_path": str(edl_path) if edl_path else None,
        "generated_luts": written_luts,
        "regions": [
            {
                "clip_name": r.clip.name,
                "clip_anchor": r.anchor,
                "asset_id": r.clip.asset_id,
                "scene_id": r.scene_id,
                "source_start_s": r.source_start_s,
                "source_end_s": r.source_end_s,
                "timeline_start_s": r.timeline_start_s,
                "timeline_end_s": r.timeline_end_s,
                "sample_times_s": r.sample_times_s,
                "source_sample_times_s": r.source_sample_times_s,
                "issue_tags": r.issue_tags,
                "policy": r.policy,
                "correction": r.correction,
                "consistency_group": r.consistency_group,
                "look_id": r.look_id,
                "lut_path": r.lut_path,
                "look_strength": r.look_strength,
                "clip_input_space": r.clip.clip_input_space,
                "score": r.score,
                "rationale": r.rationale,
            }
            for r in regions
        ],
    }


def write_markdown(path: Path, plan: dict[str, Any]) -> None:
    lines = [
        "# Look Region Plan",
        "",
        f"- Status: `{plan['status']}`",
        f"- Regions: {len(plan['regions'])}",
        f"- EDL: `{plan.get('edl_path') or 'not written'}`",
        "",
        "## Generated LUTs",
        "",
    ]
    if plan["generated_luts"]:
        lines.extend([f"- `{p}`" for p in plan["generated_luts"]])
    else:
        lines.append("- None.")
    lines.extend(["", "## Regions", ""])
    for region in plan["regions"]:
        lines.extend(
            [
                f"### {region['clip_name']} / {region['scene_id']}",
                "",
                f"- Timeline: {region['timeline_start_s']:.3f}s to {region['timeline_end_s']:.3f}s",
                f"- Source: {region['source_start_s']:.3f}s to {region['source_end_s']:.3f}s",
                f"- Tags: {', '.join(region['issue_tags']) or 'none'}",
                f"- Consistency group: `{region['consistency_group']}`",
                f"- Look: `{region['look_id']}`",
                f"- LUT: `{region['lut_path'] or 'none'}`",
                f"- Score: {region['score']:.3f}",
                f"- Rationale: {region['rationale']}",
                "",
            ]
        )
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(lines), encoding="utf-8")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--project", required=True, help="Montage project root")
    parser.add_argument("--color-index", action="append", required=True, help="color-analysis sidecar JSON")
    parser.add_argument("--style", default="natural", choices=["natural", "cinematic", "warm", "cool", "punchy"])
    parser.add_argument("--output-edl", required=True)
    parser.add_argument("--output-json", required=True)
    parser.add_argument("--report-md")
    parser.add_argument(
        "--lut-size",
        type=int,
        default=None,
        help=(
            "Override cube size for every generated LUT. Omit to use each look's "
            "catalog default_size (e.g. teal_orange opts into 33-cube)."
        ),
    )
    parser.add_argument("--no-splits", action="store_true", help="Do not emit Split Clip for multiple color regions in one clip")
    args = parser.parse_args()

    project = Path(args.project)
    clips = timeline_clips(project)
    indexes = load_color_indexes([Path(p) for p in args.color_index])
    regions = make_regions(clips, indexes, args.style)
    written_luts = generate_luts(project, regions, args.lut_size)
    edl_text = build_edl(regions, emit_splits=not args.no_splits)

    edl_path = Path(args.output_edl)
    edl_path.parent.mkdir(parents=True, exist_ok=True)
    edl_path.write_text(edl_text, encoding="utf-8")

    plan = plan_json(regions, written_luts, edl_path)
    json_path = Path(args.output_json)
    json_path.parent.mkdir(parents=True, exist_ok=True)
    json_path.write_text(json.dumps(plan, indent=2) + "\n", encoding="utf-8")
    if args.report_md:
        write_markdown(Path(args.report_md), plan)
    print(json.dumps(plan, indent=2))


if __name__ == "__main__":
    main()
