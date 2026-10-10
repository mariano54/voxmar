"""Cuts the recorded shots into the 30-second trailer, with title cards and music.

    python3 tools/trailer/edit.py OUT_DIR CARDS_DIR TRAILER.mp4

OUT_DIR is shoot.py's (its clips/), CARDS_DIR cards.py's. Each section of TIMELINE is a
montage of shots, with its card over the first couple of seconds; the music is a stretch
of one of the game's own tracks, ending with it.
"""

import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
MUSIC = ROOT / "assets/music/old_forest.mp3"
MUSIC_FROM = 120.0

# (card or None, seconds the card stays, [(shot, seconds into it, seconds used), ...])
TIMELINE = [
    ("title", 2.7, [("peaks", 0.2, 3.5)]),
    ("biomes", 1.7, [("ocean", 0.3, 1.0), ("cherry", 0.4, 0.8), ("gorge", 0.3, 0.7), ("silverwood", 0.4, 0.95), ("village", 0.4, 1.05)]),
    ("creatures", 1.7, [("deer", 1.0, 1.2), ("wolves", 0.2, 1.0), ("bear", 0.3, 0.9), ("magic", 0.6, 0.9)]),
    ("skills", 1.7, [("character", 0.2, 1.8), ("guide", 0.2, 1.7)]),
    ("combat", 1.3, [("combat", 0.6, 2.0), ("combat_night", 1.3, 1.5)]),
    ("factories", 1.7, [("factory_wide", 0.2, 1.8), ("factory_line", 0.4, 1.4), ("factory_top", 0.4, 1.3)]),
    ("quests", 1.7, [("quests", 0.2, 1.3), ("inventory", 0.2, 1.1), ("crafting", 0.2, 1.1)]),
    ("end", 2.4, [("moonlit", 0.2, 3.0)]),
]
FADE = 0.25


def main():
    out, cards, trailer = Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3])
    inputs, filters, pieces, overlays = [], [], [], []

    def add(*args):
        """Adds an input, returning its index."""
        inputs.append(list(args))
        return len(inputs) - 1

    at = 0.0
    for card, hold, shots in TIMELINE:
        if card:
            overlays.append((card, at + 0.1, hold))
        for name, start, length in shots:
            i = add("-i", str(out / "clips" / f"{name}.mp4"))
            filters.append(f"[{i}:v]trim=start={start}:duration={length},setpts=PTS-STARTPTS,fps=30,format=yuv420p[p{i}]")
            pieces.append(f"[p{i}]")
            at += length
    total = at
    filters.append("".join(pieces) + f"concat=n={len(pieces)}:v=1:a=0[base]")
    last = "base"
    for k, (card, start, hold) in enumerate(overlays):
        i = add("-loop", "1", "-t", f"{hold:.2f}", "-i", str(cards / f"{card}.png"))
        filters.append(
            f"[{i}:v]format=rgba,fade=in:st=0:d={FADE}:alpha=1,fade=out:st={hold - FADE:.2f}:d={FADE}:alpha=1,"
            f"setpts=PTS-STARTPTS+{start:.2f}/TB[c{k}]"
        )
        filters.append(f"[{last}][c{k}]overlay=eof_action=pass:format=auto[o{k}]")
        last = f"o{k}"
    filters.append(f"[{last}]fade=in:st=0:d=0.4,fade=out:st={total - 0.6:.2f}:d=0.6[video]")
    music = add("-ss", str(MUSIC_FROM), "-t", f"{total:.2f}", "-i", str(MUSIC))
    filters.append(f"[{music}:a]loudnorm=I=-14:TP=-1.5:LRA=11,afade=in:st=0:d=0.3,afade=out:st={total - 1.5:.2f}:d=1.5[audio]")
    subprocess.run(
        ["ffmpeg", "-v", "error", "-y", *[a for i in inputs for a in i], "-filter_complex", ";".join(filters),
         "-map", "[video]", "-map", "[audio]", "-t", f"{total:.2f}",
         "-c:v", "libx264", "-preset", "slow", "-crf", "17", "-pix_fmt", "yuv420p",
         "-c:a", "aac", "-b:a", "192k", "-movflags", "+faststart", str(trailer)],
        check=True,
    )
    print(f"{trailer}: {total:.1f} s")


if __name__ == "__main__":
    main()
