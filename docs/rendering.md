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
| 1. Render from raw vertex/index data, not engine mesh objects | Chunks are Bevy meshes, but Bevy packs all of them into shared slab buffers and draws from GPU-built indirect arguments. Remeshing replaces one chunk's range. |
| 2. Batch chunks with indexed indirect drawing | Bevy batches into multi-draw-indirect. **Metal has no native multi-draw**, so wgpu issues one indirect draw per chunk. This is the one real gap. |
| 3. GPU frustum culling | Yes, Bevy's GPU preprocessing. |
| 4. Hierarchical (octree) culling | Not needed yet: per-chunk tests for ~3,500 chunks are negligible, the visibility graph culls through terrain, and LOD rings keep the chunk count constant as the view grows. |
| 5. Paged geometry buffers | Yes, Bevy's mesh allocator slabs. |
| 6–11. Vertex compression, 48 → 16 bytes | **8 bytes per vertex**: normal from a 3-bit face id, UVs from position, 12-bit texture layer, tint as a 2-bit kind plus climate, AO and light bit-packed. |
| Not in the video | Binary greedy meshing, cave culling, distant LOD rings, meshing and lighting on worker threads with a per-frame upload budget. |

## Next steps, in order of payoff

1. **Custom GPU-driven terrain pass.** Store each quad once (8 bytes) and expand it to
   vertices in the vertex shader (5× less vertex memory than now). A compute pass culls
   chunks against the frustum and last frame's depth pyramid and writes a compacted draw
   list, so each geometry page is one draw per pass. That removes the per-chunk draw cost
   on Metal and makes 32+ chunk view distances practical. The shadow pass reuses it.
2. **Light out of the vertex.** Sample sky and block light (and derive AO) from a small
   per-chunk 3D texture. Greedy merging then only has to match textures, which typically
   cuts quads by 2–3×.
3. **Larger real view distance**, once 1 and 2 are in.

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
