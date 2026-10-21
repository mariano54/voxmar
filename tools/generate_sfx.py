#!/usr/bin/env python3
"""Synthesizes the everyday action sounds: eating, crafting, lighting a fire, opening a
chest, a barrel, a cupboard, a jar or a basket, a door (and shutting it), a gate,
shutters, a hatch or a curtain, cooking, lying down to sleep (on a wooden bed too),
sitting on a wooden seat,
casting a line, picking an item up, feeding an animal, emptying a pouch, loading a machine, picking a lock, praying at an altar,
spreading fertiliser, throwing an item down, a snare springing shut, a fish landed, food
coming off the fire done or burnt; for the interface, a quest book's page turned and a
soft "no" when something is refused; and the building materials' steps, placings and
breakings (timber, thatch, shingle, cobblestone, stone brick, wattle, daub, hide,
terracotta) under materials/;
and the work of the first two ages under work/: flint knapped,
a hide scraped, clay shaped, pottery, stone dressed, cord twisted, wood whittled, the
home stations loaded, a kiln fed, ore smelted, a hearth or kiln kindled, a sheep shorn,
a cow milked, bees smoked and honey taken, grain ground at a quern, wool spun on a
spindle and woven at a loom, and the flour, twine and cloth they give.

Our own work, built from noise, resonators and decaying modes (no samples). Run once
offline; the clips are written to assets/content/base/sounds/ as Ogg Vorbis and listed in
sounds.ron, so a package can replace any of them. Needs numpy and ffmpeg.

    python3 tools/generate_sfx.py [names...]  (all, or only those named)
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


def catch(rng):
    """Thatch or wattle catching fire (fire.ron): a soft, low whoomph of flame taking hold,
    swelling quickly and settling, with the first dry crackles running through it. Softer
    than a strike: a warning heard across a room, not a bang."""
    n = int(1.4 * RATE)
    t = np.arange(n) / RATE
    rise = rng.uniform(0.12, 0.18)
    swell = np.minimum(t / rise, 1.0) ** 1.5 * np.exp(-np.maximum(t - rise, 0) / rng.uniform(0.38, 0.5))
    body = lowpass(rng.standard_normal(n), 0.035) * swell
    air = bandpass(rng.standard_normal(n), 900, 0.6) * swell * 0.25
    out = body / (np.abs(body).max() + 1e-9) * 0.8 + air
    tail = int(1.2 * RATE)
    place(out, rise * 0.6, crackles(rng, 1.2, 30, 0.7) * np.exp(-np.arange(tail) / RATE / 0.55))
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


def rustle(rng, seconds, centre, level=1.0):
    """Grain, straw or cloth shifted: band noise grains swelling and fading."""
    n = int(seconds * RATE)
    t = np.arange(n) / n
    grains = rng.standard_normal(n) * (0.4 + 0.6 * (rng.random(n) < 0.25)) * np.sin(np.pi * t) ** 1.2
    return (bandpass(grains, centre, 1.3) + 0.4 * bandpass(grains, centre * 0.4, 1.5)) * level


def trickle(rng, seconds, centre, rate):
    """Small hard bits falling on something soft: many short ticks, thinning out."""
    n = int(seconds * RATE)
    out = np.zeros(n)
    for _ in range(int(rate * seconds)):
        at = int(rng.uniform(0, 1) ** 1.4 * n)
        width = int(rng.uniform(0.0003, 0.0012) * RATE)
        out[at : at + width] += rng.standard_normal(min(width, n - at)) * rng.uniform(0.3, 1.0)
    return bandpass(out, centre, 1.0)


def tend(rng):
    """Food held out to an animal: a handful of grain shaken, and a soft pat."""
    out = np.zeros(int(0.55 * RATE))
    place(out, 0.0, rustle(rng, 0.3, rng.uniform(2400, 3000)), 0.7)
    place(out, 0.05, trickle(rng, 0.3, 3600, 160), 0.5)
    n = int(0.12 * RATE)
    pat = lowpass(rng.standard_normal(n), 0.06) * envelope(n, 0.002, 0.02) * 6
    place(out, 0.36, pat, 0.8)
    return out


def pouch(rng):
    """A pouch tipped out: cloth rustling and a few small things clinking together."""
    out = np.zeros(int(0.6 * RATE))
    place(out, 0.0, rustle(rng, 0.25, rng.uniform(1700, 2200)), 0.6)
    for k in range(rng.integers(3, 6)):
        at = 0.12 + k * rng.uniform(0.05, 0.09)
        place(out, at, modes(rng.uniform(2600, 3600), 0.08, [(1.0, 0.5, 0.02), (2.76, 0.25, 0.01)], attack=0.0003), rng.uniform(0.5, 0.9))
    return out


def machine(rng):
    """Something fed into a machine: a scrape at its mouth and a heavy, low clunk inside."""
    out = np.zeros(int(0.5 * RATE))
    place(out, 0.0, scrape(rng, 0.1, rng.uniform(900, 1300)), 0.3)
    base = rng.uniform(110, 140)
    clunk = modes(base, 0.3, [(1.0, 1.0, 0.07), (2.1, 0.6, 0.04), (3.9, 0.35, 0.02), (6.3, 0.2, 0.01)], attack=0.0008)
    place(out, 0.08, clunk, 1.0)
    place(out, 0.08, trickle(rng, 0.12, 2400, 90), 0.4)
    return out


def lock(rng):
    """A pick worked in a lock: small metal ticks, then the bolt giving with a click."""
    out = np.zeros(int(0.6 * RATE))
    at = 0.0
    for _ in range(rng.integers(4, 7)):
        place(out, at, modes(rng.uniform(4200, 5600), 0.03, [(1.0, 0.4, 0.004), (1.6, 0.25, 0.003)], attack=0.0002), rng.uniform(0.3, 0.6))
        at += rng.uniform(0.04, 0.07)
    bolt = modes(rng.uniform(1500, 1800), 0.12, [(1.0, 1.0, 0.02), (2.4, 0.5, 0.012), (5.1, 0.25, 0.006)], attack=0.0003)
    place(out, at + 0.04, bolt, 1.0)
    return out


def pray(rng):
    """An offering laid at an altar: a soft bell swelling up and a breath of air."""
    seconds = 1.3
    n = int(seconds * RATE)
    t = np.arange(n) / RATE
    root = rng.uniform(520, 600)
    bell = modes(root, seconds, [(1.0, 1.0, 0.7), (2.0, 0.35, 0.45), (2.76, 0.25, 0.3), (4.07, 0.12, 0.2)], attack=0.12)[:n]
    air = bandpass(rng.standard_normal(n), 2400, 0.8) * np.sin(np.pi * t / seconds) ** 2 * 0.15
    return (bell + air) * np.minimum((seconds - t) / 0.3, 1.0)


def sprinkle(rng):
    """Fertiliser spread on the soil: a fine dry scatter falling over a moment."""
    out = np.zeros(int(0.6 * RATE))
    place(out, 0.0, trickle(rng, 0.5, rng.uniform(2800, 3400), 1400), 1.0)
    place(out, 0.0, rustle(rng, 0.3, 1500), 0.6)
    return out


def toss(rng):
    """Something thrown down: a short swish through the air and a soft landing."""
    out = np.zeros(int(0.45 * RATE))
    n = int(0.16 * RATE)
    t = np.arange(n) / n
    swish = bandpass(rng.standard_normal(n) * np.sin(np.pi * t) ** 2, 900 + 1500 * t, 1.5)
    place(out, 0.0, swish, 0.8)
    n = int(0.1 * RATE)
    thud = lowpass(rng.standard_normal(n), 0.04) * envelope(n, 0.002, 0.025) * 6
    place(out, 0.2, thud + 0.3 * rustle(rng, 0.1, 1800), 0.9)
    return out


def snare(rng):
    """A snare springing shut: the bent stick whipping straight with a twang of cord, and a
    dry snap."""
    out = np.zeros(int(0.5 * RATE))
    twang = modes(rng.uniform(170, 220), 0.35, [(1.0, 1.0, 0.09), (2.02, 0.5, 0.06), (3.01, 0.3, 0.04)], attack=0.001)
    n = twang.size
    bend = np.cumsum(2 * np.pi * rng.uniform(170, 220) * (1.3 - 0.3 * np.minimum(np.arange(n) / (0.05 * RATE), 1.0)) / RATE)
    twang = twang * 0.4 + 0.6 * np.sin(bend) * np.exp(-np.arange(n) / RATE / 0.08)
    place(out, 0.02, twang, 0.8)
    place(out, 0.0, knock(rng, rng.uniform(600, 800)), 0.6)
    place(out, 0.0, crackles(rng, 0.04, 400, 1.0))
    return out


def splash(rng, seconds, centre):
    """Water broken: a wet burst of noise falling in pitch, and a few drops after."""
    n = int(seconds * RATE)
    t = np.arange(n) / RATE
    burst = bandpass(rng.standard_normal(n), centre * (0.55 + 0.45 * np.exp(-t / 0.08)), 0.9) * envelope(n, 0.004, 0.09)
    return burst + 0.4 * lowpass(rng.standard_normal(n), 0.05) * envelope(n, 0.002, 0.05)


def drop(rng, freq):
    """One drop falling back into water: a short rising bloop."""
    n = int(0.07 * RATE)
    t = np.arange(n) / RATE
    phase = np.cumsum(2 * np.pi * freq * (1 + 1.2 * t / 0.07) / RATE)
    return np.sin(phase) * envelope(n, 0.002, 0.018)


def catch(rng):
    """A fish landed: it breaks the water, drops patter back, and it flaps twice on the
    bank."""
    out = np.zeros(int(0.85 * RATE))
    place(out, 0.0, splash(rng, 0.35, rng.uniform(1400, 1800)), 1.0)
    for _ in range(5):
        place(out, rng.uniform(0.12, 0.38), drop(rng, rng.uniform(700, 1300)), rng.uniform(0.15, 0.3))
    for at in [0.5, 0.66]:
        n = int(0.06 * RATE)
        slap = bandpass(rng.standard_normal(n), rng.uniform(900, 1200), 1.4) * envelope(n, 0.001, 0.015)
        place(out, at + rng.uniform(-0.02, 0.02), slap + 0.5 * lowpass(rng.standard_normal(n), 0.04) * envelope(n, 0.001, 0.01), 0.6)
    return out


def burnt(rng):
    """Food left too long: a sharp hiss as it catches, a dull puff of smoke and a few dry
    sputters, darker than a sizzle."""
    seconds = 1.0
    n = int(seconds * RATE)
    t = np.arange(n) / RATE
    hiss = bandpass(rng.standard_normal(n), 2600 * (1 - 0.5 * np.minimum(t / 0.6, 1)), 0.8) * envelope(n, 0.005, 0.25)
    puff = lowpass(rng.standard_normal(n), 0.015) * envelope(n, 0.02, 0.18) * 3
    out = hiss + puff
    place(out, 0.25, crackles(rng, 0.6, 12, 0.7))
    return out * np.minimum((seconds - t) / 0.2, 1.0)


def cooked(rng):
    """Food done and lifted off: the sizzle easing away and a light tap on a board."""
    seconds = 0.7
    n = int(seconds * RATE)
    t = np.arange(n) / RATE
    sizzle = bandpass(rng.standard_normal(n), 4800, 0.8) * np.exp(-t / 0.22) * 0.6
    out = sizzle + crackles(rng, seconds, 18, 0.5) * np.exp(-t / 0.3)
    place(out, 0.3, knock(rng, rng.uniform(380, 460), 0.8))
    return out


def page(rng):
    """A page of the quest book turned: a quick papery swish rising, settling with a light
    flap."""
    seconds = 0.16
    n = int(seconds * RATE)
    t = np.arange(n) / RATE
    swish = bandpass(rng.standard_normal(n), 2400 + 3200 * np.minimum(t / 0.09, 1), 1.2) * envelope(n, 0.025, 0.05)
    out = swish * 0.8
    m = int(0.03 * RATE)
    flap = bandpass(rng.standard_normal(m), rng.uniform(1300, 1600), 1.6) * envelope(m, 0.001, 0.008)
    place(out, 0.085 + rng.uniform(-0.01, 0.01), flap, 0.9)
    return out


def refuse(rng):
    """Something the player tried refused: two soft, muted wooden knocks falling a step,
    as a "no" said kindly, never a buzzer."""
    out = np.zeros(int(0.3 * RATE))
    low = rng.uniform(300, 330)
    place(out, 0.0, lowpass(knock(rng, low * 1.25, 1.0), 0.25), 1.0)
    place(out, 0.11, lowpass(knock(rng, low, 1.0), 0.25), 0.85)
    return out


# Building materials (sounds.ron's `materials`): a step, a placing and a breaking for each
# material the building pieces are made of. A step is the material's short signature,
# placing adds the weight of a piece set down, breaking is longer and comes apart.


def thud(rng, seconds, k, decay, level=1.0):
    """The low, dull weight of something set down: low-passed noise dying quickly."""
    n = int(seconds * RATE)
    return lowpass(rng.standard_normal(n), k) * envelope(n, 0.002, decay) * level * 6


def stone(rng, freq, decay, level=1.0):
    """Stone struck: a few dense, quickly damped inharmonic modes and a gritty click."""
    n = int(0.3 * RATE)
    body = modes(freq, 0.3, [(1.0, 1.0, decay), (1.73, 0.7, decay * 0.7), (2.61, 0.45, decay * 0.5), (4.3, 0.25, decay * 0.3)], attack=0.0004)[:n]
    grit = bandpass(rng.standard_normal(n) * envelope(n, 0.0003, 0.006), 3200, 0.9)
    return (body + 0.5 * grit) * level


def clatter(rng, seconds, count, sound, spread=1.6):
    """Pieces falling and knocking together: `count` hits from `sound`, bunched early."""
    out = np.zeros(int(seconds * RATE))
    for i in range(count):
        at = seconds * 0.85 * rng.uniform(0, 1) ** spread
        place(out, at, sound(rng), rng.uniform(0.3, 1.0) * (1.0 if i else 1.2))
    return out


def timber_step(rng):
    """A foot on a beam or a board: a hollow knock and a breath of creak."""
    out = np.zeros(int(0.25 * RATE))
    place(out, 0.0, knock(rng, rng.uniform(150, 190)))
    place(out, 0.0, thud(rng, 0.08, 0.06, 0.02), 0.5)
    return out


def timber_place(rng):
    """A post or beam set into place: a firm knock with weight under it, a smaller one as
    it settles."""
    out = np.zeros(int(0.4 * RATE))
    place(out, 0.0, knock(rng, rng.uniform(120, 150)))
    place(out, 0.0, thud(rng, 0.12, 0.04, 0.035), 0.9)
    place(out, rng.uniform(0.06, 0.09), knock(rng, rng.uniform(220, 280)), 0.35)
    return out


def timber_break(rng):
    """Timber pulled apart: a splintering crack, fibres tearing and a board falling."""
    out = np.zeros(int(0.6 * RATE))
    place(out, 0.0, crackles(rng, 0.12, 260, 1.2))
    place(out, 0.0, knock(rng, rng.uniform(110, 140)))
    place(out, 0.03, scrape(rng, 0.18, rng.uniform(1300, 1700)), 0.5)
    place(out, rng.uniform(0.22, 0.28), knock(rng, rng.uniform(170, 210)), 0.6)
    place(out, 0.25, thud(rng, 0.1, 0.05, 0.03), 0.5)
    return out


def thatch_step(rng):
    """A foot on straw: a short dry crush of stalks."""
    out = np.zeros(int(0.22 * RATE))
    place(out, 0.0, rustle(rng, 0.14, rng.uniform(2600, 3200)))
    place(out, 0.0, thud(rng, 0.06, 0.03, 0.015), 0.25)
    return out


def thatch_place(rng):
    """A bundle of thatch laid down: a soft, full rustle and a muffled pat."""
    out = np.zeros(int(0.4 * RATE))
    place(out, 0.0, rustle(rng, 0.26, rng.uniform(2200, 2700)))
    place(out, 0.02, thud(rng, 0.1, 0.03, 0.03), 0.5)
    return out


def thatch_break(rng):
    """Thatch pulled loose: a long tearing rustle and stalks pattering down."""
    out = np.zeros(int(0.7 * RATE))
    place(out, 0.0, rustle(rng, 0.4, rng.uniform(2400, 3000), 1.2))
    place(out, 0.15, trickle(rng, 0.45, 3400, 120), 0.6)
    return out


def tile(rng):
    """A fired tile or shingle clacking: a bright, short, slightly ringing clay knock."""
    n = int(0.15 * RATE)
    freq = rng.uniform(700, 1000)
    body = modes(freq, 0.15, [(1.0, 1.0, 0.018), (2.4, 0.6, 0.012), (4.9, 0.3, 0.006)], attack=0.0003)[:n]
    tap = bandpass(rng.standard_normal(n) * envelope(n, 0.0002, 0.003), 4000, 1.0)
    return body + 0.4 * tap


def shingle_step(rng):
    """A foot on a roof of shingles: two tiles knocking under it."""
    out = np.zeros(int(0.22 * RATE))
    place(out, 0.0, tile(rng))
    place(out, rng.uniform(0.015, 0.03), tile(rng), 0.5)
    place(out, 0.0, thud(rng, 0.06, 0.05, 0.015), 0.3)
    return out


def shingle_place(rng):
    """Shingles laid: a slide of tile over tile, then a firm clack."""
    out = np.zeros(int(0.4 * RATE))
    place(out, 0.0, scrape(rng, 0.1, rng.uniform(2200, 2800)), 0.4)
    place(out, 0.09, tile(rng), 1.0)
    place(out, 0.09, thud(rng, 0.08, 0.05, 0.025), 0.5)
    return out


def shingle_break(rng):
    """Shingles prised off: a crack and a clatter of tiles sliding down."""
    out = np.zeros(int(0.65 * RATE))
    place(out, 0.0, crackles(rng, 0.05, 300, 0.8))
    place(out, 0.0, clatter(rng, 0.6, 7, tile))
    return out


def cobble_step(rng):
    """A foot on cobbles: a gritty tap of stone on stone."""
    out = np.zeros(int(0.2 * RATE))
    place(out, 0.0, stone(rng, rng.uniform(380, 480), 0.02))
    place(out, 0.0, thud(rng, 0.06, 0.05, 0.015), 0.4)
    place(out, 0.0, trickle(rng, 0.06, 2600, 300), 0.4)
    return out


def cobble_place(rng):
    """A cobble set down: the heavy knock of rough stone and grit settling."""
    out = np.zeros(int(0.45 * RATE))
    place(out, 0.0, stone(rng, rng.uniform(220, 280), 0.02))
    place(out, 0.0, thud(rng, 0.12, 0.035, 0.04), 1.0)
    place(out, 0.02, trickle(rng, 0.18, 2400, 160), 0.5)
    return out


def cobble_break(rng):
    """Cobbles knocked loose: a crack, stones tumbling and grit pouring after."""
    out = np.zeros(int(0.7 * RATE))
    place(out, 0.0, stone(rng, rng.uniform(200, 260), 0.025), 1.0)
    place(out, 0.0, thud(rng, 0.12, 0.035, 0.04))
    place(out, 0.04, clatter(rng, 0.5, 5, lambda r: stone(r, r.uniform(320, 520), 0.015)), 0.8)
    place(out, 0.1, trickle(rng, 0.5, 2200, 220), 0.6)
    return out


def brick_step(rng):
    """A foot on dressed stone: a cleaner, tighter tap than cobbles."""
    out = np.zeros(int(0.2 * RATE))
    place(out, 0.0, stone(rng, rng.uniform(500, 600), 0.024))
    place(out, 0.0, thud(rng, 0.05, 0.06, 0.012), 0.3)
    return out


def brick_place(rng):
    """A brick set in its course: a solid knock and a short scrape of mortar."""
    out = np.zeros(int(0.45 * RATE))
    place(out, 0.0, stone(rng, rng.uniform(300, 360), 0.022))
    place(out, 0.0, thud(rng, 0.1, 0.04, 0.035), 0.9)
    place(out, 0.05, scrape(rng, 0.12, rng.uniform(1600, 2000)), 0.35)
    return out


def brick_break(rng):
    """Brickwork broken: a sharp crack, the block splitting and chips falling."""
    out = np.zeros(int(0.65 * RATE))
    place(out, 0.0, stone(rng, rng.uniform(280, 340), 0.03), 1.1)
    place(out, 0.0, crackles(rng, 0.04, 400, 0.8))
    place(out, 0.06, clatter(rng, 0.45, 4, lambda r: stone(r, r.uniform(600, 800), 0.01)), 0.6)
    place(out, 0.08, trickle(rng, 0.4, 2800, 160), 0.4)
    return out


def withies(rng, seconds, level=1.0):
    """Woven sticks shifting against each other: dry little creaks and ticks of wood."""
    out = np.zeros(int(seconds * RATE))
    for _ in range(int(rng.uniform(5, 9))):
        place(out, rng.uniform(0, seconds * 0.8), knock(rng, rng.uniform(500, 900)), rng.uniform(0.1, 0.3))
    place(out, 0.0, rustle(rng, seconds * 0.9, rng.uniform(1600, 2200)), 0.5)
    return out * level


def wattle_step(rng):
    """A foot against woven wattle: a springy knock and the withies ticking."""
    out = np.zeros(int(0.25 * RATE))
    place(out, 0.0, knock(rng, rng.uniform(260, 340)), 0.8)
    place(out, 0.01, withies(rng, 0.14), 0.6)
    return out


def wattle_place(rng):
    """A wattle panel set up: a knock of its stakes and a shiver through the weave."""
    out = np.zeros(int(0.45 * RATE))
    place(out, 0.0, knock(rng, rng.uniform(180, 230)))
    place(out, 0.0, thud(rng, 0.1, 0.05, 0.03), 0.6)
    place(out, 0.03, withies(rng, 0.3), 0.7)
    return out


def wattle_break(rng):
    """Wattle pulled apart: withies snapping and rasping out of the weave."""
    out = np.zeros(int(0.65 * RATE))
    place(out, 0.0, crackles(rng, 0.2, 140, 1.0))
    place(out, 0.02, withies(rng, 0.5, 1.2))
    place(out, 0.3, knock(rng, rng.uniform(200, 260)), 0.5)
    return out


def mud(rng, seconds, centre):
    """Damp clay: a soft, low, slightly wet pat."""
    n = int(seconds * RATE)
    body = lowpass(rng.standard_normal(n), 0.05) * envelope(n, 0.003, seconds * 0.25) * 6
    return body + 0.25 * bandpass(rng.standard_normal(n) * envelope(n, 0.001, 0.012), centre, 1.2)


def daub_step(rng):
    """A foot on daub: a dull, soft pat with a little grit."""
    out = np.zeros(int(0.2 * RATE))
    place(out, 0.0, mud(rng, 0.1, rng.uniform(900, 1300)))
    place(out, 0.0, trickle(rng, 0.06, 2200, 160), 0.3)
    return out


def daub_place(rng):
    """Daub pressed on: a heavy, soft slap of clay and a smoothing smear."""
    out = np.zeros(int(0.45 * RATE))
    place(out, 0.0, mud(rng, 0.16, rng.uniform(700, 1000)), 1.0)
    place(out, 0.06, scrape(rng, 0.18, rng.uniform(900, 1300)), 0.35)
    return out


def daub_break(rng):
    """Dried daub knocked out: it cracks, crumbles and falls in clods and dust."""
    out = np.zeros(int(0.65 * RATE))
    place(out, 0.0, crackles(rng, 0.05, 250, 0.6))
    place(out, 0.0, mud(rng, 0.14, 800))
    place(out, 0.04, clatter(rng, 0.45, 4, lambda r: mud(r, 0.08, r.uniform(800, 1400))), 0.7)
    place(out, 0.08, trickle(rng, 0.45, 1900, 180), 0.5)
    return out


def drum(rng, freq, level=1.0):
    """Hide stretched on poles struck: a low, dull, quickly damped boom."""
    n = int(0.25 * RATE)
    skin = modes(freq, 0.25, [(1.0, 1.0, 0.05), (1.59, 0.5, 0.03), (2.14, 0.3, 0.02)], attack=0.002)[:n]
    return (skin + 0.2 * bandpass(rng.standard_normal(n) * envelope(n, 0.001, 0.01), 1200, 1.0)) * level


def hide_step(rng):
    """A foot against hide: a soft, leathery flap and a dull thump of the skin."""
    out = np.zeros(int(0.22 * RATE))
    place(out, 0.0, drum(rng, rng.uniform(90, 130)), 0.7)
    place(out, 0.0, rustle(rng, 0.1, rng.uniform(900, 1300)), 0.6)
    return out


def hide_place(rng):
    """A hide stretched and pegged: a slap and the skin booming low as it tautens."""
    out = np.zeros(int(0.45 * RATE))
    place(out, 0.0, rustle(rng, 0.12, rng.uniform(1100, 1500)), 0.7)
    place(out, 0.06, drum(rng, rng.uniform(80, 110)))
    place(out, 0.06, knock(rng, rng.uniform(200, 260)), 0.35)
    return out


def hide_break(rng):
    """A hide pulled down: lashings giving, leather flapping and a pole knocking."""
    out = np.zeros(int(0.65 * RATE))
    place(out, 0.0, crackles(rng, 0.06, 200, 0.6))
    place(out, 0.02, rustle(rng, 0.35, rng.uniform(900, 1300), 1.1))
    place(out, 0.15, drum(rng, rng.uniform(70, 100)), 0.6)
    place(out, 0.35, knock(rng, rng.uniform(160, 210)), 0.6)
    return out


def pot(rng):
    """Fired clay struck: a hollow, ringing earthenware knock."""
    n = int(0.25 * RATE)
    freq = rng.uniform(420, 620)
    body = modes(freq, 0.25, [(1.0, 1.0, 0.035), (2.2, 0.5, 0.02), (3.9, 0.25, 0.01)], attack=0.0004)[:n]
    tap = bandpass(rng.standard_normal(n) * envelope(n, 0.0002, 0.004), 3500, 1.0)
    return body + 0.4 * tap


def terracotta_step(rng):
    """A foot on fired clay: a hollow clink."""
    out = np.zeros(int(0.2 * RATE))
    place(out, 0.0, pot(rng))
    place(out, 0.0, thud(rng, 0.05, 0.06, 0.012), 0.3)
    return out


def terracotta_place(rng):
    """A clay pot set down: a hollow knock with weight and a scrape of grit."""
    out = np.zeros(int(0.4 * RATE))
    place(out, 0.0, pot(rng))
    place(out, 0.0, thud(rng, 0.1, 0.04, 0.03), 0.8)
    place(out, 0.04, scrape(rng, 0.08, rng.uniform(2000, 2600)), 0.3)
    return out


def terracotta_break(rng):
    """Fired clay broken: a crack, then sherds tinkling down."""
    out = np.zeros(int(0.6 * RATE))
    place(out, 0.0, pot(rng), 1.1)
    place(out, 0.0, crackles(rng, 0.04, 400, 0.9))
    place(out, 0.05, clatter(rng, 0.5, 6, tile), 0.6)
    return out


# The work of the first two ages, by what it is done with (sounds.ron's `ways`).


def knap(rng):
    """Flint knapped: a hammerstone's sharp, glassy cracks, each flake ticking away."""
    out = np.zeros(int(0.6 * RATE))
    at = 0.0
    for blow in range(3):
        level = (0.7, 0.85, 1.0)[blow]
        place(out, at, stone(rng, rng.uniform(1100, 1500), 0.02), level)
        place(out, at, thud(rng, 0.05, 0.08, 0.015), 0.4 * level)
        n = int(0.03 * RATE)
        snap = bandpass(rng.standard_normal(n) * envelope(n, 0.0001, 0.003), rng.uniform(5000, 6500), 1.4)
        place(out, at, snap, 0.5 * level)
        place(out, at + rng.uniform(0.03, 0.06), modes(rng.uniform(2600, 3400), 0.05, [(1.0, 0.3, 0.008), (1.7, 0.2, 0.005)], attack=0.0002), 0.6 * level)
        at += rng.uniform(0.14, 0.19)
    return out


