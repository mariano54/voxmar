"""Draws the parts of the shaped furnishings' faces that must sit where their shape's boxes
are (assets/content/base/shapes.ron) and writes assets/packs/default/recipes/shaped.ron.
Run `python3 tools/art/shaped.py` after changing a shape; edit this file, not the book.

A shaped block's boxes take their texture as Minecraft's block models do, each face the
part of the texture under its place in the cell (voxmar_engine::shape::box_face). So a
barrel's iron hoops, a crate's corner posts, a sack's tie and a lid's rim must be drawn at
the rows and columns of the boxes that stand for them, read from the shape table here,
over the rest of the face drawn in furnishings.ron (`shape/barrel_staves` and the like),
which does not depend on the boxes. Both packs draw these 32×32 faces.
"""
import os
import re

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', '..')
N = 32
K = N // 16


def shapes():
    """Every shape's boxes as ((x, y, z), (x, y, z)) in 16ths, from the shape table."""
    text = open(os.path.join(ROOT, 'assets/content/base/shapes.ron')).read()
    out = {}
    for m in re.finditer(r'"(\w+)": \(boxes: \[(.*?)\]\)', text, re.S):
        boxes = []
        for b in re.finditer(r'from: \(([^)]*)\), to: \(([^)]*)\)', m.group(2)):
            boxes.append(tuple(tuple(float(v) for v in g.split(',')) for g in b.groups()))
        out[m.group(1)] = boxes
    return out


def rows_of(y0, y1):
    """The texture rows a box spanning heights y0..y1 (16ths, from the bottom) covers."""
    return range(int((16 - y1) * K), int(round((16 - y0) * K)))


def grid(cells, legend):
    """A 32×32 Grid layer painting `cells` ({(x, y): letter}), clear elsewhere."""
    rows = [''.join(cells.get((x, y), '.') for x in range(N)) for y in range(N)]
    leg = ', '.join(f"'{k}': \"{v}\"" for k, v in legend.items())
    body = ''.join(f'                "{r}",\n' for r in rows)
    return f'Grid(legend: {{{leg}}}, rows: [\n{body}            ])'


def barrel(boxes):
    """Hoops at the heights of the barrel's thin boxes, lit along their top and darkened
    along their bottom; on the lid, an iron rim round the staves' footprint."""
    staves = [b for b in boxes if b[1][1] - b[0][1] >= 8]
    hoops = sorted({(b[0][1], b[1][1]) for b in boxes if b[1][1] - b[0][1] < 3})
    side = {}
    for y0, y1 in hoops:
        rows = list(rows_of(y0, y1))
        for i, y in enumerate(rows):
            shade = 'h' if i == 0 else 'd' if i == len(rows) - 1 else 'm'
            for x in range(N):
                side[(x, y)] = shade
        for x in range(N):
            side[(x, rows[-1] + 1)] = 's'
    inside = {(x, z) for x in range(N) for z in range(N)
              if any(b[0][0] * K <= x < b[1][0] * K and b[0][2] * K <= z < b[1][2] * K for b in staves)}
    top = {}
    for x, z in inside:
        if any((x + dx, z + dz) not in inside for dx in (-1, 0, 1) for dz in (-1, 0, 1)):
            top[(x, z)] = 'h' if x + z < N else 'd'
        elif any((x + dx, z + dz) not in inside for dx in (-2, 0, 2) for dz in (-2, 0, 2)):
            top[(x, z)] = 'm'
    legend = {'h': 'hoop:3', 'm': 'hoop:2', 'd': 'hoop:1', 's': 'main:0'}
    return {
        'barrel_side': ('shape/barrel_staves', grid(side, legend)),
        'barrel_top': ('shape/barrel_lid', grid(top, legend)),
    }


def crate(boxes):
    """Corner posts down the columns where the crate's corner boxes stand, lit on their
    left edge and shadowed on their right, nailed top and bottom."""
    posts = sorted({(b[0][0], b[1][0]) for b in boxes if b[1][0] - b[0][0] <= 3})
    side = {}
    for x0, x1 in posts:
        cols = range(int(x0 * K), int(round(x1 * K)))
        for i, x in enumerate(cols):
            shade = 'h' if i == 0 else 'd' if i == len(cols) - 1 else 'm'
            for y in range(N):
                side[(x, y)] = shade
        mid = int((x0 + x1) / 2 * K)
        for y in (2, N - 3):
            side[(mid, y)] = 'n'
    legend = {'h': 'main:4', 'm': 'main:3', 'd': 'main:1', 'n': 'nail:2'}
    return {'crate_side': ('shape/crate_slats', grid(side, legend))}


def sack(boxes):
    """The tie: a twine band round the box just above the sack's narrowest neck box, the
    cloth gathered above and below it darkened where it bunches."""
    upper = [b for b in boxes if b[0][1] >= 8]
    neck = min(upper, key=lambda b: b[1][0] - b[0][0])
    tie = min((b for b in upper if b[0][1] >= neck[1][1] - 1e-6), key=lambda b: b[0][1])
    rows = list(rows_of(tie[0][1], tie[1][1]))
    side = {}
    for i, y in enumerate(rows):
        for x in range(N):
            side[(x, y)] = 'h' if i == 0 else 'd'
    for x in range(N):
        side[(x, rows[0] - 1)] = 's'
        side[(x, rows[-1] + 1)] = 's'
    legend = {'h': 'tie:1', 'd': 'tie:0', 's': 'main:0'}
    return {'sack_side': ('shape/sack_cloth', grid(side, legend))}


def main():
    table = shapes()
    faces = {}
    faces.update(barrel(table['barrel']))
    faces.update(crate(table['crate']))
    faces.update(sack(table['sack']))
    out = ['''// The shaped furnishings' faces where their boxes are (content/base/shapes.ron), drawn
// by tools/art/shaped.py over the rest of each face in furnishings.ron. Regenerate with
// `python3 tools/art/shaped.py` after changing a shape; edit the Python, not this file.
(
    textures: {''']
    for name, (base, layer) in faces.items():
        out.append(f'        "block/{name}": (base: "{base}", layers: [\n            {layer},\n        ]),')
    out.append('    },\n)\n')
    path = os.path.join(ROOT, 'assets/packs/default/recipes/shaped.ron')
    open(path, 'w').write('\n'.join(out))
    print(len(faces), 'faces ->', path)


if __name__ == '__main__':
    main()
