# Weather effects: what is drawn, and what the renderer still has to draw

Oct 4, 2026. The weather track (W5) owns the weather's state: the server simulates it,
`weather.ron` turns it into effects, and the client eases it and exposes it. This document
is the handover to the graphics thread for the effects that need render passes or sky and
terrain shader changes, which the weather track does not make. It says exactly what each
effect reads and how it should look.

## Where the state is

Every frame on the client (all in `client/src/weather.rs` and `weather/atmosphere.rs`):

| Resource | What | Notes |
| --- | --- | --- |
| `ClientWeather::current` | `voxmar_engine::weather::Weather`: every channel below, eased | `wind` already includes the gusts at the camera |
| `ClientWeather::flash()` | lightning brightness 0..1 | already lights sky, land and ambient (render/mod.rs) |
| `ClientWeather::bolts()` | `Bolt { channel, branches, brightness }` per visible bolt | world-space polylines; see Lightning bolts |
| `ClientWeather::effects` | names of the effects showing (`valley_fog`, `hail`) | for debugging (F3 shows them) |
| `Wind` | mean wind, gustiness, gust offset, world seconds | see docs/rendering.md, "Wind" |
| `ClientAtmosphere` | haze colour and density, sky tint, warmth, saturation, ambient particle sets | the land's air (biomes) with the weather applied |

`--weather KIND` shows any kind from `weather.ron` without a server, `--command "/weather
KIND"` forces it on the server, `--wait SECONDS` lets a capture run longer first, and
captures log the weather and air they show. Every kind: `/weather list`.

## Already drawn (no renderer work needed)

- Rain, drizzle (finer, slower, wind-blown streaks), snow, sleet (small icy pellets), hail
  (bright short streaks), dust and sand (specks streaming along the wind, in the ground's
  colour), glowing motes, and the ambient particle sets of `ClientAtmosphere` (pollen,
  fireflies that blink, spores, drifting snow, marsh mist, aether motes): all in the
  existing precipitation particles (`render/precipitation.rs` and `.wgsl`, contained
  additions; a group of drops per kind, the same box around the camera and height map).
  When the graphics thread's reusable particle system lands, these move to it; the data
  that drives them (`Weather` channels, `ClientAtmosphere::particles`) stays.
