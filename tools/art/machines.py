"""Draws the machine blocks' faces at 32×32 (content/base/blocks_machines.ron) and writes them
to assets/packs/voxmar_hd/recipes/machines.ron. Run `python3 tools/art/machines.py`; edit
this file, not the book.

Every face is built from a few materials, each a ramp of shades lit from the top left, in
vanilla's design language: clear shades, crisp edges, a one-texel dark outline where parts
meet. Bronze-age machines are oak bound in bronze; steam and electric machines share a
riveted iron casing; high-voltage machines a darker steel casing with hazard bands.
"""
import math
import os
import random

N = 32

# Materials: shade letters, darkest first. Each face picks from these by name.
MATERIALS = {
    # Wood is HD's own oak (wood.ron's `oak_wood`) and dark oak for seams, wells and the
    # bands of machines that hold no metal, through palette slots, so machine wood matches
    # the planks it is made from.
    'wood': ['wood:0', 'wood:1', 'wood:2', 'wood:3', 'wood:4', 'wood:5'],
    'dark': ['dark:0', 'dark:1', 'dark:2', 'dark:3', 'dark:4', 'dark:5'],
    'bronze': ['#5a3410', '#8a5420', '#b87a34', '#dca456', '#f4cc80'],
    'iron': ['#2a2c32', '#464a52', '#62676f', '#80858e', '#a2a7b0', '#c8ccd4'],
    'steel': ['#1c1e24', '#30333b', '#444852', '#5a5f6a', '#767c88', '#9aa0ac'],
    'copper': ['#5a2410', '#8a3e1c', '#b8602e', '#da8a4c', '#f2b47a'],
    'brick': ['#3a1e14', '#5e3020', '#84442c', '#a85a3a', '#c8764e'],
    'glow': ['#6a1404', '#b8360a', '#e8701a', '#fcb040', '#ffe08a'],
    'water': ['#1a3a5a', '#2a5a84', '#3e7aa8', '#5e9cc8', '#8ac0e0'],
    'rubber': ['#141416', '#222226', '#34343a', '#48484e'],
    'canvas': ['#7a6a4e', '#9a8a68', '#b8a882', '#d2c4a0'],
    'leather': ['#3a2414', '#5a3a20', '#7a5230', '#9a6a40'],
    'hazard': ['#1a1a1a', '#d8a020', '#f0c040'],
    'blue': ['#142a5a', '#1e4290', '#3a66c4', '#6a96e8'],
    'green': ['#103a1a', '#1e6a2e', '#3aa04a', '#7ad87a'],
    'red': ['#4a0e0e', '#8a1e1a', '#c43a2e', '#ec6a50'],
    'glass': ['#24363e', '#3a5664', '#5e8090', '#9ab8c4'],
    # Voltage tiers: not colours but palette slots, filled from the `tier_lv`, `tier_mv`
    # and `tier_hv` palettes (default's machines.ron) so both packs code a tier alike.
    'tier': ['tier:0', 'tier:1', 'tier:2'],
    'high': ['high:0', 'high:1', 'high:2'],
    # Item sprites: the `head` slot is the metal (default's `metal_*` palettes), so one
    # drawing serves copper, iron and bronze; a dark outline and a glint as vanilla's.
    'head': ['head:0', 'head:1', 'head:2'],
    'ink': ['#1a140e'],
    'glint': ['#fff6e0'],
}
TIERS = {'lv': 'tier_lv', 'mv': 'tier_mv', 'hv': 'tier_hv'}
SLOTS = {'wood': 'oak_wood', 'dark': 'dark_oak_wood'}


