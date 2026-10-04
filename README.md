# voxmar

A moddable, open source voxel sandbox with an infinite procedurally generated world.
Rust, Bevy 0.19, wgpu (Metal on macOS, Vulkan/DX12 elsewhere).

## Running

```bash
cargo run --release                         # title screen: singleplayer, multiplayer, settings
cargo run --release -- --world NAME         # play (or create) the saved world NAME right away
cargo run --release -- --connect HOST:PORT  # join a server right away
cargo run --release -- --host               # also open singleplayer worlds on the LAN
cargo run --release -p voxmar_server        # dedicated server on port 25665, world in ./world
```

While developing, use `--profile iter` in place of `--release` (for `run`, `build` and `test`):
it rebuilds in seconds rather than minutes after an edit and runs nearly as fast. Keep
`--release` for performance measurements.

Client flags: `--seed N` (for worlds `--world` creates, and captures), `--name NAME`,
`--view-distance N` (columns of 32 blocks, default 16), `--fov DEGREES`, `--no-vsync` (uncapped
frame rate), `--weather KIND[:INTENSITY]` (show a kind of weather from `weather.ron`, such as
`storm` or `rain:0.3`, instead of the server's), `--command LINE` (run a console line once in game, such as
`--command "/time set noon"`; repeatable, and handy with captures), and
`--capture out.png [--pose dx,dy,dz,yaw,pitch | --flight SECONDS] [--time FRACTION] [--mine PROGRESS] [--menu | --screen title|worlds|create|join|settings]`,
which runs headless, waits for the world (an unsaved one unless `--world` or `--connect` is
given) or the menu backdrop to load, logs frame time and saves a screenshot; `--time` holds the
time of day (a fraction of the day, 0 at sunrise), and `--mine` mines the block in the middle of
the view that far (0..1, cracks and chips showing; 1 breaks it just before the screenshot).

The title screen, over a slowly turning view of a demo world, leads to Singleplayer (saved
worlds, most recently played first, to play, create or delete), Multiplayer (join by address;
the last few servers are remembered) and Settings. A new world takes a name, an optional seed
(a number, or any text, which is hashed; empty means random) and a game mode: Creative allows
flight, Survival does not. Either way you are the world's admin, who can run commands.

Controls: WASD move, Space jump, Ctrl sprint, Shift descend, F toggle flight, left mouse break
(hold), right mouse place, 1-9 or wheel select hotbar slot, E inventory (click a slot, then
another to move it), T chat, / command, F3 debug overlay, Esc menu, R respawn, C (hold) zoom in
creative: the wheel changes the magnification while zoomed, and mouse look slows to match.

### Console and commands

T opens the console to chat with everyone on the server; / opens it with a command. Up and down
recall earlier lines, Tab completes a command's name, Enter sends and Esc closes. Replies and
chat show above the hotbar and fade after ten seconds. The server checks and runs commands,
like actions: admins (the singleplayer host, and players named with `--admin` on a dedicated
server) may use them all, everyone else `/help`, `/seed`, `/biome` and `/list`.

- `/time` tells the time; `/time set sunrise|day|morning|noon|afternoon|sunset|dusk|night|midnight|dawn|TICKS`
  moves forward to that time of day (a day is 24000 ticks, 0 is sunrise), `/time add 100|30s|2h|1d`
  moves it on, `/time speed 10` runs days ten times as fast, `/time freeze` stops the sun and
  `/time resume` lets it run normally. `/day` and `/night` are shortcuts. The time of day and its
  speed are saved with the world; the weather keeps its own pace.
- `/weather` describes the weather where you are. `/weather KIND [INTENSITY] [SECONDS]` brings
  that kind (`/weather list`) to the land within about a kilometre of you, as intense as you say
  (`light`, `moderate`, `heavy`, `max` or 0 to 1; each kind has its usual intensity, and 1 is its
  fiercest), for five minutes unless you say; then the natural weather takes over again, slowly.
  `/weather natural` ends it at once.
- `/wind` describes the wind; `/wind calm|breeze|strong|gale [FROM] [SECONDS]` brings that
  wind (from a compass point, `n` to `nw`, or degrees from north) on top of any forced
  weather.
- `/tp X Y Z`, `/tp X Z` (onto the ground), `/tp PLAYER`, `/tp PLAYER X Y Z`, `/tp PLAYER PLAYER`;
  `~` means where the player is, so `/tp ~ ~10 ~` goes ten blocks up.
