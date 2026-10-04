# Rendering: where the time goes and what comes next

Measured on an M5 Max, 1080p and Retina, view distance 16 (1,089 columns, ~3,500 chunk
meshes, 3–4 million quads), plus distant terrain rings out to ~6.6 km.

- All real terrain together costs about 1 ms per frame. Shrinking the view to 2 columns
  only takes the frame from ~3 ms to ~1.9 ms.
- The rest is fixed per-frame cost: the render graph, HDR and bloom (~0.3 ms), cascaded
  sun shadows (~0.4 ms).
- Resolution barely matters (1080p and 3024×1964 are within a few percent), so we are not
  fill-bound.
- A depth prepass with GPU occlusion culling measured ~0.3 ms *slower* in open and
  forested terrain, because it draws opaque geometry twice and the chunk visibility graph
  already hides caves. It is off; the voxel shader still supports the prepass (shadows
  use it).
- wgpu's indirect-draw validation is already disabled by Bevy in release builds.

## Compared with "I Built a Custom GPU-Driven Rendering Pipeline (10x Faster)"

| Technique in the video | voxmar today |
| --- | --- |
| 1. Render from raw vertex/index data, not engine mesh objects | Yes (`render/terrain.rs`): chunk meshes live in shared pages with no entity per chunk, and a table of chunk origins is indexed by instance. Remeshing replaces one chunk's range. |
| 2. Batch chunks with indexed indirect drawing | **Metal has no native multi-draw** (wgpu loops one draw per chunk), so instead each view's visible chunks are gathered into one index buffer on the GPU and drawn once per page and stream (see "Drawing chunks" below). |
| 3. GPU frustum culling | CPU culling per view (cameras and shadow cascades), in parallel, ~0.05 ms; the GPU only copies what survives. |
| 4. Hierarchical (octree) culling | Not needed yet: per-chunk tests for ~3,500 chunks are negligible, the visibility graph culls through terrain, and LOD rings keep the chunk count constant as the view grows. |
| 5. Paged geometry buffers | Yes: 32 MB vertex and 24 MB index pages with first-fit range allocation. |
| 6–11. Vertex compression, 48 → 16 bytes | **8 bytes per vertex**: normal from a 3-bit face id, UVs from position, 12-bit texture layer, 2-bit tint kind (the colour is looked up per block, see "Biome colours"), AO and light bit-packed. |
| Not in the video | Binary greedy meshing, cave culling, distant LOD rings, meshing and lighting on worker threads with a per-frame upload budget. |

## Drawing chunks

Each chunk's mesh is split into streams, each with its own pages: opaque faces by the
direction they face, cut-out faces, plants, inner leaf faces, water and ice, and leaf
fringes. Per view, culling then skips whole streams per chunk: the three opaque
directions turned away from a camera (or from the sun, for a shadow cascade), which is
half of all opaque faces; plants beyond 320 blocks, where they have sunk away; fringes
beyond 96.

Drawing a chunk at a time cost a draw call per chunk and stream: about 4,700 for the
camera and 1,900 for the shadow cascades at view distance 32, which wgpu and the Metal
driver turned into milliseconds of CPU time. Instead:

- Pages hand vertices out to chunks in blocks of 64, and keep a table of which chunk owns
  each block. A page's indices count from the start of the page, and the vertex shader
  finds its chunk's origin through the table, so any set of chunks in a page can be drawn
  with one draw call.
- Culling writes, per view, where each surviving chunk's indices go in that view's own
  index buffer, and a compute pass (`terrain/compact.wgsl`) writes them there, a thread
  per quad. Pages keep no indices at all: a quad's four vertices are consecutive, and
  the pass splits it along its darker diagonal from the ambient occlusion of its corners,
  exactly as the mesher does, which saves 24 of every 56 bytes of chunk geometry.
  Translucent chunks are gathered back to front. When neither a view's chunks nor their
  meshes changed since the last frame, the pass is skipped.
- Each view then draws once per page and stream: about 100 draws for the camera and 5–10
  per shadow cascade at view distance 32.

## Far chunks in less detail

Most of a large view's faces are far away: at view distance 32, three quarters of the
real chunks lie beyond 256 blocks, where a block is a few pixels wide. Those chunks are
meshed with two or four columns merged into one along x and z (`mesh_chunk`'s `detail`,
`LodGrid`): each layer of a group of columns shows its most common solid block when at
least half the group is solid, and air otherwise, keeping pillars such as tree trunks so
crowns do not float. Heights stay exact, so far chunks meet near ones without steps in
the ground or the water; a face at a chunk border is only hidden by a block that every
level of detail shows there, so chunks at different levels never leave holes between
them. Far chunks grow no plants, fringes or grass blades.

