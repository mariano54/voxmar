#!/usr/bin/env python3
"""Synthesizes the stand-in voice of settlements' people (`person` in
creature_sounds.ron): hums, a surprised "oh", grunts when struck and a falling cry; and
the barks (a greeting, an alarm, a content sigh).

A source-filter voice: a glottal pulse train with a little jitter and breath, shaped by
formant resonators for each vowel, under an envelope. It stands in until recordings of
real voices replace it. Writes ogg files with ffmpeg.

    python3 tools/synth_person_voice.py assets/content/base/sounds/creatures/person [names...]
"""

import math
import os
import subprocess
import sys
import tempfile
import wave

import numpy as np

RATE = 44100

# Formants (frequency, bandwidth) of the vowels used.
VOWELS = {
    "m": [(260, 80), (1000, 150), (2300, 300)],
    "u": [(330, 70), (850, 100), (2250, 200)],
    "o": [(520, 80), (860, 100), (2450, 200)],
    "a": [(720, 90), (1150, 120), (2500, 200)],
    "e": [(480, 70), (1800, 120), (2550, 200)],
}


def resonator(x, freq, bw):
    """A two-pole resonator (Klatt's), gain one at its peak."""
    t = 1.0 / RATE
    c = -math.exp(-2 * math.pi * bw * t)
    b = 2 * math.exp(-math.pi * bw * t) * math.cos(2 * math.pi * freq * t)
    a = 1 - b - c
    y = np.zeros_like(x)
    y1 = y2 = 0.0
    for i, v in enumerate(x):
        y0 = a * v + b * y1 + c * y2
        y[i] = y0
        y2, y1 = y1, y0
    return y


def voice(seconds, pitch, vowel, envelope, breath=0.08, seed=1):
    """A voiced sound: pitch (Hz) and envelope are functions of time 0..1."""
    rng = np.random.default_rng(seed)
    n = int(seconds * RATE)
    out = np.zeros(n)
    phase = 0.0
    for i in range(n):
        f = pitch(i / n) * (1 + 0.004 * rng.standard_normal())
        phase += f / RATE
        if phase >= 1.0:
            phase -= 1.0
        # Rosenberg-like glottal pulse: open 60% of the period.
        out[i] = math.sin(math.pi * phase / 0.6) ** 2 if phase < 0.6 else 0.0
    out = np.diff(out, prepend=0.0)
    out += breath * rng.standard_normal(n) * 0.05
    shaped = sum(resonator(out, f, bw) * (1.0 / (k + 1)) for k, (f, bw) in enumerate(VOWELS[vowel]))
    env = np.array([envelope(i / n) for i in range(n)])
    return shaped * env


def glide(a, b):
    return lambda t: a + (b - a) * t


def swell(attack=0.15, release=0.35):
    def e(t):
        if t < attack:
            return t / attack
        if t > 1 - release:
            return max(0.0, (1 - t) / release)
        return 1.0
    return e


def burst(t):
    return min(1.0, t / 0.05) * math.exp(-3.5 * t)


def mix(*parts, gap=0.04):
    pieces = []
    for p in parts:
        pieces.append(p)
        pieces.append(np.zeros(int(gap * RATE)))
    return np.concatenate(pieces[:-1])


def finish(x, level=0.6):
    x = x - x.mean()
    peak = np.abs(x).max()
    x = x / peak * level
    fade = int(0.02 * RATE)
    x[-fade:] *= np.linspace(1, 0, fade)
    return np.concatenate([x, np.zeros(int(0.01 * RATE))])


CLIPS = {
    # A hum while busy, a hum of agreement, a soft "ah", a questioning "hm?".
    "idle_1": lambda: voice(0.42, glide(128, 112), "m", swell(), seed=1),
    "idle_2": lambda: mix(voice(0.16, glide(122, 132), "m", swell(0.2, 0.3), seed=2), voice(0.22, glide(138, 108), "m", swell(0.1, 0.5), seed=3)),
    "idle_3": lambda: voice(0.3, glide(118, 104), "a", swell(0.25, 0.5), breath=0.3, seed=4),
    "idle_4": lambda: voice(0.34, glide(108, 150), "m", swell(0.2, 0.3), seed=5),
    # Surprised.
    "alert_1": lambda: voice(0.26, glide(170, 140), "o", swell(0.08, 0.4), seed=6),
    "alert_2": lambda: voice(0.3, glide(120, 190), "e", swell(0.12, 0.3), breath=0.2, seed=7),
    # Struck.
    "hurt_1": lambda: voice(0.22, glide(190, 120), "u", burst, breath=0.6, seed=8),
    "hurt_2": lambda: voice(0.26, glide(210, 140), "a", burst, breath=0.5, seed=9),
    "hurt_3": lambda: voice(0.2, glide(170, 130), "o", burst, breath=0.7, seed=10),
    # A falling cry.
    "death_1": lambda: voice(0.9, glide(200, 90), "a", swell(0.06, 0.7), breath=0.4, seed=11),
    # Barks (Server::bark). A greeting: a bright "mm-hm!" rising, a "heh" and a hum up.
    "greet_1": lambda: mix(voice(0.14, glide(132, 140), "m", swell(0.2, 0.3), seed=12), voice(0.24, glide(150, 182), "m", swell(0.1, 0.45), seed=13), gap=0.05),
    "greet_2": lambda: voice(0.24, glide(150, 178), "e", swell(0.1, 0.5), breath=0.25, seed=14),
    "greet_3": lambda: mix(voice(0.12, glide(140, 146), "m", swell(0.2, 0.3), seed=15), voice(0.28, glide(160, 150), "a", swell(0.1, 0.5), breath=0.15, seed=16), gap=0.03),
    # Alarm: a sharp "hey!" rising, and a "hey, hey!".
    "alarm_1": lambda: voice(0.32, glide(190, 270), "e", swell(0.04, 0.35), breath=0.3, seed=17),
    "alarm_2": lambda: mix(voice(0.18, glide(200, 250), "e", swell(0.04, 0.4), breath=0.3, seed=18), voice(0.24, glide(220, 285), "a", swell(0.04, 0.4), breath=0.3, seed=19), gap=0.07),
    # Content: a slow, breathy sigh falling away, and a settled "mm".
    "content_1": lambda: voice(0.7, glide(135, 92), "a", swell(0.3, 0.6), breath=0.9, seed=20),
    "content_2": lambda: voice(0.55, glide(120, 98), "m", swell(0.25, 0.6), breath=0.5, seed=21),
}


def main():
    out = sys.argv[1]
    only = set(sys.argv[2:])
    os.makedirs(out, exist_ok=True)
    for name, make in CLIPS.items():
        if only and name not in only:
            continue
        samples = finish(make())
        with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as tmp:
            path = tmp.name
        with wave.open(path, "wb") as w:
            w.setnchannels(1)
            w.setsampwidth(2)
            w.setframerate(RATE)
            w.writeframes((samples * 32767).astype("<i2").tobytes())
        subprocess.run(["ffmpeg", "-loglevel", "error", "-y", "-i", path, "-c:a", "libvorbis", "-q:a", "3", os.path.join(out, name + ".ogg")], check=True)
        os.unlink(path)


if __name__ == "__main__":
    main()
