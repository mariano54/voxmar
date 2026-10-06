"""Cave flora and the supernatural plants at 32×32, from the same shapes as wild.py with a
few of their own (glowing buds, crystal petals, a lotus), each in its own colours. Glowing
ones keep their glow to a few small points, so they read as strange rather than bright."""
import math, random, sys, os
sys.path.insert(0, os.path.dirname(__file__))
from kit import *
from wild import root_veg, vine_fruit, grain, bush, herb, mushroom, moss_carpet, tuft_grass, horsetail

CAVE = {}
SUPER = {}


def glints(fn, n=6, seed=0, area=(4, 2, 28, 16)):
    """Faint glowing motes over a plant."""
    def f():
        c = fn(); r = random.Random(seed)
        for i in range(n):
            x, y = r.uniform(area[0], area[2]), r.uniform(area[1], area[3])
            if c.get(int(x), int(y)) is None: c.put(x, y, 'G', 2 if i % 3 == 0 else 1)
        return c
    return f


def glow_buds(stems=3, seed=0):
    """Thin dark stems ending in small glowing buds (lumenwort)."""
    def f():
        c = Canvas(); r = random.Random(seed)
        for a in (-150, -120, -60, -30): blade_leaf(c, 16, 30, a, 7, 2.4, 3, droop=0.3)
        for s in range(stems):
            x = 16 + (s - (stems - 1) / 2) * 6 + r.uniform(-1, 1); top = r.uniform(5, 11)
            pts = curve(16, 31, x, top, r.uniform(-3, 3), 8); stem(c, pts, 2)
            tx, ty = pts[-1]
            c.put(tx, ty, 'G', 2); c.put(tx - 1, ty, 'G', 1); c.put(tx + 1, ty, 'G', 1); c.put(tx, ty - 1, 'G', 1); c.put(tx, ty + 1, 'G', 0)
            for k in (3, 5):
                px, py = pts[k]; c.put(px + (1 if k == 3 else -1), py, 'G', 1)
        rim(c); return c
    return f


def bell_flower(bells=4, seed=0):
    """A dark arching stem hung with glowing bells (echo bloom)."""
    def f():
        c = Canvas(); r = random.Random(seed)
        for a in (-140, -40): blade_leaf(c, 16, 30, a, 8, 2.6, 3, droop=0.4 if a > -90 else -0.4)
        stem(c, [(16, 31), (17, 24), (19, 17), (20, 11), (19, 7), (17, 5)], 2)
        for (sx, sy, bx, by) in [(17, 5, 12, 9), (19, 9, 14, 15), (20, 13, 24, 18), (19, 17, 15, 22)][:bells]:
            line(c, sx, sy, bx + 1, by - 3, 'L', 2); bell(c, bx, by, 2, 'P', 2)
            c.put(bx, by + 2, 'G', 2)
        rim(c); return c
    return f


def crystal_lily():
    """A lily of faceted crystal petals around a glowing heart (prism lily)."""
    def f():
        c = Canvas()
        for a in (-150, -120, -60, -30): blade_leaf(c, 16, 30, a, 8, 2.4, 3, droop=0.3 if a > -90 else -0.3)
        stem(c, curve(16, 31, 16, 15, 0), 2)
        cx, cy = 16, 11
        for a, l in [(-90, 9), (-130, 8), (-50, 8), (-165, 7), (-15, 7), (160, 5), (20, 5)]:
            ra = math.radians(a); dx, dy = math.cos(ra), math.sin(ra)
            for i in range(l):
                w = max(0, int((1 - abs(i / l - 0.45) * 1.8) * 2))
                for o in range(-w, w + 1):
                    x = cx + dx * i - dy * o; y = cy + dy * i + dx * o
                    c.put(x, y, 'P', (3 if o < 0 else 1) + (1 if i > l - 3 else 0))
        c.put(cx, cy, 'G', 2); c.put(cx - 1, cy, 'G', 1); c.put(cx + 1, cy, 'G', 1); c.put(cx, cy + 1, 'G', 1)
        rim(c); return c
    return f


def lotus():
    """A water lotus: layered pointed petals over two round pads."""
    def f():
        c = Canvas()
        ellipse(c, 9, 28, 7, 2.6, 'L', 3); ellipse(c, 23, 29, 6, 2.2, 'L', 3)
        for x in (9, 23): line(c, x, 27, x + 3, 29, 'L', 1)
        stem(c, [(16, 31), (16, 20)], 2)
        cx, cy = 16, 18
        for ring, (n, l, w, s) in enumerate([(8, 8, 3.2, 1), (6, 6, 3.0, 2), (4, 4, 2.6, 3)]):
            for i in range(n):
                a = -180 + (i + 0.5) * 180 / n
                petal(c, cx, cy, a, l, w, 'P', s + (1 if a < -110 else 0), tip_round=False)
        c.put(cx, cy - 2, 'G', 2)
        rim(c); return c
    return f