The Far detail setting picks where the levels change: Balanced (the default) keeps full
detail out to 10 columns and half detail out to 20, Fast switches at 6 and 12, and Full
keeps every block. A column keeps its level until it is a column past a boundary, so
walking back and forth does not remesh. At view distance 32, Balanced draws about 2.2×
fewer quads than Full, and loads much faster, since fewer quads are uploaded.

## Where a frame goes now

Moving terrain off Bevy's mesh entities and trimming Bevy to the plugins in use took a
dense view from 3.25 to 2.8 ms (and flights from 2.6–2.9 to 2.4–2.5 ms median). What
remains is mostly Bevy's fixed per-frame cost, not terrain:

- Render thread ~2.9 ms: ~1.5 ms of render-world systems and their scheduling before the
  graph runs, ~1.3 ms encoding the graph (terrain draws, shadows, bloom, UI).
- Main thread ~1.4 ms, which overlaps the render thread.
- Uploading streamed meshes is budgeted at 32k quads a frame (~0.5 ms at most).

## Distant shimmer and anti-aliasing

Flicker in the distance came from real chunks 50–500 blocks out, not from the distant
terrain: leaf and plant cut-outs passing or failing whole pixels, block textures sampled
nearest-texel when minified, and sub-pixel grass. Now:

- Block textures blend when minified (linear min and mip filtering; magnification stays
  nearest, so texels are crisp up close).
- Anti-aliasing is a setting: Off, FXAA, SMAA or MSAA 4× (the default). With MSAA, leaves
  and plants use alpha to coverage, so their cut-outs fade over samples instead of
  flickering. MSAA costs about 0.2 ms at 1080p.
- Plants sink into the ground between 192 and 320 blocks.

Measured as pixels that change by more than 60 levels when the camera moves 4 cm: 0.33%
before, 0.12% now.

A far waterline under MSAA showed a row of black dashes with bright blue glints that
crawled as the camera moved. With MSAA the fragment shader runs once per pixel, at the
pixel's centre even where a triangle covers only a corner of it, so its inputs are
extrapolated past the triangle. Far water and terrain are slivers seen edge on, and
values extrapolated that far run wild: sky light past full (which `light_curve` turns
into infinity), negative colours and water depths, a position off the surface that lights
a glint. The resolve turned them into black pixels and bloom into blue blobs, all along
the edge of the real water where it meets the distant sheet. The lighting inputs of
terrain, water and distant terrain are now interpolated at the centroid of the covered
samples (`@interpolate(perspective, centroid)`), which always lies inside the triangle.
Texture coordinates stay at the pixel centre, where the derivatives that pick the mip
level are exact. With the clock stopped and a 4 cm step at
render distance 32, looking over the sea at a far shore (seed 3), 0.17% of pixels
flickered before and 0.025% after, and none of those left are on the waterline; a high
view over land stayed at 0.045%.

## Dark teeth on terraced ground

Beaches and grass ledges are staircases of one-block steps with jagged edges. Where an
edge jogs, a one-block riser faces along the step; seen from above, the tread in front
hides its lower half, so it shows as a triangle standing out of the ground (the mesher
was checked: every face is where it should be). Facing away from the sun, it was lit by
the sky alone, with Verdant's deep shade several times darker than the sunlit sand
beside it, so these triangles looked like dark teeth. Walls and undersides now also get
the sunlight the open ground around bounces onto them (`GROUND_ALBEDO`, 0.3, half of it
for a wall), which lifts shaded sides next to sunlit ground as real ones are lifted; the
shader pack's `bounce` scales it.

## Next steps, in order of payoff

1. **Quad pulling.** Store each quad once (8 bytes) and expand it to vertices in the
   vertex shader (5× less vertex memory than now), and let a compute pass cull chunks
   against the frustum and last frame's depth pyramid into a compacted list, so a page is
   one draw per pass. That removes the remaining per-chunk draw calls on Metal and makes
   32+ chunk view distances practical.
2. **Light out of the vertex.** Sample sky and block light (and derive AO) from a small
   per-chunk 3D texture. Greedy merging then only has to match textures, which typically
   cuts quads by 2–3×.
3. **Larger real view distance**, once 1 and 2 are in.

## Distant terrain, and what Distant Horizons does

Distant Horizons (DH), read from its source (`distant-horizons-core`, 2025):

