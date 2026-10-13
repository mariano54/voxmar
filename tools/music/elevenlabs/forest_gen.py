#!/usr/bin/env python3
"""Calm, cozy ambient music generator (forest/snow/fantasy/desert biomes) (ElevenLabs Music, model music_v2_5).

Instrument and vibe weights mirror the calm 55-track subset of the Wuthering Waves
3.0-3.3 OST mix (vocal, loud, beat-driven and aggressive tracks removed): piano,
violin and cello lead; harp, guitar, zither, flute, mallets, bells and singing bowl
add color; pads sit underneath. The forest/cozy frame and the "never" list are fixed
so every sample stays calm.

    ELEVENLABS_API_KEY=... python3 forest_gen.py --count 3 --seed 7
    python3 forest_gen.py --count 5 --dry-run     # just print prompts
"""
import argparse
import json
import os
import random
import re
import sys
import urllib.request
from pathlib import Path

# Track counts from the calm subset, used as sampling weights.
LEADS = {"soft felt piano": 17, "warm solo violin": 14}
SUPPORT = {
    "cello": 11, "glockenspiel": 8, "harp": 7, "marimba": 7, "gentle bells and chimes": 7,
    "nylon-string acoustic guitar": 6, "wooden flute": 6, "double bass": 6, "zither": 5,
    "pizzicato strings": 4, "vibraphone": 3, "electric piano (Rhodes)": 8, "clarinet": 1,
    "soft muted saxophone": 2, "soft reed organ": 2, "ukulele": 1,
}
TEXTURES = {"warm ambient synth pads": 12, "singing bowl": 10, "distant soft gong": 2,
            "airy string pad": 6, "soft organ-like sustained pad": 3}
# Leads for tracks that get neither piano nor violin (plus the biome's signature instruments).
ALT_LEADS = {"harp": 7, "wooden flute": 6, "cello": 6, "clarinet": 3, "soft French horn": 2}
PIANO_LIKE = {"soft felt piano", "electric piano (Rhodes)"}
VIOLIN_LIKE = {"warm solo violin", "pizzicato strings"}
# Light WoW "Teldrassil" touch (flute/clarinet/horn/harp colour, mystical night-forest mood),
# applied to ~40% of tracks as a single extra instrument or one swapped vibe.
TOUCH_P = 0.4
TOUCH_INSTR = ["wooden flute", "clarinet", "soft French horn", "harp"]
TOUCH_VIBES = ["gently mystical and moonlit", "quietly reverent", "ancient and dreamlike"]
VIBES = {
    "melancholic but comforting": 20, "dreamy and weightless": 12, "tender, like a lullaby": 8,
    "meditative and still": 6, "intimate chamber-music warmth": 5, "gentle folk storytelling": 6,
    "nostalgic, like an old memory": 6, "hopeful morning light": 3, "late-night quiet": 2,
}
# Scenes are only imagery: the game layers its own ambience, so the music carries none.
FOREST = [
    "sunlight filtering through a mossy forest canopy", "a warm cabin deep in the woods",
    "a misty pine forest at dawn", "fireflies over a forest clearing at dusk",
    "an old mossy path between ancient trees", "autumn leaves drifting in a still forest",
    "a hidden glade in spring", "a lantern-lit forest path at night",
]
# Per-biome flavor on top of the shared tables: scenes plus a few extra weighted
# instruments/vibes (and optional keys). Everything else stays the same.
BIOMES = {
    "forest": {"label": "forest", "scenes": FOREST, "support": {}, "vibes": {}},
    "snow": {
        "label": "snowy",
        "scenes": ["a hushed snowfield under a faint aurora", "snow-laden pines at blue hour",
                   "a frozen lake at dusk", "a lantern-lit village buried in snow",
                   "a quiet mountain hut on a winter night", "frost glittering on a still morning"],
        "support": {"celesta": 8, "music box": 5, "gentle bells and chimes": 6, "soft French horn": 3},
        "vibes": {"crisp and hushed": 8, "lonely but warm, like a fire indoors": 6, "sparkling and frosty": 5},
    },
    "fantasy": {
        "label": "enchanted fantasy",
        "scenes": ["a glowing mushroom grove", "floating islands drifting above the clouds",
                   "a crystal library hidden in old ruins", "a fairy village in the roots of a great tree",
                   "a moonlit tower overlooking a sleeping kingdom", "a lake that reflects the stars"],
        "support": {"harp": 8, "celesta": 7, "music box": 4, "dulcimer": 4},
        "vibes": {"whimsical and enchanted": 8, "storybook wonder": 7, "softly magical": 6},
    },
    "general": {  # no place or time of day: plays anywhere, any time
        "label": "cozy adventure game",
        "scenes": ["a quiet moment between adventures", "a cozy inn where travelers rest",
                   "wandering an open road through gentle hills", "the hero's little home base",
                   "a peaceful village square", "an old study full of maps and books",
                   "tending a small garden", "a calm harbor town"],
        "support": {},
        "vibes": {"timeless and welcoming": 8, "peaceful and unhurried": 6, "homey and familiar": 6},
        "exclude": {"hopeful morning light", "late-night quiet", "gently mystical and moonlit",
                    "like the last hour before sleep", "like watching the light change slowly"},
        "note": "Timeless and peaceful: it should fit any place and any time of day, day or night.",
        "no_percussion": True,
    },
    "desert": {
        "label": "desert",
        "scenes": ["golden dunes at sunset", "a quiet oasis under a sky full of stars",
                   "sandstone ruins in warm evening light", "a caravan camp resting at night",
                   "a sleepy desert town at noon shade", "wind-carved canyons at dawn"],
        "support": {"oud": 8, "qanun": 6, "ney flute": 6, "duduk": 4},
        "vibes": {"warm and sun-bleached": 8, "vast and solitary": 6, "starlit oasis calm": 6},
        "keys": ["D harmonic minor", "A harmonic minor", "E Phrygian", "D Dorian", "G minor"],
    },
}
# Variety knobs (each track draws one of each): opening line, room sound, musical form,
# optional pad, optional soft percussion and an optional creative image. Large hall reverb
# and constant pads are now the exception, not the rule.
OPENERS = {"Calm, cozy instrumental": 4, "Gentle, warm video-game soundtrack piece": 3,
           "Relaxed, intimate instrumental tune": 3, "Soft, cozy ambient instrumental": 1}
