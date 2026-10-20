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

Three more things make far chunks cheap, which counting the quads each view draws by
distance showed mattered most (a forest by the sea at view distance 32, Balanced): past
320 blocks the camera drew 1.24 million quads, three quarters of its 1.64 million.

- **Crowns are solid.** Leaves in a merged chunk are drawn as opaque blocks: the faces
  between leaves (a third of a distant forest's) go, and the rest skip the alpha test, so
  the GPU's hidden surface removal works on them.
- **Dark caves are left out.** A face looking into air no daylight reaches (sky light 0,
  at least fifteen blocks from any opening) is not meshed: cave walls, floors and
  ceilings no one far away can see. Faces under water stay.
- **Faces are lit evenly.** A merged chunk's faces take the light of the cell in front
  and no corner occlusion, which nobody can make out at that distance, so neighbouring
  faces merge: this alone halves what is left.

Together the far quads went from 1.24 million to 0.30 million, the camera's from 1.64 to
0.71 million, with no difference a side-by-side capture shows. Foliage also stops moving
from 160 blocks from the player (`motion_near` in wind.wgsl), so far leaves and plants
skip the gust noise in their vertex shader.

The Far detail setting picks where the levels change: Balanced (the default) keeps full
detail out to 10 columns and half detail out to 20, Fast switches at 6 and 12, and Full
keeps every block. A column keeps its level until it is a column past a boundary, so
walking back and forth does not remesh. At view distance 32, Balanced draws about 2.2×
fewer quads than Full, and loads much faster, since fewer quads are uploaded.

## Where a frame goes now

On the player's settings (view distance 32, SMAA, shadows High, volumetric light, the
world at 75% of the 3024×1964 screen) after the October 5 rounds, relative to the display
transform's time in the same Metal trace (Low Power Mode; see Measuring), in a dense
forest seen from above the canopy:

- Terrain pixels about 10×, terrain vertices about 11×: leaves, plants and fringes are
  most of both. Shading is about half the pixels' cost, and the soft shadow filter most of
  that on terrain and grass blades.
- Sun shadows about 5× over four cascades, the farthest two most of it.
- Water, light shafts and clouds, bloom and SMAA about 1× each where they show.

On AC power (Low Power Mode off), headless captures at the player's settings went from
124 to 164 fps at full resolution and from 149 to 233 fps at 75% in that forest, and from
185 to 210 and 204 to 237 over the sea at the spawn, against the morning's code on the
same terrain (one round each, so indicative).

The CPU side: about 130 terrain draws a frame. With the GPU at full clocks the frame is
limited by hand-offs between the main and render threads and the task pool rather than by
any one system (see Measuring).

### Every pass costs a command buffer

Those GPU times added up to far less than a frame took. At the player's settings (view
distance 32, SMAA, the Retina window at 3024×1964) a frame took about 9–10 ms while the GPU
was busy for 6.4 ms of it: it sat idle between passes. wgpu records every render or
compute pass into a Metal command buffer of its own (so it can slip barriers in front of
it), and on Metal each command buffer costs about 0.1–0.2 ms of frame time however little
it draws: the kernel call that submits it, and the GPU waiting for the next. A frame had
about 50 of them:

- Bevy's bloom, a render pass per level of its chain: 16. Ours (`render::post`) runs the
  same filters as compute dispatches in one pass, and the display transform, now our own
  render pass, adds it to the image: 2 in place of 17.
- Bevy's light clustering, five compute and render passes for point and spot lights we
  do not have: none now, clustered on the CPU.
- Bevy building mesh instances on the GPU for our handful of Bevy meshes (sky, rain,
  cracks, other players): four compute passes, plus wgpu's checks of the indirect draws
  they feed and their own copies. Batched on the CPU instead, with views told there is no
  GPU preprocessing at all: otherwise Bevy files opaque meshes for indirect draws that
  only its GPU builder fills, and for a day the sky, the one opaque Bevy mesh, went
  undrawn, leaving the clear colour, with no sun, stars or high cloud.
- The terrain's index compaction submitted itself every frame, with a command buffer of
  pending writes, even with nothing to copy. It now rides in the frame's own submission.

That leaves about 19 a frame: four shadow cascades, the opaque, water and translucent
passes, two for light shafts, bloom and display, three for SMAA, the interface and the
final copy. Turning Bevy's bloom off alone took the full-resolution frame from 97 to 138
fps; all of the above together, measured in Low Power Mode on battery, from 48 to 76 fps
at full resolution. Measurements on this machine swing widely while other work runs, so
these compare runs interleaved with each other, not with the numbers above.

### Shadow casters

The sun's cascades drew even more than the camera: 2.84 million quads a frame in the same
view. A cascade's box is as wide as the diagonal of the stretch of view it serves, and
reaches up the sun's rays without end, so it takes in chunks hundreds of blocks off to
the side and towards the sun. `Casters` keeps only chunks whose shadow can land within
the cascade's range of view depth: the shadow falls along the sun's direction, no further
than ground 160 blocks above what it shades would throw it at the sun's height. That
halves the shadow quads with the sun high (2.84 to 1.41 million) and cuts a fifth with it
low, where long shadows really do reach far. The shadow must also fall within the
camera's four side planes, not just its range of depth: the box takes in chunks well off
to either side of the view, and leaving them out took a fifth off the two farthest
cascades (0.2 ms of their 1.3 ms at 2268×1473).

