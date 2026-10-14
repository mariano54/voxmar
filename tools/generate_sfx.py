#!/usr/bin/env python3
"""Synthesizes the everyday action sounds: eating, crafting, lighting a fire, opening a
chest or a door, cooking, lying down to sleep, casting a line, praying at an altar, and
picking an item up.

Our own work, built from noise, resonators and decaying modes (no samples). Run once
offline; the clips are written to assets/content/base/sounds/ as Ogg Vorbis and listed in
sounds.ron, so a package can replace any of them. Needs numpy and ffmpeg.

    python3 tools/generate_sfx.py          # every clip
    python3 tools/generate_sfx.py pray     # only these
"""

import sys
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
from generate_jingles import OUT, RATE, ROOT, loudness, modes, write_ogg  # noqa: E402

# How loud each clip is at its loudest 50 ms (RMS), as the game measures; the game plays
# them at the gain sounds.ron gives.
LOUDNESS = 0.22
PEAK = 0.89


def bandpass(signal, centre, q):
    """A resonant band-pass (the RBJ biquad), its centre fixed or one per sample."""
    centre = np.broadcast_to(np.asarray(centre, dtype=float), signal.shape)
    out = np.zeros_like(signal)
    x1 = x2 = y1 = y2 = 0.0
    for i, x in enumerate(signal):
        w = 2 * np.pi * min(centre[i], RATE * 0.45) / RATE
        alpha = np.sin(w) / (2 * q)
        a0 = 1 + alpha
        b0, b2 = alpha / a0, -alpha / a0
        a1, a2 = -2 * np.cos(w) / a0, (1 - alpha) / a0
        y = b0 * x + b2 * x2 - a1 * y1 - a2 * y2
        x2, x1, y2, y1 = x1, x, y1, y
        out[i] = y
    return out


def lowpass(signal, k):
    """A one-pole low-pass: `k` (0..1) of the way to each new sample."""
    out = np.zeros_like(signal)
    low = 0.0
    for i, x in enumerate(signal):
        low += k * (x - low)
        out[i] = low
    return out


def envelope(n, attack, decay):
    t = np.arange(n) / RATE
    return np.minimum(t / max(attack, 1e-4), 1.0) * np.exp(-t / decay)


def place(out, at, sound, level=1.0):
    start = int(at * RATE)
    end = min(start + sound.size, out.size)
    out[start:end] += sound[: end - start] * level


def crunch(rng, seconds=0.09, bright=2600.0):
    """One bite of something crisp: a cluster of tiny cracks through the mouth's resonance,
    over a soft low thump of the jaw."""
    n = int(seconds * RATE)
    grains = np.zeros(n)
    for _ in range(rng.integers(14, 24)):
        at = int(rng.uniform(0, 0.6) * n)
        width = int(rng.uniform(0.0006, 0.002) * RATE)
        grains[at : at + width] += rng.standard_normal(min(width, n - at)) * rng.uniform(0.4, 1.0)
    grains *= envelope(n, 0.002, seconds * 0.45)
    cracks = bandpass(grains, bright * rng.uniform(0.85, 1.15), 1.4) + 0.4 * bandpass(grains, 900 * rng.uniform(0.9, 1.1), 2.0)
    jaw = modes(rng.uniform(140, 190), seconds, [(1.0, 1.0, 0.025)], attack=0.003)[:n]
    return cracks + 0.5 * jaw


def chew(rng, seconds=0.16):
    """The wet, soft part of a chew between bites."""
    n = int(seconds * RATE)
    noise = rng.standard_normal(n) * envelope(n, 0.03, seconds * 0.4)
    return bandpass(noise, 650 * rng.uniform(0.8, 1.2), 3.0) * 0.5


def eat(rng):
    """Three bites and their chews, a little uneven, over about two thirds of a second (the
    time eating takes)."""
    out = np.zeros(int(0.75 * RATE))
    at = 0.0
    for bite in range(3):
        place(out, at, crunch(rng, bright=rng.uniform(2200, 3200)), 1.0 - 0.15 * bite)
        place(out, at + 0.07, chew(rng), 0.8)
        at += rng.uniform(0.18, 0.24)
    return out


def knock(rng, freq, level=1.0):
    """A knock on wood: a few short, inharmonic modes of a board and a tap of noise."""
    n = int(0.25 * RATE)
    board = modes(freq, 0.25, [(1.0, 1.0, 0.05), (2.32, 0.5, 0.03), (4.1, 0.25, 0.015), (6.7, 0.1, 0.008)], attack=0.0006)[:n]
    tap = rng.standard_normal(n) * envelope(n, 0.0003, 0.004)
    return (board + 0.3 * bandpass(tap, 2500, 1.0)) * level


def scrape(rng, seconds, centre):
    """Something drawn across wood or stone: band noise that swells and fades."""
    n = int(seconds * RATE)
    t = np.arange(n) / n
    noise = rng.standard_normal(n) * np.sin(np.pi * t) ** 1.5
    return bandpass(noise, centre * (0.9 + 0.2 * t), 2.0)