- `/gamemode survival|creative [PLAYER]` switches a player's mode, which is kept with them.
- `/setblock X Y Z BLOCK` puts a block somewhere (`~` again means where you are), and
  `/fill X1 Y1 Z1 X2 Y2 Z2 BLOCK` fills a box (at most 32768 blocks and 128 across; `air`
  empties it). Both load the columns they touch first. `/give BLOCK [COUNT]` hands you blocks
  (64 unless you say), so `/give water` lets you pour water. Block names may leave out the
  package (`water` for `base:water`).
- `/biome` names the biome you stand in; `/locate biome NAME [RADIUS]` finds the nearest one
  (within 4000 blocks unless you say).
- `/seed`, `/list` (who is online), `/help [COMMAND]`.

Commands are data: each package's `commands.ron` names its commands, who may use them, their
usage and help, and what they run: one of the engine's built-in commands, or other command
lines with the arguments filled in (`$1`, `$*`), which is how a package adds its own (`/day`
is `time set day`).

Esc opens the settings menu: render distance, field of view, anti-aliasing, particles (All,
Fewer or Off: the pieces that fly from broken blocks and the chips while mining), how far the hotbar
sits above the bottom edge (Raise HUD), VSync, debug overlay, music, sound and ambience volume,
mouse sensitivity, the zoom key (click it, then press a key), zoom magnification, cinematic zoom
(the view follows the mouse smoothly while zoomed), zoom in survival, and fly speed for admins,
with Resume, Save and quit to title (Disconnect on a server) and Quit game. Settings are saved
to `voxmar/settings.ron` in the OS config directory (`~/Library/Application Support` on macOS,
`$XDG_CONFIG_HOME` or `~/.config` on Linux, `%APPDATA%` on Windows; `$VOXMAR_CONFIG` replaces
`…/voxmar`); `--view-distance` and `--no-vsync` override them. On a dedicated server, name
admins with `--admin NAME` (repeatable), and cap render distance with `--view-distance N`
(default 32).

### Saved worlds

Singleplayer worlds live in `voxmar/worlds/<name>/` next to the settings:

- `world.ron`: name, seed, game mode, created and last played times, the generator id and
  version that made it, and the time of day.
- `columns/<x>.<z>.bin`: each column a player has edited, as zlib-compressed postcard with the
  names of the blocks it uses, so a content change that renumbers blocks does not break saves.
  Untouched columns are not stored; generation recreates them exactly.
- `players/<name>.ron`: each player's position, health and inventory, restored when they join.

The server saves edited columns, players and the time every 30 seconds, when a player leaves
and when it stops (leaving to the title screen or quitting). Every file is written to a
temporary file and renamed, so a crash never leaves half a file. Saved columns are unloaded
like any other. The dedicated server keeps its world in `--world DIR` (default `./world`),
created with `--seed N` and, given `--no-flight`, in survival mode if it does not exist yet;
type `stop` to save and quit. Anything else typed there runs as a command, as an admin
standing at spawn (`time set noon`, `weather storm 600`, `gamemode creative Ada`), and
`say …` tells everyone.

### Textures

Textures come from a stack of packs, each a folder, zip or jar; every texture is taken from the
first pack that has it:

1. the pack chosen with `VOXMAR_TEXTURES` (a resource pack, a Minecraft jar, or
   `assets/packs/default` to see only ours), which overrides everything below;
2. otherwise, the newest Minecraft client jar found (official launcher or Prism Launcher), for
   the familiar look while our pack matures;
3. our default pack, `assets/packs/default`, which draws every texture the game uses: whatever
   Minecraft lacks (our stones, woods, plants and wonders) and, without Minecraft, everything;
4. any other pack in `assets/packs`, in name order: mods ship art this way, adding textures but
   never replacing ours.

Settings → Textures switches to "Voxmar", which puts our pack first (the Minecraft jar then only
fills names ours lacks); it takes effect the next time the game starts.
Anything still missing gets a generated placeholder. The startup log says how many textures
each pack provided. A pack keeps images at `textures/<path>.png` (`textures/block/stone.png`),
or at `assets/minecraft/textures/<path>.png` as Minecraft and its resource packs do. No
Minecraft assets are shipped.