def scrape_hide(rng):
    """A hide scraped: two long, rasping strokes of a stone edge over a stretched skin."""
    out = np.zeros(int(0.85 * RATE))
    for stroke in range(2):
        n = int(rng.uniform(0.28, 0.34) * RATE)
        t = np.arange(n) / n
        rasp = rng.standard_normal(n) * (1 + 0.8 * np.sin(2 * np.pi * rng.uniform(55, 75) * np.arange(n) / RATE))
        rasp *= np.sin(np.pi * t) ** 1.2
        body = bandpass(rasp, 1700 + 900 * t, 1.6) + 0.5 * bandpass(rasp, 420, 2.5)
        place(out, 0.04 + stroke * 0.4, body, 0.7 + 0.3 * stroke)
    return out


def shape_clay(rng):
    """Clay shaped by hand: wet, soft slaps and a squelch as it is pressed."""
    out = np.zeros(int(0.6 * RATE))
    for slap in range(3):
        n = int(0.09 * RATE)
        body = lowpass(rng.standard_normal(n), 0.12) * envelope(n, 0.001, 0.02) * 6
        wet = bandpass(rng.standard_normal(n) * envelope(n, 0.0005, 0.012), rng.uniform(900, 1300), 1.8)
        place(out, slap * rng.uniform(0.13, 0.17), body + 0.8 * wet, (0.8, 0.7, 1.0)[slap])
    n = int(0.2 * RATE)
    t = np.arange(n) / n
    squelch = bandpass(rng.standard_normal(n) * np.sin(np.pi * t), 500 + 500 * t, 3.0)
    place(out, 0.36, squelch, 0.7)
    return out


