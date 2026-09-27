#!/usr/bin/env python3
"""Generate theme icons from code.

Every icon this script owns is a **mark**: a function that draws into a
supersampled canvas in 20px coordinates. Adding one is two edits — write the
`draw_*` function, add one `MARKS` entry — and nothing else. The theme list, the
color token, the output paths, the anti-aliasing and the slot check are all
handled here, because every one of those is a place a new icon could otherwise
forget.

    python3 themes/generate_icons.py                # every mark, every theme
    python3 themes/generate_icons.py reverse save   # just these
    python3 themes/generate_icons.py --list
    python3 themes/generate_icons.py --check        # verify the slots line up

**`--check` is the one that matters.** A generated icon lives in three places: a
mark here, a variant in `Icon` plus a row in `ALL` plus a row in `DATA`
(`src/gui/theme.rs` — all three append-only, see the Icons bullet in AGENTS.md),
and a PNG in every theme. This catches the direction nothing else can: a mark
with no slot. A missing PNG is not an error anywhere in the app — `load_icons`
does `fs::read(..).ok()?` and the button falls back to its glyph, which on this
font is a tofu box, so the icon would be broken in every theme forever and
nothing would say so. The other direction, a slot with no PNG, is caught by
`every_bundled_theme_decodes_the_generated_icons` in `tests/gui_tests.rs`.

Colors are baked per theme from that theme's own `theme.json`; a mark may
override the token. Marks are drawn at 8x and downsampled, which is where the
anti-aliasing comes from — Pillow does not anti-alias line drawing.

Requires Pillow >= 9.2 (`ImageDraw.ellipse` gained `width=`).
"""

import argparse
import json
import math
import pathlib
import re
import sys
from dataclasses import dataclass
from typing import Callable

from PIL import Image, ImageDraw

ROOT = pathlib.Path(__file__).resolve().parent
REPO = ROOT.parent

SIZE = 20  # every toolbar icon; `nocover.png` is the one exception, at 128
SS = 8  # supersample factor: draw at SIZE*SS, then downsample
TOKEN = "text_secondary"  # the token a mark is baked in unless it overrides it


# ── primitives ───────────────────────────────────────────────────────────────
# Coordinates are in icon space (0..SIZE); `px` maps them onto the canvas.


def px(v):
    return v * SS


def line(d, x1, y1, x2, y2, w=2.0):
    d.line([px(x1), px(y1), px(x2), px(y2)], fill=255, width=int(w * SS), joint="curve")


def triangle(d, points):
    d.polygon([(px(x), px(y)) for x, y in points], fill=255)


def circle(d, cx, cy, r, w=2.0):
    """A stroked circle. `joint="curve"` is not needed here — Pillow strokes
    ellipses cleanly — so this is the smooth counterpart to `rays`."""
    d.ellipse(
        [px(cx - r), px(cy - r), px(cx + r), px(cy + r)],
        outline=255,
        width=int(w * SS),
    )


def rays(d, cx, cy, r_in, r_out, count, w=2.0, offset_deg=-90.0):
    """`count` straight spokes from `r_in` out to `r_out` — a sun with no core.

    Radii are given as fractions of the icon's half-width by the caller, so a
    brief like "20% out to 100%" is expressed directly. The outer's fraction is
    there and not 1.0 because the stroke is centred on the line: at w=2 its outer
    edge already reaches half a pixel past r, so 0.9 puts the ray tip exactly on
    the icon's edge instead of clipping it."""
    for i in range(count):
        a = math.radians(offset_deg + 360.0 * i / count)
        line(
            d,
            cx + r_in * math.cos(a),
            cy + r_in * math.sin(a),
            cx + r_out * math.cos(a),
            cy + r_out * math.sin(a),
            w=w,
        )


# ── marks ────────────────────────────────────────────────────────────────────


def draw_reverse(d):
    """One arrow up, one arrow down, side by side (a '⇅').

    Two earlier attempts failed and both failures are worth remembering. Two
    *horizontal* arrows pointing opposite ways was indistinguishable from the
    hand-drawn `shuffle.png` beside it in the same toolbar — two crossings read
    the same as two directions. A single *left* arrow then shared a silhouette
    with the hand-drawn `prev` (a bar plus a left triangle), which is the one
    mark a "back to front" arrow is most likely to be mistaken for. Vertical is
    the axis nothing else in the set uses."""
    line(d, 7.0, 15.5, 7.0, 6.0)
    triangle(d, [(7.0, 2.5), (3.5, 7.5), (10.5, 7.5)])
    line(d, 13.0, 4.5, 13.0, 14.0)
    triangle(d, [(13.0, 17.5), (9.5, 12.5), (16.5, 12.5)])


def draw_new_list(d):
    """Eight rays from 40% out to the edge, with no core — a blanking mark. The
    core is deliberately wide: a 4px hole left the rays long enough to read as a
    ring with dashes instead of as a sun.

    Not a ring of chords (that reads as a solid token) and not a plus (that says
    "add", the wrong verb for a button that *creates* an empty playlist rather
    than adding to the current one)."""
    half = SIZE / 2
    rays(d, half, half, 0.4 * half, 0.9 * half, 8, w=2.0)


