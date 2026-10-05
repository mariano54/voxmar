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

## Tectonic (terrain splines)

- Source: https://github.com/Apollounknowndev/tectonic (version 3.0.25, commit 34241bd)
- Author: Apollo (Apollounknowndev), with contributors HB Stratos, DawnKiro and Uni
- Licence: MIT, as declared in the project's build file (`build.gradle.kts`) and on its
  Modrinth page; the repository carries no licence file, so the notice below is the standard
  MIT text with the author named there.
- What we adapted: the structure and knot values of its terrain density functions: the
  continents, erosion and ridges fields (octave weights and scales, folded with `abs()`),
  the erosion sharpening curve, the continental offset spline over continentalness, erosion
  and ridges, the terrain regions (club, heart, spade and diamond: plateaus, single and
  double valleys, badlands ridges, rolling hills, wet plateaus, two-tier terraces, wetland)
  with their temperature and vegetation indices, the jaggedness on range spines with its
  gradient-shifted ridge detail, and the trench-and-ridge sea floor. Rewritten as our own
  RON expressions, with larger continents and ranges, valley floors kept above the sea for
  our own rivers, and lowlands lifted a few blocks. Minecraft's own data, which Tectonic
  builds on, was not copied.
- Where: `assets/content/base/land.ron`, evaluated by `crates/worldgen/src/router.rs`.

```
MIT License

Copyright (c) Apollo (Apollounknowndev) and Tectonic contributors

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

## Field recordings from Freesound (rain, thunder, water, hail, caves, lava)

Every recording below is dedicated to the public domain under Creative Commons CC0 1.0
Universal (https://creativecommons.org/publicdomain/zero/1.0/), which requires no notice;
we credit the recordists anyway. Licences were checked on each sound's page on 5 October
2026 (the last five rows on 6 October). We used Freesound's high-quality previews and changed them: cut into short pieces,
filtered (high- and low-pass), levelled, faded in and out, a mono recording made stereo
from two different stretches, a close thunder clap low-passed to sound farther off, and
encoded as MP3. The files are in `assets/content/base/sounds/` and are listed in
`ambience.ron` (rain, drips, stream, lake, waterfall, hail, cave air, lava) and `sounds.ron`
(thunder, swimming, splashes).

| Our files | Recording | Recordist | Source |
| --- | --- | --- | --- |
| `rain_light_1..3.mp3` | rain in the forest atmo field recording | Garuda1982 | https://freesound.org/s/536843/ |
| `rain_heavy_1..3.mp3` | heavy_rain_outside | Gustavo_C | https://freesound.org/s/768870/ |
| `rain_roof_1..3.mp3` | Rain On Tarp Texture.wav | dflee4 | https://freesound.org/s/685596/ |
| `drips_1..3.mp3` | forest drop rain.wav | bruno.auzet | https://freesound.org/s/670303/ |
| `brook_1..3.mp3` | Kervijen babbling creek travelling.wav | bruno.auzet | https://freesound.org/s/690326/ |
| `lake_1..3.mp3` | Waves gentle med cu lake water TTP 24-96 180618.wav | TRP | https://freesound.org/s/573191/ |
| `thunder_near_1.mp3`, `thunder_middle_1.mp3` | th_loud.wav | Pingel | https://freesound.org/s/2689/ |
| `thunder_near_2.mp3`, `thunder_middle_2.mp3` | thunder crack.wav | cognito perceptu | https://freesound.org/s/347853/ |
| `thunder_far_1..3.mp3` | Dry storm thunders | fran_marenco | https://freesound.org/s/796529/ |
| `swim_1..6.mp3` | water movements swimming around various splashes random.flac | kyles | https://freesound.org/s/637958/ |
| `splash_1..3.mp3` | Foley_Natural_Water_Jump_Mono.wav | Nox_Sound | https://freesound.org/s/585744/ |
| `waterfall_1..3.mp3` | Waterfall sound. | Tom_Kaszuba | https://freesound.org/s/660255/ |
| `hail_1..3.mp3` | Hail and Rain in the Courtyard | qubodup | https://freesound.org/s/162213/ |
| `cave_1..3.mp3` | Quarry Tunnel Ambience 1, Large Splash (a quiet stretch without the splash) | ecfike | https://freesound.org/s/476318/ |
| `lava_1..3.mp3` | Lava Loop 3 (a slowed, processed recording of a chemical boiling) | Fission9 | https://freesound.org/s/474851/ |

## Kenney UI Audio (interface sounds)

The interface's Micro switch sounds (the default click for buttons and sound for items) are
two recordings from Kenney's UI Audio pack, by Kenney Vleugels (https://kenney.nl/assets/ui-audio),
dedicated to the public domain under Creative Commons CC0 1.0 Universal
(https://creativecommons.org/publicdomain/zero/1.0/), which requires no notice; we credit
Kenney anyway. Licence checked in the pack's License.txt on 5 October 2026. Unchanged, renamed.

| Our files | Kenney's file |
| --- | --- |
| `assets/ui/sounds/micro_switch_click.ogg` | `switch13.ogg` |
| `assets/ui/sounds/micro_switch_item.ogg` | `switch14.ogg` |

## Fusion Pixel Font (Japanese text)

- Source: https://github.com/TakWolf/fusion-pixel-font (release 2026.09.25, 10 px monospaced, `ja` variant)
- Author: TakWolf, built from Ark Pixel Font (TakWolf), Galmuri (Lee Minseo) and Boutique
  Bitmap 9x9 (Cen-cyun Liu, fancy type foundry)
- Licence: SIL Open Font License 1.1 (all three sources too). Licence checked in the release's
  `OFL.txt` and `LICENSES/` on 5 October 2026.
- What we use: the font file, unmodified, for every kana and kanji the interface writes. The
  game reads its glyphs at startup and merges them under the chosen interface font
  (`voxmar_texgen::font::import`); the file itself ships as released.
- Where: `assets/ui/fonts/japanese/fusion-pixel-10px-monospaced-ja.ttf`, with the font's
  `OFL.txt` and its sources' licences in `assets/ui/fonts/japanese/LICENSES/` beside it,
  listed in `assets/ui/fonts/fallback.ron`.

The full licence text is in `assets/ui/fonts/japanese/OFL.txt`. Its notice:

```
Copyright (c) 2022, TakWolf (https://takwolf.com).

This Font Software is licensed under the SIL Open Font License, Version 1.1.
This license is copied below, and is also available with a FAQ at:
https://openfontlicense.org
```
