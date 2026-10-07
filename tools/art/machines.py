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
    'wood': ['#3e2a16', '#5a3e22', '#76552e', '#93703c', '#ad884c'],
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
}


class Face:
    def __init__(self):
        self.px = [[None] * N for _ in range(N)]

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

    def grain(self, mat, seed, chance=0.12, shift=-1):
        """Darker or lighter texels scattered over what is already `mat`."""
        r = random.Random(seed)
        for y in range(N):
            for x in range(N):
                p = self.px[y][x]
                if p and p[0] == mat and r.random() < chance:
                    self.put(x, y, mat, p[1] + (shift if r.random() < 0.7 else -shift))

    def rivets(self, points, mat='iron'):
        for x, y in points:
            self.put(x, y, mat, 5 if mat == 'iron' else len(MATERIALS[mat]) - 1)
            self.put(x + 1, y + 1, mat, 0)

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
        return f"(size: (32, 32), layers: [Grid(legend: {{{leg}}}, rows: [\n{body},\n        ])])"


# Shared bodies ----------------------------------------------------------------------------

def iron_casing(f=None, seed=1):
    f = f or Face()
    f.rect(0, 0, 31, 31, 'iron', 2)
    f.outline(0, 0, 31, 31, 'iron', 0)
    f.rect(2, 2, 29, 29, 'iron', 3)
    f.grain('iron', seed, 0.05)
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
    f.grain('steel', seed, 0.05)
    f.rivets([(3, 5), (27, 5), (3, 25), (27, 25)], 'steel')
    return f


def wood_frame(f=None, seed=3):
    f = f or Face()
    for y in range(N):
        board = y // 8
        for x in range(N):
            s = 2 + (board % 2) - (1 if y % 8 == 7 else 0) + (1 if y % 8 == 0 else 0)
            f.put(x, y, 'wood', s)
    f.grain('wood', seed, 0.15)
    for x in range(N):
        for y in (0, 1, 30, 31):
            f.put(x, y, 'bronze', 3 if y in (0, 30) else 1)
    for y in range(N):
        for x in (0, 1, 30, 31):
            f.put(x, y, 'bronze', 3 if x in (0, 30) else 1)
    f.rivets([(3, 3), (27, 3), (3, 27), (27, 27)], 'bronze')
    return f


