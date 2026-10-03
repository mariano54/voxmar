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

Client flags: `--seed N` (for worlds `--world` creates, and captures), `--name NAME`,
`--view-distance N` (columns of 32 blocks, default 16), `--no-vsync` (uncapped frame rate), and
`--capture out.png [--pose dx,dy,dz,yaw,pitch | --flight SECONDS] [--menu | --screen title|worlds|create|join|settings]`,
which runs headless, waits for the world (an unsaved one unless `--world` or `--connect` is
given) or the menu backdrop to load, logs frame time and saves a screenshot.

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

No Minecraft assets are shipped. At startup the client reads textures from the newest Minecraft
client jar it finds (official launcher or Prism Launcher), or from `VOXMAR_TEXTURES` pointing at
a jar, resource pack zip or folder. Without one it generates placeholder textures.

## Layout

| crate | role |
| --- | --- |
| `engine` | world data and rules, no renderer: palette chunks, column lighting, binary greedy meshing, visibility graph, collision, raycast, content packages, actions, scheduler, traits, fields |
| `worldgen` | staged generator: world dials → geology, rivers → climate fields → ecology → resources → terrain, caves, trees, plants |
| `protocol` | client/server messages and transports (in-process channels, TCP) |
| `server` | authoritative simulation (fixed 20 Hz tick), validation, streaming, world saves; `voxmar-server` binary |
| `client` | Bevy app: rendering, input, prediction, HUD, menus; the only crate that depends on Bevy |

Content is data: blocks are defined in `assets/content/base/blocks.ron` and loaded through the
same registry a mod will use. Client and server compare a content fingerprint on join.

How the design primitives map to the code today:

- **Definition**: blocks (`content/base/blocks.ron`) and actions (`content/core/actions.ron`),
  namespaced per package (`base:stone`, `core:mine`; `core:air` is built in).
- **Action**: the only way discrete changes happen. Every action goes through one lifecycle on
  the server (propose, validate, schedule one completion, re-validate, commit or cancel) and
  compiles down to a fixed set of engine effects. Clients validate with the same rules to
  predict. Mining, placing, moving items and respawning are all actions.
- **Process**: continuous server rules at a scheduling tier (`High` every tick, `Low` once a
  second): falling damage, and regeneration, which heals slower where the temperature field
  is freezing.
- **Trait**: typed data with declared replication and persistence (`core:health`,
  `core:inventory`, `core:position`); owner-replicated traits are sent when they change.
- **Field**: coarse 16-block grids, generated baseline plus emitted differences
  (`core:temperature`, in °C, from the climate generator).
- **Agent**: players, whose controller is input. Machines and creatures come later.
- **Generator**: the `Generator` trait; `Overworld` is the first implementation. Random
  streams are seeded from world seed + generator id and version + stream + position, so
  generation is pure and order-independent. Distant terrain is sampled from the same functions.

## World generation

There are no biomes. Each world draws a few physically meaningful dials from its seed
(`Overworld::dials`: mean temperature, equator-to-pole temperature difference and distance,
ocean fraction, continent size, humidity, prevailing wind, tectonic activity, erosion age,
river density, vegetation), with sensible correlations. Everything else is a continuous field:

- **Temperature** falls towards the poles (latitude repeats along z, colder to the north, so
  hot, temperate and cold land always lie in that order) and with altitude.
- **Precipitation** follows moisture the prevailing wind carries in from the sea: wet coasts,
  drier interiors, rain shadows behind mountains, wet tropics and dry subtropics.
- **Rivers and lakes** are carved into the height function, ending as dry valleys in high country.
- **Surfaces, trees and plants** follow smoothly from temperature, moisture, slope and height:
  transitions are bands where materials interleave (grass, snowy grass, snow; grass, coarse
  dirt, sand), and each climate has its own procedurally shaped trees (giant spruce, pine,
  birch, oak, dark oak, cherry, willow, jungle giants, acacia, palm, mangrove, shrubs, cacti).

`Overworld::site(x, z)` returns every parameter at a position for gameplay to use.
`cargo test --release -p voxmar_worldgen map_preview -- --ignored` (optionally with `SEED=n`)
writes a shaded 8 km map to `target/map_preview.ppm`.

## Sound

Music is in `assets/music` (generated offline with `tools/generate_music.py`) and plays now and
then with long quiet gaps; `M` toggles it. Sound effects are synthesized at startup from each
block's material (`sound: Wood` in its definition), so new blocks sound right without new audio:
short, soft and warm, with several variants per cue and a small random pitch change so nothing
repeats exactly. `cargo test -p voxmar_client sfx_preview -- --ignored` writes every cue to
`target/sfx_preview.wav`.

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
- Day/night cycle, sky dome with sun, moon and stars, aerial perspective (haze that thins
  with height and glows around the sun), HDR with bloom.
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