In a dense forest the dearest faces were the leaf faces inside crowns (a sixth of the
terrain's vertex work and a seventh of its pixels), which only fill what shows through the
holes in a crown's leaves; they are now drawn within 64 blocks, past which no capture told
them apart. Plants and leaf fringes cast shadows only in the two nearest cascades, where
they were most of the third cascade's work; further out their shadows are specks and
fuzz. Together these took about 0.85 ms off that forest at 2268×1473 (Low Power Mode).

Shading asks for soft or hard shadow edges (`sun_shadow`). Terrain and grass blades keep
the soft filter; water, particles, plants and leaves take one comparison of the four
nearest shadow texels. Water's light only tints what it scatters, particles are too
small for a penumbra, a plant takes one shadow for its whole cell, and the holes of
leaves and plants break any edge up: the soft filter had cost the water up to a quarter
of its time over a lake, the plants half their shading in a savanna (0.24 ms) and a
dense forest a fifth of its terrain's pixels (0.4 ms), with no difference a side-by-side
capture shows.

### Measuring

Other threads' games share the GPU, and Metal timings of two identical runs differed by
2–3× while they ran; measure in a quiet window (`~/Projects/voxmar-wt/QUIET`), and check
`pgrep -fl voxmar` first. In one such window (Low Power Mode, battery, the player's
settings, the window at 75%: 2268×1473), a frame's GPU passes took 7.8 ms in all: the
terrain's pixels 1.8 ms and vertices 1.0 ms, the volumetric pass (light shafts, mist and
now the clouds) 1.2 ms, the four shadow cascades about 1.7 ms plus 0.7 ms of filtering in
the terrain's pixels, SMAA 0.5 ms. View distance 16 instead of 32 saved only 0.9 ms. At
75% and 50% the frame was limited by the CPU (about 10 and 7.6 ms) rather than the GPU.
For the CPU side, `cargo build --release --features chrome` records a Chrome trace of
every system (`trace-*.json` in the working directory, large: keep runs short). Its spans
cost enough to distort small systems and wgpu's own work (a submit measured four times
its untraced cost), so check what it points at with `sample`.

Low Power Mode holds the GPU at its minimum or medium performance state ("induced by
device conditions"): adding the Metal GPU Counters instrument to a Metal System Trace
records the state over time (`gpu-performance-state-intervals`). GPU times taken in Low
Power Mode are therefore long, and change with how loaded the GPU is. Dividing a pass's
time by the display transform's, a fixed full-screen pass, in the same trace cancels
most of that: across six runs with different shading switched off, the volumetric pass
stayed at 8.4–8.5 times the display pass while its time in milliseconds moved by 25%.
That is how each part of the terrain's shading was weighed: a scratch build passes
`VOXMAR_ABLATE` to the terrain pipelines as shader defs that switch parts off (fog,
shadows, cloud shade, biome tint, weathering, all lighting), one run each.

On the Linux box (Vulkan on an NVIDIA L4), `VOXMAR_GPU_TIMES=1` makes a capture print
the GPU time of every pass that records a span (Bevy's `RenderDiagnosticsPlugin`, from
timestamp queries; passes without a span are not listed), and `perf record -p` on a
running capture shows the CPU side by thread. There, most scenes are limited by the CPU:
the GPU was 40% busy in the creature scene. `VOXMAR_GPU_FRAMES=FILE` (`render::gpu_clock`)
records every frame instead: its GPU time split into the terrain's compaction, shadows,
main passes and post-processing (FILE), the main world's frame time and busy time
(FILE.cpu), and the render world's preparation, graph and cleanup (FILE.render), by frame
number. In a fast flight it showed the GPU's frames smooth (at most 5.4 ms) while the worst
frames came from the CPU.

A run at a tiny size (640×416) shows the CPU's limit, as the GPU then does little:
about 3.6–4.2 ms a frame in Low Power Mode. No thread is busy for all of it (the render
thread about 40%): the frame waits on hand-offs between the main and render threads and
the task pool more than on any one system.

### Render scale

Most of what is left scales with pixels (shading the terrain, light shafts, SMAA, the
display transform), and a Retina screen has four times the pixels of its size in points.
The Resolution setting (`render::scale`) draws the world at 75% (the default) or 50% of
the window's width and height and stretches it over the window, while menus, text and the
HUD stay at full resolution on a second camera. At Full the world draws to the window
directly and the second camera does not exist. Headless captures always draw at their
`--size`; `--windowed` captures draw to a real window and so take the setting.

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

Under MSAA every pass Bevy draws into the camera's image also resolves the four samples
into the main image when it ends, a full screen of memory traffic each time. The water and
the volumetric composite now draw into the samples without resolving, and the image is
resolved once after the transparent pass (`render::resolve`): 0.3-0.5 ms off 5-6 ms frames
at 2268×1473 on the Linux box's L4. MSAA as a whole still cost 1.7 ms of flight75's 5 ms
there before this.

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

1. **Cheaper vertices.** Most of the vertex time is far faces. Fewer and smaller vertex
   outputs (texture coordinates rebuilt from the position for cube faces, the flat words
   packed into one) and quad pulling (four vertices from one quad record, no index
   buffer) both cut it.
2. **Cheaper shadows.** Fewer filter taps at High, or Medium as the default; the farthest
   cascade redrawn every other frame.
3. **Light out of the vertex.** Sample sky and block light (and derive AO) from a small
   per-chunk 3D texture. Greedy merging then only has to match textures, which typically
   cuts quads by 2–3×.
4. **Larger real view distance**, once 1 and 3 are in.

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
vertex data is needed.

A plant's block data picks its shape (`model`, voxmar_engine::block::PlantModel) and
height (`plant_height`, a share of the usual, so 1.4 for a big flower): `Cross`, the two
diagonal quads every plant had; `Hash`, four straight quads in a # standing square with
the blocks, for crops and reeds; `Cap`, a mushroom of eight quads: a thin crossed stem (the
side texture), a flat crown and four sloping sides down to a wider rim (the top texture),
and gills under the rim (the bottom texture), each mushroom a little bigger or smaller and
leaning its own way by a hash of its cell; and `Bush`, the cross with a straight one through it. Every model
starts with the crossed pair; the mesher packs each further quad as a diagonal one, with
the model in bits 28..29 and which quad it is in bits 30..31 and the face's low bit, and
the height in eighths in word 1 bits 14..18, so the vertex stays 8 bytes; the shader
(`model_across`) places the quads, and they sway and turn as the cross does (crops stay
square with the blocks). Mariano asked for more things in 3D (2026-10-06).

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

## Block shapes

Blocks that are not whole cubes (a slab, stairs, a barrel, a crate, a sack) are drawn as
boxes, as Minecraft's block models are. A package's `shapes.ron` names each shape's boxes
in 16ths of a block (`from`, `to`); a block takes one with `render: Model` and
`shape: "<name>"` (voxmar_engine::shape). The boxes come from the package that defines the
block, which the server loads too, so what a block hides, what light it stops and (from
phase 2) what it collides with are never something a texture pack can change.

At load each shape is made into quads once for each facing and way up a block takes it in
(blocks that share one, the stairs of every wood, share its quads) and checked: every box
runs from less to more within the cell; a box may hide behind a solid neighbour (`cull`)
only with a face on the cell's side; a box turned about the vertical (`turn`) hides nothing and counts, for what it hides and will
collide with, as its unturned bound; and the caps on boxes per shape and on quads in all
are data (core's `shapes.ron`, which a later package may change field by field, though
never past the 16384 quads a vertex can number: 12 bits in word 1 and 2 in word 0); a shape is defined by one package only. Which of the cell's six sides the boxes cover whole is
worked out by sampling every 16th, never read from a flag: only a shape covering all six
hides its neighbours' faces and stops light as a cube does, so a slab's top or a stair's
side never hides a face, and slabs and stairs neither darken what they touch nor stop
torchlight.

The renderer keeps every shape's quads in one storage buffer (`shape_quads`, binding 17 of
the terrain bind group: each quad's corners, texture coordinates, normal and the cube face
it is lit as). The mesher emits a shaped block's quads as plant faces of a seventh plant
model (`PLANT_SHAPE`), with the quad's number in word 1 bits 12..23 where a plant keeps its
tint and height, so the vertex stays 8 bytes; the vertex shader looks each corner up. They
go into the cut-out stream with the cut-out cube faces, not with the plants (terrain.rs
`split`, engine `mesh::is_shape_quad`): drawn from one side, alpha-tested, at every
distance and into every shadow cascade, where plants stop at 320 blocks and the near
cascades. A quad on the cell's side takes the brighter
light of its cell and the one beyond, as a cube face takes the light in front of it; one
inside the cell takes the cell's. Shapes have no ambient occlusion yet. In the coarse
meshes of far chunks a shape is drawn as its block's cube, too small there to make out.
Inventory icons draw a shape's own quads at the cube icon's angle (texgen
`isometric_quads`), so a barrel shows its staves and stairs their steps.

