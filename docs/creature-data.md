# Creature, food and breeding data, and modding

Oct 5, 2026. The data formats behind creatures, behaviours, loot, food, recipes and genes,
how mods add to them, what validation checks, and the order the code lands in. Part of the
creatures, food and breeding design (see [creatures.md](creatures.md) and
[systems-web.md](systems-web.md)).

## Principles

From the design philosophy, applied here:

- **Everything is a package's data.** `base` ships its creatures in exactly the files and
  format a mod uses; nothing about the first-party wolf is special.
- **Reference by tag, not by name**, wherever a list could grow: diets, recipe inputs,
  biome selectors, tool modifiers. Names only for signature cases.
- **Extend by adding.** A mod adds a creature, a behaviour to an existing creature (by
  tag), a loot entry to an existing table, a recipe that existing ingredients fit, a breed,
  a gene. It never edits another package's file.
- **No engine types in the data**, and no Bevy types in the engine API. The server's
  creature code is plain Rust over engine types; the client draws what it is told.
- **RON now, WASM later.** Behaviour templates, considerations, loot conditions and
  recipe effects are closed engine vocabularies today; WASM adds new ones later through the
  same names.

## Files

| File | Holds | Read by |
| --- | --- | --- |
| `creatures.ron` | species: body, stats, habitat, temperament, senses, diet, behaviours, loot, products, taming, breeding, genes | engine `creature` |
| `behaviours.ron` | named behaviour sets and quirks a species can include by name | engine `creature` |
| `loot.ron` | shared loot tables (`bones`, `rare_seeds`, `diet_seeds`) | engine `loot` |
| `foods.ron` | ingredient properties for items that need more than tags | engine `food` |
| `recipes.ron` | cooking recipes and named dishes | engine `food` |
| `breeds.ron` | named breeds as gene conditions | engine `genes` |
| `skills.ron` | skills, the curve, and each skill's guide lines | engine `skill` (exists) |
| `crops.ron` | crops (exists); its harvest and forage lists become loot entries | engine `crop` (exists) |

Every file is optional in a package; any package can add to any of them.

One package's `creatures.ron` also sets how many creatures the world holds, as one sets
movement (a second package setting them fails to load):

```ron
limits: (most: 512, animals_near: 6, monsters_near: 6),
```

`most` is the most alive at once, whatever the players. Wild ones (spawned or summoned)
stop short of it by the kept animals still waiting in the save, so those always have room
to come back. `animals_near` and `monsters_near` cap each kind within 64 blocks of a
player. Without any package's limits, these numbers are the defaults.

## A creature

```ron
// assets/content/base/creatures.ron
(
    creatures: [
        (
            name: "rabbit",
            kind: Animal,                          // Animal | Monster | Fish | Spirit
            rarity: Common,
            supernatural: false,
            level: 1,                              // combat level
            health: 6,
            attack: None,                          // or (damage: (1, 3), reach: 1.5, every: 1.2)
            defence: 1,
            body: (
                size: (0.5, 0.45),                 // width, height of the collision box
                model: Quadruped(                  // a part plan from the part library
                    body: (0.3, 0.26, 0.42), head: (0.22, 0.21, 0.24), legs: 0.1, snout: (0.12, 0.08, 0.04),
                    ears: (0.07, 0.22), ear_lean: 8.0, tail: (0.1, 0.1, 0.08), tail_droop: -40.0, haunches: true,
                ),
                colours: [(150, 118, 86), (226, 216, 196), (70, 52, 40)],   // coat, belly, features
                skin: (                            // how each individual's skin is painted
                    eyes: Side,                    // Side (prey) | Front (hunters) | Cluster | Hidden
                    fur: 0.12, belly: 0.32, feet: (196, 184, 164),
                    markings: [EarTips(colour: (82, 62, 46)), Muzzle(colour: (196, 176, 150))],
                ),
            ),
            movement: (style: Hopper, walk: 2.2, run: 7.0, turn: 360.0, jump: 1.2),
            habitat: (
                biomes: [("tag:meadow", 1.0), ("tag:grassland", 1.0), ("tag:heath", 0.5)],
                comfort: (temperature: (-12.0, 28.0), moisture: (0.15, 1.6), aether: (0.0, 1.0)),
                water: Land,
                active: Crepuscular,               // Day | Night | Crepuscular | Any
            ),
            group: (size: (1, 3), structure: Family),
            temperament: (aggression: 0.0, fear: 0.9, curiosity: 0.3, sociability: 0.4, territory: 0.1),
            senses: (sight: 16.0, hearing: 24.0, smell: 8.0),
            leash: 12.0,
            diet: (eats: ["tag:clover", "base:dandelion", "base:carrot", "base:wild_carrot"], favourite: ["base:carrot"]),
            behaviours: ["prey_small", "grazer"],  // sets from behaviours.ron, plus inline ones
            quirks: ["stands_to_look", "thumps_alarm"],
            loot: [
                (item: "raw_rabbit"),
                (item: "rabbit_hide", chance: 0.8, tools: [(tag: "knife", chance: 1.25)]),
                (item: "rabbit_foot", chance: 0.02, rarity: Rare),
            ],
            xp: (kill: 3.0),
            // Phase 2 onward:
            products: [],
            taming: (method: Lure, level: 1),
            breeding: (level: 5, food: ["base:carrot", "base:dandelion"], litter: (2, 5), gestation: 6.0, growth: 20.0),
            genes: [("coat", Alleles(["agouti", "black", "white"], dominance: [0, 1, 2])), ("size", Number(mean: 1.0, sd: 0.08))],
            forms: [(into: "moonhare", when: [Aether(0.7), GeneAbove("aether", 0.6), Fed("base:moonbloom")])],
        ),
    ],
)
```

