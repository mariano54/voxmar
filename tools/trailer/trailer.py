#!/usr/bin/env python3
"""Records and cuts voxmar's trailer.

Every shot is a `--clip` capture: the game runs frame by frame at 30 frames a second of game
time while the camera glides from the shot's `pose` to its `pose_to`, so the footage is the
same however fast the machine draws. Title cards are drawn in the game's own pixel font
(assets/ui/fonts/voxmar.txt), with their numbers counted from the content, and the music is
one of voxmar's own tracks. What is shot, for how long, what the cards say and which music
plays are all in trailer.json beside this script.

    python3 tools/trailer/trailer.py                    # capture, cards, cut
    python3 tools/trailer/trailer.py capture --only village,combat
    python3 tools/trailer/trailer.py cut --language ja
    python3 tools/trailer/trailer.py --preview          # small and quick, to check the shots
    python3 tools/trailer/trailer.py check              # only check trailer.json

Steps: `capture` records each shot's frames and encodes them to clips/NAME.mp4 (shots whose
clip exists are skipped; `--again` records them anew), `cards` draws the title cards and `cut`
puts clips, cards and music together. Writes to ~/Projects/voxmar-wt/trailer unless --out says
otherwise, never into the repository: the 2160p master voxmar-trailer-LANG-2160p.mp4 and a
1080p copy beside it. Needs ffmpeg, and a build of the game with `--pose-to`
(target/release/voxmar unless --game).
"""

import argparse
import json
import math
import os
import re
import shutil
import struct
import subprocess
import sys
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
CONFIG = Path(__file__).resolve().parent / "trailer.json"
FONT = ROOT / "assets" / "ui" / "fonts" / "voxmar.txt"
STEPS = ("capture", "cards", "cut")
LANGUAGES = ("en", "ja")
# Frames per second of a `--clip` (`CLIP_RATE` in crates/client/src/capture.rs).
RATE = 30
# Card numbers, each counted as the entries of a list in a content file.
COUNTED = {
    "biomes": ("assets/content/base/biomes.ron", "biomes"),
    "cave_biomes": ("assets/content/base/cave_biomes.ron", "biomes"),
    "skills": ("assets/content/core/skills.ron", "skills"),
    "creatures": ("assets/content/base/creatures.ron", "creatures"),
}


def fail(message):
    sys.exit(f"trailer: {message}")


def count_entries(path, key):
    """The entries of the list `key: [` in a RON file: its `name:` fields at the first one's
    indentation."""
    text = (ROOT / path).read_text(encoding="utf-8")
    start = text.find(f"{key}: [")
    first = re.compile(r'^( +)name: "', re.M).search(text, start) if start >= 0 else None
    if not first:
        fail(f"no `{key}` list with names in {path}")
    return len(re.findall(rf'^{first.group(1)}name: "', text[start:], re.M))


def counts():
    return {name: count_entries(path, key) for name, (path, key) in COUNTED.items()}