A block that `flips` also comes upside down (`<name>_top`: a top slab, stairs under a
ceiling), and one with `facing` in four facings. Placed against the underside of a block
it goes upside down, and it faces whoever places it, so stairs climb away from the player
(engine `action::placed_block`, which client and server both run). Players collide with
the same boxes (`BlockRegistry::collision`), on the client and as the server judges their
moves, and step up by `movement.ron`'s `step_up`; where that box reaches into a shape's cell the
server checks every block against the player's box less a fixed `wall_margin`, not a
share of it (the share box sits inside it, so a shape placed or mined beside a player
never turns a spot they stood in into a wall). Targeting (what the crosshair
picks) and creatures still take whole cells.

## Leaves

What made the shader-pack screenshots' trees look better than ours was mostly the leaves'
texture and colour, then their light; their bushy geometry was the smaller part.

- **Texture.** A leaf block's texture is single leaves drawn as small sprites, lit along
  their upper edges, in a few shapes and both ways round, over shadowed depths, with holes
  cut only in the depths between leaves (texgen's `Cut` with `darker_than`), so the sky
  shows between leaves rather than through them. The old one, per-texel grain with random
  holes, read as static.
- **Colour.** Leaves take the place's foliage colour a little lighter, yellower and less
  saturated (`biome_colour` in voxel.wgsl): the climates' foliage colours are saturated
  enough that a canopy read as one dark, pure green.
- **Light through the crown.** A leaf in its own crown's shade still glows where only a
  leaf or so lies between it and the sun: its shadow is looked up again a block and a
  quarter towards the sun (`shade_normal`), only for leaves less than half lit.
- **Cards.** Leaf blocks on the outside of a crown carry two crossed upright cards, two
  blocks wide and 1.75 tall, and leaves open to the sky a flat one, tilted, in their own
  pass (`TerrainPass::Fringe`, mesh bit 27, flag 2 for the flat card). The texture runs on
  past the block at its own 16 texels a block, cut to a ragged round cluster (after Better
  Leaves Lite), different on every card, and a card shows its block's overlay (fruit) only
  over the block itself. Each card is shaded as the face of the crown it grows from (an
  open side, else the top), not as facing up, so a card seen edge on reads as part of that
  face rather than as a light seam across the crown. A lone leaf block on the ground (a
  heap of fallen leaves) grows none: as a round cluster it looked like a ball resting on
  nothing. Cards shrink into their block from 40 blocks and are drawn to 64.
  They cast no shadow: a card's shadow is the crown's own blurred by a few texels, and in
  a dense forest the cards in the shadow cascades were most of their cost.

Measured in a dense forest from a hill and inside it (release, 1920×1080 at 75%):
cards casting shadows cost 1.2–1.5 ms over none; casting none, they were within the
noise of main in every interleaved round.

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
  `BlockChanged` to air) burst too, within 32 blocks, but not plants: those also vanish
  under feet as the server wears paths, which is not breaking them.

