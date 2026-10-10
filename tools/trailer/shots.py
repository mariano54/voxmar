"""The trailer's shots: where the camera is, how it moves and what the game shows.

Every shot plays seed 2026, whose sights `scout.py` found within a few kilometres of spawn.
`--pose dx,dy,dz,yaw,pitch` puts the camera relative to the player's eye (yaw 0 looks north,
-z; 90 west), `--dolly` moves it on each second, and `--time` holds the time of day (0 is
sunrise, 0.25 noon, 0.5 sunset). Cinematic shots hide the HUD and empty the hand.
"""

import math

SEED = 2026
COMMON = ["--seed", SEED, "--view-distance", 24]
CINEMATIC = ["--cinematic"]


def fly(yaw, speed, climb=0.0, turn=0.0, tilt=0.0):
    """A dolly moving `speed` blocks a second the way `yaw` looks, rising `climb`, turning
    `turn` degrees and tilting `tilt` a second."""
    r = math.radians(yaw)
    return ["--dolly", f"{-math.sin(r) * speed:.3f},{climb},{-math.cos(r) * speed:.3f},{turn},{tilt}"]


def pose(dx, dy, dz, yaw, pitch):
    return ["--pose", f"{dx},{dy},{dz},{yaw},{pitch}"]


def shot(seconds, *args, commands=(), clip_commands=(), cinematic=True):
    return {
        "seconds": seconds,
        "clip_commands": list(clip_commands),
        "args": COMMON + (CINEMATIC if cinematic else []) + [a for part in args for a in part],
        "commands": (["/clear"] if cinematic else []) + list(commands),
    }


def factory():
    """A factory yard north of the player (relative to them, `~`): a stone-brick floor, a
    conveyor line feeding a row of LV machines, boilers and steam pipes into a turbine hall,
    a processing array, a blast furnace blown by bellows, battery banks and windmills on
    towers."""
    c = [
        "/fill ~-22 ~-1 ~-32 ~22 ~-1 ~-2 stone_bricks",
        "/fill ~-22 ~-4 ~-32 ~22 ~-2 ~-2 stone",
        "/fill ~-22 ~ ~-32 ~22 ~14 ~-2 air",
        # The conveyor line and the machines it feeds, cabled behind.
        "/fill ~-16 ~ ~-7 ~16 ~ ~-7 conveyor",
        "/setblock ~-17 ~ ~-7 hopper",
        "/setblock ~17 ~ ~-7 chest",
        "/fill ~-16 ~ ~-9 ~16 ~ ~-9 lv_cable",
    ]
    machines = ["macerator", "electric_furnace", "ore_washer", "steam_crusher", "induction_smelter", "sawmill", "trip_hammer"]
    for i, x in enumerate(range(-15, 17, 2)):
        c.append(f"/setblock ~{x} ~ ~-8 {machines[i % len(machines)]}")
    c += [
        # Boilers piping steam into the turbine hall (HV casing five long, hollow, its core in front).
        "/fill ~-3 ~ ~-17 ~1 ~ ~-17 boiler",
        "/fill ~-3 ~ ~-16 ~1 ~ ~-16 boiler",
        "/fill ~2 ~ ~-17 ~3 ~ ~-17 steam_pipe",
        "/fill ~4 ~ ~-19 ~8 ~2 ~-15 hv_casing",
        "/fill ~5 ~1 ~-18 ~7 ~1 ~-16 air",
        "/setblock ~6 ~1 ~-15 turbine_hall",
        "/fill ~10 ~ ~-17 ~16 ~ ~-17 hv_cable",
        "/fill ~10 ~ ~-20 ~16 ~1 ~-19 hv_battery_bank",
        # The processing array: a hollow cube of HV casing, the array in its face.
        "/fill ~-10 ~ ~-19 ~-8 ~2 ~-17 hv_casing",
        "/setblock ~-9 ~1 ~-18 air",
        "/setblock ~-9 ~1 ~-17 processing_array",
        # The blast furnace: a kiln-brick shaft four high, bellows in its ring.
        "/fill ~-17 ~ ~-19 ~-15 ~3 ~-17 kiln_bricks",
        "/fill ~-16 ~1 ~-18 ~-16 ~3 ~-18 air",
        "/setblock ~-16 ~ ~-18 blast_furnace",
        "/setblock ~-16 ~ ~-17 bellows",
        "/setblock ~-17 ~ ~-18 bellows",
        "/fill ~-14 ~ ~-18 ~-12 ~ ~-18 shaft",
        "/setblock ~-11 ~ ~-18 beam_engine",
        # Windmills on stone towers at the back, four apart so none is becalmed.
    ]
    for x in range(-18, 20, 6):
        c += [f"/fill ~{x} ~ ~-27 ~{x} ~7 ~-27 stone_bricks", f"/setblock ~{x} ~8 ~-27 windmill", f"/fill ~{x} ~ ~-26 ~{x} ~ ~-23 shaft"]
    c += [
        "/fill ~-14 ~ ~-24 ~-4 ~ ~-24 mv_cable",
        "/fill ~-4 ~ ~-25 ~-2 ~1 ~-25 mv_battery_bank",
        "/setblock ~0 ~ ~-24 mv_transformer",
        "/setblock ~1 ~ ~-24 lv_transformer",
    ]
    return c


