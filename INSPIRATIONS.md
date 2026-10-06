# Inspirations

voxmar stands on a lot of other people's work: Minecraft mods, shader packs, libraries and
games whose ideas we studied and made our own. This file credits every one of them whose
ideas or techniques we used, including the ones we copied code from.

It is not the legal record. Code and assets we actually copied or adapted are listed, with
the notices their licences require, in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
Everything else here was used as an idea only: we read how it works and wrote our own
version, with our own code, constants and art.

Each entry gives the author, a link, the licence, what we took and where it lives in our
code. Licences were checked on GitHub, GitLab or Modrinth on 4 October 2026; a licence that
forbids reuse only matters for the entries marked as copied. Most of the research behind
these entries is in the project's research notes (`research/hiking-simulator-mods.md` and
`research/mod-source-deep-dive/`), and the plans that came out of it are in
[docs/world-plan.md](docs/world-plan.md), [docs/rendering.md](docs/rendering.md) and
[docs/weather-effects.md](docs/weather-effects.md).

**Keeping it current.** When you take an idea from somewhere, add or extend its entry here
in the same commit, and credit it where it lands in the code (the existing comments read
"after X", "X's" or "in the spirit of X"). When you copy code or assets, also add the source
to THIRD_PARTY_NOTICES.md with the notice its licence requires, and mark the entry here as
copied.

## Copied code (also in THIRD_PARTY_NOTICES.md)

### Freesound recordists (rain, thunder, water, hail, caves, lava)

- Authors: Garuda1982, Gustavo_C, dflee4, bruno.auzet, TRP, Pingel, cognito perceptu,
  fran_marenco, kyles, Nox_Sound, Tom_Kaszuba, qubodup, ecfike and Fission9
- Link: https://freesound.org (each recording's page is in the notices)
- Licence: CC0 1.0. **Copied**: field recordings, cut, filtered and levelled (see the
  notices for which recording became which file).
- What we took: the sound of rain on leaves, a downpour, rain on a tarp, drops off trees, a
  babbling creek, a lake lapping, a waterfall, hail, a tunnel's air, lava bubbling, close and
  distant thunder, swimming and splashes.
- Where: `assets/content/base/sounds/`, played by `ambience.ron` and `sounds.ron`.

### RoadWeaver

- Author: shiroha-233
- Link: https://github.com/shiroha-233/RoadWeaver
- Licence: MIT. **Copied**: its trail pathfinding cost model and path post-processing,
  rewritten in Rust and retuned for footpaths (see the notices).
- What we took: the potential-field pathfinder's costs (a discount for following
  contours, soft and hard grade limits, a penalty for walking straight down the fall line),
  which make switchbacks fall out of the search; snapping a trail onto a longer one nearby
  and following it until they part, which makes junctions; relaxing the path, drawing it as
  a Catmull-Rom curve, limiting its rise, bridges and ramps, and cut and fill where the
  strongest trail wins.
- Where: `crates/worldgen/src/trails/route.rs`, `network.rs` and `path.rs`.

### Kenney's UI Audio

- Author: Kenney Vleugels (Kenney.nl)
- Link: https://kenney.nl/assets/ui-audio
- Licence: CC0. **Copied**: two recordings, the interface's default Micro switch sounds
  (see the notices).
- What we took: a button click and an item sound, chosen by Mariano from Kenney's UI Audio
  and Interface Sounds packs; our other interface sound sets are synthesized.
- Where: `assets/ui/sounds/`, played by `crates/client/src/ui/sound.rs`.

### FastNoiseLite

- Author: Jordan Peck (Auburn) and contributors
- Link: https://github.com/Auburn/FastNoiseLite (we use the Rust crate `fastnoise-lite`)
- Licence: MIT. **Copied**: its OpenSimplex2 noise, fractal sums and domain warp, ported to
  NEON so four points or octaves are worked out at once (see the notices).
- What we took: all of our noise. Most of generation still calls the crate itself.
- Where: everywhere in `crates/worldgen/src/`; the vectorised port is in `noise.rs`.

### Glimmer

- Author: Josh Britain (jbritain); its screen-space tracer credits a public gist by Belmu
- Link: https://github.com/jbritain/glimmer-shaders
- Licence: MIT. **Copied**: the screen-space reflection march, the refraction and the
  light-through-water model (see the notices).
- What we took: reflections traced through the depth buffer with a binary refinement,
  refraction that keeps the straight view where the bent one lands in front of the water,
  and water as absorption plus in-scattering. It is also where we found afl_ext's waves
  ("Very fast procedural ocean", MIT), which we used until our own wave texture replaced them.
- Where: `crates/client/src/render/water_surface.wgsl` and `water.wgsl`.

### Tectonic

- See its entry under World generation: its terrain splines and regions are adapted in
  `assets/content/base/land.ron` (MIT, in the notices).

### Bevy (bloom)

- Author: the Bevy contributors
- Link: https://github.com/bevyengine/bevy
- Licence: MIT or Apache-2.0. Copied (adapted).
- What we took: its bloom's filters, threshold, chain sizing and blend, rebuilt as one
  compute pass instead of a render pass per level.
- Where: `crates/client/src/render/bloom.wgsl`, `post.rs` and `display.wgsl`.

## World generation

### Tectonic

