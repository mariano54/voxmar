# Settlements

Hamlets and villages are planned by worldgen (`crates/worldgen/src/settlements/`) from
`assets/content/<package>/settlements.ron`. Everything a settlement is made of is a list
in that file; the code only rolls from the lists and draws what it rolled.

## Where they stand

The world is cut into squares (`grid.spacing`, 1024 blocks). Each square has a
settlement with chance `grid.chance` (0.33), kept `grid.margin` blocks inside its edges so
it never reaches a neighbour's square. Within the square the planner scores spots every
32 blocks for flat, firm, dry ground near fresh water, and takes the best that:

- is not in a biome with an `avoid` tag (`no_settlement`, oceans, rivers, peaks, ice,
  fantastical and giant biomes);
- has a temperature between −8 and 34;
- has no sinkhole or ravine within 150 blocks.

About half the squares that roll a settlement find no such spot, so about one square in
six has one.

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
  always have (well; trader, inn and smithy in villages) and places they sometimes have
  (stalls, fields, pens, orchards, woodyards, shrines).
- `layouts`: spokes, street, green, scatter, fork.
- `houses`: cottage, long house, town house, farmhouse, hut, tower house, hall, wing
  house, each with its own size ranges, storeys and leans.
- `extras`: chimney, porch, lean-to shed, flower boxes, garden, hedge, lamp, woodpile,
  moss, wing.
- `props`: woodpile, crates, stone pile, flower bed, herb planter, campfire, lamp, bush,
  compost, boulder.
- `styles`: the building materials of a place by biome tag (sandstone, red sandstone,
  mudbrick, stilts, log cabin, stone cottage, blossom house, birch cottage, maple lodge,
  jungle stilts, turf house, villa, and timber frame anywhere else). A style names its
  woods and a weighted list per part (foundation, floor, walls, frame, roof, trim,
  paving, hedge, weathered, crops); `{wood}` and `{trim}` stand for the two woods the
  settlement picks. A settlement keeps one to three entries of each list, fewer where it
  is orderly. Where two styles fit a biome, either may be built.
- `names`: name parts by biome tag, each with its Japanese reading, small words for
  hamlets ("End", "Cross"), and prefixes that lean on the levers ("Old", "Great").

## What else knows about them

- Trails join settlements' gates (the ends of their main lanes) to each other and to
  landmarks within reach.
- Trees and decorations keep out of buildings, lanes and a glade round the houses.
- `Overworld::settlements_near` lists them with their buildings, doors and gates, for
  gameplay (villagers, shops) and maps; `/locate settlement` finds the nearest.
