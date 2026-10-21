#!/usr/bin/env python3
"""Composes the game's event jingles: a quest finished, its rewards claimed, a level reached,
every tenth level reached, and a path's capstone finished.

Our own work. The four tunes are short brass fanfares (trumpets, horns, trombone, strings,
timpani, harp and glockenspiel), written here as notes and played by FluidSynth from the
FluidR3 General MIDI sound font (Frank Wen, MIT; see THIRD_PARTY_NOTICES.md). The claim's
coins and glint are synthesized from scratch. Run once offline; the clips are written to
assets/content/base/sounds/ as Ogg Vorbis and listed in sounds.ron, so nothing is played
from a sound font at runtime and a package can replace any of them. Needs numpy, ffmpeg,
fluidsynth and FluidR3_GM.sf2 (found at the usual places, or named by VOXMAR_SF2).

    python3 tools/generate_jingles.py [JINGLE ...]
"""

import os
import struct
import subprocess
import sys
import tempfile
import wave
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "assets" / "content" / "base" / "sounds"
RATE = 44_100

# How loud each clip is at its loudest 50 ms (RMS), as the game measures clips (sfx.rs,
# `loudness`): as loud as the synthesized chimes these replaced. The game plays each at its
# own gain (ui/sound.rs), the claim softest.
LOUDNESS = {"quest_done": 0.30, "quest_claim": 0.30, "level_up": 0.30, "level_milestone": 0.31, "capstone_done": 0.31}
PEAK = 0.89


def note(name: str) -> float:
    """The frequency of a note such as "F#5" (A4 = 440 Hz)."""
    steps = {"C": -9, "D": -7, "E": -5, "F": -4, "G": -2, "A": 0, "B": 2}
    pitch, octave = name[:-1], int(name[-1])
    semitone = steps[pitch[0]] + pitch.count("#") - pitch.count("b")
    return 440.0 * 2 ** ((semitone + 12 * (octave - 4)) / 12)


def modes(freq, seconds, partials, attack=0.002):
    """Decaying sine partials: (ratio to `freq`, level, decay seconds), rendered until they
    have died away (at least `seconds`)."""
    seconds = max(seconds, min(6 * max(decay for _, _, decay in partials), 5.0))
    t = np.arange(int(seconds * RATE)) / RATE
    out = np.zeros_like(t)
    for ratio, level, decay in partials:
        f = freq * ratio
        if f < RATE / 2.2:
            out += level * np.sin(2 * np.pi * f * t) * np.exp(-t / decay)
    return out * np.minimum(t / attack, 1.0)


def glock(freq, velocity=1.0, ring=1.0):
    """A small bright bell bar: the partials of a glockenspiel, ringing long (shorter for a
    smaller `ring`)."""
    partials = [(1.0, 1.0, 1.1), (2.76, 0.28 * velocity, 0.35), (5.40, 0.10 * velocity, 0.12), (8.93, 0.04 * velocity, 0.05)]
    return modes(freq, 2.4 * ring, [(r, lvl, dec * ring) for r, lvl, dec in partials], attack=0.0008)


def coin(freq, rng):
    """A small coin struck against others: close, inharmonic metal partials and a tick."""
    ratios = [1.0, 1.52, 2.17, 2.73, 3.38]
    out = modes(freq, 0.6, [(r * rng.uniform(0.99, 1.01), lvl, dec) for r, lvl, dec in zip(ratios, [1.0, 0.7, 0.5, 0.33, 0.2], [0.22, 0.16, 0.11, 0.07, 0.045])], attack=0.0004)
    tick = rng.standard_normal(int(0.004 * RATE)) * np.exp(-np.arange(int(0.004 * RATE)) / (0.0008 * RATE))
    out[: tick.size] += 0.35 * (tick - np.convolve(tick, np.ones(4) / 4, mode="same"))
    return out


class Mix:
    """A stereo clip notes are laid into, each at a time, a level and a place left to right."""

    def __init__(self, seconds):
        self.out = np.zeros((2, int(seconds * RATE)))

    def add(self, at, samples, level, pan=0.0):
        start = int(at * RATE)
        end = min(start + samples.size, self.out.shape[1])
        if end <= start:
            return
        samples = samples[: end - start].copy()
        # Every note fades out over its last 60 ms, so none stops with a click.
        fade = min(int(0.06 * RATE), samples.size)
        samples[-fade:] *= np.cos(np.linspace(0, np.pi / 2, fade)) ** 2
        angle = (pan + 1) * np.pi / 4
        self.out[0, start:end] += samples * level * np.cos(angle)
        self.out[1, start:end] += samples * level * np.sin(angle)


