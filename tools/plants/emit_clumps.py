"""Writes recipes/flowers.ron: every flower as a clump of tiny blossoms, and fine grass
(clumps.py). Names Minecraft also uses get our own texture name (`poppy_blooms`,
`fine_grass`), so a Minecraft jar on the texture stack never stands in for them; the plain
name stays as an alias for stamps and recipes built on it."""
import os, sys
sys.path.insert(0, os.path.dirname(__file__))
import clumps, emit_wild
VANILLA = {'dandelion', 'poppy', 'cornflower', 'oxeye_daisy', 'azure_bluet', 'allium', 'blue_orchid',
           'lily_of_the_valley', 'red_tulip', 'orange_tulip', 'white_tulip', 'pink_tulip'}
OWN = {n: f'{n}_blooms' for n in VANILLA}
OWN.update({'short_grass': 'fine_grass', 'tall_grass_bottom': 'tall_fine_grass_bottom', 'tall_grass_top': 'tall_fine_grass_top'})
if __name__ == '__main__':
    out = sys.argv[1] if len(sys.argv) > 1 else 'assets/packs/default/recipes/flowers.ron'
    entries = {}
    for name, entry in list(clumps.C.items()) + list(clumps.GRASS.items()):
        entries[OWN.get(name, name)] = entry
    emit_wild.book(entries, '''// Flowers as clumps of tiny blossoms, and fine grass, at 32×32, drawn by
// tools/plants/clumps.py. Regenerate with `python3 tools/plants/emit_clumps.py`; edit the
// Python, not this file.''', out)
    with open(out) as f: book = f.read()
    aliases = "".join(f'        "block/{n}": (base: "block/{own}"),\n' for n, own in OWN.items())
    book = book.replace("    },\n)", "        // The plain names, for recipes and stamps built on them.\n" + aliases + "    },\n)", 1)
    with open(out, 'w') as f: f.write(book)
