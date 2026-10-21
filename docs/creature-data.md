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

Under `body.skin`, `eye: 0.065` sets the eye radius in model blocks (scaled with
`body.scale`). It must be finite and positive. Explicit `eyes: Front` eyes paint a pale
sclera round the iris, pupil and catchlight, so small human heads do not paint as black
slits. Omit `eye` to keep the existing head-derived sizing and animal painting.

A package's `creatures.ron` may also set how many creatures the world holds, any of these
numbers alone. A later package's numbers (packages load in name order) replace an earlier
one's, and the load log names both packages and the numbers replaced, so a mod tunes one
without forking base (the project-wide rule for single-valued data, Design draft 91):

```ron
limits: (most: 1024, wild_most: 512, kept_each: 64, keepers: 8, animals_near: 12, monsters_near: 8, first_night_monsters: 3,
         spawn_every: 1.5, spawn_near: 20.0, spawn_far: 40.0,
         spawn_unseen_angle: 75.0, empty_for_monsters: 1.5, empty_for_animals: 2.5),
```

Wild creatures and kept animals have budgets of their own, so a big ranch never stops wild
animals and monsters spawning for anyone:

- `wild_most` is the most wild creatures alive at once; kept animals never count against it.
- `kept_each` is the most animals one player keeps, herds and pets together, alive or
  waiting in the save. Taming one more, or a birth past it, is refused with a message, and
  breeding food is not taken while no young could come. A keeper already over the cap (a
  package lowered it, or an old save) keeps every animal; taming and births wait until
  they are under it. The per-species caps by Farming level planned in P31 will sit inside
  it.
- `most` is the most alive at once, wild and kept, and must hold `wild_most` and `keepers`
  players' `kept_each`; a server for more players raises `keepers` and `most` together.
  Wild ones (spawned or summoned) and births also stop short of it by the kept animals
  still waiting in the save, so those always have room to come back; past `keepers`
  players at their cap, wild ones get fewer.
- `animals_near` and `monsters_near` cap each kind within 64 blocks of a player;
  `first_night_monsters` is the lower cap on monsters until the world's first dawn (a world
  begins at sunrise: its first day, and any darkness running past its end), when the first
  night is a new player's first meeting with them.
- A spot with a block giving light within 5 blocks is never dark: torches and lit bases keep
  monsters from spawning there, on the surface as underground.
- `spawn_every` is the seconds between spawn attempts around each player (one spot an
  attempt), `spawn_near` and `spawn_far` how far from the player the spot lies (20 to 40
  blocks: nearer puts more of what the caps allow within sight), and `empty_for_animals` /
  `empty_for_monsters` how heavy "nothing here" weighs against the species that suit the
  spot (a common one at home weighs about 1), so a higher number leaves more spots empty.
  Together with the caps these set how full the land feels, and how fast it refills: with
  these numbers a player's surroundings fill in 10 to 30 seconds of standing still, not the
  minutes to an hour older notes assumed. Fast refill is intended.
- `spawn_unseen_angle` is the half angle in degrees around where any joined player looks
  (75 covers the screen; 0 is off). Natural spawns, including every group member, wait
  while a player could see them appear. The individual's gene-scaled body is checked at
  its centre and corners, so heads above walls and flanks round their edges count too.
  Behind a hill or trees is fine. Explicit summons and settlement people are unaffected.

What no package sets keeps the defaults above.

The same file sets, the same way, when creatures' blows train Defence (design wiki draft
63):

```ron
defence: (fought_within: 10.0, least_level: 0.5),
```

A blow that misses trains the player's Defence only if they landed a damaging blow on that
very creature within `fought_within` seconds, and only from a creature of at least
`least_level` times their Defence level. Standing among weak monsters, or tapping a penned
hen between a strong one's blows, trains nothing; with two players at one creature, only
the last to hurt it trains on its blows.

The same file may set the condition rules of kept animals the same way, field by field
(see breeding.md, "Keeping animals"):

```ron
condition: (up: 0.15, down: 0.25, met: 1.0, partly: 0.8, unmet: 0.5, tamed: 0.6, hurt: 0.5,
    output: (0.5, 0.7), breed: 0.5, fed: (1.0, 2.0), space_radius: 5.0, company: 14.0,
    climate: (0.75, 0.25), wet: 0.05, roof_within: 3.0, snow_hardy: -10.0),
```

A species says what it needs when kept with `needs: (space: 6, shelter: false, company:
true)`, each optional: room from its width, shelter wanted, company if it is social.

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
            senses: (sight: 16.0, hearing: 24.0, field: 170.0, close: 3.0, react: 0.3, keep: 10.0),
            leash: 12.0,
            diet: (eats: ["tag:clover", "base:dandelion", "base:carrot", "base:wild_carrot"], favourite: ["base:carrot"]),
            behaviours: ["prey_small", "grazer"],  // sets from behaviours.ron, plus inline ones
            quirks: ["stands_to_look", "thumps_alarm"],
            loot: [
                (item: "raw_small_game"),
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
- the limits leave no room for wild creatures (`wild_most` 0), one player's `animals_near`
  and `monsters_near` together are more than `wild_most`, or `most` is less than
  `wild_most` and `keepers` players' `kept_each`; `spawn_every` is under 0.05 s or not a
  number, `empty_for_animals` or `empty_for_monsters` is negative or not finite, or
  `spawn_far` is not more than `spawn_near` (which must be 0 or more), or
  `spawn_unseen_angle` is not finite or is outside 0 to 180;
- `defence` makes a fight last no time (`fought_within` 0 or less) or asks a negative
  `least_level`;
- the condition rules have a share outside 0..1, a need met counting less than partly
  met (or that less than unmet), a rate or reach of 0, an output that falls below 0,
  `fed` days or `climate` fits out of order, no company reach, a negative `wet` or
  `roof_within`, or no `snow_hardy` temperature.

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

For admin inspection, `/creatures populate` makes a bounded batch of ordinary natural spawn attempts and holds new arrivals still for a minute. It obeys visibility, darkness and all population budgets. `/creatures disperse` removes wild creatures within 64 blocks without loot, leaving kept animals alone. The game gate compares dark births on a clear pad with a pad lit by a sparse torch grid after dispersing the first population, so a full budget cannot falsely pass the light check. `/creatures release` lets go of a hold (`/creatures hold`, `/summon ... held`) at once: the gate's Climb step holds its wolf at the foot of a step and lets it go there, so the step is climbed every run.
