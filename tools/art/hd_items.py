"""Draws Voxmar HD's 32×32 item sprites that have no recipe of their own yet (coins, the
coin purse) and writes assets/packs/voxmar_hd/recipes/items.ron. Run
`python3 tools/art/hd_items.py`; edit this file, not the book. Same language as the 16×16
sprites: a one-texel dark outline, three or four shades lit from the top left."""
import math
import os

N = 32
INK = '#1a140e'
GOLD = ['#6a4008', '#a87418', '#d8a830', '#f4d264', '#fff2b0']
LEATHER = ['#3e2814', '#5a3a20', '#7a5230', '#9a6c40', '#b88a58']
TWINE = ['#8a7048', '#c0a470', '#e0cc98']


class Sprite:
    def __init__(self):
        self.px = [[None] * N for _ in range(N)]

    def put(self, x, y, colour):
        x, y = int(round(x)), int(round(y))
        if 0 <= x < N and 0 <= y < N:
            self.px[y][x] = colour

    def ellipse(self, cx, cy, rx, ry, fill, shade=None):
        for y in range(N):
            for x in range(N):
                dx, dy = (x + 0.5 - cx) / rx, (y + 0.5 - cy) / ry
                if dx * dx + dy * dy <= 1:
                    self.put(x, y, shade(dx, dy) if shade else fill)

    def outline(self):
        """A one-texel ink outline round everything drawn, as vanilla's items have."""
        edge = []
        for y in range(N):
            for x in range(N):
                if self.px[y][x] is None and any(
                    0 <= x + dx < N and 0 <= y + dy < N and self.px[y + dy][x + dx] not in (None, INK)
                    for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1))
                ):
                    edge.append((x, y))
        for x, y in edge:
            self.px[y][x] = INK

    def recipe(self, comment):
        letters = iter('abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ')
        legend, rows = {}, []
        for line in self.px:
            row = ''
            for c in line:
                if c is None:
                    row += '.'
                    continue
                if c not in legend:
                    legend[c] = next(letters)
                row += legend[c]
            rows.append(row)
        leg = ', '.join(f"'{k}': \"{c}\"" for c, k in legend.items())
        body = ''.join(f'            "{r}",\n' for r in rows)
        return f'        // {comment}\n', f'(size: (32, 32), layers: [Grid(legend: {{{leg}}}, rows: [\n{body}        ])])'


def coin(s, cx, top, rx=6.5, stamped=False):
    """One coin seen from above and a little in front: a lit face over a rim, a dark line
    where it sits on the coin below."""
    ry = rx * 0.42
    for y in range(N):
        for x in range(N):
            dx = (x + 0.5 - cx) / rx
            if abs(dx) > 1:
                continue
            for yy, c in ((top + ry + 2, GOLD[0]), (top + ry + 1, GOLD[2])):
                dy = (y + 0.5 - yy) / ry
                if dx * dx + dy * dy <= 1:
                    s.put(x, y, GOLD[1] if (dx > 0.6 and c != GOLD[0]) else c)
    s.ellipse(cx, top + ry, rx, ry, None, lambda dx, dy: GOLD[4] if dx + dy < -0.8 else GOLD[3])
    if stamped:
        # A raised ring struck round the face, and a glint.
        for y in range(N):
            for x in range(N):
                dx, dy = (x + 0.5 - cx) / (rx - 2), (y + 0.5 - top - ry) / max(ry - 1, 1)
                if 0.55 <= dx * dx + dy * dy <= 1.0:
                    s.put(x, y, GOLD[2])
        s.put(cx - 2, top + ry - 1, '#ffffff')


def gold_coin():
    s = Sprite()
    for i, j in enumerate((0, 1, 0, -1)):
        coin(s, 8 + j, 24 - 3 * i - 4, stamped=(i == 3))
    for i, j in enumerate((0, -1, 0, 1, 0, 0)):
        coin(s, 20 + j, 24 - 3 * i - 4, rx=6, stamped=(i == 5))
    # One loose coin in front, flat on the ground.
    coin(s, 25, 24, rx=5, stamped=True)
    s.outline()
    return s


def coin_purse():
    s = Sprite()
    # The pouch: full and round below, lit top left.
    s.ellipse(16, 22, 11.5, 8.5, None, lambda dx, dy: LEATHER[4] if dx + dy < -1.0 else LEATHER[3] if dx + dy < -0.2 else LEATHER[2] if dx + dy < 0.7 else LEATHER[1])
    # Gathered tight at the neck, the frill flaring above it, scalloped at its lip.
    for y in range(4, 15):
        half = 2.5 if y >= 11 else 2.5 + (11 - y) * 0.75
        for x in range(N):
            d = x + 0.5 - 16
            if abs(d) <= half and not (y == 4 and int(d + 16) % 3 == 0):
                s.put(x, y, LEATHER[4] if d < -half + 1.5 else LEATHER[3] if d < 0 else LEATHER[2])
    for x in range(11, 22, 2):
        for y in range(6, 10):
            if s.px[y][x] is not None:
                s.put(x, y, LEATHER[1])
    # Gold heaped in the gaping mouth, two rows of it with a bright glint, so the purse
    # still reads as full of coins at slot size.
    for x, c in ((13, GOLD[2]), (14, GOLD[3]), (15, GOLD[4]), (16, '#ffffff'), (17, GOLD[4]), (18, GOLD[3])):
        s.put(x, 5, c)
    for x, c in ((14, GOLD[3]), (15, '#ffffff'), (16, GOLD[4]), (17, GOLD[3])):
        s.put(x, 6, c)
    # The cord, wound round the neck and tied, its two ends hanging.
    for y, c in ((11, TWINE[2]), (12, TWINE[1])):
        for x in range(12, 21):
            s.put(x, y, c)
    s.put(20, 11, TWINE[2]); s.put(21, 12, TWINE[1])
    for i in range(6):
        s.put(21 + i * 0.5, 13 + i, TWINE[2] if i % 2 else TWINE[1])
        s.put(19 + i * 0.3, 13 + i, TWINE[1])
    # A stitched seam down its belly.
    for y in range(16, 29, 2):
        s.put(15, y, LEATHER[0])
    s.outline()
    return s


SPRITES = {
    'block/gold_coin': (gold_coin, 'Gold coins: two short stacks, each coin lit on its face and rim, the top ones struck with a star.'),
    'block/coin_purse': (coin_purse, 'A coin purse: a full leather pouch gathered under a frill by a tied cord, gold glinting in its mouth.'),
}


def main():
    root = os.path.join(os.path.dirname(__file__), '..', '..')
    out = ['// Item sprites redrawn at 32×32 by tools/art/hd_items.py; edit the Python, not this file.\n(\n    textures: {\n']
    for name, (draw, comment) in SPRITES.items():
        c, r = draw().recipe(comment)
        out.append(c + f'        "{name}": {r},\n')
    out.append('    },\n)\n')
    path = os.path.join(root, 'assets/packs/voxmar_hd/recipes/items.ron')
    open(path, 'w').write(''.join(out))
    print(len(SPRITES), 'sprites ->', path)


if __name__ == '__main__':
    main()
