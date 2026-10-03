#!/usr/bin/env python3
"""Generates the game's background music with Google Lyria 3 through OpenRouter.

Run once offline; the tracks are written to assets/music/ and shipped with the game, so
nothing calls the API at runtime. Existing tracks are skipped, so re-running only fills
gaps. Needs OPENROUTER_KEY in the environment or in .env, and ffmpeg to re-encode.

    python3 tools/generate_music.py
"""

import base64
import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "assets" / "music"
MODEL = "google/lyria-3-pro-preview"
STYLE = (
    "Instrumental only, absolutely no vocals or singing. Calm, gentle background music for a "
    "retro fantasy exploration game, in the spirit of 16-bit era RPG soundtracks. Soft dynamics, "
    "no drums kicking in, loops naturally, about two and a half minutes."
)
TRACKS = {
    "dawn_meadow": "Peaceful morning over green hills. Fingerpicked lute, warm analog synth pads, a soft wooden flute melody, 72 bpm, D major.",
    "old_forest": "An ancient quiet forest. Slow harp arpeggios, mellow recorder, distant chimes, a hint of mystery, 66 bpm, A dorian.",
    "lantern_village": "A cozy village at dusk. Music box, nylon guitar, round soft bass, light shaker, 80 bpm, F major.",
    "starlit_night": "A clear starry night. Celesta, airy wordless pads, slow and spacious, 60 bpm, E minor.",
    "highland_wind": "Wide windy highlands. Ocarina melody over a gentle drone, soft hand drum far away, 70 bpm, G mixolydian.",
    "rain_on_stone": "Soft rain on old stone ruins. Felt piano and a soft retro square-wave lead, contemplative, 64 bpm, C major.",
}


def api_key() -> str:
    if key := os.environ.get("OPENROUTER_KEY"):
        return key
    env = ROOT / ".env"
    if env.exists():
        for line in env.read_text().splitlines():
            if line.startswith("OPENROUTER_KEY="):
                return line.split("=", 1)[1].strip().strip('"')
    sys.exit("OPENROUTER_KEY is not set (environment or .env)")


def generate(prompt: str, key: str) -> bytes:
    body = {
        "model": MODEL,
        "messages": [{"role": "user", "content": f"{STYLE} {prompt}"}],
        "modalities": ["text", "audio"],
        "audio": {"format": "mp3"},
        "stream": True,
    }
    request = urllib.request.Request(
        "https://openrouter.ai/api/v1/chat/completions",
        data=json.dumps(body).encode(),
        headers={"Authorization": f"Bearer {key}", "Content-Type": "application/json"},
    )
    chunks = []
    with urllib.request.urlopen(request, timeout=600) as response:
        for raw in response:
            line = raw.decode().strip()
            if not line.startswith("data:"):
                continue
            data = line[5:].strip()
            if data == "[DONE]":
                break
            event = json.loads(data)
            if "error" in event:
                raise RuntimeError(event["error"])
            for choice in event.get("choices", []):
                audio = choice.get("delta", {}).get("audio") or {}
                if audio.get("data"):
                    chunks.append(base64.b64decode(audio["data"]))
    if not chunks:
        raise RuntimeError("response contained no audio")
    return b"".join(chunks)


def main() -> None:
    key = api_key()
    OUT.mkdir(parents=True, exist_ok=True)
    for name, prompt in TRACKS.items():
        target = OUT / f"{name}.mp3"
        if target.exists():
            print(f"{name}: exists, skipping")
            continue
        for attempt in range(1, 6):
            try:
                print(f"{name}: generating (attempt {attempt})", flush=True)
                audio = generate(prompt, key)
                break
            except (urllib.error.URLError, RuntimeError, TimeoutError) as error:
                print(f"{name}: failed: {error}", flush=True)
                time.sleep(10 * attempt)
        else:
            print(f"{name}: giving up")
            continue
        raw = target.with_suffix(".raw.mp3")
        raw.write_bytes(audio)
        # Re-encode at a modest bitrate to keep the repository small; fades hide any hard
        # start or stop.
        subprocess.run(
            ["ffmpeg", "-loglevel", "error", "-y", "-i", str(raw), "-af", "afade=t=in:d=2,areverse,afade=t=in:d=4,areverse",
             "-codec:a", "libmp3lame", "-b:a", "128k", str(target)],
            check=True,
        )
        raw.unlink()
        print(f"{name}: wrote {target.relative_to(ROOT)} ({target.stat().st_size // 1024} KiB)")


if __name__ == "__main__":
    main()