def craft(rng):
    """Parts fitted together: a scrape, then two knocks, the second firmer."""
    out = np.zeros(int(0.45 * RATE))
    place(out, 0.0, scrape(rng, 0.12, rng.uniform(1400, 2000)), 0.35)
    base = rng.uniform(260, 340)
    place(out, 0.11, knock(rng, base * rng.uniform(1.05, 1.15)), 0.7)
    place(out, 0.11 + rng.uniform(0.09, 0.13), knock(rng, base), 1.0)
    return out


def crackles(rng, seconds, rate, level=1.0):
    """Sparse pops of burning wood."""
    n = int(seconds * RATE)
    out = np.zeros(n)
    for _ in range(int(rate * seconds)):
        at = int(rng.uniform(0, 1) * n)
        width = int(rng.uniform(0.0004, 0.0015) * RATE)
        out[at : at + width] += rng.standard_normal(min(width, n - at)) * rng.uniform(0.2, 1.0)
    return bandpass(out, 3200, 0.8) * level


def light(rng):
    """Flint struck to steel, sparks, and the tinder catching: a soft rush of flame swelling
    up with the first crackles."""
    out = np.zeros(int(1.3 * RATE))
    for strike in range(2):
        at = strike * rng.uniform(0.12, 0.17)
        n = int(0.05 * RATE)
        click = rng.standard_normal(n) * envelope(n, 0.0002, 0.005)
        place(out, at, bandpass(click, 4200, 1.2), 0.9)
        place(out, at, modes(rng.uniform(3800, 4600), 0.08, [(1.0, 0.3, 0.012), (1.48, 0.2, 0.008)], attack=0.0003), 1.0)
        place(out, at + 0.005, crackles(rng, 0.12, 120, 0.5))
    catch = 0.32
    n = int(0.95 * RATE)
    t = np.arange(n) / RATE
    swell = np.minimum(t / 0.22, 1.0) ** 2 * np.exp(-np.maximum(t - 0.22, 0) / 0.32)
    rush = lowpass(rng.standard_normal(n), 0.05) * swell
    place(out, catch, rush / (np.abs(rush).max() + 1e-9), 0.7)
    place(out, catch + 0.1, crackles(rng, 0.8, 26, 0.9) * np.exp(-np.arange(int(0.8 * RATE)) / RATE / 0.5))
    return out


def creak(rng, seconds, low, high, body):
    """A hinge or a lid creaking: stick-slip pulses whose rate rises and falls, rung through
    the wood's body."""
    n = int(seconds * RATE)
    t = np.arange(n) / RATE
    shape = np.sin(np.pi * t / seconds)
    rate = low + (high - low) * (0.5 + 0.5 * np.sin(np.pi * (t / seconds) * rng.uniform(1.2, 1.8) + rng.uniform(0, 1)))
    phase = np.cumsum(rate / RATE)
    pulses = np.where(np.diff(np.floor(phase), prepend=0) > 0, 1.0, 0.0) * (0.7 + 0.3 * rng.random(n))
    rung = bandpass(pulses, body, 6.0) + 0.6 * bandpass(pulses, body * 2.3, 5.0) + 0.3 * bandpass(pulses, body * 4.1, 4.0)
    return rung * shape ** 0.8


def store_open(rng):
    """A chest lid lifted: a short creak and the lid knocking back."""
    out = np.zeros(int(0.6 * RATE))
    creaking = rng.uniform(0.22, 0.3)
    place(out, 0.0, creak(rng, creaking, 55, 110, rng.uniform(520, 700)), 0.8)
    place(out, creaking - 0.02, knock(rng, rng.uniform(150, 190)), 0.8)
    return out


def door(rng):
    """A wooden door swung: the latch, then a slower creak on its hinges."""
    out = np.zeros(int(0.8 * RATE))
    n = int(0.03 * RATE)
    latch = modes(rng.uniform(1900, 2300), 0.05, [(1.0, 0.6, 0.01), (2.7, 0.3, 0.005)], attack=0.0003)[:n]
    place(out, 0.0, latch + bandpass(rng.standard_normal(n) * envelope(n, 0.0002, 0.003), 3000, 1.0), 0.35)
    place(out, 0.06, creak(rng, rng.uniform(0.45, 0.6), 35, 80, rng.uniform(380, 480)), 0.9)
    return out


def cook(rng):
    """Food meeting a hot fire: a sudden hiss settling into a sizzle with spits of fat."""
    seconds = 1.1
    n = int(seconds * RATE)
    t = np.arange(n) / RATE
    hiss = bandpass(rng.standard_normal(n), 5200, 0.7) * (np.minimum(t / 0.01, 1) * (0.35 + 0.65 * np.exp(-t / 0.15)))
    flutter = 0.6 + 0.4 * lowpass(rng.random(n), 0.002) / 0.5
    spits = crackles(rng, seconds, 40, 1.3)
    out = (hiss * flutter + spits) * np.minimum((seconds - t) / 0.25, 1.0)
    return out


