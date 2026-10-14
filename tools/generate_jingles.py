#!/usr/bin/env python3
"""Composes the game's event jingles: a quest finished, its rewards claimed, a level reached
and every tenth level reached.

Our own work, synthesized from scratch (no samples): plucked harp, marimba, glockenspiel and
a soft pad, played into a small room. Run once offline; the clips are written to
assets/content/base/sounds/ as Ogg Vorbis and listed in sounds.ron, so nothing is synthesized
at runtime and a package can replace any of them. Needs numpy and ffmpeg.

    python3 tools/generate_jingles.py
"""

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
LOUDNESS = {"quest_done": 0.30, "quest_claim": 0.30, "level_up": 0.30, "level_milestone": 0.31}
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


def harp(freq, velocity=1.0):
    """A plucked string: nearly whole harmonics, the high ones dying first, brighter when
    plucked harder."""
    decay = 0.95 * (440 / freq) ** 0.35
    partials = [(n * np.sqrt(1 + 0.00012 * n * n), (velocity ** (0.3 * (n - 1))) / n ** 1.6, decay / (1 + 0.9 * (n - 1))) for n in range(1, 9)]
    return modes(freq, 2.6, partials, attack=0.0015)


def marimba(freq, velocity=1.0):
    """A warm wooden bar: the fundamental with its fourth and tenth (as tuned bars ring)."""
    decay = 0.9 * (262 / freq) ** 0.5
    return modes(freq, 2.0, [(1.0, 1.0, decay), (3.99, 0.22 * velocity, decay * 0.18), (9.9, 0.05 * velocity, decay * 0.06)], attack=0.001)


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


def pad(freqs, seconds, rise=0.18, fall=0.6):
    """A soft held chord: two slightly detuned, rounded voices per note, swelling in and
    dying away."""
    t = np.arange(int(seconds * RATE)) / RATE
    out = np.zeros_like(t)
    for f in freqs:
        for cents in (-5, 5):
            g = f * 2 ** (cents / 1200)
            out += np.sin(2 * np.pi * g * t) + np.sin(2 * np.pi * 3 * g * t) / 9 + np.sin(2 * np.pi * 5 * g * t) / 25
    envelope = np.minimum(t / rise, 1.0) * np.exp(-np.maximum(t - rise, 0.0) / fall) * np.minimum((seconds - t) / 0.3, 1.0)
    return out * envelope / (2 * len(freqs))


def whoosh(seconds, rng):
    """A breath of noise that brightens and swells, under a run of notes."""
    n = int(seconds * RATE)
    noise = rng.standard_normal(n)
    out = np.zeros(n)
    low = 0.0
    for i in range(n):
        k = 0.02 + 0.25 * (i / n) ** 2
        low += k * (noise[i] - low)
        out[i] = low
    t = np.arange(n) / n
    return out * t ** 2 * (1 - t) ** 0.4 * 4


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


def pan_of(freq):
    """Higher notes sit a little to the right, as on a keyboard."""
    return float(np.clip(np.log2(freq / 523.0) * 0.18, -0.35, 0.35))


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


def quest_done(rng):
    """A quest finished: a plucked rise on the fourth chord settling home onto the first (the
    "amen" close), the top note held under a sprinkle of bells."""
    mix = Mix(4.0)
    for at, name, vel in [(0.0, "F4", 0.55), (0.075, "A4", 0.6), (0.15, "C5", 0.68)]:
        mix.add(at + rng.uniform(0, 0.006), harp(note(name), vel), vel, pan_of(note(name)))
    home = 0.31
    mix.add(home, marimba(note("C3")), 0.5, -0.15)
    mix.add(home + 0.004, harp(note("C4"), 0.5), 0.35, -0.1)
    mix.add(home, pad([note("C4"), note("E4"), note("G4")], 2.2, rise=0.12, fall=0.7), 0.16)
    for at, name, vel in [(0.0, "E5", 0.7), (0.075, "G5", 0.68), (0.15, "C6", 0.8)]:
        mix.add(home + at + rng.uniform(0, 0.006), harp(note(name), vel), vel, pan_of(note(name)))
    mix.add(home + 0.15, marimba(note("C5")), 0.22, 0.05)
    for at, name, vel in [(0.24, "E6", 0.2), (0.32, "G6", 0.17), (0.40, "C7", 0.15)]:
        mix.add(home + at, glock(note(name), 0.6), vel, pan_of(note(name)))
    return mix.out


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
    """A level reached: a quick harp run up the scale (no fourth or seventh, so it only rises
    brighter) under a breath of air, landing on a full bright chord, with bells shimmering
    above it."""
    mix = Mix(3.6)
    run = ["D5", "E5", "F#5", "A5", "B5", "D6"]
    for i, name in enumerate(run):
        vel = 0.42 + 0.06 * i
        mix.add(i * 0.042 + rng.uniform(0, 0.004), harp(note(name), vel), vel, pan_of(note(name)))
    mix.add(0.0, whoosh(0.3, rng), 0.05)
    land = 0.29
    mix.add(land, marimba(note("D3")), 0.45, -0.2)
    mix.add(land, marimba(note("A3")), 0.3, -0.1)
    for i, name in enumerate(["D5", "F#5", "A5", "D6"]):
        mix.add(land + i * 0.012, harp(note(name), 0.75), 0.5, pan_of(note(name)))
    mix.add(land, pad([note("D4"), note("F#4"), note("A4")], 1.8, rise=0.06, fall=0.55), 0.18)
    mix.add(land + 0.01, glock(note("A6"), 0.8), 0.3, 0.2)
    mix.add(land + 0.09, glock(note("D7"), 0.8), 0.26, 0.28)
    for i in range(8):
        name = "F#6" if i % 2 == 0 else "A6"
        mix.add(land + 0.2 + i * 0.065, glock(note(name), 0.4), 0.16 * 0.78 ** i, 0.3 if i % 2 else 0.1)
    return mix.out