def room(dry, rng, wet=0.2, seconds=1.4, decay=0.32):
    """The clip in a small warm room: noise impulses, darkening as they die, one for each
    ear so the space is wide."""
    n = int(seconds * RATE)
    t = np.arange(n) / RATE
    out = np.zeros((2, dry.shape[1] + n - 1))
    for ch in range(2):
        noise = rng.standard_normal(n) * np.exp(-t / decay)
        smooth = np.zeros(n)
        low = 0.0
        for i in range(n):
            k = 0.6 * np.exp(-t[i] / 0.25) + 0.08
            low += k * (noise[i] - low)
            smooth[i] = low
        smooth[: int(0.012 * RATE)] = 0.0
        smooth /= np.sqrt(np.sum(smooth ** 2))
        size = 1 << int(np.ceil(np.log2(out.shape[1])))
        wetted = np.fft.irfft(np.fft.rfft(dry[ch], size) * np.fft.rfft(smooth, size), size)[: out.shape[1]]
        out[ch] = wet * wetted
        out[ch, : dry.shape[1]] += dry[ch]
    return out


def loudness(stereo):
    """The RMS of the loudest 50 ms stretch, as the game measures."""
    window = int(0.05 * RATE)
    mono_sq = (stereo ** 2).mean(axis=0)
    sums = np.convolve(mono_sq, np.ones(window), mode="valid") / window
    return float(np.sqrt(sums.max()))


def finish(stereo, name):
    """Trimmed of silence, faded at the end, and brought to its loudness."""
    level = np.abs(stereo).max(axis=0)
    last = np.nonzero(level > level.max() * 10 ** (-48 / 20))[0][-1]
    stereo = stereo[:, : last + 1].copy()
    fade = int(0.05 * RATE)
    stereo[:, -fade:] *= np.linspace(1, 0, fade) ** 2
    stereo *= LOUDNESS[name] / loudness(stereo)
    peak = np.abs(stereo).max()
    if peak > PEAK:
        stereo *= PEAK / peak
    return stereo


SOUND_FONTS = ["/usr/share/sounds/sf2/FluidR3_GM.sf2", "/usr/share/soundfonts/FluidR3_GM.sf2", "/opt/homebrew/share/soundfonts/FluidR3_GM.sf2", "/usr/local/share/soundfonts/FluidR3_GM.sf2"]

# General MIDI programs and the drum channel.
TRUMPET, TROMBONE, BRASS, HORN, STRINGS, TIMPANI, GLOCK, HARP = 56, 57, 61, 60, 48, 47, 9, 46
DRUMS = 9
CRASH = 49
TICKS = 960  # per second: 480 a beat at 120 beats a minute


def key(name: str) -> int:
    """The MIDI key of a note such as "F#5" (middle C, C4, is 60)."""
    steps = {"C": 0, "D": 2, "E": 4, "F": 5, "G": 7, "A": 9, "B": 11}
    pitch, octave = name[:-1], int(name[-1])
    return 12 * (octave + 1) + steps[pitch[0]] + pitch.count("#") - pitch.count("b")


