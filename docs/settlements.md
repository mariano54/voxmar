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
    chapels stand in villages.
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
  for the two woods the settlement picks. Crops are everyday ones: a size's fields grow
  only crops up to its `crop_level` (hamlets 5: wheat and carrot; villages 12: potato
  too), by weight, and always growing, never ripe; loading fails on a crop rarer than
  uncommon, above every size's level, or a style with nothing a hamlet may grow. A settlement keeps one to three entries of
  each list, fewer where it is orderly. Where two styles fit a biome, either may be
  built.
- `names`: name parts by biome tag, each with its Japanese reading, small words for
  hamlets ("End", "Cross"), and prefixes that lean on the levers ("Old", "Great"), with
  how often each is used (`small_chance`, `prefix_chance`).
- `site`: the temperatures, heights and relief a settlement stands on, how far it keeps
  from sinkholes and ravines, and the glade it opens in the woods.
- `furnishings`: the blocks every settlement uses whatever its style (torch, campfire,
  table, farmland, water, dirt, planter, anvil, barrel, bell, altar) and the lists of
  flowers, bushes, stall canopies, compost, boulders, orchard fruit (in blossom or
  unripe, never the ripe leaves), hay, things hung to
  dry, sacks and cloth. Hay, sacks and drying herbs stand in with terracotta and hanging
  roots until the pack has blocks of their own.

A package adds to every list, and its `grid`, `site` and each furnishing it names replace
earlier ones, so a mod can add a style, a name culture, a house type or a prop without
code.

## What else knows about them

- Trails join settlements' gates (the ends of their main lanes) to each other and to
  landmarks within reach.
- Trees and decorations keep out of buildings, lanes and a glade round the houses.
- `Overworld::settlements_near` lists them with their buildings (each with what it is
  in the lists and the work done in it), doors and gates, for gameplay (villagers,
  shops) and maps; `/locate settlement` finds the nearest.

## Looking at them without the game

- `cargo test --profile iter -p voxmar_worldgen settlement_preview -- --ignored --nocapture`
  draws the settlements near spawn (`SEEDS`, `RADIUS`, `COUNT`, `ONLY`), and with
  `FOCUS=all` (or names) the first of each building close up.
- `cargo test --profile iter -p voxmar_worldgen house_catalogue -- --ignored --nocapture`
  draws every house, roof, extra, prop and centrepiece on flat grass, fronts and backs.
