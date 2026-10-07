"""Draws Voxmar HD's 32×32 fish that have their own shape (sturgeon, marlin, deepwater
lanternfish) and writes assets/packs/voxmar_hd/recipes/fish.ron. Run
`python3 tools/art/fish.py`; edit this file, not the book.

Each fish is drawn once in default's `head` slot (three shades, dark to light) with fixed
ink and eye colours; its cooked form is the raw drawing in default's cooked palette, as the
16×16 sprites in default's tools.ron are. The lanternfish's photophores are a `glow` slot:
bright raw, dull cooked. Vanilla's language: a one-texel dark outline, lit from the top left.
"""
import math
import os

N = 32
INK = '#1a140e'
EYE = '#f6f2e8'
COOKED = '["#6e3a1c", "#a86a38", "#e0b070"]'


class Sprite:
    def __init__(self):
        self.px = [[None] * N for _ in range(N)]

    def put(self, x, y, v):
        x, y = int(x), int(y)
        if 0 <= x < N and 0 <= y < N:
            self.px[y][x] = v

    def get(self, x, y):
        return self.px[y][x] if 0 <= x < N and 0 <= y < N else None

    def fill(self, mask, slot='head'):
        """A mask lit from the top left: light along its top and left edges, dark along its
        bottom and right ones and its lower third, the middle shade between."""
        ys = [y for _, y in mask]
        top, bottom = (min(ys), max(ys)) if ys else (0, 0)
        for x, y in mask:
            up, left, down, right = (x, y - 1) in mask, (x - 1, y) in mask, (x, y + 1) in mask, (x + 1, y) in mask
            low = y > top + (bottom - top) * 0.62
            s = 2 if not up or not left else 0 if not down or not right or low else 1
            self.put(x, y, f'{slot}:{s}')

    def ink(self):
        edge = [(x, y) for y in range(N) for x in range(N) if self.get(x, y) is None and any(
            (p := self.get(x + dx, y + dy)) is not None and p != INK for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)))]
        for x, y in edge:
            self.put(x, y, INK)
        return self

    def recipe(self, palettes):
        letters = iter('abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ')
        legend, rows = {}, []
        for line in self.px:
            row = ''
            for v in line:
                if v is None:
                    row += '.'
                    continue
                if v not in legend:
                    legend[v] = next(letters)
                row += legend[v]
            rows.append(row)
        leg = ', '.join(f"'{c}': \"{v}\"" for v, c in legend.items())
        pal = ', '.join(f'"{k}": {v}' for k, v in palettes.items())
        body = ''.join(f'            "{r}",\n' for r in rows)
        return f'(size: (32, 32), palettes: {{{pal}}}, layers: [Grid(legend: {{{leg}}}, rows: [\n{body}        ])])'


def poly(points):
    out = set()
    for y in range(N):
        for x in range(N):
            px, py, inside = x + 0.5, y + 0.5, False
            for (x0, y0), (x1, y1) in zip(points, points[1:] + points[:1]):
                if (y0 > py) != (y1 > py) and px < x0 + (py - y0) * (x1 - x0) / (y1 - y0):
                    inside = not inside
            if inside:
                out.add((x, y))
    return out


def ellipse(cx, cy, rx, ry):
    return {(x, y) for y in range(N) for x in range(N) if ((x + 0.5 - cx) / rx) ** 2 + ((y + 0.5 - cy) / ry) ** 2 <= 1}


def eye(s, x, y, big=False):
    s.put(x, y, EYE)
    s.put(x + 1, y, EYE)
    s.put(x, y + 1, EYE)
    s.put(x + 1, y + 1, INK)
    if big:
        for dx, dy in ((-1, 0), (-1, 1), (0, -1), (1, -1), (0, 2), (1, 2), (2, 0), (2, 1)):
            s.put(x + dx, y + dy, EYE)
        s.put(x + 1, y + 1, INK)
        s.put(x, y + 1, INK)


FISH = {}


def fish(name, raw, extra=None):
    def wrap(fn):
        FISH[name] = (fn, raw, extra or {})
        return fn
    return wrap