- Rain is tinted half towards the sky's haze, so it matches the sky it falls from, and
  its streaks lie along its motion as seen from the moving camera, so running into rain
  tilts it towards you (Particle Rain's trick). A sparser layer of faint streaks in a box
  three times as wide carries rain (or, as specks, snow) into the distance. Raindrops
  splash as crowns on the ground and ring out as ripples on water; under leaves they come
  through as slow, heavy drips that carry on for a few minutes after the rain, as the
  ground dries. Snowflakes are mostly fine with a few large ones, six-armed, turning and
  glinting as they flutter down.
- Every kind of weather has an intensity (`/weather rain 0.3`, `--weather snow:heavy`):
  what falls scales with it, from a few streaks to a downpour that fills the view.
- Rain, snow and particles slant and drift with the gusting wind; clouds drift with the
  mean wind; grass sways with `Wind` (graphics thread).
- Lightning: the flash (existing) and the bolt, drawn for now as bright gizmo lines that
  bloom (`draw_bolts` in weather.rs); replace with real geometry, below.
- Snow lying on the ground: the server turns grass blocks snowy as the snow cover deepens
  and back as it melts (`snow_cover` in weather.rs; data in `weather.ron`).
- Cloud cover, heaped and layered cloud, cirrus, storm gloom (existing sky).

## Waiting on the graphics thread

In order of how much they show. Each is a few lines to a small pass; all read
`ClientWeather::current` (call it `w`) unless said.

### 1. Fog, mist, whiteout and dust haze (most visible; affects every common effect)

Today `render/mod.rs` closes the distance fog in only for rain, snow and humidity
(`murk`). Weather now also has `fog` (mist, sea fog, whiteout in blizzards, drizzle and
downpours), `dust` (dust and sandstorms) and `Weather::visibility()` (blocks), and
`ClientAtmosphere` has the haze colour and density for the place. Replace the `murk` lines
in `update_environment` with:

```rust
let visibility = shown.visibility();                       // ~3000 clear .. 30 in thick fog
let end = (cycle.view_distance).min(visibility * 1.6);
let murk = 1.0 - (visibility / 3000.0).clamp(0.0, 1.0);
(end * (0.6 - 0.55 * murk), end)
```

and tint the fog colour (and the haze in lighting.wgsl) towards the air's haze:

```rust
let air = &atmosphere;                                     // Res<ClientAtmosphere>
let thick = (shown.fog + shown.dust).min(1.0);
horizon = horizon.lerp(air.haze * horizon.length().max(0.05) * 1.6, thick * 0.8);
// and the pack's haze density: pack.haze * air.haze_density
```

So in a blizzard the world fades to white-grey within ~60 blocks, in a sandstorm to the
sand's colour within ~150, and mist after rain softens the distance without hiding it.
`ClientAtmosphere::haze_density` is already about 1 in clear air and grows with fog, dust,
rain and humidity; the pack's own haze setting stays in charge of the look.

### 2. Ground fog: valley fog at dawn, sea fog, dawn mist

`w.ground_fog` (0..1) is a layer of fog whose top is `w.fog_top` (world height): thick
below it, thinning over about 6 blocks above. Valley fog fills hollows to about 14 blocks
over the lowest land nearby; sea fog lies 30 blocks over the sea; dawn mist 6 blocks.
Height fog in the lighting/haze path, per fragment from the camera at `c` to the point `p`:

```wgsl
// density(y) = ground_fog * k for y < top, falling smoothly to 0 by top + 6.
fn ground_fog(c: vec3<f32>, p: vec3<f32>, top: f32, amount: f32) -> f32 {
    let k = 0.06;                                   // per block at full density
    let below = clamp((top + 3.0 - min(c.y, p.y)) / max(abs(p.y - c.y), 1e-3), 0.0, 1.0);
    let inside = select(below, 1.0, max(c.y, p.y) < top + 3.0);
    return 1.0 - exp(-amount * k * length(p - c) * inside);
}
```

mixed towards the haze colour (lit like the haze, so it glows at dawn). From above (a hill
over a valley) the layer should read as a white sea between ridges; from inside, as fog.

### 3. Lightning bolts

`ClientWeather::bolts()` yields each visible bolt: `channel` (points from the cloud base,
150–220 blocks up, to the strike point, about 10 blocks apart), `branches` (forks that
end in the air) and `brightness` (flickers with the flash, gone in 0.45 s). Draw camera-
facing ribbons along the polylines, 0.5 blocks wide for the channel and 0.25 for branches,
additive, colour `(0.75, 0.8, 1.0) * 8.0 * brightness` (HDR, so bloom gives the glow), not
fogged by more than half. Then delete `draw_bolts` in weather.rs. Thunder is already
delayed by distance (343 blocks/s) and played by sfx.rs.

### 4. Rainbows

`w.rainbow`: 0.6 a rainbow, 1.0 a double one. In sky.wgsl, with `a` the angle between the
view direction and the antisolar point (`-sun`), when the sun is above the horizon:

- primary bow: hue from red at 42° to violet at 40.5° (`hue = (42 - a) / 1.5`), brightness
  `0.35 * rainbow * daylight` with soft edges, a little brighter sky inside it (a < 40°:
  ×1.05);
- if rainbow > 0.75: secondary bow 50°–53.5° with the colours reversed at 0.4 of the
  brightness, and Alexander's dark band between 42° and 50° (sky ×0.92);
- it only exists below the horizon line of the sky dome and fades where cloud cover is
  thick in that direction.

### 5. Halo and sun dogs

`w.halo`: up to 0.5 a 22° halo, above that sun dogs too. Ring at 21.5°–23° around the sun,
reddish on the inner edge, `0.25 * halo * daylight`. Sun dogs: two bright, slightly
coloured spots at ±22° of azimuth from the sun at the sun's height, strength
`2 * (halo - 0.5)`, strongest with a low sun. Also around the moon at night, fainter.

### 6. Aurora

`w.aurora` (0..1; 0.6 for the rare aurora far from the poles). Night sky only: curtains
towards the pole (north is −z), from about 10° to 60° above the horizon. Vertical rays from
a 1D noise along azimuth, folded ribbons drifting slowly (a few degrees a second), green
`(0.2, 1.0, 0.45)` at the bottom fading to violet `(0.6, 0.3, 1.0)` at the top, brightness
`aurora * night * (1 - cover)` at about the Milky Way's peak, so it needs a dark sky.

### 7. Falling stars and meteor showers

`w.falling_stars`: about `2 × falling_stars` stars a second, so 0.01 (an ordinary clear
night) is one every 50 s or so and 1.0 (a meteor shower) a couple a second, all from one
radiant point per night. Night sky: short-lived
(0.3–0.8 s) streaks 5°–15° long, brightening then fading, from hashes of the second (use
the `pcg` already in sky.wgsl with the world seconds), only through clear sky.

### 8. Heat haze

`w.heat_haze` (0..1): hot, dry, sunny afternoons. A post-process shimmer near the horizon:
offset the screen UV by a scrolling noise of amplitude `1.5 px * heat_haze`, masked to
pixels farther than ~30 blocks and within ~8° of the horizon, rising slowly.

### 9. Wet ground and puddles

`w.wetness` (0..1): rises in rain, halves in 300 s (faster in sun and warmth), 0 under snow.
In the terrain shader, on faces open to the sky (sky light 15): darken albedo by
`0.3 * wetness`, raise specular/reflectance; on flat top faces, puddles where a world-space
noise is below `wetness - 0.4` reflect the sky. Never under roofs, in caves or under dense
leaves. (W12 shader pack.)

### 10. Frost and lying snow on surfaces

`w.frost` (0..1, clear cold nights and mornings): whiten top faces and plants with a
blue-white rime, `0.5 * frost`, on surfaces open to the sky. `w.snow_cover` (0..1): the
server already turns grass snowy block by block; a shader can add a thin white dusting on
other top faces (stone, sand, leaves) at `snow_cover > 0.3`, open to the sky.

### 11. Wisps and fantastical storms

`w.wisps` (ball lightning in storms, marsh lights on calm wet nights): a few (up to about
12) large glowing orbs, 0.3–0.6 blocks, drifting 0.5–3 blocks above the ground within 30
blocks, pale blue-green, slowly pulsing. For now they add to the motes in the particles
(`motes.max(wisps * 0.3)`), which are smaller. `w.motes` and `w.falling_stars` together
with `aether_storm` in `ClientWeather::effects` mark the rare aether storm: a violet tint to
the storm cloud and the lightning would sell it.

### 12. The air's colour

`ClientAtmosphere::{haze, haze_density, sky, warmth, saturation}`: apply within the shader
pack: haze colour and density as in 1, `sky` multiplied into the sky colours, `warmth` and
`saturation` as small nudges to the pack's grading (warmth ±1 ≈ a few hundred kelvin).

### 13. Rain and snow: what the mods and the hiking video still have that we don't

Compared side by side with Particle Rain (MIT; read for its techniques, nothing copied) and
the hiking video's rain frames (`voxmar-reference/hiking-video-frames/0440`, `0449`), what
the precipitation particles cannot do on their own:

- **Haze in and after rain** (biggest): in the video, rain turns the distance grey-white
  within a hundred blocks and the near land loses saturation; ours keeps hills sharp at
  3 km in a downpour. Item 1 (fog from `visibility()`) plus `ClientAtmosphere::haze_density`
  and `saturation` do it. Max-intensity rain should leave about 150 blocks of view, light
  rain most of it.
- **Wet surfaces**: rain darkens exposed blocks and leaves and makes them glossy (item 9);
  in the video every top face reads wet.
- **Near streaks as glassy sheets**: the video's closest streaks are wide, refracting
  smears. A refraction pass sampling the scene behind each near streak (offset by its
  normal) would give it; our streaks are only tinted.
- **Fog sheets after rain** (Particle Rain's `fog_after_rain`): flat, camera-facing cards
  lying a block above wet ground and fading edge-on, for a few minutes after rain.
  Drive them with `w.wetness` and `w.fog`.
- **Wall streaks and ledge drips**: water running down the faces of blocks open to the sky
  and dripping off their lower edges. Needs per-face spawning from the mesher (faces open to
  the sky, from `SkyMap`), so it belongs with the shader pack.
- **Storm fronts and rain shafts**: `ClientWeather::nearby_rain()` gives the rain in each
  128-block weather cell of the 640 blocks around the player. Draw a grey curtain from the
  cloud base to the ground under every cell with rain above about 0.2 beyond 150 blocks
  (darker and denser with more rain, soft-edged, swaying with the wind), so a storm is
  seen coming over the hills before it arrives.
- **Lightning inside clouds**: most flashes now stay within the cloud
  (`ClientWeather::cloud_flashes()`: position about 160 blocks up and brightness). Light the
  cloud layer around that point (a soft glow of 200–400 blocks) rather than the whole sky.
- **Snow on leaves and other surfaces**: `w.snow_cover` whitens grass blocks already; tops of
  leaves, stone and sand should whiten in the shader too (item 10).
- **A rainbow over waterfall mist**: once waterfalls are known (the ambience track finds
  them at meshing time), a small bow in the spray when the sun is behind the player.

## Channels

`voxmar_engine::weather::Weather`, fractions 0..1 unless said:

| Channel | Meaning | Raised by (weather.ron) |
| --- | --- | --- |
| cover, convection, cirrus | low cloud cover, how heaped, high ice cloud | simulation |
| humidity, temperature (°C), wind (blocks/s), gusts, gust_offset | the air | simulation, wind model |
| precipitation, snow | how much falls, and how much of it is snow | simulation |
| drizzle, hail, sleet | shares of what falls | drizzle, hail, sleet |
| fog | fog in the air | mist, sea fog, blizzard, downpour, drizzle, supercell, sandstorm, diamond dust, aether storm |
| ground_fog, fog_top | a layer of fog and its top height | valley fog, sea fog, dawn mist, lake mist, steam after rain |
| dust, dust_color | dust in the air and its colour (the ground's) | sandstorm, dust storm, dusty afternoon, drifting snow |
| wetness, snow_cover | the ground | simulation |
| frost | hoar frost | frost, freezing rain |
| lightning | strikes nearby beyond thunderstorm cells | supercell, heat lightning, thundersnow, aether storm |
| rainbow, halo, aurora, heat_haze | light in the sky | rainbow, double rainbow, halo, sun dogs, aurora, low aurora, heat haze |
| wisps, motes, falling_stars | lights in the air and sky | ball lightning, marsh lights, aether motes, aether storm, shooting stars, meteor shower, starfall |

Effects are data: a mod adds one to its `weather.ron` by naming conditions on the inputs
(weather, climate, land, time of day) and the channels it raises, with a rarity. The
renderer only ever sees channels, so new effects need no renderer change unless they need
a new kind of look.

## Gameplay hooks (no gameplay yet)

For whoever builds warmth, wetness and visibility buffs later, the weather is exposed as
fields and values any system can read on the server:

| Hook | Where | What |
| --- | --- | --- |
| `core:temperature` | `Server::temperature` (Field, 16-block cells) | the climate's mean air temperature, °C |
| `core:humidity`, `core:clouds`, `core:precipitation` | `WeatherState` (Fields, 128-block cells) | the simulated air, cloud and rain or snow, 0..1 |
| `core:wetness` | `WeatherState::wetness` | how wet the ground is, 0..1 (half-life about 300 s in mild, cloudy weather) |
| `core:snow_cover` | `WeatherState::snow_cover` | snow lying, 0..1 (the climate's lasting snow is its baseline) |
| `WeatherState::local(temperature, at, tick, day)` | per place | the full `Weather` there and the effects showing, with strengths |
| `Weather::temperature`, `feels_like()` | engine | air temperature now (day and night), and how cold it feels with wind chill, rain and humid heat |
| `Weather::visibility()` | engine | blocks one can see near the ground |
| `Weather::wind`, `gusts`, `voxmar_engine::weather::gust` | engine | the wind, gusts included, the same on server and client |
| effect tags | `weather.ron` | `cold`, `hot`, `wet`, `dry`, `visibility`, `hazard`, `lightning`, `wonder`, `night`, `aether`: find what is happening by kind, not by name |

A buff system should read these through modifiers (a stew's +2 warmth against
`feels_like()`), never by naming effects, so a mod's new effect with the `cold` tag
counts too.

