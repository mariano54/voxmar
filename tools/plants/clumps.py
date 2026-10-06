"""Flowers as clumps of tiny blossoms, and fine grass: each flower block is a rounded,
bushy clump of small leaves holding dozens of blossoms a few texels across, at several
heights, with a few loose ones on thin stems beside it; grass is many thin pale blades with
light tips. Each species keeps its own blossom colour and shape (a cross of four petals, a
five-petal star, a nodding bell, a little cup, a fluffy ball or a spike of florets), so it
reads at a glance. Our own art: the idea of tiny-blossom clumps came from a description of
another pack's look (see INSPIRATIONS.md); no pixels were taken from anything."""
import math, os, random, sys
sys.path.insert(0, os.path.dirname(__file__))
from kit import *

C = {}  # name -> (draw, colours)


def blossom(c, x, y, shape, depth):
    """One tiny blossom at (x, y). `depth` 0 is at the back (darker), 1 at the front."""
    x, y = int(round(x)), int(round(y))
    lit = 2 + (1 if depth > 0.5 else 0)
    if shape == 'cross':  # four petals around a centre
        for dx, dy, s in ((0, -1, 1), (-1, 0, 1), (1, 0, 0), (0, 1, 0)):
            c.put(x + dx, y + dy, 'P', lit + s - 1)
        c.put(x, y, 'C', 2)
    elif shape == 'star':  # five petals: a 3×3 with the centre and a soft corner
        for dx in (-1, 0, 1):
            for dy in (-1, 0, 1):
                if (dx, dy) in ((1, 1), (-1, 1)):
                    continue
                c.put(x + dx, y + dy, 'P', lit - (1 if dx + dy > 0 else 0) + (1 if dx + dy < 0 else 0))
        c.put(x, y, 'C', 2)
    elif shape == 'bell':  # a nodding bell on a short curved stalk
        c.put(x, y - 1, 'L', 2)
        c.put(x, y, 'P', lit); c.put(x - 1, y + 1, 'P', lit); c.put(x, y + 1, 'P', lit - 1); c.put(x + 1, y + 1, 'P', lit - 1)
    elif shape == 'cup':  # a little upright cup (tulip, crocus)
        c.put(x - 1, y - 1, 'P', lit + 1); c.put(x + 1, y - 1, 'P', lit)
        c.put(x - 1, y, 'P', lit); c.put(x, y, 'P', lit); c.put(x + 1, y, 'P', lit - 1); c.put(x, y + 1, 'P', lit - 1)
    elif shape == 'ball':  # a fluffy ball of florets (allium, clover)
        for dx in (-1, 0, 1):
            for dy in (-1, 0, 1):
                if abs(dx) + abs(dy) < 2:
                    c.put(x + dx, y + dy, 'P', lit + (1 if dx + dy < 0 else 0) - (1 if dx + dy > 0 else 0))
        c.put(x - 1, y - 1, 'P', lit - 1)
    elif shape == 'spike':  # a short spike of florets (lavender, lupine)
        for k in range(4):
            c.put(x, y - k, 'P', lit + (k % 2)); c.put(x + (1 if k % 2 else -1), y - k + 1, 'P', lit - 1)
    else:  # 'dot': the smallest blossom
        c.put(x, y, 'P', lit + 1); c.put(x + 1, y, 'P', lit); c.put(x, y + 1, 'P', lit - 1)


def clump(shape, n=34, width=11, height=17, loose=3, centre_y=None, tall=False, seed=0):
    """A rounded bushy clump of small leaves and dozens of tiny blossoms at several heights,
    lightest at the front and top; a few loose blossoms on thin stems beside it."""
    def f():
        c = Canvas(); r = random.Random(seed)
        cy = centre_y or (31 - height * 0.55)
        # Thin stems inside the clump, then the leafy mound over them.
        for i in range(9):
            x0 = 16 + r.uniform(-width * 0.5, width * 0.5)
            line(c, 16 + (x0 - 16) * 0.3, 31, x0, cy - height * 0.35 + r.uniform(-2, 2), 'L', 1 + (i % 2))
        foliage(c, 16, cy + height * 0.15, width, height * 0.42, 3, seed=seed, leaf_len=1.9, density=1.1)
        # Blossoms, back to front so the nearer overlap the further.
        spots = []
        for i in range(n):
            a = r.uniform(0, 2 * math.pi); d = math.sqrt(r.random())
            x = 16 + math.cos(a) * d * width * 0.95
            y = cy - height * 0.25 + math.sin(a) * d * height * 0.45 - (1 - d) * 3
            spots.append((y, x))
        spots.sort()
        for y, x in spots:
            blossom(c, x, y, shape, (y - (cy - height * 0.7)) / (height * 0.9))
        # Loose blossoms on long thin stems around the clump.
        for i in range(loose):
            side = -1 if i % 2 == 0 else 1
            x = 16 + side * r.uniform(width * 0.8, width * 1.25); top = r.uniform(3, cy - 2)
            stem(c, curve(16 + side * 3, 31, x, top + 1, side * 1.5, 6), 2)
            blossom(c, x, top, shape, 1.0)
        # Soft shade at the foot of the clump.
        for x in range(int(16 - width), int(16 + width) + 1):
            if c.get(x, 31) is None and abs(x - 16) < width * 0.8: c.put(x, 31, 'L', 0)
        rim(c); return c
    return f