A pack can also describe textures as text, in recipe books (`recipes/*.ron`), which the game
renders as it loads, so a pack or mod can ship its art with no image files and no build step.
A recipe is a texture program (palettes, noise, patterns such as cells, bricks, boards and
rings, edge shading, highlights and cut-outs) or a text sprite (a grid of characters, each a
colour or palette shade); recipes derive from each other, and templates make a whole family
(a wood's log, planks, leaves and sapling) from one entry. See the `voxmar_texgen` crate docs.
A PNG beats a recipe of the same name. The `texgen` tool helps author them:

```bash
cargo run --release -p voxmar_texgen -- check                  # render every recipe, report errors
cargo run --release -p voxmar_texgen -- sheet --only stone,ore --compare PACK_OR_JAR
cargo run --release -p voxmar_texgen -- export --out DIR       # write every recipe as a PNG
```

`sheet` writes `target/texgen/sheet.png`: each texture large, tiled 3×3 and at one pixel per
texel, optionally beside the same name from another pack (`--pack DIR` picks the pack,
`--columns`, `--zoom` and `--out` the layout).

Code and assets adapted from other open source projects are listed, with their licences, in
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

## Layout

| crate | role |
| --- | --- |
| `engine` | world data and rules, no renderer: palette chunks, column lighting, binary greedy meshing, visibility graph, collision, raycast, content packages, actions, scheduler, traits, fields |
| `worldgen` | staged generator: world dials → geology, rivers → climate fields → ecology → resources → terrain, caves, trees, plants |
| `protocol` | client/server messages and transports (in-process channels, TCP) |
| `server` | authoritative simulation (fixed 20 Hz tick), validation, streaming, world saves; `voxmar-server` binary |
| `client` | Bevy app: rendering, input, prediction, HUD, menus; the only crate that depends on Bevy |
| `texgen` | texture recipes and text sprites rendered to pixel art, and the `texgen` review tool |

Content is data: blocks are defined in `assets/content/base/blocks.ron` and loaded through the
same registry a mod will use. Client and server compare a content fingerprint on join. Templates
make whole families from one line (a wood: log, stripped log, wood, planks, leaves, sapling; a
rock: raw, cobbled, polished, bricks; a flower in three looks), and templates are shared across
packages, so a mod's new wood is one line. Blocks carry tags from a shared vocabulary (listed at
the top of `blocks.ron`: what a block is, where it grows, what it is good for, such as `meadow`,
`edible`, `alchemy:calming`, `dye:blue`, `wonder`), a rarity and an optional inventory icon, so
biomes, food and alchemy select blocks without naming them. A plant's `variants` are other looks
of it, each its own block that drops the plant itself, as many as its size;
`BlockRegistry::looks` lists them for generators. A block's `support` says what it rests on:
plants stand on solid ground (the default for plants), hanging ones (`support: Above`: glow
berries, hanging roots, stalactites) need a solid block above; either can only be placed with
that support, and goes, with the column resting on it, when the support is broken. The inventory shows plants as their sprite and
full blocks as a shaded cube. Templates
make whole families from one line (a wood's log, stripped log, wood, planks, leaves and sapling;
a rock's raw, cobbled, polished and brick blocks), and templates are shared across packages, so
a mod's new wood is one line too. A plant can list variants, other looks of itself that become
their own blocks and drop the plant, as many as their size. Blocks carry tags (`flower`,
`wetland`, `edible`, `alchemy:calming`, `dye:blue`; the vocabulary is at the top of
`blocks.ron`) and a rarity, so biomes, food and alchemy select them without naming them.

How the design primitives map to the code today:

- **Definition**: blocks (`content/base/blocks.ron`), actions (`content/core/actions.ron`),
  console commands (`content/core/commands.ron`) and kinds of weather (`content/base/weather.ron`),
  namespaced per package (`base:stone`, `core:mine`; `core:air` is built in).
- **Action**: the only way discrete changes happen. Every action goes through one lifecycle on
  the server (propose, validate, schedule one completion, re-validate, commit or cancel) and
  compiles down to a fixed set of engine effects. Clients validate with the same rules to
  predict. Mining, placing, moving items and respawning are all actions.
- **Process**: continuous server rules at a scheduling tier (`High` every tick, `Low` once a
  second): falling damage, regeneration, which heals slower where the temperature field
  is freezing, the weather (below), and footsteps, which wear paths (below).
- **Trait**: typed data with declared replication and persistence (`core:health`,
  `core:inventory`, `core:position`); owner-replicated traits are sent when they change.
- **Field**: coarse grids (16 blocks per cell unless a field chooses otherwise), generated
  baseline plus emitted differences: `core:temperature`, in °C, from the climate generator,
  and `core:humidity`, `core:clouds` and `core:precipitation` at 128 blocks, which the weather
  process evolves.
- **Agent**: players, whose controller is input. Machines and creatures come later.
- **Generator**: the `Generator` trait; `Overworld` is the first implementation. Random
  streams are seeded from world seed + generator id and version + stream + position, so
  generation is pure and order-independent. Distant terrain is sampled from the same functions.

## World generation

The world is 384 blocks tall (columns of 12 chunks) with the sea surface at 128: 128 blocks
of ground and caves below it, 256 of land, mountains and sky above. Each world draws a few
physically meaningful dials from its seed (`Overworld::dials`: mean temperature,
equator-to-pole temperature difference and distance, ocean fraction, continent size,
humidity, prevailing wind, tectonic activity, erosion age, river density, vegetation, fantasy,
volcanism, cave density, mountain height, biodiversity), with sensible correlations, and a
featured set: a few rare biomes and other things it boosts and common ones it suppresses, so
no two worlds share a rarity table. Everything else is a continuous field:

- **Temperature** falls towards the poles (latitude repeats along z, colder to the north, so
  hot, temperate and cold land always lie in that order) and with altitude.
- **Precipitation** follows moisture the prevailing wind carries in from the sea: wet coasts,
  drier interiors, rain shadows behind mountains, wet tropics and dry subtropics.
- **Rivers and lakes** (`worldgen/src/water/`) run where water would. Trunk rivers are routed on
  a 64-block grid over the land's broad shape and cut their valleys (and, rarely, tunnels
  through a ridge) into it; creeks and streams are routed on an 8-block grid from the hills to a
  trunk, a lake or the sea, joining as they go. Each river is as big as the rain on its whole
  catchment. Shallow pits are breached; deeper ones fill as lakes at their own height, lower
  and without outlet in dry climates, down to salt flats. Water steps down in whole blocks,
  falling as waterfalls onto plunge pools where the drop is three blocks or more. Flow is
  summed region by region through records of where water crosses each 1 km region's border,
  so rivers can be any length and nothing depends on generation order.
