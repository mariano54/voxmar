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

**Towns.** A third size, `town` (wiki: 20 to 60 buildings with walls, a temple and shops),
is drawn where the place is wealthy, trading, orderly and crowded: 16 to 30 homes, 5 to 8
working buildings, a temple (a chapel grown tall and long, `temple`), three to five market
stalls round a wider square, broad paved streets,
and 3 to 6 guards. It is rare: about one settlement in fifteen. Where a town's ground will
not take that many houses it is laid out as its `smaller` size (a village) with the same
draws, so a good site is never lost to a town that does not fit.

**Walls.** A size's `walls` is the chance it is walled and how that leans (towns 0.85,
more where martial and orderly; villages 0.12, mostly martial ones; hamlets never), drawn
from a stream of its own so an unwalled place is laid out as it would be anyway. The wall
(`rules.walls`, `settlements/walls.rs`) runs `margin` (4) blocks out round the convex ring
of the square and its buildings, leaving out the fields, pens, orchards, woodyards and
shrines out at the ends of the lanes. It is a palisade of the place's logs (3 high) where
it is poor, and of its foundation stone (4 high, a merlon on every other block) past
`stone` (0.5) wealth. A gate is left wherever a lane passes, with a post either side and a
lintel over it with three blocks clear where the wall is high enough; a tower three
blocks square stands at each corner and every `towers.0` (24) blocks along it, `towers.1`
(2) higher. The wall stands on the ground of each column, leaves out any column in water
or against a building, and no tree or prop stands in or against it. Each lane leading out
runs on to 17 blocks past the wall (straight on, else turned up to 90°, never over a lot);
one that cannot no longer leads out, while another does. Trails treat the wall as built,
and no trail's sight stands on or beside anything built, so a trail comes in at a gate and
never over the wall. Plans report each
column and its height (`walls`, `wall_block`) for maps and the gate.

A new player finds one within `rules.spawn_settlement_within` (1024) blocks of where they
spawn: where no square near is settled within that reach anyway, the nearest square in
reach that can hold one always does, its climate ruling no spot out and its next best
spot tried where one cannot be laid out: it looks at `rules.spawn_tries` (8) times as many
spots, and lays each out with `rules.spawn_salts` (8) draws, then as the smallest place.
Where no spot in reach will do (a strip of coast under mountains), none is promised. A
player never spawns inside a settlement.

No two settlements within `rules.names_apart` (4096) blocks of each other share a name
(design, draft 81). Complete Japanese readings belong to periodic groups of grid
squares; squares in the same group are farther apart than that reach. A square draws
from its group's names, preferring offers its neighbours have not also offered, then
further draws and the complete list. This needs no neighbour planned to know its own.
Loading requires every family to cover every group and checks that equal final English
or Japanese names, including prefixes and echoed parts, belong to the same group.

## The levers

Every settlement draws five levers, each 0 to 1, and every list entry may lean on them:
an entry's weight is multiplied by `1 + lean × (2 × lever − 1)` for each lever it names.
A `lean` may also name one of the eight culture axes below (piety, martial, trade, craft,
festivity, outlook, hierarchy, lore), which are drawn the same way, 0 to 1.
The levers are correlated, so a place reads as a whole:

| Lever | Drawn from | Leads to |
|---|---|---|
| wealth | luck, fertile soil, fresh water, flat ground | villages over hamlets, lamps along lanes, paved lanes and squares, town houses and towers, flower boxes, "Great …" |
| age | luck (two rolls averaged) | weathered and mossy blocks, moss on walls, "Old …" or "New …" |
| order | luck, a little wealth | straight streets and greens over scatters, straighter lanes, fewer materials (houses of a piece), "Long …" |
| crowding | luck, a little wealth | more houses, closer together, "Little …" where low |
| green | forest and plants around | greens, gardens, hedges, orchards, more yard props, more trees left in the glade |

## Cultures

