"""The wild plants (vegetables, grains, berries, herbs, grasses, ferns, fungi, shrubs), drawn
from a handful of shapes in each plant's own colours. Colours are key colours: emit.py builds
a hue-shifted ramp around each."""
import math, random, sys, os
sys.path.insert(0, os.path.dirname(__file__))
from kit import *

W = {}  # name -> (draw function, {slot: key colour})


def add(name, fn, **colours):
    W[name] = (fn, colours)


def at(c, x, y=31, bend=0, top=10, sh=2):
    stem(c, curve(x, y, x + bend, top, bend * 0.4), sh)


# --- shapes -----------------------------------------------------------------

def root_veg(kind='round', tops=5, frilly=False, seed=0):
    """A leafy top over a root half out of the ground: round (turnip, beet, radish), long
    (parsnip, carrot) or a bulb (onion, leek)."""
    def f():
        c = Canvas(); r = random.Random(seed)
        if kind == 'bulb':
            for i in range(tops):
                x = 13 + i * 1.5; a = -90 + (i - tops / 2) * 9
                blade_leaf(c, x, 24, a, r.uniform(13, 18), 1.6, 3, droop=(i - tops / 2) * 0.12)
            ellipse(c, 16, 26, 4, 4, 'P', 2); line(c, 16, 22, 16, 29, 'P', 3)
            for x in (13, 16, 19): c.put(x, 31, 'W', 1)
        else:
            for i in range(tops):
                a = -90 + (i - (tops - 1) / 2) * 22
                if frilly: feather_leaf(c, 16, 22, a, r.uniform(10, 13), 3)
                else: blade_leaf(c, 16, 22, a, r.uniform(9, 12), 3.4, 3, droop=(i - (tops - 1) / 2) * 0.25)
            if kind == 'round':
                ellipse(c, 16, 26, 4.2, 3.6, 'P', 2); c.put(16, 31, 'P', 1); c.put(16, 30, 'P', 1)
            else:
                for y in range(22, 32):
                    w = max(0.6, 3.2 * (1 - (y - 22) / 11))
                    for x in range(int(16 - w), int(16 + w) + 1):
                        c.put(x, y, 'P', 2 + (1 if x < 16 else 0) - (1 if x > 16 + w / 2 else 0))
        rim(c); return c
    return f


def vine_fruit(shape='round', seed=0):
    """Broad lobed leaves on a creeping vine, with one big ribbed fruit (pumpkin, melon)."""
    def f():
        c = Canvas(); r = random.Random(seed)
        stem(c, [(2, 24), (8, 21), (14, 22), (22, 19), (29, 21)], 2)
        for x, y, a in [(6, 21, -120), (13, 19, -70), (21, 17, -110), (27, 19, -60), (10, 15, -95)]:
            ellipse(c, x + math.cos(math.radians(a)) * 3, y + math.sin(math.radians(a)) * 3, 3.6, 3.0, 'L', 3, jag=0.4, seed=x)
            line(c, x, y, x + math.cos(math.radians(a)) * 3, y + math.sin(math.radians(a)) * 3, 'L', 1)
        for x in (5, 26): c.put(x, 18, 'L', 4); c.put(x + 1, 17, 'L', 4)
        if shape == 'long':
            ellipse(c, 16, 26, 9, 4.2, 'F', 2)
            for x in (10, 14, 18, 22): line(c, x, 23, x + 1, 29, 'F', 1)
        else:
            ellipse(c, 16, 25, 8.5, 5.6, 'F', 2)
            for x in (11, 14, 18, 21): line(c, x, 20.5, x + (x - 16) * 0.3, 29.5, 'F', 1)
            line(c, 16, 18, 17, 20, 'W', 1); c.put(17, 17, 'W', 2)
        rim(c); return c
    return f


def big_leaf(root=True, seed=0):
    """Heart-shaped leaves on tall stalks (taro, yam, ginger), the root showing."""
    def f():
        c = Canvas(); r = random.Random(seed)
        for x0, xt, yt, w in [(15, 8, 9, 6), (17, 23, 7, 6.5), (16, 15, 4, 5.5), (16, 21, 15, 4.5)]:
            stem(c, curve(x0, 28, xt, yt + 3, 0), 2)
            for y in range(int(yt - w), int(yt + w * 0.9)):
                t = (y - (yt - w)) / (w * 1.9)
                half = w * math.sin(math.pi * min(1, t * 1.1)) * (1.0 if t > 0.25 else 0.8)
                for x in range(int(xt - half), int(xt + half) + 1):
                    if t < 0.12 and abs(x - xt) < 1: continue
                    u = (x - xt) / max(half, 0.5)
                    c.put(x, y, 'L', 3 + (1 if u < -0.3 else 0) - (1 if u > 0.55 else 0))
            line(c, xt, yt - w + 1, xt, yt + w * 0.8, 'L', 2)
        if root:
            ellipse(c, 16, 29, 5, 2.4, 'P', 2)
        rim(c); return c
    return f


