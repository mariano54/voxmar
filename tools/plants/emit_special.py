"""Writes the cave flora and supernatural plant books at 32×32 from special.py."""
import os, sys
sys.path.insert(0, os.path.dirname(__file__))
import special, emit_wild
if __name__ == '__main__':
    out = sys.argv[1] if len(sys.argv) > 1 else 'assets/packs/default/recipes'
    emit_wild.book(special.CAVE, '''// Gatherable cave flora at 32×32, drawn by tools/plants/special.py. Regenerate with
// `python3 tools/plants/emit_special.py`; edit the Python, not this file.''', os.path.join(out, 'cave_flora.ron'))
    emit_wild.book(special.SUPER, '''// The supernatural plants at 32×32 (docs/systems-web.md), drawn by tools/plants/special.py
// from the shapes of ordinary plants in strange colours, with a few faint glints. Regenerate
// with `python3 tools/plants/emit_special.py`; edit the Python, not this file.''', os.path.join(out, 'supernatural_art.ron'))
