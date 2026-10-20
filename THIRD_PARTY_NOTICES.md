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
| `swim_2.mp3`, `swim_3.mp3`, `swim_4.mp3`, `swim_6.mp3` | water movements swimming around various splashes random.flac | kyles | https://freesound.org/s/637958/ |
| `splash_1..3.mp3` | Foley_Natural_Water_Jump_Mono.wav | Nox_Sound | https://freesound.org/s/585744/ |
| `waterfall_1..3.mp3` | Waterfall sound. | Tom_Kaszuba | https://freesound.org/s/660255/ |
| `hail_1..3.mp3` | Hail and Rain in the Courtyard | qubodup | https://freesound.org/s/162213/ |
| `cave_1..3.mp3` | Quarry Tunnel Ambience 1, Large Splash (a quiet stretch without the splash) | ecfike | https://freesound.org/s/476318/ |
| `lava_1..3.mp3` | Lava Loop 3 (a slowed, processed recording of a chemical boiling) | Fission9 | https://freesound.org/s/474851/ |

## Creature sounds from Freesound (animal voices)

The creatures' voices (`assets/content/base/sounds/creatures/`, listed in
`assets/content/base/creature_sounds.ron`) were cut from the Freesound recordings below,
whose licences were checked on each sound's page on 6 October 2026. We used Freesound's
high-quality previews and changed them: cut into single calls, high-passed and lightly
denoised, faded in and out, levelled, some sped up or slowed down (hurt cries and dying
calls are made from calls), mixed to mono and encoded as Ogg Vorbis. The game also plays
them faster or slower for each creature's size and shimmers the aether creatures' ones.