def clink(rng):
    """Fired clay knocked together: bright, short, ringing clinks of pottery."""
    out = np.zeros(int(0.5 * RATE))
    for knock_ in range(2):
        place(out, knock_ * rng.uniform(0.11, 0.16), tile(rng), (0.8, 1.0)[knock_])
    place(out, 0.02, modes(rng.uniform(1500, 1900), 0.2, [(1.0, 0.4, 0.05), (2.6, 0.2, 0.03)], attack=0.0003), 0.4)
    return out


def dress_stone(rng):
    """Stone dressed: a hammerstone pecking at rock, grit falling from each blow."""
    out = np.zeros(int(0.6 * RATE))
    for blow in range(3):
        at = blow * rng.uniform(0.13, 0.17)
        place(out, at, stone(rng, rng.uniform(600, 820), 0.014), (0.75, 0.85, 1.0)[blow])
        place(out, at, thud(rng, 0.05, 0.06, 0.012), 0.3)
        place(out, at + 0.01, trickle(rng, 0.1, 2600, 220), 0.35)
    return out


def twist(rng):
    """Fibre twisted into cord: dry stalks creaking as they wind tight, a soft rustle."""
    out = np.zeros(int(0.6 * RATE))
    place(out, 0.0, rustle(rng, 0.45, 2400), 0.5)
    place(out, 0.08, creak(rng, 0.38, 70, 140, 900), 0.8)
    return out