class Face:
    def __init__(self, base=None):
        self.px = [[None] * N for _ in range(N)]
        self.palettes = {}
        # A face drawn over another texture (`block/<base>`): only what differs is drawn.
        self.base = base

    def tiers(self, tier, high=None):
        self.palettes['tier'] = TIERS[tier]
        if high:
            self.palettes['high'] = TIERS[high]
        return self

    def put(self, x, y, mat, shade):
        x, y = int(round(x)), int(round(y))
        if 0 <= x < N and 0 <= y < N:
            ramp = MATERIALS[mat]
            self.px[y][x] = (mat, max(0, min(len(ramp) - 1, shade)))

    def get(self, x, y):
        if 0 <= x < N and 0 <= y < N:
            return self.px[y][x]
        return None

    def rect(self, x0, y0, x1, y1, mat, shade, bevel=True):
        """A filled panel, lit top left: lighter top and left rows, darker bottom and right."""
        for y in range(y0, y1 + 1):
            for x in range(x0, x1 + 1):
                s = shade
                if bevel:
                    if y == y0 or x == x0:
                        s += 1
                    elif y == y1 or x == x1:
                        s -= 1
                self.put(x, y, mat, s)

    def outline(self, x0, y0, x1, y1, mat, shade):
        for x in range(x0, x1 + 1):
            self.put(x, y0, mat, shade)
            self.put(x, y1, mat, shade)
        for y in range(y0, y1 + 1):
            self.put(x0, y, mat, shade)
            self.put(x1, y, mat, shade)

    def disc(self, cx, cy, r, mat, shade, ring=None):
        for y in range(N):
            for x in range(N):
                dx, dy = x + 0.5 - cx, y + 0.5 - cy
                d = math.hypot(dx, dy)
                if d <= r:
                    lit = -(dx + dy) / max(r, 1)
                    s = shade + (1 if lit > 0.5 else 0) - (1 if lit < -0.5 else 0)
                    if ring is not None and d > r - 1.2:
                        s = ring
                    self.put(x, y, mat, s)

    def line(self, x0, y0, x1, y1, mat, shade, width=1):
        steps = int(max(abs(x1 - x0), abs(y1 - y0)) * 2) + 1
        for i in range(steps + 1):
            t = i / steps
            x, y = x0 + (x1 - x0) * t, y0 + (y1 - y0) * t
            for w in range(width):
                if abs(x1 - x0) > abs(y1 - y0):
                    self.put(x, y + w, mat, shade)
                else:
                    self.put(x + w, y, mat, shade)

    def streaks(self, seed, count=40, vertical=False):
        """Wood grain as HD's planks draw it: streaks 3-8 texels long along the boards, a
        shade darker or lighter, each kept to the board it starts on. Never single texels:
        the HD guide forbids per-texel grain."""
        r = random.Random(seed)
        for _ in range(count):
            x, y = r.randrange(N), r.randrange(N)
            start = self.get(x, y)
            if not start or start[0] != 'wood':
                continue
            shift = -1 if r.random() < 0.65 else 1
            for i in range(r.randint(3, 8)):
                xx, yy = (x, y + i) if vertical else (x + i, y)
                if self.get(xx, yy) != start:
                    break
                self.put(xx, yy, 'wood', start[1] + shift)

    def rivets(self, points, mat='iron'):
        for x, y in points:
            self.put(x, y, mat, 5 if mat == 'iron' else len(MATERIALS[mat]) - 1)
            self.put(x + 1, y + 1, mat, 0)

    def ink(self):
        """A one-texel dark outline round everything drawn, as vanilla's items have."""
        edge = [(x, y) for y in range(N) for x in range(N) if self.px[y][x] is None and any(
            (p := self.get(x + dx, y + dy)) is not None and p[0] != 'ink'
            for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)))]
        for x, y in edge:
            self.put(x, y, 'ink', 0)
        return self

    def recipe(self):
        """The face as a Grid recipe: one legend letter per (material, shade) used."""
        letters = iter('abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789')
        legend, rows = {}, []
        for y in range(N):
            row = ''
            for x in range(N):
                p = self.px[y][x]
                if p is None:
                    row += '.'
                    continue
                if p not in legend:
                    legend[p] = next(letters)
                row += legend[p]
            rows.append(row)
        leg = ", ".join(f"'{c}': \"{MATERIALS[m][s]}\"" for (m, s), c in legend.items())
        body = ",\n".join(f'            "{r}"' for r in rows)
        palettes = dict(self.palettes)
        palettes.update({m: SLOTS[m] for (m, _) in legend if m in SLOTS})
        pal = ''
        if palettes:
            pal = ', palettes: {' + ', '.join(f'"{k}": "{v}"' for k, v in palettes.items()) + '}'
        head = f'base: "block/{self.base}"' if self.base else 'size: (32, 32)'
        return f"({head}{pal}, layers: [Grid(legend: {{{leg}}}, rows: [\n{body},\n        ])])"


# Shared bodies ----------------------------------------------------------------------------

def iron_casing(f=None, seed=1):
    f = f or Face()
    f.rect(0, 0, 31, 31, 'iron', 2)
    f.outline(0, 0, 31, 31, 'iron', 0)
    f.rect(2, 2, 29, 29, 'iron', 3)
    f.rivets([(2, 2), (28, 2), (2, 28), (28, 28), (15, 2), (15, 28), (2, 15), (28, 15)])
    return f