class Score:
    """Notes for a band, each on its instrument's channel, at times in seconds."""

    def __init__(self, band):
        """`band`: (channel, program, volume, pan) for each instrument, 0-127 each."""
        self.events = []
        for channel, program, volume, pan in band:
            if channel != DRUMS:
                self.events.append((0, 0, bytes([0xC0 | channel, program])))
            for control, value in [(7, volume), (10, pan), (91, 50)]:
                self.events.append((0, 1, bytes([0xB0 | channel, control, value])))

    def play(self, channel, at, notes, seconds, velocity):
        """A note, or a chord of them, held `seconds` (a drum's key as a number)."""
        for name in [notes] if isinstance(notes, (str, int)) else notes:
            k = name if isinstance(name, int) else key(name)
            self.events.append((round(at * TICKS), 2, bytes([0x90 | channel, k, velocity])))
            self.events.append((round((at + seconds) * TICKS), 1, bytes([0x80 | channel, k, 0])))

    def midi(self) -> bytes:
        """A standard MIDI file of the score: one track, 500 000 microseconds a beat."""
        def vlq(v):
            out = [v & 0x7F]
            v >>= 7
            while v:
                out.append(0x80 | (v & 0x7F))
                v >>= 7
            return bytes(reversed(out))
        track = bytearray(b"\x00\xFF\x51\x03\x07\xA1\x20")
        last = 0
        for tick, _, message in sorted(self.events, key=lambda e: (e[0], e[1])):
            track += vlq(tick - last) + message
            last = tick
        track += b"\x00\xFF\x2F\x00"
        return b"MThd" + struct.pack(">IHHH", 6, 0, 1, 480) + b"MTrk" + struct.pack(">I", len(track)) + bytes(track)


def sound_font() -> str:
    named = os.environ.get("VOXMAR_SF2")
    for path in ([named] if named else []) + SOUND_FONTS:
        if Path(path).is_file():
            return path
    sys.exit("no FluidR3_GM.sf2: install the fluid-soundfont-gm package (or name one in VOXMAR_SF2)")


def perform(score: Score):
    """The score played by FluidSynth in a small hall, as a stereo clip."""
    with tempfile.TemporaryDirectory() as tmp:
        mid, wav = Path(tmp) / "score.mid", Path(tmp) / "score.wav"
        mid.write_bytes(score.midi())
        settings = ["-o", "synth.reverb.room-size=0.45", "-o", "synth.reverb.level=0.5", "-o", "synth.chorus.active=0"]
        subprocess.run(["fluidsynth", "-ni", "-q", "-g", "0.4", "-r", str(RATE), *settings, "-F", str(wav), sound_font(), str(mid)], check=True, stdout=subprocess.DEVNULL)
        with wave.open(str(wav)) as w:
            pcm = np.frombuffer(w.readframes(w.getnframes()), "<i2")
    return pcm.reshape(-1, 2).T / 32768.0


def quest_done(rng):
    """A quest finished: a trumpet call over the fourth and fifth chords landing home on the
    first (IV - V - I), held, with a harp sweeping up and two bells above it. About two
    seconds."""
    score = Score([(0, TRUMPET, 110, 70), (1, HORN, 100, 50), (2, STRINGS, 90, 64), (3, TIMPANI, 110, 64), (4, GLOCK, 80, 84), (5, HARP, 95, 40), (6, TROMBONE, 85, 56)])
    home = 0.52
    for at, name, length, velocity in [(0.0, "A4", 0.16, 96), (0.18, "B4", 0.16, 100), (0.36, "C5", 0.14, 104), (home, "E5", 1.05, 112)]:
        score.play(0, at, name, length, velocity)
    for at, chord, bass, length, velocity in [(0.0, ["F3", "A3", "C4"], "F2", 0.17, 84), (0.18, ["G3", "B3", "D4"], "G2", 0.17, 88), (home, ["C4", "E4", "G4"], "C3", 1.05, 96)]:
        score.play(1, at, chord, length, velocity)
        score.play(6, at, bass, length, velocity - 4)
    score.play(2, home, ["C3", "G3", "E4", "C5"], 1.15, 80)
    score.play(3, 0.18, "G2", 0.2, 90)
    score.play(3, home, "C2", 0.6, 118)
    for i, name in enumerate(["C5", "E5", "G5", "C6", "E6", "G6"]):
        score.play(5, home + i * 0.035, name, 0.9, 70 + 4 * i)
    score.play(4, home + 0.08, "G6", 0.6, 74)
    score.play(4, home + 0.2, "C7", 0.8, 70)
    return perform(score)


def quest_claim(rng):
    """Rewards claimed: two or three coins clinking into the purse and a small bright glint
    in the key of the quest chime. Each one a little different."""
    mix = Mix(1.2)
    at = 0.0
    for _ in range(rng.integers(2, 4)):
        mix.add(at, coin(rng.uniform(2500, 3300), rng), rng.uniform(0.55, 0.8), rng.uniform(-0.25, 0.25))
        at += rng.uniform(0.045, 0.085)
    glint = at + 0.03
    first, second = ("G6", "C7") if rng.random() < 0.5 else ("E6", "G6")
    mix.add(glint, glock(note(first), 0.5, ring=0.35), 0.28, 0.15)
    mix.add(glint + 0.07, glock(note(second), 0.5, ring=0.35), 0.24, 0.25)
    return mix.out