- **Trails** (`worldgen/src/trails/`, materials in `trails.ron`) join the places worth walking
  to: summits that stand above the land around, lake shores, the banks of waterfalls, the edges
  of long drops and flat trailheads in the valleys, a few per 512-block region. Nearby places
  are joined into a sparse network (a Gabriel graph, with no two trails setting off side by
  side), each trail routed by A* over the land with costs adapted from RoadWeaver: walking
  along the contour is cheaper, grades past soft and hard limits cost more, and so does the
  fall line, so trails traverse slopes and switch back up them. Trails that run within 18
  blocks of a longer one join it, making junctions. Each is drawn as a smooth curve cut into
  hillsides and raised on low banks, its tread worn to packed earth, gravel or mud by biome
  with trodden grass at its edges, across shallow water on stepping stones, across creeks on
  logs and across rivers on plank bridges with piers (crossings over 100 blocks are not built).
  Cairns mark trails above the treeline and stand on summits, old waymarkers stand at
  trailheads, and summits and viewpoints keep a clearing. Everything is a pure function of the
  seed and the land, planned region by region in caches bounded by bytes, so nothing depends
  on generation order. Coarse sampling (`Overworld::height`, `site`, `surface_sample`, used by
  distant terrain and the weather) leaves trails out; generated columns have them.
- **Caves** (`worldgen/src/caves.rs`) come in kinds of very different rarity: winding tunnels
  everywhere, more tangled with depth, opening to the surface as entrances in some places;
  cavern regions with pillared cheese caverns; thin noodle tunnels; rare multi-floor
  labyrinths, ravines (30–70 deep, some open to the sky, a few a kilometre long) and
  sinkholes; and very rare giant cavern systems, most holding one great lake. Water tables
  per 16×16×12 cell (as in Minecraft 1.18) flood some caves at their own levels, with sea
  level near the surface so caves under the sea are flooded rather than draining it, and lava
  below y 10, with rock left wherever a fluid would otherwise touch air. The cave density
  dial, the world's featured rarities and the seed set how much of each kind a world has
  (`Overworld::cave_style`).
- **Selection fields**: continentalness, a landform class (erosion), and seeded weirdness,
  aether (how fantastical the land is), fertility, variant and rock provinces 2-4 km across.