Who built a place is drawn from `assets/content/<package>/cultures.ron`
(`crates/worldgen/src/settlements/culture.rs`; design wiki draft 108, P57a). The base package
has 17 families: hearth folk (the old default), fen folk, glen clans, skald folk, Sabari,
canyon folk (ochre), kraal folk, Lani islanders, Hanami folk, Sunvale, steppe riders,
birchwood folk, Hold folk, cloud monks, river folk, verdant temple folk and the Old Empire.
The six name lists that used to live in settlements.ron are the names of hearth, fen, glen,
skald, Sabari and Lani.

- **Region and family.** The world is cut into Voronoi cells on a jittered 6144-block grid
  (`rules.field`). Each cell names one family: the one most of its 3 by 3 sample points suit
  by biome tag (fully, or at half weight for a family's `half` tags) and mean temperature,
  times the family's weight and a roll of luck per cell and family. So neighbours share a
  family (a test holds 70% of pairs 3 km apart to it), one biome holds different peoples in
  different parts of the world, and a cell no family suits falls back on the first that can
  be given out. Within 512 blocks of a cell's edge a place also takes a **second family**,
  the next cell's, with a share of 0.1 to 0.45 (more nearer the edge): every list weight
  becomes main × (1 − share) + second × share, so border towns borrow roofs and props.
- **Site families.** River folk claim a place by its site, not its region: fresh water near,
  trade drawn at 0.55 or more, and a 35% roll. The region's family becomes the second
  family, share 0.3. The Old Empire (`claim: City`) is for cities and capitals only (P57c).
- **Axes.** Eight, each 0 to 1: *piety* (temples, bells), *martial* (walls, towers, guards),
  *trade* (markets, stalls), *craft* (workshops, signs), *festivity* (bunting, lanterns),
  *outlook* (open or closed to strangers), *hierarchy* (rich and poor, a keep) and *lore*
  (libraries, ruins). A place's value is its family's mean, plus a regional drift (a 2048-block
  noise, ±0.12, so a valley leans together), local noise (±0.1) and shifts from its site:
  piety + age × 0.15 (+0.1 at level 7 and up); martial + 0.06 per level above 3; trade + 0.15
  by fresh water; craft + forest × 0.05; festivity + green × 0.1 − martial × 0.1; outlook +
  trade × 0.3 − martial × 0.2 + 0.1 by water; hierarchy + wealth × 0.15; lore + age × 0.1 +
  order × 0.1; and towns and cities shift martial, trade and hierarchy (`rules.size_shift`,
  P57b and c). The wiki also draws trade from trail count and craft from ore veins near;
  neither is read yet. Biome levels use the site's home biome (P15), so rivers and border
  dithering do not change the level. Site shifts are data in `rules.site_shift`.
- **Tilt.** A family multiplies the weights of styles, houses, roofs, props, centrepieces,
  layouts and extras by name (`houses: {"tower_house": 3.5}`; 0 removes an entry; an entry
  it does not name keeps its weight). The lists in settlements.ron still hold everything: a glen hamlet
  in a forest still builds with what grows there, only more towers and cairns.
- **Palette.** Eight colours as blocks (walls, trim and roofs; red and white terracotta for
  the steppe, cherry trim for Hanami). A place keeps three to five and adds them to its style's,
  as `rules.palette` (0.5) times the style's own weight.
- **Names.** Each family has its own first and last parts, small words and prefixes, in
  English and Japanese. Kana families join the Japanese by the `kana` table in settlements.ron
  (a test checks every first and last part, and that no kana is doubled); Hanami's are kanji
  and run together (Sakura + gawa is 桜川, a hamlet 松宿, a prefix 古桜川).
- **Mutation.** No two places of a family are alike. Each drops one to three of the family's
  favourite entries (fewer where order is high), borrows nought to two favourites from the
  second family or a random one (about outlook × trade × 3 of them), keeps its colours and has
  one of 40 **quirks** (one place in four a second): a lean on levers and axes for how likely
  it is, what it needs of the ground (relief, water), a shift of the axes and a small tilt of
  its own. A family also favours some quirks. Several quirks (blue doors, cats, kites, twin
  wells) show fully only with P57b and P58; until then their tilt only leans the lists.

`/locate settlement` names the culture ("a village in Highland country", ja「…の村…」), the
settlement preview prints the family, second family, quirks, axes and the entries dropped and
borrowed (`ONLY=glen` draws just that family), and `Overworld::settlements_near` carries it
(`SettlementInfo::culture`). `SettlementInfo::levers` carries both the five levers and
the eight axes, so people and shops can use the same lean keys as planning. Culture rolls
use streams of their own; adding culture draws does not consume building draws.

Region fits read home biomes and regional temperature from the smooth climate fields;
they do not generate rivers at their distant probes. The settlement's own site still
supplies its water, slopes, vegetation and biome level. These fits share a 1 MiB cache
weighed by owned bytes, including mod families;
eviction changes only how often the terrain is sampled. Culture storage also counts in
the settlement plan's 32 MiB cache, along with allocated vector capacities, names,
building strings, room cells, residents and their species strings, lane polylines and
the voxel table. The ignored release
`culture_planning_budget` checks warm planning over 20 seeds, requiring both hamlets and
villages, at twice the 3/6 ms CPU budgets and at most 2 MiB of retained plan storage.
Final voxel sorting groups independent columns first, avoiding a whole-canvas vector
and stable merge storage. Positions are unique within each canvas map, and the
ground-relative flag separates their keys, so each column retains the same exact order.
Within one planning call, terrain samples share relief corners and channel groups for
their exact region bounds, including the neighbour halo used to seal water. Layout
retries reuse those original land samples. Rectangle checks prefetch only missing
centres in row chunks of at most 32, then retain every original row-major water,
river, solid-ground and height check; unused lookahead still belongs to the full timer.
Water-direction rings batch all 24 original samples, slopes batch their four original
cardinal samples, and bank searches batch ring centres in groups of at most 32. Those
decisions already consume every centre before stopping; cardinal bank checks retain
their original short-circuit order. No extra terrain or altered RNG is needed.
Lane walking likewise groups the at most five turns whose pure bounds/avoidance
checks permit a terrain read, then keeps the original best-cost and tie decisions.
The temporary terrain memo caps samples at 32,768 so spawn layout retries can reuse
the same land without the observed 16,384-point eviction. Water-query samples remain
bounded to 16,384. Relief evaluation allocates noise state only for missing corners;
house weights and extra chances are computed once per layout. Each request's ordered
wet and dry house choices are reused until a house is counted, which invalidates every
choice list. Zero weights and all random draws retain their original order. Interiors
are copied only after a lot passes placement checks, before stair/chimney adaptation.
A rejected house gives its extras-vector allocation back to the layout. The next roll
clears it before the same ordered extra draws; a placed house keeps its own allocation.
Water is searched for a house request
only when an otherwise eligible water-dependent house can serve it. Scratch caps
corners at 8,192, and channel groups at 16; eviction repeats
the same pure computation. None of this scratch storage is retained in the plan cache.
Sealing also reuses up to 32,768 raw columns keyed by position and the requesting
centre's exact channel-region bounds. Its four neighbours keep those same bounds;
corner-only eviction preserves the raw values, and changing worlds clears all scratch.
The complete bounds are interned to IDs in a separate registry capped at 32,768 keys.
Tier-group eviction preserves those IDs; registry or raw-column pressure resets both
maps together before the whole batch. Raw-column lookups hash only position and ID.
The larger corner bound avoids observed close-site relief eviction; the raw-column
bound avoids eviction just before the successful ninth layout of a spawn village.
Once actual next-batch work fills a large terrain or raw-column table, allocation
skips one intermediate rehash. Small maps keep ordinary growth; entry limits, reset
rules and the maximum allocation of an ordinary full map are unchanged. Allocation
and copying remain inside the full planning timer.
The close-water memo remains bounded to 16,384 points.
Freshwater-only queries first check the original channel indexes. Without an indexed
edge or a home lake/shore/salt cell in either tier, carving cannot make freshwater at
any geology height. That exact false answer is stored separately from full terrain;
both witness stores share the same cap and reset together. Possible water still uses
the full original sampler. Physical-read diagnostics count only those actual samples.
House-water checks use the same proof at their original eight rounded spokes. Their
false witnesses share the 32,768-point terrain memo bound and reset with it; a full
sample always takes precedence and removes its former false witness. Possible water
keeps the original terrain predicate and spoke short-circuit order.
Batch sealing gathers the same missing cardinal neighbours in first-use order, under
each requesting centre's exact key, and shares their original geology evaluation. Its
work vectors and membership set reuse only their allocated capacity within the planner;
all values are cleared between batches, bounded to 32 centres and 128 neighbours.
On ARM with NEON enabled, each immutable edge prepares the four segment origins,
directions and original squared-length denominators once, counted in the existing
channel cache byte budget. Its projections and distances then use four lanes with the
same unfused arithmetic, followed by the original ordered strict minimum. Bounded
finite coordinates keep every intermediate finite; unusual values and other
architectures retain scalar evaluation. The original curve geometry is unchanged.
Climate, biome, home level and fertility checks reuse the same full original site at
each position, rather than sampling its five-block terrain neighbourhood repeatedly.
Site water checks walk the same ranked qualifying prefix lazily, stopping when enough
usable sites have been found. A close freshwater query consumes the exact grid's circle
row by row, grouping at most 32 centre samples and stopping after a matching batch.
Each centre keeps the original neighbour halo and sealing; the corner memo reserves up
to twenty corners per centre before the batch begins. At most 31 later centres in the
matching batch are computed, all within the whole planning interval. Synthetic land
retains its full-grid fallback. Overlapping close queries share a separate site-local
memo of up to 16,384 exact centre samples; its own world identity resets it before any
lookup in a different world. The real river land seeds it with the already sampled
coarse site grid's exact sealed values; synthetic land keeps its original fallback.
A known freshwater witness can finish the Boolean query
without reading earlier pending unknown points. Both memos store only original terrain,
never grading or occupancy. Lot-clearance checks stop at the first lane point that
rejects the lot, retaining the same strict and inclusive boundaries. Temporary lanes
share their exact interpolation samples, capped at 16,384 points per lane with the
original on-demand calculation above that cap. Bounds of each 16-point chunk use the
actual rounded samples; their componentwise lower distance can exclude only chunks
whose every point fails the same comparison. A bound over all chunks can reject a
whole distant lane first. Paving clips its row-major scan to the computed projection
extrema with conservative axis lower-distance checks over the original integer
coordinates; it keeps the exact distance, building and palette predicates. Plans
retain only the original polylines.
Layout RNG draws
for name prefixes stay in place; only the successful layout constructs its final name.
Neighbour-name collisions compare English names, so those offers draw the same parts
without constructing unused Japanese readings. Only families that can spell the literal
English candidate construct their original neighbour offers, cached on first use.
Selected names still join both languages.
Before attempting queued lots, a layout with an unsatisfiable mandatory catalogue
request is rejected: its eligible houses have no positive weight even before built
caps grow. Water is ignored in this necessary check. Successful layouts and their
draws are unchanged; every retry starts from its own original RNG stream.
Lane steps share their original seven interpolation samples across avoided rectangles.
Actual sample extrema provide a conservative signed-distance bound; a possible match
retains the original seven-point predicate, including inside distances and its MAX cap.
Allocator metadata is outside this storage count. Later packages replace only culture rule fields they
name, preserving the others and reporting which package and field they replaced. Loading
refuses unknown catalogue names, missing palette blocks, non-finite weights and shifts,
and unbounded mutation counts before a world can be created.

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
    a home may have a byre (`byre`: a `chance` and `rows`, fewest and most): its back
    rows behind a wall across from the room, through one doorway, on an earth floor,
    with hay by a stall's post, a trough let into the floor and a wide door out the back.
    Its people live in the rows in front (four at least), the hearth on the side wall
    there; only homes of one storey on the ground have one. Long houses mostly do (0.7),
    farmhouses now and then (0.35). Loading fails on a chance outside 0 to 1, fewer than
    3 rows, a byre on a working building, one not of four walls or with fittings, or a
    house too shallow for its rows and the home's. Each building's `byre` columns are
    kept for gameplay;
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
- **Shaped roofs.** A style's `roof_pieces` (a `slope` and its `ridge`, each a block that
  faces four ways, by weight) lay a share (`pieced`) of its steep gables over four walls
  or a shop front in pieces: each column's top a slope whose low edge faces the eave
  below it, a ridge cap along the peak where the span is odd. Thatch in stone-cottage,
  stilt, mudbrick and timber-frame places, shingles in log cabins, blossom houses and
  maple lodges (dark oak, the woods there making none), birch shingles or thatch in
  birch cottages; the rest stay whole blocks. Loading fails on a piece missing a facing
  in any of its style's woods, a slope not tagged `roof` or a ridge not tagged `ridge`.