def grain(head='awned', stalks=5, seed=0):
    """Grass stalks with grain heads: awned spikes (barley, wheat), drooping panicles (oats,
    millet, rice) or dense clusters (quinoa)."""
    def f():
        c = Canvas(); r = random.Random(seed)
        for i in range(stalks):
            x = 8 + i * 16 / max(stalks - 1, 1) + r.uniform(-1, 1); top = r.uniform(3, 8); lean = r.uniform(-3, 3)
            pts = curve(16 + (x - 16) * 0.3, 31, x + lean, top + 8, lean * 0.3, 6)
            stem(c, pts, 2)
            blade_leaf(c, pts[2][0], pts[2][1], -90 + (25 if i % 2 else -25) + lean * 4, 7, 1.4, 3, droop=0.8 if i % 2 else -0.8)
            hx, hy = x + lean, top + 8
            if head == 'awned':
                for k in range(7):
                    y = hy - k; c.put(hx - 0.5, y, 'W', 2 + (k % 2)); c.put(hx + 0.6, y, 'W', 1 + (k % 2))
                    if k % 2 == 0: c.put(hx - 2, y - 2, 'W', 3); c.put(hx + 2, y - 2, 'W', 2)
            elif head == 'panicle':
                for k in range(6):
                    sx, sy = hx + (k % 3 - 1) * 1.5, hy - 1 - k * 1.2
                    line(c, hx, hy - k, sx + (1.5 if k % 2 else -1.5), sy + 3, 'W', 1)
                    c.put(sx + (1.5 if k % 2 else -1.5), sy + 3, 'W', 3); c.put(sx + (1.5 if k % 2 else -1.5), sy + 4, 'W', 2)
            else:
                ellipse(c, hx, hy - 3, 2.2, 3.6, 'W', 2, jag=0.5, seed=i)
        rim(c); return c
    return f