def whittle(rng):
    """Wood whittled: quick knife strokes peeling shavings, and a knock of the stick."""
    out = np.zeros(int(0.55 * RATE))
    for stroke in range(3):
        at = stroke * rng.uniform(0.11, 0.14)
        place(out, at, scrape(rng, rng.uniform(0.07, 0.09), rng.uniform(2600, 3300)), (0.7, 0.8, 0.9)[stroke])
    place(out, 0.4, knock(rng, rng.uniform(380, 460)), 0.6)
    return out


def hang(rng):
    """Meat hung on a drying rack: a wet slap on the rail and the poles creaking."""
    out = np.zeros(int(0.55 * RATE))
    n = int(0.1 * RATE)
    slap = lowpass(rng.standard_normal(n), 0.15) * envelope(n, 0.001, 0.025) * 6
    place(out, 0.0, slap + 0.6 * bandpass(rng.standard_normal(n) * envelope(n, 0.0005, 0.01), 1100, 1.6), 0.9)
    place(out, 0.02, knock(rng, rng.uniform(300, 360)), 0.5)
    place(out, 0.1, creak(rng, 0.3, 90, 60, 700), 0.6)
    return out


def stir(rng):
    """Food tipped into a pot of water: a plop, a swirl of the ladle and a knock on the rim."""
    out = np.zeros(int(0.75 * RATE))
    n = int(0.12 * RATE)
    t = np.arange(n) / RATE
    plop = np.sin(2 * np.pi * (500 + 1400 * t / t[-1]) * t) * envelope(n, 0.001, 0.03)
    place(out, 0.0, plop + 0.3 * lowpass(rng.standard_normal(n), 0.2) * envelope(n, 0.001, 0.02), 0.8)
    m = int(0.4 * RATE)
    s = np.arange(m) / m
    swirl = bandpass(rng.standard_normal(m) * np.sin(np.pi * s) * (1 + 0.6 * np.sin(2 * np.pi * 7 * s)), 700 + 300 * np.sin(2 * np.pi * 2 * s), 2.0)
    place(out, 0.1, swirl, 0.7)
    place(out, 0.55, tile(rng), 0.5)
    return out