- **Data.** Columns of vertical runs (block, biome, top and bottom y, sky and block light),
  in a quadtree where each detail level doubles the column width. Data comes from chunks
  the game has generated, or from running the generator's early stages far away.
- **Geometry.** Always boxes: top, bottom and side faces per run, with faces between
  neighbouring columns culled, at every level. Silhouettes stay blocky.
- **Colour.** One colour per block: its texture's average times the biome tint. An
  optional noise texture varies brightness per block so flat colour reads less like
  plastic; recent versions can also texture faces from an atlas.
- **Lighting.** Each vertex carries sky and block light and looks it up in Minecraft's own
  light map, and vanilla's per-face shade is baked into the colour, so day, night and
  dusk match the real terrain by construction. Ambient occlusion is a screen-space pass
  over the LOD depth buffer.
- **Seam.** LODs nearer than the vanilla render distance are discarded (optionally
  dithered), and a fade pass blends the far edge of the vanilla chunks into the LOD image
  by distance.
- **Fog.** Its own pass over LOD depth: distance fog plus height fog, coloured like the
  game's fog. Shader packs (Iris) get separate `dh_terrain`, `dh_water` and `dh_shadow`
  programs and DH's depth buffers, and apply the same lighting, water and fog to LODs as
  to terrain; that is what makes DH look seamless with shaders.

What voxmar takes from it, and where it differs (`render/lod.rs`, `render/lod.wgsl`):

- Distant terrain uses the real terrain's lighting inputs: the shared `shade_normal`, the
  same face shade table, sky light that fades under water two levels per block. Colours
  are texture averages times the biome's grass, foliage and water colours, which the
  generator's surface samples carry (`SurfaceSample::colors`), so they match real terrain
  once its chunks take the same colours. The generator is the one the world was made with,
  content packages included (`render::lod::WorldGenerator`, shared with anything else on
  the client that samples the world far away).
- Tiles stay smooth heightfields rather than boxes: a quarter of the triangles of
  4-block columns, no cracks, and 4-block columns would be a coarser staircase than the
  real one beside them anyway. Instead the shader renders what the one-block staircase
  would show from the eye: treads, risers in the side colour, ambient occlusion in inner
  corners, grass plants in front of the steps on grassland, crowns over forests (tops and
  sides, hiding the ground at low angles). Near the real chunks the risers resolve into
  the same contour lines real terrain shows; further out they average out. This is
  analytic, so it does not shimmer, and it costs no geometry.
- Up to where a tree spans about two pixels, forests resolve into single crowns: blobs of
  smooth noise five blocks across, as many as the canopy's cover, domed so they are lit
  on the sun's side, over ground darkened by their shade. Open ground gets patches a few
  blocks across, a little lighter and darker, as real ground is with its mix of blocks.
  Both fade by the pixel's area (so a crown shows where it spans several pixels across
  even if only one deep) and soften their edges by the pixel's longer side, so they blur
  rather than shimmer: flicker over distant forest and hills stayed at 0.27% and 0.20%.