def grass(blades=24, low=10, high=24, tips=True, seed=0, ground=False):
    """Many thin, straight, pale blades with light tips: a short tuft or the foot of tall grass."""
    def f():
        c = Canvas(); r = random.Random(seed)
        order = sorted(range(blades), key=lambda i: r.random())
        for i in order:
            x0 = r.uniform(3, 29); h = r.uniform(low, high); lean = r.uniform(-2.5, 2.5)
            shade = r.choice((1, 2, 3, 3))
            pts = curve(x0, 31, x0 + lean, 31 - h, lean * 0.2, 6)
            for k, ((ax, ay), (bx, by)) in enumerate(zip(pts, pts[1:])):
                line(c, ax, ay, bx, by, 'L', shade - (1 if k == 0 else 0) + (1 if k >= 4 else 0))
            if tips:
                tx, ty = pts[-1]; c.put(tx, ty, 'L', 5); c.put(tx, ty + 1, 'L', 5 if shade > 2 else 4)
        if ground:
            for x in range(32):
                if r.random() < 0.6: c.put(x, 31, 'L', 1)
        return c
    return f


def tall_top(blades=20, seed=0):
    """The upper block of two-block grass: blades rising from the bottom edge, thinning to light
    tips, a few seed heads."""
    def f():
        c = Canvas(); r = random.Random(seed)
        for i in range(blades):
            x0 = r.uniform(3, 29); h = r.uniform(8, 27); lean = r.uniform(-3, 3)
            shade = r.choice((2, 3, 3, 4))
            pts = curve(x0, 32, x0 + lean, 32 - h, lean * 0.2, 6)
            for k, ((ax, ay), (bx, by)) in enumerate(zip(pts, pts[1:])):
                line(c, ax, ay, bx, by, 'L', shade + (1 if k >= 3 else 0))
            tx, ty = pts[-1]; c.put(tx, ty, 'L', 5)
            if i % 5 == 0:
                for k in range(4): c.put(tx + (k % 2), ty + k, 'L', 5 - (k % 2))
        return c
    return f