def load(numbers):
    """trailer.json, checked: every mistake is reported before anything runs."""
    config = json.loads(CONFIG.read_text(encoding="utf-8"))
    problems = []
    shots, cards, timeline = config.get("shots", {}), config.get("cards", {}), config.get("timeline", [])
    if not timeline:
        problems.append("the timeline is empty")
    for i, item in enumerate(timeline):
        kinds = [k for k in ("shot", "card") if k in item]
        if len(kinds) != 1:
            problems.append(f"timeline item {i} needs one of `shot` or `card`")
            continue
        name = item[kinds[0]]
        if name not in (shots if kinds[0] == "shot" else cards):
            problems.append(f"timeline item {i}: no {kinds[0]} called `{name}`")
        if not (isinstance(item.get("beats"), (int, float)) and item["beats"] > 0):
            problems.append(f"timeline item {i} ({name}) needs `beats` above 0")
    for name, shot in shots.items():
        for key in ("pose", "pose_to"):
            pose = shot.get(key)
            if not (isinstance(pose, list) and len(pose) == 5 and all(isinstance(v, (int, float)) for v in pose)):
                problems.append(f"shot `{name}`: `{key}` needs five numbers (dx, dy, dz, yaw, pitch)")
        if not isinstance(shot.get("seed"), int):
            problems.append(f"shot `{name}` needs a whole-number `seed`")
        if not (isinstance(shot.get("time"), (int, float)) and 0 <= shot["time"] < 1):
            problems.append(f"shot `{name}` needs a `time` of day from 0 to 1")
    for name, card in cards.items():
        for language in LANGUAGES:
            lines = card.get(language)
            if not (isinstance(lines, list) and 1 <= len(lines) <= 2 and all(isinstance(l, str) and l for l in lines)):
                problems.append(f"card `{name}` needs `{language}`: one or two lines")
                continue
            for line in lines:
                for field in re.findall(r"\{(\w+)\}", line):
                    if field not in numbers:
                        problems.append(f"card `{name}` ({language}) names `{{{field}}}`; counted are {', '.join(numbers)}")
    music = config.get("music", {})
    if not (ROOT / music.get("track", "")).is_file():
        problems.append(f"music track `{music.get('track')}` is not a file in the repository")
    if not (isinstance(config.get("beat"), (int, float)) and config["beat"] > 0):
        problems.append("`beat` (seconds) must be above 0")
    if problems:
        fail("trailer.json:\n  " + "\n  ".join(problems))
    return config


def seconds(config, item):
    return item["beats"] * config["beat"]


def write_settings(config, directory):
    """The game's settings for the captures, in their own configuration directory (never the
    player's): `settings` from trailer.json, and always voxmar's own textures, never anything
    from a Minecraft install (a settings file that leaves `minecraft_assets` out turns it on
    where a Minecraft jar is found)."""
    settings = {k: v for k, v in config.get("settings", {}).items() if k != "about"}
    settings.update(minecraft_assets=False, textures="Voxmar")
    lines = []
    for key, value in settings.items():
        if isinstance(value, bool):
            value = "true" if value else "false"
        elif isinstance(value, str):
            if not re.fullmatch(r"[A-Z][A-Za-z0-9]*", value):
                fail(f"setting `{key}`: `{value}` is not a setting's value")
        lines.append(f"    {key}: {value},")
    directory.mkdir(parents=True, exist_ok=True)
    (directory / "settings.ron").write_text("(\n" + "\n".join(lines) + "\n)\n", encoding="utf-8")


def run(command, **kwargs):
    print("+", " ".join(str(c) for c in command), flush=True)
    result = subprocess.run([str(c) for c in command], **kwargs)
    if result.returncode != 0:
        fail(f"{Path(str(command[0])).name} failed with exit code {result.returncode}")


# --- Capture --------------------------------------------------------------------------------


