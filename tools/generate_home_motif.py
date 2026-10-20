#!/usr/bin/env python3
"""Composes the home motif: a short calm cue played the first time each night a player
steps inside their own lit home (music.ron, `home`).

Our own work: a lullaby in F major in three, a celesta singing over a harp's broken chords
and soft strings (F - Dm - Bb - C - F), about fifteen seconds with its ring. Played offline
by FluidSynth from the FluidR3 General MIDI sound font (Frank Wen, MIT; see
THIRD_PARTY_NOTICES.md), as the fanfares are (tools/generate_jingles.py, whose score and
player this uses). Written to assets/music/home/ as Ogg Vorbis; a package can replace it or
add others to the `home` category's folder.

    python3 tools/generate_home_motif.py
"""

import sys
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
from generate_jingles import RATE, ROOT, Score, perform, write_ogg  # noqa: E402

OUT = ROOT / "assets" / "music" / "home"
CELESTA, HARP, SLOW_STRINGS, FLUTE = 8, 46, 49, 73
BEAT = 0.8  # 75 beats a minute, three to a bar
# Under the other tracks as made: music.ron brings each to its loudness by its level.
RMS_DB = -20.0


def hearth_welcome() -> np.ndarray:
    score = Score([(0, CELESTA, 92, 70), (1, HARP, 88, 46), (2, SLOW_STRINGS, 70, 64), (3, FLUTE, 58, 80)])
    bars = [
        (["F2", "C3", "A3", "C4"], ["F3", "C4", "F4", "A4", "C5", "A4"]),
        (["D2", "A2", "F3", "A3"], ["D3", "A3", "D4", "F4", "A4", "F4"]),
        (["Bb1", "F2", "D3", "F3"], ["Bb2", "F3", "Bb3", "D4", "F4", "D4"]),
        (["C2", "G2", "E3", "G3"], ["C3", "G3", "C4", "E4", "G4", "Bb4"]),
    ]
    bar = 3 * BEAT
    for i, (pad, broken) in enumerate(bars):
        at = i * bar
        score.play(2, at, pad, bar + 0.1, 52)
        for j, name in enumerate(broken):
            score.play(1, at + j * BEAT / 2, name, BEAT * 1.5, 70 - 4 * (j % 2))
    melody = [
        (0, "A5", 1), (1, "C6", 1), (2, "F6", 1),
        (3, "E6", 2), (5, "D6", 1),
        (6, "D6", 1), (7, "C6", 1), (8, "Bb5", 1),
        (9, "G5", 1.5), (10.5, "A5", 0.5), (11, "Bb5", 1),
        (12, "A5", 4),
    ]
    for beat, name, beats in melody:
        score.play(0, beat * BEAT, name, beats * BEAT, 78 if beats < 4 else 70)
    # The last bar: home, the harp rolling up and a flute breathing the third.
    home = 4 * bar
    score.play(2, home, ["F2", "C3", "A3", "F4"], 3.2, 48)
    for j, name in enumerate(["F3", "A3", "C4", "F4", "A4", "C5"]):
        score.play(1, home + j * 0.09, name, 3.0, 64 + 2 * j)
    score.play(3, home + 0.4, "A5", 2.4, 50)
    return perform(score)


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    stereo = hearth_welcome()
    level = np.abs(stereo).max(axis=0)
    last = np.nonzero(level > level.max() * 10 ** (-50 / 20))[0][-1]
    stereo = stereo[:, : last + 1].copy()
    fade = int(0.8 * RATE)
    stereo[:, -fade:] *= np.linspace(1, 0, fade) ** 2
    rms = np.sqrt((stereo ** 2).mean())
    stereo *= 10 ** (RMS_DB / 20) / rms
    stereo *= min(1.0, 0.89 / np.abs(stereo).max())
    path = OUT / "hearth_welcome.ogg"
    write_ogg(stereo, path)
    print(f"{path.relative_to(ROOT)}: {stereo.shape[1] / RATE:.2f} s, peak {np.abs(stereo).max():.2f}", file=sys.stderr)


if __name__ == "__main__":
    main()