def heap(rng):
    """Greens forked onto a compost heap: a soft, wet flump and a damp rustle settling."""
    out = np.zeros(int(0.55 * RATE))
    place(out, 0.0, thud(rng, 0.14, 0.03, 0.05), 1.0)
    place(out, 0.0, rustle(rng, 0.3, 1300), 0.6)
    place(out, 0.12, trickle(rng, 0.3, 1500, 90), 0.5)
    return out


def clamp(rng):
    """An earth clamp loaded: things laid in with a hollow knock, then soil shovelled over."""
    out = np.zeros(int(0.7 * RATE))
    place(out, 0.0, knock(rng, rng.uniform(200, 240)), 0.8)
    place(out, 0.0, thud(rng, 0.1, 0.04, 0.03), 0.6)
    place(out, 0.18, thud(rng, 0.15, 0.025, 0.05), 0.8)
    place(out, 0.2, trickle(rng, 0.45, 1600, 260), 0.8)
    return out


def kiln_load(rng):
    """Clay set into a pit kiln: a pot knocked into the embers and a breath of flame."""
    out = np.zeros(int(0.8 * RATE))
    place(out, 0.0, tile(rng), 0.8)
    place(out, 0.02, thud(rng, 0.08, 0.05, 0.02), 0.4)
    n = int(0.6 * RATE)
    t = np.arange(n) / RATE
    breath = lowpass(rng.standard_normal(n), 0.06) * np.minimum(t / 0.12, 1.0) * np.exp(-t / 0.25)
    place(out, 0.08, breath / (np.abs(breath).max() + 1e-9), 0.5)
    place(out, 0.1, crackles(rng, 0.6, 40, 0.8))
    return out


