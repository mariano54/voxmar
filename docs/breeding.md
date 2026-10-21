# Taming, husbandry and breeding

Oct 6, 2026. How wild animals become yours, how you keep them, and how breeding works,
from Minecraft-simple to deep genetics. Breeding is part of the **Farming** skill
(Mariano); taming is part of the **Hunter** skill, which Mariano chose (see [skills.md](skills.md)). Part of the creatures, food and breeding design (see
[creatures.md](creatures.md)).

## Decisions

| # | Question | Decision |
| --- | --- | --- |
| 1 | Accessible | The first layer is Minecraft's: feed two adults the food they like and a baby comes. Nothing else is required. |
| 2 | Deep | Underneath, every animal carries genes. Selective breeding, domestication, mutations, named breeds, hybrids and inbreeding reward players who care, with an inspect action that reveals more with level. |
| 3 | One genetics engine | The same genes code serves animals and crops (crop crossing, research/farming.md 12.9), so seed stats and coat colours follow the same rules. |
| 4 | Where the world comes in | Condition (food, water, shelter, space, climate) sets fertility and product quality; the aether field at the pen raises mutation; seasons set breeding seasons; temperature hatches eggs. |
| 5 | Supernatural from mundane | Mundane parents in a high-aether pen, fed aether plants, can have a supernatural child. Moonhares come from rabbits this way. |
| 6 | Cheapest update | Gestation, incubation, growth and products are due times in a heap, as crops are. A barn of animals costs nothing between events. |
| 7 | Levels | Gate which species you may breed and what inspect shows; raise mutation finds and product quality a little (the shared formulas in [skills.md](skills.md)). Never make anything grow faster. |

## Taming

Each species names a method and the Hunter level it takes (levels in
[skills.md](skills.md); the first set tops out at 50). Trust is a meter that can decay and
backfire: it fades over days scaled by the species' wildness (RimWorld), a failed attempt
can turn the animal on you (RimWorld's manhunter chance, a grizzly's 30%), and how you
treated it as trust filled shapes what it becomes (Don't Starve's beefalo: fed, ridden or
fought).

| Method | How | Species |
| --- | --- | --- |
| Lure | lead it home with food it likes held in hand; it is yours once penned and fed | junglefowl, rabbit, mallard, mouflon, aurochs |
| Feed | offer a favourite food repeatedly; trust rises with each, falls if you hurt or startle it | goat, horse, wolf, fox, alpaca |
| Patience | stay near it, sneaking, without threatening; trust rises with time | lynx, reindeer, frost stag |
| Rescue | heal a hurt one (with a healing herb or bandage) and feed it | boar, bear (cub) |
| Imprint | be the first thing a hatchling sees | ducks, sea turtles, griffins (from an egg) |
| Gift | bring the right strange thing | moonhare (aether mote), silver hart (silvermint, then patience) |

Trust is a value 0..1 per creature and player. Taming fails (and the creature flees) if
you attack it, sprint at it, or a companion of yours does. A tamed animal is saved as an
individual with its owner, name and genes.

**Wild stock is wild.** A tamed mouflon has little wool and a skittish temper; a tamed
boar is fierce. Domestic breeds come from breeding (below), so the first flock is the start
of a project, not the end.

## Keeping animals

One player keeps at most 64 animals, herds and pets together, counting any not yet back in
the world. At the cap a wild animal won't take your food and a pair won't take breeding
food, with a line saying why. A cap lowered later takes none of yours away: taming and
births wait until you are under it.

To make room, let one go: Shift and right click your own animal, and again within five
seconds (the first click only asks). It wanders off wild, keeping its genes and breed but
not its trust or readiness, and from then on lives by wild rules: it is not
saved, goes when nobody is near, and gives no products or young. Letting go pays nothing,
and taming one that was let go, by anyone, pays no Hunter experience and counts for no
quest (design draft 99). Nor does it mend an animal: one let go with a poor condition
is taken in again with it, while a better one comes back as an ordinary newly tamed animal.

An animal's **condition** (0..1) is the product of what its species needs:

| Need | Met by |
| --- | --- |
| Food | grazing its food blocks within its leash, or feed in a trough |
| Water | water, a well or a feed trough within its leash (`drinking_water` tag) |
| Shelter | a roof within a few blocks when it rains or snows (except ducks) |
| Space | blocks of room per animal (chickens 2, sheep 6, cattle 12); crowding lowers condition and raises disease |
| Climate | its comfort range (the temperature field): cold coats thrive in the north, zebu in the heat; a heated barn or a shaded one fixes a misfit |
| Company | social species want at least one of their kind |

Condition drives everything that follows: product amount and quality, fertility, growth
of the young (well-fed young grow at the full rate, starved ones slower), and illness.
Condition comes from the world and the keeper's work, never from a level.