**Biomes** are data in content packages (`assets/content/base/biomes.ron`, with the climate
bands in `climate.ron` and rock provinces in `rocks.ron`), loaded like blocks, so a mod adds
one the same way. The temperature, moisture, continentalness and landform bands form a table
in which every cell belongs to some biome (checked at load); rarer biomes take the extremes of
the weirdness field, so they appear in whole regions of some worlds and not in others;
variants take shares of their parent; ecotones sit on borders; fantastical biomes claim cells
where aether runs high. Borders blend over 16-48 blocks. Each biome's ground comes from
ordered surface rules (slope, altitude, climate, rock province, noise patches and contour
lines such as game trails; shared sets in `surfaces.ron` break every soil slope up into
coarse dirt, gravel and rock) over the stones of its rock province, while snow, ice and bare
rock follow the climate everywhere. Trees (`trees.ron`: procedurally shaped giant spruce, pine,
birch, oak, dark oak, cherry, willow, jungle giants, acacia, palm, mangrove, shrubs, cacti)
and plants (`plants.ron`, with nutrition, alchemy and dye properties) are a shared pool:
each says what climate, ground and habitat it likes, biomes select from the pool by tag or
name, and the climate at each spot weighs the candidates, so a new plant with the right
tags grows in every biome that fits. Biomes, trees, plants and rock provinces carry rarity
tiers from common to legendary, and wonders (anything tagged `wonder`) grow where aether
runs high. Forests have structure: one field grades them from dense old-growth cores with the
giant trees through ordinary woodland to scrubby edges and gaps, and a global field opens
clearings with a soft edge. Ground decoration (`decorations.ron`, another shared pool biomes
select by tag) is placed in a fixed order of stages: patches of dirt, gravel and moss; boulders
and scattered stones of the rock province, mossy on their north side in damp woods; bushes of
the trees growing there; ornaments each tree makes of its own blocks (needle and leaf litter,
fallen leaves, fungi at the trunk's foot); fallen logs (some thick and hollow, with branches and
the stump they broke from) and stumps, each laid only where the ground holds it; then flowers
(sparse, a few kinds at a time, thicker at forest edges and along creeks), fungi and a carpet
of grasses and ferns over most open ground; and last a snow blanket on every level open to the
sky where snow lies, using the same snowy blocks as snow that falls in play. Each place also has an atmosphere (`Overworld::atmosphere_at`: haze, sky and
colour-grading nudges, and ambient particle sets from `ambient.ron` such as pollen,
fireflies, spores, dust, drifting snow and aether motes), taken from the climate unless a
biome sets parts of it, and blended across borders.

**Cave biomes** (`cave_biomes.ron`) are biomes of the `Underground` layer, placed by the same
picker in 3D: the climate above, plus a depth, height and rock province a claim may ask for,
and a weirdness field that also varies with depth, so cave biomes are wide regions stacked in
layers. Plain caves fill most of the underground; dripstone caves lie under dry land and in
karst, lush and fungal caves under wet land, frozen caves under cold land, molten caves near
the lava and under volcanic land, deep dark caves far down; painted (bands of fired clay) and
crystal caves are rare, and two fantasy caves, glowroot hollows and the starlit grotto, grow
only where aether runs high. Each lines its floors and ceilings through its surface rules,
replaces the rock around it with its own stones or strata, and grows decorations from floors
and ceilings (`cave_decoration`): plants and mushrooms, stalagmites and stalactites, icicles,
hanging roots and glowing berries, crystals and lichen, which light the caves; hanging growth
always hangs under rock. Frozen caves freeze their pools, and lush caves and glowroot hollows
have pools that glow from below. Wherever daylight falls straight down a shaft to a cave
floor, a small garden of turf, grass, ferns and flowers grows in the light. Buried in the rock
are geodes (hollows lined with crystal, calcite and basalt, glowing clusters on their floors)
and fossil skeletons, found by digging or where a cave cuts through them. Lava burns (4 health
every half second). `Overworld::cave_biome_at` gives the cave biome at a block.

