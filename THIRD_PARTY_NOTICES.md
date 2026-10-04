# Third-party notices

Parts of voxmar are adapted from other open source projects. Each entry names the source,
its author and licence, what we took and where it lives in this repository, and the notice
the licence requires.

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

## "Very fast procedural ocean" (water waves)

- Source: https://www.shadertoy.com/view/MdXyzX, as carried in Glimmer's
  `shaders/lib/water/waveNormals.glsl` (https://github.com/jbritain/glimmer-shaders)
- Author: afl_ext
- Licence: MIT (the source reads "afl_ext 2017-2024 // MIT License")
- Became: `crates/client/src/render/water_surface.wgsl` (`wave_normal`) and
  `crates/client/src/render/water.rs` (`octaves`): octaves of `exp(sin x - 1)` waves whose
  frequency, speed and weight grow and decay by afl_ext's factors, turned by afl_ext's
  direction step, each dragging the next along its slope.
- Changed: the slope is summed analytically in one pass instead of from three height
  samples; phases are worked out on the CPU in double precision around the camera; the long
  waves turn towards the weather's wind; octaves fade out as they shrink below a few pixels.

```
Copyright (c) 2017-2024 afl_ext

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

## Glimmer shaders (water reflections, refraction and absorption)

- Source: https://github.com/jbritain/glimmer-shaders, files
  `shaders/lib/util/screenSpaceRayTrace.glsl` (itself after a public gist by Belmu),
  `shaders/program/c3_translucents.glsl`, `shaders/lib/water/waterFog.glsl` and
  `shaders/lib/water/waveNormals.glsl`
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

Copyright(c) 2023 Jordan Peck (jordan.me2@gmail.com)
Copyright(c) 2023 Contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files(the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and / or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions :

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
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.IN NO EVENT SHALL THE

FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