def smelt(rng):
    """Ore tipped into a hot crucible: a rattle of stones on clay and a deep hiss."""
    out = np.zeros(int(0.85 * RATE))
    place(out, 0.0, clatter(rng, 0.3, 5, lambda r: stone(r, r.uniform(700, 1000), 0.01)), 0.8)
    n = int(0.6 * RATE)
    t = np.arange(n) / RATE
    hiss = bandpass(rng.standard_normal(n), 3600, 0.7) * np.minimum(t / 0.05, 1.0) * np.exp(-t / 0.22)
    place(out, 0.2, hiss, 0.6)
    place(out, 0.2, lowpass(rng.standard_normal(n), 0.03) * np.exp(-t / 0.3) * 4, 0.5)
    return out


def kindle(rng, depth):
    """A hearth or kiln lit: a strike, the kindling catching with a whoomph, and the fire
    swelling into a roar that settles into its bed. `depth` lowers the roar (a kiln)."""
    out = np.zeros(int(1.4 * RATE))
    n = int(0.05 * RATE)
    click = rng.standard_normal(n) * envelope(n, 0.0002, 0.005)
    place(out, 0.0, bandpass(click, 4200, 1.2), 0.8)
    place(out, 0.0, modes(rng.uniform(3800, 4600), 0.08, [(1.0, 0.3, 0.012), (1.48, 0.2, 0.008)], attack=0.0003), 0.8)
    m = int(1.2 * RATE)
    t = np.arange(m) / RATE
    swell = np.minimum(t / 0.35, 1.0) ** 1.5 * np.exp(-np.maximum(t - 0.45, 0) / 0.5)
    whoomph = lowpass(rng.standard_normal(m), 0.025 / depth) * np.exp(-t / 0.12) * 1.5
    roar = lowpass(rng.standard_normal(m), 0.07 / depth) * swell
    flutter = 1 + 0.35 * np.sin(2 * np.pi * rng.uniform(5, 8) * t)
    body = whoomph + roar * flutter
    place(out, 0.12, body / (np.abs(body).max() + 1e-9), 1.0)
    place(out, 0.3, crackles(rng, 1.0, 30, 0.7) * np.exp(-np.arange(int(1.0 * RATE)) / RATE / 0.6))
    return out


def kindle_hearth(rng):
    return kindle(rng, 1.0)


def kindle_kiln(rng):
    return kindle(rng, 1.6)


def shear(rng):
    """Wool shorn: crisp snips of iron blades closing, the fleece rustling as it parts."""
    out = np.zeros(int(0.75 * RATE))
    place(out, 0.0, rustle(rng, 0.65, rng.uniform(1600, 2000)), 0.6)
    for snip in range(3):
        at = 0.05 + snip * rng.uniform(0.17, 0.21)
        n = int(0.04 * RATE)
        blades = bandpass(rng.standard_normal(n) * envelope(n, 0.0005, 0.014), rng.uniform(4600, 5600), 1.2)
        place(out, at, blades, 0.6)
        place(out, at + 0.004, modes(rng.uniform(2700, 3200), 0.12, [(1.0, 0.25, 0.03), (2.3, 0.12, 0.015)], attack=0.0002), 0.2)
    return out


def milk(rng):
    """Milk drawn into a pail: hissing squirts, two hands in turn, ringing on the wood."""
    out = np.zeros(int(0.9 * RATE))
    for squirt in range(4):
        n = int(rng.uniform(0.1, 0.13) * RATE)
        t = np.arange(n) / n
        jet = rng.standard_normal(n) * np.sin(np.pi * t ** 0.6) ** 1.5
        body = bandpass(jet, rng.uniform(2600, 3200) - 600 * t, 1.1) + 0.5 * bandpass(jet, 700, 3.0)
        at = squirt * rng.uniform(0.19, 0.23)
        place(out, at, body, (0.8, 0.9, 0.85, 1.0)[squirt])
        place(out, at + 0.02, modes(rng.uniform(320, 380), 0.25, [(1.0, 0.3, 0.06), (2.7, 0.1, 0.03)], attack=0.004), 0.25)
    return out


def hive(rng):
    """Bees calmed with smoke: two breathy puffs, and the swarm's hum swelling and settling."""
    out = np.zeros(int(1.0 * RATE))
    for puff in range(2):
        n = int(0.16 * RATE)
        place(out, puff * 0.22, lowpass(rng.standard_normal(n), 0.09) * envelope(n, 0.02, 0.05) * 3, 0.7)
    n = int(0.9 * RATE)
    t = np.arange(n) / RATE
    swell = np.minimum(t / 0.3, 1.0) * np.exp(-np.maximum(t - 0.4, 0) / 0.3)
    hum = sum(np.sin(2 * np.pi * f * t + rng.uniform(0, 6.3)) for f in rng.uniform(205, 245, 6))
    hum = hum * (1 + 0.4 * np.sin(2 * np.pi * rng.uniform(5, 9) * t))
    place(out, 0.1, bandpass(hum + 0.3 * rng.standard_normal(n), 470, 0.9) * swell, 0.6)
    return out


def gate(rng):
    """A field gate swung: a wooden latch bar lifted with a clack, a light, dry creak, and
    the gate knocking against its post."""
    out = np.zeros(int(0.9 * RATE))
    place(out, 0.0, knock(rng, rng.uniform(520, 620)), 0.6)
    swing = rng.uniform(0.38, 0.48)
    place(out, 0.07, creak(rng, swing, 45, 110, rng.uniform(600, 760)), 0.6)
    place(out, 0.07 + swing, knock(rng, rng.uniform(240, 290)), 0.7)
    return out


def shutters(rng):
    """Shutters folded: two light leaves swinging on small hinges, each knocking home."""
    out = np.zeros(int(0.6 * RATE))
    for leaf in range(2):
        at = leaf * rng.uniform(0.16, 0.21)
        place(out, at, creak(rng, 0.12, 80, 160, rng.uniform(800, 950)), 0.35)
        place(out, at + 0.11, knock(rng, rng.uniform(330, 400)), (0.75, 0.9)[leaf])
    return out


def hatch(rng):
    """A loft hatch: a heavy board creaking on its hinges and dropping home with a thump."""
    out = np.zeros(int(0.8 * RATE))
    swing = rng.uniform(0.3, 0.38)
    place(out, 0.0, creak(rng, swing, 30, 70, rng.uniform(300, 380)), 0.7)
    place(out, swing - 0.02, knock(rng, rng.uniform(110, 140)), 1.0)
    place(out, swing - 0.02, thud(rng, 0.12, 0.03, 0.03), 0.6)
    return out


