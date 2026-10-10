"""A contact sheet of the shots' middle frames, to review them at a glance.

    python3 tools/trailer/sheet.py OUT_DIR [NAME ...]

Writes OUT_DIR/sheet.png: the frames two across at 640 wide, in the order named.
"""

import subprocess
import sys
from pathlib import Path


def main():
    out = Path(sys.argv[1])
    names = sys.argv[2:] or sorted(p.stem for p in (out / "clips").glob("*.png"))
    inputs, filters = [], []
    for i, name in enumerate(names):
        inputs += ["-i", str(out / "clips" / f"{name}.png")]
        filters.append(f"[{i}]scale=640:-1[v{i}]")
    if len(names) % 2:
        filters.append(f"color=black:s=640x360[v{len(names)}]")
        names.append("")
    layout = "|".join(f"{(i % 2) * 640}_{(i // 2) * 360}" for i in range(len(names)))
    filters.append("".join(f"[v{i}]" for i in range(len(names))) + f"xstack=inputs={len(names)}:layout={layout}")
    subprocess.run(["ffmpeg", "-v", "error", "-y", *inputs, "-filter_complex", ";".join(filters), "-frames:v", "1", str(out / "sheet.png")], check=True)
    print(out / "sheet.png")


if __name__ == "__main__":
    main()
