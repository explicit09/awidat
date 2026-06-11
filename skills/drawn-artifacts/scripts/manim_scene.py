#!/usr/bin/env python3
"""Manim Community Edition template scenes for Montage drawn artifacts.

This file is a manim SCENE MODULE — run it through the manim CLI via uv
(do not execute it directly):

    uv run --with manim manim render -qh -t --format=mov \
        skills/drawn-artifacts/scripts/manim_scene.py CounterScene \
        -o counter.mov --media_dir /tmp/manim-media

    uv run --with manim manim render -qh -t --format=mov \
        skills/drawn-artifacts/scripts/manim_scene.py BarGrowthScene \
        -o bars.mov --media_dir /tmp/manim-media

`-t` renders with a transparent background; `--format=mov` writes a
QuickTime .mov whose alpha survives into Montage's overlay compositor.
The rendered file lands under
`<media_dir>/videos/manim_scene/<quality>/<name>.mov` — copy it into the
project's `generated/drawn/` afterwards.

Scenes are parameterized through environment variables so the agent can
drive them without editing this file:

    CounterScene:
        DRAWN_COUNTER_START   (default 0)
        DRAWN_COUNTER_END     (default 100)
        DRAWN_COUNTER_SUFFIX  (default "%")
        DRAWN_COUNTER_LABEL   (default "GROWTH")
        DRAWN_COUNTER_SECONDS (default 2.0)

    BarGrowthScene:
        DRAWN_BARS_JSON     JSON array of {"label": str, "value": float}
                            (default: 3 demo bars)
        DRAWN_BARS_TITLE    (default "")
        DRAWN_BARS_SECONDS  (default 2.0)

For custom scenes, copy this file into the project, add a Scene
subclass using the brand constants below, and render it the same way.
"""

from __future__ import annotations

import json
import os

from manim import (
    DOWN,
    UP,
    Create,
    GrowFromEdge,
    Rectangle,
    Scene,
    Text,
    ValueTracker,
    VGroup,
)

GOLD = "#C8A84E"
NAVY = "#070D17"
IVORY = "#F2EDE3"


class CounterScene(Scene):
    """Animated number counting up next to a label — stat callout.

    Uses Pango `Text` (not `DecimalNumber`) so it renders without a
    LaTeX installation.
    """

    def construct(self) -> None:
        start = float(os.environ.get("DRAWN_COUNTER_START", "0"))
        end = float(os.environ.get("DRAWN_COUNTER_END", "100"))
        suffix = os.environ.get("DRAWN_COUNTER_SUFFIX", "%")
        label_text = os.environ.get("DRAWN_COUNTER_LABEL", "GROWTH")
        seconds = float(os.environ.get("DRAWN_COUNTER_SECONDS", "2.0"))

        tracker = ValueTracker(start)
        anchor = Text(
            f"{round(end):d}{suffix}", color=GOLD, font_size=140, weight="BOLD"
        )

        def counter_text() -> Text:
            value = round(tracker.get_value())
            text = Text(
                f"{value:d}{suffix}", color=GOLD, font_size=140, weight="BOLD"
            )
            # Pin to the final value's bottom edge so digits don't jitter.
            text.move_to(anchor, aligned_edge=DOWN)
            return text

        number = counter_text()
        number.add_updater(lambda m: m.become(counter_text()))
        label = Text(label_text, color=IVORY, font_size=44, weight="BOLD")
        label.next_to(anchor, DOWN, buff=0.5)
        self.add(number, label)

        self.play(tracker.animate.set_value(end), run_time=seconds)
        number.clear_updaters()
        self.wait(0.5)


class BarGrowthScene(Scene):
    """Bars growing from the baseline — quick comparison beat."""

    def construct(self) -> None:
        raw = os.environ.get(
            "DRAWN_BARS_JSON",
            '[{"label": "2023", "value": 4}, '
            '{"label": "2024", "value": 7}, '
            '{"label": "2025", "value": 11}]',
        )
        bars_spec = json.loads(raw)
        title_text = os.environ.get("DRAWN_BARS_TITLE", "")
        seconds = float(os.environ.get("DRAWN_BARS_SECONDS", "2.0"))

        max_value = max(item["value"] for item in bars_spec) or 1.0
        max_height = 4.0
        bar_width = 1.1

        columns = VGroup()
        for item in bars_spec:
            height = max_height * item["value"] / max_value
            bar = Rectangle(
                width=bar_width,
                height=height,
                fill_color=GOLD,
                fill_opacity=1.0,
                stroke_width=0,
            )
            label = Text(str(item["label"]), color=IVORY, font_size=32)
            value = Text(
                f"{item['value']:g}", color=GOLD, font_size=36, weight="BOLD"
            )
            column = VGroup(bar, label).arrange(DOWN, buff=0.3)
            value.next_to(bar, UP, buff=0.2)
            columns.add(VGroup(column, value))
        columns.arrange(buff=0.8, aligned_edge=DOWN)
        columns.move_to(DOWN * 0.5)

        if title_text:
            title = Text(title_text, color=GOLD, font_size=52, weight="BOLD")
            title.to_edge(UP)
            self.play(Create(title), run_time=0.5)

        per_bar = max(seconds / max(len(bars_spec), 1), 0.3)
        for column in columns:
            self.play(GrowFromEdge(column, DOWN), run_time=per_bar)
        self.wait(0.5)


if __name__ == "__main__":
    raise SystemExit(
        "manim_scene.py is a manim scene module; render it with:\n"
        "  uv run --with manim manim render -qh -t --format=mov "
        "manim_scene.py CounterScene"
    )