def curtain(rng):
    """A curtain drawn: cloth swishing along, its rings sliding and ticking on the pole."""
    out = np.zeros(int(0.6 * RATE))
    n = int(0.45 * RATE)
    t = np.arange(n) / n
    swish = bandpass(rng.standard_normal(n) * np.sin(np.pi * t) ** 1.4, 1500 + 900 * t, 0.9)
    place(out, 0.02, swish, 0.8)
    for ring in range(5):
        place(out, 0.04 + ring * rng.uniform(0.06, 0.08), modes(rng.uniform(2400, 3000), 0.05, [(1.0, 0.3, 0.012), (2.4, 0.15, 0.006)], attack=0.0003), 0.18)
    return out


def honey(rng):
    """Comb cut from a full skep: a sticky tearing, and honey dripping into the pot."""
    out = np.zeros(int(0.9 * RATE))
    n = int(0.3 * RATE)
    t = np.arange(n) / n
    tear = rng.standard_normal(n) * (rng.random(n) < 0.3) * np.sin(np.pi * t)
    place(out, 0.0, bandpass(tear, 1300, 1.6) + 0.4 * lowpass(tear, 0.1), 0.8)
    for drip in range(3):
        m = int(0.07 * RATE)
        u = np.arange(m) / RATE
        plop = np.sin(2 * np.pi * (rng.uniform(380, 460) + 900 * u / u[-1]) * u) * envelope(m, 0.002, 0.018)
        place(out, 0.36 + drip * rng.uniform(0.13, 0.17), plop, (0.6, 0.5, 0.45)[drip])
    return out


def shut(rng):
    """A wooden door pushed shut: a short creak on its hinges, the door meeting its frame
    with a deep thud, and the latch dropping into its keeper."""
    out = np.zeros(int(0.8 * RATE))
    swing = rng.uniform(0.22, 0.3)
    place(out, 0.0, creak(rng, swing, 40, 90, rng.uniform(380, 460)), 0.6)
    place(out, swing, knock(rng, rng.uniform(95, 120)), 1.0)
    place(out, swing, thud(rng, 0.15, 0.025, 0.04), 0.8)
    n = int(0.03 * RATE)
    latch = modes(rng.uniform(1700, 2100), 0.05, [(1.0, 0.6, 0.012), (2.6, 0.3, 0.006)], attack=0.0003)[:n]
    place(out, swing + rng.uniform(0.06, 0.09), latch + bandpass(rng.standard_normal(n) * envelope(n, 0.0002, 0.003), 3000, 1.0), 0.4)
    return out


def store_barrel(rng):
    """A barrel's lid prised up: wood scraping on the staves, and the hollow, low boom of
    the cask as the lid is set against it."""
    out = np.zeros(int(0.7 * RATE))
    place(out, 0.0, scrape(rng, rng.uniform(0.14, 0.2), rng.uniform(900, 1200)), 0.5)
    cask = modes(rng.uniform(85, 105), 0.4, [(1.0, 1.0, 0.12), (1.58, 0.5, 0.08), (2.4, 0.3, 0.05), (3.9, 0.12, 0.02)], attack=0.001)
    at = rng.uniform(0.17, 0.22)
    place(out, at, cask, 0.7)
    place(out, at, knock(rng, rng.uniform(200, 240)), 0.5)
    return out


def store_cupboard(rng):
    """A cupboard door opened: a little latch, then a small, quick creak of its hinges."""
    out = np.zeros(int(0.5 * RATE))
    n = int(0.025 * RATE)
    latch = modes(rng.uniform(2300, 2700), 0.04, [(1.0, 0.6, 0.008), (2.8, 0.3, 0.004)], attack=0.0003)[:n]
    place(out, 0.0, latch, 0.3)
    place(out, 0.04, creak(rng, rng.uniform(0.2, 0.26), 70, 150, rng.uniform(750, 900)), 0.7)
    return out


def store_jar(rng):
    """A jar's clay lid lifted: a gritty scrape of glaze on glaze and a ringing clink as it
    is set down beside it."""
    out = np.zeros(int(0.5 * RATE))
    place(out, 0.0, scrape(rng, 0.08, rng.uniform(2800, 3400)), 0.35)
    place(out, rng.uniform(0.1, 0.14), tile(rng), 0.9)
    place(out, 0.12, modes(rng.uniform(1300, 1600), 0.25, [(1.0, 0.5, 0.07), (2.7, 0.2, 0.03)], attack=0.0003), 0.35)
    return out


def store_basket(rng):
    """A lidded basket opened: the dry, crackling creak of wicker flexing and a soft rustle
    of what lies inside."""
    out = np.zeros(int(0.6 * RATE))
    place(out, 0.0, crackles(rng, 0.28, 90, 0.5), 0.8)
    place(out, 0.0, creak(rng, 0.25, 120, 220, rng.uniform(1300, 1600)), 0.25)
    place(out, 0.2, rustle(rng, 0.3, rng.uniform(1800, 2300)), 0.5)
    return out


def bed_frame(rng):
    """Lying down on a wooden bed: the frame creaking under the weight, the straw tick
    rustling, and a low settling."""
    out = sleep(rng)
    place(out, 0.28, creak(rng, rng.uniform(0.35, 0.45), 25, 55, rng.uniform(260, 330)), 0.5)
    return out


def seat_wood(rng):
    """Sitting on a wooden seat: a quick knock of its legs on the floor and a short creak
    as it takes the weight."""
    out = np.zeros(int(0.5 * RATE))
    place(out, 0.0, knock(rng, rng.uniform(160, 200)), 0.6)
    place(out, 0.04, creak(rng, rng.uniform(0.18, 0.24), 35, 75, rng.uniform(420, 520)), 0.55)
    place(out, 0.02, rustle(rng, 0.2, 1500), 0.25)
    return out


def grind(rng):
    """A quern turned: the upper stone rumbling round on the lower, a turn and more of gritty,
    low grinding, and grain crunching between them."""
    seconds = 1.4
    n = int(seconds * RATE)
    t = np.arange(n) / RATE
    turns = rng.uniform(1.0, 1.25)
    sweep = 0.55 + 0.45 * np.abs(np.sin(np.pi * turns * t))
    rumble = lowpass(rng.standard_normal(n), 0.02) * 5 * sweep
    grit = bandpass(rng.standard_normal(n) * (rng.random(n) < 0.08), 2600, 1.2) * sweep
    out = rumble + 0.6 * grit + 0.5 * bandpass(rng.standard_normal(n), 420 + 120 * sweep, 3.0) * sweep
    out += crackles(rng, seconds, 22, 0.5)
    return out * np.minimum(t / 0.08, 1.0) * np.minimum((seconds - t) / 0.2, 1.0)