def capture(config, args, out, size, scale_factor):
    game = Path(args.game).expanduser().resolve()
    if not game.is_file():
        fail(f"no game build at {game} (build it, or pass --game)")
    # A shot used twice is recorded once, for its longest use.
    wanted = {}
    for item in config["timeline"]:
        if "shot" in item:
            wanted[item["shot"]] = max(wanted.get(item["shot"], 0), seconds(config, item))
    only = set(args.only.split(",")) if args.only else None
    if only and not only <= wanted.keys():
        fail(f"--only names shots the timeline does not use: {', '.join(sorted(only - wanted.keys()))}")
    (out / "clips").mkdir(parents=True, exist_ok=True)
    write_settings(config, out / "config")
    env = dict(os.environ, VOXMAR_CONFIG=str(out / "config"), VOXMAR_MUTE="1")
    for name, length in wanted.items():
        if only and name not in only:
            continue
        clip = out / "clips" / f"{name}.mp4"
        if clip.exists() and not args.again:
            print(f"{name}: {clip} is there already (--again records it anew)")
            continue
        shot = config["shots"][name]
        frames_dir = out / "frames" / name
        shutil.rmtree(frames_dir, ignore_errors=True)
        frames_dir.mkdir(parents=True)
        # A little more than the cut uses, so the last frame is never short.
        length += 2 / RATE
        command = [
            game,
            "--seed", shot["seed"],
            "--capture", frames_dir / "f.png",
            "--clip", f"{length:.3f}",
            "--size", f"{size[0]}x{size[1]}",
            "--scale-factor", scale_factor,
            "--fov", shot.get("fov", config.get("fov", 70)),
            "--time", shot["time"],
            "--pose", ",".join(str(v) for v in shot["pose"]),
            "--pose-to", ",".join(str(v) for v in shot["pose_to"]),
            "--no-music",
        ]
        command.append("--no-hud")
        if "view_distance" in shot:
            command += ["--view-distance", shot["view_distance"]]
        if "weather" in shot:
            command += ["--weather", shot["weather"]]
        if "wait" in shot:
            command += ["--wait", shot["wait"]]
        for line in shot.get("commands", []):
            command += ["--command", line]
        for line in shot.get("clip_commands", []):
            command += ["--clip-command", line]
        command += shot.get("args", [])
        run(command, cwd=ROOT, env=env)
        frames = sorted(frames_dir.glob("f-*.png"))
        expected = math.ceil(length * RATE - 1e-6)
        if len(frames) < expected:
            fail(f"{name}: {len(frames)} frames recorded, {expected} expected")
        run([
            "ffmpeg", "-v", "error", "-y", "-framerate", RATE, "-i", frames_dir / "f-%03d.png",
            "-c:v", "libx264", "-preset", args.preset, "-crf", "8", "-pix_fmt", "yuv420p", clip,
        ])
        if not args.keep_frames:
            shutil.rmtree(frames_dir)
        print(f"{name}: {clip}")


# --- Cards ----------------------------------------------------------------------------------


def load_font():
    """The game's pixel font: each glyph's rows of `#` (ink) and `.`, the first row at the top
    of a capital letter. Accented letters (composed in the file) are left out."""
    glyphs, rows, current = {}, [], None
    for line in FONT.read_text(encoding="utf-8").splitlines():
        line = line.rstrip()
        if line.startswith("= "):
            if current is not None and rows:
                glyphs[current] = rows
            rows = []
            words = line[2:].split()
            current = " " if words[0] == "space" else words[0]
            if len(words) > 1:
                glyphs[current] = ["." * int(words[1])]
                current = None
        elif line and set(line) <= {"#", "."} and current is not None:
            rows.append(line)
        elif line.startswith(("mark ", "compose ")):
            if current is not None and rows:
                glyphs[current] = rows
            current, rows = None, []
    if current is not None and rows:
        glyphs[current] = rows
    return glyphs


def ink(glyphs, text):
    """The text's ink as (x, y) font pixels, one pixel of space after each glyph, and its width."""
    pixels, x = [], 0
    for c in text:
        glyph = glyphs.get(c)
        if glyph is None:
            fail(f"the game's font has no `{c}` (in “{text}”)")
        for y, row in enumerate(glyph):
            pixels += [(x + i, y) for i, cell in enumerate(row) if cell == "#"]
        x += len(glyph[0]) + 1
    return pixels, max(x - 1, 0)


def colour(hex_colour):
    return bytes.fromhex(hex_colour.lstrip("#"))


def write_png(path, width, height, rgb):
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    stride = width * 3
    raw = b"".join(b"\x00" + bytes(rgb[y * stride:(y + 1) * stride]) for y in range(height))
    path.write_bytes(
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 6))
        + chunk(b"IEND", b"")
    )