def kelp():
    def f():
        c = Canvas(); r = random.Random(3)
        for x0, xt in [(11, 7), (16, 15), (21, 25)]:
            pts = curve(x0, 30, xt, 3, r.uniform(-4, 4), 10)
            for (ax, ay), (bx, by) in zip(pts, pts[1:]):
                line(c, ax, ay, bx, by, 'L', 3); line(c, ax + 1, ay, bx + 1, by, 'L', 2)
            for k in (3, 6, 8):
                x, y = pts[k]; berry(c, x + 2, y, 'F', 1)
        ellipse(c, 16, 31, 9, 1.5, 'W', 2)
        rim(c); return c
    return f


def fruit_shrub(fruit_shape='pepper', seed=0):
    """A small bush hung with peppers (ember pepper) or round fruit."""
    def f():
        c = Canvas(); r = random.Random(seed)
        twigs(c, 16, 31, 4, 8, 30, 1, seed)
        foliage(c, 16, 14, 11, 8, 3, seed=seed, leaf_len=3.0)
        for x, y in [(10, 17), (15, 20), (21, 18), (18, 13), (12, 12)]:
            for k in range(5): c.put(x, y + k, 'F', 2 + (1 if k < 2 else 0)); c.put(x + 1, y + k, 'F', 1)
            c.put(x, y - 1, 'L', 2); c.put(x, y + 1, 'G', 1)
        rim(c); return c
    return f


def silver_herb():
    return glints(herb(4, 4.4, 3.0, 3, 'leaf', 9), 5, 9)


# --- cave ---
CAVE['pale_cap'] = (mushroom('dome', 2, seed=21), dict(petal='#d8d4ca', centre='#efece4', leaf='#5a6a5a'))
CAVE['ember_cap'] = (glints(mushroom('dome', 2, spots=True, seed=22), 3, 22, (4, 10, 28, 22)), dict(petal='#e0581a', centre='#6a4030', second='#ffd060', leaf='#3a2a22', glow='#ffb040'))
CAVE['rimecap'] = (mushroom('cone', 3, seed=23), dict(petal='#9ac4e0', centre='#e4f2fa', fruit='#4a7aa0', leaf='#5a6a7a'))
CAVE['cavern_moss'] = (moss_carpet(24), dict(leaf='#2e6a5a'))
CAVE['lumenwort'] = (glow_buds(3, 25), dict(leaf='#3a6a52', glow='#7af0d0'))
CAVE['paleroot'] = (root_veg('round', 5, seed=26), dict(leaf='#8a9a86', petal='#e6dcc8', wood='#7a6a5a'))
CAVE['echo_bloom'] = (bell_flower(4, 27), dict(leaf='#1e4450', petal='#2ab0c0', glow='#9cfff4'))
CAVE['prism_lily'] = (crystal_lily(), dict(leaf='#3e5a70', petal='#a690e8', glow='#e8f4ff'))

# --- supernatural ---
SUPER['starwheat'] = (glints(grain('awned', 5, 31), 7, 31), dict(leaf='#7a8a5a', wood='#e8d8a0', glow='#e8f0ff'))
SUPER['moonmelon'] = (glints(vine_fruit('round', 32), 5, 32), dict(leaf='#4a7a7a', fruit='#a8c0e8', wood='#5a6a6a', glow='#e8f0ff'))
SUPER['ember_pepper'] = (fruit_shrub('pepper', 33), dict(leaf='#4a3a2a', fruit='#f06018', wood='#2a1e18', glow='#ffd060'))
SUPER['frost_turnip'] = (root_veg('round', 5, seed=34), dict(leaf='#5a8a90', petal='#b8dcf4', wood='#6a8090'))
SUPER['dreamberry_bush'] = (glints(bush(14, 1, 1, seed=35), 6, 35), dict(leaf='#2e3e66', fruit='#b050e0', wood='#3a2e4a', glow='#e0c8ff'))
SUPER['whisper_oats'] = (grain('panicle', 5, 36), dict(leaf='#7a8a90', wood='#dce4ea'))
SUPER['sky_lotus'] = (lotus(), dict(leaf='#2a7a72', petal='#b8c8f4', glow='#e8f4ff'))
SUPER['bloodroot'] = (root_veg('long', 4, seed=38), dict(leaf='#2a5a2a', petal='#a01a28', wood='#5a1a1a'))
SUPER['sunpetal'] = (glints(lambda: __import__('flowers').F['sunflower'][0](), 6, 39), dict(leaf='#4a8a2a', petal='#ffcc30', centre='#c07018', glow='#fff0a0'))
SUPER['crystal_moss'] = (moss_carpet(40, sparkle=True), dict(leaf='#7a7ad0', glow='#e8ecff'))
SUPER['aether_truffle'] = (glints(mushroom('ball', 2, seed=41), 4, 41, (4, 16, 28, 28)), dict(petal='#5a3480', leaf='#4a7a2a', glow='#e0c8ff'))
SUPER['ghostcap'] = (mushroom('cone', 3, seed=42), dict(petal='#d8eaf2', centre='#f2fafc', fruit='#9ab4c4', leaf='#6a7a80'))
SUPER['thunderbloom'] = (glints(lambda: __import__('flowers').F['cornflower'][0](), 5, 43), dict(leaf='#3e6a6a', petal='#5a8af0', centre='#fff060', glow='#fff4a0'))
SUPER['tidekelp'] = (kelp(), dict(leaf='#2a7a5a', fruit='#8ab83a', wood='#c4ac80'))
SUPER['wyrmroot'] = (root_veg('long', 4, frilly=True, seed=45), dict(leaf='#8a9a7a', petal='#d8a858', wood='#8a6a3a'))
SUPER['hearthgrain'] = (grain('awned', 5, 46), dict(leaf='#7a7a3a', wood='#e07028'))
SUPER['honeydew_lily'] = (glints(lambda: __import__('flowers').F['red_tulip'][0](), 4, 47), dict(leaf='#3e7a4a', petal='#f8c030', glow='#fff0b0'))
SUPER['lantern_gourd'] = (glints(vine_fruit('round', 48), 3, 48, (8, 18, 24, 30)), dict(leaf='#4a7a3a', fruit='#f09030', wood='#5a4a2a', glow='#ffe0a0'))
SUPER['silvermint'] = (silver_herb(), dict(leaf='#a8bcc8', glow='#f0f8ff'))
SUPER['cinder_moss'] = (moss_carpet(50, sparkle=True), dict(leaf='#4a3428', glow='#ff9a40'))