SPACES = {"dry, close-miked and intimate, very little reverb": 4, "warm small-room sound, light reverb": 5,
          "natural chamber acoustics, moderate reverb": 3, "spacious hall reverb": 0.5}
FORMS = {
    "a simple, memorable melody that returns as a motif": 6,
    "call-and-response phrases between the lead and a second instrument": 4,
    "the lead starts alone, other instruments join gradually, then it ends alone again": 4,
    "a slow folk-song melody with a clear verse and refrain": 4,
    "a gentle waltz in 3/4 time": 3,
    "sparse phrases with real rests and moments of near silence between them": 4,
    "a repeating music-box-like figure with a slow melody floating on top": 3,
    "a theme followed by a soft variation": 3,
}
PAD_P = 0.6  # otherwise no background pad at all
PERC_P = 0.75  # chance a batch gets one (and only one) track with soft percussion
PERC = {
    "forest": ["very soft hand drum played with fingertips", "light brushes on a snare, barely there", "soft wooden shaker"],
    "snow": ["soft sleigh-bell shimmer", "light brushes on a snare, barely there", "gentle woodblock ticks"],
    "fantasy": ["delicate finger cymbals", "soft frame drum heartbeat", "gentle woodblock ticks"],
    "desert": ["soft frame drum played with fingertips", "gentle tabla, sparse and quiet", "delicate finger cymbals now and then"],
}
HINT_P = 0.5
HINTS = [
    "like a traveler resting and remembering home", "like a lullaby someone half-remembers",
    "like a quiet walk with an old friend", "like the last hour before sleep",
    "like an old storybook being read by lamplight", "like a small celebration that stays quiet",
    "like watching the light change slowly", "like a song passed down for generations",
]
BPM_RANGE = (50, 62)  # 60 felt right, 67 too fast
KEYS = ["C major", "G major", "D major", "F major", "A minor", "E minor", "D minor", "B-flat major"]
NEVER = ("no bass drops, no vocals, no choir, no loud crescendos, no distortion, "
         "no nature sounds, no rain, no water, no birdsong, no wind, no fire crackle, "
         "no field recordings or sound effects - pure instruments only")


