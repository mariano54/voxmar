#!/usr/bin/env python3
"""Composes the settlement motif: a short bright cue played the first time each game day a
player steps within a settlement's bounds (music.ron, `settlement`).

Our own work: a market-day tune in D major in six-eight, a recorder over a plucked guitar
and a fiddle's drone (D - G - A - D), about ten seconds with its ring. Played offline by
FluidSynth from the FluidR3 General MIDI sound font (Frank Wen, MIT; see
THIRD_PARTY_NOTICES.md), as the home motif is (tools/generate_home_motif.py), whose
finishing this shares. Written to assets/music/settlement/ as Ogg Vorbis, the font's name
and SHA-256 in its comment tag and printed (VOXMAR_SF2 names another); a package can
replace it or add others to the `settlement` category's folder.

    python3 tools/generate_settlement_motif.py
"""

import hashlib
import sys
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
from generate_jingles import RATE, ROOT, Score, perform, sound_font, write_ogg  # noqa: E402

OUT = ROOT / "assets" / "music" / "settlement"
GUITAR, FIDDLE, RECORDER, PIZZICATO = 24, 40, 74, 45
EIGHTH = 0.22  # a lilting six-eight, two dotted beats to the bar
# Under the other tracks as made: music.ron brings each to its loudness by its level.
RMS_DB = -20.0


def market_day() -> np.ndarray:
    score = Score([(0, RECORDER, 92, 70), (1, GUITAR, 86, 50), (2, FIDDLE, 60, 84), (3, PIZZICATO, 70, 40)])
    bar = 6 * EIGHTH
    chords = [
        (["D3", "A3", "D4", "F#4"], "D2"),
        (["G2", "D3", "G3", "B3"], "G2"),
        (["A2", "E3", "A3", "C#4"], "A2"),
        (["D3", "A3", "D4", "F#4"], "D2"),
    ]
    for i, (chord, bass) in enumerate(chords):
        at = i * bar
        score.play(3, at, bass, EIGHTH * 2, 74)
        score.play(3, at + 3 * EIGHTH, bass, EIGHTH * 2, 64)
        # The guitar strums on each dotted beat, lightly between.
        for beat, velocity in [(0, 70), (2, 48), (3, 62), (5, 46)]:
            for j, name in enumerate(chord):
                score.play(1, at + beat * EIGHTH + j * 0.012, name, EIGHTH * 1.6, velocity)
        score.play(2, at, [chord[0], chord[1]], bar + 0.05, 44)
    melody = [
        (0, "A4", 2), (2, "D5", 1), (3, "F#5", 2), (5, "E5", 1),
        (6, "D5", 2), (8, "B4", 1), (9, "G4", 2), (11, "B4", 1),
        (12, "A4", 1), (13, "C#5", 1), (14, "E5", 1), (15, "A5", 2), (17, "G5", 1),
        (18, "F#5", 3), (21, "E5", 1), (22, "D5", 2),
    ]
    for eighth, name, eighths in melody:
        score.play(0, eighth * EIGHTH, name, eighths * EIGHTH * 0.95, 80)
    # The close: home on D, the guitar rolled and the fiddle holding the fifth.
    home = 4 * bar
    score.play(3, home, "D2", 2.6, 70)
    for j, name in enumerate(["D3", "A3", "D4", "F#4", "A4", "D5"]):
        score.play(1, home + j * 0.05, name, 2.6, 60 + 2 * j)
    score.play(2, home, ["D4", "A4"], 2.6, 40)
    score.play(0, home, "D5", 1.8, 70)
    return perform(score)


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    stereo = market_day()
    level = np.abs(stereo).max(axis=0)
    last = np.nonzero(level > level.max() * 10 ** (-50 / 20))[0][-1]
    stereo = stereo[:, : last + 1].copy()
    fade = int(0.8 * RATE)
    stereo[:, -fade:] *= np.linspace(1, 0, fade) ** 2
    rms = np.sqrt((stereo ** 2).mean())
    stereo *= 10 ** (RMS_DB / 20) / rms
    stereo *= min(1.0, 0.89 / np.abs(stereo).max())
    # Which sound font played it, kept in the clip's tags and said here, so its licence can
    # be checked against the one THIRD_PARTY_NOTICES.md names.
    font = Path(sound_font())
    rendered = f"rendered by FluidSynth from {font.name}, sha256 {hashlib.sha256(font.read_bytes()).hexdigest()}"
    path = OUT / "market_day.ogg"
    write_ogg(stereo, path, rendered)
    print(f"{path.relative_to(ROOT)}: {stereo.shape[1] / RATE:.2f} s, peak {np.abs(stereo).max():.2f}, {rendered}", file=sys.stderr)


if __name__ == "__main__":
    main()