# Every flower: blossom shape, blossom colour, centre colour, and the clump's leaf colour.
LEAF = '#4e8a36'
F = {
    'dandelion': ('star', '#f2c42a', '#c8901a', LEAF),
    'poppy': ('cross', '#d8322a', '#2a1a14', LEAF),
    'cornflower': ('star', '#4a6ad8', '#2a2a7a', '#5a8a4a'),
    'oxeye_daisy': ('star', '#f2f0e8', '#e8b828', LEAF),
    'azure_bluet': ('cross', '#dce4f2', '#e8c840', LEAF),
    'allium': ('ball', '#a868d8', '#7a40a8', '#5a8a4a'),
    'blue_orchid': ('star', '#3aa8e8', '#1a5a9a', '#3e7a3a'),
    'lily_of_the_valley': ('bell', '#f4f4ee', '#d8d8d0', '#3e7a3a'),
    'red_tulip': ('cup', '#d83a2a', '#8a1a14', '#5a8a5a'),
    'orange_tulip': ('cup', '#ec842a', '#a84a14', '#5a8a5a'),
    'white_tulip': ('cup', '#eeeee8', '#b8b8b0', '#5a8a5a'),
    'pink_tulip': ('cup', '#ee9ac0', '#b8507a', '#5a8a5a'),
    'buttercup': ('star', '#f6d82a', '#d8a018', LEAF),
    'bluebell': ('bell', '#5a6ad8', '#3a3aa0', '#3e7a3a'),
    'foxglove': ('bell', '#c86ac0', '#7a2a7a', '#4e7a3e'),
    'lavender': ('spike', '#9a80d8', '#6a50a8', '#6a8a6a'),
    'marigold': ('star', '#f08a20', '#a04a10', LEAF),
    'sunflower': ('star', '#f8c830', '#5a3418', '#4e8a36'),
    'heather': ('spike', '#c060b0', '#8a3a80', '#3e5a3a'),
    'edelweiss': ('star', '#ecece4', '#d8c070', '#8a9a8a'),
    'gentian': ('cup', '#2a4ad8', '#1a2a8a', '#4e7a3e'),
    'lupine': ('spike', '#6a7ae8', '#3a4ab0', '#4e8a46'),
    'clover': ('ball', '#e8a0b8', '#b86a8a', '#4e8a36'),
    'chamomile': ('star', '#f4f4ec', '#f0c830', LEAF),
    'yarrow': ('dot', '#eeeae0', '#c8c4b8', '#5a8a4a'),
    'thistle': ('ball', '#a050c0', '#6a2a8a', '#5a7a52'),
    'iris': ('star', '#6a4ad0', '#e8c840', '#4e7a5a'),
    'marsh_marigold': ('star', '#f6d42a', '#d89a18', '#3e7a3a'),
    'desert_bloom': ('star', '#f08c3a', '#c05a1a', '#7a8a6a'),
    'hibiscus': ('star', '#e02a3a', '#f0d040', '#2e6a2e'),
    'bird_of_paradise': ('spike', '#f88a2a', '#3a5ad0', '#2e6a3a'),
    'moth_orchid': ('star', '#f4c4dc', '#c0408a', '#3e7a3a'),
    'snowdrop': ('bell', '#f4f6f4', '#c8d8c8', '#5a8a6a'),
    'crocus': ('cup', '#9a5ad8', '#f09a20', LEAF),
    'primrose': ('star', '#f2e88a', '#f0a030', LEAF),
    'aster': ('star', '#9a80e0', '#e8c040', LEAF),
    'goldenrod': ('spike', '#ecc22a', '#b88a10', '#5a8a3a'),
    'fireweed': ('spike', '#e060b0', '#a03080', '#4e7a3e'),
    'forget_me_not': ('cross', '#6aa0f0', '#f0d040', LEAF),
    'sweet_violet': ('cross', '#6a3ab8', '#f0d040', '#3e7a3a'),
    'columbine': ('bell', '#7a6ae0', '#f4f4f0', '#5a8a5a'),
    'peony': ('ball', '#ec80aa', '#c04a7a', '#3e7a3a'),
    'cosmos': ('star', '#e870b0', '#f0c030', LEAF),
    'zinnia': ('star', '#e8402a', '#f0c030', LEAF),
    'bellflower': ('bell', '#8a70e0', '#5a40b0', LEAF),
    'saxifrage': ('cross', '#f0b0c8', '#e8c040', '#5a8a6a'),
    'st_johns_wort': ('star', '#f6d02a', '#c89018', LEAF),
    'thyme': ('dot', '#b898e0', '#8a6ab8', '#4e6a3e'),
}
# Clump sizes: big flowers make bigger, taller clumps; tiny ones small low cushions.
SIZE = {
    'sunflower': (26, 12, 22, 2), 'foxglove': (30, 9, 22, 2), 'lupine': (30, 10, 20, 2),
    'hibiscus': (24, 12, 18, 2), 'peony': (24, 12, 16, 1), 'goldenrod': (30, 11, 20, 2),
    'fireweed': (28, 10, 21, 2), 'bird_of_paradise': (18, 10, 18, 2), 'iris': (24, 9, 18, 3),
    'saxifrage': (30, 11, 9, 2), 'thyme': (40, 12, 9, 1), 'clover': (18, 12, 9, 2),
    'forget_me_not': (40, 11, 12, 3), 'sweet_violet': (22, 10, 10, 2), 'crocus': (14, 8, 10, 2),
    'snowdrop': (16, 8, 13, 2), 'azure_bluet': (34, 10, 12, 3), 'yarrow': (40, 10, 16, 2),
}
for name, (shape, petal, centre, leaf) in F.items():
    n, w, h, loose = SIZE.get(name, (34, 11, 15, 3))
    if shape in ('star', 'ball', 'cup'):
        n = int(n * 0.6)  # bigger blossoms: fewer of them, so each stays distinct
    C[name] = (clump(shape, n, w, h, loose, seed=len(name) * 7 + 3), dict(petal=petal, centre=centre, leaf=leaf))

GRASS = {
    'short_grass': (grass(17, 7, 19, seed=5), dict(leaf='#8c8c8c')),
    'tall_grass_bottom': (grass(22, 20, 32, tips=False, seed=6), dict(leaf='#8c8c8c')),
    'tall_grass_top': (tall_top(16, seed=7), dict(leaf='#8c8c8c')),
}