def corn():
    def f():
        c = Canvas()
        stem(c, [(16, 31), (16, 4)], 2); line(c, 17, 31, 17, 6, 'L', 1)
        for y, side, l in [(26, -1, 12), (21, 1, 13), (16, -1, 11), (11, 1, 10), (7, -1, 8)]:
            blade_leaf(c, 16, y, -90 + side * 60, l, 2.2, 3, droop=side * 1.2)
        for k in range(5): c.put(15 + (k % 3), 3 - k // 2, 'W', 3)
        ellipse(c, 19, 15, 2.0, 4.5, 'F', 2)
        for y in range(12, 19, 2): c.put(19, y, 'F', 3)
        blade_leaf(c, 18, 19, -60, 6, 2.4, 2, droop=0.2)
        rim(c); return c
    return f


def pods(seed=0):
    """A climbing pulse: tendrils, small leaves, white flowers and hanging pods."""
    def f():
        c = Canvas(); r = random.Random(seed)
        for x0, xt in [(12, 9), (16, 17), (20, 24)]:
            pts = curve(x0, 31, xt, 3, r.uniform(-3, 3), 8); stem(c, pts, 2)
            for k in range(1, 8):
                x, y = pts[k]
                if k % 2: leaf(c, x, y, -150 if k % 4 == 1 else -30, 3.4, 2.6, 3)
                elif k < 7: line(c, x, y, x + 1, y + 4, 'F', 2); line(c, x + 1, y, x + 2, y + 4, 'F', 1); c.put(x + 1, y + 5, 'F', 1)
            c.put(xt, 3, 'P', 3); c.put(xt + 1, 3, 'P', 2); c.put(xt, 2, 'P', 3)
        rim(c); return c
    return f


def chickpea(seed=0):
    """A bushy upright pulse: branching stems of tiny paired leaflets, a few small flowers
    and plump, puffed pods (the wild form of the chickpea crop)."""
    def f():
        c = Canvas(); r = random.Random(seed)
        for x0, xt in [(13, 7), (15, 12), (16, 17), (18, 22), (19, 26)]:
            pts = curve(x0, 31, xt, 9 + r.randint(0, 5), r.uniform(-2, 2), 7); stem(c, pts, 2)
            for k in range(1, 7):
                x, y = pts[k]
                for side in (-1, 1):
                    leaf(c, x, y, -90 + side * (55 + r.uniform(-10, 10)), 2.4, 1.6, 3)
                if k in (3, 5) and r.random() < 0.8:
                    ex = x + r.choice((-2, 2))
                    ellipse(c, ex, y + 1, 1.6, 1.2, 'F', 2)
            xt2, yt = pts[-1]
            c.put(xt2, yt - 1, 'P', 3); c.put(xt2 + 1, yt - 1, 'P', 2)
        rim(c); return c
    return f


def bush(fruit=10, cluster=1, size=1, leaf_len=2.6, low=False, flowers=0, seed=0, spiky=False):
    """A rounded shrub of small leaves, with berries, fruit or flowers among them."""
    def f():
        c = Canvas()
        if low:
            foliage(c, 16, 26, 14, 5, 3, seed=seed, leaf_len=leaf_len)
            berries(c, 16, 25, 12, 4, fruit, seed=seed, size=size, cluster=cluster)
        else:
            twigs(c, 16, 31, 5, 9, 40, 1, seed)
            foliage(c, 16, 17, 12, 10, 3, seed=seed, leaf_len=leaf_len, density=1.2)
            if spiky:
                rr = random.Random(seed + 3)
                for i in range(30):
                    x, y = 16 + rr.uniform(-12, 12), 17 + rr.uniform(-10, 10)
                    if ((x - 16) / 12) ** 2 + ((y - 17) / 10) ** 2 < 1: c.put(x, y, 'L', 5)
            berries(c, 16, 17, 10, 8, fruit, seed=seed, size=size, cluster=cluster)
            rr = random.Random(seed + 7)
            for i in range(flowers):
                a = rr.uniform(0, 6.28); d = math.sqrt(rr.random()) * 0.8
                x, y = 16 + math.cos(a) * d * 11, 17 + math.sin(a) * d * 9
                for b in range(5): petal(c, x, y, b * 72 - 90, 1.3, 1.3, 'P', 2 + (1 if b in (0, 4) else 0))
                c.put(x, y, 'C', 2)
        rim(c); return c
    return f


def herb(pairs=4, leaf_len=4.5, leaf_w=2.6, stems=3, top='leaf', seed=0, droop=0.2):
    """Upright stems with leaves alternating up them, smaller towards the top (mint, basil,
    sage), tipped by a leaf bud or a few small flowers."""
    def f():
        c = Canvas(); r = random.Random(seed)
        for s in range(stems):
            x = 16 + (s - (stems - 1) / 2) * 7 + r.uniform(-1, 1); top_y = r.uniform(4, 10)
            pts = curve(16 + (x - 16) * 0.3, 31, x, top_y, r.uniform(-3, 3), 12); stem(c, pts, 2)
            for k in range(pairs * 2):
                px, py = pts[2 + k * 9 // (pairs * 2)]
                l = leaf_len * (1 - k * 0.07)
                side = -1 if (k + s) % 2 else 1
                blade_leaf(c, px, py, -90 + side * r.uniform(45, 70), l, leaf_w * (1 - k * 0.04), 3, droop=side * droop)
            tx, ty = pts[-1]
            if top == 'leaf': leaf(c, tx, ty + 1, -90, 2.6, 1.8, 4)
            elif top == 'flower':
                for k in range(5): c.put(tx - 1 + k % 2 * 2, ty - k // 2, 'P', 2 + k % 2)
        rim(c); return c
    return f


def umbel(heads=3, flat=True, seed=0):
    """Flat-topped clusters of tiny flowers on stalks, with feathery leaves (carrot, dill)."""
    def f():
        c = Canvas(); r = random.Random(seed)
        for a in (-150, -120, -60, -30): feather_leaf(c, 16, 29, a, 9, 3)
        spots = [(10, 7), (21, 9), (15, 13)][:heads]
        for x, y in spots:
            stem(c, curve(16, 31, x, y + 2, r.uniform(-2, 2)), 2)
            for k in range(7):
                a = math.radians(-160 + k * 23)
                line(c, x, y + 3, x + math.cos(a) * 3.6, y + 0.6 + math.sin(a) * 1.4, 'L', 2)
            for dx in range(-4, 5):
                for dy in (-1, 0):
                    if abs(dx) + abs(dy) * 3 < 5 and (dx + dy) % 1 == 0:
                        c.put(x + dx, y + dy, 'P', 2 + (1 if dy < 0 else 0) + (1 if (dx % 3 == 0 and dy < 0) else 0))
            c.put(x, y - 1, 'C', 1)
        rim(c); return c
    return f


def tuft_grass(blades=11, h=24, plume=None, seed=0, curl=1.0):
    """A clump of grass blades, with plumes (pampas), cotton puffs or brown heads (cattail)."""
    def f():
        c = Canvas(); r = random.Random(seed)
        tops = []
        for i in range(blades):
            x0 = 16 + r.uniform(-3, 3); lean = r.uniform(-9, 9); t = h * r.uniform(0.55, 1.0)
            pts = curve(x0, 31, x0 + lean, 31 - t, lean * 0.4 * curl, 7)
            for k, ((ax, ay), (bx, by)) in enumerate(zip(pts, pts[1:])):
                line(c, ax, ay, bx, by, 'L', 2 + (1 if k > 2 else 0) + (1 if k > 4 and lean < 0 else 0))
                if k < 2: line(c, ax + 1, ay, bx + 1, by, 'L', 1)
            tops.append(pts[-1])
        if plume == 'pampas':
            for x, y in tops[:4]: ellipse(c, x, y - 2, 1.8, 4, 'P', 2, jag=0.6, seed=int(x))
        elif plume == 'cotton':
            for x, y in tops[:5]: ellipse(c, x, y - 1, 1.8, 1.8, 'P', 3, jag=0.5, seed=int(x))
        elif plume == 'cattail':
            for x, y in tops[:3]:
                # A slim brown head, rounded at both ends, lit on its left, on a spike.
                for k in range(7):
                    w = 0 if k in (0, 6) else 1
                    for dx in range(-w, w + 1):
                        c.put(x + dx, y - k, 'F', (2 if dx < 0 else 1 if dx == 0 else 0) + (1 if (k + dx) % 3 == 0 else 0))
                line(c, x, y - 7, x, y - 10, 'L', 3)
        elif plume == 'papyrus':
            for x, y in tops[:4]:
                for k in range(9):
                    a = math.radians(-180 + k * 22.5); line(c, x, y, x + math.cos(a) * 4, y + math.sin(a) * 2.5, 'L', 3 + k % 2)
        rim(c); return c
    return f


def rush_tuft(seed=0, stems=12, h=27):
    """A tight, upright tuft of thin round stems, a few carrying small brown seed tufts
    near their tips (rush)."""
    def f():
        c = Canvas(); r = random.Random(seed)
        tops = []
        for i in range(stems):
            x0 = 16 + (i - (stems - 1) / 2) * 0.5 + r.uniform(-0.4, 0.4)
            lean = (x0 - 16) * 4.2 + r.uniform(-1.5, 1.5); t = h * r.uniform(0.6, 1.0)
            pts = curve(x0, 31, x0 + lean, 31 - t, lean * 0.12, 8)
            for k, ((ax, ay), (bx, by)) in enumerate(zip(pts, pts[1:])):
                line(c, ax, ay, bx, by, 'L', (1, 3, 2, 4)[i % 4] + (1 if k > 5 else 0))
            tops.append(pts[-1])
        for x, y in tops[1:9:2]:
            for dx, dy, sh in ((0, 4, 2), (1, 5, 1), (1, 4, 3), (0, 5, 1)):
                c.put(x + dx, y + dy, 'F', sh)
        rim(c); return c
    return f


def fern(fronds=6, length=13, seed=0, maiden=False):
    def f():
        c = Canvas(); r = random.Random(seed)
        for i in range(fronds):
            a = -90 + (i - (fronds - 1) / 2) * (100 / fronds)
            if maiden:
                pts = curve(16, 31, 16 + math.cos(math.radians(a)) * length, 31 + math.sin(math.radians(a)) * length, 2, 8)
                stem(c, pts, 1)
                for x, y in pts[1:]: ellipse(c, x, y - 1, 1.8, 1.2, 'L', 3, seed=int(x))
            else:
                feather_leaf(c, 16, 31, a, length * r.uniform(0.8, 1.1), 3)
        rim(c); return c
    return f


def horsetail():
    def f():
        c = Canvas()
        for x, t in [(11, 6), (16, 3), (21, 8)]:
            line(c, x, 31, x, t, 'L', 3)
            for y in range(t + 2, 30, 3):
                c.put(x, y, 'W', 1)
                for k in range(1, 4): c.put(x - k, y + k * 0.6, 'L', 4); c.put(x + k, y + k * 0.6, 'L', 2)
        rim(c); return c
    return f


def mushroom(cap='dome', n=2, spots=False, seed=0):
    """Mushrooms: dome, flat, conical (inkcap), honeycomb (morel) or ball (puffball)."""
    def f():
        c = Canvas(); r = random.Random(seed)
        for i, (x, s) in enumerate([(13, 1.35), (22, 0.95), (7, 0.75)][:n]):
            base = 31; h = 9 * s
            if cap == 'ball':
                ellipse(c, x, base - 4 * s, 4.5 * s, 4 * s, 'P', 2)
                continue
            for y in range(int(base - h), base + 1):
                c.put(x - 1, y, 'C', 2); c.put(x, y, 'C', 1); c.put(x + 1, y, 'C', 0)
            cy = base - h
            if cap == 'dome':
                ellipse(c, x, cy, 6 * s, 3.6 * s, 'P', 2)
                for k in range(int(-5 * s), int(5 * s) + 1): c.put(x + k, cy + 2 * s, 'C', 1)
            elif cap == 'flat':
                ellipse(c, x, cy, 6.5 * s, 2.4 * s, 'P', 2)
                for k in range(int(-6 * s), int(6 * s) + 1, 2): c.put(x + k, cy + 2 * s, 'C', 0)
            elif cap == 'cone':
                for y in range(int(cy - 7 * s), int(cy + 2)):
                    w = 3.2 * s * (y - (cy - 7 * s)) / (9 * s) + 0.8
                    for xx in range(int(x - w), int(x + w) + 1):
                        c.put(xx, y, 'P', 2 + (1 if xx < x else 0) - (1 if xx > x + w * 0.5 else 0))
                for xx in range(int(x - 3.5 * s), int(x + 3.5 * s) + 1, 2): c.put(xx, cy + 2, 'F', 1)
            elif cap == 'morel':
                for y in range(int(cy - 8 * s), int(cy + 2)):
                    w = 3 * s * math.sin(math.pi * (y - (cy - 8 * s)) / (10 * s)) + 0.5
                    for xx in range(int(x - w), int(x + w) + 1):
                        c.put(xx, y, 'P', (1 if (xx + y) % 3 == 0 else 3) - (1 if xx > x else 0))
            if spots:
                for k in range(int(5 * s)): c.put(x + r.randint(int(-5 * s), int(4 * s)), cy + r.randint(-3, 0) * s, 'S', 4)
        for x in (5, 26): c.put(x, 31, 'L', 3); c.put(x, 30, 'L', 4)
        rim(c); return c
    return f


def shelf(log=True, mane=False):
    def f():
        c = Canvas()
        if log:
            for y in range(25, 32):
                for x in range(2, 30):
                    c.put(x, y, 'W', 1 + (1 if y < 27 else 0) - (1 if y > 30 else 0) + (1 if (x * 7 + y) % 9 == 0 else 0))
        for (x, y, w) in [(10, 22, 7), (20, 17, 6), (12, 12, 5)]:
            if mane:
                for k in range(-w // 2 * 2, w):
                    line(c, x + k * 0.8, y - 2, x + k * 0.8, y + 4 + (k % 2), 'P', 2 + (1 if k < 0 else 0))
                ellipse(c, x, y - 2, w * 0.7, 2, 'P', 3)
            else:
                ellipse(c, x, y, w, 2.6, 'P', 2)
                for k in range(-w + 1, w, 2): c.put(x + k, y + 2, 'P', 0)
        rim(c); return c
    return f


def shrub_flowers(flower='spiky', seed=0):
    """A flowering shrub: spiny gorse, broom's tall green rods, or big round flower trusses."""
    def f():
        c = Canvas(); r = random.Random(seed)
        if flower == 'rods':
            for i in range(9):
                x0 = 16 + r.uniform(-2, 2); lean = r.uniform(-10, 10)
                pts = curve(x0, 31, x0 + lean, r.uniform(3, 9), lean * 0.3, 6); stem(c, pts, 3)
                for x, y in pts[2:]: c.put(x + 1, y, 'P', 3); c.put(x - 1, y + 1, 'P', 2)
        elif flower == 'grey':
            twigs(c, 16, 31, 7, 12, 55, 1, seed)
            rr = random.Random(seed)
            for i in range(90):
                a = rr.uniform(0, 6.28); d = math.sqrt(rr.random())
                x, y = 16 + math.cos(a) * d * 12, 15 + math.sin(a) * d * 8
                c.put(x, y, 'L', 2 + rr.randint(0, 3) - (1 if math.sin(a) > 0.4 else 0))
        else:
            twigs(c, 16, 31, 5, 9, 40, 1, seed)
            foliage(c, 16, 17, 12, 10, 3, seed=seed, leaf_len=3.2 if flower == 'truss' else 2)
            rr = random.Random(seed + 5)
            if flower == 'truss':
                for x, y in [(10, 12), (20, 10), (16, 19), (24, 18), (8, 20)]:
                    ellipse(c, x, y, 3, 2.6, 'P', 2, jag=0.6, seed=x)
                    for k in range(4): c.put(x - 1 + k % 2 * 2, y - 1 + k // 2, 'P', 4)
            else:
                for i in range(26):
                    a = rr.uniform(0, 6.28); d = math.sqrt(rr.random())
                    x, y = 16 + math.cos(a) * d * 11, 17 + math.sin(a) * d * 9
                    c.put(x, y, 'P', 3); c.put(x + 1, y, 'P', 2); c.put(x, y - 1, 'P', 4)
        rim(c); return c
    return f


def dead_bush():
    def f():
        c = Canvas(); twigs(c, 16, 31, 9, 18, 70, 1, 3); twigs(c, 16, 31, 5, 12, 50, 2, 8); rim(c); return c
    return f


def prickly_pear():
    def f():
        c = Canvas()
        for (x, y, rx, ry) in [(16, 22, 5, 7), (10, 13, 4, 5.5), (22, 12, 4, 5.5), (16, 6, 3, 4)]:
            ellipse(c, x, y, rx, ry, 'L', 3)
            for k in range(4): c.put(x - rx + 2 + k * 2, y - 1 + (k % 2) * 3, 'P', 4)
        for x, y in [(8, 7), (13, 2), (24, 6), (19, 2)]: berry(c, x, y, 'F', 2)
        rim(c); return c
    return f


def pineapple():
    def f():
        c = Canvas()
        for a in (-150, -120, -60, -30): blade_leaf(c, 16, 30, a, 12, 2.2, 3, droop=0.5 if a > -90 else -0.5)
        ellipse(c, 16, 15, 5, 7, 'F', 2)
        for y in range(9, 22, 3):
            for x in range(12, 21, 3): c.put(x + (y // 3) % 2, y, 'F', 0); c.put(x + 1 + (y // 3) % 2, y - 1, 'F', 3)
        for k in range(7): blade_leaf(c, 16, 8, -90 + (k - 3) * 20, 6, 1.4, 4, droop=(k - 3) * 0.2)
        rim(c); return c
    return f


def grape_vine():
    def f():
        c = Canvas()
        stem(c, [(4, 31), (6, 20), (12, 12), (20, 9), (28, 12)], 0)
        for x, y in [(6, 14), (14, 7), (24, 5), (28, 16), (9, 22)]: ellipse(c, x, y, 3.6, 3, 'L', 3, jag=0.4, seed=x)
        for x, y in [(13, 15), (21, 13)]:
            for row in range(5):
                for k in range(5 - row):
                    berry(c, x - (5 - row) + k * 2 + 1, y + row * 2, 'F', 1)
        rim(c); return c
    return f


def chili():
    def f():
        c = Canvas(); twigs(c, 16, 31, 4, 8, 30, 1, 2)
        foliage(c, 16, 14, 11, 8, 3, seed=4, leaf_len=3.0)
        for x, y in [(10, 17), (15, 20), (21, 18), (18, 13), (12, 12)]:
            for k in range(5): c.put(x, y + k, 'F', 2 + (1 if k < 2 else 0)); c.put(x + 1, y + k, 'F', 1)
            c.put(x, y - 1, 'L', 2)
        rim(c); return c
    return f


def rosette_berries(seed=0):
    """A low rosette of broad leaves with a cluster of berries in the middle (ginseng,
    mandrake)."""
    def f():
        c = Canvas()
        for a in range(0, 360, 60): blade_leaf(c, 16, 22, a - 90 + 30, 9, 4, 3, droop=0.3)
        for x, y in [(15, 12), (17, 12), (16, 10), (14, 14), (18, 14)]: berry(c, x, y, 'F', 1)
        line(c, 16, 22, 16, 15, 'L', 2)
        ellipse(c, 16, 29, 4, 2.4, 'W', 2)
        rim(c); return c
    return f


def windswept(seed=0):
    """A wiry mountain herb the wind has combed over (windroot): a gnarled pale root crown
    half out of the ground, wiry stems fanning up from it and curling over downwind, fine
    needle leaves streaming off them, and small pale flower tufts at the tips."""
    def f():
        c = Canvas(); r = random.Random(seed)
        tips = []
        # (start angle, length): steep ones curl over the most.
        for a0, length in ((-112, 22), (-98, 25), (-86, 21), (-72, 17), (-58, 14), (-124, 15), (-45, 11)):
            a = math.radians(a0 + r.uniform(-4, 4)); x, y = 11 + r.uniform(-1, 1), 28.0
            pts = [(x, y)]
            n = int(length)
            for i in range(n):
                # The wind turns every stem a little more downwind as it rises.
                a += math.radians(3.2 + 2.8 * i / n)
                x += math.cos(a); y += math.sin(a)
                pts.append((x, y))
            for (ax, ay), (bx, by) in zip(pts, pts[1:]):
                line(c, ax, ay, bx, by, 'L', 1)
            for k in range(5, len(pts) - 1, 4):
                px, py = pts[k]
                ang = math.degrees(a) * 0.3 + r.uniform(-10, 15)
                blade_leaf(c, px, py, ang, r.uniform(2.6, 3.8), 0.9, 3 + k % 2, droop=0.3)
            tips.append(pts[-1])
        # Tufts of tiny pale flowers at the tips of the tallest stems.
        for x, y in sorted(tips, key=lambda p: p[1])[:3]:
            for dx, dy, sh in ((0, 0, 3), (1, 0, 4), (1, 1, 2), (0, -1, 4), (2, 0, 3)):
                c.put(x + dx, y + dy, 'P', sh)
        # The root crown: gnarled, pale, roots twisting into the ground.
        ellipse(c, 11, 29.5, 3.6, 1.8, 'W', 2, jag=0.6, seed=seed)
        for ax, bx in ((9, 6), (13, 16)):
            line(c, ax, 30, bx, 31, 'W', 1)
        c.put(10, 28, 'W', 3); c.put(11, 28, 'W', 3)
        rim(c); return c
    return f


def moss_carpet(seed=0, sparkle=False):
    def f():
        c = Canvas(); r = random.Random(seed)
        ellipse(c, 16, 28, 15, 4, 'L', 2, jag=0.5, seed=seed)
        for i in range(40):
            x, y = r.uniform(2, 30), r.uniform(24, 31)
            c.put(x, y, 'L', r.randint(2, 5)); c.put(x, y - 1, 'L', 4)
        if sparkle:
            for i in range(8): c.put(r.uniform(3, 29), r.uniform(23, 29), 'G', 2)
        rim(c); return c
    return f


# --- the plants ---------------------------------------------------------------
LG = '#4a8a2a'  # ordinary leaf green
add('wild_onion', root_veg('bulb', 5, seed=1), leaf='#5a9a6a', petal='#e8e4d0', wood='#9a8060')
add('wild_leek', root_veg('bulb', 6, seed=2), leaf='#4e8a3a', petal='#e4ecd4', wood='#9a8060')
add('wild_turnip', root_veg('round', 5, seed=3), leaf=LG, petal='#c088c0', wood='#8a6a4a')
add('wild_beet', root_veg('round', 5, seed=4), leaf='#3a6a2a', petal='#901a3e', wood='#6a2a2a')
add('wild_radish', root_veg('round', 4, seed=5), leaf='#58a23a', petal='#e0405a', wood='#8a6a4a')
add('wild_parsnip', root_veg('long', 5, frilly=True, seed=6), leaf='#5a9a3a', petal='#e2d4a4', wood='#8a6a4a')
add('wild_carrot', root_veg('long', 6, frilly=True, seed=7), leaf='#4e922e', petal='#e8862a', wood='#8a6a4a')
add('wild_asparagus', horsetail(), leaf='#6aa24a', wood='#8a5a7a')
add('wild_pumpkin', vine_fruit('round', 1), leaf='#4a823a', fruit='#e07818', wood='#6a5a2a')
add('wild_squash', vine_fruit('long', 2), leaf='#4a823a', fruit='#e8c030', wood='#6a5a2a')
add('wild_melon', vine_fruit('round', 3), leaf='#4a823a', fruit='#5e9a3e', wood='#6a5a2a')
add('wild_yam', big_leaf(True, 1), leaf='#3e7a2e', petal='#8a5a3a')
add('wild_taro', big_leaf(True, 2), leaf='#3a8a4a', petal='#8a7a7a')
add('wild_ginger', big_leaf(True, 3), leaf='#3a7a3a', petal='#d0a858')
add('wild_wheat', grain('awned', 5, 1), leaf='#6a8a3a', wood='#d0aa58')
add('wild_barley', grain('awned', 6, 2), leaf='#6a8a3a', wood='#c09a4a')
add('wild_oats', grain('panicle', 5, 3), leaf='#6a9a3a', wood='#d8c88e')
add('wild_rice', grain('panicle', 6, 4), leaf='#5a9a3a', wood='#bcba7a')
add('wild_millet', grain('dense', 5, 5), leaf='#6a9a3a', wood='#c08a40')
add('wild_quinoa', grain('dense', 4, 6), leaf='#5a8a5a', wood='#c04a4a')
add('wild_corn', corn(), leaf='#5a9a3a', fruit='#f0c030', wood='#e8d890')
add('wild_beans', pods(1), leaf='#4a8a2a', fruit='#5a9a2a', petal='#f0f0e8')
add('wild_peas', pods(2), leaf='#5aa23a', fruit='#80c050', petal='#f0e4f0')
add('wild_lentils', pods(3), leaf='#5a8a5a', fruit='#b0a050', petal='#d8d0f0')
add('wild_chickpea', chickpea(5), leaf='#6a8e5a', fruit='#c4b878', petal='#d8a8c8')
add('blueberry_bush', bush(14, 1, 1, seed=1), leaf='#3a6a3a', fruit='#3a58a8', wood='#5a4030')
add('raspberry_bush', bush(10, 3, 1, leaf_len=3.2, seed=2), leaf='#4a7a2a', fruit='#d03050', wood='#6a4a30')
add('elderberry_bush', bush(6, 4, 1, leaf_len=3.4, seed=3), leaf='#3e6a2e', fruit='#4a2050', wood='#6a5a4a')
add('juniper_bush', bush(12, 1, 1, leaf_len=1.6, seed=4, spiky=True), leaf='#3a6a5a', fruit='#4a6aa0', wood='#5a4030')
add('wild_rose_bush', bush(6, 1, 1, flowers=7, seed=5), leaf='#3e6e2e', fruit='#c02a2a', petal='#f088b0', centre='#f0d040', wood='#5a4030')
add('holly_bush', bush(14, 1, 1, leaf_len=3.0, seed=6, spiky=True), leaf='#1e4e2a', fruit='#d02020', wood='#4a3a2a')
add('tea_bush', bush(0, 1, 1, leaf_len=3.4, flowers=6, seed=7), leaf='#3a7a2a', petal='#f4f0e0', centre='#e8c040', wood='#5a4030')
add('sweet_berry_bush_stage3', bush(12, 1, 1, leaf_len=2.8, seed=8), leaf='#3a6a3a', fruit='#c02030', wood='#5a4030')
add('cranberry_bush', bush(14, 1, 1, leaf_len=1.8, low=True, seed=9), leaf='#3a5a3a', fruit='#b01830')
add('lingonberry_bush', bush(12, 1, 1, leaf_len=2.2, low=True, seed=10), leaf='#2e5a2a', fruit='#e03a3a')
add('cloudberry', bush(5, 1, 2, leaf_len=3.6, low=True, seed=11), leaf='#4a8a2a', fruit='#f0a040')
add('gooseberry_bush', bush(10, 1, 2, leaf_len=3.0, seed=12, spiky=True), leaf='#4a7a2a', fruit='#a8d070', wood='#5a4030')
add('currant_bush', bush(8, 4, 1, leaf_len=3.2, seed=13), leaf='#4a8a2a', fruit='#d02030', wood='#5a4030')
add('blackberry_bush', bush(10, 3, 1, leaf_len=3.2, seed=14, spiky=True), leaf='#3e6a2a', fruit='#2a1a3a', wood='#6a4a30')
add('goji_bush', bush(14, 1, 1, leaf_len=2.4, seed=15), leaf='#4a7a5a', fruit='#f05030', wood='#7a6a50')
add('coffee_bush', bush(12, 2, 1, leaf_len=3.6, seed=16), leaf='#2e5a2a', fruit='#b02020', wood='#5a4030')
add('hazel_shrub', bush(6, 2, 1, leaf_len=3.8, seed=17), leaf='#4a7a2a', fruit='#a87a3e', wood='#6a5040')
add('mint', herb(4, 4.2, 3.0, 3, 'leaf', 1), leaf='#4aa04a')
add('basil', herb(3, 5, 3.6, 3, 'leaf', 2), leaf='#3e9a2e')
add('parsley', fern(9, 15, 3), leaf='#3e8a2a')
add('sage', herb(4, 5, 2.6, 3, 'flower', 4, droop=0.5), leaf='#7a9a7a', petal='#9a7ac8')
add('windroot', windswept(3), leaf='#8aa48c', petal='#f0e6b8', wood='#d8c8a0')
add('wormwood', herb(5, 4, 1.6, 3, 'leaf', 5), leaf='#a4b4ac')
add('sorrel', herb(3, 6, 2.4, 2, 'flower', 6, droop=0.6), leaf='#5a9a3a', petal='#b03a3a')
add('nettle', herb(5, 4.2, 2.6, 3, 'leaf', 7), leaf='#3a7a2a')
add('wild_garlic', root_veg('bulb', 4, seed=8), leaf='#4a9a3a', petal='#f0f0e8', wood='#8a7a5a')
add('dill', umbel(3, seed=1), leaf='#5a9a6a', petal='#e8d840', centre='#c0a020')
add('fennel', umbel(3, seed=2), leaf='#6aa24a', petal='#e8d040', centre='#c0a020')
add('coriander', umbel(3, seed=3), leaf='#4a9a3a', petal='#f4f0f0', centre='#e0a0c0')
add('cumin', umbel(2, seed=4), leaf='#6a9a7a', petal='#f0c8d8', centre='#c08aa0')
add('valerian', umbel(3, seed=5), leaf='#4a8a3a', petal='#f0d0e0', centre='#d090b0')
add('ginseng', rosette_berries(1), leaf='#3e7a2e', fruit='#d02a2a', wood='#d8c090')
add('mandrake', rosette_berries(2), leaf='#2e5a2a', fruit='#e8c040', wood='#a07a4a')
add('chili', chili(), leaf='#3a7a2a', fruit='#d82a1a', wood='#5a4030')
add('dune_grass', tuft_grass(13, 22, None, 1), leaf='#9aa86a')
add('bunchgrass', tuft_grass(15, 18, None, 2, 0.4), leaf='#b8a058')
add('lemongrass', tuft_grass(12, 24, None, 3, 1.4), leaf='#9ac04a')
add('pampas_grass', tuft_grass(12, 20, 'pampas', 4), leaf='#7a9a6a', petal='#e8e0c8')
add('cotton_grass', tuft_grass(10, 18, 'cotton', 5), leaf='#7a9a5a', petal='#f4f4f0')
add('cattail', tuft_grass(9, 26, 'cattail', 6, 0.3), leaf='#5a8a3a', fruit='#6a4020')
add('papyrus', tuft_grass(6, 26, 'papyrus', 7, 0.3), leaf='#5aa23a')
add('rush', rush_tuft(8), leaf='#b0b0b0', fruit='#7a6a50')
add('maidenhair_fern', fern(7, 16, 1, maiden=True), leaf='#9a9a9a')
add('bracken', fern(7, 19, 2), leaf='#8a9a3a')
add('horsetail', horsetail(), leaf='#5a9a3a', wood='#2a3a2a')
add('chanterelle', mushroom('flat', 3, seed=1), petal='#f0a828', centre='#e8b84a', leaf='#4a7a2a')
add('porcini', mushroom('dome', 2, seed=2), petal='#7a4a28', centre='#e0d4b0', leaf='#4a7a2a')
add('puffball', mushroom('ball', 3, seed=3), petal='#e8e4d8', leaf='#4a7a2a')
add('inkcap', mushroom('cone', 3, seed=4), petal='#d4d0ca', centre='#f0ece4', fruit='#2a2a30', leaf='#4a7a2a')
add('morel', mushroom('morel', 2, seed=5), petal='#a88a5e', centre='#e8dcc0', leaf='#4a7a2a')
add('deathcap', mushroom('dome', 2, seed=6), petal='#c8d0a0', centre='#f0f0e0', leaf='#4a7a2a')
add('brown_mushroom', mushroom('dome', 2, seed=7), petal='#9a6a48', centre='#e0d4b8', leaf='#4a7a2a')
add('red_mushroom', mushroom('dome', 2, spots=True, seed=8), petal='#c82a1e', centre='#e0d4b8', second='#ffffff', leaf='#4a7a2a')
add('fly_agaric', mushroom('dome', 2, spots=True, seed=9), petal='#e0281a', centre='#f0eadc', second='#ffffff', leaf='#4a7a2a')
add('blewit', mushroom('dome', 2, seed=10), petal='#7a68b0', centre='#d0c8e0', leaf='#4a7a2a')
add('shiitake', mushroom('flat', 2, seed=11), petal='#5a3a26', centre='#e8dcc8', leaf='#4a7a2a')
add('truffle', mushroom('ball', 2, seed=12), petal='#3a2a22', leaf='#4a7a2a')
add('oyster_mushroom', shelf(), petal='#b8b8bc', wood='#5a4a3a')
add('chicken_of_the_woods', shelf(), petal='#f08a28', wood='#5a4a3a')
add('lions_mane', shelf(mane=True), petal='#ece6da', wood='#5a4a3a')
add('gorse', shrub_flowers('spiky', 1), leaf='#2e5a3a', petal='#f0c020', wood='#4a3a2a')
add('broom_shrub', shrub_flowers('rods', 2), leaf='#4a7a2a', petal='#f8e040')
add('rhododendron', shrub_flowers('truss', 3), leaf='#2a5a2a', petal='#d04aa0', wood='#4a3a2a')
add('oleander', shrub_flowers('truss', 4), leaf='#4a7a5a', petal='#f0a0b8', wood='#5a4a3a')
add('sagebrush', shrub_flowers('grey', 5), leaf='#9aa894', wood='#6a5a4a')
add('creosote_bush', shrub_flowers('grey', 6), leaf='#7a8a3a', wood='#5a4a3a')
add('saltbush', shrub_flowers('grey', 7), leaf='#b8c8c4', wood='#7a6a5a')
add('dead_bush', dead_bush(), wood='#9a6a32')
add('prickly_pear', prickly_pear(), leaf='#5a8a4a', petal='#e8e0a0', fruit='#c02a6a')
add('wild_pineapple', pineapple(), leaf='#5a8a6a', fruit='#e0b030')
add('wild_grape_vine', grape_vine(), leaf='#4a8a2a', fruit='#5a2e7a', wood='#5a3a20')