- Water and ice are a sheet over the true floor at the level of their own water (the sea,
  or a lake's or river's), shaded like real water: same
  ripples near the real chunks fading into their average (a broader glint, fresnel facing
  the eye by the ripples' slope variance), sky reflection, the floor dimmed by depth.
- No fade pass is needed: tiles come from the same generator functions as the chunks,
  and finer detail hides coarser detail exactly in the fragment shader. Skirts face
  outwards and sunk terrain fills the first cell under finer detail, so edges never crack.
- Fog is one aerial perspective function for chunks, tiles and the sky's horizon:
  exponential haze densest at sea level and thinning with height, glowing around the sun
  like the sky, plus a fade over the last 40% of the distant terrain.

Measured with the same poses at view distance 8 and 16, so the band 256–512 blocks is
real terrain in one capture and distant terrain in the other: the mean colour difference
over eight regions fell from 10–17 to 4–6 levels per channel (of 255).

## Plants

Every grass, fern and flower is placed as a whole in the vertex shader from a hash of its
cell, in the manner of Rainbow's Foliage: turned up to 40° either way, shifted up to 0.15
blocks, sunk up to 0.25, and scaled 0.6–1.25 in height, with a smooth noise making
tufts of taller and shorter plants across a meadow. Stacks (tall grass, sugar cane) are
shaped as one plant. The mesher marks each plant vertex with its corner (in the AO bits,
which plants do not use) and how many plant cells it stands on (bits 25..27), so no extra
vertex data is needed. Model variants with their own textures can come from block data
later; the placement already varies every plant.

Leaf blocks on the outside of a crown also get a fringe (bushy leaves): two crossed quads
through the block, turned at random, grown 1.45–1.9× and leaning, in their own pass
(`TerrainPass::Fringe`, mesh bit 27). Chunks farther than 96 blocks skip that pass, and
within that the sprigs shrink back into their block from 56 blocks, so they cost nothing
in the distance.

Grass blocks open to the air also grow blades, in the spirit of the Grassier Grass mod.
The mesher records one 4-byte seed per such block (`ChunkMesh::blades`: position in the
chunk and the light above it), and all seeds share one storage buffer. Chunks within 34
blocks of the camera draw up to 16 blades per seed with no vertex buffer: `grass.wgsl`
pulls each vertex's seed by its index and places, sizes, colours and bends the blade from
hashes of the block and blade. An index buffer repeating one blade's three triangles over
its five points lets the GPU shade each point once. Blades thin out and shrink into the
ground between 16 and 34 blocks, and each chunk draws only as many blades per block as
survive at its nearest point, so distant grass costs little: at view distance 32 the
blades went from about 2.5 ms of GPU vertex time to a fraction of that. They are darker
at the root and lighter at the tip, receive shadows but cast none, and part around the
player's feet.

## Biome colours

Grass, foliage and water take their colour from the biome data (`Column::colors`, one set
per block, blended across borders by worldgen). The renderer keeps every loaded column's
colours in one table (three words a block) and the fragment shader looks up the colour of
the block a face belongs to, so colours change block by block even across a merged face,
and the vertex carries only the kind of tint. Greedy meshing no longer has to split faces
where the climate changes.

## Particles and breaking blocks

`render/particles.rs` is a general particle renderer: quads facing the camera that show a
square piece of a block texture (tinted like its face) or a soft speck of colour, lit like
the terrain (`shade_normal` with the sky and block light they were spawned with) or
glowing. A particle is 64 bytes, written to a ring of 8,192 on the GPU once, when it is
spawned; the vertex shader works out where it is from where and when it started, so live
particles cost nothing on the CPU per frame, and the whole ring is one instanced draw
(six vertices per slot, the dead collapsing to a point) in the alpha-tested pass, only
while any particle is alive. Being alpha-tested and depth-writing like leaves, particles
need no sorting and are hidden by terrain correctly; they fade at the end of their life
through alpha to coverage under MSAA and a dither otherwise.

How a particle moves is its `Motion`, a case in `particles.wgsl`. The one so far,
`Ballistic`, is Minecraft's terrain particle worked out in closed form: thrown with a
velocity, falling at 16 blocks/s² (0.04 per tick²), slowed across by 2% a tick, then
hopping on its floor three times at 0.35 of its speed and sliding to rest. Walls are not
collided with: a piece flying into a block is hidden inside it. The floor is found once at
spawn, below where the piece will come down (`breaking::landing_floor`), so pieces thrown
off a ledge fall to the ground beneath it.

The ambient particle sets (pollen, fireflies, spores, drifting snow, mist, motes) stay in
`precipitation.rs`: a fixed field of particles wrapping around a box that follows the
camera costs no CPU at all and has no spawn rate to manage, which suits a steady haze of
particles everywhere better than spawning. Particles tied to events or places (splashes,
falling leaves from a particular tree, sparks) belong here.

Breaking a block (`client/src/breaking.rs`) uses it as Minecraft does:

- **Cracks** (`render/cracks.rs`): ten stages over the block being mined, from the texture
  packs' `block/destroy_stage_0..9` (a Minecraft install has them) or drawn by us. A cube a
  hair larger than the block multiplies what is behind it by twice the texel, Minecraft's
  crumbling blend: mid grey leaves the block's own texture and light alone, and the
  factor is raised to 2.2 so on our linear image it darkens as much as on Minecraft's
  gamma-encoded one.
- **Chips**: a dozen pieces a second pop from the face being hit, at a fifth of a burst's
  speed, from the hit face's texture.
- **Burst**: when the block breaks, 4×4×4 pieces (3×3×3 from a narrower box for plants) of
  a quarter of its side texture, each picked among the mostly opaque quarters
  (`TexturePieces`), so a flower breaks into petals and leaves rather than empty texels;
  0.1–0.2 blocks wide, living 0.5–1.5 s. Blocks broken by others (an unpredicted
  `BlockChanged` to air) burst too, within 32 blocks.

Particles are lit with a sky light estimated once per burst from the columns around the
block, by the engine's rule (straight down from the sky, less through water and leaves,
stopped by solid blocks, then one level less per block sideways, up to three), and the
block's own glow as block light, since the client keeps no light field outside the
mesher. A cave lit by torches therefore
gets dark pieces; a light field kept from meshing would fix it.

