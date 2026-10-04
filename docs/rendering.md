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
| 2. Batch chunks with indexed indirect drawing | One phase item per pass and view issues all of that pass's chunk draws. **Metal has no native multi-draw**, so it is still one draw call per chunk. |
| 3. GPU frustum culling | CPU culling per view (cameras and shadow cascades), in parallel, ~0.05 ms. |
| 4. Hierarchical (octree) culling | Not needed yet: per-chunk tests for ~3,500 chunks are negligible, the visibility graph culls through terrain, and LOD rings keep the chunk count constant as the view grows. |
| 5. Paged geometry buffers | Yes: 32 MB vertex and 24 MB index pages with first-fit range allocation. |
| 6–11. Vertex compression, 48 → 16 bytes | **8 bytes per vertex**: normal from a 3-bit face id, UVs from position, 12-bit texture layer, tint as a 2-bit kind plus climate, AO and light bit-packed. |
| Not in the video | Binary greedy meshing, cave culling, distant LOD rings, meshing and lighting on worker threads with a per-frame upload budget. |

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
  are texture averages times climate tint, as before.
- Tiles stay smooth heightfields rather than boxes: a quarter of the triangles of
  4-block columns, no cracks, and 4-block columns would be a coarser staircase than the
  real one beside them anyway. Instead the shader renders what the one-block staircase
  would show from the eye: treads, risers in the side colour, ambient occlusion in inner
  corners, grass plants in front of the steps on grassland, crowns over forests (tops and
  sides, hiding the ground at low angles). Near the real chunks the risers resolve into
  the same contour lines real terrain shows; further out they average out. This is
  analytic, so it does not shimmer, and it costs no geometry.
- Water and ice are a sheet over the true sea floor, shaded like real water: same
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
