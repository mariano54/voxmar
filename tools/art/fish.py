"""Draws Voxmar HD's 32×32 fish, each in its own shape, and writes
assets/packs/voxmar_hd/recipes/fish.ron. Run `python3 tools/art/fish.py`; edit this file,
not the book.

Each fish is drawn once in the `head` slot (three shades, dark to light) from default's
`fish_<name>` palette, with fixed ink and eye colours; its cooked form is the raw drawing in
default's `fish_cooked` palette, as default's 16×16 sprites in tools.ron are. The lanternfish's photophores are a `glow` slot:
bright raw, dull cooked. Vanilla's language: a one-texel dark outline, lit from the top left.
"""
import math
import os

N = 32
INK = '#1a140e'
EYE = '#f6f2e8'
COOKED = '"fish_cooked"'
DULL = {'glow': '["#5a3014"]', 'glint': '["#e8c890"]'}


class Sprite:
    def __init__(self):
        self.px = [[None] * N for _ in range(N)]

    def put(self, x, y, v):
        x, y = int(x), int(y)
        if 0 <= x < N and 0 <= y < N:
            self.px[y][x] = v

    def get(self, x, y):
        x, y = int(x), int(y)
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


def fish(name, extra=None):
    """A raw fish drawn in default's `fish_<name>` palette (tools.ron)."""
    def wrap(fn):
        FISH[name] = (fn, f'"fish_{name}"', extra or {})
        return fn
    return wrap


@fish('sturgeon')
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


@fish('marlin')
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


@fish('swordfish')
def _():
    # The broadbill: a longer, flat sword of a bill, a short rigid sickle of a dorsal fin
    # instead of the marlin's sail, a smooth unbarred flank and a sickle tail.
    s = Sprite()
    body = poly([(9, 16), (12, 13.5), (18, 12.5), (24, 14), (27, 16), (24, 18.5), (18, 19.5), (12, 19)])
    bill = poly([(0, 15.2), (10, 14.4), (10, 17), (0, 16.4)])
    dorsal = poly([(14, 13), (16, 6), (18, 6.5), (17.5, 9), (19, 13)])
    tail = poly([(26, 15.5), (29, 9), (31, 6), (30.5, 9), (29.5, 16), (30.5, 23), (31, 26), (29, 23), (26, 16.5)])
    s.fill(body | bill | tail)
    s.fill(dorsal)
    # The bill's flat edge catches the light along its top.
    for x in range(1, 10):
        s.put(x, 15, 'head:2')
    eye(s, 11, 15)
    return s.ink()


@fish('deepwater_lanternfish', {'glow': '["#8af8ff"]'})
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


def on(s, x, y, shade):
    """Shades a texel of the fish itself (never the outline or what is clear)."""
    if s.get(x, y) not in (None, INK, EYE):
        s.put(x, y, f'head:{shade}')


def spindle(x0, x1, cy, top, bottom, nose=0.0):
    """A fish body from snout (x0) to tail stalk (x1): deepest a third of the way back,
    `top` and `bottom` texels above and below its middle line there."""
    pts_top, pts_bottom = [], []
    for i in range(13):
        t = i / 12
        x = x0 + (x1 - x0) * t
        w = math.sin(math.pi * min(1.0, t * 1.5 if t < 0.33 else 0.5 + (t - 0.33) / 1.34)) if t > 0 else nose
        w = max(w, 0.22 if t > 0.9 else w)
        pts_top.append((x, cy - top * w))
        pts_bottom.append((x, cy + bottom * w))
    return poly(pts_top + pts_bottom[::-1])


def forked(x, cy, reach, spread):
    return poly([(x, cy - 1.5), (x + reach, cy - spread), (x + reach - 1.5, cy), (x + reach, cy + spread), (x, cy + 1.5)])


@fish('minnow')
def _():
    # A little slim river fish with a dark stripe from eye to tail.
    s = Sprite()
    s.fill(spindle(8, 22, 16, 3.2, 3, 0.5) | forked(21, 16, 5, 3.5))
    for x in range(11, 23):
        on(s, x, 16, 0)
    eye(s, 10, 14)
    return s.ink()