**In the game now** (engine `creature::condition`, server `condition.rs`; every number in
`creatures.ron`'s `condition`, from the progression plan and Review's rulings). Every 10
seconds each kept animal in loaded land looks at its needs. A need met counts 1, partly
met 0.8, unmet 0.5, and the target is their product. Below half health it is held to 0.5.
Condition walks toward the target by at most 0.15 an in-game day up and 0.25 down, so one
wet night or a missed meal shows only over days. An animal whose land, or home, is not
loaded is paused with it: it neither hungers, changes nor lays while nobody is near.

- Food: fed (grazing its food, its food by hand, or its food from a feed trough within its
  leash of home once a day unfed, docs/home.md) within a day, partly within two. A
  hungry kept animal takes its food from your hand as a meal; one that is fed takes only
  breeding food, and only when that could ready it.
- Water: something to drink within its leash (7 blocks) of its home: any block tagged
  `drinking_water`, which is water itself, a well and a feed trough in the game's own content
  (a mod's water station is one tag away). A dug channel does too; an animal with no
  water near can't breed. Loading refuses creatures when no block carries `drinking_water`.
- Shelter: rain or snow on it under open sky is met if within 3 blocks there is a spot it
  could stand in with something solid a few blocks over it, partly met otherwise; snow is unmet unless its kind's comfort reaches −10 °C (reindeer,
  arctic fox, mountain goat, wolf). Ducks don't mind.
- Space: the ground within 5 blocks (about 78) against what every kept animal on it wants
  together (`needs: (space: …)`: chickens 2, sheep 6, cattle 12; else about 8 blocks per
  block of width). Up to twice crowded is partly met. Fences and walls don't count yet.
- Climate: its comfort fit at its home, 0.75 and up met, 0.25 and up partly.
- Company: a social species (sociability 0.5 and up) wants one of its kind within 14.

A tamed animal starts at 0.6 and a newborn at its parents' average. Products come 0.5 to
1.2 times as often by condition (0.5 + 0.7 × condition), while the yield gene sets how many
come; litter odds and the share of work a treadwheel or gin gets scale the same way.
Breeding takes condition above half: breeding food given to one kept worse says so, with
what it wants, and stays in hand unless the animal is hungry. One unfed past two days stops
working. Inspecting a kept animal shows its condition in a word (poor, too poorly kept to breed;
fair, good, excellent), whether it is improving or getting worse, and what it wants; Farming 30 shows
the number. Troughs, feed quality, illness and inbreeding come next.

**Feed** is cooked food (Cooking 12): mash, seed mix, hay. Each species prefers feed whose
ingredients match its diet tags; better feed raises condition faster and counts as its
breeding food. Special feeds act on genetics (see "Mutation").

**Illness** comes from crowding, wet and poor food, spreads to neighbours, and is cured
with herbs tagged `alchemy:healing` (dandelion, oxeye daisy and others already carry the
tag). Untreated, it lowers condition and can kill.

## Breeding: the simple layer

1. Two adults of a species, owned, in condition above half, in their breeding season
   (year-round for chickens and rabbits; autumn for deer; spring for most birds).
2. Feed both their breeding food (data, usually their favourite food or feed).
3. They court for a moment (a behaviour, see [creature-ai.md](creature-ai.md)), then:
   - mammals: after the gestation time, a litter is born (rabbits 2–5, sheep 1–2, cattle 1);
   - birds and reptiles: eggs are laid in a nest if one is near, or on the ground.
     Built so far: a kept bird's egg laid inside its keeper's home goes into the home's
     stores, nearest first (`nests` on the product; [home.md](home.md), "Eggs in the stores").
4. Both parents rest for a cooldown before breeding again.
5. Young follow their mother and grow to adults over the growth time.

The Farming level for the species is needed to start it. Experience comes at the birth,
scaled by the species' level (`10 × 2^(level / 10)`), with a first-time bonus for a new
breed or variant.

**Eggs** hatch after their incubation time if kept warm: a hen sitting on them, or a nest
where the temperature field (plus any heat nearby) is within the species' range. A cold
coop in winter needs a heater; a sunwing egg needs a fire. Collected eggs are food instead.

## Genes: the deep layer

Each species declares its **genes** in data. Two kinds:

**Discrete genes** (Mendelian). Each animal carries two alleles; each parent passes one at
random; dominance decides what shows. Examples: coat colour (agouti, black, white),
pattern (spotted), polled (no horns), cold coat (thick, white in snow), desert ears, albino
(recessive, rare). Hidden carriers make breeding a puzzle: two brown rabbits can have a
white kit.