def hv_casing(f=None, seed=2):
    f = f or Face()
    f.rect(0, 0, 31, 31, 'steel', 2)
    f.outline(0, 0, 31, 31, 'steel', 0)
    f.rect(2, 4, 29, 27, 'steel', 3)
    for x in range(32):
        for y in (0, 1, 2, 29, 30, 31):
            f.put(x, y, 'hazard', 0 if ((x + y) // 3) % 2 else 1)
    f.rivets([(3, 5), (27, 5), (3, 25), (27, 25)], 'steel')
    return f


def wood_frame(f=None, seed=3, band='bronze'):
    """Oak boards in a frame of `band`: bronze for the Bronze-age machines that are made with
    bronze, iron for those with iron, dark oak for those of wood alone, so bronze stays a
    Bronze-age marker."""
    f = f or Face()
    for y in range(N):
        board = y // 8
        for x in range(N):
            if y % 8 == 7:
                f.put(x, y, 'dark', 2)
            else:
                f.put(x, y, 'wood', 2 + (board % 2) + (1 if y % 8 == 0 else 0))
    f.streaks(seed, 50)
    lit, shade = {'bronze': (3, 1), 'iron': (4, 2), 'dark': (3, 1)}[band]
    for x in range(N):
        for y in (0, 1, 30, 31):
            f.put(x, y, band, lit if y in (0, 30) else shade)
    for y in range(N):
        for x in (0, 1, 30, 31):
            f.put(x, y, band, lit if x in (0, 30) else shade)
    f.rivets([(3, 3), (27, 3), (3, 27), (27, 27)], 'bronze' if band == 'bronze' else 'iron')
    return f


def kiln(f=None, seed=4):
    """Kiln bricks that tile: 8-row courses of bricks 16 wide, each course offset by 8 (default's
    4-row courses of 8-wide bricks, drawn at 32), each brick its own tone, lit along its
    top and left, shadowed along its bottom, a dark mortar line between."""
    f = f or Face()
    r = random.Random(seed)
    tone = {}
    for y in range(N):
        course = y // 8
        off = 8 if course % 2 else 0
        for x in range(N):
            u, v = (x + off) % 16, y % 8
            if v == 0 or u == 15:
                f.put(x, y, 'brick', 1)
                continue
            key = (course, ((x + off) // 16) % 2)
            t = tone.setdefault(key, r.choice((2, 3, 3)))
            f.put(x, y, 'brick', t + 1 if v == 1 or u == 0 else t - 1 if v == 7 else t)
    # A few chips, 2-3 texels, in the brick faces.
    for _ in range(6):
        x, y = r.randrange(2, 29), r.randrange(2, 30)
        if y % 8 in (3, 4, 5) and (x + (8 if (y // 8) % 2 else 0)) % 16 in range(2, 12):
            for i in range(r.randint(2, 3)):
                f.put(x + i, y, 'brick', 1)
    return f


def window(f, x0, y0, x1, y1, mat, frame='iron'):
    f.rect(x0 - 1, y0 - 1, x1 + 1, y1 + 1, frame, 1, bevel=False)
    for y in range(y0, y1 + 1):
        for x in range(x0, x1 + 1):
            t = (y - y0) / max(1, y1 - y0)
            f.put(x, y, mat, 3 - int(t * 2.5))


def grille(f, x0, y0, x1, y1, mat='iron'):
    f.rect(x0, y0, x1, y1, mat, 1, bevel=False)
    for y in range(y0 + 1, y1, 2):
        for x in range(x0 + 1, x1):
            f.put(x, y, mat, 0)
    f.outline(x0, y0, x1, y1, mat, 4)


def gauge(f, cx, cy, needle=-0.6):
    f.disc(cx, cy, 3.6, 'iron', 4, ring=1)
    f.disc(cx, cy, 2.4, 'canvas', 3)
    f.line(cx, cy, cx + 2 * math.cos(needle), cy + 2 * math.sin(needle), 'red', 2)


def coil(f, x0, y0, x1, y1, mat='copper'):
    for y in range(y0, y1 + 1):
        for x in range(x0, x1 + 1):
            f.put(x, y, mat, 3 if (y - y0) % 2 == 0 else 1)
        f.put(x0, y, mat, 0)
        f.put(x1, y, mat, 0)


def cells(f, x0, y0, n, w, h, mat, charge):
    for i in range(n):
        cx = x0 + i * (w + 2)
        f.rect(cx, y0, cx + w - 1, y0 + h - 1, 'rubber', 2)
        f.rect(cx + 1, y0 - 2, cx + w - 2, y0 - 1, mat, 3)
        for k in range(int((h - 2) * charge)):
            for x in range(cx + 1, cx + w - 1):
                f.put(x, y0 + h - 2 - k, 'green', 2 if k % 2 else 3)


def wheel(f, cx, cy, r, spokes, mat='wood', rim='bronze', paddles=False):
    # A dark well behind the wheel, so spokes read against the frame.
    f.disc(cx, cy, r - 1.5, 'dark', 0)
    for y in range(N):
        for x in range(N):
            d = math.hypot(x + 0.5 - cx, y + 0.5 - cy)
            if r - 2 <= d <= r:
                f.put(x, y, rim, 3 if (x + y) % 5 else 2)
    for k in range(spokes):
        a = k * 2 * math.pi / spokes
        f.line(cx, cy, cx + (r - 1) * math.cos(a), cy + (r - 1) * math.sin(a), mat, 4, 2)
        if paddles:
            px, py = cx + r * math.cos(a), cy + r * math.sin(a)
            f.rect(int(px) - 1, int(py) - 1, int(px) + 1, int(py) + 1, mat, 2)
    f.disc(cx, cy, 3, rim, 3, ring=1)


# Faces ----------------------------------------------------------------------------------

FACES = {}


def face(name):
    def wrap(fn):
        FACES[name] = fn
        return fn
    return wrap


@face('iron_casing')
def _():
    return iron_casing()


@face('hv_casing')
def _():
    return hv_casing()


@face('water_wheel_side')
def _():
    f = wood_frame()
    wheel(f, 16, 16, 13, 8, paddles=True)
    return f


@face('water_wheel_top')
def _():
    f = wood_frame()
    f.rect(4, 13, 27, 18, 'wood', 3)
    f.rect(4, 15, 27, 16, 'bronze', 3, bevel=False)
    return f


@face('windmill_side')
def _():
    f = wood_frame()
    f.disc(16, 16, 14.5, 'dark', 0)
    for a in range(4):
        ang = a * math.pi / 2 + math.pi / 4
        ux, uy = math.cos(ang), math.sin(ang)
        px, py = -uy, ux
        for t10 in range(30, 145):
            t = t10 / 10
            for w10 in range(0, 60):
                w = w10 / 10
                x, y = 16 + ux * t + px * w, 16 + uy * t + py * w
                # Lattice: a bar every 3 texels along the sail and down its middle.
                bar = int(t) % 3 == 0 or 2.6 < w < 3.4
                edge = w > 5.2 or t > 13.8
                f.put(x, y, 'wood' if (bar or edge) else 'canvas', 2 if edge else 3 if bar else 3 - (int(w) % 2))
        f.line(16, 16, 16 + 14.5 * ux, 16 + 14.5 * uy, 'wood', 1, 2)
    f.disc(16, 16, 3, 'bronze', 3, ring=1)
    return f


@face('windmill_top')
def _():
    f = wood_frame()
    f.disc(16, 16, 7, 'wood', 3, ring=1)
    f.disc(16, 16, 3, 'bronze', 3, ring=1)
    return f


@face('treadwheel_side')
def _():
    f = wood_frame(band='dark')
    wheel(f, 16, 16, 14, 12)
    return f


@face('treadwheel_top')
def _():
    f = wood_frame(band='dark')
    for x in range(3, 29, 3):
        f.rect(x, 4, x + 1, 27, 'wood', 3)
    return f


@face('trip_hammer_side')
def _():
    f = wood_frame()
    f.rect(5, 6, 8, 29, 'wood', 1)
    f.line(6, 7, 26, 11, 'wood', 0, 1)
    f.line(6, 8, 26, 12, 'wood', 4, 2)
    f.line(6, 10, 26, 14, 'wood', 0, 1)
    f.rect(20, 12, 27, 20, 'iron', 3)
    f.rect(17, 25, 29, 29, 'iron', 2)
    return f


@face('trip_hammer_top')
def _():
    f = wood_frame()
    f.rect(10, 6, 21, 25, 'iron', 3)
    return f


@face('kiln_bricks')
def _():
    # The furnaces are built of these (bloomery, coke oven and blast furnace below draw only
    # their openings over them), and so are default's crucible and pit kiln.
    return kiln(seed=5)


@face('bloomery_side')
def _():
    f = Face(base='kiln_bricks')
    for y in range(14, 30):
        for x in range(9, 23):
            if (x - 15.5) ** 2 / 49 + (y - 30) ** 2 / 256 <= 1:
                f.put(x, y, 'glow', 4 - int((y - 14) / 5))
    f.outline(8, 13, 23, 30, 'brick', 0)
    return f


@face('bellows_top')
def _():
    f = wood_frame(band='dark')
    f.disc(16, 16, 4, 'iron', 2, ring=0)
    return f


@face('coke_oven_side')
def _():
    f = Face(base='kiln_bricks')
    f.rect(10, 12, 21, 28, 'iron', 2)
    window(f, 12, 18, 19, 25, 'glow')
    f.rect(10, 12, 21, 14, 'iron', 4)
    return f


@face('blast_furnace_side')
def _():
    f = Face(base='kiln_bricks')
    for y in range(18, 30):
        for x in range(10, 22):
            f.put(x, y, 'glow', 4 - (y - 18) // 3)
    f.rect(8, 15, 23, 17, 'iron', 3)
    f.outline(9, 17, 22, 30, 'iron', 1)
    f.rect(13, 2, 18, 8, 'iron', 2)
    return f


@face('sawmill_side')
def _():
    f = wood_frame(band='iron')
    f.disc(16, 16, 11, 'iron', 4, ring=2)
    for k in range(24):
        a = k * 2 * math.pi / 24
        f.put(16 + 11.5 * math.cos(a), 16 + 11.5 * math.sin(a), 'iron', 5)
    f.disc(16, 16, 3, 'iron', 1)
    return f


@face('sawmill_top')
def _():
    f = wood_frame(band='iron')
    f.rect(15, 2, 16, 29, 'iron', 4, bevel=False)
    return f


@face('sawmill_bottom')
def _():
    return wood_frame(seed=8, band='iron')


@face('hopper_side')
def _():
    # Planks only (recipes_machines.ron), so oak bound in bronze like the chest: a funnel of
    # darker boards narrowing to the spout, lit along its left slope.
    f = wood_frame(seed=9, band='dark')
    for y in range(4, 28):
        half = 12 - (y - 4) // 2
        for x in range(16 - half, 16 + half):
            edge = x == 16 - half
            f.put(x, y, 'wood', 4 if edge else 1 if x < 16 else 0)
        for x in (16 - half - 1, 16 + half):
            f.put(x, y, 'wood', 0)
    f.rivets([(15, 25)])
    return f


@face('hopper_top')
def _():
    f = wood_frame(seed=10, band='dark')
    f.rect(5, 5, 26, 26, 'wood', 1, bevel=False)
    f.outline(5, 5, 26, 26, 'wood', 0)
    for i in range(5, 12):
        f.outline(i, i, 31 - i, 31 - i, 'wood', 1 if i % 2 else 0)
    f.rect(12, 12, 19, 19, 'rubber', 0, bevel=False)
    return f


@face('hopper_bottom')
def _():
    f = wood_frame(seed=11, band='dark')
    f.disc(16, 16, 5, 'wood', 1, ring=0)
    f.disc(16, 16, 3, 'rubber', 0)
    return f


@face('chest_side')
def _():
    f = wood_frame(seed=12, band='dark')
    f.rect(13, 10, 18, 17, 'iron', 4)
    f.rect(15, 13, 16, 15, 'rubber', 0, bevel=False)
    return f


@face('chest_top')
def _():
    f = wood_frame(seed=13, band='dark')
    for x in range(N):
        f.put(x, 15, 'bronze', 1)
        f.put(x, 16, 'bronze', 3)
    return f


@face('boiler_side')
def _():
    f = iron_casing(seed=14)
    for y in range(4, 22):
        for x in range(4, 28):
            f.put(x, y, 'copper', 3 if y < 6 else 1 if y > 19 else 2)
    for x in (4, 27):
        for y in range(4, 22, 3):
            f.put(x, y, 'iron', 5)
    gauge(f, 22, 9)
    grille(f, 8, 23, 23, 28)
    for x in range(9, 23):
        f.put(x, 27, 'glow', 3)
    return f


@face('boiler_top')
def _():
    f = iron_casing(seed=15)
    f.disc(16, 16, 9, 'copper', 2, ring=0)
    f.disc(16, 16, 3, 'iron', 4, ring=1)
    return f


@face('beam_engine_side')
def _():
    f = iron_casing(seed=16)
    f.rect(6, 12, 13, 27, 'iron', 4)
    f.rect(7, 8, 12, 11, 'iron', 3)
    f.line(5, 7, 27, 5, 'iron', 1, 3)
    f.disc(16, 6, 2, 'bronze', 3)
    f.rect(23, 6, 25, 20, 'iron', 2)
    wheel(f, 23, 22, 6, 6, mat='iron', rim='iron')
    return f


@face('beam_engine_top')
def _():
    f = iron_casing(seed=17)
    f.rect(4, 14, 27, 17, 'iron', 4)
    return f


@face('steam_crusher_side')
def _():
    f = iron_casing(seed=18)
    window(f, 6, 7, 25, 22, 'iron')
    for x in range(7, 25, 3):
        f.line(x, 8, x + 1, 13, 'iron', 5)
        f.line(x, 21, x + 1, 16, 'iron', 5)
    gauge(f, 24, 26)
    return f


@face('steam_crusher_top')
def _():
    f = iron_casing(seed=19)
    grille(f, 6, 6, 25, 25)
    return f


@face('dynamo_side')
def _():
    f = iron_casing(seed=20)
    coil(f, 6, 6, 25, 25)
    f.rect(14, 6, 17, 25, 'iron', 4)
    return f


@face('dynamo_top')
def _():
    f = iron_casing(seed=21)
    f.disc(16, 16, 9, 'copper', 3, ring=1)
    f.disc(16, 16, 3, 'iron', 4, ring=1)
    return f


@face('electric_furnace_side')
def _():
    f = iron_casing(seed=22)
    window(f, 8, 8, 23, 20, 'glow')
    coil(f, 6, 24, 25, 26)
    return f


@face('macerator_side')
def _():
    f = iron_casing(seed=23)
    window(f, 6, 7, 25, 23, 'iron')
    for cx in (11, 20):
        f.disc(cx, 15, 5, 'iron', 4, ring=1)
        for k in range(8):
            a = k * math.pi / 4
            f.put(cx + 5 * math.cos(a), 15 + 5 * math.sin(a), 'iron', 5)
    return f


@face('macerator_top')
def _():
    f = iron_casing(seed=24)
    grille(f, 8, 8, 23, 23)
    return f


@face('ore_washer_side')
def _():
    f = iron_casing(seed=25)
    window(f, 6, 7, 25, 23, 'water')
    for x in range(8, 24, 4):
        f.line(x, 22, x + 2, 12, 'water', 4)
    return f


@face('ore_washer_top')
def _():
    f = iron_casing(seed=26)
    f.disc(16, 16, 9, 'water', 3, ring=1)
    return f


@face('steam_turbine_side')
def _():
    f = iron_casing(seed=27)
    f.disc(16, 16, 11, 'iron', 1, ring=4)
    for k in range(12):
        a = k * 2 * math.pi / 12
        f.line(16, 16, 16 + 10 * math.cos(a), 16 + 10 * math.sin(a), 'iron', 4)
    f.disc(16, 16, 2.5, 'copper', 3)
    return f


@face('steam_turbine_top')
def _():
    f = iron_casing(seed=28)
    f.disc(16, 16, 6, 'copper', 2, ring=0)
    return f


@face('turbine_hall_side')
def _():
    f = hv_casing(seed=29)
    f.disc(16, 16, 11, 'steel', 1, ring=4)
    for k in range(16):
        a = k * 2 * math.pi / 16
        f.line(16, 16, 16 + 10 * math.cos(a), 16 + 10 * math.sin(a), 'steel', 5)
    f.disc(16, 16, 3, 'bronze', 3, ring=1)
    return f


@face('turbine_hall_top')
def _():
    f = hv_casing(seed=30)
    grille(f, 6, 6, 25, 25, 'steel')
    return f


# Cables, shafts, pipes, chutes and conveyors are full cubes until Shapes gives them thin
# models, and the opaque pass has no cutout: every texel of their faces is solid.

def cable_faces(prefix):
    # The jacket fills the face, banded in its tier's colour; HV's is armoured in dark steel.
    jacket = 'steel' if prefix == 'hv' else 'rubber'

    @face(f'{prefix}_cable_side')
    def _():
        f = Face().tiers(prefix)
        for y in range(N):
            for x in range(N):
                lit = 3 if y < 6 else 2 if y < 22 else 1
                if y in (0, 31):
                    lit = 0
                if x % 8 in (2, 3, 4) and 0 < y < 31:
                    f.put(x, y, 'tier', 2 if y < 8 else 1 if y < 24 else 0)
                else:
                    f.put(x, y, jacket, lit - (1 if x % 8 == 5 else 0))
        if jacket == 'steel':
            for x in range(N):
                if x % 8 in (6, 7):
                    for y in (1, 30):
                        f.put(x, y, 'steel', 5)
        return f

    @face(f'{prefix}_cable_end')
    def _():
        f = Face().tiers(prefix)
        f.rect(0, 0, 31, 31, jacket, 2)
        f.outline(0, 0, 31, 31, jacket, 0)
        f.disc(16, 16, 12, 'tier', 1, ring=0)
        f.disc(16, 16, 9, jacket, 1, ring=0)
        f.disc(16, 16, 6.5, 'copper', 2, ring=0)
        for cx, cy in ((13, 13), (19, 13), (16, 16), (13, 19), (19, 19), (16, 11), (16, 21), (11, 16), (21, 16)):
            f.put(cx, cy, 'copper', 4)
            f.put(cx + 1, cy + 1, 'copper', 1)
        return f


for tier in ('lv', 'mv', 'hv'):
    cable_faces(tier)


@face('shaft_side')
def _():
    f = Face()
    for y in range(N):
        for x in range(N):
            f.put(x, y, 'wood', 0 if y in (0, 31) else 4 if y < 4 else 3 if y < 12 else 2 if y < 24 else 1)
    f.streaks(9, 60)
    for x0 in (4, 25):
        f.rect(x0, 0, x0 + 2, 31, 'dark', 2)
        f.rivets([(x0 + 1, 6), (x0 + 1, 24)], 'dark')
    return f


@face('shaft_end')
def _():
    f = Face()
    for y in range(N):
        for x in range(N):
            d = math.hypot(x + 0.5 - 16, y + 0.5 - 16)
            f.put(x, y, 'wood', 3 if int(d) % 4 < 2 else 2)
    f.outline(0, 0, 31, 31, 'wood', 0)
    f.rect(11, 11, 20, 20, 'dark', 3)
    f.rect(14, 14, 17, 17, 'rubber', 0, bevel=False)
    return f


@face('bellows_side')
def _():
    f = Face()
    f.rect(0, 0, 31, 7, 'wood', 3)
    f.rect(0, 24, 31, 31, 'wood', 2)
    for y in range(8, 24):
        fold = (y - 8) % 4
        for x in range(N):
            if x < 2 or x > 29:
                f.put(x, y, 'wood', 1 if fold else 0)
            else:
                f.put(x, y, 'leather', 3 if fold == 0 else 2 if fold == 1 else 1)
    f.rivets([(3, 3), (28, 3), (3, 27), (28, 27)])
    return f


@face('chute_side')
def _():
    # A plank trough (planks only, made four at a time): boards along its length, a lit lip,
    # the dark inside showing over it, and battens nailed across.
    f = Face()
    for y in range(N):
        for x in range(N):
            if y % 7 == 6:
                f.put(x, y, 'dark', 2)
            else:
                f.put(x, y, 'wood', 2 + (y // 7) % 2)
    f.streaks(29, 50)
    f.rect(0, 0, 31, 3, 'wood', 4)
    f.rect(1, 4, 30, 9, 'dark', 0, bevel=False)
    for x0 in (3, 26):
        f.rect(x0, 10, x0 + 2, 31, 'wood', 3)
        f.rivets([(x0 + 1, 13), (x0 + 1, 27)])
    f.outline(0, 0, 31, 31, 'dark', 1)
    return f


@face('chute_end')
def _():
    # The trough seen end on: thick plank walls round a V-shaped dark channel.
    f = Face()
    for y in range(N):
        for x in range(N):
            f.put(x, y, 'dark', 2) if x % 8 == 7 else f.put(x, y, 'wood', 3 if (x // 8) % 2 else 2)
    f.streaks(30, 40, vertical=True)
    for y in range(4, 24):
        half = 11 - max(0, y - 14)
        for x in range(16 - half, 16 + half):
            f.put(x, y, 'rubber', 1 if y < 7 else 0)
        f.put(16 - half - 1, y, 'wood', 4)
        f.put(16 + half, y, 'dark', 2)
    f.outline(0, 0, 31, 31, 'dark', 1)
    f.rivets([(3, 27), (27, 27), (3, 3), (27, 3)])
    return f


@face('conveyor_side')
def _():
    f = Face()
    f.rect(0, 0, 31, 31, 'iron', 2)
    f.outline(0, 0, 31, 31, 'iron', 0)
    for x in range(N):
        for y in range(0, 5):
            f.put(x, y, 'leather', 3 if y == 0 else 2 if x % 4 < 2 else 1)
    f.rect(1, 5, 30, 18, 'iron', 1, bevel=False)
    for cx in (5, 16, 27):
        f.disc(cx, 11, 4.6, 'iron', 4, ring=2)
        f.disc(cx, 11, 1.5, 'iron', 1)
    f.rect(2, 20, 29, 29, 'iron', 3)
    f.rivets([(4, 22), (27, 22), (4, 27), (27, 27)])
    return f


@face('conveyor_end')
def _():
    f = iron_casing(seed=31)
    f.rect(1, 1, 30, 5, 'leather', 2)
    f.disc(16, 15, 6, 'iron', 4, ring=2)
    f.disc(16, 15, 2, 'iron', 1)
    return f


@face('steam_pipe_side')
def _():
    f = Face()
    for y in range(N):
        for x in range(N):
            f.put(x, y, 'copper', 0 if y in (0, 31) else 4 if y < 5 else 3 if y < 12 else 2 if y < 24 else 1)
    for x0 in (0, 28):
        f.rect(x0, 0, x0 + 3, 31, 'iron', 3)
        f.rivets([(x0 + 1, 4), (x0 + 1, 14), (x0 + 1, 25)])
    for x in range(8, 24):
        f.put(x, 7, 'copper', 4)
    return f


@face('steam_pipe_end')
def _():
    f = Face()
    f.rect(0, 0, 31, 31, 'iron', 3)
    f.outline(0, 0, 31, 31, 'iron', 0)
    f.disc(16, 16, 12, 'copper', 3, ring=1)
    f.disc(16, 16, 8, 'rubber', 0)
    f.disc(14.5, 14.5, 3, 'rubber', 1)
    f.rivets([(2, 2), (28, 2), (2, 28), (28, 28)])
    return f


def bank_faces(name, tier, casing, cell_count, charge, terminal):
    @face(f'{name}_side')
    def _():
        f = casing(seed=len(name)).tiers(tier)
        for x in range(2, 30):
            for y in (5, 26):
                f.put(x, y, 'tier', 2 if y == 5 else 1)
                f.put(x, y + 1, 'tier', 0)
        w = (24 - 2 * (cell_count - 1)) // cell_count
        x0 = 16 - (cell_count * (w + 2) - 2) // 2
        cells(f, x0, 11, cell_count, w, 14, terminal, charge)
        return f

    @face(f'{name}_top')
    def _():
        f = casing(seed=len(name) + 1).tiers(tier)
        posts = min(cell_count - 1, 3)
        spacing = 24 / posts
        r = min(3.6, spacing / 2 - 0.9)
        for i in range(posts):
            x = 16 + (i - (posts - 1) / 2) * spacing
            f.disc(x, 16, r, 'tier', 1, ring=0)
            f.disc(x, 16, r * 0.45, terminal, 3)
        return f


bank_faces('battery_bank', 'lv', iron_casing, 3, 0.35, 'copper')
bank_faces('mv_battery_bank', 'mv', iron_casing, 4, 0.6, 'copper')
bank_faces('hv_battery_bank', 'hv', hv_casing, 5, 0.85, 'bronze')


def transformer_faces(low, high, coil_mat):
    # A transformer steps `low` up to `high`: its top carries the higher tier's terminals
    # and colour, its bottom the lower's.
    @face(f'{low}_transformer_side')
    def _():
        f = iron_casing(seed=31 + len(low)).tiers(low, high)
        for x in range(2, 30):
            f.put(x, 4, 'high', 2)
            f.put(x, 5, 'high', 0)
            f.put(x, 26, 'tier', 1)
            f.put(x, 27, 'tier', 0)
        coil(f, 6, 8, 13, 23, coil_mat)
        coil(f, 18, 8, 25, 23, coil_mat)
        f.rect(6, 14, 25, 16, 'iron', 4)
        return f

    @face(f'{low}_transformer_top')
    def _():
        f = iron_casing(seed=33 + len(low)).tiers(low, high)
        for x in (10, 21):
            f.disc(x, 16, 4.5, 'high', 1, ring=0)
            f.disc(x, 16, 2.6, 'canvas', 3)
            f.disc(x, 16, 1.3, 'copper', 3)
        return f

    @face(f'{low}_transformer_bottom')
    def _():
        f = iron_casing(seed=35 + len(low)).tiers(low, high)
        f.rect(6, 12, 25, 19, 'rubber', 1)
        for x in (10, 21):
            f.disc(x, 16, 3, 'tier', 1, ring=0)
            f.disc(x, 16, 1.3, 'copper', 3)
        return f


transformer_faces('lv', 'mv', 'copper')
transformer_faces('mv', 'hv', 'bronze')


def straps(f, top=False):
    """A locked chest: the plain chest bound in two iron straps."""
    for x0 in (6, 23):
        for y in range(N):
            for x in range(x0, x0 + 3):
                f.put(x, y, 'iron', 4 if x == x0 else 1 if x == x0 + 2 else 3)
        f.rivets([(x0 + 1, 4), (x0 + 1, 26)])
        if top:
            for x in range(x0, x0 + 3):
                f.put(x, 30, 'iron', 5)


@face('treasure_chest_side')
def _():
    f = FACES['chest_side']()
    straps(f)
    f.rect(12, 9, 19, 20, 'iron', 2)
    f.outline(12, 9, 19, 20, 'iron', 0)
    f.rect(13, 10, 18, 15, 'bronze', 3)
    f.rect(15, 12, 16, 17, 'rubber', 0, bevel=False)
    return f


@face('treasure_chest_top')
def _():
    f = FACES['chest_top']()
    straps(f, top=True)
    return f



@face('processing_array_side')
def _():
    f = hv_casing(seed=37)
    for gy in range(3):
        for gx in range(3):
            x0, y0 = 6 + gx * 7, 7 + gy * 7
            f.rect(x0, y0, x0 + 5, y0 + 5, 'steel', 1)
            lit = (gx + gy) % 3
            f.rect(x0 + 1, y0 + 1, x0 + 4, y0 + 4, ('green', 'blue', 'glow')[lit], 2, bevel=False)
            f.put(x0 + 1, y0 + 1, ('green', 'blue', 'glow')[lit], 3)
    return f


@face('induction_smelter_side')
def _():
    f = hv_casing(seed=38)
    window(f, 10, 8, 21, 24, 'glow')
    coil(f, 6, 8, 8, 24)
    coil(f, 23, 8, 25, 24)
    return f


@face('induction_smelter_top')
def _():
    f = hv_casing(seed=39)
    f.disc(16, 16, 8, 'copper', 3, ring=1)
    f.disc(16, 16, 5, 'glow', 3)
    return f


# Item sprites -----------------------------------------------------------------------------
# Machines' products, drawn once in the `head` slot; default's iron dust and iron and bronze
# plates take these with their own metal's palette (`base: "block/copper_dust"`).

def item(name, metal):
    def wrap(fn):
        def draw():
            f = Face()
            f.palettes['head'] = metal
            fn(f)
            return f.ink()
        FACES[name] = draw
        return fn
    return wrap


def lit(dx, dy):
    """The head shade for a point on a lump, by where it faces: top left lit, bottom right dark."""
    t = dx + dy
    return 2 if t < -0.55 else 1 if t < 0.45 else 0


@item('copper_dust', 'metal_copper')
def _(f):
    # A heap of fine dust, its slope speckled with grains, a few spilled at its foot.
    r = random.Random(41)
    for y in range(12, 28):
        for x in range(3, 29):
            dx, dy = (x + 0.5 - 16) / 12.5, (y + 0.5 - 27) / 14.5
            if dx * dx + dy * dy <= 1 and not (y == 27 and abs(dx) > 0.8):
                s = lit(dx * 1.2, dy * 0.8 + 0.35)
                if r.random() < 0.22:
                    s = max(0, min(2, s + r.choice((-1, 1))))
                f.put(x, y, 'head', s)
    for x, y in ((2, 27), (29, 26), (27, 28), (5, 28)):
        f.put(x, y, 'head', 1)
    for x, y in ((12, 15), (10, 18), (14, 17)):
        f.put(x, y, 'glint', 0)


@item('copper_plate', 'metal_copper')
def _(f):
    # A square plate lying tilted towards us: its face, a two-texel edge below, a sheen.
    for y in range(8, 25):
        lean = (y - 8) // 2
        for x in range(13 - lean, 30 - lean):
            f.put(x, y, 'head', 1)
    for y in (25, 26):
        for x in range(4, 22):
            f.put(x, y, 'head', 0)
    for y in range(8, 27):
        lean = (y - 8) // 2
        f.put(30 - lean - (1 if y > 24 else 0), y, 'head', 0)
    for x in range(13, 30):
        f.put(x, 8, 'head', 2)
    for y in range(8, 25):
        f.put(13 - (y - 8) // 2, y, 'head', 2)
    # Light catching the face in one broad band, as on rolled sheet.
    for y in range(9, 24):
        lean = (y - 8) // 2
        for x in range(13 - lean, 30 - lean):
            u = (x - (13 - lean)) + (y - 8) * 0.9
            if 9 <= u <= 13:
                f.put(x, y, 'head', 2)
    f.put(15, 9, 'glint', 0)
    f.put(16, 9, 'glint', 0)
    f.put(14, 10, 'glint', 0)


@item('copper_wire', 'metal_copper')
def _(f):
    # A coil of wire, three turns seen at a slant, one end left loose.
    for turn in range(3):
        cx, cy = 13 + turn * 2.5, 15 + turn * 1.5
        for i in range(200):
            a = i / 200 * 2 * math.pi
            x, y = cx + 9 * math.cos(a), cy + 7 * math.sin(a)
            front = math.sin(a) > 0
            s = 2 if math.cos(a) + math.sin(a) < -0.6 else 1 if front or turn == 2 else 0
            f.put(x, y, 'head', s)
            f.put(x + 0.6, y + 0.6, 'head', 0 if s else 0)
    for i in range(7):
        f.put(27 + i * 0.5, 22 + i, 'head', 1)
        f.put(28 + i * 0.5, 22 + i, 'head', 0)
    f.put(9, 10, 'glint', 0)
    f.put(10, 9, 'glint', 0)


@item('coke', 'metal_coke')
def _(f):
    # Three porous lumps of coke, grey, dull, pitted all over.
    r = random.Random(43)
    for cx, cy, rx, ry in ((11, 20, 8, 7), (21, 21, 8, 6.5), (16, 13, 7, 6)):
        for y in range(N):
            for x in range(N):
                dx, dy = (x + 0.5 - cx) / rx, (y + 0.5 - cy) / ry
                d = dx * dx + dy * dy
                if d <= 1 - 0.12 * math.sin(7 * math.atan2(dy, dx) + cx):
                    f.put(x, y, 'head', lit(dx, dy))
    for _ in range(60):
        x, y = r.randrange(4, 29), r.randrange(7, 29)
        p = f.get(x, y)
        if p and p[0] == 'head' and f.get(x + 1, y) and f.get(x, y + 1):
            f.put(x, y, 'ink', 0)
            if r.random() < 0.5:
                f.put(x + 1, y, 'head', 0)
            if f.get(x - 1, y - 1) and f.get(x - 1, y - 1)[0] == 'head':
                f.put(x - 1, y - 1, 'head', 2)


@item('iron_bloom', 'metal_bloom')
def _(f):
    # A spongy bloom of iron fresh from the bloomery: a rounded lump full of slag pits, its
    # metal showing bright where the hammer has struck it.
    r = random.Random(44)
    for y in range(N):
        for x in range(N):
            dx, dy = (x + 0.5 - 16) / 12, (y + 0.5 - 18) / 10
            a = math.atan2(dy, dx)
            if dx * dx + dy * dy <= 1 - 0.1 * math.sin(5 * a) - 0.06 * math.sin(11 * a + 1):
                f.put(x, y, 'head', lit(dx, dy))
    for _ in range(34):
        x, y = r.randrange(5, 28), r.randrange(9, 28)
        p = f.get(x, y)
        if p and p[0] == 'head' and p[1] > 0:
            f.put(x, y, 'head', 0)
            if r.random() < 0.4:
                f.put(x + 1, y, 'ink', 0)
    for x, y in ((10, 12), (11, 12), (12, 11), (18, 14), (19, 14), (9, 17)):
        f.put(x, y, 'glint', 0)


def main():
    root = os.path.join(os.path.dirname(__file__), '..', '..')
    out = ['''// Machine faces and products at 32×32 (content/base/blocks_machines.ron), drawn by
// tools/art/machines.py in vanilla's design language from a few materials: oak bound in
// bronze for the Bronze age, a riveted iron casing for steam and electric machines, darker
// steel with hazard bands for high voltage. Regenerate with `python3 tools/art/machines.py`; edit the
// Python, not this file.
(
    textures: {''']
    for name, fn in sorted(FACES.items()):
        out.append(f'        "block/{name}": {fn().recipe()},')
    out.append('    },\n)\n')
    path = os.path.join(root, 'assets/packs/voxmar_hd/recipes/machines.ron')
    open(path, 'w').write("\n".join(out))
    print(len(FACES), 'textures ->', path)


if __name__ == '__main__':
    main()