def level_up(rng):
    """A level reached: a quick trumpet arpeggio up the home chord to a bright held top note,
    the brass and strings striking the chord under it with a timpani hit, a harp sweeping up
    and bells above. Brighter and quicker than a quest's call."""
    score = Score([(0, TRUMPET, 112, 72), (1, BRASS, 100, 56), (2, STRINGS, 88, 64), (3, TIMPANI, 105, 64), (4, GLOCK, 82, 84), (5, HARP, 95, 40)])
    top = 0.255
    for i, name in enumerate(["D5", "F#5", "A5"]):
        score.play(0, i * 0.085, name, 0.08, 92 + 6 * i)
    score.play(0, top, "D6", 0.85, 112)
    score.play(1, top, ["D4", "F#4", "A4", "D5"], 0.85, 100)
    score.play(2, top, ["D3", "A3", "F#4", "D5"], 0.95, 80)
    score.play(3, top, "D2", 0.5, 116)
    for i, name in enumerate(["D5", "F#5", "A5", "D6", "F#6"]):
        score.play(5, i * 0.05, name, 0.8, 64 + 5 * i)
    score.play(4, top + 0.045, "A6", 0.5, 76)
    score.play(4, top + 0.145, "D7", 0.7, 72)
    return perform(score)


def level_milestone(rng):
    """A tenth level reached: a timpani roll into a trumpet call on the home chord, answered a
    step higher, then a rising close (IV - V - I) held full under a cymbal, harp and bells.
    About three seconds, the grandest of the three."""
    score = Score([(0, TRUMPET, 112, 72), (1, BRASS, 104, 56), (2, STRINGS, 92, 64), (3, TIMPANI, 110, 64), (4, GLOCK, 82, 84), (5, HARP, 95, 40), (6, TROMBONE, 92, 52), (7, HORN, 96, 48), (DRUMS, 0, 90, 64)])
    for i in range(7):
        score.play(3, i * 0.04, "A1", 0.05, 50 + 6 * i)
    for start, chord, bass, root, top in [(0.0, ["D4", "F#4", "A4"], "D3", "D2", "D5"), (0.62, ["C4", "E4", "G4"], "C3", "C2", "E5")]:
        pickup = "A4" if top == "D5" else "C5"
        score.play(0, start, pickup, 0.12, 96)
        score.play(0, start + 0.14, pickup, 0.12, 96)
        score.play(0, start + 0.28, top, 0.3, 110)
        score.play(1, start + 0.28, chord, 0.3, 98)
        score.play(6, start + 0.28, bass, 0.3, 92)
        score.play(3, start + 0.28, root, 0.3, 118)
    close = 1.56
    for at, name, chord in [(1.24, "G5", ["G3", "B3", "D4"]), (1.40, "F#5", ["A3", "C#4", "E4"])]:
        score.play(0, at, name, 0.14, 104)
        score.play(7, at, chord, 0.15, 90)
    for i in range(8):
        score.play(3, 1.24 + i * 0.04, "A1", 0.05, 60 + 6 * i)
    score.play(0, close, "A5", 1.4, 118)
    score.play(1, close, ["D4", "F#4", "A4", "D5"], 1.4, 108)
    score.play(7, close, ["D3", "A3", "F#4"], 1.4, 100)
    score.play(6, close, "D2", 1.4, 100)
    score.play(2, close, ["D3", "A3", "F#4", "D5", "A5"], 1.6, 88)
    score.play(3, close, "D2", 0.8, 124)
    score.play(DRUMS, close, CRASH, 1.2, 70)
    for i, name in enumerate(["D5", "F#5", "A5", "D6", "F#6", "A6"]):
        score.play(5, close + i * 0.03, name, 1.0, 66 + 5 * i)
    for i, name in enumerate(["A6", "D7", "F#6", "A6", "D7"]):
        score.play(4, close + 0.08 + i * 0.11, name, 0.5, 74 - 4 * i)
    return perform(score)