`Overworld::site(x, z)` returns every parameter at a position for gameplay to use, including
its biome, the biome next door and the distance to their border; `Overworld::biome_at` and
`Overworld::locate_biome` serve commands.
`cargo test --release -p voxmar_worldgen map_preview -- --ignored --nocapture` (optionally with
`SEED=n` or `SEED=1,2,3`, `BIOMES=1` to colour by biome, `FIELD=erosion` and so on, `X`, `Z`
and `SCALE` to move and zoom) writes a shaded 8 km map to `target/map_preview.ppm`, with lakes,
rivers and creeks in their own blues, trails over them (brown, with fords cyan, bridges red,
and the places they join marked; `TRAILS=0` leaves them out), and prints each biome's share of
it. `trail_patch` (with `SEED`, `X`, `Z`, and `LINES=1` for trail centrelines) draws 160×160
generated blocks from above to `target/trail_patch.ppm`; `trail_sights` prints `--pose`
arguments for fords, bridges and switchbacks near spawn; `trail_stats` counts places, trails
and routes that failed. `river_sights` prints
`--pose` arguments for captures of a highland lake, a waterfall, a confluence and a river mouth
near spawn.
`cave_preview` (with `SEED`, and a centre `X`, `Z` and two heights `Y1`, `Y2` for level cuts)
writes an x-ray map, sections, level cuts and a cave biome map of the caves over 1.5 km to
`target/caves_*.ppm`; `cave_sights` prints `--pose` arguments for captures of caverns,
labyrinths, ravines, sinkholes, entrances, geodes, bare fossils and the best view of each cave
biome near spawn (or near `X`, `Z`, or the nearest surface biome `NEAR=name`, with the
`--command "/tp X Z"` that takes a capture there), with each cave biome's share of the caves
there; `decor_sights` prints `--pose` arguments for captures of old-growth forest, a forest
edge, a clearing, a meadow, a slope, a cold forest, dry grassland and snow near spawn;
`column_timing` reports generation time per column, and `river_speed` the cost of building a
region's rivers apart from the columns generated after (all `-- --ignored --nocapture`).

## Worn paths

Feet wear the ground (`crates/server/src/wear.rs`, rules in `assets/content/base/wear.ron`).
Each step onto a block wears it by a step, the blocks to either side by half and the one ahead
by a quarter, so paths form about three blocks wide. Plants go first: tall grass is trampled to
short grass, short grass, ferns and flowers to nothing. Then the ground passes through stages,
grass to worn grass to packed earth to coarse dirt, each block drawing how many steps each stage
takes from a hash of the seed and its position, so paths fray irregularly at their edges. The
steps are kept sparsely per surface column in the column (`Column::wear`), so they save and
load with it; only the changed blocks are sent, and only to players who have the column. Worn
ground heals a stage every five days nobody walks on it (twice as fast for a lone patch),
applied when a saved column loads, and grass coming back sometimes sprouts a tuft. Healing only
undoes what feet did, so generated trails keep their tread.

## Weather

