# Plan: a diverse, connected world

Oct 4, 2026. Covers biomes, blocks and art, terrain, rivers and water, caves, decoration,
weather, and the visual and camera work Mariano asked for. Read with
[design-philosophy.md](design-philosophy.md), especially "Everything connects" and
"Slightly fantastic".

## Decisions

| # | Question | Decision |
| --- | --- | --- |
| 1 | Biome model | Hybrid: biomes are data definitions that claim boxes in climate space; the climate stays continuous underneath. |
| 2 | Selection axes | Temperature, moisture, continentalness, erosion and weirdness, plus extra seeded axes for diversity: aether, rock province, fertility and a variant field. |
| 3 | Borders | Blended over 16–48 blocks. |
| 4 | Biome size | Large, about 1–2 km across. |
| 5 | Count | About 30 base biomes plus variants first; reaching 100+ is just more data. |
| 6 | Fantasy | Part of everything: slight fantasy in every biome, much more in rare fantasy biomes. Follows the aether field. |
| 7 | Terrain | Biomes shape terrain (dunes, terraces, spires, karst, volcanoes), and geology gets Tectonic-style bigger ranges, a bit less extreme. |
| 8 | Caves | Now. Big caverns and ravines allowed, not extremely common. Cave biomes. |
| 9 | Water | Streams-style rivers that flow downhill into lakes and the sea, plus flowing-water physics. |
| 10 | Art | Our own Minecraft-style pixel-art pack; nothing from Minecraft required. |
| 11 | System links | Data hooks now (tags, plant properties, fields), gameplay later. |
| 12 | Old saves | Always fine to break. |

Also asked for: many more weather effects, commands to trigger weather and set the time,
merging work to main frequently, and the hiking-mod ideas (research in the project files,
`research/hiking-simulator-mods.md`).

Two requirements that shape every content format:

- **A shared pool, not one set per biome.** Blocks, trees and plants are a pool that many
  biomes draw from. A biome lists selectors (by tag, or by name for a few signature or unique
  species), each with a weight. A candidate's final weight is the selector weight times how
  well its own climate preference fits the spot. New blocks and plants, including a mod's,
  appear in every fitting biome without editing any biome.
- **Diverse rarity.** Biomes, variants, trees, plants, wonders, cave features and weather
  effects carry a rarity: common, uncommon, rare, very rare or legendary, with well-spread
  multipliers. Rare biomes are gated by a low-frequency mask, so they appear in some regions
  and not others. Each world also boosts a few rare things and suppresses a few common ones
  from its seed, so no two worlds share a rarity table. All of it stays deterministic.

My defaults where the answers left a fork (say if any is wrong):

- **World height 256 → 384, sea level 64 → 128.** Matches Minecraft's 128 blocks of
  underground and 256 above, which big caves and taller ranges need. Costs little: solid and
  empty chunks are cheap. Coordinated with the renderer thread because the GPU terrain pages
  hold columns of chunks.
- **Plants and blocks connect to gameplay through properties and tags only**, no food or
  potion gameplay yet.
- **Minecraft resource packs stay loadable** as an optional user pack layered over ours.

## Where we start

- Climate is continuous (`climate.rs`): temperature from latitude and altitude, moisture
  carried by the prevailing wind with rain shadows. No biomes exist.
- `ecology.rs` hardcodes what grows where: 18 tree species (`trees.rs`), 12 flowers, grass,
  ferns, berries and cane, each with a climate band in code.
- `features.rs` places only trees, cacti and boulders.
- Rivers are where a warped noise crosses zero, always at sea level; lakes come from a
  separate noise and are always low and flat (`geology.rs`). Water never flows.
- Caves (`terrain.rs`) are one cheese-cavern noise plus two tunnel lattices, always dry.
- About 90 blocks (`assets/content/base/blocks.ron`). Textures come from the player's own
  Minecraft jar, with flat placeholders otherwise (`client/src/textures.rs`).
- Weather is simulated on the server (humidity, cloud, rain, snow, storms, lightning). There
  is no command console.

## Architecture

### Biomes

**Selection fields.** Each is a pure function of seed and position, smooth over hundreds of
blocks, and exposed on `Site` so gameplay can read it:

| Field | Source | Mostly decides |
| --- | --- | --- |
| temperature | existing climate | cold, temperate, hot families |
| moisture | existing climate | desert … rainforest |
| continentalness | existing continentality, negative over sea | coast, inland, ocean depth |
| erosion | existing mountain and slope | flat, hilly, rugged |
| weirdness | new noise, ~1/1500 | unusual variants and terrain (Minecraft's ridges) |
| aether | new noise plus world dial | how fantastical; fantasy biomes need it high |
| rock province | new cellular noise, ~2–4 km cells | which stones and clays: granite shield, limestone karst, basalt volcanic, marble, sandstone, slate |
| fertility | new noise | decoration density, later crop growth |
| variant | new noise, ~400 blocks | which sub-biome inside a parent |

Rock province is independent of climate, so a desert can sit on red sandstone or on granite,
and stone palettes vary inside a climate band. More world dials join the existing ones:
fantasy level (mean aether), volcanism, cave density, mountain height, biodiversity.

**Definition.** A biome is a RON Definition in a content package:

```ron
(
    name: "maple_forest",
    tags: ["forest", "temperate", "autumn"],
    climate: (temperature: (6.0, 14.0), moisture: (0.9, 1.6), continentalness: (0.1, 1.0),
              erosion: (0.0, 0.6), aether: (0.0, 0.7)),
    weight: 1.0,
    terrain: [Hills(amplitude: 6.0)],
    surface: (top: "base:grass_block", filler: "base:dirt", depth: 3, cliff: "base:stone"),
    stones: Province,                       // or an explicit list for this biome
    colors: (grass: (178, 140, 60), foliage: (196, 92, 40), water: (60, 90, 160)),
    decoration: [ /* fixed order: patches, rocks, trees, logs, flowers, fungi, moss, grass */ ],
    trees: [("base:maple", 1.0), ("base:birch", 0.2)],
    plants: [("base:aster", 0.4), ("base:fern", 0.6)],
    wonders: [("base:silverbark_tree", 0.002), ("base:moonbloom", 0.004)],  // × aether
    variants: [("base:red_maple_grove", 0.25)],
    ambience: "base:forest_day",
)
```

`wonders` is how every biome gets slight fantasy: rare fantastical features whose chance
scales with aether. Fantasy biomes are ordinary definitions whose box requires high aether
and whose lists are full of fantastical things.

**Picking.** Per 4×4-block cell, the picker scores every biome by its distance to the box in
normalised parameter space (0 inside), biased by weight, and keeps the best two. Nearest-box
scoring means every climate maps to some biome, with no holes (Wilder Wild's approach; the
same idea as Minecraft's multi-noise). A few hundred biomes × 9 axes is microseconds per
column. Sub-biomes then take their share of the parent's region by the variant field
(Terrestria's pattern).

**Blending.** The score gap between the best two sets a blend factor over 16–48 blocks.
Continuous things (colours, terrain amplitude) interpolate; discrete things (surface blocks,
species) dither by hash, the way `terrain.rs` already interleaves materials across bands.

**Trees and plants as data.** The 18 hardcoded species become parameterised tree
definitions (trunk and crown shapes, blocks) that biomes reference. Flowers and plants become
definitions with properties for later systems:

```ron
(name: "moonbloom", block: "base:moonbloom", tags: ["flower", "night", "wonder"],
 properties: (nutrition: 0, alchemy: ["luminous", "calming"], dye: "pale_blue"))
```

**Stability.** `Overworld::sampler` and `surface_sample` keep their signatures; distant
terrain reads biome colours and blocks through `SurfaceSample` as today. Gameplay gets
`Site::biome` and `/biome` and `/locate biome` commands.

### Blocks and art

- **Families.** RON templates expand into block sets so a new wood or stone is one line: a
  wood family makes log, stripped log, wood, planks, leaves and sapling; a stone family makes
  raw, cobbled, polished and bricks.
- **First wave of content:**
  - Stones: marble, limestone, slate, basalt, chalk, sandstone, red sandstone, travertine.
  - Clays: fired clay in about 12 colours, for mesa bands and painted caves.
  - Soils: loam, peat, red earth, silt.
  - Woods: maple, redwood, cypress, willow, baobab, palm.
  - Fantasy woods: silverwood, emberwood, glowcap.
  - Ores: cobalt, tin, silver, plus one fantastical ore (aetherite).
  - Plants: about 60 flowers, herbs, bushes, grasses, reeds and fungi, each with properties.
- **Inventory icons.** Every block and plant gets a 2D icon. It defaults to its texture and
  can be its own sprite.
- **Texture pack.** `assets/packs/default/` holds PNGs and texture programs (RON recipes
  that render deterministically to 16×16: palette, noise layers, edge darkening, highlights,
  patterns) and text-grid sprites. A `texgen` tool renders recipes and writes a contact sheet
  for review. Packs layer: ours, then any user pack (a Minecraft jar or resource pack still
  works), then placeholders.
- **Style.**
  - Vanilla-like 16×16 with 4–6 hue-shifted shades per material, readable at distance.
  - Leaf shapes in the spirit of Xali's Bushy Leaves, grass sides like Fast Better Grass,
    crops like Flowering Crops.
  - Variety over resolution: weighted texture variants and connected textures, as in
    Reimagined and Continuity.
- **Schema additions:** `tags`, `family`, `icon` now; `variants`, `connect_group` and an
  emissive mask later, together with the renderer.
- **Loading.** Only `textures.rs` changes: the source becomes a stack of packs. It still
  hands `render::setup` a list of images, so the renderer is untouched.

### Terrain shape

- **Geology, Tectonic-lite:** taller and longer ranges (peaks to about 230 above sea level),
  deeper oceans, plateaus, broad valleys, and jagged peaks where erosion is low.
- **Biome shapers:** named terrain modifiers with parameters, faded out near biome edges.
  - Dunes: `|simplex|`, capped.
  - Mesa terraces with clay bands.
  - Shattered spires.
  - Karst towers on limestone.
  - Volcanic cones with a caldera, outline from angle noise.
  - Canyons from a terrace function.
- **Cliff rule.** Steep slopes show the biome's cliff block (stone, sandstone, basalt)
  instead of grass. `terrain.rs` already does this for bare rock; it becomes per biome.

### Rivers and lakes

Streams-style rivers in an infinite, deterministic world:

1. **Springs** are seeded on a jittered grid in uplands, with density from rainfall and the
   river dial.
2. **Tracing.** Each spring traces steepest descent over the analytic height field. Where it
   reaches a local minimum, a bounded flood fill finds the spill point: that depression is a
   lake, and the river continues from the outlet.
3. **Joining.** Rivers that meet merge. Width and depth grow with the number of springs
   upstream.
4. **Stamping.** Every column a river crosses gets its water level, which falls downhill.
   Steep drops become rapids and waterfalls.
5. **Purity.** Tracing reads only the analytic heights, so any region can compute every
   river through it by tracing springs within a bounded radius (the longest river allowed).
   Results are cached per 1024-block region. Output never depends on generation order.
6. **Carving.** Rivers cut their beds and valleys into the terrain and feed `Site.river`.
   Lakes sit at their own spill height, not sea level.

`SurfaceSample` gains the water surface height (today water is assumed to be at sea level).
The renderer thread is told before that lands.

### Water physics

- **Native fluid kernel** in the engine. Water and lava are fluid definitions: a source, 7
  flow levels and a falling state, registered as block states so chunks stay a palette of
  ids. Rates, spread distance and the infinite-source rule are data. Interactions (water
  meets lava) are actions.
- **Event-driven.** Only changed cells enter a queue. A per-tick budget applies, and settled
  water sleeps. Generated rivers and lakes are placed already settled, so loading a chunk
  causes no update storm.
- **Meshing.** Sloped surfaces from corner levels, with flow direction for the scrolling
  texture. This touches the packed vertex and `voxel.wgsl`, so it is done with the renderer
  thread.
- **Currents** push players and dropped items. Changes reach clients as ordinary batched
  block diffs.

### Caves

What the cave mods do, and what we take:

- **Minecraft 1.18 noise caves:** cheese caverns with pillars, winding spaghetti tunnels,
  narrow noodles, and aquifers (local water tables) instead of one sea level underground.
- **YUNG's Better Caves:** caves that grow larger and more complex with depth, lava caverns,
  underground lakes and rivers, flooded caverns.
- **Immersive Cavegen and Cave Overhaul:** longer, joined systems and domain-warped
  labyrinths.
- **Cave biomes** from Terralith, YUNG's Cave Biomes and Profundis: frozen, mushroom, molten,
  crystal, black, white, arid and painted, floral lush. Alex's Caves points the way for
  fantasy ones.

Our version:

- **Layers**, each with its own frequency dial, evaluated on coarse 3D lattices with trilinear
  interpolation as today:
  - Cheese caverns: rare, large, deep, with pillars, and clustered by a region mask so some
    areas are cavernous.
  - Spaghetti and noodle tunnels connecting them.
  - Ravines carved along seeded splines: about one per 4 km², 30–70 deep, some open to the
    sky.
  - Sinkholes on karst.
- **Water tables per 16×16×12 cell** give flooded caves and underground lakes at their own
  levels, with lava below a depth. Barriers keep them from hanging in the air. Underground
  rivers join the watershed later.
- **Cave biomes are biome definitions with `layer: Underground`**, chosen by depth,
  temperature, moisture, rock province and aether:
  - dripstone, lush, fungal, frozen, molten, painted (clay bands), crystal, flooded and deep
    dark caves;
  - one or two fantasy caves, such as glowroot hollows.
- **Decoration:** stalactites, moss, hanging roots, mushrooms, crystals and glowing lichen.

### Ground decoration

From Geophilic, Wilder Wild and Blooming Biosphere:

- **Fixed order per biome:** dirt and moss patches, rocks, trees, tree ornaments, fallen
  logs, flowers, fungi, moss, grass.
- **Noise-modulated counts**, so trees come in dense stands with gaps.
- **Clearings** with a soft fade at the edge: trees skip them, meadow decoration goes in.
- **Fallen logs** (some hollow, some with branches) placed only when supported, stumps 1–3
  tall, bushes, rock piles.
- **Flower species chosen by noise**, so they grow in drifts.
- **Gravel and coarse-dirt ribbons** that read as natural trails.
- **Every feature checks its support**, so nothing hangs in mid-air.

### Hiking, from the research

- **Half-height terrain:**
  - A half block wherever the fractional part of the height field is ≥ 0.5. This is pure and
    needs no neighbour reads.
  - Stored as a second occupancy mask and meshed with an extra half-unit Y bit.
  - Step height 0.5, plus client-side camera smoothing on step-ups.
  - Plants on half blocks are lowered.
- **Trails:**
  - Points of interest per 512-block region (summits, lakes, waterfalls, later settlements).
  - A neighbourhood graph between them, owned by the region of each edge's lower endpoint.
  - Integer-cost A* with deterministic tie-breaks over the analytic fields.
  - Splines, then per-chunk stamps. Bridges over deep water and fords over shallow.
  - Reuses the rivers' region cache.
- **Ground wear:**
  - A sparse u8 per surface column in chunk data, raised by footsteps.
  - Thresholds from a hash; stages (worn grass, worn dirt, coarse dirt) in RON.
  - Decay applied lazily when the chunk loads.

### Weather and commands

- **More effects**, each a data entry keyed by weather state and climate:
  - fog in valleys at dawn, sea fog, mist after rain
  - drizzle and downpour, hail, sleet
  - blizzards with whiteout and snow cover building up
  - sand and dust storms in dry land
  - wind gusts that bend plants and carry leaves
  - visible lightning strikes, rainbows after rain, aurora at high latitudes, heat haze
  - one rare fantastical storm where aether is high (glowing motes, falling stars)
- **Shared wind.** Server and client share one deterministic wind function, so visuals and
  gameplay agree.
- **Puddles and wet surfaces** need the shader pack and come with it.
- **Command console:** a chat line with commands that are admin-permissioned actions through
  the protocol:
  - `/time set <day|noon|night|midnight|ticks>` and `/time speed <x>`
  - `/weather <kind> [seconds]`
  - `/tp`, `/biome`, `/locate biome <name>`, `/seed`, `/gamemode`
  - Commands are data-registered, so mods can add their own.

### Visuals and camera

- **Zoom** (CinematicZoom): hold a key to zoom in creative, scroll to change the amount,
  optional cinematic smoothing. Key, amount and smoothing live in settings.
- **Raised:** lift the hotbar and other HUD elements off the bottom edge by a configurable
  offset.
- **Clouds:** thicker, fewer holes. Raise the base coverage, erode less with fine octaves,
  and add a second, thicker layer. The full volumetric look comes with the shader pack.
- **Bushy leaves** (Xali's, made our own):
  - Leaf blocks marked `bushy` emit tilted cards that reach past the cell, rotated per
    voxel by hash in the vertex shader.
  - Drawn in their own alpha-tested bucket outside greedy meshing, with dark-cutout mipmaps
    so distant trees don't dissolve.
  - Works for every tree, because trees are data.
- **Better grass:** a grass block's side shows grass where the block in front and below is
  also grass, so slopes read as continuous turf. Short tufts are optional.
- **Shader pack system and our Complementary-style pack** (Unbound plus Euphoria Patches):
  - **Loader:** loads WGSL stage modules and a RON settings schema. The settings menu is
    generated from the schema.
  - **First pack:** soft filtered shadows, volumetric light and god rays, atmospheric fog,
    water with reflections, refraction and waves, puddles and wetness, SSAO, bloom, waving
    plants and leaves, aurora and rainbows, and Complementary-style clouds.
  - **Options:** every feature is a toggle or slider, with low, medium, high and ultra
    presets.
  - **Budget:** high stays under ~4 ms per frame on an M-series Mac.
- **Ambience:**
  - A RON table of particle emitters: falling leaves, fireflies, pollen, cave dust,
    waterfall mist.
  - Emitters fire from a fixed budget of random voxel samples near the camera, with
    waterfalls found at meshing time.
  - Layered ambient sound: crossfaded beds by environment, plus one-shots placed at real
    trees and water.
- **Backlog from the videos:** pick-up notifications, footprints, boat wakes once boats
  exist, menu blur, and a map with waypoints once worlds get big.

### Revisions after reading the mods' source

A source-level study of about 25 mods (project files, `research/mod-source-deep-dive/`)
found places where the design above was weaker. These replace the earlier text where they
disagree:

- **Biome picker (W1).**
  - Erosion and weirdness come from a smoothed terrain classification, not raw noise and
    slope, so borders follow landforms.
  - The axes are quantised into bands and the biome table has no holes by construction.
  - Rare biomes live in the noise tails rather than getting tiny weights.
  - Ecotone slices are generated across temperature boundaries, and biomes can `inherit` a
    parent.
  - `Site` carries the distance to the biome edge and the neighbouring biome.
  - Variants get equal shares of area.
- **Surface (W1, W7, W9).** An ordered list of surface rules replaces `(top, filler, cliff)`.
  Rules can test depth, slope, dithered altitude, rock province and noise contour bands, which
  gives creeks, game trails, mineral rivulets, sand ripples, treelines and snowlines.
- **Colours.** By default they come from a continuous climate colour map plus palette noise.
  Only identity biomes override them.
- **Terrain (W8).**
  - Terrain never reads the chosen biome: shapes switch on through climate ramps, with
    breakpoints shared with the biome table.
  - Mountains, rivers and coasts follow noises folded with `abs()`, so they form chains
    (Tectonic).
  - Shapes can add or carve in 3D, gated by 2D masks, for arches, sea cliffs and leaning
    spires.
  - Densities are given per 16³ section.
- **Rivers (W4).**
  - Two tiers on a coarse grid: trunks first, then tributaries. Regions record where rivers
    cross their borders.
  - Flow is catchment × rain. Water levels are kept in 1/8-block units and never rise
    downstream.
  - Waterfalls need a drop of at least 3 and get a plunge pool.
  - Lakes fill by humidity, down to closed salt basins.
  - Generated water is sealed.
  - Underground rivers run where a river meets a mountain, and caves keep a band clear
    around rivers.
- **Water physics (W11).**
  - River water is pinned, with a current direction, speed and fractional height.
  - Fluid ticks are jittered and their budget adapts to load.
- **Clouds (W6).** A thin slab with a sharp vertical profile and a high opacity gain, which is
  why Complementary's clouds look thick.
- **Renderer (graphics thread).**
  - Coloured block light, as a compute flood fill.
  - Performance profiles separate from look presets, with options compiled as pipeline
    constants.
  - Half-resolution clouds and volumetrics, with dithering and TAA.
- **Smaller additions.**
  - A snow blanket down through the canopy.
  - Tree ornaments built from each tree's own blocks.
  - Forest structure graded by one noise.
  - Deterministic geysers.
  - Fireflies on only some nights.
  - An enclosure flood fill for ambient sound.
  - Waterfall size classes.
  - Engineered trails with switchbacks and cut and fill.
  - Wear floors on generated trails.

## Work items

Each item runs in its own git worktree and branch. It merges `main` in often, and lands on
`main` as soon as it builds, its tests pass and it looks right in game (`--capture`
screenshots, `map_preview` for worldgen). Renderer files (`client/src/render/`,
`engine/src/mesh.rs`, shaders) belong to the graphics thread; items that need them say so and
coordinate first.

| ID | Item | Needs | Main areas | Wave |
| --- | --- | --- | --- | --- |
| W1 | Biome core: selection fields, biome definitions, picker, blending, trees and plants as data, ~10 starter biomes, `/biome` | | worldgen `climate`, new `biome`, `ecology`, `trees`, content | 1 |
| W2 | Blocks and art: families, first content wave, icons, `texgen`, default texture pack, pack stack | | content, `tools/texgen`, `assets/packs`, client `textures.rs` | 1 |
| W3 | Caves: world height and sea level, caverns, tunnels, ravines, water tables, lava | (height: renderer heads-up) | engine `coords`, worldgen `terrain` | 1 |
| W4 | Rivers and lakes: springs, tracing, lakes, carving, water levels | | worldgen `geology`, `terrain` water | 1 |
| W5 | Weather effects and command console | (renderer for new effects) | server `weather`, protocol, client HUD and `weather` | 1 |
| W6 | Zoom, raised HUD, thicker clouds | (clouds: `sky.wgsl`) | client `player`, `settings`, `hud` | 1 |
| W7 | Ground decoration: clearings, stands, logs, stumps, bushes, rocks, drifts, ribbons, cliff rule | W1 | worldgen `features`, `ecology` | 2 |
| W8 | Terrain shape: Tectonic-lite geology and biome shapers | W1 | worldgen `geology`, `terrain` | 2 |
| W9 | Biome content: ~30 base biomes, variants, 5–6 fantasy biomes, wonders | W1, W2, W7 | content | 2 |
| W10 | Cave biomes and cave decoration | W1, W3 | worldgen, content | 2 |
| W11 | Water physics: fluid kernel, settled generation; sloped water meshing by the graphics thread | W4 | engine, server, mesh, shaders | 2 |
| W12 | Shader pack loader and Complementary-style pack, puddles (graphics thread) | its LOD rewrite | client render | 3 |
| W13 | Bushy leaves and better grass (graphics thread) | its LOD rewrite | mesh, shaders | 3 |
| W14 | Half-height terrain and smooth steps (meshing by the graphics thread) | W8 | engine, worldgen, client | 3 |
| W15 | Trails | W4 | worldgen | 3 |
| W16 | Ambience: particles and layered sound | W5 | client | 3 |
| W17 | Ground wear | | server, engine chunk data | 3 |

Wave 1 starts now in parallel. Wave 2 items start as soon as what they need is on `main`.
Wave 3's renderer items start when the graphics thread finishes its current pieces
(anti-aliasing, distant terrain), so the two lines of work don't collide.

## Done means

- A seed shows at least 15 distinct biomes within 5 km of spawn, with smooth borders and no
  desert next to snow.
- Every biome has its own blocks, trees and plants. Something fantastical can be found in
  most biomes, and fantasy biomes are rare and striking.
- Rivers start in the hills, flow downhill through lakes to the sea, and fall as waterfalls.
- Big caves and ravines exist but are not everywhere.
- No frame-time regression: generation stays off the main thread, and chunk generation time
  is reported before and after each worldgen item.
- Every new block and plant has its own pixel art and icon.