- Author: Apollo (Apollounknowndev)
- Link: https://github.com/Apollounknowndev/tectonic
- Licence: MIT (stated in its build file and on Modrinth; the repository has no licence
  file). **Copied**: its terrain splines and density-function structure, rewritten as our
  RON (see the notices).
- What we took: the shape of the land. Coasts, mountain ranges and broad valleys lie along
  the zero lines of noises folded with `abs()`, so they form chains and networks instead of
  blobs; heights are stretched by their square so lowlands stay low while ranges rise; a
  terrain classification with short ramps that biome borders follow; terrain shapes switched
  on by climate ramps, never by the chosen biome; and rivers that tunnel under a range
  rather than dead-ending against it.
- Where: `crates/worldgen/src/geology.rs`, `climate.rs` (`landform`), `shapes.rs`, and the
  river tunnels in `water/`.
- Planned in [docs/terrain-foundation.md](docs/terrain-foundation.md) (read from its source,
  3.0.25, on 4 October 2026): its whole terrain router. Large parameter fields with nine
  octaves (continents about 7.9 km across, so coasts are fractal), its offset, factor and
  jaggedness splines with the `offset²` elevation boost, terrain regions chosen by a
  selector noise and climate (plateaus, valleys, rolling hills, two-tier terraces stepping
  down to the river lines, jungle pillars, wind-aligned dunes), mountain detail shifted along
  its own gradient, the trench-and-ridge sea floor, and biome parameters synthesised from the
  terrain classes. Its splines and regions are adapted directly in
  `assets/content/base/land.ron`, run by `crates/worldgen/src/router.rs`.

### Terralith

- Author: Stardust Labs (Starmute)
- Link: https://github.com/Stardust-Labs-MC/Terralith
- Licence: Stardust Labs' own licence, which forbids redistribution. Ideas only; no data
  files or parameters copied.
- What we took: a biome table with no holes, where every climate cell belongs to some biome
  and rare biomes live in the noise tails rather than behind low weights; curves that bend an
  axis for the biome picker only; surface rules as a small language of conditions, including
  lines drawn along noise contours (`|n| < w`) for trails, creeks and rivulets; and forests
  graded from old-growth cores to scrubby edges by bands of one noise.
- Where: `crates/worldgen/src/biome.rs`, `content.rs` (remaps, the coverage check),
  `surface.rs`, `ecology.rs` and `shapes.rs`.
- Planned in [docs/terrain-foundation.md](docs/terrain-foundation.md), from the earlier
  research notes: terrain spline knots placed at the biome table's breakpoints, so a biome's
  region of parameter space has its own landform; and extra terrain added and cut with `max`
  and `min` behind cheap 2D gates (arches, leaning spires that lean by a height-dependent
  shift, sea cliffs cut by re-evaluating the height a little seaward).
- Its licence also forbids using its code or data to configure, test, debug or augment AI
  or other generative systems, so since 4 October 2026 we no longer open its files and work
  from the earlier notes and its public pages.

### Wilder Wild

- Author: FrozenBlock
- Link: https://github.com/FrozenBlock/WilderWild
- Licence: FrozenBlock Modding Oasis License, which restricts reuse. Ideas only.
- What we took: ecotones (thin biomes along the border of two others, such as birch taiga
  between taiga and forest); clearings opened by a global noise with a soft edge, where trees
  fade out first and meadow plants fill in; a snow blanket laid on every level open to the
  sky, down through the canopy; hollowed fallen logs; and three of its tree shapes: snags'
  and junipers' limbs that climb in irregular stairs (each step goes up with a chance that
  grows with how much of the way left is up), the willow trunk that jogs a block sideways
  where it forks, and leaves that hang from the underside of willow crowns.