The server simulates the weather; clients only draw it. What drives it is a pure function of
the seed, place and time: moist and dry air masses a few kilometres across that drift with the
world's prevailing wind and slowly change shape, and smaller, shorter-lived updrafts that are
strongest on warm afternoons. Near players, a `Low` tier process evolves humidity, cloud,
precipitation, ground wetness and lying snow cell by cell (128 blocks): humidity follows the
passing air mass away from its baseline (the usual humidity of the climate's annual rainfall),
cloud condenses as the air nears saturation, sooner in rising or cold air, and evaporates as it
dries, and thick cloud rains, which dries the air again. Typical land rains 8-20% of the time,
rainforests 30-50%, deserts almost never. Rain wets the ground, which dries over minutes, and
snow lies where it is cold enough and melts above freezing; as it deepens, grass turns snowy
block by block and turns back as it melts. The air is colder at night and warmer in the
afternoon, more so in dry climates and under clear skies, and it snows below about 0 °C. A
downpour under strong updrafts is a thunderstorm: each second it may raise a lightning strike
somewhere in the cell, which nearby clients see as a flash and a bolt and hear as thunder once
the sound has travelled. A cell coming into range first catches up on its last ten minutes, so
the weather at a place does not depend on when anyone arrived, and cells left behind return to
their baseline. The process uses only exact arithmetic on seeded hash streams, so it is
deterministic on every machine. It runs on the simulation clock, so it carries on when the
time of day is frozen or sped up; only what follows the sun reads the time of day.

The wind is part of it: light under settled skies, freshening and veering as fronts pass,
strong in storms, livelier in the afternoon, stronger on open heights and over water and
calmer in forests and valleys. Gusts sweep across the land with it, from one deterministic
function server and clients share (`voxmar_engine::weather::gust`), so grass, particles and
later gameplay agree.

Effects come on top, as data in `assets/content/base/weather.ron`: each is a product of
smooth conditions on the weather, the local climate and land (coast, valley, altitude,
latitude, moisture, vegetation, aether) and the time of day, raises channels of the weather
clients draw, and has a rarity. There are drizzle and downpours, thunderstorms (and the
towering supercells of hot, wet country, common there and rare elsewhere), heat lightning on
humid summer nights, sleet, freezing rain, hail, blizzards, thundersnow, squalls, drifting
snow, dust and sandstorms, mist after rain, dawn mist, mist off lakes on cool mornings, steam
off hot ground after rain, valley fog, sea fog, frost, diamond dust in bitter cold, heat haze,
rainbows (and rarely double ones), halos and sun dogs, the aurora near the poles (and rarely
far from them), shooting stars, meteor showers, ball lightning, marsh lights, and where the
land's aether is high glowing motes, the very rare aether storm and the legendary starfall.
Light rain and cloud are everyday; heavy rain is uncommon outside the tropics; wind is calm
to a light breeze most of the time, strong now and then, and a gale only in storms. Rarity is drawn per stretch of land (1.5 km) and time
(the effect's typical length) from the seed, and each world features a few rare effects and
suppresses a common one, so drizzle and mist are everyday sights and a meteor shower is an
event. `cargo test -p voxmar_server effect_spread -- --ignored --nocapture` prints how often
each shows across kinds of country, and `climate_spread` how often each kind of country is
cloudy, rains or snows at each intensity and how hard the wind blows.

Each player is sent `ServerMessage::Weather` for where they stand when it changes or every five
seconds, with the effects showing, and the client eases towards it: cloud builds and clears
over tens of seconds, rain sets in over a few, fog gathers slowly. The sky's cloud layer drifts
with the wind; its shape follows the weather. Rain, drizzle, snow, sleet, hail, dust and motes
fall or drift through a box around the camera, slanted by the gusting wind and stopped by a map
of the highest block around the camera, so nothing falls indoors, under trees or in caves.
Ambient particles from the land's air (pollen by day, fireflies on some warm nights, spores in
damp forests, drifting snow in the cold) come from `ClientAtmosphere`, which samples
`Overworld::atmosphere_at` around the player and applies the weather and time of day. Fog,
ground fog, rainbows, halos, aurora, shooting stars, heat haze, frost and wet ground are in the
weather the client receives and wait on the renderer (docs/weather-effects.md). F3 shows the
weather and the effects showing.

An admin's `/weather KIND` holds the cells within about a kilometre at that kind of weather
(kinds are data in `weather.ron`; what a kind leaves out stays natural, so `/weather rain`
snows where it is freezing) and its effects at full strength. Players there are told at once
and change over in a second or two. When its time is up it fades over a minute, and the
released cells drift back to the natural weather over the next few minutes. `/wind` does the
same for the wind alone, on top of any forced weather.

## Water

Water and lava flow as in Minecraft (`engine/src/fluid.rs`): a source spreads seven blocks over
flat ground (lava three), heading for the nearest edge within four blocks (lava two), falls as
far as it can, and dries up once nothing feeds it. Two sources make a third between them, so
water taken out of a pool comes back, and water that fills a hollow it cannot run out of stays
there as a pond. Lava cools: where a flow stops, its thin edge crusts over with basalt after
twenty seconds, and a flow left without its source hardens where it lay. Water quenches it: a
lava source turns to obsidian, flowing lava to basalt, and lava pouring into water turns the
water to broken basalt (`cobbled_basalt`). Every state is its own block (`water_flow_1` to
`water_flow_7`, `water_falling`; rivers also have pinned water that carries a current), so
chunks, saves and the network treat them like any block. Flowing water and river currents
carry swimmers along.

The server's kernel is event-driven: a block change wakes only the fluid beside it, each cell
moves on its fluid's tick (water every five ticks, lava every thirty, with a little hashed jitter
so fronts do not move in lockstep), and water that has settled costs nothing. Generated water is
already settled, so loading the world wakes nothing. A tick runs at most a budget of updates
that adapts to what they cost and how busy the server is (aiming at 4 ms), and the rest wait;
each player is sent one `Blocks` message per tick with the changes in columns they have. A lake
48 blocks across pouring 60 blocks into a cave (150 thousand cells changing) runs in about
0.3 µs per update.

## Sound

Music is in `assets/music` (generated offline with `tools/generate_music.py`) and plays now and
then with long quiet gaps; `M` toggles it. Starting a game fades the menu's music out, so a world
opens in quiet; returning to the menus brings it back.

Block sounds follow each block's material (`sound: Wood` in its definition; the kinds are listed
in `blocks.ron`), so new blocks sound right without new audio. Steps, landings, placing, the taps
while mining and breaking come from the player's own Minecraft install when it has them (the
official launcher's or Prism Launcher's asset store; nothing from Minecraft is shipped), with
Minecraft's own volumes and pitches. Otherwise they are synthesized at startup: short, soft and
warm, with several variants per cue and a small random pitch change so nothing repeats exactly.
Rain is a synthesized loop of tiny drops over a soft hiss, as loud as the rain is heavy and
quieter under cover. `cargo test -p voxmar_client sfx_preview -- --ignored` writes every
synthesized cue to `target/sfx_preview.wav`.

