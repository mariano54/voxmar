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
frame rate), `--weather clear|cirrus|cumulus|cloudy|overcast|rain|storm|snow` (show that instead
of the server's weather), and
`--capture out.png [--pose dx,dy,dz,yaw,pitch | --flight SECONDS] [--time FRACTION] [--menu | --screen title|worlds|create|join|settings]`,
which runs headless, waits for the world (an unsaved one unless `--world` or `--connect` is
given) or the menu backdrop to load, logs frame time and saves a screenshot; `--time` holds the
time of day (a fraction of the day, 0 at sunrise).

The title screen, over a slowly turning view of a demo world, leads to Singleplayer (saved
worlds, most recently played first, to play, create or delete), Multiplayer (join by address;
the last few servers are remembered) and Settings. A new world takes a name, an optional seed
(a number, or any text, which is hashed; empty means random) and a game mode: Creative allows
flight and makes you the world's admin, Survival does neither.

Controls: WASD move, Space jump, Ctrl sprint, Shift descend, F toggle flight, left mouse break
(hold), right mouse place, 1-9 or wheel select hotbar slot, E inventory (click a slot, then
another to move it), F3 debug overlay, Esc menu, R respawn.

Esc opens the settings menu: render distance, field of view, VSync, debug overlay, music and
sound volume, mouse sensitivity, and fly speed for admins, with Resume, Save and quit to title
(Disconnect on a server) and Quit game. Settings are saved to `voxmar/settings.ron` in the OS
config directory (`~/Library/Application Support` on macOS, `$XDG_CONFIG_HOME` or `~/.config`
on Linux, `%APPDATA%` on Windows; `$VOXMAR_CONFIG` replaces `…/voxmar`); `--view-distance` and
`--no-vsync` override them. On a dedicated server, name admins with `--admin NAME`
(repeatable), and cap render distance with `--view-distance N` (default 32).

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
type `stop` to save and quit.

### Textures

Textures come from a stack of packs, each a folder, zip or jar; every texture is taken from the
first pack that has it:

1. the pack chosen with `VOXMAR_TEXTURES` (a resource pack or a Minecraft jar also works), which
   overrides ours;
2. our default pack, `assets/packs/default`;
3. any other pack in `assets/packs`, in name order: mods ship art this way, adding textures but
   never replacing ours;
4. without a chosen pack, the newest Minecraft client jar found (official launcher or Prism
   Launcher), only for names the packs above lack.

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
make whole families from one line (a wood's log, stripped log, wood, planks, leaves and sapling;
a rock's raw, cobbled, polished and brick blocks), and templates are shared across packages, so
a mod's new wood is one line too. A plant can list variants, other looks of itself that become
their own blocks and drop the plant, as many as their size. Blocks carry tags (`flower`,
`wetland`, `edible`, `alchemy:calming`, `dye:blue`; the vocabulary is at the top of
`blocks.ron`) and a rarity, so biomes, food and alchemy select them without naming them.

How the design primitives map to the code today:

- **Definition**: blocks (`content/base/blocks.ron`) and actions (`content/core/actions.ron`),
  namespaced per package (`base:stone`, `core:mine`; `core:air` is built in).
- **Action**: the only way discrete changes happen. Every action goes through one lifecycle on
  the server (propose, validate, schedule one completion, re-validate, commit or cancel) and
  compiles down to a fixed set of engine effects. Clients validate with the same rules to
  predict. Mining, placing, moving items and respawning are all actions.
- **Process**: continuous server rules at a scheduling tier (`High` every tick, `Low` once a
  second): falling damage, regeneration, which heals slower where the temperature field
  is freezing, and the weather (below).
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
- **Rivers and lakes** are carved into the height function, ending as dry valleys in high country.
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
lines such as game trails) over the stones of its rock province, while snow, ice and bare rock
follow the climate everywhere. Trees and plants still follow the climate directly: each
climate has its own procedurally shaped trees (giant spruce, pine, birch, oak, dark oak,
cherry, willow, jungle giants, acacia, palm, mangrove, shrubs, cacti).

`Overworld::site(x, z)` returns every parameter at a position for gameplay to use, including
its biome, the biome next door and the distance to their border; `Overworld::biome_at` and
`Overworld::locate_biome` serve commands.
`cargo test --release -p voxmar_worldgen map_preview -- --ignored --nocapture` (optionally with
`SEED=n` or `SEED=1,2,3`, `BIOMES=1` to colour by biome, `FIELD=erosion` and so on) writes a
shaded 8 km map to `target/map_preview.ppm` and prints each biome's share of it.

## Weather

The server simulates the weather; clients only draw it. What drives it is a pure function of
the seed, place and time: moist and dry air masses a few kilometres across that drift with the
world's prevailing wind and slowly change shape, and smaller, shorter-lived updrafts that are
strongest on warm afternoons. Near players, a `Low` tier process evolves the humidity, cloud
and precipitation fields cell by cell: humidity follows the passing air mass away from its
baseline (the usual humidity of the climate's annual rainfall), cloud condenses as the air
nears saturation, sooner in rising or cold air, and evaporates as it dries, and thick cloud
rains, which dries the air again. Typical land rains 8-20% of the time, rainforests 30-50%,
deserts almost never, and below 0 °C it snows. A downpour under strong updrafts is a
thunderstorm: each second it may raise a lightning event somewhere in the cell, which nearby
clients see as a flash and hear as thunder once the sound has travelled. A cell coming into range first catches up on its
last ten minutes, so the weather at a place does not depend on when anyone arrived (or whether
the world was just loaded), and cells left behind return to their baseline. The process uses
only exact arithmetic on seeded hash streams, so it is deterministic on every machine.

Each player is sent `ServerMessage::Weather` for where they stand (low cloud cover, how heaped
it is, cirrus, humidity, precipitation, how much falls as snow, wind) when it changes or every
five seconds, and the client eases towards it: cloud builds and clears over tens of seconds,
rain sets in over a few. The sky's cloud layer sits at fixed places in the world and drifts
with the wind; its shape follows the weather, from flat grey stratus through billowing cumulus
to dark storm cloud, under streaks of cirrus. Thick cloud greys and dims the sky, sunlight,
ambient light and fog, and rain and snow close in the view. Rain streaks, splashes and
snowflakes fall through a box around the camera, slanted by the wind and stopped by a map of
the highest block around the camera, so nothing falls indoors, under trees or in caves.
F3 shows the weather.

## Sound

Music is in `assets/music` (generated offline with `tools/generate_music.py`) and plays now and
then with long quiet gaps; `M` toggles it. Sound effects are synthesized at startup from each
block's material (`sound: Wood` in its definition), so new blocks sound right without new audio:
short, soft and warm, with several variants per cue and a small random pitch change so nothing
repeats exactly. Rain is a synthesized loop of tiny drops over a soft hiss, as loud as the rain
is heavy and quieter under cover. `cargo test -p voxmar_client sfx_preview -- --ignored` writes
every cue to `target/sfx_preview.wav`.

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
