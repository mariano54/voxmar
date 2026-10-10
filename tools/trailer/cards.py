# /// script
# dependencies = ["pillow"]
# ///
"""Title cards for the trailer, drawn in Voxmar's own pixel font (assets/ui/fonts/voxmar.txt).

    uv run tools/trailer/cards.py OUT_DIR

Writes one transparent 1920x1080 PNG per card in CARDS: a big headline in the middle of the
screen and a smaller line under it, with a dark outline and a soft shadow so they read over
any footage.
"""

import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

ROOT = Path(__file__).resolve().parents[2]
FONT = ROOT / "assets/ui/fonts/voxmar.txt"
W, H = 1920, 1080

# name: (headline, line under it)
CARDS = {
    "title": ("VOXMAR", "an endless voxel world"),
    "biomes": ("68 BIOMES", "mountains, rivers, seas, caves and stranger lands"),
    "creatures": ("50 CREATURES", "tame, breed and hunt animals and monsters"),
    "skills": ("14 SKILLS", "level each one to 99"),
    "combat": ("TIMED COMBAT", "well-timed blows strike harder"),
    "factories": ("BUILD FACTORIES", "water wheels, steam and electricity"),
    "quests": ("QUESTS & CRAFTING", "an age-by-age quest book"),
    "end": ("VOXMAR", "open source  ·  moddable  ·  multiplayer"),
}


def load_font(path):
    """Glyphs as lists of rows of booleans; lower case and descenders keep their offsets."""
    glyphs, name, rows = {}, None, []
    lines = path.read_text().splitlines()

    def done():
        if name is not None:
            glyphs[name] = rows

    for line in lines:
        if line.startswith("//") or not line.strip():
            continue
        if line.startswith("= "):
            done()
            parts = line[2:].split()
            if parts[0] == "space":
                glyphs[" "] = int(parts[1])
                name, rows = None, []
                continue
            name, rows = parts[0], []
        elif name is not None and set(line) <= set("#."):
            rows.append([c == "#" for c in line])
        else:
            done()
            name, rows = None, []
    done()
    return glyphs


def text_mask(glyphs, text, scale):
    """The text as a white-on-transparent image, `scale` screen pixels per font pixel."""
    cells = []
    width = 0
    for ch in text:
        g = glyphs.get(ch)
        if isinstance(g, int):
            cells.append((width, None, 0))
            width += g + 1
            continue
        if g is None:
            g = glyphs.get("?")
        # Every glyph's rows start at a capital's top, lower case and marks padded down.
        cells.append((width, g, 0))
        width += max(len(r) for r in g) + 1
    height = 11
    img = Image.new("L", (width * scale, height * scale), 0)
    d = ImageDraw.Draw(img)
    for x0, g, top in cells:
        if g is None:
            continue
        for y, row in enumerate(g):
            for x, on in enumerate(row):
                if on:
                    px, py = (x0 + x) * scale, (top + y) * scale
                    d.rectangle([px, py, px + scale - 1, py + scale - 1], fill=255)
    return img


def outlined(mask, colour, outline, step):
    """Text in `colour` with a hard pixel outline `step` wide and a blurred shadow."""
    pad = step * 6
    w, h = mask.size[0] + pad * 2, mask.size[1] + pad * 2
    base = Image.new("L", (w, h), 0)
    base.paste(mask, (pad, pad))
    ring = base.filter(ImageFilter.MaxFilter(step * 2 + 1))
    shadow = ring.filter(ImageFilter.GaussianBlur(step * 3))
    out = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    out.paste(Image.new("RGBA", (w, h), (0, 0, 0, 150)), (step, step * 2), shadow)
    out.paste(Image.new("RGBA", (w, h), outline), (0, 0), ring)
    out.paste(Image.new("RGBA", (w, h), colour), (0, 0), base)
    return out


def card(glyphs, head, sub):
    img = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    # A soft dark band behind the words, so they read over bright skies and snow.
    band = Image.new("L", (W, H), 0)
    ImageDraw.Draw(band).rectangle([0, H // 2 - 170, W, H // 2 + 150], fill=120)
    band = band.filter(ImageFilter.GaussianBlur(70))
    img.paste(Image.new("RGBA", (W, H), (8, 10, 18, 255)), (0, 0), band)

    scale = 16 if len(head) <= 12 else 13
    h = outlined(text_mask(glyphs, head, scale), (255, 236, 170, 255), (24, 18, 10, 255), 5)
    img.alpha_composite(h, ((W - h.size[0]) // 2, H // 2 - 60 - h.size[1] // 2))
    s = outlined(text_mask(glyphs, sub, 5), (236, 240, 248, 255), (12, 14, 22, 255), 3)
    img.alpha_composite(s, ((W - s.size[0]) // 2, H // 2 + 95 - s.size[1] // 2))
    return img


def main():
    out = Path(sys.argv[1])
    out.mkdir(parents=True, exist_ok=True)
    glyphs = load_font(FONT)
    for name, (head, sub) in CARDS.items():
        card(glyphs, head, sub).save(out / f"{name}.png")
        print(out / f"{name}.png")


if __name__ == "__main__":
    main()