def draw_save(d):
    """An arrow down into a tray — a floppy's notch does not survive 20px."""
    line(d, 10.0, 2.5, 10.0, 12.0)
    triangle(d, [(6.0, 10.0), (14.0, 10.0), (10.0, 15.5)])
    line(d, 3.0, 17.5, 17.0, 17.5)


@dataclass(frozen=True)
class Mark:
    """One icon. `size` and `token` default to the house values; override either
    when an icon is not a 20×20 toolbar mark (`nocover.png` is 128) or must be
    baked in a different palette token than its neighbours."""

    draw: Callable[[ImageDraw.ImageDraw], None]
    size: int = SIZE
    token: str = TOKEN


# The one list. A mark is registered by being a key here, so `OWNED` cannot
# drift away from `MARKS` the way a second hand-maintained list would.
MARKS: dict[str, Mark] = {
    "reverse": Mark(draw_reverse),
    "new_list": Mark(draw_new_list),
    "save": Mark(draw_save),
}


# ── rendering ────────────────────────────────────────────────────────────────


def hex_to_rgba(value):
    v = value.lstrip("#")
    return (int(v[0:2], 16), int(v[2:4], 16), int(v[4:6], 16), 255)


def render(mark: Mark, color) -> Image.Image:
    """Draw one mark: the mark white, its coverage as the alpha, color baked in."""
    big = Image.new("L", (mark.size * SS, mark.size * SS), 0)
    mark.draw(ImageDraw.Draw(big))
    small = big.resize((mark.size, mark.size), Image.LANCZOS)
    out = Image.new("RGBA", (mark.size, mark.size), (0, 0, 0, 0))
    out.putalpha(small)
    out.paste(Image.new("RGBA", (mark.size, mark.size), color), (0, 0), small)
    return out


def themes():
    return sorted(p.parent.name for p in ROOT.glob("*/theme.json"))


def palette(theme_id):
    return json.loads((ROOT / theme_id / "theme.json").read_text())["palette"]


# ── the slot check ───────────────────────────────────────────────────────────


def png_slots():
    """Every `.png` file name `Icon::DATA` in `src/gui/theme.rs` claims."""
    source = (REPO / "src" / "gui" / "theme.rs").read_text()
    return set(re.findall(r'\("([a-z_0-9]+\.png)"', source))


def check():
    """Every mark has a slot. Returns the problems, empty when it all lines up.

    Only *this* direction is checked. A slot with no mark is the normal case —
    twenty icons are hand-drawn — and a slot with no *PNG* is already covered by
    `every_bundled_theme_decodes_the_generated_icons` in `tests/gui_tests.rs`."""
    slots = png_slots()
    return sorted(
        f"{name}.png has a mark here but no Icon::DATA slot"
        for name in MARKS
        if f"{name}.png" not in slots
    )


# ── cli ──────────────────────────────────────────────────────────────────────


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("marks", nargs="*", help="marks to write (default: all)")
    ap.add_argument("--list", action="store_true", help="list the marks and exit")
    ap.add_argument(
        "--check",
        action="store_true",
        help="verify every mark has an Icon slot; writes nothing",
    )
    args = ap.parse_args()

    if args.list:
        for name, mark in MARKS.items():
            print(f"{name}.png  {mark.size}x{mark.size}  {mark.token}")
        return 0

    if args.check:
        problems = check()
        for p in problems:
            print(f"tplay: {p}", file=sys.stderr)
        if problems:
            print(
                "\nadd the variant to `Icon` AND a row to `ALL` AND a row to "
                "`DATA` in src/gui/theme.rs — all three, at the end",
                file=sys.stderr,
            )
            return 1
        print(f"ok: {len(MARKS)} marks, all slotted")
        return 0

    try:
        import PIL  # noqa: F401
    except ImportError:
        return sys.exit("Pillow >= 9.2 is required: pip install pillow")

    unknown = [n for n in args.marks if n not in MARKS]
    if unknown:
        return sys.exit(f"no such mark: {', '.join(unknown)} (try --list)")

    wanted = args.marks or list(MARKS)
    for theme_id in themes():
        tokens = palette(theme_id)
        for name in wanted:
            mark = MARKS[name]
            if mark.token not in tokens:
                sys.exit(f"{theme_id}: no {mark.token!r} token in theme.json")
            out_dir = ROOT / theme_id / "icons"
            out_dir.mkdir(parents=True, exist_ok=True)
            render(mark, hex_to_rgba(tokens[mark.token])).save(
                out_dir / f"{name}.png", optimize=True
            )
        print(f"{theme_id}: wrote {len(wanted)} icon(s)")

    problems = check()
    if problems:
        for p in problems:
            print(f"tplay: {p}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