@fish('sturgeon', '["#2e2a24", "#6a6254", "#b8ae98"]')
def _():
    # A long armoured bottom-feeder: a shovel snout with barbels, rows of bony scutes, a
    # small dorsal far back, and the shark-like tail, its upper lobe the longer.
    s = Sprite()
    body = poly([(1, 17), (5, 14), (12, 12.5), (21, 13), (26, 15), (27, 17), (26, 19), (20, 21), (11, 21.5), (5, 20.5)])
    tail = poly([(25, 15.5), (29, 10), (31, 6), (31, 9), (29.5, 15), (31, 21), (29, 21), (25, 18.5)])
    dorsal = poly([(19, 13.5), (21, 10.5), (23, 13.5)])
    s.fill(body | tail | dorsal)
    # Scutes: a row of lit diamonds along the back and one along the flank.
    for x in range(7, 24, 3):
        s.put(x, 13, 'head:2')
        s.put(x, 14, 'head:2')
        s.put(x + 1, 14, 'head:1')
    for x in range(8, 24, 3):
        s.put(x, 17, 'head:2')
        s.put(x + 1, 17, 'head:0')
    eye(s, 6, 15)
    s.ink()
    # Barbels hang under the snout.
    for x in (4, 6):
        s.put(x, 21, INK)
        s.put(x, 22, INK)
    return s


@fish('marlin', '["#14284a", "#2e5a9a", "#8ab4e8"]')
def _():
    # A billfish of warm seas: a long spear of a bill, a tall sail of a dorsal fin ribbed
    # with spines, faint bars down the flank, and a deep crescent tail. The swordfish has no
    # sail and a flat bill; this is its own fish.
    s = Sprite()
    body = poly([(6, 16), (10, 13.5), (17, 12.5), (24, 14), (27, 16), (24, 18.5), (17, 20), (10, 19.5)])
    bill = poly([(0, 15.6), (7, 14.8), (7, 16.6)])
    sail = poly([(9, 14), (10, 5), (13, 3.5), (17, 5), (21, 9), (23, 14)])
    tail = poly([(26, 15.5), (30, 7), (31, 7), (29, 16), (31, 25), (30, 25), (26, 16.5)])
    pectoral = poly([(12, 18.5), (16, 23), (15, 19)])
    s.fill(body | bill | tail)
    s.fill(sail)
    s.fill(pectoral)
    for x in (12, 15, 18):
        for y in range(4, 14):
            if s.get(x, y) == 'head:1':
                s.put(x, y, 'head:0')
    for x in (14, 17, 20, 23):
        for y in range(14, 19):
            if s.get(x, y) == 'head:1':
                s.put(x, y, 'head:2')
    eye(s, 9, 15)
    return s.ink()


@fish('deepwater_lanternfish', '["#0c1220", "#243656", "#6ad0e0"]', {'glow': '["#8af8ff"]'})
def _():
    # A small fish of the deep night: a blunt head with a huge eye, a row of glowing
    # photophores down its belly and two more by the tail.
    s = Sprite()
    body = poly([(8, 16), (10, 12.5), (16, 12), (22, 14), (24, 16), (22, 18.5), (16, 20), (10, 19.5)])
    tail = poly([(23, 15.5), (27, 12), (28, 12.5), (26, 16), (28, 19.5), (27, 20), (23, 16.5)])
    s.fill(body | tail)
    eye(s, 10, 14, big=True)
    for x in range(10, 22, 2):
        s.put(x, 18, 'glow:0')
    for x, y in ((14, 16), (17, 16), (23, 16)):
        s.put(x, y, 'glow:0')
    return s.ink()


def main():
    root = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', '..')
    out = ['''// Fish with their own shape at 32×32, drawn by tools/art/fish.py: each raw in its own
// palette, cooked as the raw drawing in default's cooked palette. Regenerate with
// `python3 tools/art/fish.py`; edit the Python, not this file.
(
    textures: {''']
    for name, (draw, raw, extra) in FISH.items():
        palettes = {'head': raw, **extra}
        out.append(f'        "block/raw_{name}": {draw().recipe(palettes)},')
        cooked = {'head': COOKED}
        if 'glow' in extra:
            cooked['glow'] = '["#5a3014"]'
        pal = ', '.join(f'"{k}": {v}' for k, v in cooked.items())
        out.append(f'        "block/cooked_{name}": (base: "block/raw_{name}", palettes: {{{pal}}}),')
    out.append('    },\n)\n')
    path = os.path.join(root, 'assets/packs/voxmar_hd/recipes/fish.ron')
    open(path, 'w').write('\n'.join(out))
    print(len(FISH), 'fish ->', path)


if __name__ == '__main__':
    main()