def level_milestone(rng):
    """A tenth level reached: the level-up run stretched over two octaves, a fuller landing, and
    a little rising call answered on the home note."""
    mix = Mix(4.6)
    run = ["D4", "E4", "F#4", "A4", "B4", "D5", "E5", "F#5", "A5", "B5", "D6"]
    for i, name in enumerate(run):
        vel = 0.38 + 0.04 * i
        mix.add(i * 0.036 + rng.uniform(0, 0.004), harp(note(name), vel), vel, pan_of(note(name)))
    mix.add(0.0, whoosh(0.42, rng), 0.06)
    land = 0.42
    mix.add(land, marimba(note("D3")), 0.5, -0.2)
    mix.add(land, marimba(note("A3")), 0.32, -0.1)
    for i, name in enumerate(["D5", "F#5", "A5", "D6"]):
        mix.add(land + i * 0.012, harp(note(name), 0.8), 0.5, pan_of(note(name)))
    mix.add(land + 0.01, glock(note("A6"), 0.8), 0.28, 0.2)
    mix.add(land, pad([note("D4"), note("F#4"), note("A4")], 1.0, rise=0.05, fall=0.4), 0.16)
    call = land + 0.42
    mix.add(call, pad([note("G3"), note("B3"), note("D4")], 0.6, rise=0.05, fall=0.3), 0.14)
    for at, name, vel in [(0.0, "B5", 0.55), (0.12, "D6", 0.6)]:
        mix.add(call + at, harp(note(name), vel), vel, pan_of(note(name)))
        mix.add(call + at, glock(note(name), 0.4), 0.12, pan_of(note(name)))
    home = call + 0.3
    mix.add(home, marimba(note("D3")), 0.5, -0.2)
    mix.add(home, pad([note("D4"), note("F#4"), note("A4"), note("D5")], 2.2, rise=0.1, fall=0.8), 0.17)
    mix.add(home, harp(note("A5"), 0.7), 0.45, 0.1)
    mix.add(home + 0.01, harp(note("D6"), 0.85), 0.62, 0.15)
    for i in range(10):
        name = "F#6" if i % 2 == 0 else "A6"
        mix.add(home + 0.12 + i * 0.06, glock(note(name), 0.4), 0.15 * 0.8 ** i, 0.3 if i % 2 else 0.1)
    mix.add(home + 0.75, glock(note("D7"), 0.5), 0.12, 0.25)
    return mix.out


def write_ogg(stereo, path):
    pcm = (np.clip(stereo.T, -1, 1) * 32767).astype("<i2")
    with tempfile.NamedTemporaryFile(suffix=".wav") as tmp:
        with wave.open(tmp.name, "wb") as w:
            w.setnchannels(2)
            w.setsampwidth(2)
            w.setframerate(RATE)
            w.writeframes(pcm.tobytes())
        subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-i", tmp.name, "-c:a", "libvorbis", "-q:a", "6", str(path)], check=True)


JINGLES = {
    "quest_done": (quest_done, 1),
    "quest_claim": (quest_claim, 3),
    "level_up": (level_up, 1),
    "level_milestone": (level_milestone, 1),
}


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    for name, (compose, takes) in JINGLES.items():
        for take in range(1, takes + 1):
            rng = np.random.default_rng(1000 * take + sum(map(ord, name)))
            stereo = finish(room(compose(rng), rng), name)
            path = OUT / f"{name}_{take}.ogg"
            write_ogg(stereo, path)
            print(f"{path.relative_to(ROOT)}: {stereo.shape[1] / RATE:.2f} s, loudest {loudness(stereo):.3f}, peak {np.abs(stereo).max():.2f}", file=sys.stderr)


if __name__ == "__main__":
    main()
