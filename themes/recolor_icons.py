#!/usr/bin/env python3
"""Re-bake each theme's icon fills from its own `theme.json`.

An icon's colour is that theme's palette token, written into the SVG's `fill` —
which is what lets a theme ship a set nobody else has, at the cost of the colour
living in 23 files instead of one. This is what pays that cost back: change a
token in `theme.json`, run this, and every icon follows.

    python3 themes/recolor_icons.py             # every theme
    python3 themes/recolor_icons.py dark        # just this one
    python3 themes/recolor_icons.py --list      # the token each icon is baked in
    python3 themes/recolor_icons.py --check     # report drift, write nothing

**It only ever rewrites `fill`/`stroke` values, in file order.** Path data,
viewBox and structure are never touched, so this cannot alter a glyph's shape and
cannot clobber the icons that are original to this repository. Recolouring is the
whole job — there is no drawing and no rasterization here, so it needs nothing
installed. The one exception to "every icon is one palette token" is `logo`, which
`FIXED` skips: it is the app's mark, not a themed glyph.

`--check` is the safety net for the workflow this replaces: it reports any file
whose fills are not the tokens it is supposed to carry, which is what a missed
hand-edit looks like. `every_icon_has_a_parseable_svg_source` in
`tests/gui_tests.rs` covers the other half — a file that will not rasterize.
"""

import argparse
import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent

# The one list: slot -> the palette token its fill is baked from.
TOKENS = {
    "play": "text_primary",
    "pause": "text_primary",
    "stop": "text_primary",
    "prev": "text_primary",
    "next": "text_primary",
    "shuffle": "text_primary",
    "repeat": "text_primary",
    "volume": "text_primary",
    "remove": "text_primary",
    "sort_asc": "accent",
    "sort_desc": "accent",
    "star_on": "accent",
    "star_off": "text_secondary",
    "folder": "text_secondary",
    "minimize": "text_primary",
    "maximize": "text_primary",
    "restore": "text_primary",
    "gapless": "text_primary",
    "crossfade": "text_primary",
    "reverse": "text_secondary",
    "new_list": "text_secondary",
    "save": "text_secondary",
}

# `nocover` is a placeholder, not a button, and the only multi-colour file: a
# panel_bg plate so it reads as an empty slot, a border to give it an edge
# against the pane, then the disc. Its three tokens are positional — the order
# below is the order the `fill`/`stroke` attributes appear in the file, which is
# why the recolour counts them.
NOCOVER_TOKENS = ("panel_bg", "border", "text_secondary")

# Slots the recolour must not touch, and says nothing about. The logo is the app's
# mark rather than a themed glyph: it is full-colour artwork, and the three
# per-theme copies it replaces were byte-identical, so there is no palette token
# behind it. A slot with no row is already left alone, so this is not about the
# bytes — without it the logo is reported as a problem on every run and `--check`
# exits 1 forever, which is a CI failure for a file that is correct.
FIXED = {"logo"}

# Only a **hex** colour is a fill to rewrite. `fill="none"` is structural: the
# hand-authored icons group their stroked paths under `<g fill="none" …>`, and
# painting that would fill every open path — the recolour would silently turn a
# line drawing into a blob.
FILL = re.compile(r'((?:fill|stroke)=")#[0-9a-fA-F]{6}(")')


def themes():
    return sorted(p.parent.name for p in ROOT.glob("*/theme.json"))


def palette(theme_id):
    return json.loads((ROOT / theme_id / "theme.json").read_text())["palette"]


def wanted_colors(slot, tokens, count):
    """One colour per `fill`/`stroke` attribute in the file, in file order.

    A single-token slot paints *every* attribute the same colour: the
    hand-authored icons are built from several stroked and filled paths, all of
    one ink. Only `nocover` genuinely needs more than one.
    """
    if slot == "nocover":
        if count != len(NOCOVER_TOKENS):
            return None, f"has {count} fill/stroke attributes, expected {len(NOCOVER_TOKENS)}"
        return [tokens[t] for t in NOCOVER_TOKENS], None
    return [tokens[TOKENS[slot]]] * count, None


def recolor(slot, path, tokens):
    """`(new contents or None, problem or None)`.

    The colours are applied **in file order** rather than matched against the
    current hex, so this works whatever the old palette was — a token that has
    since been edited in `theme.json` no longer matches anything, and matching on
    it would leave that theme's icons stale forever.
    """
    svg = path.read_text()
    found = FILL.findall(svg)
    if not found:
        return None, "no fill/stroke attribute to recolor"
    wanted, problem = wanted_colors(slot, tokens, len(found))
    if problem:
        return None, problem
    it = iter(wanted)
    new = FILL.sub(lambda m: m.group(1) + next(it) + m.group(2), svg)
    return (None if new == svg else new), None


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("themes", nargs="*", help="themes to recolor (default: all)")
    ap.add_argument("--list", action="store_true", help="list the token per icon and exit")
    ap.add_argument("--check", action="store_true", help="report drift, write nothing")
    args = ap.parse_args()

    if args.list:
        for name, token in sorted(TOKENS.items()):
            print(f"{name}.svg  {token}")
        print(f"nocover.svg  {' + '.join(NOCOVER_TOKENS)}")
        for name in sorted(FIXED):
            print(f"{name}.svg  (fixed colours, not recoloured)")
        return 0

    available = themes()
    wanted = args.themes or available
    unknown = [t for t in wanted if t not in available]
    if unknown:
        return sys.exit(f"no such theme: {', '.join(unknown)} — try without arguments to list them")

    problems, drift = [], 0
    for theme_id in wanted:
        tokens = palette(theme_id)
        for token in {*TOKENS.values(), *NOCOVER_TOKENS} - set(tokens):
            sys.exit(f"{theme_id}: no {token!r} token in theme.json")

        changed = 0
        for path in sorted((ROOT / theme_id / "icons").glob("*.svg")):
            slot = path.stem
            if slot in FIXED:
                continue
            if slot not in TOKENS and slot != "nocover":
                problems.append(f"{theme_id}/{path.name}: no token in TOKENS, left alone")
                continue
            result, problem = recolor(slot, path, tokens)
            if problem:
                problems.append(f"{theme_id}/{path.name}: {problem}")
                continue
            if result is None:
                continue
            drift += 1
            changed += 1
            if not args.check:
                path.write_text(result)
        print(f"{theme_id}: {'would recolor' if args.check else 'recolored'} {changed} icon(s)")

    for p in problems:
        print(f"tplay: {p}", file=sys.stderr)
    if drift and args.check:
        print(f"\n{drift} icon(s) are not carrying their token — rerun without --check", file=sys.stderr)
        return 1
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
