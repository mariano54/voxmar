"""Writes assets/packs/default/recipes/wild.ron from wild.py (32×32 wild plants)."""
import os, re, sys
sys.path.insert(0, os.path.dirname(__file__))
import wild
SHADES = {'leaf': 6, 'petal': 5, 'centre': 3, 'second': 5, 'tuft': 3, 'fruit': 4, 'wood': 4, 'glow': 3}
LIGHT = {'leaf': 0.42, 'petal': 0.42, 'fruit': 0.42, 'wood': 0.3, 'centre': 0.3, 'second': 0.3, 'tuft': 0.3, 'glow': 0.25}
def book(entries, header, path):
    out = [header, '''(
    legend: {
        'a': "leaf:0", 'b': "leaf:1", 'c': "leaf:2", 'd': "leaf:3", 'e': "leaf:4", 'f': "leaf:5",
        'p': "petal:0", 'q': "petal:1", 'r': "petal:2", 's': "petal:3", 't': "petal:4",
        'x': "centre:0", 'y': "centre:1", 'z': "centre:2",
        'j': "second:0", 'k': "second:1", 'l': "second:2", 'm': "second:3", 'n': "second:4",
        'u': "tuft:0", 'v': "tuft:1", 'w': "tuft:2",
        'g': "fruit:0", 'h': "fruit:1", 'i': "fruit:2", 'o': "fruit:3",
        'A': "wood:0", 'B': "wood:1", 'D': "wood:2", 'E': "wood:3",
        'G': "glow:0", 'H': "glow:1", 'I': "glow:2",
    },
    textures: {''']
    for name, (fn, colours) in entries.items():
        colours = dict(colours)
        for slot in SHADES: colours.setdefault(slot, colours.get('petal', colours.get('leaf', '#808080')))
        pal = ", ".join(f'"{k}": (ramp: "{v}", shades: {SHADES[k]}, light: {LIGHT[k]})' for k, v in colours.items())
        out.append(f'        "block/{name}": (size: (32, 32), palettes: {{{pal}}}, layers: [Grid(rows: [')
        out += [f'            "{r}",' for r in fn().rows()]
        out.append('        ])]),')
    out.append('    },\n)\n')
    open(path, 'w').write("\n".join(out))
if __name__ == '__main__':
    book(wild.W, '''// Wild plants at 32×32: vegetables, grains, pulses, berry bushes, herbs, grasses, ferns,
// fungi and shrubs, drawn by tools/plants/wild.py from a few shapes in each plant's own
// colours (a hue-shifted ramp around each key colour). Regenerate with
// `python3 tools/plants/emit_wild.py`; edit the Python, not this file.''', sys.argv[1] if len(sys.argv) > 1 else 'assets/packs/default/recipes/wild.ron')
