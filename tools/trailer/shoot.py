"""Records the trailer's shots with the game's headless capture and encodes each as a clip.

    python3 tools/trailer/shoot.py BINARY OUT_DIR [SHOT ...]

Each shot in shots.py is a `--capture … --clip SECONDS` run: the game plays an unsaved
world, settles, then saves every frame of that much game time at 30 fps (`--dolly` moving
the camera; `clip_commands` run as recording starts, so summoned creatures are
still where they were put). The frames are encoded to OUT_DIR/clips/NAME.mp4 and deleted. Shots run a few
at a time; name some to run only those.
"""

import shutil
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from shots import SHOTS  # noqa: E402

PARALLEL = 3


def shoot(binary, out, name, shot):
    frames = out / "frames" / name
    shutil.rmtree(frames, ignore_errors=True)
    frames.mkdir(parents=True)
    args = [str(binary), "--capture", str(frames / "f.png"), "--clip", str(shot["seconds"]), "--no-music"]
    args += [str(a) for a in shot["args"]]
    for line in shot.get("commands", []):
        args += ["--command", line]
    for line in shot.get("clip_commands", []):
        args += ["--clip-command", line]
    log = out / "logs" / f"{name}.log"
    log.parent.mkdir(parents=True, exist_ok=True)
    with log.open("w") as f:
        f.write(" ".join(args) + "\n\n")
        f.flush()
        code = subprocess.run(args, stdout=f, stderr=subprocess.STDOUT, timeout=900).returncode
    pngs = sorted(frames.glob("f-*.png"))
    if code != 0 or not pngs:
        return f"{name}: FAILED (exit {code}, {len(pngs)} frames), see {log}"
    clip = out / "clips" / f"{name}.mp4"
    clip.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        ["ffmpeg", "-v", "error", "-y", "-framerate", "30", "-i", str(frames / "f-%03d.png"),
         "-c:v", "libx264", "-preset", "slow", "-crf", "14", "-pix_fmt", "yuv420p", str(clip)],
        check=True,
    )
    shutil.copy(pngs[len(pngs) // 2], out / "clips" / f"{name}.png")
    shutil.rmtree(frames)
    return f"{name}: {len(pngs)} frames -> {clip}"


def main():
    binary, out = Path(sys.argv[1]), Path(sys.argv[2])
    wanted = sys.argv[3:] or list(SHOTS)
    with ThreadPoolExecutor(PARALLEL) as pool:
        for line in pool.map(lambda n: shoot(binary, out, n, SHOTS[n]), wanted):
            print(line, flush=True)


if __name__ == "__main__":
    main()