def kiln(f=None, seed=4):
    f = f or Face()
    for y in range(N):
        course = y // 6
        off = 4 if course % 2 else 0
        for x in range(N):
            mortar = y % 6 == 5 or (x + off) % 12 == 11
            if mortar:
                f.put(x, y, 'brick', 0)
            else:
                v = random.Random(seed * 97 + course * 13 + (x + off) // 12).randint(2, 3)
                f.put(x, y, 'brick', v + (1 if y % 6 == 0 else 0))
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
    f.disc(cx, cy, r - 1.5, mat, 0)
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


def cable(f, mat, stripe, vertical=False):
    for y in range(N):
        for x in range(N):
            u, v = (y, x) if vertical else (x, y)
            if 10 <= v <= 21:
                s = 3 if v in (11, 12) else 0 if v in (20, 21) else 2
                f.put(x, y, mat, s)
                if stripe and v in (15, 16) and (u // 4) % 2 == 0:
                    f.put(x, y, stripe, len(MATERIALS[stripe]) - 1)


def cable_end(f, mat, core='copper'):
    f.disc(16, 16, 6.5, mat, 2, ring=0)
    f.disc(16, 16, 3, core, 3)


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
    f.disc(16, 16, 14.5, 'wood', 0)
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
    f = wood_frame()
    wheel(f, 16, 16, 14, 12)
    return f


@face('treadwheel_top')
def _():
    f = wood_frame()
    for x in range(3, 29, 3):
        f.rect(x, 4, x + 1, 27, 'wood', 3)
    return f


@face('shaft_side')
def _():
    f = Face()
    for y in range(10, 22):
        for x in range(N):
            f.put(x, y, 'wood', 3 if y < 13 else 1 if y > 19 else 2)
    f.grain('wood', 9, 0.15)
    for x0 in (4, 26):
        f.rect(x0, 9, x0 + 2, 22, 'bronze', 3)
    return f


@face('shaft_end')
def _():
    f = Face()
    f.disc(16, 16, 6.5, 'wood', 3, ring=1)
    f.rect(14, 14, 17, 17, 'bronze', 2)
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


@face('bloomery_side')
def _():
    f = kiln(seed=5)
    for y in range(14, 30):
        for x in range(9, 23):
            if (x - 15.5) ** 2 / 49 + (y - 30) ** 2 / 256 <= 1:
                f.put(x, y, 'glow', 4 - int((y - 14) / 5))
    f.outline(8, 13, 23, 30, 'brick', 0)
    return f


@face('bellows_side')
def _():
    f = Face()
    f.rect(0, 0, 31, 7, 'wood', 3)
    f.rect(0, 24, 31, 31, 'wood', 2)
    for y in range(8, 24):
        fold = (y - 8) % 4
        for x in range(2, 30):
            f.put(x, y, 'leather', 3 if fold == 0 else 2 if fold == 1 else 1)
    f.rivets([(3, 3), (28, 3), (3, 27), (28, 27)], 'bronze')
    return f


@face('bellows_top')
def _():
    f = wood_frame()
    f.disc(16, 16, 4, 'iron', 2, ring=0)
    return f


@face('coke_oven_side')
def _():
    f = kiln(seed=6)
    f.rect(10, 12, 21, 28, 'iron', 2)
    window(f, 12, 18, 19, 25, 'glow')
    f.rect(10, 12, 21, 14, 'iron', 4)
    return f


@face('blast_furnace_side')
def _():
    f = kiln(seed=7)
    for y in range(18, 30):
        for x in range(10, 22):
            f.put(x, y, 'glow', 4 - (y - 18) // 3)
    f.rect(8, 15, 23, 17, 'iron', 3)
    f.outline(9, 17, 22, 30, 'iron', 1)
    f.rect(13, 2, 18, 8, 'iron', 2)
    return f


@face('sawmill_side')
def _():
    f = wood_frame()
    f.disc(16, 16, 11, 'iron', 4, ring=2)
    for k in range(24):
        a = k * 2 * math.pi / 24
        f.put(16 + 11.5 * math.cos(a), 16 + 11.5 * math.sin(a), 'iron', 5)
    f.disc(16, 16, 3, 'iron', 1)
    return f


@face('sawmill_top')
def _():
    f = wood_frame()
    f.rect(15, 2, 16, 29, 'iron', 4, bevel=False)
    return f


@face('sawmill_bottom')
def _():
    return wood_frame(seed=8)


@face('hopper_side')
def _():
    f = iron_casing(seed=9)
    for y in range(4, 28):
        half = 12 - (y - 4) // 2
        for x in range(16 - half, 16 + half):
            f.put(x, y, 'iron', 2 if x < 16 else 1)
    return f


@face('hopper_top')
def _():
    f = iron_casing(seed=10)
    f.rect(5, 5, 26, 26, 'iron', 0, bevel=False)
    f.rect(12, 12, 19, 19, 'rubber', 0, bevel=False)
    return f


@face('hopper_bottom')
def _():
    f = iron_casing(seed=11)
    f.disc(16, 16, 4, 'rubber', 0)
    return f


@face('chest_side')
def _():
    f = wood_frame(seed=12)
    f.rect(13, 10, 18, 17, 'iron', 4)
    f.rect(15, 13, 16, 15, 'rubber', 0, bevel=False)
    return f


@face('chest_top')
def _():
    f = wood_frame(seed=13)
    for x in range(N):
        f.put(x, 15, 'bronze', 1)
        f.put(x, 16, 'bronze', 3)
    return f


@face('chute_side')
def _():
    f = Face()
    for y in range(6, 26):
        for x in range(N):
            f.put(x, y, 'iron', 4 if y < 9 else 1 if y > 22 else 3)
    for x0 in (3, 27):
        f.rect(x0, 5, x0 + 1, 26, 'iron', 4)
    return f


@face('chute_end')
def _():
    f = Face()
    f.disc(16, 16, 10, 'iron', 3, ring=1)
    f.disc(16, 16, 7, 'rubber', 0)
    return f


@face('conveyor_side')
def _():
    f = Face()
    f.rect(0, 10, 31, 21, 'iron', 2)
    for x in range(N):
        f.put(x, 10, 'rubber', 2 if x % 4 < 2 else 3)
        f.put(x, 11, 'rubber', 1)
    for cx in (5, 16, 27):
        f.disc(cx, 17, 3, 'iron', 4, ring=1)
    return f


@face('conveyor_end')
def _():
    f = Face()
    f.rect(2, 10, 29, 21, 'iron', 2)
    f.rect(2, 10, 29, 12, 'rubber', 2)
    f.disc(16, 17, 3, 'iron', 4, ring=1)
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


@face('steam_pipe_side')
def _():
    f = Face()
    for y in range(10, 22):
        for x in range(N):
            f.put(x, y, 'copper', 4 if y < 12 else 1 if y > 19 else 2)
    for x0 in (2, 28):
        f.rect(x0, 8, x0 + 1, 23, 'iron', 4)
    return f


@face('steam_pipe_end')
def _():
    f = Face()
    f.disc(16, 16, 8, 'iron', 4, ring=1)
    f.disc(16, 16, 5.5, 'copper', 2, ring=0)
    f.disc(16, 16, 3, 'rubber', 0)
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


def cable_faces(prefix, mat, stripe):
    @face(f'{prefix}_cable_side')
    def _():
        f = Face()
        cable(f, mat, stripe)
        return f

    @face(f'{prefix}_cable_end')
    def _():
        f = Face()
        cable_end(f, mat)
        return f


cable_faces('lv', 'rubber', 'red')
cable_faces('mv', 'rubber', 'blue')
cable_faces('hv', 'rubber', 'hazard')


def bank_faces(prefix, terminal, charge):
    @face(f'{prefix}_side')
    def _():
        f = iron_casing(seed=len(prefix))
        cells(f, 5, 9, 4, 4, 17, terminal, charge)
        return f

    @face(f'{prefix}_top')
    def _():
        f = iron_casing(seed=len(prefix) + 1)
        for x in (8, 15, 22):
            f.disc(x + 1, 16, 2.5, terminal, 3, ring=1)
        return f


bank_faces('battery_bank', 'copper', 0.4)
bank_faces('mv_battery_bank', 'iron', 0.6)
bank_faces('hv_battery_bank', 'bronze', 0.85)


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


def transformer_faces(prefix, frame, coil_mat):
    @face(f'{prefix}_transformer_side')
    def _():
        f = frame(seed=31 + len(prefix))
        coil(f, 6, 8, 13, 25, coil_mat)
        coil(f, 18, 8, 25, 25, coil_mat)
        f.rect(6, 15, 25, 17, 'iron', 4)
        return f

    @face(f'{prefix}_transformer_top')
    def _():
        f = frame(seed=33 + len(prefix))
        for x in (10, 21):
            f.disc(x, 16, 3.5, 'canvas', 3, ring=1)
            f.disc(x, 16, 1.5, 'copper', 3)
        return f

    @face(f'{prefix}_transformer_bottom')
    def _():
        return frame(seed=35 + len(prefix))


transformer_faces('lv', iron_casing, 'copper')
transformer_faces('mv', iron_casing, 'bronze')


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


def main():
    root = os.path.join(os.path.dirname(__file__), '..', '..')
    out = ['''// Machine faces at 32×32 (content/base/blocks_machines.ron), drawn by tools/art/machines.py
// in vanilla's design language from a few materials: oak bound in bronze for the Bronze
// age, a riveted iron casing for steam and electric machines, darker steel with hazard
// bands for high voltage. Regenerate with `python3 tools/art/machines.py`; edit the
// Python, not this file.
(
    textures: {''']
    for name, fn in sorted(FACES.items()):
        out.append(f'        "block/{name}": {fn().recipe()},')
    out.append('    },\n)\n')
    path = os.path.join(root, 'assets/packs/voxmar_hd/recipes/machines.ron')
    open(path, 'w').write("\n".join(out))
    print(len(FACES), 'faces ->', path)


if __name__ == '__main__':
    main()