Particles are lit with a sky light estimated once per burst from the columns around the
block, by the engine's rule (straight down from the sky, less through water and leaves,
stopped by solid blocks, then one level less per block sideways, up to three, only
through what lets light by, so a cave a block inside a cliff is not lit through it), and a block
light from the torches and glowing blocks within seven blocks (`breaking::light_at`, the
dropped items' `block_light`), or the block's own glow if that is more, since the client
keeps no light field outside the mesher. Pieces breaking from a block in a cave by a torch
are lit by the torch, in a dark cave they are dark, at night the moon's. (They once took
only the block's own glow, so a torch-lit cave gave dark pieces.)

The cracks are lit by the face under them. Multiplying keeps the block's light on their
dark lines, and their light edges (a texel over mid grey) are light a crack catches, so
they show as much as that face is lit: the same sky and block light as the pieces, through
`SceneLight`, faded by the pack's `crack_light` (default luminance 0.02 to 0.25). In a dark
cave the crack only darkens; by a torch or in the sun it glints.

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
`wind.wgsl`, bound with the terrain so shadows bend alike). As in the Grassier Grass mod
(whose code is all rights reserved, so only the idea is taken), the motion is mostly the
lean, not the flutter: the lean follows gust fronts tens of blocks across and ripples a
few blocks across, both rolling over the land with the wind, so neighbouring blades bend
together and waves sweep across a meadow, with a lighter sheen on the blades a gust bends.
The lean grows from barely a stir in still air through rolling waves in a breeze to
streaming over in a gale, with deeper swings in gustier weather; a blade tips over to the
angle the lean gives without growing longer. Each blade gives to the wind a little more or
less than its neighbours and keeps a small rest lean of its own in calm air. A small
flutter, mostly across the wind, plays on top, each blade at its own pace; plants flutter
slower, and tall ones are stiffer above their first block. The player parts what they
walk through.

Nothing in it is a function of the clock times the wind. The gust fronts are carried by an
offset integrated from the mean wind each frame, rather than by `gust` itself: `gust` puts
its fronts at `mean * seconds`, so when the mean changes by δ they jump by `δ * seconds`
(with `seconds` in the hundreds of thousands, a whole meadow would lurch as the wind
veered). The flutter runs on a phase integrated the same way at a pace set by the wind's
strength, wrapped every 320 turns; every flutter is a whole number of tenths or 64ths of
it, so it stays in step across the wrap. It used to be `sin(time * pace)`, which leaps by
`time * Δpace` whenever the pace changes: an hour into a game, any change of wind threw the
grass into a fit, and a slowly changing wind kept it shaking far faster than its pace. The
flutter's phase also follows the gusts rather than the position along the wind, which far
from the origin the slightest veer would leap. `--clip SECONDS` records frames of game
time at 30 fps to check motion like this.

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

- **Light through water** (`water.wgsl`). Absorption follows real water's, taking red
  first, much less green and hardly any blue (0.15, 0.035, 0.018 per block), shifted towards
  the biome's hue; a little light is scattered back, a muted version of the biome's colour.
  The path is the real distance from the surface to the scene behind it along the view,
  plus the depth of water above that point for the light that lit it. Shallows show their
  bed clearly in a cyan tint, deep water turns blue, and the colour changes smoothly with
  depth along the eye's ray instead of in whole-block bands. The beds are lit as the surface
  above them is (the mesher gives faces under water the sky light above the water), so the
  dimming happens once, here, and not also through the light engine's falloff per block of
  water. The scene behind already carries the haze between it and the eye; the haze in
  front of the surface is taken back out, the water applied, and the haze put back, so far
  water is not fogged twice.
- **Present however shallow.** Mariano's playtest found the first version "very
  transparent": a thin flow over stone vanished altogether. Water now always shows a glaze
  of its own colour, lit (28%, desaturated a little), and reflects with a softer fresnel
  (Schlick's with a cube in place of the fifth power, as stylised packs do), so it carries a
  sheen of sky and waves from a hiker's height. Neither fades along the shore except over
  the first sixteenth of a block, so the shoreline is soft but not a gap. The side of water
  that does not pour (the thin edge of a flow) is shaded as the edge of its surface; shaded
  as a wall of water it caught the sky and outlined every flow in white.
- **Refraction.** The bed is seen through a point of the surface moved by the waves' slope,
  more so over the first block of depth, so at the very edge nothing wobbles; where the
  moved point lands on something nearer than the surface, the straight view is kept. With
  MSAA, a pixel's depth samples can straddle the edge of a bank or plant in front of the
  water; the nearest sample behind the surface stands for the pixel, where the first sample
  alone put the bank under the water and drew foam and odd colours along every edge.
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
- **Waves** (`render/water/waves.rs`). A 256² tiling texture of gentle swells (octaves of
  gradient noise, with their slopes), made at start-up, laid over the surface in four
  layers, 43, 17.3, 6.9 and 2.7 blocks across, turned by unrelated angles and drifting with
  the wind (in calm air each its own slow way), so the sum never repeats visibly; the first
  version, a dozen sine waves, showed its pattern from any height. Each layer is one
  filtered texture sample, its mip chosen by the pixel's footprint, so distant water does
  not sparkle. Positions are worked out on the CPU in double precision around the camera.
  How high the waves are is the wind's (a fifth of their height in calm air, full in a
  strong breeze) and the water's: the mesher counts how much open water lies around each
  vertex in a 24-block square (open meaning water at that level with no roof), so a puddle
  or a sheltered pool lies nearly still and only open water in a wind carries waves; caves,
  where little sky light reaches, are calmer still. A shader pack can make water
  **pixelated**, one normal per texel of the block textures, the glittering squares of
  Complementary's Reimagined style (Verdant does).