@fish('sardine')
def _():
    # Slim and silver, a blue back, a row of dark spots down the flank, a deep fork.
    s = Sprite()
    s.fill(spindle(5, 24, 16, 4, 3.6, 0.5) | forked(23, 16, 6, 5))
    for (x, y) in list(s_body_cells(s)):
        if y <= 14:
            on(s, x, y, 0 if y <= 13 else 1)
    for x in range(10, 22, 3):
        on(s, x, 15, 0)
    eye(s, 8, 15)
    return s.ink()


def s_body_cells(s):
    return [(x, y) for y in range(N) for x in range(N) if s.get(x, y) not in (None, INK, EYE)]


@fish('perch')
def _():
    # A deep-bodied lake fish: a spiny first dorsal, dark bars down its flank, a forked tail.
    s = Sprite()
    body = spindle(6, 23, 16, 6, 5, 0.6)
    spiny = poly([(10, 11), (11, 6.5), (13, 7.5), (14, 6.5), (16, 7.5), (17, 6.5), (19, 9), (20, 11)])
    s.fill(body | forked(22, 16, 6, 5.5))
    s.fill(spiny)
    for x in (11, 14, 17):
        for y in range(7, 11):
            on(s, x, y, 0)
    for x in (12, 15, 18, 21):
        for y in range(11, 21):
            if s.get(x, y) == 'head:1' or s.get(x, y) == 'head:2':
                on(s, x, y, 0)
    eye(s, 8, 14)
    return s.ink()


@fish('trout')
def _():
    # A stream trout: a pink band down its flank, dark spots over its back, a square tail.
    s = Sprite()
    body = spindle(4, 24, 16, 4.5, 4, 0.5)
    tail = poly([(23, 15), (28, 11.5), (29, 12), (29, 20), (28, 20.5), (23, 17)])
    adipose = poly([(19.5, 15), (21.5, 11.5), (23, 15)])
    s.fill(body | tail | adipose)
    for x in range(7, 23):
        on(s, x, 16, 2)
        on(s, x, 17, 2)
    for x, y in ((9, 13), (12, 14), (14, 12), (16, 14), (18, 13), (20, 15), (11, 18), (15, 18), (19, 17), (25, 14), (26, 17)):
        on(s, x, y, 0)
    eye(s, 6, 15)
    return s.ink()


@fish('cod')
def _():
    # A heavy cod: three dorsal fins, a chin barbel, a pale curving lateral line, mottled.
    s = Sprite()
    body = spindle(3, 24, 16, 5.5, 5, 0.7)
    dorsals = poly([(8, 11.5), (10, 9), (12, 11)]) | poly([(13, 11), (15, 8.5), (17, 11)]) | poly([(18, 14.5), (20, 10), (22, 15)])
    tail = poly([(23, 15), (28, 11), (29, 11.5), (29, 20.5), (28, 21), (23, 17)])
    s.fill(body | dorsals | tail)
    for x in range(6, 24):
        y = 14 + (1 if x > 14 else 0)
        on(s, x, y, 2)
    for x, y in ((9, 12), (13, 13), (17, 12), (11, 17), (16, 18), (20, 16), (8, 19)):
        on(s, x, y, 0)
    eye(s, 5, 14)
    s.ink()
    s.put(5, 21, INK)
    s.put(5, 22, INK)
    return s


@fish('salmon')
def _():
    # Streamlined and strong: a hooked jaw, black spots over its back, an adipose fin and
    # a broad forked tail.
    s = Sprite()
    body = spindle(3, 25, 16, 5, 4.2, 0.6)
    jaw = poly([(2, 17), (5, 17.5), (4, 19)])
    adipose = poly([(19.5, 15), (21.5, 10.5), (23, 15)])
    s.fill(body | jaw | adipose | forked(24, 16, 6, 6))
    for x, y in ((8, 13), (11, 12), (13, 14), (15, 12), (17, 13), (19, 14), (21, 13), (26, 13), (27, 18)):
        on(s, x, y, 0)
    eye(s, 5, 14)
    return s.ink()