def spin(rng):
    """A spindle spun: a flick of the fingers, the whorl whirring as it turns and slows,
    and the wool drawing out with a soft rasp."""
    seconds = 1.4
    n = int(seconds * RATE)
    t = np.arange(n) / RATE
    speed = 9.0 * np.exp(-t / rng.uniform(1.4, 1.8)) + 2.0
    phase = np.cumsum(speed / RATE)
    hum = np.sin(2 * np.pi * phase * 28) * (0.5 + 0.5 * np.sin(2 * np.pi * phase))
    whirr = bandpass(rng.standard_normal(n), 900, 2.0) * (0.6 + 0.4 * np.sin(2 * np.pi * phase)) * (speed / 11)
    out = 0.35 * hum * (speed / 11) + whirr
    place(out, 0.0, knock(rng, rng.uniform(600, 700)), 0.4)
    place(out, 0.3, rustle(rng, 1.0, 2600), 0.35)
    return out * np.minimum(t / 0.01, 1.0) * np.minimum((seconds - t) / 0.25, 1.0)


def weave(rng):
    """A loom worked: the shuttle swishing through the shed and the beater clacking the
    weft home, twice, the frame knocking under it."""
    out = np.zeros(int(1.4 * RATE))
    for pick in range(2):
        at = pick * rng.uniform(0.6, 0.66)
        m = int(0.32 * RATE)
        s = np.arange(m) / m
        swish = bandpass(rng.standard_normal(m) * np.sin(np.pi * s) ** 2, 1800 + 1400 * s, 1.5)
        place(out, at, swish, 0.5)
        place(out, at + 0.4, knock(rng, rng.uniform(260, 320)), 0.9)
        place(out, at + 0.41, knock(rng, rng.uniform(130, 160)), 0.6)
        place(out, at + 0.5, creak(rng, 0.2, 50, 100, 500), 0.25)
    return out


def flour(rng):
    """Flour gathered up off the quern: a soft, dusty sift and a pat of the hand."""
    out = np.zeros(int(0.5 * RATE))
    place(out, 0.0, trickle(rng, 0.35, 3800, 160), 0.5)
    place(out, 0.3, thud(rng, 0.1, 0.05, 0.02), 0.5)
    return out


def twine_wound(rng):
    """Spun twine wound off the spindle: a dry, quick winding creak and a snap of the
    thread."""
    out = np.zeros(int(0.45 * RATE))
    place(out, 0.0, creak(rng, 0.25, 110, 220, 1100), 0.5)
    n = int(0.03 * RATE)
    place(out, 0.27, bandpass(rng.standard_normal(n) * envelope(n, 0.0002, 0.006), 3200, 1.5), 0.8)
    return out


def cloth_off(rng):
    """Cloth taken off the loom: a soft fold of fabric shaken out and laid down."""
    out = np.zeros(int(0.55 * RATE))
    m = int(0.18 * RATE)
    s = np.arange(m) / m
    place(out, 0.0, bandpass(rng.standard_normal(m) * np.sin(np.pi * s), 1400, 1.0), 0.6)
    place(out, 0.2, rustle(rng, 0.3, 1600), 0.6)
    return out


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
    "fire_catch": (catch, 2),
    "store_open": (store_open, 2),
    "door": (door, 2),
    "cook": (cook, 2),
    "sleep": (sleep, 1),
    "cast": (cast, 2),
    "pickup": (pickup, 4),
    "tend": (tend, 2),
    "pouch": (pouch, 2),
    "machine": (machine, 2),
    "lock": (lock, 2),
    "pray": (pray, 1),
    "sprinkle": (sprinkle, 2),
    "toss": (toss, 2),
    "snare": (snare, 2),
    "catch": (catch, 2),
    "burnt": (burnt, 2),
    "cooked": (cooked, 2),
    "page": (page, 2),
    "refuse": (refuse, 2),
    "materials/timber_step": (timber_step, 4),
    "materials/timber_place": (timber_place, 3),
    "materials/timber_break": (timber_break, 3),
    "materials/thatch_step": (thatch_step, 4),
    "materials/thatch_place": (thatch_place, 3),
    "materials/thatch_break": (thatch_break, 3),
    "materials/shingle_step": (shingle_step, 4),
    "materials/shingle_place": (shingle_place, 3),
    "materials/shingle_break": (shingle_break, 3),
    "materials/cobble_step": (cobble_step, 4),
    "materials/cobble_place": (cobble_place, 3),
    "materials/cobble_break": (cobble_break, 3),
    "materials/brick_step": (brick_step, 4),
    "materials/brick_place": (brick_place, 3),
    "materials/brick_break": (brick_break, 3),
    "materials/wattle_step": (wattle_step, 4),
    "materials/wattle_place": (wattle_place, 3),
    "materials/wattle_break": (wattle_break, 3),
    "materials/daub_step": (daub_step, 4),
    "materials/daub_place": (daub_place, 3),
    "materials/daub_break": (daub_break, 3),
    "materials/hide_step": (hide_step, 4),
    "materials/hide_place": (hide_place, 3),
    "materials/hide_break": (hide_break, 3),
    "materials/terracotta_step": (terracotta_step, 4),
    "materials/terracotta_place": (terracotta_place, 3),
    "materials/terracotta_break": (terracotta_break, 3),
    "work/knap": (knap, 3),
    "work/scrape_hide": (scrape_hide, 2),
    "work/shape_clay": (shape_clay, 2),
    "work/clink": (clink, 2),
    "work/dress_stone": (dress_stone, 3),
    "work/twist": (twist, 2),
    "work/whittle": (whittle, 3),
    "work/hang": (hang, 2),
    "work/stir": (stir, 2),
    "work/heap": (heap, 2),
    "work/clamp": (clamp, 2),
    "work/kiln_load": (kiln_load, 2),
    "work/smelt": (smelt, 2),
    "work/kindle_hearth": (kindle_hearth, 2),
    "work/kindle_kiln": (kindle_kiln, 2),
    "work/shear": (shear, 2),
    "work/milk": (milk, 2),
    "work/hive": (hive, 2),
    "work/honey": (honey, 2),
    "gate": (gate, 2),
    "shutters": (shutters, 2),
    "hatch": (hatch, 2),
    "curtain": (curtain, 2),
    "shut": (shut, 2),
    "store_barrel": (store_barrel, 2),
    "store_cupboard": (store_cupboard, 2),
    "store_jar": (store_jar, 2),
    "store_basket": (store_basket, 2),
    "bed_frame": (bed_frame, 2),
    "seat_wood": (seat_wood, 2),
    "work/grind": (grind, 2),
    "work/spin": (spin, 2),
    "work/weave": (weave, 2),
    "work/flour": (flour, 2),
    "work/twine": (twine_wound, 2),
    "work/cloth": (cloth_off, 2),
}


def main() -> None:
    only = set(sys.argv[1:])
    for name, (make, takes) in SOUNDS.items():
        if only and name not in only:
            continue
        for take in range(1, takes + 1):
            rng = np.random.default_rng(7000 + 1000 * take + sum(map(ord, name)))
            stereo = finish(make(rng), rng)
            path = OUT / f"{name}_{take}.ogg"
            path.parent.mkdir(exist_ok=True)
            write_ogg(stereo, path)
            print(f"{path.relative_to(ROOT)}: {stereo.shape[1] / RATE:.2f} s, loudest {loudness(stereo):.3f}, peak {np.abs(stereo).max():.2f}", file=sys.stderr)


if __name__ == "__main__":
    main()
