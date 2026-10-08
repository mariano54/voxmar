# Settlements

Hamlets and villages are planned by worldgen (`crates/worldgen/src/settlements/`) from
`assets/content/<package>/settlements.ron`. Everything a settlement is made of is a list
in that file; the code only rolls from the lists and draws what it rolled.

## Where they stand

The world is cut into squares (`grid.spacing`, 1024 blocks). Each square rolls a
settlement with chance `grid.chance` (0.45), kept `grid.margin` blocks inside its edges so
it never reaches a neighbour's square. Within the square the planner scores spots every
`site.step` blocks (16) for flat, firm, dry ground with fresh water near, and takes the
best of the first `site.tries` (6) that:

- has firm, dry ground (no river bank, crag or terrain shape) within `site.firm` (24)
  blocks of its middle, and no sea within `site.reach` (48);
- has a river or lake (not the sea) within `site.water` (48) blocks; a closer look finds
  the creeks the coarse samples miss;
- is not in a biome with an `avoid` tag (`no_settlement`, oceans, rivers, peaks, ice,
  fantastical and giant biomes);
- has a temperature between −10 and 36 °C: as likely as anywhere between 0 and 26, a
  quarter as likely at the edges (`site.taper`);
- prefers gentler land: as likely as anywhere in biomes of level 1 to 5, 0.7 times as
  likely in harsher ones (`site.levels`, biome levels P15, design draft 95; above level 5
  rather than 3 so the level 4 and 5 villages that grow chickpea, rice and taro stay);
- has no sinkhole or ravine within 150 blocks.

Once planned, its buildings keep `site.bank` (3) blocks from water, and the ground under
them must average `site.fertility` (0.3; 0.15 in cold and desert biomes, `site.poor`).
These are design's numbers (wiki, settlements, draft 60). With them about one square in
seven holds a settlement, four hamlets to every village.

A new player finds one within `rules.spawn_settlement_within` (1024) blocks of where they
spawn: where no square near is settled within that reach anyway, the nearest square in
reach that can hold one always does, its climate ruling no spot out and its next best
spot tried where one cannot be laid out. Where no spot in reach will do (a strip of coast
under mountains), none is promised. A player never spawns inside a settlement.

No two settlements within `rules.names_apart` (4096) blocks of each other share a name
(design, draft 81). Names draw from a stream of their own: each square offers a few
names, takes the first its neighbours in that reach don't also offer, and so needs no
neighbour planned to know its own.

## The levers

Every settlement draws five levers, each 0 to 1, and every list entry may lean on them:
an entry's weight is multiplied by `1 + lean × (2 × lever − 1)` for each lever it names.
The levers are correlated, so a place reads as a whole:

| Lever | Drawn from | Leads to |
|---|---|---|
| wealth | luck, fertile soil, fresh water, flat ground | villages over hamlets, lamps along lanes, paved lanes and squares, town houses and towers, flower boxes, "Great …" |
| age | luck (two rolls averaged) | weathered and mossy blocks, moss on walls, "Old …" or "New …" |
| order | luck, a little wealth | straight streets and greens over scatters, straighter lanes, fewer materials (houses of a piece), "Long …" |
| crowding | luck, a little wealth | more houses, closer together, "Little …" where low |
| green | forest and plants around | greens, gardens, hedges, orchards, more yard props, more trees left in the glade |

## The lists

- `sizes`: hamlet and village, with house and lane counts, the square's size, places they
  always have (well; trader, inn and smithy in villages), places they sometimes have
  (stalls, fields, pens, orchards, woodyards, shrines), and the chance of a centrepiece
  (rare in hamlets, most villages).
- `layouts`: spokes, street, green, scatter, fork, crossroads (four ways meeting square
  on), ring (a lane round the square, houses either side), riverside (a street along the
  bank of the water near, only where there is some) and terraces (streets along the
  slope, one above another).
- `houses`: twenty kinds, each with its own size ranges, storeys and leans:
  - homes: cottage, long house, town house, farmhouse, hut, tower house, hall, wing house;
  - new outlines (`form`): a roundhouse under a cone, an A-frame that is all roof, a
    courtyard house with rooms round an open court and a tree in it (villages), and a
    house over a shop, its counter open to the street under an awning (villages);
  - working buildings, one to a place (barns two), each naming its `work` for the
    people who will work there and what stands `inside`: bakery (oven, counter), mill
    (millstone, bins), tannery (vats, racks), barn (stalls and hay, a gambrel roof,
    wide doors), granary (bins, on low stilts), smokehouse (a smoking fire, racks),
    chapel (an altar between two lights, pews; its altar will restore prayer) and
    boathouse (a boat, wide doors; beside water only). Bakeries, mills, tanneries and
    chapels stand in villages. A building with fittings has one storey;
  - a house of two or more storeys has stairs up its walls to each floor, clear of the
    door and the hearth; windows are old glass, which breaks to nothing, so a village is
    no glass mine.