@fish('carp')
def _():
    # A deep, rounded carp: big scales in rows, barbels at the mouth, a rounded tail.
    s = Sprite()
    body = spindle(5, 23, 16, 7, 5.5, 0.7)
    tail = poly([(22, 15), (26, 10.5), (29, 12), (29.5, 16), (29, 20), (26, 21.5), (22, 17)])
    dorsal = poly([(11, 10), (13, 7.5), (19, 8.5), (20, 11)])
    s.fill(body | tail | dorsal)
    for x, y in s_body_cells(s):
        if 9 <= x <= 21 and (x + 2 * ((y // 3) % 2)) % 4 == 0 and y % 3 == 0 and s.get(x, y) == 'head:1':
            on(s, x, y, 0)
            on(s, x + 1, y + 1, 0)
            on(s, x - 1, y + 1, 0)
    eye(s, 7, 14)
    s.ink()
    s.put(4, 18, INK)
    s.put(3, 19, INK)
    return s


@fish('bass')
def _():
    # A bass: a big mouth gaping back under the eye, a dark blotched stripe down the
    # flank, a spiny dorsal joined to a soft one.
    s = Sprite()
    body = spindle(3, 24, 16, 5.5, 5, 0.8)
    dorsal = poly([(9, 11.5), (10, 8), (12, 9), (13, 8), (15, 9.5), (16, 10), (18, 8.5), (21, 11), (22, 13)])
    tail = poly([(23, 15), (28, 11), (29, 12), (28, 16), (29, 20), (28, 21), (23, 17)])
    s.fill(body | dorsal | tail)
    for x in range(3, 9):
        s.put(x, 17, INK)
    for x in range(9, 24):
        if (x // 2) % 2 == 0:
            on(s, x, 16, 0)
            on(s, x, 15, 0)
    eye(s, 6, 14)
    return s.ink()


@fish('pike')
def _():
    # A long pike: a flat duck-bill snout, rows of pale spots, its dorsal and anal fins far
    # back by the tail, ready to lunge.
    s = Sprite()
    body = poly([(1, 16), (6, 14.5), (12, 13.5), (22, 13.5), (25, 15), (25, 17), (22, 18.5), (12, 18.5), (6, 17.5), (1, 16.8)])
    fins = poly([(18, 13.8), (20, 10.5), (23, 13.8)]) | poly([(18, 18.2), (20, 21.5), (23, 18.2)])
    s.fill(body | fins | forked(24, 16, 6, 5))
    for x in range(8, 23, 3):
        on(s, x, 15, 2)
        on(s, x + 1, 17, 2)
    eye(s, 6, 15)
    return s.ink()


@fish('eel')
def _():
    # An eel in an S: a long round body tapering to the tail, a low fin running the length
    # of its back.
    s = Sprite()
    cells = set()
    for i in range(140):
        t = i / 139
        x = 2 + 27 * t
        y = 16 + 5 * math.sin(t * 2 * math.pi * 1.1)
        r = 2.3 * (1 - 0.6 * t) + 0.5
        cells |= {(cx, cy) for cx in range(int(x - r) - 1, int(x + r) + 2) for cy in range(int(y - r) - 1, int(y + r) + 2) if (cx + 0.5 - x) ** 2 + (cy + 0.5 - y) ** 2 <= r * r}
    s.fill(cells)
    eye(s, 3, 14)
    return s.ink()


@fish('tuna')
def _():
    # A tuna: a deep torpedo, little finlets top and bottom before a tall crescent tail,
    # a short pectoral fin, a dashed line down the flank.
    s = Sprite()
    body = spindle(3, 25, 16, 6.5, 6, 0.6)
    dorsal = poly([(11, 11), (13, 6.5), (15.5, 10.5)])
    tail = poly([(23.5, 15), (27, 9), (30.5, 4), (30, 9), (28.5, 16), (30, 23), (30.5, 28), (27, 23), (23.5, 17)])
    s.fill(body | dorsal | tail)
    s.fill(poly([(10, 16.5), (17, 19), (11, 18)]))
    for x, y in ((19, 11), (21, 12), (23, 13), (19, 20), (21, 19), (23, 18)):
        s.put(x, y, 'head:2' if y < 16 else 'head:0')
    for x in range(6, 23, 2):
        on(s, x, 14, 0)
    eye(s, 6, 14)
    return s.ink()


@fish('glimmer_koi', {'glint': '["#ffffff"]'})
def _():
    # A koi of the glimmering pools: pale and dark patches on its colour, long flowing
    # fins and a butterfly tail, bright glints scattered over it.
    s = Sprite()
    body = spindle(5, 22, 16, 5.5, 5, 0.7)
    tail = poly([(21, 15), (25, 9), (30, 8), (31, 11), (27, 15.5), (31, 21), (30, 24), (25, 23), (21, 17)])
    dorsal = poly([(10, 12), (13, 8.5), (17, 8), (19, 12.5)])
    pectoral = poly([(10, 19), (11, 25), (14, 24), (15, 19.5)])
    s.fill(body | tail | dorsal)
    s.fill(pectoral)
    for x, y in s_body_cells(s):
        if (x, y) in ellipse(11, 14, 3, 2.2) or (x, y) in ellipse(17, 17, 2.5, 2):
            on(s, x, y, 2)
        elif (x, y) in ellipse(14.5, 18.5, 2, 1.4) or (x, y) in ellipse(19, 13, 1.5, 1.2):
            on(s, x, y, 0)
    eye(s, 7, 14)
    s.ink()
    for x, y in ((12, 13), (18, 16), (26, 11), (27, 20), (12, 22), (16, 10)):
        if s.get(x, y) not in (None, INK):
            s.put(x, y, 'glint:0')
    return s


@fish('shrimp')
def _():
    # A shrimp curled on itself: banded segments from its head round to a fanned tail, long
    # feelers and a row of little legs.
    s = Sprite()
    cells = set()
    seg = []
    for i in range(60):
        t = i / 59
        a = math.pi * (1.05 + 1.25 * t)
        x, y = 16 + 8 * math.cos(a), 15 + 8 * math.sin(a)
        r = 4.2 - 2.2 * t
        part = {(cx, cy) for cx in range(int(x - r) - 1, int(x + r) + 2) for cy in range(int(y - r) - 1, int(y + r) + 2) if (cx + 0.5 - x) ** 2 + (cy + 0.5 - y) ** 2 <= r * r}
        cells |= part
        seg.append((x, y))
    fan = poly([(seg[-1][0] - 1, seg[-1][1]), (seg[-1][0] - 6, seg[-1][1] + 3), (seg[-1][0] - 4, seg[-1][1] - 2)])
    s.fill(cells | fan)
    for i in range(6, 56, 8):
        x, y = seg[i]
        for d in range(-4, 5):
            ang = math.pi * (1.05 + 1.25 * i / 59)
            on(s, x + d * math.cos(ang), y + d * math.sin(ang), 0)
    hx, hy = seg[0]
    eye(s, int(hx) - 1, int(hy) - 2)
    s.ink()
    for i in range(10):
        s.put(hx - 2 - i, hy - 3 - i * 0.6, INK)
        s.put(hx - 1 - i * 0.8, hy - 4 - i, INK)
    return s


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
        # Cooked, what glowed or glinted raw goes dull.
        cooked = {'head': COOKED, **{slot: DULL[slot] for slot in extra}}
        pal = ', '.join(f'"{k}": {v}' for k, v in cooked.items())
        out.append(f'        "block/cooked_{name}": (base: "block/raw_{name}", palettes: {{{pal}}}),')
    out.append('    },\n)\n')
    path = os.path.join(root, 'assets/packs/voxmar_hd/recipes/fish.ron')
    open(path, 'w').write('\n'.join(out))
    print(len(FISH), 'fish ->', path)


if __name__ == '__main__':
    main()