def draw_card(lines, size, style, glyphs, path):
    """A card in the game's font, each line centred, its font pixels `style["pixels"]` screen
    pixels across at 3840 wide (the second line's `style["sub_pixels"]`), with the game's
    drop shadow one font pixel down and right."""
    width, height = size
    scale = width / 3840
    sizes = [max(1, round(style["pixels"] * scale)), max(1, round(style["sub_pixels"] * scale))]
    background, text, shadow = colour(style["background"]), colour(style["text"]), colour(style["shadow"])
    rgb = bytearray(background * (width * height))
    laid = []
    for i, line in enumerate(lines):
        px = sizes[min(i, 1)]
        pixels, line_width = ink(glyphs, line)
        laid.append((pixels, line_width, px))
    gap = sizes[0] * style["line_gap"]
    total = sum(9 * px for _, _, px in laid) + gap * (len(laid) - 1)
    top = (height - total) // 2
    for pixels, line_width, px in laid:
        left = (width - line_width * px) // 2
        for offset, fill in ((px, shadow), (0, text)):
            for x, y in pixels:
                x0, y0 = left + x * px + offset, top + y * px + offset
                span = fill * px
                for row in range(y0, y0 + px):
                    if 0 <= row < height and 0 <= x0 and x0 + px <= width:
                        start = (row * width + x0) * 3
                        rgb[start:start + px * 3] = span
        top += 9 * px + gap
    write_png(path, width, height, rgb)