Ambience is synthesized the same way and follows where the player is. About once a second a
flood fill from the eye (on a worker, capped at 16,000 cells) measures how much air is around,
how much of it is open to the sky or under leaves, and the leaves, wood, water, grass, rock,
sand and snow it meets, nearer ones counting more. With the climate and biome from the world
generator, height above the land around, depth underground, the weather and the time of day,
that makes an environment vector, and `ambience.ron` in any content package lists what plays
where: beds (breeze, high wind and storm wind, rustling leaves, grasshoppers, crickets,
streams, surf, cave air) as stereo segments that crossfade and never repeat back to back, and
one-shots at a rate per minute (songbirds and a dawn chorus in the leaves around, a woodpecker
on a trunk, owls, a cricket in the grass, frogs and plops in the water, gusts on heights,
creaking trees, drips and rumbles in caves). One-shots sound from real blocks the fill found,
panned by direction, with a reverb as wet as the place is enclosed. Every condition is a soft
range with a fade, and rain ducks layers rather than silencing them. Captures run it silently
and log which beds would play. `cargo test -p voxmar_client ambience_preview -- --ignored`
writes about twenty seconds of each of nine typical places to `target/ambience_preview.wav`, and
`ambience_survey -- --ignored` prints what the scan makes of real places in a world (`SEED`).

## Rendering and performance

- 32³ chunks, palette-compressed at 0/1/2/4/8/16 bits per block; uniform chunks store nothing.
- Lighting: 4-bit sky + 4-bit block light, flood-filled per column on worker threads over the
  column plus 17 blocks of padding (light reach + mesh border), which is exact without any
  shared state. Edits relight only the columns within reach.
- Meshing: face culling 64 cells at a time on bit columns, then greedy merging of faces with
  equal texture, tint, AO and smooth light. 8-byte vertices; 16-bit indices when they fit.
- Smooth lighting and ambient occlusion per vertex, AO-aware quad triangulation.
- Cave culling: per-chunk face connectivity and a breadth-first visibility walk
  (Tommaso Checchi's algorithm) hide chunks the camera cannot see.
- Bevy's GPU preprocessing does frustum culling and indirect batched draws.
- Pixel-art texture array with alpha-weighted mipmaps; climate tints from colormaps in the shader.
- Plants (grass, flowers) are two crossed, double-sided quads that sway from their base.
- Leaves draw the faces between them too, like Minecraft's fancy leaves, so crowns show the
  same depth through their holes from every angle: one double-sided face where two leaves
  meet, none between leaves buried on every side, and only a crown's shell casts shadows.
  Faces between two of the same glass, ice or water are hidden; ice is clear blue with a sky
  reflection over still water.
- Day/night cycle, sky dome with sun, moon, stars and weather-driven clouds, rain and snow,
  aerial perspective (haze that thins with height and glows around the sun), HDR with bloom.
  The sun, moon, stars and planets are measured in pixels, so they stay round at any field
  of view. Nights are dark, lit by a faint blue moon.
- A sunlit top face gets about as much light as Minecraft's full brightness, and the display
  transform passes colours up to 0.8 through unchanged (rolling off only brighter ones), so
  blocks keep their texture colours in daylight.
- Distant terrain to ~6.6 km: smooth heightfield rings from the generator's surface
  functions, shaded as the block staircase they stand for (see docs/rendering.md).

Measured on an M5 Max at 1920×1080, view distance 16 (1089 columns, ~4.7M quads loaded):
~540 fps at ground level, ~360 fps looking over the landscape from above. Column generation is
~2.5 ms, lighting ~2.5 ms and meshing ~1.2 ms per column on one core; the 33×33 column area
streams in about a second.

Profile with Tracy: `cargo run --release --features tracy` (engine work is instrumented with
`tracing` spans).

See [docs/rendering.md](docs/rendering.md) for measurements, a comparison with GPU-driven
pipeline techniques, and the rendering plan.

## Next steps

- Draw only the face directions that can face the camera (per-direction index ranges).
- A custom GPU-driven terrain pass (see docs/rendering.md).
- Lower-detail meshes for distant chunks.
- Incremental light updates instead of relighting the affected columns.
- Entities, items, tools; the Trait/Field primitives; WASM mods.
