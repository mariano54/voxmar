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
- What we adapted: its 2D and 3D OpenSimplex2 noise and its fractal sums (Brownian motion and
  ridged), with its gradient tables and hashing constants, rewritten for NEON to evaluate four
  points (3D) or four octaves (2D) in step, with selects in place of branches. It returns
  exactly the crate's values; other noise still comes from the crate itself.
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
