#!/usr/bin/env python3
"""Writes the default pack's decor.ron: the greyscale drawings of the decor blocks.

Run from the repo root: `python3 tools/art/decor.py`. Edit this script, not the book.

These textures are not coloured. A block's `tint: Fixed(r, g, b)` (blocks_furnishings.ron)
multiplies them by a wood's or a dye's colour when the texture array is built, so every
shade here is light: a mid shade times a dark dye still reads. They are 16x16 like the rest
of the default pack; Voxmar HD draws every texture it does not redraw as this recipe at
twice the fineness, so both packs show them, and the pack thread can redraw them at 32.

Shapes (content/base/shapes.ron) map a texture by position, as a block model does, so a box
shows the part of its texture under it: a desk's drawers (a box from x 0 to 6) show columns
0-5, or 10-15 seen from the front, and its knobs are drawn in both.
"""

import os

W, C, I = "wood", "cloth", "ink"


def canvas(fill="."):
    return [[fill] * 16 for _ in range(16)]


def rect(g, x0, y0, x1, y1, ch, fill=False):
    for y in range(y0, y1 + 1):
        for x in range(x0, x1 + 1):
            if fill or x in (x0, x1) or y in (y0, y1):
                g[y][x] = ch


def rows(g):
    return "[" + ", ".join('"' + "".join(r) + '"' for r in g) + "]"


def grid(g, legend):
    leg = ", ".join(f"'{k}': \"{v}\"" for k, v in legend.items())
    return f"Grid(legend: {{{leg}}}, rows: {rows(g)})"


def panel():
    # Two raised panels in a frame, a knob at each side of the gap. Mirrored, so the same
    # from either side.
    g = canvas()
    rect(g, 0, 0, 15, 15, "d")
    for x in (7, 8):
        for y in range(16):
            g[y][x] = "d"
    for x0 in (2, 10):
        rect(g, x0, 2, x0 + 3, 13, "m", fill=True)
        for x in range(x0, x0 + 4):
            g[2][x] = "l"
            g[13][x] = "d"
        for y in range(2, 14):
            g[y][x0] = "l"
            g[y][x0 + 3] = "d"
    for x in (6, 9):
        g[7][x] = "l"
        g[8][x] = "d"
    return grid(g, {"d": f"{W}:0", "m": f"{W}:3", "l": f"{W}:5"})


def drawers():
    # Three drawers a row, a knob at the middle of each end of the row.
    g = canvas()
    for y0 in (0, 5, 10):
        rect(g, 0, y0, 15, y0 + 4, "d")
        for x in range(1, 15):
            for y in range(y0 + 1, y0 + 4):
                g[y][x] = "m"
            g[y0 + 1][x] = "l"
        for x in (3, 12):
            g[y0 + 2][x] = "k"
            g[y0 + 3][x] = "d"
    g[15] = ["d"] * 16
    return grid(g, {"d": f"{W}:0", "m": f"{W}:3", "l": f"{W}:5", "k": f"{I}:2"})


def books():
    # Two bands of spines, the lower taller. Deterministic spines of one to three columns.
    g = canvas("d")
    shades = "lmnop"
    seed = 7
    for (top, bottom) in ((8, 13), (2, 5)):
        x = 0
        while x < 16:
            seed = (seed * 1103515245 + 12345) & 0x7FFFFFFF
            width = 1 + (seed >> 8) % 3
            rise = (seed >> 12) % 2 if bottom - top > 4 else 0
            shade = shades[(seed >> 16) % len(shades)]
            for xx in range(x, min(x + width, 16)):
                for y in range(top + rise, bottom + 1):
                    g[y][xx] = shade
                g[bottom][xx] = "d"
            if x + width < 16:
                for y in range(top + rise, bottom + 1):
                    g[y][min(x + width, 15)] = "d" if width > 1 else shade
            x += width + (1 if (seed >> 20) % 5 == 0 else 0)
    return grid(g, {"d": f"{W}:0", "l": f"{C}:1", "m": f"{C}:2", "n": f"{C}:3", "o": f"{C}:4", "p": f"{C}:5"})