def wpick(rng, table, k=1):
    """Weighted sample of k distinct items."""
    items, weights, out = list(table), list(table.values()), []
    for _ in range(min(k, len(items))):
        i = rng.choices(range(len(items)), weights=weights)[0]
        out.append(items.pop(i)); weights.pop(i)
    return out


def plan_batch(rng, n):
    """Per-track (piano, violin) flags: each instrument in at most half the batch."""
    cap = n // 2
    piano, violin = set(rng.sample(range(n), cap)), set(rng.sample(range(n), cap))
    return [(i in piano, i in violin) for i in range(n)]


def scene_plan(rng, biome, n):
    """Distinct scenes for a batch (cycles only if n exceeds the scene list)."""
    sc = BIOMES[biome]["scenes"]
    return [x for _ in range(-(-n // len(sc))) for x in rng.sample(sc, len(sc))][:n]


def make_spec(rng, biome="forest", piano=False, violin=False, scene=None, perc=False):
    b = BIOMES[biome]
    leads = ["soft felt piano"] * piano + ["warm solo violin"] * violin
    if not leads:
        leads = wpick(rng, ALT_LEADS | b["support"])
    banned = set(leads) | (set() if piano else PIANO_LIKE) | (set() if violin else VIOLIN_LIKE)
    pool = {k: v for k, v in (SUPPORT | b["support"]).items() if k not in banned}
    n = rng.randint(2, 3)
    sig = wpick(rng, {k: v for k, v in b["support"].items() if k not in banned})  # one biome signature
    support = sig + wpick(rng, {k: v for k, v in pool.items() if k not in sig}, n - len(sig))
    ex = b.get("exclude", set())
    vibes = wpick(rng, {k: v for k, v in (VIBES | b["vibes"]).items() if k not in ex}, 2)
    touch = None
    if rng.random() < TOUCH_P:
        extra = [x for x in TOUCH_INSTR if x not in leads + support]
        if extra and rng.random() < 0.5:
            touch = rng.choice(extra); support.append(touch)
        else:
            touch = vibes[1] = rng.choice([v for v in TOUCH_VIBES if v not in ex])
    spec = {
        "leads": leads,
        "support": support,
        "texture": wpick(rng, TEXTURES)[0] if rng.random() < PAD_P else None,
        "opener": wpick(rng, OPENERS)[0],
        "space": wpick(rng, SPACES)[0],
        "form": wpick(rng, FORMS)[0],
        "percussion": rng.choice(PERC[biome]) if perc else None,
        "hint": rng.choice([h for h in HINTS if h not in ex]) if rng.random() < HINT_P else None,
        "vibes": vibes,
        "touch": touch,
        "biome": biome,
        "scene": scene or rng.choice(b["scenes"]),
        "bpm": rng.randint(*BPM_RANGE),
        "key": rng.choice(b.get("keys", KEYS)),
        "length_s": rng.randint(150, 180),
    }
    spec["prompt"] = build_prompt(spec)
    return spec


# Boss/fight tracks: intense but musical, biome-flavored, still no vocals/choir/nature sounds.
BOSS = {
    "forest": {"leads": ["driving string section", "bold French horns", "fiery solo violin"],
               "colors": ["wooden flute ostinato", "pizzicato strings", "harp glissandi", "marimba", "low cellos and basses"],
               "drums": ["taiko and frame drums", "deep toms and timpani"],
               "vibes": ["primal and urgent", "ancient and wild", "heroic determination"],
               "scenes": ["an ancient tree guardian awakens", "a corrupted beast stalking the deep woods"]},
    "snow": {"leads": ["staccato string section", "brass section", "fiery solo violin"],
             "colors": ["tubular bells", "celesta shimmer", "low cellos and basses", "glockenspiel", "French horns"],
             "drums": ["timpani and taiko", "deep toms and snare rolls"],
             "vibes": ["icy and relentless", "colossal and unyielding", "a heroic last stand"],
             "scenes": ["a frost giant on a frozen glacier", "an ice wyrm beneath the aurora"]},
    "fantasy": {"leads": ["full orchestral strings", "brass fanfares", "fiery solo violin"],
                "colors": ["harp glissandi", "celesta", "glockenspiel sparkle", "woodwind runs", "low brass"],
                "drums": ["snare and timpani", "orchestral bass drum and cymbals"],
                "vibes": ["grand and magical", "arcane and electric", "heroic and soaring"],
                "scenes": ["a duel with a rogue archmage in a floating citadel", "a dragon circling an enchanted castle"]},
    "desert": {"leads": ["oud riff", "string section in harmonic minor", "brass section"],
               "colors": ["qanun runs", "ney flute", "duduk", "low brass", "pizzicato strings"],
               "drums": ["darbuka and frame drums", "frame drums and deep toms"],
               "vibes": ["scorching and relentless", "sun-scorched fury", "ancient and dangerous"],
               "scenes": ["a giant sand serpent erupting from the dunes", "the guardian of a buried tomb"],
               "keys": ["D harmonic minor", "E Phrygian dominant", "A harmonic minor"]},
}
BOSS_FORMS = [
    "tense intro, relentless driving main theme, a brief breather, then a final heroic surge that loops cleanly",
    "a pulsing ostinato that builds layer by layer into a soaring main theme",
    "call-and-response between the lead and the drums, escalating to a climax",
]
BOSS_NEVER = ("no vocals, no choir, no nature sounds or sound effects, no EDM drops, no dubstep, "
              "no heavy-metal distortion guitars")


def make_boss_spec(rng, biome):
    b = BOSS[biome]
    spec = {
        "mode": "boss", "biome": biome,
        "lead": rng.choice(b["leads"]),
        "colors": rng.sample(b["colors"], 2),
        "drums": rng.choice(b["drums"]),
        "vibes": rng.sample(b["vibes"], 2),
        "scene": rng.choice(b["scenes"]),
        "form": rng.choice(BOSS_FORMS),
        "bpm": rng.randint(128, 150),
        "key": rng.choice(b.get("keys", ["D minor", "C minor", "E minor", "G minor", "F minor"])),
        "length_s": rng.randint(120, 150),
    }
    spec["prompt"] = build_prompt(spec)
    return spec


def build_boss_prompt(s):
    return (
        f"Intense boss battle theme for {article(BIOMES[s['biome']]['label'])} video game: {s['scene']}. "
        f"Mood: {s['vibes'][0]}, {s['vibes'][1]}. "
        f"Lead on {s['lead']}, with {s['colors'][0]} and {s['colors'][1]}; driving rhythm from {s['drums']}. "
        f"Structure: {s['form']}. "
        f"Epic orchestral game-soundtrack sound with a memorable main motif, punchy and clear but musical, not harsh. "
        f"Fast tempo around {s['bpm']} BPM in {s['key']}. Strictly {BOSS_NEVER}."
    )


def article(w):
    return f"{'an' if w[0] in 'aeiou' else 'a'} {w}"


def build_prompt(s):
    if s.get("mode") == "boss":
        return build_boss_prompt(s)
    lead = " and ".join(s["leads"]) if s["leads"] else s["support"][0]
    rest = [x for x in s["support"] if x != lead]
    if "form" not in s:  # specs saved before the variety knobs: original wording
        return (
            f"Calm, cozy ambient instrumental for {article(BIOMES[s.get('biome', 'forest')]['label'])} scene: {s['scene']}. "
            f"Mood: {s['vibes'][0]}, {s['vibes'][1]}. "
            f"Lead melody on {lead}, accompanied by {', '.join(rest)}, over {s['texture']}. "
            f"Lead instrument clear and upfront in the mix. "
            f"Slow, unhurried tempo around {s['bpm']} BPM in {s['key']}, soft dynamics throughout, "
            f"spacious reverb, relaxing video-game-soundtrack feel for studying or sleep. "
            f"Strictly no drums, no beats, {NEVER}."
        )
    bed = (f"over {s['texture']}" if s["texture"] else
           "with no background pad or drone, just the instruments and the space between notes")
    perc = (f"Occasional {s['percussion']}, very sparse, no drum kit and no electronic beats. " if s["percussion"]
            else "No drums or beats. ")
    hint = f", {s['hint']}" if s["hint"] else ""
    return (
        f"{s['opener']} for {article(BIOMES[s['biome']]['label'])} scene: {s['scene']}{hint}. "
        f"Mood: {s['vibes'][0]}, {s['vibes'][1]}. {BIOMES[s['biome']].get('note', '')} "
        f"Lead melody on {lead}, accompanied by {', '.join(rest)}, {bed}. "
        f"Structure: {s['form']}; a clear, singable melody, and rests and silences are welcome. "
        f"Sound: {s['space']}. "
        f"Slow, unhurried tempo around {s['bpm']} BPM in {s['key']}, soft dynamics throughout. "
        f"{perc}Strictly {NEVER}."
    )


def respec(path, rng):
    """Reload a saved spec under the current rules (drop ambience, clamp tempo)."""
    s = json.loads(Path(path).read_text())
    s.pop("ambience", None)
    if not BPM_RANGE[0] <= s["bpm"] <= BPM_RANGE[1]:
        s["bpm"] = rng.randint(*BPM_RANGE)
    s["prompt"] = build_prompt(s)
    return s


def compose(spec, key, model):
    body = json.dumps({"prompt": spec["prompt"], "music_length_ms": spec["length_s"] * 1000,
                       "model_id": model, "force_instrumental": True}).encode()
    req = urllib.request.Request(
        "https://api.elevenlabs.io/v1/music?output_format=mp3_44100_128", data=body,
        headers={"xi-api-key": key, "Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=600) as r:
        return r.read()


def slug(s):
    return re.sub(r"[^a-z0-9]+", "-", s.lower()).strip("-")[:40]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--count", type=int, default=3)
    ap.add_argument("--seed", type=int)
    ap.add_argument("--model", default="music_v2_5")
    ap.add_argument("--out", default=str(Path(__file__).parent / "out"), help="writes to <out>/<biome>/")
    ap.add_argument("--biome", choices=list(BIOMES), default="forest")
    ap.add_argument("--mode", choices=["calm", "boss"], default="calm")
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument("--respec", nargs="+", metavar="SPEC_JSON",
                    help="regenerate saved specs (keeps instruments/vibes/scene) under current rules")
    a = ap.parse_args()
    rng = random.Random(a.seed)
    key = os.environ.get("ELEVENLABS_API_KEY")
    if not a.dry_run and not key:
        sys.exit("set ELEVENLABS_API_KEY (or use --dry-run)")
    out = Path(a.out) / a.biome; out.mkdir(parents=True, exist_ok=True)
    perc = rng.randrange(a.count) if rng.random() < PERC_P and not BIOMES[a.biome].get("no_percussion") else -1
    jobs = ([(Path(p).stem.split("-")[0], respec(p, rng)) for p in a.respec] if a.respec
            else [(f"boss-{n:02d}", make_boss_spec(rng, a.biome)) for n in range(1, a.count + 1)] if a.mode == "boss"
            else [(f"{n:02d}", make_spec(rng, a.biome, pv, vl, sc, n - 1 == perc)) for n, ((pv, vl), sc) in enumerate(
                zip(plan_batch(rng, a.count), scene_plan(rng, a.biome, a.count)), 1)])
    for n, spec in jobs:
        print(f"\n[{n}] {spec['prompt']}")
        if a.dry_run:
            continue
        name = f"{n}-{slug(spec['scene'])}"
        v = 1
        while (out / f"{name}.mp3").exists():
            v += 1
            name = f"{n}-{slug(spec['scene'])}-v{v}"
        (out / f"{name}.mp3").write_bytes(compose(spec, key, a.model))
        (out / f"{name}.json").write_text(json.dumps(spec | {"model": a.model, "seed": a.seed}, indent=2))
        print(f"    -> {out / name}.mp3")


if __name__ == "__main__":
    main()
