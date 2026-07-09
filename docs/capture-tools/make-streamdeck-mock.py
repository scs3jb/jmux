#!/usr/bin/env python3
"""Render docs/demos/streamdeck.gif — a privacy-safe animated mock of the
Stream Deck Plus companion (~/src/deck) showing jmux workspaces as keys.

Uses deck's OWN key/touchscreen renderer (deck.render) with synthetic session
data, so the mock is pixel-identical to what the real device shows — no
hardware or real sessions involved.

Run from the deck checkout's environment so `deck` and Pillow resolve:

    uv run --project ~/src/deck python docs/capture-tools/make-streamdeck-mock.py
"""

from __future__ import annotations

import math
import os
import sys

DECK_REPO = os.environ.get("DECK_REPO", os.path.expanduser("~/src/deck"))
sys.path.insert(0, DECK_REPO)

from PIL import Image, ImageDraw  # noqa: E402

from deck import render  # noqa: E402
from deck.config import DEFAULT_COLORS  # noqa: E402
from deck.sessions import Claude, Session, State  # noqa: E402

OUT = os.path.join(os.path.dirname(__file__), "..", "demos", "streamdeck.gif")

KEY = (120, 120)
TS = (800, 100)
FRAMES = 24          # 24 × 120 ms ≈ 2.9 s loop
FRAME_MS = 120
PULSE_PERIOD = 12    # needs-input amber pulse, two cycles per loop


def claude(state: State, title: str = "claude", selected: bool = False) -> Claude:
    return Claude(panel_id=title, title=title, raw_title=title, state=state, selected=selected)


# Synthetic workspaces — one per key, states chosen to show every octopus.
SESSIONS: list[Session | None] = [
    Session("ws1", 1, "web", [claude(State.WORKING, selected=True),
                              claude(State.IDLE, "tests")], selected=True),
    Session("ws2", 2, "api", [claude(State.NEEDS_INPUT)]),
    Session("ws3", 3, "docs", [claude(State.WAITING)]),
    Session("ws4", 4, "infra", [claude(State.IDLE)]),
    Session("ws5", 5, "deploy", [claude(State.WORKING)], host="buildbox"),
    Session("ws6", 6, "data", [claude(State.IDLE)]),
    Session("ws7", 7, "ml", [claude(State.DISCONNECTED)], host="gpubox"),
    None,
]
TS_SESSION = SESSIONS[1]  # the needs-input workspace on the touchscreen
TS_SESSION.claudes[0].activity = "Which migration strategy should I use?"

MARGIN = 36
KEY_GAP = (TS[0] - 4 * KEY[0]) // 3          # spread keys across the screen width
ROW_GAP = 30
KEYS_TOP = MARGIN
TS_TOP = KEYS_TOP + 2 * KEY[1] + ROW_GAP + 28
DIALS_TOP = TS_TOP + TS[1] + 30
W = TS[0] + 2 * MARGIN
H = DIALS_TOP + 44 + MARGIN

_corner = Image.new("L", KEY, 0)
ImageDraw.Draw(_corner).rounded_rectangle([0, 0, KEY[0] - 1, KEY[1] - 1], radius=14, fill=255)


def bezel() -> Image.Image:
    img = Image.new("RGB", (W, H), (24, 25, 27))
    d = ImageDraw.Draw(img)
    d.rounded_rectangle([2, 2, W - 3, H - 3], radius=26, outline=(52, 54, 58), width=2)
    for i in range(4):  # dials
        cx = MARGIN + TS[0] * (2 * i + 1) // 8
        d.ellipse([cx - 22, DIALS_TOP, cx + 22, DIALS_TOP + 44],
                  fill=(38, 40, 43), outline=(70, 73, 78), width=2)
        d.ellipse([cx - 3, DIALS_TOP + 6, cx + 3, DIALS_TOP + 12], fill=(105, 108, 114))
    return img


def frame(fonts: render.Fonts, n: int) -> Image.Image:
    img = bezel()
    pulse = 0.8 + 0.2 * math.sin(2 * math.pi * n / PULSE_PERIOD)
    for i, s in enumerate(SESSIONS):
        key = render.render_key(KEY, s, fonts, DEFAULT_COLORS, spinner_frame=n, pulse=pulse)
        x = MARGIN + (i % 4) * (KEY[0] + KEY_GAP)
        y = KEYS_TOP + (i // 4) * (KEY[1] + ROW_GAP)
        img.paste(key, (x, y), _corner)
    ts = render.render_touchscreen_detail(TS, TS_SESSION, fonts, DEFAULT_COLORS)
    img.paste(ts, (MARGIN, TS_TOP))
    return img


def main() -> None:
    fonts = render.Fonts()
    frames = [frame(fonts, n).quantize(colors=128, dither=Image.Dither.NONE)
              for n in range(FRAMES)]
    out = os.path.abspath(OUT)
    frames[0].save(out, save_all=True, append_images=frames[1:],
                   duration=FRAME_MS, loop=0, optimize=True)
    print(f"wrote {out} ({os.path.getsize(out) // 1024} KiB, {FRAMES} frames)")


if __name__ == "__main__":
    main()
