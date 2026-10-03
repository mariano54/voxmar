# voxmar

A moddable, open source voxel sandbox with an infinite procedurally generated world.
Rust, Bevy 0.19, wgpu (Metal on macOS, Vulkan/DX12 elsewhere).

## Running

```bash
cargo run --release                         # singleplayer (integrated server)
cargo run --release -- --host               # singleplayer, also open on the LAN
cargo run --release -p voxmar_server        # dedicated server on port 25665
cargo run --release -- --connect HOST:PORT  # join a server
```

Client flags: `--seed N`, `--name NAME`, `--view-distance N` (columns of 32 blocks, default 16),
`--no-vsync` (uncapped frame rate), and `--capture out.png [--pose dx,dy,dz,yaw,pitch | --flight SECONDS]`, which runs headless, waits for the
world to load, logs frame time and saves a screenshot.

Controls: WASD move, Space jump, Ctrl sprint, Shift descend, F toggle flight, left mouse break
(hold), right mouse place, 1-9 or wheel select hotbar slot, E inventory (click a slot, then
another to move it), F3 debug overlay, Esc release the mouse, R respawn.

### Textures

No Minecraft assets are shipped. At startup the client reads textures from the newest Minecraft
client jar it finds (official launcher or Prism Launcher), or from `VOXMAR_TEXTURES` pointing at
a jar, resource pack zip or folder. Without one it generates placeholder textures.

## Layout

| crate | role |
| --- | --- |
| `engine` | world data and rules, no renderer: palette chunks, column lighting, binary greedy meshing, visibility graph, collision, raycast, inventory, block registry |
| `worldgen` | staged generator: geology → climate/biomes → resources → terrain, caves, trees |
| `protocol` | client/server messages and transports (in-process channels, TCP) |
| `server` | authoritative simulation (fixed 20 Hz tick), validation, streaming; `voxmar-server` binary |
| `client` | Bevy app: rendering, input, prediction, HUD; the only crate that depends on Bevy |

Content is data: blocks are defined in `assets/content/base/blocks.ron` and loaded through the
same registry a mod will use. Client and server compare a content fingerprint on join.

How the design primitives map to the code today:

- **Definition**: `BlockDef` (RON), namespaced per package (`base:stone`; `core:air` is built in), compiled into `BlockRegistry` lookup tables.
- **Generator**: the `Generator` trait; `Overworld` is the first implementation. Random
  streams are seeded from world seed + generator id and version + stream + position, so generation is pure and
  order-independent.
- **Action**: `protocol::Action` (intents such as break, place, move item, respawn), validated
  and committed by the server.
- **Process**: per-tick server rules (fall damage, regeneration, void damage).
- **Agent**: players. **Trait** and **Field** are not built yet; replication is currently
  explicit messages.

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
- Pixel-art texture array with alpha-weighted mipmaps; biome tints from colormaps in the shader.
- Day/night cycle, sky dome with sun, moon and stars, distance fog, HDR with bloom.

Measured on an M5 Max at 1920×1080, view distance 16 (1089 columns, ~4.7M quads loaded):
~540 fps at ground level, ~360 fps looking over the landscape from above. Column generation is
~2.5 ms, lighting ~2.5 ms and meshing ~1.2 ms per column on one core; the 33×33 column area
streams in about a second.

Profile with Tracy: `cargo run --release --features tracy` (engine work is instrumented with
`tracing` spans).

## Next steps

- Draw only the face directions that can face the camera (per-direction index ranges).
- GPU occlusion culling (Hi-Z) via a depth prepass for the voxel material.
- Lower-detail meshes for distant chunks.
- Saving the world (edited columns are kept in memory and never unloaded for now).
- Incremental light updates instead of relighting the affected columns.
- Entities, items, tools; the Trait/Field primitives; WASM mods.