- `roofs`: how each shape leans (gable, hip, half-hip, saltbox, gambrel, conical, flat,
  shed), multiplying the weights each style gives it: half-hips and cones where it is
  old, saltboxes where it grew rather than was planned, gambrels in green, roomy places,
  sheds where it is poor. Round houses take cones and courtyard houses hips whatever
  was rolled, unless flat; A-frames are steep gables.
- `extras`: chimney, porch, lean-to shed, flower boxes, garden, hedge, lamp, woodpile,
  moss, wing, dormer, balcony, outside stair, shutters, awning, sign board, rain barrel,
  weathervane, dovecote and herbs drying under the eaves, each by its own chance; a
  house's `always` list adds its own, and forms leave out what they have no room for.
- `props`: woodpile, crates, stone pile, flower bed, herb planter, campfire, lamp, bush,
  compost, boulder, cart, barrel, hay bale, laundry line, fence run, scarecrow, skep,
  anvil, grindstone, washtub, sacks, chicken coop and signpost, drawn by the code; a boat
  drawn up on the bank and nets drying, on the bank of fresh water (`water: true`); and
  props of a package's own, given as `blocks` (positions from the spot, y 1 on the
  ground, and block names), like the log bench, cairn, drying rack, trough and stacked
  stone in base. Props stand turned to the lane or any way in yards, and keep their whole
  footprint clear of buildings and lanes.
- `centrepieces`: the one great thing a place is known by: a great tree, a bell frame, a
  fallen statue, a maypole, a market cross, a fountain, a standing stone or a duck pond
  at the heart of the square (which grows to hold it, the well moving beside it); a
  stone circle, a watchtower ruin, a windmill or a beacon out by the end of a lane. Each
  is made of its `parts` (log, leaves, stone, worn stone, water, cloth, light, bell;
  `{wood}` the place's wood), any left out coming from the style, or is a package's own
  `blocks` round its middle.
- `styles`: the building materials of a place by biome tag (sandstone, red sandstone,
  mudbrick, stilts, log cabin, stone cottage, blossom house, birch cottage, maple lodge,
  jungle stilts, turf house, villa, and timber frame anywhere else). A style names its
  woods, its roof shapes by weight, and a weighted list per part (foundation, floor,
  walls, frame, roof, trim, paving, hedge, weathered, crops); `{wood}` and `{trim}` stand
  for the two woods the settlement picks. Crops are everyday ones, by region (design
  draft 64): rye by log cabins and turf houses, chickpea in sandstone, mudbrick and
  villa country, rice on stilts in wetlands and jungles, taro in jungles, wheat, carrot
  and potato elsewhere. A size's fields grow only crops up to its `crop_level` (hamlets 5:
  wheat, carrot, rye; villages 12: the rest), by weight, always growing (any stage but the
  last of its `stages` in crops.ron), never ripe; loading fails on a crop rarer than
  uncommon, above every size's level, or a style with nothing a hamlet may grow. Orchards
  grow only fruit up to the same level. A settlement keeps one to three entries of each
  list, fewer where it is orderly. Where two styles fit a biome, either may be
  built.
- `names`: name parts by biome tag, each with its Japanese reading, small words for
  hamlets ("End", "Cross"), and prefixes that lean on the levers ("Old", "Great"), with
  how often each is used (`small_chance`, `prefix_chance`).
- `kana`: how the Japanese parts join where the English runs a consonant into a vowel:
  Sab + ir is Sabir, so サブ + イル is サビル, not サブイル (Glenach グレナック, Oasis オアシス).
- `rules`: every other number planning reads, documented in the file: the settlement
  promised near spawn, how levers are drawn, how lanes are walked (width, bends, steps,
  greens, rings, terraces, side lanes grown, the square round a centrepiece and the well
  beside it, a green's rim), how lots are laid along them (gaps, setbacks, the steepest
  ground, field or pen, the outskirts), the sizes of fields, pens, orchards, smithies and
  the rest, the inn's and trader's sizes, windows, plinths and moss, and how props are
  spaced. Loading refuses numbers planning cannot use or that would never finish.
- `site`: where a settlement stands (see above), and the glade it opens in the woods.
- `furnishings`: the blocks every settlement uses whatever its style (torch, campfire,
  table, farmland, water, dirt, planter, anvil, barrel, bell, altar) and the lists of
  flowers, bushes, stall canopies, compost, boulders, orchard fruit (in blossom or
  unripe, never the ripe leaves), hay, things hung to
  dry, nets, sacks, crates and cloth. The altar (`base:stone_altar`) stands in chapels and
  wayside shrines, where it restores prayer. Left out, the barrel is a log, crates are
  the place's trim and a shrine has its light alone. Every name must be a block when the
  game loads its content.