def sleep(rng):
    """Lying down on bedding: two soft rustles of cloth and straw and a low settling."""
    out = np.zeros(int(0.9 * RATE))
    for at, seconds in [(0.0, 0.28), (0.3, 0.4)]:
        n = int(seconds * RATE)
        noise = rng.standard_normal(n) * np.sin(np.pi * np.arange(n) / n) ** 2
        rustle = bandpass(noise, rng.uniform(1600, 2200), 1.2) + 0.5 * bandpass(noise, 700, 1.5)
        place(out, at, rustle, 0.6)
    n = int(0.4 * RATE)
    place(out, 0.32, lowpass(rng.standard_normal(n), 0.01) * envelope(n, 0.03, 0.12) * 4, 0.6)
    return out


def cast(rng):
    """A rod whipped forward: air rushing past it, rising, and the line paying out."""
    seconds = 0.35
    n = int(seconds * RATE)
    t = np.arange(n) / n
    noise = rng.standard_normal(n) * np.sin(np.pi * t) ** 2
    out = bandpass(noise, 500 + 2600 * t ** 1.5, 1.6)
    out = np.concatenate([out, np.zeros(int(0.1 * RATE))])
    place(out, 0.24, crackles(rng, 0.15, 90, 0.25))
    return out


def pray(rng):
    """A prayer at the altar: a low stone bowl struck soft, its hum swelling and fading,
    and a breath of air over it."""
    root = rng.uniform(196, 208)
    bowl = modes(root, 1.0, [(1.0, 1.0, 0.22), (2.76, 0.35, 0.14), (5.4, 0.12, 0.08)], attack=0.004)
    n = len(bowl)
    t = np.arange(n) / RATE
    swell = np.minimum(t / 0.2, 1.0) * np.exp(-t / 0.25)
    hum = np.sin(2 * np.pi * root * 0.5 * t) * swell * 0.35
    air = lowpass(rng.standard_normal(n), 0.03) * swell * 0.25
    return bowl + hum + air / (np.abs(air).max() + 1e-9) * 0.2


def pickup(rng):
    """An item taken up into the pack: a small, round, rising pop."""
    seconds = 0.12
    n = int(seconds * RATE)
    t = np.arange(n) / RATE
    start = rng.uniform(420, 520)
    freq = start * (1.9 ** np.minimum(t / 0.045, 1.0))
    phase = np.cumsum(2 * np.pi * freq / RATE)
    pop = (np.sin(phase) + 0.15 * np.sin(2 * phase)) * envelope(n, 0.002, 0.035)
    tick = rng.standard_normal(n) * envelope(n, 0.0002, 0.002)
    return pop + 0.15 * bandpass(tick, 3000, 1.0)


def finish(mono, rng, width=0.15):
    """The clip made stereo (a hair of width from a short delay), faded, and brought to
    its loudness."""
    delay = int(rng.uniform(0.0004, 0.0009) * RATE)
    left = mono
    right = np.concatenate([np.zeros(delay), mono[:-delay]]) if delay else mono
    stereo = np.stack([left * (1 - width) + right * width, right * (1 - width) + left * width])
    level = np.abs(stereo).max(axis=0)
    last = np.nonzero(level > level.max() * 10 ** (-50 / 20))[0][-1]
    stereo = stereo[:, : last + 1].copy()
    fade = min(int(0.02 * RATE), stereo.shape[1])
    stereo[:, -fade:] *= np.linspace(1, 0, fade) ** 2
    stereo *= LOUDNESS / loudness(stereo)
    peak = np.abs(stereo).max()
    if peak > PEAK:
        stereo *= PEAK / peak
    return stereo


SOUNDS = {
    "eat": (eat, 3),
    "craft": (craft, 3),
    "light": (light, 2),
    "store_open": (store_open, 2),
    "door": (door, 2),
    "cook": (cook, 2),
    "sleep": (sleep, 1),
    "cast": (cast, 2),
    "pray": (pray, 2),
    "pickup": (pickup, 4),
}


def main() -> None:
    # Names given on the command line make only those (`generate_sfx.py pray`).
    only = set(sys.argv[1:])
    for name, (make, takes) in SOUNDS.items():
        if only and name not in only:
            continue
        for take in range(1, takes + 1):
            rng = np.random.default_rng(7000 + 1000 * take + sum(map(ord, name)))
            stereo = finish(make(rng), rng)
            path = OUT / f"{name}_{take}.ogg"
            write_ogg(stereo, path)
            print(f"{path.relative_to(ROOT)}: {stereo.shape[1] / RATE:.2f} s, loudest {loudness(stereo):.3f}, peak {np.abs(stereo).max():.2f}", file=sys.stderr)


if __name__ == "__main__":
    main()