- **Glints and foam.** A sharp sun highlight with a broader sheen, shadowed by the sun's
  shadow maps and hidden where the low cloud covers the sun seen from that bit of water;
  foam in drifting patches where waves break, where the bed comes within a
  third of a block of the surface (none on still water), slightly tinted by the water.
- **Under water** the water takes the air's place in `aerial` (`under_water` in
  atmosphere.wgsl), with the same absorption and scattering as water seen from above, for
  the biome's water colour at the eye: red goes first and blue last, so the view closes in
  over a few dozen blocks into blue, the bed takes the water's tint, and what lies deeper
  is darker for the light that came down to it. The water's own glow is single scattering
  of light fading with depth, integrated exactly along each ray (`water_glow`), three times
  brighter for the light scattered more than once. Looking up, the surface shows the world
  above inside Snell's window and, outside it, mirrors the water below, lit as the water
  around the eye. The sky dome, which writes no depth so passes reading depth know the view
  reaches it, shows only the water.
- **Flowing water** (see "Fluid state" below for what the mesher reads). The surface slopes
  down from its source, the top edge of the sides with it, and the normal tilts with the
  slope. Like Minecraft's flowing water (which the Streams mod's rivers show), it carries
  streaks running with the flow: the swell texture stretched four times along it, 11 and
  4.3 blocks across, two mip levels softer than the pixel needs so it reads as long smooth
  streaks rather than grain, carried along in two layers half a cycle (2 s) apart, each
  fading out as it starts again, so the pattern moves without ever stretching. Each cell
  turns its streaks as a whole by its own flow, so where the flow changes the pattern
  changes at the block's edge, as in Minecraft, not along a diagonal through it. Rapids
  froth a little from 2 blocks a second, as do pools where water falls in (the mesher gives
  their surface a flow away from the fall). Falling water, and the sides over which water
  pours down an edge (the mesher marks them), are streaked and partly white.

Distant water (`lod.wgsl`) uses the same optics and the same biome water colour (the
generator's, carried by the sheet's vertices): the floor seen through a depth derived from
the sky light at the floor, the water's own colour where deep, and the sky's gradient as the
reflected sky (its zenith and horizon colours come with `DistantView`), so the two meet in
nearly the same colour where real chunks give way to tiles: within about 10 levels of 255
at view distance 8, seen from 30 blocks up, where they had differed by over 20.

The water's colour is the biome's (`Column::colors`, looked up per block like grass), so
absorption and the colour of deep water follow it.

The Water setting picks how much work this is: Low (two wave layers, sky reflections only),
Medium (three layers, a 12-step march) and High (four layers, 20 steps, the default). Shader
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

## Light, shadow and the air

Mariano's playtest (2026-10-04): shadows flickered, the look still did not read like
Complementary Reimagined, and there was no atmospheric light, mist or fog. What changed,
and why:

**Shadows stand still.** Bevy already snaps each cascade to its texels, so a static
scene rasterises identically as the camera moves; what kept shadows moving was the sun,
which turned a little every frame and so redrew every shadow edge, leaf hole and grazing
face at a new sub-texel position. The light now moves in steps of a tenth of a degree of
its arc (about a third of a second at the normal pace; `LIGHT_STEP` in render/mod.rs).
Between steps nothing in the shadow maps changes; a step moves a shadow cast from eight
blocks up by under a texel of the nearest cascade, which the soft filter hides, so there
is nothing to blend. Leaves cut out of the shadow maps at the mip whose texels are half
again as large as the cascade's (voxel.wgsl, `alpha_test`), so their holes are never finer
than the map holds and the dappled shade under a crown does not sparkle as cascades move.

**Shadow resolution.** High draws each of the four cascades (12, 31, 78 and 200 blocks
out) into a 2048-texel map; from about 30 blocks out, in the third cascade, a shadow's edge
shows steps (Mariano's playtest of 2026-10-05: shadows don't look great, maybe they need
higher res). The Ultra setting draws 4096-texel maps, which takes the steps out, for about
1.8 ms a frame at his settings (Low Power Mode, a busy machine) and 268 MB of maps; near
shadows look the same at either size, as the soft filter, not the map, sets their edge.

**Soft shadows with a penumbra** (shadows.wgsl, our own sampling of Bevy's cascades): a
blocker search finds how far the caster is and the filter widens with that distance
(percentage-closer soft shadows), so a trunk's contact shadow is crisp and a canopy's
shade on the forest floor soft-edged, as under a sun of real size. Taps are hardware 2×2
comparisons on a fixed golden-angle spiral at most five texels wide, so a shadow depends
only on where a point is and nothing in it changes as the camera moves (a per-pixel noise
pattern, the usual way to hide the taps, swims with the camera and sparkles at the edges
of bright sunflecks unless temporal anti-aliasing resolves it, which we do not have).
Receiver plane depth bias makes each tap compare against the face's own depth there; voxel
faces are flat with exact normals, so a wide filter never shades the face itself. Cascades
blend over their overlap and shadows fade over the last 15% of their reach instead of
ending in a line. The first cascade ends at 12 blocks (texels of about 2 cm), shadows reach 200
blocks at High. Settings: Shadows Off, Low (one hard tap, two cascades to 96 blocks),
Medium (8 taps, three to 160), High (16 taps, four to 200, the default).

**Light and shade** (lighting.wgsl), after Complementary's way of lighting a voxel world
rather than its numbers: warm direct sun against cool sky light; sky light falls off
steeply with the sky light level (`sky_falloff`, an exponent), so forest floors, overhangs
and the inside of crowns are far darker than open ground, and only sunflecks are bright
there; voxel ambient occlusion grounds corners and wall bases (`ambient_occlusion`);
sunlit leaves and plants glow, most when seen against the sun (`subsurface`); passing
clouds shade the land (`cloud_shadows`): a point's cloud is where its ray to the sun
crosses the middle of the low cloud's layer, and the same density (atmosphere.wgsl,
`cloud_density`) draws the cloud and its shadow. Rain darkens and wets open
surfaces, fills puddles on level ground that mirror the sky, and frost whitens what lies
under the open sky (`wet_ground`, `frost`).

