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
  They stay there now that the spawned particle system (`render/particles.rs`) exists: a
  steady field wrapping around the camera costs no CPU and has no spawn rate to manage,
  which suits air full of particles better (see docs/rendering.md, "Particles and breaking
  blocks"); particles tied to events or places use the spawned system.
- Rain is tinted half towards the sky's haze, so it matches the sky it falls from, and
  its streaks lie along its motion as seen from the moving camera, so running into rain
  tilts it towards you (Particle Rain's trick). A sparser layer of faint streaks in a box
  three times as wide carries rain (or, as specks, snow) into the distance. Raindrops
  splash as crowns on the ground and ring out as ripples on water; under leaves they come
  through as slow, heavy drips that carry on for a few minutes after the rain, as the
  ground dries. Snowflakes are soft, slightly ragged blobs of a few pixels, varied in size
  and opacity. Sparkle belongs to diamond dust alone: in bitter cold under a halo, ice
  crystals hang in the air, each glinting for an instant as it turns (`glints` in
  precipitation.rs, drawn as an ambient particle set).
- Hail reads as hail: hard, bright white pellets falling fast with a short blur (not
  streaks), and on the ground they bounce twice, lower each time, roll a little and lie
  white for a few seconds before melting (`hailstone` in precipitation.wgsl, in the splash
  slots: as many as the hail's share). On water they only ring. Its sound is an icy rattle
  that leads over the rain (the ambience's `hail` bed ducks the rain beds).
- Mist hangs as big, faint puffs drifting with the wind: ground fog (dawn mist, valley and
  sea fog, lake mist, steam after rain) below its top, and plain mist while nothing falls
  (`mist` in precipitation.rs, an ambient particle set), over the renderer's fog.
- Heat lightning is silent: with no storm overhead its flashes are beyond where thunder is
  heard (`THUNDER_HEARD`, 1300 blocks). Drifting snow is white whatever the ground.
- Every kind of weather has an intensity (`/weather rain 0.3`, `--weather snow:heavy`):
  what falls follows a steep curve (`density` in precipitation.rs, intensity^1.8), so
  light stays a few streaks or flakes, heavy sits between, and max is a downpour, a
  whiteout or a brown-out: at heavy and above, the far layer adds veils of rain, snow or
  dust a few blocks off that grey, white or tan out the distance.
- Rain, snow and particles slant and drift with the gusting wind; clouds drift with the
  mean wind; grass sways with `Wind` (graphics thread).
- Lightning: the flash (existing) and the bolt, drawn for now as bright gizmo lines that
  bloom (`draw_bolts` in weather.rs); replace with real geometry, below.
- Snow lying on the ground: the server turns grass blocks snowy as the snow cover deepens
  and back as it melts (`snow_cover` in weather.rs; data in `weather.ron`).
- Cloud cover, heaped and layered cloud, cirrus, storm gloom. The low cloud is a layer in
  the world (graphics thread, volumetric.wgsl): its base comes down from 135 blocks over sea
  level in fair weather to 85 in rain, its clouds float higher or lower by drifts, it heaps up with convection, darkens in storms and is
  lit by lightning; the sky dome keeps the high sheet and cirrus.
- Fog, mist, whiteout, rain murk and dust haze (graphics thread, render/atmosphere.rs and
  atmosphere.wgsl): the weather's `fog`, `dust` and falling rain and snow are an extinction
  set from `Weather::visibility` (2% shows at that distance), thick around and below the
  eye and thinning above it, so a blizzard closes the view to ~60 blocks and the sky turns
  pale rather than white through mist. Fog is lit by the sky and sun (glowing at dawn) and
  takes the colour of blowing dust. `ClientAtmosphere`'s haze colour and density tint and
  thicken the clear-air haze, and `sky`, `warmth` and `saturation` nudge the sky and the
  grading, as far as the shader pack's `local_air` lets them.
- Ground fog (`ground_fog`, `fog_top`: valley fog, dawn mist, sea fog): densest below the
  top and thinning by e every 9 blocks above it, so it reaches thinly up the slopes. Within
  160 blocks the volumetric pass marches it as an uneven, drifting volume (its top swells by
  up to 6 blocks, its body billows), so from a hill it lies as an uneven sea between ridges
  and from inside it thickens and thins as it passes; beyond, it is integrated in closed form
  along each view ray. It glows where the sun reaches it.
- Wet ground and puddles (`wetness`), frost (`frost`) and a snow dusting on top faces
  (`snow_cover` over 0.3), on surfaces open to the sky (render/lighting.wgsl, `weathered`
  and `wet_sheen`): wet surfaces darken and mirror the sky at glancing angles, puddles
  gather on level ground as it gets wetter and mirror it strongly.

## Every kind of weather: what it has, and what is still missing

Ours (particles in `render/precipitation.*`, sounds in `ambience.ron`, `sounds.ron` and
sfx.rs) are done for every kind; what is left is the graphics thread's, in the sections
after this table.

| Kind (effects) | Particles and sound (done) | Still missing (graphics thread) |
| --- | --- | --- |
| drizzle | fine drizzle, light rain bed | – |
| rain, downpour | streaks, splashes, ripples, drips under leaves, far veils; rain beds by strength and cover | glassy near streaks; rain shafts under distant cells (§8) |
| storm, thunderstorm, supercell | as a downpour, gusts; storm wind; thunder near to far | storm fronts (§8) (bolts and flashes in the cloud drawn) |
| heat lightning | silent far flashes | lighting the horizon's cloud from inside (§8) |
| hail | white pellets that bounce and lie; icy rattle | hail lying white on top faces for a while (as the snow dusting, from `w.hail`) |
| sleet, freezing rain | icy pellets; sleet patter | the glaze of freezing rain: a glassy sheen on surfaces open to the sky while `w.frost` and `w.sleet` are both up |
| snow, blizzard, thundersnow | soft flakes, whiteout veils; blizzard howl, gusts, thunder | – (fog, snow dusting and bolts as above) |
| drifting snow | white dust blowing low; gusts | – |
| dusty afternoon, dust storm, sandstorm | grains and brown-out veils; sandstorm bed | – |
| mist, dawn mist, valley fog, sea fog, lake mist, steam | mist puffs below the fog's top | – (fog drawn) |
| frost | – (silent) | – (frost drawn) |
| heat haze | – | the shimmer (§6) |
| rainbow, double rainbow | – | – (drawn, §2) |
| halo, sun dogs | – | – (drawn, §3) |
| diamond dust | glinting crystals | – (its halo drawn, §3) |
| aurora, low aurora | – | – (drawn, §4) |
| shooting stars, meteor shower, starfall | starfall's motes | – (drawn, §5) |
| ball lightning, marsh lights | marsh lights as glowing motes on dry nights | ball lightning's few large orbs, which glow even in rain (§7) |
| aether motes, aether storm | glowing motes, lightning | a violet tint to the aether storm's cloud and lightning (§7) |

## Waiting on the graphics thread

In order of how much they show. Each is a few lines to a small pass; all read
`ClientWeather::current` (call it `w`) unless said.

### 1. Lightning bolts (drawn: render/bolts.rs)

`ClientWeather::bolts()` yields each visible bolt: `channel` (points from the cloud base,
150–220 blocks up, to the strike point, about 10 blocks apart), `branches` (forks that
end in the air) and `brightness` (flickers with the flash, gone in 0.45 s). Draw camera-
facing ribbons along the polylines, 0.5 blocks wide for the channel and 0.25 for branches,
additive, colour `(0.75, 0.8, 1.0) * 8.0 * brightness` (HDR, so bloom gives the glow), not
fogged by more than half. Then delete `draw_bolts` in weather.rs. Thunder is already
delayed by distance (343 blocks/s) and played by sfx.rs.

### 2. Rainbows (drawn: `rainbow` in sky.wgsl)

`w.rainbow`: 0.6 a rainbow, 1.0 a double one. In sky.wgsl, with `a` the angle between the
view direction and the antisolar point (`-sun`), when the sun is above the horizon:

- primary bow: hue from red at 42° to violet at 40.5° (`hue = (42 - a) / 1.5`), brightness
  `0.35 * rainbow * daylight` with soft edges, a little brighter sky inside it (a < 40°:
  ×1.05);
- if rainbow > 0.75: secondary bow 50°–53.5° with the colours reversed at 0.4 of the
  brightness, and Alexander's dark band between 42° and 50° (sky ×0.92);
- it only exists below the horizon line of the sky dome and fades where cloud cover is
  thick in that direction.

### 3. Halo and sun dogs (drawn: `halo` in sky.wgsl)

`w.halo`: up to 0.5 a 22° halo, above that sun dogs too. Ring at 21.5°–23° around the sun,
reddish on the inner edge, `0.25 * halo * daylight`. Sun dogs: two bright, slightly
coloured spots at ±22° of azimuth from the sun at the sun's height, strength
`2 * (halo - 0.5)`, strongest with a low sun. Also around the moon at night, fainter.

### 4. Aurora (drawn: `aurora` in sky.wgsl)

`w.aurora` (0..1; 0.6 for the rare aurora far from the poles). Night sky only: curtains
towards the pole (north is −z), from about 10° to 60° above the horizon. Vertical rays from
a 1D noise along azimuth, folded ribbons drifting slowly (a few degrees a second), green
`(0.2, 1.0, 0.45)` at the bottom fading to violet `(0.6, 0.3, 1.0)` at the top, brightness
`aurora * night * (1 - cover)` at about the Milky Way's peak, so it needs a dark sky.

### 5. Falling stars and meteor showers (drawn: `falling_stars` in sky.wgsl)

`w.falling_stars`: about `2 × falling_stars` stars a second, so 0.01 (an ordinary clear
night) is one every 50 s or so and 1.0 (a meteor shower) a couple a second, all from one
radiant point per night. Night sky: short-lived
(0.3–0.8 s) streaks 5°–15° long, brightening then fading, from hashes of the second (use
the `pcg` already in sky.wgsl with the world seconds), only through clear sky.

### 6. Heat haze

`w.heat_haze` (0..1): hot, dry, sunny afternoons. A post-process shimmer near the horizon:
offset the screen UV by a scrolling noise of amplitude `1.5 px * heat_haze`, masked to
pixels farther than ~30 blocks and within ~8° of the horizon, rising slowly.

### 7. Wisps and fantastical storms

`w.wisps` (ball lightning in storms, marsh lights on calm wet nights): a few (up to about
12) large glowing orbs, 0.3–0.6 blocks, drifting 0.5–3 blocks above the ground within 30
blocks, pale blue-green, slowly pulsing. For now they add to the motes in the particles
(`motes.max(wisps * 0.3)`), which are smaller, and only while nothing falls: in rain the
motes and wisps are grounded, so ball lightning shows nothing until it has its orbs. `w.motes` and `w.falling_stars` together
with `aether_storm` in `ClientWeather::effects` mark the rare aether storm: a violet tint to
the storm cloud and the lightning would sell it.

### 8. Rain and snow: what the mods and the hiking video still have that we don't

Compared side by side with Particle Rain (MIT; read for its techniques, nothing copied) and
the hiking video's rain frames (`voxmar-reference/hiking-video-frames/0440`, `0449`), what
the precipitation particles cannot do on their own:

- **Haze in and after rain**: drawn now (the weather's fog from `visibility()`, and
  `ClientAtmosphere`'s haze and saturation), see "Already drawn".
- **Wet surfaces**: drawn now (wet ground and puddles).
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
- **Lightning inside clouds** (drawn: volumetric.wgsl lights the low cloud within about
  180 blocks of the brightest flash): most flashes now stay within the cloud
  (`ClientWeather::cloud_flashes()`: position about 160 blocks up and brightness). Light the
  cloud layer around that point (a soft glow of 200–400 blocks) rather than the whole sky.
- **Snow on leaves and other surfaces**: drawn now (a dusting on top faces).
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

### Hazards: what weather does to machines and buildings

For whoever builds machines, roofs and crops: the weather process works out, in every
simulated 128-block cell (the 5×5 around each player), what the weather would do to things
left out in it, as fields on `WeatherState::hazards` (`Hazards::of` in server/weather.rs
for the formulas). They are rates or states now, for a machine to integrate over time
(damage per second from `core:hail`, rust per hour from `core:corrosion`); away from
players they read 0.

| Field | Meaning | Magnitude |
| --- | --- | --- |
| `core:hail` | kinetic energy of hailstones striking a square metre facing the sky, J m⁻² s⁻¹ | 0 without hail; ~1 pea to marble hail; 2 walnut hail (the `hail` effect at full); 4 golf-ball hail |
| `core:wind_load` | the wind's pressure on a surface facing it at the gusts' peak, Pa (½ρv²) | ~10 a breeze; ~60 a strong wind; 200–600 a gale |
| `core:corrosion` | how fast bare iron rusts, relative | 1 steady wet at 15 °C inland; ~2 at 30 °C; ×2 by the sea; 0 dry or frozen |
| `core:frost` | how hard the cold bites, 0..1 | 0 above freezing; 1 at -20 °C, or in freezing rain or hard frost (ice on everything) |
| `core:heat` | heat stress, 0..1 | 0 below 32 °C; 1 at 48 °C; halved under full cloud |
| `core:lightning` | ground strikes in the cell lately, each 1, halving every 10 minutes | a few in a thunderstorm's path |
| `WeatherState::strikes` | the last 64 strikes that reached the ground: the block struck (on top of the ground), the tick and the peak current | 10–200 kA, most 20–40 |

Cloud flashes never reach the ground, so they are not strikes. A lightning rod or a
machine's surge damage reads the events; a building's wear reads the fields.

A buff system should read these through modifiers (a stew's +2 warmth against
`feels_like()`), never by naming effects, so a mod's new effect with the `cold` tag
counts too.