The particle setting (All, Fewer, Off) and the shader pack's `particles` scale how many
are spawned, and the pack's `cracks` how strongly cracks show (0 hides them).

Cost: with no particle alive nothing is drawn. A full ring (8,192 live particles, most of
them in view in front of the camera) measured under 0.1 ms a frame, drawing and not
drawing it on alternate halves of each second in one run so the load of the rest of the
machine fell on both alike. A 64-piece burst takes about 12 µs of CPU when the block
breaks (the sky light estimate and finding each piece's floor), and 4 KB of upload.

## Wind

The wind is part of the weather the server simulates and comes to the client in the
player's weather report: a mean wind (direction and speed) and how much it gusts. It is
light under settled skies, freshens and veers as fronts pass (where moist and dry air
masses meet), blows hard in storms, picks up in the afternoon, and is stronger on open
heights and over water and calmer in forests and valleys. `/wind calm|breeze|strong|gale
[from] [seconds]` forces it for testing.

The client exposes it as the `weather::Wind` resource, next to `ClientWeather`, eased
like the rest of the weather and updated every frame (`Update`):

```rust
pub struct Wind {
    pub direction: Vec2, // unit (x, z) the mean wind blows towards
    pub speed: f32,      // mean, blocks/s: <2 calm (most of the time), 2-5 light breeze,
                         // 5-9 breezy (uncommon), 9-14 strong (rare), 15+ gales in storms
    // gustiness: about 0.05 calm, 0.2-0.35 on a breezy day, 0.6-1 in squalls and storms
    pub gustiness: f32,  // 0..1
    pub seed: u64,       // seed of the gusts, the server's
    pub seconds: f64,    // world time now (the server's simulation clock), what gusts run on
    pub offset: Vec2,    // how far gust fronts have travelled with the wind, wrapped every
                         // engine::weather::GUST_WRAP (40960) blocks
}
impl Wind {
    pub fn mean(&self) -> Vec2;                     // direction * speed
    pub fn at(&self, at: Vec2, seconds: f64) -> Vec2; // the wind at (x, z) and a time, gusts included
    pub fn now(&self, at: Vec2) -> Vec2;            // at(at, self.seconds)
}
```

Gusts come from `voxmar_engine::weather::gust`, which the server uses too, so swaying
grass, blowing particles and later gameplay agree. Gust fronts about 40 blocks across
sweep along with the wind and change over a few seconds; speed swings from about 0.75 to
1.75 times the mean at full gustiness, with a little sideways veer. How far the fronts
have travelled is the mean wind integrated over time (`offset`): the server keeps it for
each player and sends it with the weather, and the client advances it every frame and
eases it onto each report, so the fronts never leap when the mean wind changes. The
noise lattice repeats every 1024 cells (40960 blocks), so the offset wraps unseen. For a
shader, the function is (all of it 32-bit, so it ports exactly; `pcg` is the hash already
in `precipitation.wgsl`):

```wgsl
// seed = (seed ^ (seed >> 32)) as u32 ^ 0x67757374u, computed on the CPU.
fn value_noise(seed: u32, p: vec2<f32>, t: f32) -> f32  // as engine::weather::value_noise:
//   lattice cell = floor(p) mod 1024 (x and z), floor(t); corners hash
//   pcg(seed ^ pcg(x ^ pcg(z ^ pcg(t)))) with x, z = (cell + 0/1) mod 1024 and t a u32
//   wrapping add of 0/1; value (h >> 8) / 2^24, smoothstep-weighted trilinear blend.
fn gust(at: vec2<f32>, seconds: f32, offset: vec2<f32>, mean: vec2<f32>, gusts: f32) -> vec2<f32> {
    let p = (at - offset) / 40.0;           // `at` relative to a nearby multiple of 40960
    let t = seconds / 5.0;                  // pass seconds wrapped, for f32 precision
    let s = value_noise(seed, p, t);
    let veer = value_noise(seed ^ 0x9e3779b9u, p, t * 0.7) - 0.5;
    let speed = 1.0 + gusts * (s * s - 0.25);
    return mean * speed + vec2(-mean.y, mean.x) * (veer * 0.4 * gusts);
}
```

Rain, snow and the other falling and drifting particles (`precipitation.rs`) already move
with `ClientWeather::current.wind`, which is `Wind::now` at the camera; clouds drift with
the mean wind.

Grass blades, plants and leaves bend with `Wind`'s mean and gustiness (`render::wind`,
`wind.wgsl`, bound with the terrain so shadows bend alike): a push along the wind from
lulls to gusts, a flutter across it that quickens as the wind rises, and the player
parting what they walk through. Their gust fronts are their own noise, carried along by
an offset integrated from the mean wind each frame, rather than `gust` itself: `gust`
puts its fronts at `mean * seconds`, so when the mean changes by δ they jump by
`δ * seconds` (with `seconds` in the hundreds of thousands, a whole meadow would lurch
as the wind veered).