### Bodies and skins

A body is boxes (engine `creature::model`), from a plan or listed one by one with
`Boxes([(role: Head, size: .., at: .., pivot: .., turn: (pitch, yaw, roll), colour: 0,
swing: Head), ..])`. The plans: `Quadruped` (optional `neck: (length, lift°)`, `hump`,
`haunches`, `snout`, `ears` with `ear_lean`, `tail` with `tail_droop`, `horns` shaped
`Straight | Curved | Curl | Antlers | Tusks`), `Biped` (`tilt`, folded `wings`, `beak`,
`crest` and `wattle` in the fourth colour, `tail` with `tail_lift`, or `arms` for walkers),
`Crawler` (jointed legs) and `Blob` (`shell`, eye `stalks`, circling `motes`). Each box
has a role (torso, head, ear, leg, horn, wing...) and a swing (leg, head, tail, ear, wing,
scuttle, orbit), which is all the animation needs.

Every individual gets its own skin, painted at 32 texels a block from the species'
colours and `skin` (engine `creature::skin`): eyes with a pupil and catchlight, hooves or
paws, pale ear insides, a ragged belly line, grain, feather rows, shell rings, and the
markings (`Spots`, `Stripes`, `Socks`, `TailTip`, `Mask`, `Saddle`, `Muzzle`, `Blaze`,
`EarTips`, `Dorsal`, `Hood`, `Collar`). The painter works in model space, so patterns run
across boxes, and the seed moves spots and grain so no two animals match. Genes will feed
the same painter.

Every field after `name` has a sensible default from its `kind`, so a minimal mod creature
is a name, a body, a habitat and a loot list; the behaviour generator fills the rest (see
"Generated behaviour" below).

## Behaviour

A behaviour entry is a template, its parameters, a weight, and considerations:

```ron
// assets/content/base/behaviours.ron
(
    sets: {
        "grazer": [
            (do: Graze(food: Diet, radius: 8.0), weight: 1.0, when: [Drive(Hunger, Rising)]),
            (do: Wander(radius: 6.0, pause: (2.0, 8.0)), weight: 0.3),
            (do: Herd(spacing: 2.0), weight: 0.5, when: [GroupNearby]),
            (do: Tempted(by: Favourite, radius: 10.0), weight: 2.0, when: [PlayerHolds(Favourite)]),
        ],
        "prey_small": [
            (do: Flee(from: Threats, radius: 10.0, path: Zigzag), weight: 4.0, when: [Threat(Seen)]),
            (do: Freeze(seconds: (1.0, 3.0)), weight: 1.5, when: [Threat(Heard)]),
        ],
        "melee_hunter": [
            (do: Chase(target: Prey, give_up: 24.0), weight: 3.0, when: [Drive(Aggression, Above(0.5))]),
            (do: Attack, weight: 5.0, when: [TargetInReach]),
        ],
    },
    quirks: { "stands_to_look": (...), "thumps_alarm": (...) },
)
```

Templates (engine vocabulary) and considerations are listed in
[creature-ai.md](creature-ai.md). A mod adds a set, adds entries to an existing species by
tag (`attach_to: "tag:grazer"`), or writes a species' whole list itself.

### Generated behaviour

A species that lists no behaviours gets them from the behaviour generator, from its
ecology: size, diet kind (herbivore, carnivore, omnivore, insectivore), habitat, group
structure, activity and supernatural flag (rules in [creature-ai.md](creature-ai.md),
"Behaviour generation"). Its quirks and movement numbers are drawn from its name's hash,
so a generated species behaves the same in every world. Regional and individual layers
apply to every species, written or generated.