FACTORY_AT = "/tp 380 -560"


def clearing(x, z):
    """Stand at x z and cut the trees and hills away around and north of the player."""
    return [f"/tp {x} {z}", "/fill ~-14 ~ ~-16 ~14 ~12 ~8 air"]


MEADOW = clearing(-1168, 384)
MOONLIT_CLEARING = clearing(-208, -1040)
SKILLED = ["/skill mining 34", "/skill farming 41", "/skill hunter 27", "/skill attack 22", "/skill cooking 18", "/skill engineering 30", "/skill fishing 15", "/skill crafting 12"]
# Menus and the HUD drawn at twice the size, as on a Retina screen, so they read in a video.
UI = ["--scale-factor", 2, "--no-console"]
# Close on animals: a longer lens, low behind the player.
CLOSE = ["--fov", 50]

SHOTS = {
    # Opening: over the snowy peaks at sunrise.
    "peaks": shot(4.0, ["--time", 0.03], pose(0, 45, 0, 30, -14), fly(30, 9, climb=1.5),
                  commands=["/tp -1792 -1456"]),
    # Biomes.
    # Low over the sea toward the sunrise, the player on a pillar raised to just above it.
    "ocean": shot(2.5, ["--time", 0.03, "--weather", "cumulus"], pose(0, 2.5, 0, -90, -3), fly(-90, 7),
                  commands=["/tp -336 624", "/fill ~ ~ ~ ~ 129 ~ stone", "/tp -336 130 624"]),
    "gorge": shot(2.5, ["--time", 0.2], pose(0, 26, 0, 0, -62), fly(0, 0, climb=5, turn=10),
                  commands=["/tp -389 139 -369"]),
    "cherry": shot(2.5, ["--time", 0.1, "--weather", "rainbow"], pose(0, 14, 0, 120, -12), fly(120, 5, turn=4),
                   commands=["/tp -1168 384"]),
    "glacier": shot(2.5, ["--time", 0.06], pose(0, 30, 0, 250, -12), fly(250, 8),
                    commands=["/tp 2672 -1120"]),
    "moonlit": shot(3.5, ["--time", 0.78, "--weather", "aurora"], pose(0, 10, 0, 60, -8), fly(60, 4, turn=-5),
                    commands=["/tp -208 -1040"]),
    "silverwood": shot(2.5, ["--time", 0.15], pose(0, 12, 0, 160, -10), fly(160, 4),
                       commands=["/tp -2176 -2160"]),
    "village": shot(2.5, ["--time", 0.1], pose(0, 22, 0, 45, -20), fly(45, 5, turn=-4),
                    commands=["/tp 560 133 -2256"]),
    # Creatures: summoned in a clearing cut into the cherry grove, seen from close by.
    "deer": shot(2.5, CLOSE, ["--time", 0.08], pose(-0.5, -0.3, 1.5, 0, -6), fly(-90, 0.6),
                 commands=MEADOW, clip_commands=["/summon red_deer 4 ~ ~ ~-4", "/creatures tame red_deer"]),
    "wolves": shot(2.5, CLOSE, ["--time", 0.55], pose(-0.5, -0.3, 1.5, 0, -6), fly(-90, 0.6),
                   commands=MEADOW, clip_commands=["/summon wolf 3 ~ ~ ~-4", "/creatures tame wolf"]),
    "bear": shot(2.5, CLOSE, ["--time", 0.15], pose(0.5, -0.3, 1.5, 6, -6),
                 commands=MEADOW, clip_commands=["/summon brown_bear 1 ~ ~ ~-4", "/summon fox 2 ~2 ~ ~-5"]),
    "magic": shot(2.5, CLOSE, ["--time", 0.69, "--weather", "aurora"], pose(-0.5, -0.6, 1.5, 0, 4), fly(-90, 0.5),
                  commands=MOONLIT_CLEARING, clip_commands=["/summon moonhare 3 ~ ~ ~-4", "/summon wisp 3 ~ ~1 ~-5", "/summon glimmer_moth 5 ~ ~1 ~-3"]),
    # Skills: the card plays over a village's fields before the menus.
    "skills_open": shot(2.5, ["--time", 0.12], pose(0, 9, 0, 200, -18), fly(200, 3, turn=-4),
                        commands=["/tp 560 133 -2256"]),
    "character": shot(2.0, UI, ["--character", "--time", 0.1],
                      commands=MEADOW + SKILLED, cinematic=False),
    "guide": shot(2.0, UI, ["--character", "--guide", "farming", "--time", 0.1],
                  commands=MEADOW + SKILLED, cinematic=False),
    # Combat: an iron sword against a bear, clicked in rhythm.
    "combat": shot(3.0, UI, ["--time", 0.12, "--hold", "iron_sword", "--press", "left", "--repeat", "--face-creature"],
                   commands=MEADOW + ["/give iron_sword 1", "/skill attack 40"],
                   clip_commands=["/summon brown_bear 1 ~ ~ ~-2"], cinematic=False),
    "combat_night": shot(3.0, UI, ["--time", 0.63, "--hold", "iron_sword", "--press", "left", "--repeat", "--face-creature"],
                         commands=MEADOW + ["/give iron_sword 1", "/skill attack 40"],
                         clip_commands=["/summon shambler 2 ~ ~ ~-2"], cinematic=False),
    # Quests, inventory and crafting: the card plays over a hamlet before the menus.
    "quests_open": shot(2.5, ["--time", 0.45], pose(0, 26, 0, 30, -24), fly(30, 4, turn=5),
                        commands=["/tp 336 146 -496"]),
    "quests": shot(2.0, UI, ["--quests", "first_bronze", "--time", 0.1], commands=MEADOW, cinematic=False),
    "inventory": shot(2.0, UI, ["--inventory", "--hover-slot", 0, "--time", 0.1], commands=MEADOW, cinematic=False),
    "factory_wide": shot(3.0, ["--time", 0.18], pose(-20, 12, 4, -46, -22), fly(-90, 3, turn=3),
                         commands=[FACTORY_AT] + factory()),
    "factory_line": shot(3.0, ["--time", 0.18], pose(-18, 0.8, -5, -100, -10), fly(-90, 4),
                         commands=[FACTORY_AT] + factory()),
    "factory_top": shot(3.0, ["--time", 0.22], pose(0, 34, 12, 0, -48), fly(0, 3, climb=-1),
                        commands=[FACTORY_AT] + factory()),
    "crafting": shot(2.0, UI, ["--inventory", "--grid", "iron_ingot,iron_ingot,iron_ingot,,stick,,,stick,", "--time", 0.1],
                     commands=MEADOW + ["/give iron_ingot 3", "/give stick 2", "/skill engineering 30"], cinematic=False),
}