## Water

Water used to be an alpha-blended sheet: the block texture in the biome's colour, made more
opaque by a count of water blocks below each vertex (up to seven), with a fresnel blend
towards the fog colour. It read as flat, nearly opaque light blue, every block of depth
along a shore showed as a band, and it reflected nothing. Complementary Reimagined's water,
which Mariano wanted, gets its look from things only the finished scene can give: what lies
behind the surface, and what lies around it.

**Its own pass** (`render/water.rs`). Water and ice (`TerrainPass::Translucent`) are drawn
after the opaque terrain and before Bevy's transparent pass, with the scene so far bound at
group 4 (`water_surface.wgsl`; group 3 is the chunks of a page). Nothing is copied that
need not be:

- Depth is sampled through a read-only depth attachment, so the pass reads the depth buffer
  it tests against (water writes no depth).
- Colour is read from the main image as the opaque pass left it, while the pass writes the
  other main image of Bevy's post-processing ping-pong, which then becomes the main one.
  With MSAA the pass draws into the multisampled image and resolves into it, so no extra
  pass or copy is needed; without MSAA it first copies the scene into it.

The water shader then composes the surface over the scene itself (alpha 1):

- **Light through water** (`water.wgsl`). Absorption is what turns white light into the
  biome's water colour over six blocks (`-ln(colour) / 6`, plus a little for clear water),
  and a little light is scattered back, a muted version of the colour. The path is the real
  distance from the surface to the scene behind it along the view, plus the depth of water
  above that point for the light that lit it. Shallows are nearly clear, a few blocks take the
  colour, deep water is its own dark colour, and the colour changes smoothly with depth along
  the eye's ray instead of in whole-block bands. The scene behind already carries the haze
  between it and the eye; the haze in front of the surface is taken back out, the water
  applied, and the haze put back, so far water is not fogged twice.
- **Refraction.** The view is bent by the waves' departure from the face normal and looks up
  the scene where the bent ray ends (no further than six blocks past the surface); where that
  lands on something nearer than the surface, the straight view is kept.