- **Fire** (`rules.fire`, `settlements/roofs.rs`). No village stands ready to burn: once
  a place is drawn, each of its flames (tag `fire`: torches, campfires, braziers) walks
  out a face at a time through open cells, `reach` (4, fire.ron's most) steps, as fire
  catches in a home (docs/home.md). A street lamp or fire on the ground that reaches
  something that catches (`thatch`, `wattle`) goes, post and all; a thatch roof a
  building's own flame reaches is laid whole in `instead` (dark oak shingles), each piece
  turned as it was; a flame that still reaches is put out. In the base worlds about one
  street lamp in seventy goes and one thatch roof in ten turns to shingles. The gate's
  "settlement roofs" checks the town's pieces stand as planned and none of its flames
  reaches its thatch.
- **Jetties** (`jetty`: a `beam` and a `chance`). A two-storey box house with no porch,
  awning, balcony or dormer may carry its upper storeys one block out over the street,
  as old town houses do: the front wall above the ground floor steps forward, the floor
  runs out over it, and a row of jetty beams (tagged `jetty` and `shelter:`, so the house
  stays a home) holds the overhang up under its front edge. The roof covers the deeper
  top. The chance grows with crowding, so jetties gather in town streets: timber frame
  most (0.6), mudbrick towns (a cumba, on timber brought in), birch cottages, maple
  lodges and stone cottages a little. Loading fails on
  a beam missing a facing in any of its style's woods, or without its `jetty` and
  `shelter:` tags. The gate's "settlement jetties" checks, at the walled town it
  already visits for walls and roofs, that each beam stands with the house behind and
  above it.
- **Rows** (`row`: a `chance`, `houses` (a number drawn from them, fewer where the street
  runs out) and `storeys` by size). Along a straight street a four-walled home off stilts
  may start a terrace on `chance` (times the place's crowding): the homes wanted next are
  built wall to wall to its right, then its left, on the same floor, each the first's
  twin but for its seed (its people and what is inside), until the street bends, the
  ground falls, or a lane or lot is in the way. A row stands at least the storeys its
  place's size names (`[("town", 2)]`: town terraces are of town houses), the first raised
  with it. They share their materials and the pieces of their roofs, and whether they are
  jettied where their roofs slope down to the street (else each rolls its own); a row
  has no porch, side shed, outside stair, woodpile or barrel, no eave or side window
  where two meet, and its roofs run on over each join: along the street, gable fronts
  side by side, or flat (any other roof is laid as a gable). The gate's "settlement
  rows" checks, at the walled town, that each join stands as two solid walls under one
  roof height.
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
- names (now each culture family's, in cultures.ron): first and last parts, each with its
  Japanese reading, small words for hamlets ("End", "Cross"), and prefixes that lean on the
  levers or axes ("Old", "Great"), with how often each is used (`small_chance`,
  `prefix_chance`).
- `kana`: how the Japanese parts join where the English runs a consonant into a vowel:
  Sab + ir is Sabir, so サブ + イル is サビル, not サブイル (Glenach グレナック, Oasis オアシス).
  A consonant kana repeated at the join is written once: Oak + combe is オークーム;
  vowel and long-mark sequences stay as written.
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
player cannot replace the first cooldown. These outlive forgetting a town, so leaving
and rejoining within the same running server cannot reset them. Cooldown and injury
recovery reset when that server restarts; earned XP and death/newcomer records persist.
A disappearance without a death releases the resident without starting a newcomer
clock. All outdoor errands, including square visits and patrol stops, use ground spots
outside wells, centrepieces and outdoor shrines. Indoor spots use the planned room
shape, including connected wings, with feet one block
above the solid floor. Live block checks exclude furnishings and low headroom;
porches, courtyard ground and exterior door steps are not indoor destinations.

They react to what is about them (`react` in settlements.ron). A player within `greet`
blocks of someone out on their day is greeted: they stop where they stand for `greet_for`
seconds, face the player and say so, then greet that player again only after
`greet_every`. Anything hostile (a monster, or a creature that hunts, not kept by a
player) within `fear` blocks sends everyone but the guards home with an alarm, and they
stay in for `afraid` seconds; a villager struck cries out once in the same span. When
it rains harder than `rain`, people bound for the `shelter` places (the square, the well,
a yard, a neighbour's door) or working an `outdoor` work (field, orchard, pen, woodyard)
go home instead; guards keep their rounds. Two neighbours out and about (no guards, no
one afraid or greeting a player) within `chat` blocks who see each other stop and talk:
both stand turned to each other for `chat_for` seconds with a contented hum, then go on
with their day, and stop to talk again only after `chat_every`. Reaching their bed for
the night, they sigh content once a night. A guard sent after a player who struck a
villager (or killed one, in one blow or many) knows who they are after, whatever stands
between them.

Field tending stays deferred until P54 theft and crime rules: unsown village fields stay
unripe in P50b so a village is not a free food source. Settlement witnesses and hunter
roles remain follow-ups. Single-valued people data and same-named routines follow the
package layering rule, with load notes naming each replaced field and its earlier setter.

## Homes inside

A home is furnished for the people drawn to live in it (`settlements/interior.rs`,
`interiors` in settlements.ron), so a look through the door tells you who they are: a bed
for each, upstairs first, its head to a wall and away from the door; a table with a seat
facing it for each of them and a guest; cupboards, shelves and crockery against the walls,
more of them where the place is wealthy and the home roomy; and a corner of what they do
for a living (sacks and baskets for a farmer, nets and barrels for a fisher, a loom for a
weaver, books for an elder or a priest, a quern for a miller). Cloth and crockery take one
dye and one glaze per home, drawn leaning on the levers and the culture's axes like any
list, so a rich martial hold hangs banners and a festive one paper lanterns and cushions.
Pieces never stand in the doorway or at the foot of a stair, and none cuts a part of a
floor off from the door: the floor is walked from the door (and from each stairwell
upstairs) before every solid piece is set. A home with a lit hearth needs no stove; one
without may get one. Only plain homes are furnished: inns, temples and workshops keep their
own insides.

Every piece is a furnishing (P58a), so breaking it in a village gives nothing, and a bed in
someone's home will not take as yours: sleeping in a bed where the plan put one is refused
(`bed.someones`). Any other bed works, one you place in its stead included. Each building reports where its beds and seats stand
(`Building::beds`, `seats`), for villagers to sleep and sit at their own.

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
- They are heard (ambience.ron, the client's ambience). The ambience measures the nearest
  one from the same plan: how far its middle is (`settlement`), how far outside its bounds
  the player is (`settlement_edge`: the ground its buildings and square stand on and six
  blocks round, 0 within), and how many live there (`settlement_people`). By day its
  people's talk (`murmur`, fuller the more live there, fewer out in the rain) and its work
  (hammering, sawing, hens, the well bucket at the well, the smith at the smithy, sheep at
  the pens and barns); at dusk low talk, doors at the houses and supper things; at night a
  dog now and then and an owl. A one-shot names the buildings it comes from by kind, house
  or work (`at: Building(buildings: [...], door: true)`), checked against settlements.ron as
  the game starts. The first time each day a player steps within a settlement's bounds, the
  settlement motif plays (music.ron's `settlement` category, on the home motif's clock, and
  never over a motif playing).

## Looking at them without the game

- `cargo test --profile iter -p voxmar_worldgen settlement_preview -- --ignored --nocapture`
  draws the settlements near spawn (`SEEDS`, `RADIUS`, `COUNT`, `ONLY`), and with
  `FOCUS=all` (or names) the first of each building close up.
- `cargo test --profile iter -p voxmar_worldgen house_catalogue -- --ignored --nocapture`
  draws every house, roof, extra, prop and centrepiece on flat grass, fronts and backs.
