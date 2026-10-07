"""Draws Voxmar HD's armour and shield sprites at 32×32 and writes
assets/packs/voxmar_hd/recipes/armour.ron. Run `python3 tools/art/armour.py`; edit this
file, not the book.

One drawing per piece, as default draws them (assets/packs/default/recipes/armour.ron):
the `metal` slot is the plate, hide or wood, `trim` the bands and rivets, `strap` the
leather, each three shades dark to light, so default's recolours (hide hood, bronze helmet,
the copper, bronze and iron shields) take these drawings in their own palettes. Vanilla's
design language: a one-texel dark outline, lit from the top left.
"""
import math
import os

N = 32
SLOTS = ('metal', 'trim', 'strap')


class Sprite:
    def __init__(self, palettes):
        self.px = [[None] * N for _ in range(N)]
        self.palettes = palettes

    def put(self, x, y, slot, shade=0):
        x, y = int(x), int(y)
        if 0 <= x < N and 0 <= y < N:
            self.px[y][x] = (slot, max(0, min(2, shade)))

    def get(self, x, y):
        return self.px[y][x] if 0 <= x < N and 0 <= y < N else None

    def inside(self, x, y):
        p = self.get(x, y)
        return p is not None and p[0] != 'ink'

    def fill(self, mask, slot):
        """Fills a mask (a set of texels) lit from the top left: its top and left edges
        light, its bottom and right edges dark, the rest the middle shade."""
        for x, y in mask:
            up, left = (x, y - 1) in mask, (x - 1, y) in mask
            down, right = (x, y + 1) in mask, (x + 1, y) in mask
            far = (x + 2, y) in mask
            s = 2 if not up or not left else 0 if not down or not right or not far else 1
            self.put(x, y, slot, s)

    def ink(self):
        edge = [(x, y) for y in range(N) for x in range(N) if self.get(x, y) is None and any(
            self.inside(x + dx, y + dy) for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)))]
        for x, y in edge:
            self.px[y][x] = ('ink', 0)
        return self

    def recipe(self):
        letters = iter('abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ')
        legend, rows = {}, []
        for line in self.px:
            row = ''
            for p in line:
                if p is None:
                    row += '.'
                    continue
                if p not in legend:
                    legend[p] = next(letters)
                row += legend[p]
            rows.append(row)
        ink = lambda p: '#1a140e' if p[0] == 'ink' else f'{p[0]}:{p[1]}'
        leg = ', '.join(f"'{c}': \"{ink(p)}\"" for p, c in legend.items())
        pal = ', '.join(f'"{k}": "{v}"' for k, v in self.palettes.items())
        body = ''.join(f'            "{r}",\n' for r in rows)
        return f'(size: (32, 32), palettes: {{{pal}}}, layers: [Grid(legend: {{{leg}}}, rows: [\n{body}        ])])'


def poly(points):
    """The texels whose centres lie inside a polygon."""
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


def rect(x0, y0, x1, y1):
    return {(x, y) for y in range(y0, y1 + 1) for x in range(x0, x1 + 1)}


def mirror(mask):
    return mask | {(N - 1 - x, y) for x, y in mask}