A package adds to every list, and its `grid`, `site` and each furnishing it names replace
earlier ones, so a mod can add a style, a name culture, a house type or a prop without
code. Its `rules` replace section by section: a mod giving only `rules: (lanes: (...))`
replaces the lanes (fields it leaves out take the code's defaults) and keeps every other
section as earlier packages set it; each special size it names replaces that kind's.

## People

Who lives in a settlement is drawn with its plan, from the seed alone
(`settlements/people.rs`, `people` in settlements.ron): each home holds as many as its
house type says (`people`, fuller where it is crowded), each working building takes its
workers from the homes nearest (`jobs`), a village keeps guards by its size (`guards`),
and `folk` fill the rest by weight. Their days are `routines`: slots of the day, each
drawing one place a day (asleep at home, at work, the square, the inn, a neighbour's
door, the gates), so no two days and no two people go quite alike. Every species they
are must be a villager (kind `Villager`, `creatures_villagers.ron`) and every job and
work must name a building's work or kind, or the content fails to load.

How they look (`creatures_villagers.ron`): every person is the same listed boxes (head with
a nose, torso and belt, sleeves and bare hands, legs) moving as animals' parts do,
dressed by genes: skin by the climate of the land about, a shirt and trousers, hair; a
calling adds a hat or an apron, a priest a long robe and stole, a guard a helmet and iron
shoulders (colours 3 to 5). Their voice is the synthesized `person` (`creature_sounds.ron`),
higher for a child. Clothing by culture is for a later package.

The server keeps them only near players (`crates/server/src/villagers.rs`, every number
in `people.live`): a settlement comes to life within `active` blocks of a player, its
people appearing where their day has them now with looks from their seed, and goes again
past `release`. Only residents within `nearby` (96 blocks) live, at most
`per_settlement` (48) in each place. Their separate reserve (`most`, 512) never competes
with wild or kept animals. They walk their day by ways found over the cells a body can stand in,
within a search budget a tick. They are never wild-spawned, never saved, never tamed or
summoned, drop nothing, teach no combat (`xp.combat` 0, guards aside) and go in no
journal.

Someone killed stays dead: the world's save keeps who died (`villagers.ron`), so neither
leaving, walking far nor a restart brings them back, and a newcomer with looks of their
own takes their place `newcomer` days on (five, never under a quarter), one a day at most
(`newcomers_per_day`). A villager struck calls the guards within `help`: they fight the
one who did it, and only them; a guard that comes no nearer them for `give_up` seconds
(they stand where it cannot go) breaks off and goes home for `calm`. During this cooldown
that player earns no combat or Defence XP from the guard, and their blows cannot restart
the chase. Recovery waits a further `give_up` seconds without a blow; each new blow delays
it. Each quarry keeps an independent cooldown by the player's canonical name; a second
player cannot replace the first cooldown. These outlive forgetting a town, so
leaving and rejoining cannot reset it. A disappearance without a death releases the resident without starting a newcomer
clock. Wells and centrepieces use ground spots outside their footprint, and indoor
spots put feet one block above the solid floor, keeping people off furnishings.

Field tending stays deferred until P54 theft and crime rules: unsown village fields stay
unripe in P50b so a village is not a free food source. Settlement witnesses and hunter
roles remain follow-ups. Single-valued people data and same-named routines follow the
package layering rule, with load notes naming each replaced field and its earlier setter.

## What else knows about them

- Trails join settlements' gates (the ends of their main lanes) to each other and to
  landmarks within reach, and route round their buildings, never through them. Every
  settlement with somewhere to go is joined at a gate; two gates towards one place share
  the shorter trail. A place no trail may run through (a settlement's other gate, a second
  viewpoint beside the first) stands in no trail's way.
- The snow blanket stops at roofs, however tall, so no snow lies inside a building.
- Every storey can be walked to: each stair's top three steps open to the floor above,
  so each step is jumped with room overhead.
- Servers on one world in one process share its generator (`Overworld::shared`), so the
  settlements, trails and rivers round spawn are planned once.
- Trees and decorations keep out of buildings, lanes and a glade round the houses; a crown
  that still reaches over a roof is cut back over it and `site.eaves` blocks round it.
- `Overworld::settlements_near` lists them with their buildings (each with what it is
  in the lists and the work done in it), doors and gates, for gameplay (villagers,
  shops) and maps; `/locate settlement` finds the nearest.

## Looking at them without the game

- `cargo test --profile iter -p voxmar_worldgen settlement_preview -- --ignored --nocapture`
  draws the settlements near spawn (`SEEDS`, `RADIUS`, `COUNT`, `ONLY`), and with
  `FOCUS=all` (or names) the first of each building close up.
- `cargo test --profile iter -p voxmar_worldgen house_catalogue -- --ignored --nocapture`
  draws every house, roof, extra, prop and centrepiece on flat grass, fronts and backs.