def cask():
    # Staves running east to west with two hoops round them at the ends (columns 4-5 and 10-11).
    g = canvas()
    for x in (4, 10):
        for y in range(16):
            g[y][x] = "h"
            g[y][x + 1] = "d"
    return grid(g, {"h": f"{W}:5", "d": f"{W}:0"})


def crate():
    g = canvas()
    rect(g, 0, 0, 15, 15, "d")
    rect(g, 1, 1, 14, 14, "m")
    rect(g, 2, 2, 13, 13, ".")
    for i in range(2, 14):
        g[i][i] = "l"
    for y in (4, 8, 11):
        for x in range(2, 14):
            if g[y][x] == ".":
                g[y][x] = "s"
    for (x, y) in ((1, 1), (14, 1), (1, 14), (14, 14)):
        g[y][x] = "d"
    return grid(g, {"d": f"{W}:0", "m": f"{W}:4", "l": f"{W}:5", "s": f"{W}:1"})


def rug():
    g = canvas()
    for y in range(16):
        for x in range(16):
            r = min(x, y, 15 - x, 15 - y)
            d = abs(x - 7.5) + abs(y - 7.5)
            if r < 1:
                g[y][x] = "d"
            elif r == 1:
                g[y][x] = "m"
            elif r == 2:
                g[y][x] = "l"
            elif d <= 3:
                g[y][x] = "d"
            elif d <= 5:
                g[y][x] = "l"
    return grid(g, {"d": f"{C}:0", "m": f"{C}:2", "l": f"{C}:5"})


def banner():
    # A band across the top, a diamond below it, a darker hem.
    g = canvas()
    for x in range(16):
        g[2][x] = g[3][x] = "d"
        g[15][x] = "d"
    for y in range(16):
        for x in range(16):
            if abs(x - 7.5) + abs(y - 8.5) <= 3.5:
                g[y][x] = "l"
            if abs(x - 7.5) + abs(y - 8.5) <= 1.5:
                g[y][x] = "d"
    return grid(g, {"d": f"{C}:0", "l": f"{C}:5"})


def paint(palette, base, shade, extra=""):
    return f"Paint(palette: \"{palette}\", base: {base}, shade: [{shade}], clean: true)" + extra