def mail(s, mask):
    """Riveted mail: rows of small rings, each lit on its upper left, staggered row to row."""
    for x, y in mask:
        p = s.get(x, y)
        shade = p[1] if p else 1
        u, v = (x + (y // 2) % 2) % 2, y % 2
        ring = 2 if (u, v) == (0, 0) else 0 if (u, v) == (1, 1) else 1
        s.put(x, y, 'metal', 0 if shade == 0 and ring else ring if shade else min(ring, 1))


def scales(s, mask):
    """Overlapping scales, rows hanging over the row below, each lit at its top."""
    for x, y in mask:
        row = y // 3
        u = (x + 2 * (row % 2)) % 4
        v = y % 3
        p = s.get(x, y)
        edge = p[1] if p else 1
        if v == 2 and u in (0, 3):
            s.put(x, y, 'metal', 0)
        elif v == 0 and u in (1, 2):
            s.put(x, y, 'metal', 2)
        else:
            s.put(x, y, 'metal', 1 if edge else 0)


DRAWINGS = {}


def drawing(name, metal, trim, strap):
    def wrap(fn):
        def draw():
            s = Sprite({'metal': metal, 'trim': trim, 'strap': strap})
            fn(s)
            return s.ink()
        DRAWINGS[name] = draw
        return fn
    return wrap


# Helmets --------------------------------------------------------------------------------

def dome():
    return ellipse(16, 15.5, 11.5, 11) & rect(0, 0, 31, 17) | mirror(rect(4, 15, 9, 25)) | mirror(poly([(4, 25), (10, 25), (9, 28), (6, 28)]))


@drawing('copper_helmet', 'armour_copper', 'armour_trim', 'armour_leather')
def _(s):
    shell = dome()
    s.fill(shell, 'metal')
    # A raised ridge from crown to brow, lit on its left.
    for y in range(5, 15):
        s.put(15, y, 'metal', 2)
        s.put(16, y, 'metal', 0)
    # The brow band, riveted.
    band = rect(4, 14, 27, 16)
    s.fill(band, 'trim')
    for x in (7, 12, 19, 24):
        s.put(x, 15, 'trim', 2)
    # Cheek guards either side, each riveted, a leather strap hanging from one.
    for x in (6, 25):
        s.put(x, 20, 'trim', 2)
    for y in range(26, 30):
        s.put(23, y, 'strap', 1 if y % 2 else 0)


@drawing('chainmail_coif', 'armour_chain', 'armour_leather', 'armour_leather')
def _(s):
    # A mail hood: a crown, long sides falling to the shoulders and flaring there, the
    # face open down to the chin.
    hood = ellipse(16, 14, 12, 11.5) & rect(0, 0, 31, 14) | mirror(rect(4, 14, 11, 24)) | mirror(poly([(4, 23), (12, 23), (13, 29), (1, 29)]))
    face = ellipse(16, 20, 4.5, 6) | rect(12, 20, 19, 31)
    hood -= face
    s.fill(hood, 'metal')
    mail(s, hood)
    # A leather edge round the face, and along the mantle's hem.
    for x, y in (ellipse(16, 20, 5.5, 7) | rect(11, 20, 20, 31)) - face:
        if (x, y) in hood:
            s.put(x, y, 'trim', 1 if x < 16 else 0)
    for x in range(1, 31):
        if (x, 28) in hood:
            s.put(x, 28, 'trim', 0)


# Body -----------------------------------------------------------------------------------

def shirt():
    return poly([(9, 3), (13, 4), (19, 4), (23, 3), (30, 8), (30, 15), (25, 15), (25, 29), (7, 29), (7, 15), (2, 15), (2, 8)])


def neck():
    return ellipse(16, 3.5, 4, 3)


@drawing('copper_chestplate', 'armour_copper', 'armour_trim', 'armour_leather')
def _(s):
    body = shirt() - neck()
    s.fill(body, 'metal')
    # The breastplate's keel, lit on the left, and a shadow under the chest.
    for y in range(7, 21):
        s.put(15, y, 'metal', 2)
        s.put(16, y, 'metal', 0)
    for x in range(9, 24):
        if x not in (15, 16):
            s.put(x, 17, 'metal', 0)
    # Sleeve cuffs and a belt with a buckle.
    for y in (13, 14):
        for x in list(range(2, 8)) + list(range(25, 31)):
            s.put(x, y, 'trim', 2 - (y - 13))
    for y in (22, 23):
        for x in range(7, 26):
            s.put(x, y, 'strap', 1 if y == 22 else 0)
    s.fill(rect(14, 21, 17, 24), 'trim')
    for x in (10, 21):
        s.put(x, 9, 'trim', 2)


@drawing('bronze_scale_shirt', 'armour_bronze', 'armour_trim', 'armour_leather')
def _(s):
    body = shirt() - neck()
    s.fill(body, 'metal')
    scales(s, body)
    for y in (22, 23):
        for x in range(7, 26):
            s.put(x, y, 'strap', 1 if y == 22 else 0)
    for x, y in ellipse(16, 3.5, 5, 4) - neck():
        if (x, y) in body:
            s.put(x, y, 'trim', 1)


@drawing('chainmail_shirt', 'armour_chain', 'armour_leather', 'armour_leather')
def _(s):
    body = shirt() - neck()
    s.fill(body, 'metal')
    mail(s, body)
    for y in (22, 23):
        for x in range(7, 26):
            s.put(x, y, 'strap', 1 if y == 22 else 0)
    s.fill(rect(14, 21, 17, 24), 'strap')
    for x, y in ellipse(16, 3.5, 5, 4) - neck():
        if (x, y) in body:
            s.put(x, y, 'trim', 2 if x < 16 else 1)


# Legs -----------------------------------------------------------------------------------

def legs():
    return poly([(7, 3), (25, 3), (26, 29), (18, 29), (16, 12), (14, 29), (6, 29)])


@drawing('copper_leggings', 'armour_copper', 'armour_trim', 'armour_leather')
def _(s):
    m = legs()
    s.fill(m, 'metal')
    s.fill(rect(7, 3, 25, 6) & m, 'trim')
    for x in (10, 16, 22):
        s.put(x, 4, 'trim', 2)
    # Knee plates.
    for cx in (10, 22):
        for x, y in ellipse(cx, 19, 3, 2.5) & m:
            s.put(x, y, 'metal', 2 if y < 19 else 1)
    for y in range(27, 30):
        for x in range(6, 27):
            if (x, y) in m:
                s.put(x, y, 'trim', 1 if y == 27 else 0)


@drawing('chainmail_leggings', 'armour_chain', 'armour_leather', 'armour_leather')
def _(s):
    m = legs()
    s.fill(m, 'metal')
    mail(s, m)
    s.fill(rect(7, 3, 25, 5) & m, 'trim')


# Hands ----------------------------------------------------------------------------------

def hand():
    palm = poly([(8, 13), (24, 13), (25, 24), (21, 28), (11, 28), (8, 23)])
    fingers = set()
    for i, (x, top) in enumerate(((9, 5), (13, 3), (17, 4), (21, 6))):
        fingers |= rect(x, top, x + 2, 14)
    thumb = poly([(24, 15), (29, 11), (31, 13), (26, 21)])
    cuff = rect(9, 25, 23, 30)
    return palm | fingers | thumb | cuff, cuff


@drawing('copper_gauntlets', 'armour_copper', 'armour_trim', 'armour_leather')
def _(s):
    m, cuff = hand()
    s.fill(m, 'metal')
    # Finger plates: a dark joint every few texels.
    for x0 in (9, 13, 17, 21):
        for y in (8, 11):
            for x in range(x0, x0 + 3):
                if (x, y) in m:
                    s.put(x, y, 'metal', 0)
    for x in range(9, 24):
        s.put(x, 16, 'metal', 0)
    s.fill(cuff, 'trim')
    for x in (12, 20):
        s.put(x, 27, 'trim', 2)


@drawing('chainmail_gloves', 'armour_chain', 'armour_leather', 'armour_leather')
def _(s):
    m, cuff = hand()
    s.fill(m, 'metal')
    mail(s, m - cuff)
    s.fill(cuff, 'strap')


# Feet -----------------------------------------------------------------------------------

def boot():
    shaft = rect(8, 3, 17, 22)
    foot = poly([(8, 20), (17, 18), (25, 21), (28, 24), (28, 28), (8, 28)])
    return shaft | foot


@drawing('copper_boots', 'armour_copper', 'armour_trim', 'armour_leather')
def _(s):
    m = boot()
    s.fill(m, 'metal')
    s.fill(rect(8, 3, 17, 6), 'trim')
    # A plate over the instep and a dark sole.
    for x, y in poly([(17, 19), (24, 21), (24, 24), (17, 24)]) & m:
        s.put(x, y, 'metal', 2 if y == 19 or x == 17 else 1)
    for x in range(8, 29):
        s.put(x, 27, 'strap', 0)
        s.put(x, 28, 'strap', 0)
    s.put(10, 9, 'metal', 2)


@drawing('chainmail_boots', 'armour_chain', 'armour_leather', 'armour_leather')
def _(s):
    m = boot()
    s.fill(m, 'metal')
    mail(s, m)
    s.fill(rect(8, 3, 17, 5), 'trim')
    for x in range(8, 29):
        s.put(x, 27, 'strap', 1)
        s.put(x, 28, 'strap', 0)


# Shields --------------------------------------------------------------------------------

@drawing('hide_buckler', 'armour_hide', 'armour_wood', 'armour_leather')
def _(s):
    m = ellipse(16, 16, 13, 13)
    s.fill(m, 'metal')
    # Stitched hide panels over a wooden rim, a wooden boss.
    for x, y in m - ellipse(16, 16, 11, 11):
        s.put(x, y, 'trim', 2 if x + y < 30 else 0 if x + y > 34 else 1)
    for a in range(0, 360, 45):
        for r in range(5, 11):
            if r % 2:
                s.put(16 + r * math.cos(math.radians(a)), 16 + r * math.sin(math.radians(a)), 'strap', 0)
    for x, y in ellipse(16, 16, 4, 4):
        s.put(x, y, 'trim', 2 if x + y < 31 else 1 if x + y < 34 else 0)


@drawing('wooden_shield', 'armour_wood', 'armour_iron', 'armour_leather')
def _(s):
    m = poly([(3, 2), (29, 2), (29, 15), (26, 23), (16, 30), (6, 23), (3, 15)])
    s.fill(m, 'metal')
    inner = poly([(6, 5), (26, 5), (26, 15), (23.5, 21.5), (16, 27), (8.5, 21.5), (6, 15)])
    for x, y in m - inner:
        s.put(x, y, 'trim', 2 if x < 16 and y < 20 else 1 if y < 20 else 0)
    # Boards, down the face.
    for x0 in (11, 16, 21):
        for y in range(5, 28):
            if (x0, y) in inner:
                s.put(x0, y, 'metal', 0)
    for x, y in ellipse(16, 14, 4.5, 4.5):
        s.put(x, y, 'trim', 2 if x + y < 29 else 1 if x + y < 32 else 0)
    s.put(15, 12, 'trim', 2)
    for x, y in ((8, 7), (24, 7), (8, 17), (24, 17)):
        s.put(x, y, 'trim', 2)


def main():
    root = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', '..')
    out = ['''// Armour and shields at 32×32 (content: base/blocks_armour.ron), drawn by
// tools/art/armour.py: one drawing per piece in default's `metal`, `trim` and `strap`
// slots, so default's recolours take them in their own palettes. Regenerate with
// `python3 tools/art/armour.py`; edit the Python, not this file.
(
    textures: {''']
    for name, draw in DRAWINGS.items():
        out.append(f'        "block/{name}": {draw().recipe()},')
    out.append('    },\n)\n')
    path = os.path.join(root, 'assets/packs/voxmar_hd/recipes/armour.ron')
    open(path, 'w').write('\n'.join(out))
    print(len(DRAWINGS), 'drawings ->', path)


if __name__ == '__main__':
    main()
