"""Finds where a seed's sights are, for placing the trailer's shots.

    python3 tools/trailer/scout.py BINARY OUT_DIR SEED [SEED ...]

Runs one still capture per seed that asks `/locate` for each biome and settlement in
PLACES, and prints the server's answers (the client logs console lines).
"""

import re
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

PLACES = [
    "biome snowy_peaks", "biome mountains", "biome alpine_meadow", "biome ocean", "biome jungle",
    "biome cherry_grove", "biome crystal_expanse", "biome moonlit_meadow", "biome lavender_fields",
    "biome glacier", "biome redwood_forest", "biome ember_wastes", "biome silverwood_glade",
    "biome sunspire_downs", "biome badlands", "settlement village", "settlement hamlet", "gorge",
]


def scout(binary, out, seed):
    args = [str(binary), "--capture", str(out / f"scout-{seed}.png"), "--seed", str(seed), "--cinematic"]
    for place in PLACES:
        args += ["--command", f"/locate {place}"]
    run = subprocess.run(args, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=600)
    lines = [re.sub(r"\x1b\[[0-9;]*m", "", l).split("console: ", 1)[1] for l in run.stdout.splitlines() if "console: " in l]
    return f"seed {seed}:\n" + "\n".join("  " + l for l in lines if not l.startswith("Usage"))


def main():
    binary, out = Path(sys.argv[1]), Path(sys.argv[2])
    out.mkdir(parents=True, exist_ok=True)
    with ThreadPoolExecutor(3) as pool:
        for text in pool.map(lambda s: scout(binary, out, s), sys.argv[3:]):
            print(text, flush=True)


if __name__ == "__main__":
    main()
