#!/usr/bin/env python3
"""Generate `.csp` 1D shaper LUTs for the camera Log encodings the
agent can name in `awidat.color_pipeline.shaper_lut`.

Each shaper maps a Log-encoded source value (per channel, in
`[0, 1]`) to a Rec.709 gamma-2.4 display-referred value. That's the
input space every catalog look declares (`default_input_space =
"rec709_g24"`), so chaining `shaper_lut → look_lut` puts the LUT in
the right space without an explicit working_space step.

Output format: CineSpace `.csp` 1D (the format FFmpeg's `lut1d`
filter expects). Identity pre-LUTs, 4096-entry 1D table per
channel.

Sources for each EOTF:
  - ARRI LogC3 (EI 800): "ARRI LOGC Curve" whitepaper, 2017
    https://www.arri.com/en/learn-help/learn-help-camera-system/image-science/log-c
  - ARRI LogC4: "ARRI LogC4 LOGarithmic Color Space SPECIFICATION",
    2022
  - Sony S-Log3: Sony Whitepaper "S-Log White Paper Sony S-Log
    Curves" (2017)
  - Panasonic V-Log: "VARICAM V-Log/V-Gamut" technical brief
  - Blackmagic Film Gen 5: DaVinci Resolve 18 manual
    ("Working with Blackmagic Design Cameras")

Each formula maps Log → linear scene-referred; then we apply
`linear → Rec.709 gamma 2.4` (`pow(linear, 1/2.4)`) as the
display-referred output encoding. The gamma-2.4 step is a
reasonable approximation of BT.1886 for SDR delivery and matches
the catalog's `rec709_g24` space id.

Re-run: `python3 generate_shaper_luts.py`. Files land in
`../shapers/`. The script is idempotent.
"""

from __future__ import annotations

import math
from pathlib import Path
from typing import Callable

LUT_SIZE = 4096
SHAPER_DIR = Path(__file__).resolve().parent.parent / "shapers"

# Final display-referred encoding: Rec.709 gamma 2.4. Real BT.1886
# is `pow(linear, 1/2.4)` so the catalog's `rec709_g24` matches.
DISPLAY_GAMMA = 2.4


def encode_rec709_g24(linear: float) -> float:
    """Linear scene-referred → Rec.709 gamma-2.4 display-referred."""
    if linear <= 0.0:
        return 0.0
    return min(1.0, linear ** (1.0 / DISPLAY_GAMMA))


def arri_logc3_ei800_to_linear(t: float) -> float:
    """ARRI LogC3 (EI 800) → linear scene. Source: ARRI LogC Curve
    whitepaper. The piecewise threshold of 0.149659 marks where the
    affine "toe" segment meets the logarithmic body."""
    a = 5.555556
    b = 0.047996
    c = 0.244161
    d = 0.386036
    e = 5.367655
    f = 0.092809
    if t > 0.149659:
        return (10 ** ((t - d) / c) - b) / a
    return (t - f) / e


def arri_logc4_to_linear(t: float) -> float:
    """ARRI LogC4 → linear scene. Source: ARRI LogC4 spec, 2022.
    Constants `a`, `b`, `c`, `s`, `t0` per §4 of the spec.
    """
    a = (2.0**18.0 - 16.0) / 117.45
    b = (1023.0 - 95.0) / 1023.0
    c = 95.0 / 1023.0
    s = (7.0 * math.log(2.0) * 2.0**6.0) / (a * math.log(10.0))
    t0 = -7.0 * math.log(2.0) / a + math.log(0.18) / math.log(10.0) - c
    if t < t0:
        return (t - t0) * s
    return (10.0 ** ((t - c) / b) - 64.0) / a


def sony_slog3_to_linear(t: float) -> float:
    """Sony S-Log3 → linear scene. Source: Sony S-Log3 whitepaper.
    The breakpoint at 0.1673609 separates the affine toe from the
    log body; the log body uses base 10."""
    if t >= 0.1673609:
        return (10.0 ** ((t * 1023.0 - 420.0) / 261.5) - 0.037584) * 0.18 / 0.075
    return ((t * 1023.0 - 95.0) * 0.01125) / (171.2102946929 - 95.0)