- **Reflections.** A march through the depth buffer in screen space, where a straight 3D
  line stays straight (uv and reverse-Z depth are both affine in 1/z), in steps that
  lengthen by 15% each so nearby banks are resolved finely, then halved back to the surface
  it passed behind. Rays that would dip into the water are bent to skim it, and hits below
  the surface are thrown away. Where the march leaves the screen the reflection is the sky's
  gradient (the sky dome's zenith and horizon colours), dark where no sky light reaches. The
  march is skipped where the fresnel term is under 3%: seen from above, water reflects
  almost nothing. The march reads a half-resolution depth (the nearest of each 2×2
  pixels, single-sampled, made by a small pass before the water: `water_depth.wgsl`),
  which took the water pass at High from about 1.7–1.9 to 1.25–1.3 ms on a lake filling
  most of a 1080p MSAA frame; the hit is then refined and confirmed at full resolution.
- **Waves**: octaves of afl_ext's `exp(sin x)` waves, each dragging the next along its slope
  (see THIRD_PARTY_NOTICES.md). Their directions, frequencies and phases are worked out on
  the CPU each frame in double precision around the camera, so the shader only adds small
  offsets and the waves stay smooth however far from the origin. The weather's wind turns
  the long waves to run with it and raises them, from a calm ripple to choppy water in a
  breeze. The slope is summed analytically in one pass (not from three height samples), and
  octaves fade out as they shrink below about three pixels, which both stops distant water
  sparkling and ends the loop early. Small drifting ripples go over the waves, and in caves,
  where little sky light reaches, the water is calmer. A shader pack can make water
  **pixelated**, one normal per texel of the block textures, the glittering squares of
  Complementary's Reimagined style (Verdant does).
- **Glints and foam.** A sharp sun highlight with a broader sheen, shadowed by the sun's
  shadow maps; foam in drifting patches where the bed comes within a third of a block of the
  surface, which lines every shore with a broken edge.
- **Under water**, looking up, the surface shows the world above inside Snell's window and
  mirrors the deep water outside it.
- **Flowing water** (see "Fluid state" below for what the mesher reads). The surface slopes
  down from its source, the top edge of the sides with it, and the normal tilts with the
  slope. The waves are carried along the flow in two layers half a cycle (1.6 s) apart,
  each fading out as it starts again, so the pattern moves with the water without ever
  stretching; each layer sums at most ten octaves, and the water gets choppier the faster
  it runs. From 1.6 blocks a second it froths, as do pools where water falls in (the mesher
  gives their surface a churning flow away from the fall). Falling water, and water pouring
  over an edge, is streaked white and runs down its sides.

Distant water (`lod.wgsl`) uses the same optics and the same biome water colour (the
generator's, carried by the sheet's vertices): the floor seen through a depth derived from
the sky light at the floor, the water's own colour where deep, and the sky's gradient as the
reflected sky (its zenith and horizon colours come with `DistantView`), so the two meet in
nearly the same colour where real chunks give way to tiles: within about 10 levels of 255
at view distance 8, seen from 30 blocks up, where they had differed by over 20.

The water's colour is the biome's (`Column::colors`, looked up per block like grass), so
absorption and the colour of deep water follow it.

The Water setting picks how much work this is: Low (the old ripples, sky reflections only),
Medium (8 wave octaves, a 12-step march) and High (12 octaves, 20 steps, the default). Shader
packs scale absorption, scattering, reflections, waves, refraction and foam, and choose
pixelated water (`ShaderPack::water`).

## The air (ClientAtmosphere)

`weather::atmosphere::ClientAtmosphere`, next to `ClientWeather`, is the air around the
player: the land's (`Overworld::atmosphere_at`, sampled twice a second from the client's
own generator and eased over a few seconds, so biome borders pass gradually) with the
weather, wind and time of day applied.

```rust
pub struct ClientAtmosphere {
    pub haze: Vec3,          // haze colour, linear RGB; towards the dust colour in dust,
                             // towards grey-white in fog and rain
    pub haze_density: f32,   // relative to the pack's haze, ~1; up to ~4 in fog and dust
    pub sky: Vec3,           // tint multiplied into the sky, linear; white leaves it
    pub warmth: f32,         // grading, -1 cooler ..= 1 warmer, ~0
    pub saturation: f32,     // grading, ~1; less under overcast, rain and fog
    pub particles: Vec<AmbientParticles>, // sets showing now, densest first:
                             // name, density 0..1, color (linear), size, motion, glow
}
```

The colour parts are for the shader pack to apply within its own look (haze colour and
density into the haze, `sky` into the sky colours, `warmth` and `saturation` into the
grading). Particle sets showing only by day, at night or at twilight, in dry or wet
weather, are already filtered; hovering ones settle out of a strong wind; night-time ones
(fireflies) come out on six nights in ten. The precipitation particles draw the four
densest sets. docs/weather-effects.md has the rest of the weather's handover.

## The air

Mariano's playtest (2026-10-04): shadows flickered, the look still did not read like
Complementary Reimagined, and there was no atmospheric light, mist or fog. What changed,
and why:

The air (atmosphere.wgsl, render/atmosphere.rs) is clear-air haze densest at sea level
and thinning with height, a layer of ground fog under the weather's `fog_top` (valley fog,
dawn mist, sea fog), and the weather's fog, dust and falling rain and snow, each
integrated analytically along the view ray (the ground fog's density profile is a
logistic step whose integral is a softplus), so fog costs a few exponentials per pixel and
never bands. Weather fog is an extinction set from `Weather::visibility` (2% shows at that
distance, so a blizzard closes the view to ~60 blocks); fog is lit by the sky and sun and
takes the colour of blowing dust; the haze takes the place's colour and density from
`ClientAtmosphere` as far as the pack's `local_air` lets it. The sky is fogged by the same
functions as the far edge of the land, so they always meet. Everything shares one uniform
(`Atmosphere`), bound with the terrain and the sky.

## Fluid state

Water and lava flow (`engine/src/fluid.rs`). Each state of a fluid is its own block, made
by the registry right after the fluid's source: `water` (the source), `water_flow_1` (the
deepest flow) to `water_flow_7`, `water_falling`, and for rivers `water_top_1..7` (a still
source with a partial surface, in eighths) and `water_current_<direction>_<speed>_<eighths>`
(pinned river water). Lava has the first nine. Every state shares its source's render kind,
textures, tint and light filter, so until the mesher reads the state, flowing water draws as
whole water blocks. A mesher reads it like this:

- `registry.info(block).fluid` (or `registry.fluid(block)`) is `Option<FluidState>`:
  `fluid` (which fluid: compare these, not block ids), `amount` (eighths of the cell filled,
  1..=8: 8 for sources and falling cells, 7 down to 1 as a flow thins, a river's own surface
  for a partial source), `source`, `falling`, and `current` (pinned river water's
  `Current { direction: 16ths of a turn counter-clockwise from +x towards +z, speed: 1..=3 }`).
- **Join faces by fluid.** The face between two cells holding the same fluid is never seen:
  hide it whenever `a.fluid == b.fluid`, not only between identical blocks. (Until the mesher
  does this, a stream draws a faint internal face wherever its level changes.)
- **Surface.** `fluid::corner_heights(registry, block, pos) -> [f32; 4]` gives the surface's
  height at the cell's top corners, 0..=1, in the order (−x, −z), (+x, −z), (+x, +z),
  (−x, +z). A cell with the same fluid above it has no top face (it is joined), and corners
  next to such cells are 1. Otherwise each corner is the mean of the four cells around it:
  the fluid's cells by height (`amount / 8`; cells at least 0.8 high count ten times),
  open cells (air, plants) as 0, solid cells and other fluids left out (Minecraft's rule), so
  a lake's surface stays flat, shores dip, and flows slope down from their source.
  `block` is any `Fn(IVec3) -> Option<BlockId>` (`None` counts as solid), such as one over
  the mesher's padded cells.
- **Flow.** `fluid::flow(registry, block, pos) -> Vec2` is the direction and speed (blocks
  per second, along x and z) to scroll the texture: a pinned cell's current (0.7, 1.4 or
  2.8), `FLOW_SPEED` (1.4) down the slope of a flow, and zero on still water. Player physics
  calls the same function, so what scrolls is what pushes a swimmer.
- **Falling.** `falling` cells are full columns of water pouring down; scroll their sides
  downwards.

The mesher (`engine/src/mesh.rs`) does all of this for see-through fluids. Faces between
cells of one fluid are hidden; water faces carry, where ambient occlusion would be, how far
the surface lies below the block's top at each corner (eighths, from `corner_heights`, on
the top face and the top edge of the sides), the flow (a direction in 16ths and one of the
speeds 0, 0.7, 1.4 and 2.8; the mean over the cells around each top corner, so it changes
smoothly), and whether the cell falls. Lava is opaque and keeps whole blocks: lowering its
top would open holes where neighbours' faces were culled against it.

Fluid changes reach the client as `ServerMessage::Blocks` (one message per tick with every
change in columns the player has) and go through `ClientWorld::set_block` like any edit.

## What we took from Voxy

Voxy (Fabric LOD mod, read from its source) stores the world as 32³ voxel sections mipped
2× per level, picking the most opaque voxel, draws them as greedy quads with real block
textures through a GPU-driven pipeline (hierarchical traversal by screen size, Hi-Z
occlusion, multi-draw), and fills only the pixels vanilla terrain left empty.

- We tried the same: textured voxel tiles (4–32 block voxels) through the terrain
  renderer. Near the real chunks it matches them well, but beyond a kilometre mountains
  become towers of 16–32 block cubes. We kept the smooth heightfield (stashed as
  "experiment: Voxy-style voxel distant terrain").
- Taken: distant tiles are drawn by the terrain renderer (no entity per tile), and they
  give way to real chunks per column, from a bit mask of the columns that have actually
  been meshed (`DistantView` in `terrain.rs`), not from the render distance. The finest
  ring also keeps tiles under the real chunks, so terrain that has not loaded yet (fast
  flight, edits far away) shows distant terrain instead of a hole.

## Keeping room for what comes next

- **Non-cube blocks** (slabs, stairs, fences, plants, torches): a second block-model path in
  the same mesher, writing a separate buffer with a 16-byte vertex (1/16-block positions,
  octahedral normal, explicit UVs). Cube terrain keeps its 8-byte fast path. Model
  definitions declare which of their faces are full, so neighbours still cull against them.
- **Items, players, creatures, dropped items:** ordinary instanced Bevy meshes and
  materials, lit by sampling chunk light at their position. They never touch the terrain
  path.
- **Multiplayer:** rendering only reads the client's replicated world, so it is unaffected
  by who is authoritative.
