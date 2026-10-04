# Third-party notices

Parts of voxmar are adapted from other open source projects. Each entry names the source,
its author and licence, what we took and where it lives in this repository, and the notice
the licence requires.

Projects we took only ideas from, without copying, are credited in
[INSPIRATIONS.md](INSPIRATIONS.md).

## RoadWeaver

- Source: https://github.com/shiroha-233/RoadWeaver
- Author: shiroha-233
- Licence: MIT
- What we adapted: the trail pathfinding cost model of its potential-field pathfinder
  (contour-following discount, soft and hard grade penalties, quadratic elevation cost,
  fall-line penalty), the trail snapping and merging with hysteresis, and the post-processing
  of paths (relaxing, Catmull-Rom centrelines, a two-pass rise limit, bridge spans and ramps,
  squared smootherstep banks that take the strongest trail rather than adding them). The
  code was rewritten in Rust and retuned for footpaths.
- Where: `crates/worldgen/src/trails/route.rs`, `network.rs` and `path.rs`.

```
MIT License

Copyright (c) 2025 shiroha-233

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Bevy (bloom)

- Source: https://github.com/bevyengine/bevy (`crates/bevy_post_process/src/bloom`, 0.19.1)
- Author: the Bevy contributors
- Licence: MIT or Apache-2.0, at our choice; we take it under MIT
- What we adapted: its bloom: the 13-tap downsample with Karis's firefly weighting on the
  first level, the soft threshold, the 3x3 tent upsample, the sizing of the chain and the
  energy-conserving blend between its levels. Bevy draws it as a render pass per level;
  ours runs the same filters as compute dispatches in one pass (rewritten in our own code).
- Where: `crates/client/src/render/bloom.wgsl`, `post.rs` and the bloom in `display.wgsl`.

```
MIT License

Copyright (c) the Bevy contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## FastNoiseLite

- Source: https://github.com/Auburn/FastNoiseLite (the Rust port, crate `fastnoise-lite` 1.1.1)
- Author: Jordan Peck and contributors
- Licence: MIT
- What we adapted: its 2D and 3D OpenSimplex2 noise, its fractal sums (Brownian motion and
  ridged) and its progressive OpenSimplex2 domain warp, with its gradient and random-vector
  tables and hashing constants, rewritten for NEON to evaluate four points (3D and 2D) or four
  octaves (2D) in step, with selects in place of branches. It returns exactly the crate's
  values; other noise still comes from the crate itself.
- Where: `crates/worldgen/src/noise.rs`.

```
MIT License

Copyright(c) 2023 Jordan Peck (jordan.me2@gmail.com)
Copyright(c) 2023 Contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files(the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and / or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions :

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Glimmer shaders (water reflections, refraction and absorption)

- Source: https://github.com/jbritain/glimmer-shaders, files
  `shaders/lib/util/screenSpaceRayTrace.glsl` (itself after a public gist by Belmu),
  `shaders/program/c3_translucents.glsl` and `shaders/lib/water/waterFog.glsl`
- Author: Josh Britain (jbritain)
- Licence: MIT
- Became: `crates/client/src/render/water_surface.wgsl` (`trace`, the screen-space march
  for reflections with its binary refinement; the refraction in `shade_water`, which bends
  the view by the waves' departure from the face normal and keeps the straight view where
  the bent one lands on something in front of the water) and
  `crates/client/src/render/water.wgsl` (light through water as absorption plus
  in-scattering).
- Changed: written for reverse-Z depth, with steps that lengthen geometrically and a
  thickness test scaled for reverse-Z; absorption and scattering follow the biome's water
  colour and the shader pack; the haze in front of the surface is taken out of the scene
  behind before the water acts on it and put back after.

```
MIT License

Copyright (c) 2024 Josh Britain (jbritain)

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