**Glowing blocks** glow by themselves, not only light their surroundings: every texture
layer of a block that gives off light gets a glow strength from its light level (in the
layer table, one texel per layer, built in main.rs, `layer_table` in render/mod.rs), and
the terrain shader adds the tinted texel times that strength to its bright texels only (a
lantern's flame, magma's cracks, lichen's spots; dark texels stay dark), up to three times
the lit texture at full light, so the brightest bloom. A per-texel mask texture can
replace the brightness test when a pack ships one. Strength: the pack's `glow`.

**Grass block sides** match their tops: the side of an opaque block with a tinted top
takes the pack's `<side>_overlay` texture (Minecraft's `grass_block_side_overlay`, our
pack's recipe of the same name), from the same pack as the side so the two line up, and
the terrain shader draws it over the side by its alpha, tinted with the biome colour the
top has; before, the side showed the pack's fixed green fringe whatever the biome. The
layer table holds each layer's overlay and its tint; break particles, inventory icons and
the distant terrain's colours take the overlay too.

**Mist belongs to the open air** (render/exposure.rs). Ground fog, the weather's fog and the
puffs of mist (`precipitation.rs`) show only where the camera sees the sky: dug down, in a
cave or in a room there is none, though the camera is still under the fog's top. Sky light
cannot tell, since a shaft dug straight down keeps full sky light the whole way, so the
camera looks for the sky as an eye does: 32 rays over the upper hemisphere, evenly by solid
angle, and the share that go 32 blocks without meeting a block that hides the sky (leaves
do not, so a forest is open). Every 0.1 s, eased over about half a second. Open land sees
1, a cliff beside it about half, a one-block hole a third, a shaft three deep next to
nothing. The pack's `mist_sky` (default 0.12 to 0.4) says between which shares the mist
fades from none to all (`ShaderPack::mist_share`), and scales `Atmosphere`'s ground fog and
weather fog (`View::mist_share`) and the puffs. The haze of clear air stays.

**The air** (atmosphere.wgsl, render/atmosphere.rs): clear-air haze densest at sea level
and thinning with height, ground fog (valley fog, dawn mist, sea fog) densest below the
weather's `fog_top` and thinning by e every 9 blocks above it, so it fills valleys, has no
hard top and reaches thinly up the slopes, and the weather's fog, dust and falling rain
and snow. Beyond the volumetric pass's reach each is integrated analytically along the
view ray (exponential height profiles integrate exactly), so it costs a few exponentials
per pixel and never bands. Weather fog is an extinction set from `Weather::visibility`
(2% shows at that distance, so a blizzard closes the view to ~60 blocks); fog is lit by
the sky and sun and takes the colour of blowing dust; the haze takes the place's colour
and density from `ClientAtmosphere` as far as the pack's `local_air` lets it. The sky is
fogged by the same functions as the far edge of the land, so they always meet.
Everything shares one uniform (`Atmosphere`), bound with the terrain, the sky, particles
and the volumetric pass.

**The volumetric pass** (volumetric.wgsl, render/volumetric.rs) draws the air in front of
the view over the opaque scene and the water, before Bevy's transparent pass, so rain and
snow fall in front of it. Each view ray is marched at half resolution (all of this is
soft) for the light the air adds and how much of the scene shows through, its steps
starting at noise that changes every frame; accumulated with the frames before, found
where the same air was by last frame's view (the cloud's light where the cloud lay, other
rays where they ended), kept to the range of this frame's neighbouring rays so nothing
that moved leaves a trail, and dropped where the ray then ended at another depth; then
blended over the image at full resolution from the four nearest rays whose surfaces lie
about as far away as the pixel's own, so nothing bleeds across a trunk or a ridge. The
accumulation (about eight frames) replaced a 3×3 blur among neighbouring rays, which
left the cloud grainy and soft (Mariano's playtest, 2026-10-05): the grain averages out
instead of being smeared. It draws:

- *The low cloud*, a layer in the world (Mariano's playtest of 8e6c104: clouds were a
  picture on the sky, and on a mountain they should pass you). Its base lies 135 blocks
  above sea level in fair weather and comes down to 85 in rain, onto the hills; it is 32
  blocks deep as a flat layer and up to 150 as heaped cumulus. Its clouds don't all sit
  at that height (Mariano: "clouds only at 1 y level, looks weird, especially on a
  mountain"): in drifts some 700 blocks across they float higher or lower by up to 0.6 of
  the layer's depth (`cloud_lift`), a few blocks for a flat layer and dozens for heaped
  cloud, so they meet the hills at different heights. The same 2D noise as before
  sets where cloud is, and within the layer heaped cloud rises in domes over flat bases,
  its thickest cores tallest; within 800 blocks 3D puffs about 20 blocks across bite into
  its edges and tops. 16 steps cross the layer, up to 32 where it is seen at a slant so no
  step is much longer than 12 blocks, closer together near the eye from inside it; inside,
  you see about 20 blocks. Each step looks twice towards the sun for the cloud in the way
  (12 and 48 blocks off), so sunlit faces and tops are bright, the far side and the bases
  grey, and puffs shade each other; light scattered many times inside softens that, edges
  towards the sun glow silver, storms darken the bases and lightning lights the layer. The
  haze between the eye and the cloud is the same aerial perspective as the land's, and
  cloud thins into the sky beyond 3000 blocks. Steps start at white noise per ray (the
  interleaved gradient noise used before left diagonal streaks without temporal filtering). From a peak in rain the cloud
  drifts past and around you, hides the valley below and the summit above. The sky dome
  keeps only the high sheet and the cirrus, which lie far above any mountain.
- *Ground fog* near the eye (within 160 blocks), as an uneven, drifting volume rather than
  a smooth layer (Mariano: fog sat in one layer low down): its top swells and sinks by up
  to 6 blocks across the land and its body billows in two octaves of 3D noise, drifting
  with the wind at a third of the clouds' pace, so from inside it thickens and thins as it
  passes and from above it lies as an uneven sea with a soft top. Beyond 160 blocks it is
  the smooth analytic layer, of the same mean density. It glows towards the sun only where
  the sun reaches it, and it stops at sea level (water and caves lie below).
- *Light shafts*: sunlight scattered by haze, fog and ground fog where the cascaded shadow
  maps say the sun reaches, out to 96 blocks in 12 steps spaced more closely near the eye:
  shafts through gaps in canopies, mist glowing in the sun, the sun's glow in haze. Clear
  air scatters it more than it dims the view (an artistic boost, as shader packs do), so
  shafts show in clear weather too. Forward fog leaves the sunlight within the pass's reach
  to it, so nothing is counted twice. Setting: Light shafts.

Water reflects the low cloud where its reflection of the sky crosses the layer (one look
at the layer's middle). The fast white streaks Mariano saw were the cirrus: they were
streaked along the wind at the camera, so every gust swung them across the sky; they now
run along a fixed high wind and only drift.

**The cloud map** (cloud_map.rs): the cloud's 2D body around the eye, 1024² texels over
12.8 km, drawn once a frame before the main passes. Terrain, grass, water and particles
look up their cloud shadow there and the volumetric pass its cloud, with one filtered read
instead of several octaves of hashed noise; its texels are fixed in the world, so shadows
do not swim. That noise had been the most expensive part of shading under cloud: a Metal
trace of a cumulus view over a fjord took about 5 ms more GPU time a frame with cloud
shadows than without (most of it in the opaque pass, where grass blades multiply the
fragments). The volumetric pass reads the 2D and 3D value-noise lattices from two small
textures too (`NOISE_TEXTURES`), the same lattices the hashes give elsewhere. The map
itself measured 0.2 ms on a busy machine.

**The shade map** (cloud_map.rs): the cloud's density through the middle of its layer,
512² texels over 2 km, drawn from the body map right after it. Shadows and reflections
only ever look the cloud up there, and the map spares each fragment the rest of that
look-up: the two octaves of billows at the cloud's edge and the layer's profile. Taking
each part of the terrain's shading out in turn (a `VOXMAR_ABLATE` shader def in a scratch
build, timed by Metal System Trace and divided by the display pass's time to cancel the
GPU's clock changes) found cloud shadows the costliest part, more than the shadow filter
or the fog. At 2268×1473 under broken cloud the shade map took 0.22 ms off the terrain's
pass and 0.19 ms off the water's, and costs 0.025 ms. Its texels are 4 blocks, a quarter
of the cloud's finest detail, and it holds the density unclamped so the bilinear filter
keeps edges sharp. It is centred where the ground around the eye takes its shadows from,
towards the light; past it shaders work the density out as before.

The terrain now reads the value-noise lattices from the same two textures as the
volumetric pass (`NOISE_TEXTURES`, held with the cloud maps), for the puddles, frost and
snow that weather leaves on it: one filtered read per noise instead of four hashes. In
drizzle that took 0.26 ms more off the terrain's pass at 2268×1473, and the frost's noise
is no longer worked out where there is no frost. The cell is wrapped to the lattice before
it becomes a texture coordinate, so the noise keeps its precision far from the origin.

**Night** (Mariano: too bright). The sky gradient was already a dark navy; what lit the
night sky grey was cloud, which the dome lit at 7% of daylight. Cloud is now lit by the
moon and the night sky's ambient light like everything else, and the Milky Way is dimmer:
a clear night sky reads about (7, 9, 13) of 255 in captures, against about (19, 20, 24) in
his screenshot; moonlit cumulus stays visible as dim grey shapes.

**Compared with Complementary** (item 29). Minecraft could not be started for matched
shots (NeoForge found no monitor while the display slept), so the comparison is against the
hiking video's Complementary frames. Light levels on sunlit grass match closely (about
(77, 94, 29) against (72, 95, 41)); voxmar's greens were more saturated, so Verdant's
saturation goes from 1.15 to 1.05 and vibrance from 0.25 to 0.15. Shade under trees and the
cloud bases now read as in the frames; a matched Minecraft session is still worth doing when
Mariano is at the machine.

**Grading** (display.wgsl): contrast is now an S-curve on the display image (below mid
grey exactly the old power curve, above it rolling into white instead of past it), warmth
leaves highlights neutral, and clouds in full sun are as bright as a sunlit white block,
never above the bloom threshold. So bright cumulus and cloudy skies under Verdant stay
white and shaded instead of washing out to cream.

**Measured** (1920×1080, MSAA 4×, view distance 32, `--flicker-still` and `--flicker`,
pixels changing by more than 60 levels between two frames; before is main just before
this work, with Bevy's own shadow filter). With the camera still while the game runs on a
frame, a forest view went from 0.013% to 0.012% and a low-sun view stayed at 0.010%: at
that threshold the old shadows already changed few pixels per frame, and what remains is
swaying leaves and grass; at 4 levels, where slow crawling shows, the forest went from
0.66% to 0.47% and a valley view from 0.40% to 0.24%. For a 4 cm step (1.6% and 2.5% in
those views) shadows make no measurable difference: turning them off leaves 2.4% and
2.9%, so what flickers there is pixel-art texture aliasing on leaves and grass, not
shade. Cost, measured on a machine busy with other builds and so only roughly: soft
shadows about 1 ms more than Bevy's filter at view distance 32 (shadows Off saves about
2 ms in all, most of it drawing the cascades), light shafts about 0.4 ms. High takes 16
taps where a penumbra is wider than a texel and 4 where it is not; Medium and Low are
cheaper.

**Shader packs** gained `sun_tint`, `shadow_softness`, `sky_falloff`, `ambient_occlusion`,
`subsurface`, `cloud_shadows`, `glow`, `rays`, `mist`, `fog`, `wet_ground`, `frost` and
`local_air`; Classic keeps the plain look (no volumetric light).

**Dusk and dawn** (Mariano's playtest: "dusk and morning should have different lighting
whether close or far to the sun"). The low sun warms one side of the sky: towards it the
horizon glows warmer and brighter, away from it, under the earth's shadow, it lies cooler
and dimmer. `dusk_tint` in atmosphere.wgsl scales a colour by `1 + DUSK_CONTRAST · k · d`,
with `k` how far a direction looks towards the sun across the land (its horizontal part, so
the zenith and a top face take neither side) and `d` how strongly dusk shows (from the sun's
height, none under thick cloud). The sky's horizon, the haze the land fades into, the sky
water mirrors and the sky light on walls all take it, so they agree.

## Texture variety

Block textures used to repeat identically on every block, so a large motif (a crystal, a
pebble pattern) showed its grid across a wall or a floor. A block's data can now ask for
variety (`texture_variety: (turn: true, mirror: true)` in its definition, opaque blocks
only): each block picks, by a hash of its position, a quarter turn of its top and bottom
and whether its faces are mirrored left to right, as Minecraft's stone, sand and dirt do.
The flags travel per texture layer in the layer table's tint byte (64 turn, 128 mirror)
and the terrain shader lays the texture by them (`laid` in voxel.wgsl), mapping the
texture coordinates' per-pixel changes the same way so mip levels stay right across block
edges. It costs a hash and a 2×2 matrix per pixel on the blocks that ask for it. Stone,
dirt, gravel, sand and grass blocks ask for both.

An ore names the block it lies in (`host: "stone"`): each of its faces is the host's
texture with its own drawn over it by alpha, composed as the textures load, from
whichever pack draws the host (`TextureRef::under`, `Rgba::over` in textures.rs). So the
rock around the ore always matches the rock around the block, Minecraft's stone or ours,
and any ore can lie in any rock with one ore texture: an ore's texture is the ore alone,
on clear texels (our recipes draw cobalt, tin, silver and aetherite that way).

## Night, dawn and dusk

Mariano's playtest of 2026-10-05: nights too bright, stars too shiny, comets and aurora too
common and too short, the same constellations and planets every night, and a sunrise
that showed nothing until the sun came up. Things in the sky should be rare.

The night sky is laid out for each world from its seed (`render/night.rs`, drawn by
sky.wgsl): ten constellations of our own (stick figures of five to eleven stars, one in
ten a tight cluster), one to three planets, the Milky Way's path, three galaxies, two
nebulae and two dwarf galaxies, and two or three comet visits in the sky clock's 1000
days. Nothing shows every night:

- The stars turn once against the sun every 100 days (`YEAR_DAYS`), so each season has
  its own constellations at midnight.
- Outer planets creep along the sun's path and meet it every 110 to 200 days, lost in
  its glare for a while; the pale blue planet, when a world has it, never strays more
  than 24° to 46° from the sun and shows only after sunset or before sunrise.
- The moon goes through its phases every 10 days, full on day 0 and rising later each
  night. The phase is the engine's (`voxmar_engine::calendar::moon_phase` and
  `moon_lit`, from days elapsed), so the server's creatures, drops and plants can follow
  the same moon the sky shows. It is the night's light: moonlight scales with how much of it
  is lit (to the power 1.5), and the sky and skylight are 0.6 of a full moon's without
  it (`MOONLESS`). Its dark side hides the stars behind it.
- Comets stay five to nine days, brightening and fading, a tail streaming away from the
  sun. Falling stars last one to two seconds; an ordinary clear night's rate is 0.004
  (one every few minutes), meteor showers stay very rare, and the aurora is four to five
  times rarer than it was: rare at high latitudes, very rare lower down
  (assets/content/base/weather.ron).

Every period divides the 1000 days after which the sky's day count wraps, so the wrap
never shows. Stars are dimmer than before and their halo fainter.

With the sun low, a wide warm glow surrounds it, wider once it has risen; with it still
under the horizon, the glow stands over where it will rise, in faint rays fanning up
from behind the land. The other way the sky dims, and in twilight the earth's shadow
rises from the horizon under a faint pink band. Rain and snow never go darker than a
dark night's own faint glow (`NIGHT_GLOW` in precipitation.wgsl), so they show at night.

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