def draw_card_ja(lines, size, style, path):
    """Japanese cards, which the game's font does not draw, in the Fusion Pixel font the game
    falls back to (assets/ui/fonts/fallback.ron), through ffmpeg's drawtext."""
    width, height = size
    scale = width / 3840
    font = ROOT / style["ja_font"]
    sizes = [12 * max(1, round(style["ja_pixels"] * scale)), 12 * max(1, round(style["ja_sub_pixels"] * scale))]
    gap = sizes[0] // 3
    total = sum(sizes[min(i, 1)] for i in range(len(lines))) + gap * (len(lines) - 1)
    top = (height - total) // 2
    filters = []
    for i, line in enumerate(lines):
        textfile = path.with_suffix(f".{i}.txt")
        textfile.write_text(line, encoding="utf-8")
        fontsize = sizes[min(i, 1)]
        shadow = max(1, fontsize // 12)
        filters.append(
            f"drawtext=fontfile='{font}':textfile='{textfile}':fontsize={fontsize}:fontcolor=0x{style['text'].lstrip('#')}"
            f":shadowcolor=0x{style['shadow'].lstrip('#')}:shadowx={shadow}:shadowy={shadow}:x=(w-text_w)/2:y={top}"
        )
        top += fontsize + gap
    run([
        "ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", f"color=c=0x{style['background'].lstrip('#')}:s={width}x{height}",
        "-frames:v", "1", "-vf", ",".join(filters), path,
    ])
    for textfile in path.parent.glob(f"{path.stem}.*.txt"):
        textfile.unlink()


def cards(config, numbers, language, out, size):
    (out / "cards").mkdir(parents=True, exist_ok=True)
    glyphs = load_font()
    used = {item["card"] for item in config["timeline"] if "card" in item}
    for name in sorted(used):
        lines = [line.format(**numbers) for line in config["cards"][name][language]]
        path = out / "cards" / f"{name}-{language}.png"
        if language == "en":
            draw_card(lines, size, config["card_style"], glyphs, path)
        else:
            draw_card_ja(lines, size, config["card_style"], path)
        print(f"{name}: {' / '.join(lines)} -> {path}")


# --- Cut ------------------------------------------------------------------------------------


def cut(config, args, language, out, size):
    timeline = config["timeline"]
    total = sum(seconds(config, item) for item in timeline)
    style, music = config["card_style"], config["music"]
    inputs, filters, labels = [], [], []
    for i, item in enumerate(timeline):
        length = seconds(config, item)
        if "shot" in item:
            clip = out / "clips" / f"{item['shot']}.mp4"
            if not clip.exists():
                fail(f"no clip for shot `{item['shot']}` at {clip} (run the capture step)")
            inputs += ["-i", clip]
            fades = ""
        else:
            card = out / "cards" / f"{item['card']}-{language}.png"
            if not card.exists():
                fail(f"no card `{item['card']}` at {card} (run the cards step)")
            inputs += ["-loop", "1", "-framerate", RATE, "-t", f"{length:.3f}", "-i", card]
            fade = min(style["fade"], length / 3)
            fades = f",fade=t=in:st=0:d={fade:.3f},fade=t=out:st={length - fade:.3f}:d={fade:.3f}"
        filters.append(
            f"[{i}:v]trim=duration={length:.4f},setpts=PTS-STARTPTS,fps={RATE},"
            f"scale={size[0]}:{size[1]}:flags=lanczos,setsar=1,format=yuv420p{fades}[v{i}]"
        )
        labels.append(f"[v{i}]")
    filters.append(
        "".join(labels) + f"concat=n={len(labels)}:v=1:a=0,"
        f"fade=t=in:st=0:d={config['fade_in']},fade=t=out:st={total - config['fade_out']:.3f}:d={config['fade_out']}[video]"
    )
    music_input = len(timeline)
    inputs += ["-ss", music["start"], "-t", f"{total:.3f}", "-i", ROOT / music["track"]]
    filters.append(
        f"[{music_input}:a]afade=t=in:st=0:d={music['fade_in']},"
        f"afade=t=out:st={total - music['fade_out']:.3f}:d={music['fade_out']},atrim=0:{total:.3f}[audio]"
    )
    suffix = "-preview" if args.preview else ""
    master = out / f"voxmar-trailer-{language}-{size[1]}p{suffix}.mp4"
    run([
        "ffmpeg", "-v", "error", "-y", *inputs, "-filter_complex", ";".join(filters),
        "-map", "[video]", "-map", "[audio]", "-r", RATE,
        "-c:v", "libx264", "-preset", args.preset, "-crf", "14", "-pix_fmt", "yuv420p",
        "-c:a", "aac", "-b:a", "320k", "-movflags", "+faststart", "-t", f"{total:.3f}", master,
    ])
    print(f"cut: {total:.2f} s -> {master}")
    if size[1] > 1080:
        copy = out / f"voxmar-trailer-{language}-1080p{suffix}.mp4"
        run([
            "ffmpeg", "-v", "error", "-y", "-i", master, "-vf", "scale=1920:1080:flags=lanczos",
            "-c:v", "libx264", "-preset", args.preset, "-crf", "18", "-pix_fmt", "yuv420p",
            "-c:a", "copy", "-movflags", "+faststart", copy,
        ])
        print(f"cut: 1080p copy -> {copy}")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("steps", nargs="*", help=f"any of {', '.join(STEPS)}, or check (all three unless given)")
    parser.add_argument("--out", default="~/Projects/voxmar-wt/trailer", help="where everything is written")
    parser.add_argument("--game", default=str(ROOT / "target" / "release" / "voxmar"), help="the game build to capture with")
    parser.add_argument("--only", help="capture only these shots (comma separated)")
    parser.add_argument("--again", action="store_true", help="record shots whose clips exist already")
    parser.add_argument("--keep-frames", action="store_true", help="keep each shot's PNG frames after encoding")
    parser.add_argument("--language", choices=LANGUAGES, default="en", help="the cards' language")
    parser.add_argument("--preview", action="store_true", help="960x540, into OUT/preview, to check shots quickly")
    args = parser.parse_args()
    steps = args.steps or list(STEPS)
    for step in steps:
        if step not in STEPS + ("check",):
            fail(f"no step `{step}`: the steps are {', '.join(STEPS)} and check")
    numbers = counts()
    config = load(numbers)
    print("counted: " + ", ".join(f"{n} {name}" for name, n in numbers.items()))
    if "check" in steps:
        total = sum(seconds(config, item) for item in config["timeline"])
        print(f"trailer.json is sound: {len(config['timeline'])} pieces, {total:.2f} s")
        return
    out = Path(args.out).expanduser().resolve()
    if out == ROOT or ROOT in out.parents:
        fail(f"{out} is inside the repository; captures and the cut never go there")
    size, scale_factor = tuple(config["size"]), config["scale_factor"]
    args.preset = "slow"
    if args.preview:
        out, size, scale_factor, args.preset = out / "preview", (960, 540), scale_factor * 960 / config["size"][0], "veryfast"
    if shutil.which("ffmpeg") is None:
        fail("ffmpeg is not installed")
    if "capture" in steps:
        capture(config, args, out, size, scale_factor)
    if "cards" in steps:
        cards(config, numbers, args.language, out, size)
    if "cut" in steps:
        cut(config, args, args.language, out, size)


if __name__ == "__main__":
    main()