- Where: `crates/worldgen/src/content.rs` (`EcotoneDef`), `biome.rs`, `ecology.rs`,
  `terrain.rs`, `decor.rs` (the logs) and `trees.rs` (`Grower::walk`, `Grower::curtains`,
  the willow's jog).

### Geophilic

- Author: everloste
- Link: https://github.com/everloste/Geophilic
- Licence: all rights reserved (only its Python tooling is MIT). Ideas only.
- What we took: ground decoration placed in a fixed order of stages (patches of ground,
  rocks, trees, tree ornaments, fallen logs, flowers, fungi, moss, grass), forests that come
  in dense stands with gaps, flowers whose species follows a noise so they grow in drifts,
  and lumpy bushes: a main mound with smaller ones beside it, on a buried stem.
- Where: `crates/worldgen/src/decor.rs`, `ecology.rs` and `trees.rs` (`Grower::mound`).

### Terrestria and Biolith

- Author: TerraformersMC (Biolith also gniftygnome)
- Links: https://github.com/TerraformersMC/Terrestria and
  https://github.com/TerraformersMC/Biolith
- Licence: LGPL-3.0, both. Ideas only.
- What we took: sub-biomes that take a share of a parent biome's land instead of a climate
  box of their own (our variants); a distance to the nearest biome border, and which biome
  lies across it, for every column, which we work out analytically rather than by marching;
  the idea of biome terrain shapes (dunes, canyons, volcanoes) that fade out towards a
  biome's edges; and from its tree placers, the cypress's crown profile (a radius of
  6.25·x(1−x)² along the crown) and roots flaring on the sides of big trunks.
- Where: `crates/worldgen/src/biome.rs` (variants, border distance), `shapes.rs` and
  `trees.rs` (the `Spire` crown, `Grower::flare`).

### Blooming Biosphere

- Author: hhumanoid
- Link: https://modrinth.com/mod/blooming-biosphere
- Licence: CC-BY-NC-SA-4.0. No public source, so only what its pages describe. Ideas only.
- What we took: fallen logs and boulders, a set of flowers per biome, colours tuned per
  biome, and the rule that a feature is placed only where it is supported, after its
  changelog kept fixing logs hanging in the air.
- Where: `crates/worldgen/src/decor.rs` and the biome colours in `assets/content/base/`.

### Streams, TerraFirmaCraft and Streams Reflowing

- Authors: delvr (Streams); the TerraFirmaCraft team; nice.john (Streams Reflowing)
- Links: https://github.com/delvr/Streams and https://github.com/delvr/Farseek-Mods
  (Streams 1.0); https://github.com/TerraFirmaCraft/TerraFirmaCraft;
  https://modrinth.com/mod/streams-reflowing
- Licences: MIT (Streams); EUPL-1.2 (TerraFirmaCraft); all rights reserved and closed source
  (Streams Reflowing, known to us from its changelogs only). Ideas only.
- What we took: rivers that run downhill from the hills into lakes and the sea, the main
  goal of the world plan's water work. Trunk rivers are laid out first at region scale and
  the terrain is cut to them (TerraFirmaCraft's order), streams are routed afterwards on an
  8-block grid, a river's size comes from its catchment times the rain, and lakes fill less
  in dry climates.
- Where: `crates/worldgen/src/water/`.

### Flowing Fluids

- Author: Traben
- Link: https://github.com/Traben-0/flowing_fluids
- Licence: LGPL-3.0. Ideas only.
- What we took: sealing generated water so it is already at rest when it loads, which
  Flowing Fluids found to be a large performance win.
- Where: `crates/worldgen/src/water/mod.rs` and `water/channel.rs` (generated water is a
  fixed point of the fluid rule).

### YUNG's Better Caves and Cave Overhaul

- Authors: YUNG; swankydenizen (Cave Overhaul, the mod by that name on Modrinth)
- Links: https://github.com/YUNG-GANG/YUNGs-Better-Caves and
  https://modrinth.com/mod/cave-overhaul
- Licences: LGPL-3.0; GPL-2.0. Ideas only, from their descriptions.
- What we took: tunnel systems that grow more tangled with depth (Better Caves), and
  domain-warped labyrinths of level passages (Cave Overhaul).
- Where: `crates/worldgen/src/caves.rs`.

### TanTanDev's shrubbery

- Author: TanTanDev
- Link: https://github.com/TanTanDev/shrubbery
- Licence: MIT or Apache-2.0. Ideas only; no code copied.
- What we took: trees as a handful of species parameters in data (trunk height, thickness
  and lean, roots, crown shape), laid out in continuous space and then voxelized. The rest
  of our trees (ages, clumped crowns, tiered conifers, tapering trunks) is our own.
- Where: `crates/worldgen/src/trees.rs` and `assets/content/base/trees.ron`.

### GregTech's ore veins, and StaTech Industry

- Authors: the GregTech CE and GregTech Modern teams (GregTechCEu), after GregoriusT's
  GregTech; StaTech Industry by TheStaticVoid, whose large veins come from Gregic Oregen
- Links: https://github.com/GregTechCEu/GregTech-Modern,
  https://github.com/TheStaticVoid/StaTech-Industry
- Licence: LGPL-3.0 (GregTech Modern), GPL-3.0 (StaTech Industry). Techniques only: no
  code, vein tables or textures were copied; our veins, ores and numbers are our own.
- What we took: ore only in large veins of a few ores that belong together, sited on a
  coarse grid, each kind drawn by weight within a band of heights; a vein's ores layered
  from top to bottom with some scattered through it; and small samples of a vein's main
  ore lying on the surface over it, which point down to it and give a little of the ore
  (`crates/worldgen/src/ores.rs`, `content/base/ores.ron`, the `*_ore_sample` blocks and
  the `Rock` plant model in `crates/engine/src/block.rs` and `voxel.wgsl`). Our own
  additions: veins that favour or shun rock provinces, lens and pipe shapes, and
  `/locate ore`. Motherlodes, huge deep veins to mine for hours from one spot, follow the
  deep veins of Old School RuneScape's Motherlode Mine and GregTech's large veins under
  its drilling rigs.

## Rendering

### Complementary Reimagined, Complementary Unbound and Euphoria Patches

- Authors: EminGT (Complementary); SpacEagle17 (Euphoria Patches)
- Links: https://github.com/ComplementaryDevelopment/ComplementaryReimagined (one codebase
  builds both Reimagined and Unbound); https://modrinth.com/mod/euphoria-patches
- Licence: Complementary License Agreement 1.7 for the packs, and the Complementary License
  1.5 for Euphoria Patches' shader changes. Their installer, EuphoriaPatcher, is MPL-2.0 but
  holds no rendering code. Techniques only: no code, constants or colour tables copied.
- What we took: the look we aim for, and how to get it. Verdant, our default shader pack, is
  a warm, contrasty look after Complementary Reimagined: warm sun against cool sky light, sky
  light that falls off steeply under cover so forest floors are dark, sunlit leaves that glow
  against the sun, cloud shadows, wet ground and puddles. Water is drawn in its own pass over
  the finished scene, as Complementary's is, and can take one wave slope per texel of the
  block so it reads as pixels up close. The world plan's shader-pack system and clouds were
  also planned against these packs. Leaves (judged from the hiking video's frames, not
  their code): light that reaches leaves in a crown's shade where little lies between them
  and the sun, and leaves a lighter, yellower green than the biome colour alone gives.
- Where: `assets/shaderpacks/verdant.ron`, `crates/client/src/render/lighting.wgsl`
  (`shade_normal`, leaves included), `render/pack.rs`, `render/water.rs`,
  `water_surface.wgsl` and `voxel.wgsl` (`biome_colour`, the leaves' colour).

### Distant Horizons

- Author: James Seibel (jeseibel) and the Distant Horizons team
- Link: https://gitlab.com/distant-horizons-team/distant-horizons
- Licence: LGPL-3.0 on Modrinth (GitLab lists GPL-3.0). Ideas only.
- What we took: drawing the land far beyond the loaded chunks, in rings that double their
  cell size with distance, generated straight from the world generator, with distant
  terrain lit with the same inputs as near terrain so the two match.
- Where: `crates/client/src/render/lod.rs` and `lod.wgsl` (docs/rendering.md, "Distant
  terrain, and what Distant Horizons does").

### Voxy

- Author: MCRcortex (cortex)
- Link: https://github.com/MCRcortex/voxy
- Licence: all rights reserved. Ideas only.
- What we took: distant tiles drawn by the terrain renderer rather than as entities, giving
  way to real chunks column by column as they are meshed. We also tried its textured voxel
  tiles for distant terrain and kept our smooth heightfield, because far mountains became
  towers of huge cubes.
- Where: `crates/client/src/render/terrain.rs` (`DistantView`) and docs/rendering.md ("What
  we took from Voxy").

### Alacrity (the look of its grass and flowers)

- Author: Satellence
- Licence: all rights reserved; its terms forbid using any part of it elsewhere. Nothing
  of it was downloaded, opened or viewed: we worked only from a written description of how
  a meadow looks with it.
- What we took: the idea of fine, soft grass and of flowers as rounded clumps of dozens of
  tiny blossoms at several heights rather than one big flower per block. Our own art and
  code: `tools/plants/clumps.py`, `assets/packs/default/recipes/flowers.ron` and the
  `meadow_grass_top` recipe in `soil.ron`.

### Rainbow's Foliage

- Author: PoeticRainbow
- Link: https://github.com/PoeticRainbow/rainbows-foliage
- Licence: no licence file, so all rights reserved. Ideas only.
- What we took: every plant placed from a hash of its cell, turned up to 40° either way,
  shifted, sunk a little and scaled, in tufts across a meadow.
- Where: `crates/client/src/render/voxel.wgsl` (`plant_position`).

### xali's Bushy Leaves and Better Leaves Lite

- Authors: xalixilax; TeamMidnightDust
- Links: https://github.com/xalixilax/xali-s-Bushy-Leaves and
  https://github.com/TeamMidnightDust/BetterLeavesLite
- Licences: custom terms (CC-BY-NC 4.0 per our research) for Bushy Leaves; MIT for Better
  Leaves Lite. Ideas only; none of their images copied.
- What we took: leaves that reach past their block (Bushy Leaves); Better Leaves Lite's
  shape for them, two crossed cards two blocks wide and 1.75 tall at the texture's own
  16 texels a block, the leaf texture running on past the block and cut to a ragged
  round cluster (we cut it in the shader, with our own outline, rather than baking a
  sprite), turned at fixed angles rather than at random.
- Where: the fringe pass in `crates/engine/src/mesh.rs` (`emit_fringe`) and
  `crates/client/src/render/voxel.wgsl` (`card_vertex`, `card_mask`), and the `_bushy`
  recipes in `assets/packs/default/recipes/wood.ron`.

### Grassier Grass

- Author: leonardoinc22
- Link: https://modrinth.com/mod/grassier-grass
- Licence: all rights reserved. Ideas only.
- What we took: real grass blades growing from grass blocks near the camera, rolling across
  a meadow in waves with the wind and parting around the player. From studying how it moves
  them: the motion is mostly a lean that follows a gust field scrolling with the wind, so
  neighbours bend together, with only a small flutter on top whose phase is advanced
  frame by frame on the CPU rather than computed from the clock; each blade gives to the
  wind a little more or less than its neighbours and flutters at its own pace; and the
  gusts show as a lighter sheen on the blades they bend. Our gust field, constants and
  shader are our own.
- Where: `crates/client/src/render/grass.wgsl`, `wind.wgsl` and `wind.rs`.

### Particle Rain

- Author: PigCart
- Link: https://github.com/PigCart/particle-rain
- Licence: MIT. Ideas only; nothing copied.
- What we took: rain streaks laid along their motion as seen from the moving camera, so
  running into rain tilts it towards you; and weather effects as data. Its fog after rain,
  wall streaks and drips are listed as next steps in docs/weather-effects.md.
- Where: `crates/client/src/render/precipitation.rs` and `precipitation.wgsl`.

## Textures

### Alacrity

- Author: Satellence
- Links: https://www.curseforge.com/minecraft/texture-packs/alacrity and
  https://satellence.gumroad.com/l/alacrity
- Licence: all rights reserved; its terms forbid using its assets in other projects or in
  AI-generated work. Ideas from its public text descriptions only: no image of it was
  downloaded, opened, traced or sampled.
- What we took: the direction for Voxmar HD, our 32×32 pack. A 32×32 look that stays
  recognisable while leaning a little fantasy; warmer building materials; soft, hand-made
  vegetation; atmosphere without losing readability. Mariano's notes on a screenshot of its
  grass (a fine, soft grass top with light yellow-green speckle, thin pale blades, flowers
  as dense clumps of tiny blossoms, muted warm tones) became the pack's style guide.
- Where: `docs/voxmar-hd.md` and `assets/packs/voxmar_hd/`.

### Hue-shifted ramps (pixel-art practice)

- Source: common pixel-art teaching, for example Slynyrd's "Pixelblog 1: Color Palettes"
  (https://www.slynyrd.com/blog/2018/1/10/pixelblog-1-color-palettes)
- Licence: an article; the idea only.
- What we took: ramps whose shadows turn towards blue and highlights towards yellow, with
  chroma highest in the middle shades, which is how Voxmar HD warms its stone, earth and wood.
- Where: the `Ramp` palettes in `assets/packs/voxmar_hd/recipes/` (the ramp builder itself is
  `crates/texgen/src/colour.rs`).

## Sound and ambience

### AmbientSounds

- Author: CreativeMD
- Link: https://github.com/CreativeMD/AmbientSounds
- Licence: LGPL-3.0. Ideas only.
- What we took: measuring how enclosed the player is with a capped flood fill from the eye,
  which gives openness, sky exposure and what blocks are near; and sound layers whose
  conditions are soft ranges that fade, ducked rather than muted by rain; and rain as
  recorded layers by strength and cover (open ground, a downpour, under a roof, dripping
  trees), as its rain sounds are.
- Where: `crates/client/src/ambience/enclosure.rs` and `defs.rs`.

### Resonance

- Author: TalonFloof
- Link: https://github.com/TalonFloof/Resonance
- Licence: LGPL-3.0 for the code; its sounds are CC-BY and CC0. Ideas only.
- What we took: long sound beds that crossfade with the surroundings, with one-shots
  scattered over them, played from blocks actually there, so birds sing from real leaves.
- Where: `crates/client/src/ambience/`.

### Particular and Subtle Effects

- Authors: Chailotl (Particular); MincraftEinstein (Subtle Effects)
- Links: https://github.com/Chailotl/particular and
  https://github.com/MincraftEinstein/SubtleEffects
- Licences: LGPL-3.0; custom terms that allow use as a reference only. Ideas only.
- What we took: ambient particles (fireflies, pollen, falling leaves) as a table in data,
  keyed by climate, time and weather; and fireflies as an event that comes out only on some
  nights (Particular's), which we extended to days: pollen and dust come out only on some.
- Where: `assets/content/base/ambient.ron` and `crates/client/src/weather/atmosphere.rs`.

### Designing Sound

- Author: Andy Farnell
- Link: https://mitpress.mit.edu/9780262014410/designing-sound/
- Licence: a book, all rights reserved. Ideas only.
- What we took: its procedural models. Rain as countless short taps of noise over a band of
  broadband noise, never tones; thunder as a sharp crack and tearing followed by a rumble
  made of sound arriving from farther and farther along the bolt, darker with distance;
  running water and splashes as many small bubbles, each a brief tone rising slightly in
  pitch, over the rush of the flow.
- Where: `crates/client/src/sfx.rs` (`rain_loop`, `thunder_clip`, `water`), `sfx/dsp.rs`
  (`bubble`, `tap`) and `crates/client/src/ambience/synth.rs` (`stream`, `plop`).

### Presence Footsteps

- Author: Sollace (originally Hurricaaane)
- Link: https://modrinth.com/mod/presence-footsteps
- Licence: PolyForm Shield 1.0. Ideas only.
- What we took: footsteps layered from what is underfoot: the ground's own step, the rustle
  of plants the feet pass through, splashes in shallow water.
- Where: `crates/client/src/sfx.rs` (`footsteps`).

## The ground underfoot

### The Roads More Travelled

- Author: milkucha
- Link: https://github.com/milkucha/trmt
- Licence: CC-BY-NC-4.0. Ideas only.
- What we took: ground that wears into paths where players walk. Wear spreads to the sides
  and ahead, plants are trampled first, each stage's threshold varies by block, and unused
  ground heals lazily, lone patches twice as fast. We keep wear in the column, so it saves
  and streams with it.
- Where: `crates/server/src/wear.rs` and `assets/content/base/wear.ron`.

## Minecraft

- Author: Mojang Studios
- Link: https://www.minecraft.net
- Licence: proprietary. Nothing from Minecraft is shipped. When a player has Minecraft
  installed, the game can read their own textures and block sounds from it.
- What we took: the voxel grammar the whole game builds on, and many rules matched to it:
  how water and lava flow, how breaking a block cracks and scatters it, its light curve and
  per-face shading, fancy leaves, how music plays now and then, the 1.18 noise caves
  (cheese, spaghetti and noodle caves) and aquifers, and multi-noise biome placement. Its
  interface conventions: the inventory screen's mouse and keyboard handling (click,
  right-click halves, drag to spread evenly or one by one, double-click to gather,
  shift-click between hotbar and pack, number keys to swap), item names coloured by rarity
  in tooltips and above the hotbar, middle-click to pick a block, and the blurred world
  behind menus (1.20.5). Our font and interface art are our own drawings, not Minecraft's.
  Its crops: seeds sown on soil, growth stages, water within four blocks
  keeping the ground moist (`crop::rate`). How players move: sprinting (sprint key or a double tap of forward, kept while moving
  forward, a jump boost and a small widening of the view), jumping from shallow water even
  with nothing underfoot so a bank a block above the water can be reached, with its
  ten-tick jump delay. Dropped items: how they pop from a broken block, fall, bounce,
  slide and float, their pickup delays, reach, merging and five-minute life, the copies a
  big stack shows, the spin and bob, flat items drawn extruded a texel thick, Q and Ctrl+Q
  to throw, and the pickup pop (`entity.item.pickup`, read from the player's install).
- Where: `crates/engine/src/fluid.rs`, `crates/client/src/breaking.rs`,
  `render/cracks.rs`, `render/lighting.wgsl`, `music.rs`, `sfx/minecraft.rs`,
  `textures.rs`, `crates/worldgen/src/caves.rs`, `crates/engine/src/inventory.rs`,
  `crates/client/src/hud/inventory.rs`, `hud.rs`, `interact.rs`, `ui/blur.rs`,
  `crates/engine/src/movement.rs`, `crates/engine/src/dropped.rs`,
  `crates/server/src/items.rs`, `crates/client/src/drops.rs` and `render/items.rs`.
- Planned in [docs/terrain-foundation.md](docs/terrain-foundation.md): the 1.18 noise
  router as the foundation of our terrain. Continentalness, erosion and peaks-and-valleys
  noises define a point at every position; nested cubic Hermite splines turn it into the
  surface offset, the factor (how 2D the land is) and jaggedness; a 3D density with a
  quarter-negative slope above the surface, sampled at the corners of 4 × 8 × 4 cells and
  interpolated; and biomes picked from the same point, in a table of climate rows and
  landform roles. Read from Mojang's world-generation data through misode's mcmeta mirror
  (https://github.com/misode/mcmeta) and the Minecraft Wiki's noise router and density
  function pages (CC BY-NC-SA); techniques only.
- Taken so far (terrain steps 1 to 3): the router itself (`crates/worldgen/src/router.rs`,
  with Tectonic's splines in `land.ron`), the relief worked out on a 4-block lattice and
  interpolated (`geology.rs`), biomes picked by a humidity that does not follow
  temperature, as vanilla's humidity noise is independent of its temperature noise
  (`HumidityDef` in `content.rs`, `climate.rs`), and the 3D density around the surface,
  falling more slowly above it than below (vanilla's "quarter negative"), on a lattice
  4 blocks across and 8 up (`density.rs`).

## Farming and skills

The research behind these is in `research/farming.md` (in the worktrees folder, beside
the other research notes). Ideas only: no code, numbers tables or art were copied.

### IndustrialCraft 2 crops and CropsNH (GregTech: New Horizons)

- Authors: the IC2 team; CropsNH by the GTNH team (bartimaeusnek's Crops++ before it)
- Link: https://github.com/GTNewHorizons/CropsNH
- Licence: LGPL-3.0 (CropsNH); IC2 is all rights reserved. Techniques only.
- What we took: a crop's growth rate as a product of separate environment factors
  (climate, soil, water, light), a few visual stages with the time held in hidden progress,
  and the idea of tiers gating what grows where (`crates/engine/src/crop.rs`, `rate`).
- Also: a long list of plants, each with properties other systems read, rather than a
  handful of decorative flowers: every foraging plant carries nutrition, alchemy and dye
  tags (`blocks.ron`) for cooking and alchemy to use.

### Farmer's Delight

- Author: vectorwing
- Link: https://github.com/vectorwing/FarmersDelight
- Licence: MIT. Techniques only.
- What we took: wild crops placed by climate as the first source of a crop's seeds
  (`wild` and `forage` in `content/base/crops.ron`, `CropRegistry::wild_form`).
- Also: wild crops in small clumps of one kind rather than single plants scattered
  evenly, which is how the foraging plants grow (`spread: Drifts` in `plants.ron`).

### AgriCraft

- Authors: InfinityRaider and contributors
- Link: https://github.com/AgriCraft/AgriCraft
- Licence: MIT. Techniques only.
- What we took: a climate band a crop is comfortable in that fades its growth to nothing
  over a margin rather than stopping dead at its edge (`crop::rate`).

### Pam's HarvestCraft

- Author: MatrexsVigil
- Link: https://github.com/MatrexsVigil/harvestcraft
- Licence: all rights reserved. Techniques only.
- What we took: picking a ripe crop by hand keeps the plant, which goes back to an
  earlier stage to fruit again (`regrow` in `crops.ron`, `Server::pick` in
  `crates/server/src/farm.rs`).
- Also: the breadth of a forager's world. A garden in every climate with a dozen kinds of
  wild vegetable, herb, spice and berry, so walking somewhere new turns up something new
  to gather: the foraging plants in `content/base/plants.ron` (wild onion, turnip, beans,
  rice, rosemary, cumin, cranberry and the rest), drawn in our own art in
  `assets/packs/default/recipes/forage.ron`.

### Croptopia and Fruitful Fun

- Authors: thetrdev (Croptopia); Snownee (Fruitful Fun, formerly Fruit Trees)
- Links: https://github.com/ExoPlant/Croptopia, https://github.com/Snownee/FruitfulFun
- Licence: all rights reserved (both). Techniques only.
- What we took: fruit as a leaf block of the tree's own, a share of a crown's leaves,
  going from bare through blossom and unripe to ripe and back when picked, rather than a
  block of its own hung in the tree (the `fruit_leaves` template in `blocks.ron`, fruit
  crops in `crops.ron`, the untinted fruit overlay over tinted leaves in
  `crates/client/src/main.rs`).
- Also (Croptopia): crops and fruit bushes spread by climate, tropical produce such as
  pineapple, coffee and vanilla alongside temperate ones, which shaped the foraging list's
  climate spread in `content/base/plants.ron`.

### RuneScape (Old School)

- Author: Jagex
- Link: https://oldschool.runescape.wiki/w/Farming
- Licence: proprietary. Ideas only.
- What we took: skills with levels 1 to 99 on its experience curve (`curve: RuneScape` in
  `content/core/skills.ron`, `crates/engine/src/skill.rs`), crops gated by the level they
  take, experience for planting and harvesting, yields that rise with level between a
  crop's level and 99 rather than growth that speeds up (`crop::level_bonus`), and
  experience drops rising at the side of the screen (`crates/client/src/hud/skills.rs`).

## Creatures and their behaviour

The research behind these is the creature AI deep dive and the mob survey
(`research/creature-ai.html` with its notes in `research/creature-ai/`, and
`research/mob-mods.html`, in the worktrees folder). Ideas only: no code, data or art was
copied from any of them. The design is in [docs/creature-ai.md](docs/creature-ai.md),
[docs/creatures.md](docs/creatures.md) and [docs/drops.md](docs/drops.md).

### Minecraft (creatures)

- Author: Mojang Studios. Licence: proprietary. Ideas only.
- What we took: the Brain's memories that expire and stand in for cooldowns, the random
  stroll and look-around idles, panicking when hurt, animals following a player who holds
  their food, the fox's crouch and pounce, the per-species overrides of one block-cost
  table, forgetting an unseen target, revenge on whoever struck it.
- Where: `crates/engine/src/creature/brain.rs` and `brain/modules.rs` (wander, look
  around, tempted, flee, hunt's pounce), `crates/server/src/creatures.rs`.

### Old School RuneScape (creatures and combat)

- Author: Jagex. Licence: proprietary. Ideas only.
- What we took: monsters aggressive only to players up to twice their combat level,
  tolerance after ten minutes in one small area, home and leash ranges, the combat level
  formula, accuracy rolled from attack against defence levels, four experience a point of
  damage with a third of it in Hitpoints, and drop tables with always, common and rare
  entries.
- Where: `brain/modules.rs` (`Hunt`), `crates/server/src/creatures.rs`
  (`combat_level`, `player_strikes`, tolerance), `crates/engine/src/creature.rs`
  (`hit_chance`), `crates/engine/src/loot.rs`.

### Rain World

- Authors: Videocult (Joar Jakobsson, James Therrien). Licence: proprietary. Ideas only.
- What we took: a personality of six traits rolled from the creature's id and pushed away
  from the middle, modules that each claim urgency, nervous creatures that freeze and
  watch, and a relationship table by kind (planned).
- Where: `brain.rs` (`Personality`), `brain/modules.rs` (`Freeze`, `StandToLook`).

### Zoo Tycoon 2, and Dave Mark's Infinite Axis Utility System (Guild Wars 2: Heart of Thorns)

- Authors: Blue Fang Games; Dave Mark and Mike Lewis (Game AI Pro 3, ch. 13, on Heart of
  Thorns). Licence: proprietary games and published talks. Ideas only.
- What we took: dual utility, with rank bands chosen first and a weighted draw among the
  near-best inside a band, a bonus for the running choice, and behaviours as a library of
  decisions with considerations that species pick from.
- Where: `brain.rs` (`think`, `Band`), `brain/modules.rs`.

### Untamed Wilds

- Author: Rezzolarian. Licence: GPL-3.0. Ideas only.
- What we took: herds and sentinels, and one power number for the food web (planned).
- Where: `brain/modules.rs` (`Herd`, `Sentinel`); the food web in docs/creature-ai.md.

### Dwarf Fortress, RimWorld, Don't Starve and Ark: Survival Evolved

- Authors: Bay 12 Games; Ludeon Studios; Klei Entertainment; Studio Wildcard. Licences:
  proprietary. Ideas only.
- What we took: species tokens and personality facets with species medians (Dwarf
  Fortress); wildness, taming that decays and the manhunter chance on a failed tame
  (RimWorld); treatment that shapes what a tamed animal becomes (Don't Starve's beefalo);
  named temperaments such as skittish, curious, territorial and patient (Ark). The taming
  plans in docs/breeding.md and the temperament axes in `creature.rs`.

### Minecraft creature mods (mob survey)

- Alex's Mobs and Alex's Caves (AlexModGuy, no licence shown), Animania (LGPL-3.0),
  Farmer's Delight's add-ons, Ice and Fire (LGPL-3.0) and the others in the survey.
  Ideas only.
- What we took: drops carrying food traits so recipes match on kind, a diet record per
  species, products on timers, butchering with a tool for better drops, pests that want
  your food, and many small, specific behaviours as what makes creatures feel alive.
- Where: `assets/content/base/creatures.ron` and `blocks_creatures.ron` (tags), `loot.rs`
  (tool bonuses), docs/creatures.md and docs/food-and-cooking.md.

## The hiking video

- "Turning Minecraft into a Hiking Simulator", by GOATED (27 August 2026):
  https://www.youtube.com/watch?v=axb24q2YrdQ
- Most of the mods above came from its mod list, and its frames are the visual target we
  compare captures against (`voxmar-reference/hiking-video-frames/`, outside the repo; not
  redistributed). The shader packs in those frames are mostly Complementary Reimagined, with
  BSL, Photon and Solas in places. How thick rain makes the air also follows the video
  (`crates/client/src/render/atmosphere.rs`).

## Games that shaped the design

[docs/design-philosophy.md](docs/design-philosophy.md) names what each of these got right
and where it hits a wall:

- **Minecraft**: a universal voxel grammar and a huge mod scene.
- **No Man's Sky**: endless variety of places, and the lesson that generating nouns is not
  enough; we aim to generate decisions.
- **Factorio**: deep, interlocking production, and simulation that stays fast through
  sleeping entities and merged belt lines.
- **RuneScape**: interlocking skills.
- **Roblox**: low-friction sharing of what players make.

## Algorithms and papers

General techniques we use under their usual names, credited where they appear in the code:

- Binary greedy meshing (popularised by cgerikj's `binary-greedy-meshing`, MIT, and
  TanTanDev's demo; our mesher is our own): `crates/engine/src/mesh.rs`.
- Grid traversal for raycasts, after Amanatides and Woo (1987): `crates/engine/src/physics.rs`.
- The PCG hash, after Jarzynski and Olano (2020): `crates/engine/src/weather.rs`,
  `crates/client/src/render/clouds.wgsl` and `voxel.wgsl`.
- SplitMix64 for seeds and hashing: `crates/worldgen/src/rng.rs` and elsewhere.
- Schlick's approximation for the reflectance of water: `crates/client/src/render/water.wgsl`.
- Bloom by a chain of 13-tap downsamples and tent upsamples, after Jimenez's "Next
  Generation Post Processing in Call of Duty: Advanced Warfare" (SIGGRAPH 2014), with Brian
  Karis's weighting against fireflies, as Bevy implements it (adapted; see
  THIRD_PARTY_NOTICES.md): `crates/client/src/render/bloom.wgsl` and `post.rs`.
- Percentage-closer soft shadows: `crates/client/src/render/shadows.wgsl`.
- The Gabriel graph for which places trails join: `crates/worldgen/src/trails/network.rs`.
- Catmull-Rom curves for trails and river channels: `trails/path.rs` and `water/channel.rs`.
- Pink noise from white with Paul Kellet's economical filter (musicdsp.org), and the
  resonance of an air bubble in water after Minnaert (1933): `crates/client/src/sfx/dsp.rs`.
- Space colonisation for branching, after Runions, Lane and Prusinkiewicz, "Modeling Trees
  with a Space Colonization Algorithm" (Eurographics Workshop on Natural Phenomena, 2007):
  grown tree crowns in `crates/worldgen/src/growth.rs`, used by `trees.rs`.
- The Oklab colour space, after Björn Ottosson (2020), for shading texture palettes and for
  how well colours sit together when biomes choose their trees, plants and ground blocks:
  `crates/texgen/src/colour.rs` and `crates/worldgen/src/hue.rs`.

## Read, but not used yet

These were researched and may shape later work; nothing of theirs is in the game yet.

- **Terrain Slabs** (Coun7ered, https://github.com/Coun7ered/terrain_slabs_multiloader) and
  **StepUp Camera Smoother** (chedidandrew, MIT, https://github.com/chedidandrew/StepUp-Camera-Smoother):
  half-height terrain and smooth stepping, planned in the world plan.
- **Fast Better Grass** (robotkoer, MIT, https://modrinth.com/resourcepack/fast-better-grass),
  **Flowering Crops**, **Continuity** and the **Reimagined** texture pack: texture ideas
  planned for our art.
- **Respawning Animals** (Fuzss, MPL-2.0, https://github.com/Fuzss/respawning-animals): a
  wildlife budget per player, for when creatures exist.
- **Sound Physics Remastered** (henkelmax, GPL-3.0), **Vervada's Enhanced Plants**
  (CC-BY-SA 4.0) and the 3D plant packs: read for ideas.
- **Immersive Cavegen**, **YUNG's Cave Biomes**, **Profundis** and **Alex's Caves**: named in
  the world plan's cave biomes.