## Loot

Inline in a creature (`loot: [...]`) or shared in `loot.ron` by name; format and rules in
[drops.md](drops.md). Tool modifiers name item tags (`knife`, `shears`, `silver`,
`fire`), so a mod's new knife works with every table.

## Food and recipes

```ron
// foods.ron: only what tags can't say
(foods: [
    (item: "raw_venison", food: 3, kinds: ["meat"], raw: true, spoils: 4.0, flavour: (savoury: 2)),
    (item: "emberfruit", food: 4, kinds: ["fruit"], warming: 6.0, aether: 1.0),
])

// recipes.ron
(recipes: [
    (name: "meat_stew", station: "pot", level: 25, xp: 60.0, burn: (from: 25, stop: 50), seconds: 12.0,
     inputs: [(kind: "meat"), (kind: ["vegetable", "root"], count: 2), (item: "water")],
     output: (dish: "stew", hunger_cap: 12)),
    (name: "hunters_stew", like: "meat_stew",
     inputs: [(item: "venison"), (item: "juniper_berries"), (kind: "root")],
     adds: [QuietSteps(minutes: 10)]),
])
```

Inputs match items by `kind` (tags) or name. A recipe never lists every vegetable.

## Genes and breeds

Genes are declared per species (`genes:`, at most eight) as alleles, earlier ones more
dominant, each with a wild `frequency`, a `cold` or `aether` push and `effects`
(`Colour(index, rgb)`, `Marking(..)`, `Plain`, `Scale`, `Yield`, `Temper`, `Hardiness`), or
as a `Number(mean, spread, range, effect)`. Each region of the world has its own
population of each species (docs/breeding.md, "Regional populations"). Breeds live in `breeds.ron` as conditions over genes; a breed
is recognised whenever an animal meets them, whichever package defined its species.

## Validation

Content fails to load, with the file and a reason, when:

- a name or tag reference finds nothing (an item, block, skill, biome tag, behaviour set);
- a creature's comfort overlaps no biome's climate box, or its biome selectors match no
  biome (it could never appear);
- a creature drops nothing any action uses, or an item is produced by nothing ("dead ends
  fail", design philosophy);
- a level, chance or count is out of range, or a loot table references itself;
- a supernatural species has no aether minimum;
- the limits leave no room (`most` 0), or one player's `animals_near` and `monsters_near`
  together are more than `most`.

Test-time checks (in `cargo test`): every species can be spawned and survives a simulated
day in the sandbox; the supernatural share of species is 15–30% and of encounters, over a
large seeded sample of spawn spots, near 20%; every skill has unlocks through 99.

## The engine and server pieces

| Piece | Crate | What it does |
| --- | --- | --- |
| `creature` | engine | definitions, registry, body plans, stats, temperament; pure data and math |
| `creature::ai` | engine | drives, considerations, utility selection, behaviour templates; pure functions over a snapshot, so tests and (later) WASM run them the same |
| `loot` | engine | tables, conditions, tool and level modifiers, rolls |
| `genes` | engine | alleles, inheritance, mutation, breeds |
| `food` | engine | ingredients, dish computation, effects; builds on `vitals` |
| creatures | server | live individuals near players: spawning, AI ticks, physics, combat, drops, saving tamed ones |
| populations | server | the regional counts process |
| protocol | protocol | `Creatures { changed, gone }` with position, yaw, gait, pose and look; combat and hit messages |
| creatures | client | interpolation and box models from body plans (outside `render/`, until graphics gives a proper pass) |

## Implementation plan

1. **Entity foundation** (now): creature definitions in `creatures.ron`, a server store of
   live creatures spawned near players by habitat and rarity, physics, data-driven
   utility AI (wander, graze, flee, freeze, herd, tempted, chase, attack, return home),
   replication and box models on the client, hitting creatures and being hit, loot on
   death, Attack, Defence and Hitpoints training from combat, `/summon` and `/creatures`
   commands, a gate step.
2. **The first animals and monsters**: rabbit, junglefowl, mouflon, aurochs, wild boar,
   red deer, fox, wolf, brown bear, mallard; cave crawler and shambler; moonhare, lumen
   snail and wisp as the first supernatural ones.
3. **Products, taming and breeding**: product timers (eggs, milk, wool), lure and feed
   taming, the simple breeding layer, then genes and inspect.
4. **Populations**: regional counts, hunting pressure, regrowth.
5. **Cooking**: stations, recipes, dishes and effects on top of vitals.
6. **Behaviour generation**: the ecology rules, quirks, regional modulation.