# --- mushroom caps for the Cap model: a flat cap seen from above, and the stem with the
# cap's edge seen from the side. The cap quad lies at 0.6 of the plant's height and spans
# 0.76 of the cell, so the side sprite's cap edge sits at row 12 and is 24 texels wide.
def cap_top(spots=0, ring=False, seed=0):
    def f():
        c = Canvas(); r = random.Random(seed)
        ellipse(c, 15.5, 15.5, 15, 15, 'P', 2)
        if ring:
            for a in range(0, 360, 12):
                ra = math.radians(a); c.put(15.5 + math.cos(ra) * 9, 15.5 + math.sin(ra) * 9, 'P', 1)
        for i in range(spots):
            a = r.uniform(0, 6.28); d = r.uniform(3, 12)
            x, y = 15.5 + math.cos(a) * d, 15.5 + math.sin(a) * d
            ellipse(c, x, y, 1.4, 1.4, 'S', 3, shade=False) if False else [c.put(x + dx, y + dy, 'S', 4 - (dx + dy > 0)) for dx in (0, 1) for dy in (0, 1)]
        disc(c, 15.5, 15.5, 2.5, 'P', 3, rim=False)
        rim(c); return c
    return f


def cap_side(seed=0):
    def f():
        c = Canvas()
        for y in range(13, 32):
            c.put(15, y, 'C', 2); c.put(16, y, 'C', 1); c.put(17, y, 'C', 0)
            if y > 27: c.put(14, y, 'C', 2); c.put(18, y, 'C', 0)
        ellipse(c, 16, 12, 12, 2.2, 'P', 2)
        for x in range(6, 27, 2): c.put(x, 14, 'C', 0)
        for x in (3, 28): c.put(x, 31, 'L', 3); c.put(x, 30, 'L', 4)
        rim(c); return c
    return f


CAPS = {
    'brown_mushroom': (0, False, dict(petal='#9a6a48', centre='#e0d4b8', leaf='#4a7a2a')),
    'red_mushroom': (9, False, dict(petal='#c82a1e', centre='#e0d4b8', second='#ffffff', leaf='#4a7a2a')),
    'porcini': (0, False, dict(petal='#7a4a28', centre='#e0d4b0', leaf='#4a7a2a')),
    'deathcap': (0, True, dict(petal='#c8d0a0', centre='#f0f0e0', leaf='#4a7a2a')),
    'blewit': (0, True, dict(petal='#7a68b0', centre='#d0c8e0', leaf='#4a7a2a')),
    'shiitake': (6, False, dict(petal='#5a3a26', centre='#e8dcc8', second='#d8c8b0', leaf='#4a7a2a')),
    'fly_agaric': (11, False, dict(petal='#e0281a', centre='#f0eadc', second='#ffffff', leaf='#4a7a2a')),
    'chanterelle': (0, True, dict(petal='#f0a828', centre='#e8b84a', leaf='#4a7a2a')),
    'pale_cap': (0, True, dict(petal='#d8d4ca', centre='#efece4', leaf='#5a6a5a')),
    'ember_cap': (8, False, dict(petal='#e0581a', centre='#6a4030', second='#ffd060', leaf='#3a2a22')),
}
CAP_ART = {}
for name, (spots, ring, colours) in CAPS.items():
    CAP_ART[f'{name}_cap'] = (cap_top(spots, ring, len(name)), colours)
    CAP_ART[f'{name}_stem'] = (cap_side(len(name)), colours)