Licensed under Creative Commons Attribution 4.0 International
(https://creativecommons.org/licenses/by/4.0/), each credited to its recordist as
Freesound asks; "Fox Call - Cleanup of sound by Janneair" is Copyright 2013 Iwan
Gabovitch (qubodup), CC-BY, cleaned up from Janneair's recording. The Finnish
Broadcasting Company's bear recording is "Karhut murisevat / A couple of bears growling,
brown bear" by Yle Arkisto (YleArkisto), recorded at Ähtäri zoo in 1975.

| Our files | Recording | Recordist | Source |
| --- | --- | --- | --- |
| `horse/` graze_1, graze_2, graze_3, graze_4, graze_5, graze_6 | Horse Eating 4416.wav | crattray1997 | https://freesound.org/people/crattray1997/sounds/578138/ |
| `deer/` death_2, ready_1, ready_2 | 20100912.22h.10m.roaring.wav | dobroide | https://freesound.org/people/dobroide/sounds/104655/ |
| `heron/` alert_1, alert_2, alert_3, alert_4, call_1, call_2, call_3, hurt_1 | 20081221.grey.heron.wav | dobroide | https://freesound.org/people/dobroide/sounds/65264/ |
| `squirrel/` alert_5, alert_6 | angry-squirrel1.wav | echobones | https://freesound.org/people/echobones/sounds/122261/ |
| `fox/` call_3, hurt_2 | Fox, Vocal Cry, Distant, 01.wav | InspectorJ | https://freesound.org/people/InspectorJ/sounds/485009/ |
| `grazer_small/` graze_1, graze_2, graze_3 | sheep eating close MONO 004.mp3 | klankbeeld | https://freesound.org/people/klankbeeld/sounds/625479/ |
| `toad/` idle_1 | Low Toad Groan | Metris | https://freesound.org/people/Metris/sounds/721978/ |
| `grazer_large/` graze_4, graze_5 | Cow Chewing On Grass | OpenWindow_2023_Group_14 | https://freesound.org/people/OpenWindow_2023_Group_14/sounds/707941/ |
| `fox/` call_1, call_2, idle_1, idle_2, idle_3 | Fox Call - Cleanup of sound by Janneair | qubodup | https://freesound.org/people/qubodup/sounds/192782/ |
| `pheasant/` alert_4, ready_1, ready_2 | Pheasant-male-crow with wing flap | Sonic-ranger | https://freesound.org/people/Sonic-ranger/sounds/234382/ |
| `bear/` alert_1, alert_2, attack_1, attack_2, attack_3, call_1, call_2, death_1, hurt_1, idle_1, idle_2, idle_3 | Karhut murisevat / A couple of bears growling, brown bear | YleArkisto | https://freesound.org/people/YleArkisto/sounds/249441/ |

Dedicated to the public domain under Creative Commons CC0 1.0 Universal
(https://creativecommons.org/publicdomain/zero/1.0/), which requires no notice; we credit
the recordists anyway. Many come from TheKingOfGeeks360's "Critter Zone" pack of zoo and
farm recordings.

| Our files | Recording | Recordist | Source |
| --- | --- | --- | --- |
| `camel/` alert_1, call_1, death_1, hurt_1 | Camelids - Dromedary; Single; Close Perspective | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/801959/ |
| `camel/` idle_1 | Camelids - Dromedary; Single; Medium Perspective | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/848099/ |
| `camel/` idle_2 | Camelids - Alpaca; Moan, Close Perspective | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/840890/ |
| `camel/` idle_3 | Camelids - Alpaca; Two Moans, Close Perspective | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/839524/ |
| `parrot/` alert_1, idle_2 | Parrots - Sulfur-crested Cockatoo | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/844340/ |
| `parrot/` call_1, call_2 | Parrots - Blue-and-Gold Macaw; Long Calls, Medium Perspective | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/846538/ |
| `parrot/` call_3, death_1 | Parrots - Blue-and-Gold Macaw; Long Call | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/844349/ |
| `parrot/` hurt_1, idle_1 | Parrots - Goffin's Cockatoo, Squawk | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/759965/ |
| `cavy/` idle_1, idle_2, idle_3 | capybara | %D0%9A%D0%B0%D0%BF%D1%8B%D0%B1%D0%B0%D1%80%D0%B0_12345 | https://freesound.org/people/%D0%9A%D0%B0%D0%BF%D1%8B%D0%B1%D0%B0%D1%80%D0%B0_12345/sounds/849980/ |
| `wolf/` alert_4 | dog_growling 2.wav | 15GPanskaBokstefflova_Nicola | https://freesound.org/people/15GPanskaBokstefflova_Nicola/sounds/461518/ |
| `wolf/` death_1, idle_1, idle_2, ready_1, ready_2 | dog_whining 1.wav | 15GPanskaBokstefflova_Nicola | https://freesound.org/people/15GPanskaBokstefflova_Nicola/sounds/461527/ |
| `slime/` hurt_1 | Slime 27.wav | Archos | https://freesound.org/people/Archos/sounds/433840/ |
| `zombie/` attack_1 | Zombie gargles | Breviceps | https://freesound.org/people/Breviceps/sounds/445983/ |
| `wolf/` attack_1, attack_2, death_2, idle_4 | Whining Dog | Breviceps | https://freesound.org/people/Breviceps/sounds/462660/ |
| `toad/` hurt_1, hurt_2, hurt_3, hurt_4, hurt_5 | Common toad (Bufo bufo) | Breviceps | https://freesound.org/people/Breviceps/sounds/462863/ |
| `bear/` alert_3, hurt_2 | Bear Angry Growl | celldroid | https://freesound.org/people/celldroid/sounds/763026/ |
| `bat/` idle_5, idle_6, idle_7 | Bat chirp (close-up) | coelhoigor | https://freesound.org/people/coelhoigor/sounds/455593/ |
| `horse/` alert_1, idle_4, idle_5 | G38-16-Four Horse Snorts.wav | craigsmith | https://freesound.org/people/craigsmith/sounds/437111/ |
| `bat/` idle_1, idle_2, idle_3, idle_4 | G12-02-Bats Chirping.wav | craigsmith | https://freesound.org/people/craigsmith/sounds/437934/ |
| `croc/` alert_1, alert_2, attack_1, attack_2, call_1, hurt_1 | S27-21 Alligator hiss twice.wav | craigsmith | https://freesound.org/people/craigsmith/sounds/675513/ |
| `rabbit/` graze_1, graze_2, graze_3, graze_4 | A rabbit is eating a cucumber | davi-sh | https://freesound.org/people/davi-sh/sounds/482732/ |
| `duck/` alert_1, alert_2, hurt_2, idle_3, idle_4, idle_5 | Duck quacking.mp3 | DigPro120 | https://freesound.org/people/DigPro120/sounds/558858/ |
| `grazer_large/` graze_1, graze_2, graze_3, rest_1, rest_2 | Cow Breathing and Chewing | elricadavis | https://freesound.org/people/elricadavis/sounds/764587/ |
| `cattle/` call_1, death_1 | Cow mooing in south of France (Limousin) | felix.blume | https://freesound.org/people/felix.blume/sounds/163727/ |
| `horse/` call_1, death_1 | Horse neigh | felix.blume | https://freesound.org/people/felix.blume/sounds/385947/ |
| `deer/` alert_1, alert_2, alert_3 | Deer_Bark_5_s | ferventtorpor | https://freesound.org/people/ferventtorpor/sounds/696776/ |
| `hedgehog/` idle_1, idle_2, idle_3, idle_5 | Hedgehog smell and run.mp3 | ffdown | https://freesound.org/people/ffdown/sounds/570301/ |
| `hedgehog/` alert_1, alert_2, alert_3, alert_4, alert_5 | Angry hedgehog sniffing 1 | fthgurdy | https://freesound.org/people/fthgurdy/sounds/528183/ |
| `deer/` call_1, call_2, call_3, ready_4 | Bellowing deer - Animal - Nature - Hirsch - Röhren - Tiere - Wald | IchBinChrist | https://freesound.org/people/IchBinChrist/sounds/407631/ |
| `croc/` idle_1, idle_2, idle_3 | Tiny Croc Chirp | iwanPlays | https://freesound.org/people/iwanPlays/sounds/511430/ |
| `fowl/` ready_2 | Chicken - Buck 2 - 96kHz.wav | JarredGibb | https://freesound.org/people/JarredGibb/sounds/233097/ |
| `pig/` graze_1, graze_2, graze_3, graze_4, graze_5, graze_6 | Pig - Breathing and chewing 1 - 96kHz.wav | JarredGibb | https://freesound.org/people/JarredGibb/sounds/233111/ |
| `pig/` alert_4 | Pig - Multiple Snorts 2 (2 pigs) - 96kHz.wav | JarredGibb | https://freesound.org/people/JarredGibb/sounds/233183/ |
| `pig/` rest_2, rest_3, rest_4 | Pig - Multiple Breaths 1 - 96kHz.wav | JarredGibb | https://freesound.org/people/JarredGibb/sounds/233186/ |
| `pig/` rest_1 | Pig - Moan 1 - 96kHz.wav | JarredGibb | https://freesound.org/people/JarredGibb/sounds/233187/ |
| `pig/` alert_3, attack_1 | Angry Pig Oinking | Jofae | https://freesound.org/people/Jofae/sounds/352698/ |
| `rabbit/` alert_1, alert_2 | Rabbit Thump on Soil | kessir | https://freesound.org/people/kessir/sounds/386007/ |
| `goat/` call_1, call_2, death_1, hurt_2 | Single Goat Bleating 2x | Kinoton | https://freesound.org/people/Kinoton/sounds/581240/ |
| `cat/` attack_3, call_1, call_2, death_1, hurt_1, ready_1 | Angry Cat | Kinoton | https://freesound.org/people/Kinoton/sounds/656619/ |
| `wolf/` hurt_1, hurt_2 | bark yelp dog small int.flac | kyles | https://freesound.org/people/kyles/sounds/452180/ |
| `deer/` ready_3 | Deer rut - Brame Cerf 02.wav | L.Finck | https://freesound.org/people/L.Finck/sounds/653533/ |
| `rabbit/` idle_1 | bunny sneeze | lucyve | https://freesound.org/people/lucyve/sounds/205576/ |
| `horse/` attack_2, call_2, call_3, death_2, hurt_1, ready_2 | Horse_neighing.wav | Lydmakeren | https://freesound.org/people/Lydmakeren/sounds/510903/ |
| `deer/` death_1, hurt_1, hurt_2 | baby deer4.wav | motchka | https://freesound.org/people/motchka/sounds/369795/ |
| `pig/` death_1, death_2, hurt_1, hurt_2 | Pig in slop squealing | mrmunk | https://freesound.org/people/mrmunk/sounds/344972/ |
| `deer/` idle_1, idle_2, idle_3, idle_4, idle_5 | Deer Meewing 01.wav | msantoro11 | https://freesound.org/people/msantoro11/sounds/351113/ |
| `wolf/` alert_1, alert_2, alert_3, attack_3 | dog_growling_mono_4824.wav | Mystikuum | https://freesound.org/people/Mystikuum/sounds/401820/ |
| `moth/` idle_1, idle_2, idle_3, idle_4 | Flying moth 1.wav | nahmandub | https://freesound.org/people/nahmandub/sounds/131339/ |
| `wolf/` rest_1, rest_2, rest_3 | dog snoring | nielstii | https://freesound.org/people/nielstii/sounds/346319/ |
| `fox/` death_1, hurt_1, ready_1, ready_2, ready_3, ready_4 | Fox scream in the forest | nielstii | https://freesound.org/people/nielstii/sounds/554308/ |
| `horse/` alert_2, idle_3 | Horse(snort)_3.wav | o_ciz | https://freesound.org/people/o_ciz/sounds/475483/ |
| `wolf/` call_1 | wolfs.wav | ondrosik | https://freesound.org/people/ondrosik/sounds/143568/ |
| `zombie/` alert_1, attack_2 | Zombie Groan 0 | OwNathan | https://freesound.org/people/OwNathan/sounds/754438/ |
| `zombie/` alert_2, hurt_1 | Zombie Groan 3 | OwNathan | https://freesound.org/people/OwNathan/sounds/754441/ |
| `wolf/` call_2, idle_3 | Dog Long Greeting Yelp | qubodup | https://freesound.org/people/qubodup/sounds/827539/ |
| `wolf/` hurt_3 | Dog Growling and Running.wav | ross_sinc | https://freesound.org/people/ross_sinc/sounds/444789/ |
| `sheep/` alert_2 | Ewe Shetland Sheep Baa | satoristudios3 | https://freesound.org/people/satoristudios3/sounds/692900/ |
| `fox/` alert_4 | fox_barks_cut.wav | Setzilla | https://freesound.org/people/Setzilla/sounds/173188/ |
| `fox/` alert_1, alert_2, alert_3 | foxBark2.wav | Setzilla | https://freesound.org/people/Setzilla/sounds/173208/ |
| `frog/` idle_1, idle_2, idle_3, idle_4, idle_5, idle_6, idle_7 | Frog Croaking (UK Common Frog) | shaunhillyard | https://freesound.org/people/shaunhillyard/sounds/532235/ |
| `reindeer/` idle_6, idle_7 | Reindeer.mp3 | Stillerzzy | https://freesound.org/people/Stillerzzy/sounds/676278/ |
| `crab/` step_6, step_7, step_8 | crab walking | stuniverso | https://freesound.org/people/stuniverso/sounds/761559/ |
| `badger/` graze_1, graze_2, graze_3, graze_4, graze_5, graze_6; `tortoise/` graze_1, graze_2 | badger eating peanuts | TEDAgame | https://freesound.org/people/TEDAgame/sounds/527390/ |
| `reindeer/` idle_1, idle_2, idle_3, idle_4, idle_5, ready_1, ready_2 | Reindeer Sound Mating Male Park Japan | thabigm | https://freesound.org/people/thabigm/sounds/758377/ |
| `fowl/` hurt_1 | Fowl - Chickens, Rooster and Hen Clucking | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/735767/ |
| `frog/` call_1 | Frogs - Cope's Gray Tree Frog, Single Croak, Crickets in Background | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/744309/ |
| `raptor/` alert_1, alert_2, alert_3, hurt_1, idle_1, idle_2, idle_3, idle_4 | Birds of Prey - Bald Eagle Chirping, Close Perspective | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/751269/ |
| `squirrel/` alert_1, alert_2, alert_3, alert_4 | Rodents - Eastern Gray Squirrel, Bark and Chatter, Slightly Reverberant, B/G Insects, Distant | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/753942/ |
| `bat/` alert_1 | Bats - Egyptian Fruit Bats | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/754020/ |
| `cat/` idle_3 | House Cats - Calico Queen Cat, Soft Meow and Purr | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/755363/ |
| `insect/` alert_4 | Insects - Madagascar Hissing Cockroach, Short Hiss | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/758900/ |
| `cavy/` idle_4, idle_5, idle_6, idle_7 | Rodents - American Guinea Pig, Rumble and Sniff, Close Perspective | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/759812/ |
| `cattle/` alert_2, call_3, ready_1 | Bovines - Cattle; Bull Bellowing | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/768389/ |
| `badger/` idle_1, idle_2 | Small Mammals - Kusimanse Mongoose; Two Chirps | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/777591/ |
| `goat/` idle_3 | Goats - Goat Bleat; Close Perspective | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/777755/ |
| `cattle/` hurt_1, idle_3 | Bovines - Cattle; Cow Mooing | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/777756/ |
| `horse/` ready_1 | Horses - Quarter Horse; Soft Neigh, Close Perspective | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/777763/ |
| `pig/` call_1 | Swine - Pig; Various Pig Grunts, Close Perspective | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/778119/ |
| `badger/` alert_1, alert_2, alert_3, alert_4, attack_1, attack_2, attack_3, hurt_1, hurt_2 | Small Mammals - Kusimanse Mongoose; Two Mongooses Snarling, Budgies in Background | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/784607/ |
| `cattle/` hurt_2, idle_2 | Bovines - Cattle; Cow Mooing, Take 2 | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/787562/ |
| `duck/` call_1, hurt_1, idle_1, idle_2 | Waterfowl - Mallard Drake Quack, Close Perspective | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/792532/ |
| `mouse/` idle_5, idle_6, idle_7 | Rodents - Mouse; House Mouse, Chatter, Close Perspective | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/796878/ |
| `sheep/` call_1, death_1 | Sheep - Bleat; Close Perspective | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/803460/ |
| `cat/` rest_1, rest_2, rest_3 | House Cats - Tom Cat, Purr | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/807969/ |
| `insect/` alert_1, alert_2, alert_3, attack_2, hurt_1 | Insects - Madagascar Hissing Cockroach, Several Hisses | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/811205/ |
| `pig/` idle_1, idle_10, idle_2, idle_3, idle_4, idle_5, idle_6, idle_7, idle_8, idle_9 | Swine - Pig; Various Pig Grunts, Close Perspective, Take 2 | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/811457/ |
| `cavy/` hurt_1, hurt_2; `rabbit/` death_1, hurt_1, hurt_2, hurt_3 | Rodents - American Guinea Pig, Squealing, Close Perspective | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/815837/ |
| `heron/` idle_1 | Water Birds - Anhinga; Croak and Flapping Wings | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/815872/ |
| `cat/` idle_1, idle_2 | House Cats - White Tomcat; Two Meows | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/815874/ |
| `goat/` call_3 | Goats - Billy Goat Bleat | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/825399/ |
| `pig/` idle_11 | Swine - Pig; Grunt | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/825614/ |
| `raptor/` call_1, call_2, call_3, call_4 | Birds of Prey - Red-shouldered Hawk | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/826045/ |
| `cattle/` alert_1, ready_2 | Bovines - Cattle; Loud Bellow from Hereford Bull | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/827111/ |
| `pheasant/` alert_1, alert_2, alert_3, idle_1, idle_2, idle_3, idle_4, idle_5 | Fowl - Silver Pheasant; Chatter and Squeal, Close Perspective, some Walla in Background | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/828647/ |
| `quail/` call_5 | Fowl - Japanese Quail; Trill, Close Perspective | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/829980/ |
| `horse/` rest_1, rest_2 | Mules - Mule Breaths, Close Perspective | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/832102/ |
| `duck/` alert_3, call_2, call_3, idle_6 | Waterfowl - Ducks; Various Domestic Ducks Quacking, Close Perspective | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/832103/ |
| `horse/` ready_3 | Horses - Mini Horse or Pony, Short Neigh, Close Perspective | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/839525/ |
| `zombie/` idle_1, idle_2, idle_3 | Swine - Pig; Almost Dinosaur-like Moans and some Grunts, Sheep in Background | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/839526/ |
| `fowl/` call_2 | Fowl - Chicken, Rooster Crow | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/839538/ |
| `pig/` call_2 | Swine - Warthog, Close Perspective | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/839828/ |
| `cavy/` rest_1, rest_2, rest_3, rest_4 | Rodents - Guinea Pig, Coronet, Bubble, Close Perspective | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/840081/ |
| `sheep/` hurt_1, idle_2 | Sheep - Bleat; Medium Perspective | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/840891/ |
| `fowl/` call_1 | Fowl - Chicken, Rooster Crow, Take 2 | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/842306/ |
| `goat/` hurt_1, idle_2 | Goats - Goat Bleat; Close Perspective, Take 2 | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/842307/ |
| `goat/` idle_1 | Goats - Goat Bleat; Close Perspective, Take 3 | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/842308/ |
| `quail/` call_3, call_4, hurt_1, idle_2, idle_3, idle_4 | Fowl - Japanese Quail; Three Trills, Various Perspectives | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/842309/ |
| `quail/` call_2, idle_1 | Fowl - Japanese Quail; Trill | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/842310/ |
| `quail/` call_1 | Fowl - Japanese Quail; Trill, Take 2 | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/842311/ |
| `pig/` alert_1, alert_2, attack_2 | Swine - Pig; Dinosaur-like Grunt | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/842313/ |
| `zombie/` death_1, idle_4 | Ratites - Cassowary, Groan | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/843486/ |
| `cat/` alert_4 | Wild Cats - Siberian Lynx, Huff | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/844316/ |
| `bison/` idle_4 | Bovines - Water Buffalo | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/844327/ |
| `sheep/` hurt_2, idle_1 | Sheep - Bleat; Medium Perspective, Take 2 | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/844560/ |
| `fowl/` alert_1, alert_2, alert_3, alert_4, death_1, hurt_2 | Fowl - Chickens; Frantic Clucking | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/844569/ |
| `goat/` ready_1 | Goats - Billy in Rut | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/846010/ |
| `fowl/` call_4 | Fowl - Gray Junglefowl | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/846537/ |
| `sheep/` ready_2 | Sheep - Several Sheep Bleating | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/847403/ |
| `cattle/` idle_4 | Bovines - Cattle; Cow Mooing, Take 3 | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/847404/ |
| `pheasant/` call_1 | Fowl - Common Pheasant, Crow | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/849184/ |
| `pheasant/` call_2, hurt_1 | Fowl - Golden Pheasant | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/849580/ |
| `tortoise/` ready_1 | Reptiles - Sulcata Tortoise; Breeding Croak from Male | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/850006/ |
| `fowl/` call_3, ready_1 | Fowl - Ceylon Junglefowl | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/850007/ |
| `squirrel/` idle_1, idle_2, idle_3, idle_4, idle_5, idle_6 | Rodents - Eastern Gray Squirrel; Chatter, Close Perspective | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/852386/ |
| `cattle/` call_2, death_2, idle_1 | Bovines - Cattle; Cow Mooing, Take 4 | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/856047/ |
| `horse/` call_4, hurt_2 | Horses - Palomino, Neigh | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/856359/ |
| `horse/` idle_2, rest_3 | Horses - Nicker, Single | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/861233/ |
| `horse/` idle_1 | Horses - Soft Nicker | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/861234/ |
| `goat/` ready_2 | Goats - Billy in Rut, Take 3 | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/861243/ |
| `goat/` attack_1 | Goats - Two Billies Sparring; Headbutt, Horns Clanking | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/866683/ |
| `frog/` call_2 | Frogs - Cope's Gray Tree Frog, Single | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/866772/ |
| `bison/` alert_1, idle_1, idle_2, idle_3 | Bovines - American Bison; Nasal Huffs, Close Perspective | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/867549/ |
| `insect/` attack_1 | Rodents - African Crested Porcupine; Rattling Quills, Close Perspective | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/867551/ |
| `cat/` alert_1 | Wild Cats - Serval, Hiss, some Room Tone, Close Perspective | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/867552/ |
| `toad/` call_1 | Frogs - Eastern Narrowmouth Toads; Several Calling in Puddle | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/868037/ |
| `horse/` attack_1, call_5, hurt_3 | Horses - High Neigh | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/868300/ |
| `horse/` alert_3, idle_6 | Horses - Snort; Distant | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/868302/ |
| `fowl/` idle_1, idle_10, idle_2, idle_3, idle_4, idle_5, idle_6, idle_7, idle_8, idle_9 | Fowl - Chickens; Cornish Hens Clucking | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/870780/ |
| `tortoise/` ready_2 | Reptiles - Sulcata Tortoise; Breeding Honk from Male | TheKingOfGeeks360 | https://freesound.org/people/TheKingOfGeeks360/sounds/871251/ |
| `slime/` death_1, hurt_2, hurt_3 | squelching.mp3 | tiddles451 | https://freesound.org/people/tiddles451/sounds/547446/ |
| `wings/` step_1, step_2, step_3 | Light Wing Flap | TurboFool | https://freesound.org/people/TurboFool/sounds/561009/ |
| `crab/` step_1, step_2, step_3, step_4, step_5 | Crab - Legs | Vrymaa | https://freesound.org/people/Vrymaa/sounds/770031/ |
| `cat/` alert_3, attack_1, attack_2, hurt_2 | Cat Growl into Hiss.aif | Wrenasmir | https://freesound.org/people/Wrenasmir/sounds/455495/ |
| `cat/` alert_2 | catHisses1.wav | Zabuhailo | https://freesound.org/people/Zabuhailo/sounds/146963/ |
| `sheep/` alert_1, call_2, call_3, call_4, death_2, hurt_3, idle_3, idle_4, idle_5, ready_1 | Sheep bleating | zachrau | https://freesound.org/people/zachrau/sounds/362283/ |
| `wings/` step_4, step_5, step_6 | short pigeon wing flap | zouzmusic | https://freesound.org/people/zouzmusic/sounds/414671/ |

The remaining 70 clips are synthesized (no recording): snail slime, moth wings, lizard
and tortoise hisses, crab and insect clicks, ember crackles, mouse, squirrel, hedgehog and
bat squeaks, and the wisps' chimes and whispers.

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

## FluidR3 GM sound font (the quest and level fanfares, the home and settlement motifs)

The quest-finished, level-up and tenth-level fanfares (`assets/content/base/sounds/quest_done_1.ogg`,
`level_up_1.ogg`, `level_milestone_1.ogg`) are our own tunes, written as notes in
`tools/generate_jingles.py` and played offline by FluidSynth from the FluidR3 General MIDI
sound font, Copyright (c) 2000-2002, 2008 Frank Wen and (c) 2008 Toby Smithe, released under
the MIT licence (the font's README and COPYING; checked in the Ubuntu fluid-soundfont-gm
package on 14 October 2026). The font itself is not shipped. The home motif
(`assets/music/home/hearth_welcome.ogg`) is our own lullaby, written in
`tools/generate_home_motif.py` and played the same way, as is the settlement motif
(`assets/music/settlement/market_day.ogg`), our own tune in `tools/generate_settlement_motif.py`,
which records the font that played it (its name and SHA-256) in the clip's comment tag.

> Permission is hereby granted, free of charge, to any person obtaining a copy of this
> software and associated documentation files (the "Software"), to deal in the Software
> without restriction, including without limitation the rights to use, copy, modify, merge,
> publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons
> to whom the Software is furnished to do so, subject to the following conditions:
>
> The above copyright notice and this permission notice shall be included in all copies or
> substantial portions of the Software.
>
> THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED,
> INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR
> PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE
> FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR
> OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
> DEALINGS IN THE SOFTWARE.

## Fusion Pixel Font (Japanese text)

- Source: https://github.com/TakWolf/fusion-pixel-font (release 2026.09.25, 12 px monospaced, `ja` variant)
- Author: TakWolf, built from Ark Pixel Font (TakWolf), Galmuri (Lee Minseo) and Cubic 11
  (itself built on JF Dot M+H 12, M+ FONTS PROJECT, and the M+ bitmap fonts, COZ)
- Licence: SIL Open Font License 1.1; Galmuri OFL 1.1; Cubic 11 OFL 1.1 with the M+ fonts'
  licence (free use, copying and distribution, with or without changes) for the glyphs it
  takes from them. Licence checked in the release's
  `OFL.txt` and `LICENSES/` on 5 October 2026.
- What we use: the font file, unmodified, for every kana and kanji the interface writes. The
  game reads its glyphs at startup and merges them under the chosen interface font, its
  pixels drawn at three quarters of ours (`voxmar_texgen::font::import`); the file itself
  ships as released.
- Where: `assets/ui/fonts/japanese/fusion-pixel-12px-monospaced-ja.ttf`, with the font's
  `OFL.txt` and its sources' licences in `assets/ui/fonts/japanese/LICENSES/` beside it,
  listed in `assets/ui/fonts/fallback.ron`.

The full licence text is in `assets/ui/fonts/japanese/LICENSES/fusion-pixel/OFL.txt`. Its notice:

```
Copyright (c) 2022, TakWolf (https://takwolf.com).

This Font Software is licensed under the SIL Open Font License, Version 1.1.
This license is copied below, and is also available with a FAQ at:
https://openfontlicense.org
```

## BIZ UDGothic (Japanese text)

- Source: https://github.com/googlefonts/morisawa-biz-ud-gothic (the Regular weight as
  published in Google Fonts, `ofl/bizudgothic/BIZUDGothic-Regular.ttf`)
- Author: Morisawa Inc., The BIZ UDGothic Project Authors
- Licence: SIL Open Font License 1.1 (checked in the font's `OFL.txt` on 6 October 2026)
- What we use: the font file, unmodified, for any kana or kanji Fusion Pixel Font lacks.
  Text falls back to it for those scripts at the text's own size (`crates/client/src/lang.rs`).
- Where: `assets/ui/fonts/japanese/BIZUDGothic-Regular.ttf`, with its `OFL.txt` beside it,
  listed in `assets/ui/fonts/fallback.ron`.

The full licence text is in `assets/ui/fonts/japanese/OFL.txt`. Its notice:

```
Copyright 2022 The BIZ UDGothic Project Authors (https://github.com/googlefonts/morisawa-biz-ud-mincho)

This Font Software is licensed under the SIL Open Font License, Version 1.1.
This license is copied below, and is also available with a FAQ at:
https://openfontlicense.org
```