def book():
    t = {}
    weave = "Stripes(period: 2.0, width: 0.5, amount: 0.14), Stripes(period: 2.0, width: 0.5, amount: 0.14, vertical: true), Noise(cells: (4, 4), amount: 0.12), Grain(0.1)"
    wood_pal = {"wood": "furn_wood", "ink": "furn_ink"}
    cloth_pal = {"cloth": "furn_cloth", "ink": "furn_ink"}

    def tex(name, palettes, layers):
        pal = ", ".join(f'"{k}": "{v}"' for k, v in palettes.items())
        t[name] = f'        "block/{name}": (palettes: {{{pal}}}, layers: [\n' + "".join(f"            {layer},\n" for layer in layers) + "        ]),\n"

    tex("furniture_wood", wood_pal, [paint(W, 0.58, "Boards(height: 4, seam: -0.5, joint: -0.4, value: 0.2, bevel: 0.12), Noise(cells: (4, 16), octaves: 2, amount: 0.25)")])
    tex("furniture_grain", wood_pal, [paint(W, 0.58, "Noise(cells: (8, 2), octaves: 2, amount: 0.32), Stripes(period: 4.0, width: 0.25, amount: -0.2, vertical: true, phase: 1.0), Grain(0.12)")])
    tex("furniture_panel", wood_pal, [paint(W, 0.58, "Noise(cells: (8, 2), amount: 0.25), Grain(0.1)"), panel()])
    tex("furniture_drawers", wood_pal, [paint(W, 0.58, "Noise(cells: (8, 2), amount: 0.25), Grain(0.1)"), drawers()])
    tex("furniture_books", {"wood": "furn_wood", "cloth": "furn_cloth"}, [paint(W, 0.3, "Grain(0.1)"), books()])
    tex("furniture_cask", wood_pal, [paint(W, 0.58, "Stripes(period: 3.0, width: 0.2, amount: -0.45, phase: 1.0), Noise(cells: (2, 8), amount: 0.22), Grain(0.1)"), cask()])
    tex("furniture_crate", wood_pal, [paint(W, 0.58, "Boards(height: 4, seam: -0.5, joint: -0.3, value: 0.2, bevel: 0.1), Noise(cells: (4, 8), amount: 0.2)"), crate()])
    tex("furniture_linen", cloth_pal, [paint(C, 0.62, weave + ", Stripes(period: 16.0, width: 0.125, amount: -0.4, phase: 9.0)")])
    tex("furniture_pot", wood_pal, [paint(W, 0.6, "Noise(cells: (3, 3), amount: 0.2), Stripes(period: 8.0, width: 0.125, amount: -0.35, phase: 2.0), Gradient(from: 0.15, to: -0.2, across: true), Grain(0.1)")])
    tex("cloth_grey", cloth_pal, [paint(C, 0.58, weave)])
    tex("rug_grey", cloth_pal, [paint(C, 0.5, weave), rug()])
    tex("banner_grey", cloth_pal, [paint(C, 0.55, weave), banner()])
    tex("curtain_grey", cloth_pal, [paint(C, 0.6, "Stripes(period: 4.0, width: 0.5, amount: 0.3, vertical: true, warp: 0.6), Stripes(period: 4.0, width: 0.125, amount: -0.25, vertical: true, phase: 3.0), Gradient(from: 0.05, to: -0.1), Grain(0.08)")])
    tex("lantern_grey", cloth_pal, [paint(C, 0.8, "Stripes(period: 3.0, width: 0.34, amount: -0.28, phase: 1.0), Stripes(period: 8.0, width: 0.125, amount: -0.25, vertical: true), Noise(cells: (3, 3), amount: 0.12)")])
    tex("cushion_grey", cloth_pal, [paint(C, 0.58, weave + ", Border(width: 1, amount: -0.3)"), grid_button()])
    tex("bunting_grey", cloth_pal, [paint(C, 0.6, weave + ", Stripes(period: 8.0, width: 0.25, amount: -0.3, slope: 1.0, phase: 2.0)")])
    tex("awning_grey", cloth_pal, [paint(C, 0.66, weave + ", Stripes(period: 8.0, width: 0.5, amount: -0.38, vertical: true)")])
    tex("crockery_grey", cloth_pal, [paint(C, 0.66, "Rings(spacing: 2.8, width: 0.3, amount: -0.25), Stripes(period: 8.0, width: 0.125, amount: -0.3, phase: 2.0), Noise(cells: (3, 3), amount: 0.15), Border(width: 1, amount: -0.15, sides: (0.0, 1.0, 0.0, 1.0))")])
    metal_pal = {"metal": "furn_metal", "ink": "furn_ink"}
    tex("candle_grey", cloth_pal, [paint(C, 0.78, "Stripes(period: 5.0, width: 0.2, amount: -0.15, vertical: true, phase: 1.0), Noise(cells: (2, 4), amount: 0.12)")])
    tex("metal_grey", metal_pal, [paint("metal", 0.6, "Noise(cells: (4, 4), amount: 0.2), Stripes(period: 8.0, width: 0.125, amount: -0.3, phase: 1.0), Grain(0.08)"), pane()])
    tex("brazier_coals", {"coal": "furn_coal"}, [paint("coal", 0.55, "Noise(cells: (4, 4), octaves: 2, amount: 0.5), Speckle(chance: 0.2, amount: 0.4), Grain(0.2)")])
    tex("brazier_iron", {"iron": "furn_iron"}, [paint("iron", 0.5, "Noise(cells: (4, 4), amount: 0.25), Stripes(period: 8.0, width: 0.125, amount: -0.3, phase: 5.0), Grain(0.1)")])
    tex("cauldron_iron", {"iron": "furn_iron"}, [paint("iron", 0.45, "Noise(cells: (4, 4), amount: 0.25), Stripes(period: 8.0, width: 0.125, amount: -0.3, phase: 7.0), Speckle(chance: 0.04, amount: 0.5)")])
    tex("cauldron_stew", {"stew": "furn_stew"}, [paint("stew", 0.5, "Noise(cells: (4, 4), octaves: 2, amount: 0.5), Speckle(chance: 0.12, amount: 0.5)")])
    tex("stove_bricks", {"brick": "furn_brick"}, [paint("brick", 0.55, "Bricks(size: (8, 4), mortar: -0.45, value: 0.25, bevel: 0.12), Noise(cells: (4, 4), amount: 0.15)")])
    tex("stove_plate", {"iron": "furn_iron"}, [paint("iron", 0.4, "Noise(cells: (4, 4), amount: 0.2), Grain(0.1)"), "Paint(palette: \"iron\", base: 0.15, mask: [Circle(radius: 4.0, soft: 0.5)], above: 0.5)"])
    tex("loom_warp", {"thread": "furn_thread"}, [paint("thread", 0.55, "Stripes(period: 2.0, width: 0.5, amount: 0.4, vertical: true), Noise(cells: (2, 8), amount: 0.2)")])
    tex("wind_flags", {}, [wind_flags()])
    tex("shingles_grey", wood_pal, [paint(W, 0.6, "Noise(cells: (4, 4), amount: 0.2), Stripes(period: 2.0, width: 0.5, amount: -0.08, vertical: true), Grain(0.12)"), shingles()])
    tex("basket_grey", wood_pal, [paint(W, 0.58, "Boards(height: 2, seam: -0.4, joint: -0.3, value: 0.22, bevel: 0.1), Grain(0.12)")])
    return t