**Quantitative genes** (many small effects). Each is a number around the species' mean;
a child's value is the mid-parent value plus a little noise (`σ` per gene, the breeder's
equation's heritability in data). Examples: size, yield (wool length, milk, eggs per
week), speed, stamina, temperament (calm to fierce), hardiness (shifts comfort toward heat
or cold), fertility, longevity, aether affinity.

**Regional populations.** Wild animals are not all alike. The world is cut into regions
384 blocks across, and each region holds its own population of each species: its own mix
of alleles and its own means, drawn from the world seed, then pushed by the region's
climate (an allele may be commoner in the cold, `cold: 2.5`, or where aether is high).
So wolves in the snowy north run paler and larger than wolves in southern woods, and one
valley's rabbits are mostly black. A breeder after a trait travels for it, and wild stock
tamed far away is new blood.

**In the game now** (engine `genes.rs`): allele and number genes in `creatures.ron`
(`genes: [(name: "coat", kind: Alleles([(name: "white", frequency: 0.3, cold: 2.5,
effects: [Colour(0, (238, 236, 230)), Plain])])), (name: "size", kind: Number(mean: 1.0,
spread: 0.06, range: (0.78, 1.3), effect: Scale))]`), regional populations, inheritance
(one allele from each parent, numbers at the parents' middle plus a third of the wild
spread), mutation more likely with aether, and effects on coat colours and markings, size,
product yield and temper. Kept animals' genes are saved. Breeds, domestic forms,
inbreeding, inspecting and hybrids come next.

What players do with it:

- **Selective breeding.** Keep the woolliest, breed them together, and wool rises a little
  each generation. Because the noise is small, it takes generations, which is the game.
- **Domestication.** Breed tamed wild stock for calm temperament and yield; once both
  pass a species' thresholds for a few generations, the line becomes its domestic form
  (mouflon to sheep, wild boar to pig, aurochs to cattle, junglefowl to hen). Domestic
  forms have their own looks, products and drops.
- **Named breeds** (in the game now: `breeds: [(name, title, when: [Shows(gene, allele), Above(gene, n), Below(gene, n)])]` per species, named when inspecting at Farming 10). A breed is a data definition: a set of gene conditions (a highland
  aurochs: cold coat, long hair, hardy to −10 °C). When an animal meets one, it is that
  breed, named in the studbook, with experience the first time. Mods add breeds as data.
- **Hybrids.** Species in the same family may cross where data says so: horse × donkey =
  mule (strong, sure-footed, sterile); red deer × frost stag = rimehart (supernatural,
  rare); junglefowl × sunwing = emberhen (lays warm eggs that hatch nothing but cook
  themselves). Hybrid fertility is data.
- **Inbreeding.** The studbook (Farming 20) records parents. A child's inbreeding
  coefficient from shared ancestors lowers fertility and condition and makes hidden bad
  recessives show. New blood comes from taming wild animals (Hunter) or trading, which
  keeps the two skills tied.

## Mutation

At each birth, each gene may mutate: a new allele (a colour nobody has seen, a twisted
horn) or a jump in a quantitative gene. The chance is small and comes from the world:

```
chance = base (0.5%, `genetics.mutation` in creatures.ron, from 0 to 0.2)
       × aether at the pen          (×1 at none, up to ×5 where aether is highest)
       × mutagenic feed             (whisper oats, aether orchid: ×2)
       × level finds                (up to ×1.5 at 99, the shared rare-find bonus)
```

**Supernatural births.** A species may name a supernatural form it can become, with the
conditions: rabbits become moonhares when aether at the pen is high, both parents' aether
affinity is high, and they were fed moonbloom. The first one is a discovery; after that,
moonhares breed true. This is how a farm in a mundane valley can raise supernatural
animals, at the cost of building where aether is high or bringing aether feed from far.

## Inspecting

**In the game now:** right-click a creature with nothing it wants in hand. Anyone sees its
kind, level, keeper, health, age and temper; Farming 10 shows what its genes show,
20 what it carries hidden, 30 the exact numbers, 40 the odds of a mutation where it
stands (server `inspect.rs`).

The inspect action shows more with Farming level: condition, age and personality in words
at 1; visible genes in words ("long wool, calm") at 15; hidden alleles and carriers at 30;
exact numbers and the inbreeding coefficient at 40; mutation odds at the pen at 50. This is the seed
analysis idea from farming research, applied to animals: information is progression.

## Crops share it

Crop crossing (research/farming.md 12.9: growth, gain and resistance stats, deterministic
recipes then similarity-weighted crosses) uses the same genes engine: discrete and
quantitative genes, mid-parent plus noise, mutation chance from aether and feed (here,
fertiliser). A player who learns one has learned both.