def capstone_done(rng):
    """A path's last quest finished (its capstone): a horn call rising over warm strings,
    answered by the trumpet a fourth higher, then a slow plagal close (IV - I, the hymn's
    amen) held full and soft by the horns and brass under harp and bells. About four
    seconds: warmer and broader than a level's milestone."""
    score = Score([(0, TRUMPET, 104, 72), (1, HORN, 108, 50), (2, STRINGS, 96, 64), (3, TIMPANI, 104, 64), (4, GLOCK, 64, 84), (5, HARP, 95, 40), (6, TROMBONE, 88, 56), (7, BRASS, 92, 58)])
    for at, name, length, velocity in [(0.0, "G4", 0.22, 92), (0.24, "C5", 0.22, 98), (0.48, "E5", 0.5, 106)]:
        score.play(1, at, name, length, velocity)
    score.play(2, 0.0, ["C3", "G3", "E4"], 1.0, 72)
    score.play(3, 0.0, "C2", 0.4, 96)
    for at, name, length, velocity in [(1.0, "C5", 0.2, 98), (1.22, "F5", 0.22, 104), (1.46, "A5", 0.42, 110)]:
        score.play(0, at, name, length, velocity)
    score.play(2, 1.0, ["F3", "C4", "A4"], 0.9, 80)
    score.play(6, 1.46, "F2", 0.42, 88)
    amen = 2.0
    score.play(7, 1.46, ["F3", "A3", "C4"], 0.52, 92)
    score.play(3, 1.46, "F1", 0.3, 92)
    score.play(1, amen, ["C4", "E4", "G4", "C5"], 1.3, 92)
    score.play(7, amen, ["C3", "G3", "E4"], 1.3, 86)
    score.play(6, amen, "C2", 1.3, 86)
    score.play(2, amen, ["C3", "G3", "E4", "C5"], 1.5, 86)
    for i in range(6):
        score.play(3, amen - 0.24 + i * 0.04, "C2", 0.05, 56 + 6 * i)
    score.play(3, amen, "C2", 0.7, 88)
    for i, name in enumerate(["C5", "E5", "G5", "C6", "E6", "G6", "C7"]):
        score.play(5, amen + i * 0.04, name, 1.0, 56 + 4 * i)
    for i, name in enumerate(["G6", "C7", "E7", "C7"]):
        score.play(4, amen + 0.12 + i * 0.16, name, 0.7, 60 - 4 * i)
    return perform(score)


def write_ogg(stereo, path, comment=None):
    """Writes `stereo` as Ogg Vorbis, with `comment` (say, what rendered it) in its tags."""
    pcm = (np.clip(stereo.T, -1, 1) * 32767).astype("<i2")
    with tempfile.NamedTemporaryFile(suffix=".wav") as tmp:
        with wave.open(tmp.name, "wb") as w:
            w.setnchannels(2)
            w.setsampwidth(2)
            w.setframerate(RATE)
            w.writeframes(pcm.tobytes())
        tags = ["-metadata", f"comment={comment}"] if comment else []
        subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-i", tmp.name, "-c:a", "libvorbis", "-q:a", "6", *tags, str(path)], check=True)


# Each jingle: how it is composed, how many takes, and whether it is put in the small room
# (the fanfares have FluidSynth's hall already).
JINGLES = {
    "quest_done": (quest_done, 1, False),
    "quest_claim": (quest_claim, 3, True),
    "level_up": (level_up, 1, False),
    "level_milestone": (level_milestone, 1, False),
    "capstone_done": (capstone_done, 1, False),
}


def main() -> None:
    only = set(sys.argv[1:])
    OUT.mkdir(parents=True, exist_ok=True)
    for name, (compose, takes, roomed) in JINGLES.items():
        if only and name not in only:
            continue
        for take in range(1, takes + 1):
            rng = np.random.default_rng(1000 * take + sum(map(ord, name)))
            played = compose(rng)
            stereo = finish(room(played, rng) if roomed else played, name)
            path = OUT / f"{name}_{take}.ogg"
            write_ogg(stereo, path)
            print(f"{path.relative_to(ROOT)}: {stereo.shape[1] / RATE:.2f} s, loudest {loudness(stereo):.3f}, peak {np.abs(stereo).max():.2f}", file=sys.stderr)


if __name__ == "__main__":
    main()