def panasonic_vlog_to_linear(t: float) -> float:
    """Panasonic V-Log → linear scene. Source: Panasonic V-Log
    technical brief. The cut-off at 0.181 splits a linear toe from
    a log10 body."""
    cut2 = 0.181
    b = 0.00873
    c = 0.241514
    d = 0.598206
    if t < cut2:
        return (t - 0.125) / 5.6
    return 10.0 ** ((t - d) / c) - b


def bmd_film_gen5_to_linear(t: float) -> float:
    """Blackmagic Film Gen 5 → linear scene. Source: DaVinci Resolve
    18 manual. The Gen 5 curve uses a Cineon-style log mapping with
    a published linear toe; the constants below match the published
    inverse OETF."""
    a = 0.08692876065491224
    b = 0.005494072432257808
    c = 0.5300133392291939
    cut = 0.005
    lg2 = 1.0 / math.log(2.0)
    if t < cut:
        return (t - b) / 7.0
    return (2.0 ** ((t - c) / a)) - 1.0
    _ = lg2  # currently unused; kept for clarity on the original derivation


SHAPERS: dict[str, tuple[str, Callable[[float], float]]] = {
    "arri_logc3": (
        "ARRI LogC3 EI 800 → Rec.709 gamma 2.4",
        arri_logc3_ei800_to_linear,
    ),
    "arri_logc4": (
        "ARRI LogC4 → Rec.709 gamma 2.4",
        arri_logc4_to_linear,
    ),
    "slog3_sgamut3": (
        "Sony S-Log3 → Rec.709 gamma 2.4",
        sony_slog3_to_linear,
    ),
    "vlog_vgamut": (
        "Panasonic V-Log → Rec.709 gamma 2.4",
        panasonic_vlog_to_linear,
    ),
    "bmd_film_gen5": (
        "Blackmagic Film Gen 5 → Rec.709 gamma 2.4",
        bmd_film_gen5_to_linear,
    ),
}


def render_csp_1d(title: str, log_to_linear: Callable[[float], float]) -> str:
    """Render a CineSpace 1D LUT. The format FFmpeg's `lut1d`
    consumes is:
        CSPLUTV100
        1D
        <metadata block>
        <pre-LUT R> ... <pre-LUT G> ... <pre-LUT B> ...
        <size>
        <R G B> ...

    We use identity pre-LUTs (2-entry, mapping the input axis 1:1)
    so the agent sees a clean Log → display encoding without
    surprise input remapping.
    """
    lines: list[str] = [
        "CSPLUTV100",
        "1D",
        "",
        "BEGIN METADATA",
        f"awidat_shaper {title}",
        "END METADATA",
        "",
    ]
    # Per-channel identity pre-LUT (R, G, B).
    for _ in range(3):
        lines.extend(
            [
                "2",
                "0.0 1.0",
                "0.0 1.0",
            ]
        )
        lines.append("")
    lines.append(str(LUT_SIZE))
    for i in range(LUT_SIZE):
        t = i / (LUT_SIZE - 1)
        linear = max(0.0, log_to_linear(t))
        display = encode_rec709_g24(linear)
        lines.append(f"{display:.6f} {display:.6f} {display:.6f}")
    lines.append("")
    return "\n".join(lines)


def main() -> None:
    SHAPER_DIR.mkdir(parents=True, exist_ok=True)
    for space_id, (title, eotf) in SHAPERS.items():
        out = SHAPER_DIR / f"{space_id}_to_rec709_g24.csp"
        out.write_text(render_csp_1d(title, eotf), encoding="utf-8")
        print(f"wrote {out.relative_to(SHAPER_DIR.parent)}")


if __name__ == "__main__":
    main()
