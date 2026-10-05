# Drops and loot

Oct 5, 2026. What creatures, blocks and crops give, how often, and how tools, levels and
the world change it. One loot system for everything. Part of the creatures, food and
breeding design (see [creatures.md](creatures.md)).

Mariano's brief: mobs drop things; some drops are certain, others are not; tools raise
drop rates. RuneScape's drop tables and Minecraft's loot tables are the two models.

## Decisions

| # | Question | Decision |
| --- | --- | --- |
| 1 | One system | Creatures, blocks, crops, wild plants, products (shearing, milking) and containers all use loot tables in one data format. The crop `harvest` and `forage` lists already in crops.ron become loot tables of the same shape. |
| 2 | Certain and uncertain | Entries are either always (bones, raw meat) or have a chance; groups pick one of several by weight (RuneScape's main table). |
| 3 | Shared tables | Tables can reference other tables, so a "rare seeds" or "gems" table is written once and used by many creatures, as RuneScape's rare drop table is. |
| 4 | Tools | An entry may name tool tags that change it: a knife gets more meat and better hides, shears get wool and down, a silver weapon draws more from supernatural creatures, a fire kill drops meat already cooked. |
| 5 | Levels | Uncommon and rarer entries come a little more often with the skill the entry names (Hunter for animals, Mining for ore, Farming for crops): ×1.5 at 99, never more than one rarity step. Common entries gain an occasional extra one. |
| 6 | The world | Conditions read the creature (age, variant, genes, condition), the time, season, weather, biome and aether, so a sheep's wool follows its wool gene and moon dust drops only at night. |
| 7 | Determinism | Every roll draws from `hash(world, source id, tick, entry)`. |

## A loot table

```ron
(
    name: "deer",
    rolls: [
        // Always.
        (item: "venison", count: (1, 3)),
        (item: "hide", count: (1, 1)),
        // Only stags, in the months they carry antlers.
        (item: "antler", chance: 0.6, when: [Gene("sex", "male"), Season(["summer", "autumn"])]),
        // Better with a knife.
        (item: "venison", count: (0, 1), chance: 0.5, tools: [(tag: "knife", chance: 2.0)]),
        // Seeds of what it ate, from where it lived (see "Diets carry seeds").
        (table: "diet_seeds", chance: 0.08, rarity: Uncommon),
        // One of these, rarely.
        (one_of: [(item: "deer_heart", weight: 3), (item: "rabbit_foot", weight: 1)], chance: 0.02, rarity: Rare),
    ],
    skill: "hunter",
)
```

Each entry is one of:

- `item` with a `count` range and a `chance` (default 1: always);
- `table`, rolling another table (shared or the creature's own);
- `one_of`, a weighted pick from several entries;

plus optional `rarity` (for the level bonus and how the item's name is coloured), `when`
conditions, and `tools` modifiers.

## The chance of an entry

```
chance  = base chance
        × tool multiplier                      (best matching tool, from `tools`)
        × condition multipliers                (from `when` entries that scale, e.g. Aether)
        × (1 + level bonus)                    (uncommon and rarer only)
capped  = at most one rarity step better than its own rarity:
          rarity factors common 1, uncommon 0.35, rare 0.1, very rare 0.025,
          legendary 0.004 (research/farming.md 12.6)
level bonus = 0.5 × (level − needed) / (99 − needed)        (engine::crop::level_bonus)
```

Counts: `low..=high` uniform; a common entry gains one more with chance equal to the level
bonus (engine::crop::roll already does this). A tool may also add to counts.

So a rare 2% drop is 2.4% with a good knife, 3% at level 99 with it, and never more than
the uncommon rate. Legendary stays legendary.

## Conditions

| Condition | Example |
| --- | --- |
| `Killer(Player)`, `Killer(Any)` | only player kills drop trophies; wolves killing sheep leave only bones |
| `Tool(tag)` / `Weapon(tag)` | `fire` (cooked meat), `silver`, `blunt` (hides undamaged) |
| `Age(adult)` | lambs drop little |
| `Gene(name, value)`, `GeneAbove(name, x)` | wool count follows the wool gene; albino pelts only from albinos |
| `Condition(above)` | a well-kept cow gives more and better milk |
| `Variant(name)` | gloam wolves drop gloam pelts |
| `Time(night)`, `Season(...)`, `Weather(storm)` | moon dust at night; thunderbloom seeds after storms |
| `Biome(tag)`, `Aether(min)` | aether motes only where aether is high |
| `Level(skill, min)` | some drops only for skilled hands (a clean griffin feather needs Hunter 85) |
| `Not(...)`, `Any(...)` | combinations |

## Where loot comes from

| Source | When | Skill that boosts it |
| --- | --- | --- |
| Creature death | the killing blow (by player, other creature, fall or fire) | Hunter for animals; none for monsters |
| Products | shearing, milking, collecting eggs, gathering honey, shed antlers on the ground | Farming |
| Blocks | breaking; ore may drop gems (`drops` and `drop_count` in blocks.ron stay the simple case) | Mining |
| Crops | harvest, pick, forage (crops.ron's lists) | Farming |
| Containers | ruins, nests, burrows: rolled once when first opened | none |

Drops pop out into the world as dropped items (`server::items`), which players draw in. In
multiplayer the player who made the kill has a few seconds' head start on picking up.

## Diets carry seeds

Animals that eat seeds and fruit carry them: a creature's table can roll `diet_seeds`,
which gives the seed of a plant its diet includes and that grows in the cell where it
lived. Birds and boars spread wild crops this way, and a player looking for a rare seed
can hunt the animals that eat it instead of searching every meadow. Dung dropped by
grazers can sprout what they ate, now and then, where it lands (the plant pool's comfort
decides if it survives).

## Shared tables to start with

| Table | Contents | Used by |
| --- | --- | --- |
| `diet_seeds` | seeds of the creature's diet plants that fit its home | grazers, birds, boars |
| `bones` | bone, bone meal | every vertebrate |
| `rare_seeds` | uncommon to legendary crop seeds (tomato, strawberry, chili, saffron) | monsters, rarely (RuneScape's herb-seed drops) |
| `gems` | quartz, amethyst, diamond (very rare) | ore blocks, cave crawlers' nests, sand wyrms |
| `aether` | aether mote, moon dust, star shard | supernatural creatures, by aether at the spot |

## Knowledge

A creature's drop table shows in the bestiary once you have killed or butchered one: each
item you have seen drop, with its rarity colour. Rates are never shown as numbers, only
as words (always, often, sometimes, rarely, very rarely), as RuneScape's wiki would.