def pane():
    # The lantern's glass: a lit pane in the frame, where the body is (columns 5-10, rows 6-12).
    g = canvas()
    rect(g, 6, 7, 9, 11, "l", fill=True)
    return grid(g, {"l": "metal:5"})


def wind_flags():
    colours = {"a": "#3a6ac8", "b": "#f0f0f0", "c": "#d03a30", "d": "#3a9a48", "e": "#f0c030"}
    row = "aaabbbcccdddeeee"
    g = [list(row) for _ in range(16)]
    return grid(g, colours)


def shingles():
    # Courses four texels high, as the roof shape steps (shapes.ron "roof"): each course's
    # top row catches the light, its lowest lies in its own shadow on the course below, and
    # the joints between shingles break bond from course to course.
    g = canvas()
    for c in range(4):
        y0 = 4 * c
        for x in range(16):
            g[y0][x] = "l"
            g[y0 + 3][x] = "d"
        for x in range(1 + 2 * (c % 2), 16, 4):
            for y in range(y0, y0 + 3):
                g[y][x] = "d"
    return grid(g, {"d": f"{W}:0", "l": f"{W}:5"})


def grid_button():
    g = canvas()
    for (x, y) in ((7, 7), (8, 7), (7, 8), (8, 8)):
        g[y][x] = "d"
    g[7][7] = "l"
    return grid(g, {"d": f"{C}:0", "l": f"{C}:5"})


def main():
    out = """// The decor blocks' greyscale drawings (content/base/blocks_furnishings.ron). Written by
// tools/art/decor.py: edit the script, not this book. A block's tint multiplies them by a
// wood's or a dye's colour, so every shade is light.
(
    palettes: {
        "furn_wood": ["#8c8c8c", "#a2a2a2", "#b6b6b6", "#c9c9c9", "#dadada", "#eaeaea"],
        "furn_cloth": ["#aaaaaa", "#bcbcbc", "#cdcdcd", "#dcdcdc", "#ebebeb", "#f8f8f8"],
        "furn_ink": ["#505050", "#686868", "#808080"],
        "furn_metal": ["#7c7c84", "#9494a0", "#acacb8", "#c4c4d0", "#dcdce6", "#f2f2fa"],
        "furn_iron": ["#26262a", "#34343a", "#44444c", "#585862", "#707080"],
        "furn_coal": ["#2a1810", "#5a2410", "#8c3a14", "#c85a1c", "#f08c28", "#ffc060"],
        "furn_stew": ["#3a2a14", "#4a5a1c", "#5e7a24", "#728e30", "#8aa63c"],
        "furn_brick": ["#5a3a30", "#704a3c", "#865a48", "#9a6c58", "#ae7e68"],
        "furn_thread": ["#8a7a5a", "#a6966e", "#c2b286", "#d8c89c"],
    },
    textures: {
"""
    out += "".join(book().values())
    out += "    },\n)\n"
    path = os.path.join(os.path.dirname(__file__), "../../assets/packs/default/recipes/decor.ron")
    with open(path, "w") as f:
        f.write(out)


if __name__ == "__main__":
    main()
