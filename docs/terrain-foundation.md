# Terrain foundation: one parameter space, one density field

Oct 4, 2026. Status: **accepted** (Mariano confirmed the five decisions in section 9 on
5 October); being built in the steps of section 8. Read with
[world-plan.md](world-plan.md) (decisions 4, 7 and 8) and
[design-philosophy.md](design-philosophy.md) ("Causal, layered generation").

Mariano's feedback after playing: terrain and world generation look hacky next to the mods
he has seen; ravines are poor; some coasts and landforms are perfectly circular; biomes are
far too small and too close to each other. He asked for the fundamental cause, not patches,
and for a source-level look at how vanilla Minecraft, Terralith and Tectonic actually work.

## Contents

1. [The short answer](#1-the-short-answer)
2. [What we measured](#2-what-we-measured)
3. [How the reference generators are built](#3-how-the-reference-generators-are-built)
4. [How ours is built](#4-how-ours-is-built)
5. [The fundamental differences](#5-the-fundamental-differences)
6. [Target architecture](#6-target-architecture)
7. [Performance budget](#7-performance-budget)
8. [Migration plan](#8-migration-plan)
9. [Decisions for Mariano](#9-decisions-for-mariano)
10. [Sources and licences](#10-sources-and-licences)

## 1. The short answer

Vanilla 1.18+, Terralith and Tectonic all stand on one idea: **a handful of large noise
fields define a point in parameter space at every position** (continentalness, erosion,
peaks and valleys, temperature, humidity). Terrain height and shape are cubic splines over
that point, evaluated as one 3D density field. Biomes are a table over the *same* point, at
the *same* scales. Every feature switches on in a region of the same space. Because
everything is a view of the same few numbers, a mountain biome sits on the mountain, a
valley biome in the valley, a coast curls with a fractal shoreline, and nothing has a
shape of its own that could look stamped.

voxmar has no shared parameter space. Each subsystem has its own private fields and its own
geometry:

- the land is about forty hand-tuned constants composing heights from ten noises
  (`geology.rs`);
- twelve terrain shapes stamp their own noises, jittered grids and radial profiles on top
  (`shapes.rs`), and two of them (arches, spires) are a separate 3D mechanism (`sculpt`);
- the climate that picks biomes is a physical model (latitude, altitude cooling, wind
  carrying moisture over relief) that follows every contour and coastline (`climate.rs`);
- the picker sees the land only through a derived "landform class";
- ravines are random walks that know nothing about the land (`caves.rs`).

Each piece shows its own signature: circles from the radial shapes, smooth blobs from
under-resolved coasts, contour stripes from altitude-cooled biomes, knife slots from
ravines. That is the "hacky" look. The fix is architectural: one parameter space, splines
as data, a 3D density field, and biomes picked from the same parameters at matching scales.
Our world already has vanilla's vertical layout (128 blocks under the sea, 256 above), so
the reference splines carry over one to one.

## 2. What we measured

All on seed 7 with `map_preview` (8 × 8 km, 8 blocks per pixel) and the code on `main`
today. Images are in `~/Projects/voxmar-wt/shots/terrain2/`.

**Biomes are small because the climate follows the terrain.** The median block of land lies
in a biome region 655 m across, only 32% of land is in regions at least 1 km across, and
48 different biomes lie within 5 km of spawn (the plan asked for at least 15; decision 4 asks
for biomes 1–2 km across). To find the cause I switched parts of the climate off, one at a
time, and measured again:

| Experiment | Median region | Land in regions ≥ 1 km | Biomes within 5 km |
| --- | --- | --- | --- |
| Today | 655 m | 32% | 48 |
| Climate noises 4× larger, climate zones 2× larger | 807 m | 40% | 42 |
| … and no altitude cooling in the picker's temperature | 844 m | 41% | 40 |
| … and no ecotone slices | 872 m | 44% | 38 |
| … and no moisture carried by the wind over the relief | **1083 m** | **55%** | 35 |

Making the noises bigger barely helps. What fragments biomes is that their temperature and
moisture follow local terrain: temperature cools 0.11 °C per block of regional elevation,
so a 6 °C band is only about 55 blocks of height and borders trace contour lines; moisture
is picked up over the sea and rained out over every rise, so it changes along every coast
and every range (`moisture_seed7_now.png`). With all of it decoupled, the remaining borders
come from the continentalness bands along every coast (22%), the landform classes (17%) and
splits inside a table cell (35%), which the new design also addresses (section 6.6).
`biomes_seed7_now.png` and `biomes_seed7_experiment_decoupled_climate.png` show the
difference.

**Coasts are smooth because the continental noise has no fine octaves.** It has five
octaves starting at 1/(3 × continent size): the finest detail it can draw is 260–600 blocks
across. Under that, a shoreline is a smooth contour line: rounded capes, oval islets, no
bays, headlands, stacks or spits (`relief_seed7_coast_2km_now.png`). Vanilla's
continentalness has nine octaves, down to an 8-block wavelength; Tectonic's has nine, down to
about 30 blocks.

**The circles are radial stamps.** Volcanoes and lone mountains are cones around a centre
on a jittered grid, their outline wandering by only 22–30% of the radius, and the
volcanoes stand at sea as perfectly round islands or bulge coasts into round peninsulas
(orange stars in `relief_shapes_seed7.png` in `shots/terrain/`). Karst towers, spires, sea
stacks, sinkholes and geodes are radial too, at smaller sizes. Plateaus are the level set
of a two-octave noise, so their escarpments are clean blobby contour lines.

**Ravines are random walks.** A ravine starts at a random point of a 1 km cell and wanders
for 140–1250 blocks with a random heading. Its floor follows the smoothed surface (so it
rides up and over hills), its cross-section is a fixed V with horizontal ledges hashed by
height, and 45% are open to the sky. It ignores where water would run, what the rock is and
how the land lies, which is why it reads as a cut made with a knife
(`shots/caves/ravine_7.png`).

## 3. How the reference generators are built

I read vanilla's world-generation data for 26.4 (the current snapshot; the structure is the
same since 1.18), Tectonic 3.0.25's whole density graph and the Java that configures it, the
Minecraft Wiki's noise router and density function pages, and the earlier notes in
`research/terralith-tectonic.md`. Section 10 lists the files and licences.

### 3.1 Vanilla 1.18+: the noise router

**Parameter noises.** Five 2D noises, each a sum of octaves of Perlin noise, sampled at a
quarter of block coordinates with a small domain warp, and cached per 4 × 4 column:

| Noise | Lowest frequency | Octaves | Finest wavelength | Role |
| --- | --- | --- | --- | --- |
| continentalness | 1/2048 per block | 9 | 8 blocks | sea, coast, inland |
| erosion | 1/2048 | 5 (one silent) | 128 | flat versus mountainous |
| ridges (weirdness) | 1/512 | 3 | 128 | peaks and valleys; biome variants by its sign |
| temperature | 1/4096 | 2 (1/4096, 1/1024) | 1024 | biome rows only |
| vegetation (humidity) | 1/1024 | 2 (1/1024, 1/512) | 512 | biome rows only |

"Large biomes" uses the same noises with every frequency divided by four, terrain included.
`ridges_folded = −3(| |ridges| − 2/3 | − 1/3)` is the peaks-and-valleys value: 1 on peaks, −1
in valleys, with rivers where `ridges` crosses zero.

**Terrain splines.** Three nested cubic Hermite splines turn the parameter point into
terrain, each point a location, a value (a number or another spline) and a derivative:

- `offset` (continentalness → erosion → peaks and valleys): the surface height above the
  sea, in units of 128 blocks;
- `factor`: how fast density rises below the surface, that is how 2D the land is: 6.3 on
  plains, 0.6 on shattered peaks;
- `jaggedness`: amplitude of a high-frequency noise added on peaks only.

The knots sit at the same breakpoints the biome table uses (for example continentalness −0.19
coast, −0.11 near inland, erosion −0.78, −0.375, −0.2225, 0.05, 0.45, 0.55), so each biome's
region of parameter space has a matching landform.

**3D density.** With `depth = offset + 1.5 − (y + 64)/128` (zero at the spline surface):

```
sloped_cheese = 4 · quarter_negative((depth + jaggedness · half_negative(jagged)) · factor)
              + base_3d_noise
final = squeeze(0.64 · slides(caves_combined(sloped_cheese))) min noodle caves
```

`quarter_negative` divides negative values by 4, so above the surface density falls slowly
and the 3D noise can lift lips and overhangs, while below it density rises fast and the
ground stays solid. `base_3d_noise` is a blended noise four times smoother vertically
than horizontally, which gives steep walls, buttresses and lips rather than blobs. Slides force air in the top 16 blocks and rock in the
bottom 24. `squeeze` is `x/2 − x³/24` on the clamped value. Density is computed at the
corners of 4 × 8 × 4 cells and interpolated trilinearly in between; only these corner
samples pay for noise.

**Caves are part of the same density.** Where `sloped_cheese` is below 1.5625 (near the
surface) only cave entrances may cut it; deeper, cheese caverns (a 3D noise above a
threshold, with layering and pillars), spaghetti tunnels (two noises near zero) and noodle
tunnels combine with `min` and `max`. Carvers (old caves and the canyon carver, which runs at
y 10–67 in 1% of chunks) are a separate older system on top; Tectonic offers to switch them
off as "old caves/ravines".

**Aquifers.** Space is cut into 16 × 12 × 16 cells, each with a jittered centre and a fluid
level: the sea's near the preliminary surface, otherwise empty, flooded or a random level
from two noises, lava deep down. A barrier noise and the pressure between neighbouring
cells' levels leave rock walls so fluids never touch air. **Ore veins** are three more
router noises (toggle, ridged, gap) that lay copper and iron ribbons through the depths.

**Biomes.** At every 4 × 4 × 4 cell the climate sampler reads temperature, humidity,
continentalness, erosion, depth and weirdness, the same values terrain uses, and picks the
biome whose parameter box is nearest (squared distance, zero inside). The biome table is
built as rows of temperature × humidity bands and columns of landform roles (coast, beach,
lowland, plateau, slope, peak, valley, river), so biomes follow landforms by construction.
Temperature for snow and freezing is cooled with height separately, without changing the
biome.

### 3.2 Tectonic

Tectonic keeps the vanilla router and rewrites its inputs and splines:

- **Larger, folded fields.** Continentalness is sampled at 0.13 instead of 0.25 (lowest
  wavelength about 7.9 km, nine octaves), erosion and ridges at 0.25. Land is where
  `|continentalness|` is large, so the sea forms a connected web of channels. Mountain
  ranges run along `|erosion| → 0` (long chains about 4 km apart), and rivers along
  `|ridges| → 0`, cut 5–13 blocks below sea level.
- **Its own splines** for offset, factor and jaggedness, with an `elevation_boost · offset²`
  stretch above sea level, deeper oceans (−0.22 and −0.45, about 28 and 58 blocks), a
  trench-and-ridge sea floor (`1 − 6|n|`), and a soft ceiling.
- **Terrain regions** (club, heart, spade, diamond) chosen by a region-selector noise and
  quantised temperature and humidity: plateaus, single and double valleys, badlands
  ridges, rolling hills, jungle pillars, two-tier terraces stepping down to the river
  lines, wind-aligned dunes, wetlands. They read climate fields, never the chosen biome.
- **Mountain detail.** A ridge noise plus detail noise shifted by 3000 × its own gradient,
  so gullies follow the slope; thin 3D "weathering" cracks; jaggedness only on spines.
- **Biome parameters synthesised from the terrain.** Erosion and ridges handed to the biome
  picker are step functions of the terrain classes, so biome borders snap to landforms.
- **Underground rivers** where the river line meets a mountain, lava tunnels at the bottom,
  and an option to turn carvers off.
- **3D noise only at 40%** on most land (`roughness` spline), so its lowlands are close to a
  heightfield; full 3D appears on the most eroded ground.
- Temperature and humidity stay at vanilla's scale, so **biomes under Tectonic are vanilla
  sized**; only the terrain is larger.

### 3.3 Terralith

Terralith is the same router with re-authored splines (offsets −0.825 to 1.5, factors 0.45
to 20, jaggedness up to 1.1) whose knots are placed at its biome table's breakpoints; 86 of
its own biomes plus vanilla's in 1,705 parameter boxes that tile a quantised 5D grid with no
holes, rare biomes parked in the tails of the noise distributions, and picker-only remaps
that make island slots reachable; extra 3D terrain (arches, dunes, leaning spires, coastal
cliff cuts) composed with `max` and `min` behind cheap 2D gates; and a very large surface
rule that draws trails, creeks, ripples and seams from noise contour bands. All of this is
from the earlier notes, which read its data in detail.

**Licence note.** Terralith's licence allows reading its code to learn from it, but it also
says its code and data "may not be used to configure, test, debug, or augment any
artificial intelligence, neural network, or other generative system". Reading its files
with an AI to design a world generator falls inside that wording, so for this study I did
not open them (I deleted the clone) and relied on the earlier notes and its public pages.
Nothing in this design depends on Terralith specifically: the architecture comes from
vanilla and Tectonic, both of which I read.

### 3.4 How deep the earlier research went

`research/terralith-tectonic.md` read both mods' data carefully and extracted the right
*techniques*: folded fields, climate-gated shapes with shared band edges, gradient-shifted
mountain detail, wind-aligned dunes, terraces, the band table with rare biomes in the tails,
surface contour bands. The terrain track then implemented most of them (`geology.rs` cites
Tectonic throughout). But the notes:

- skipped vanilla's router, the foundation both mods are layered on, and so never stated
  that terrain is a *spline over the same point the biome table reads*;
- treated every technique as an addition to our heightfield ("in `geology.rs`, define three
  folded fields…"), so we got the tricks without the system that makes them coherent;
- skipped caves and carvers entirely ("another team covers caves");
- did not look at noise spectra (octave counts), which is where the smooth coasts come from.

So: thorough on features, shallow on the foundation.

## 4. How ours is built

As it stood on 4 October, before the steps of section 8; each step's *as built* note there
says what has changed since.

| Stage | File | What it does |
| --- | --- | --- |
| Geology | `geology.rs` | A 2D height per position: folded continental noise through an 11-knot curve; ranges along a folded noise with a strength noise, ridged peaks and crags; two-tier plateaus with escarpments; folded valley network; hills by a hilliness noise; height stretch and ceiling ease |
| Climate | `climate.rs` | On a 64-block lattice: latitude bands, temperature anomaly, moisture walked upwind over the relief with rain shadows and orographic rain, altitude cooling; seeded fields for weirdness, aether, fertility, variant |
| Shapes | `shapes.rs`, `terrain.ron` | 12 shapes (11 kinds) adding or cutting height, gated by climate bands; volcanoes and lone mountains as radial cones on jittered grids; towers, spires and stacks on Voronoi cells; arches and leaning spires as 3D pieces |
| Water | `water/` | Two-tier drainage on 64- and 8-block grids over the 2D height, lakes by flood fill, channels carved into the height |
| Trails | `trails/` | Planned over the land with its rivers; cut and fill the height |
| Biomes | `biome.rs`, `climate.ron` | A band table over temperature, moisture, continentalness and landform class, split inside cells by weirdness, aether, variants and ecotones |
| Fill | `terrain.rs` | Solid from bedrock to the column's height; surface layers from the biome's rules; caves carved; arches and spires added (`sculpt`) |
| Caves | `caves.rs`, `underground.rs` | Noise tunnels, caverns, noodles, labyrinths; random-walk ravines; sinkholes; geodes; aquifer-style water tables; cave biomes |

The caves are already noise caves with aquifers, close to vanilla's, and the rivers are a
real drainage model that vanilla does not have. Both are worth keeping.

## 5. The fundamental differences

The brief listed five likely causes. Verdicts:

1. **A 2D heightfield with 3D shapes bolted on: confirmed, but secondary.** The fill is
   "solid up to `height`". No overhangs, no undercut cliffs, no ledges; every steep face is
   a staircase of whole columns; arches and spires are a second mechanism with its own
   anchors and cell searches. It matters most in mountains and on coasts. It is not the main
   reason for the hacky look: Tectonic's lowlands are nearly 2D too.
2. **Shapes with radial or circular footprints: confirmed.** Volcanoes, lone mountains,
   towers, spires, stacks and sinkholes are distances from a centre. Together with coasts
   that have no detail under a few hundred blocks (section 2), they are the circles.
3. **Biome fields at a different scale from the terrain noises: partly refuted.** The
   biome noises are not small (weirdness 7 km, temperature anomaly 1.6 km). The real problem
   is that the biome's temperature and moisture are coupled to local terrain (altitude
   cooling, moisture over every rise and coast), and that the picker reads the land only
   through a derived landform class instead of the terrain's own parameters. Biomes end up
   small *and* unrelated to landforms. The experiment in section 2 shows decoupling alone
   moves the median region from 655 m to 1083 m.
4. **Carvers as splines instead of noise caves: refuted for caves, confirmed for ravines.**
   Tunnels and caverns are noise caves already. Ravines are random-walk carvers, like
   vanilla's canyon carver, which is also the weakest part of vanilla.
5. **No splines mapping the parameters to terrain: confirmed.** There is a piecewise-linear
   1D `spline()` for a few curves, but height is composed by code: smoothsteps, thresholds,
   `if mountain > 0.005` gates and products of strengths. There is no parameter point, no
   nested spline and no data file for the land's shape.

And one cause the brief didn't list: **the spectrum of the fields is wrong for their jobs.**
The coast-defining noise lacks fine octaves (smooth coasts), while the fields biomes are
picked by vary with local relief at the scale of hills (small biomes). Vanilla gives the
coast field nine octaves and the biome-only fields two smooth ones.

**The root cause** is that we built several worlds side by side, each with private
fields and its own geometry, and composed their outputs. The reference generators build one
parameter space and read everything from it.

## 6. Target architecture

```
seed ─▶ parameter fields (2D, per 4×4 cell): C, E, W, Q, T, H, A, R
          │
          ├─▶ terrain splines (data): offset, factor, jaggedness, roughness
          │      └─▶ preliminary surface (2D, analytic) ─▶ rivers, lakes, trails, climate shadow
          │      └─▶ 3D density on a 4×8×4 lattice, only in a band around the surface
          │             + gated shapers (add with max, cut with min)
          │             + river, lake and trail caps and floors
          │             min caves (noise caves, aquifers)
          │             ─▶ blocks, then surface rules at every exposed top
          │
          └─▶ biome table over the same point (T, H rows × landform-role columns,
                 rare biomes in the tails of R, fantasy by A)
```

Everything stays a pure function of seed and position: any column alone, in any order, on
any thread.

### 6.1 Parameter fields

Computed at the corners of 4 × 4 cells (the density lattice's corners), seeded by name
(`hash(seed, "erosion")`), each with its own domain warp scaled with it (Tectonic's
`smootherScaling` lesson):

| Field | Lowest wavelength | Octaves (finest) | Terrain reads it for | Biomes read it for |
| --- | --- | --- | --- | --- |
| C continentalness | 8 km × continent dial | 9 (about 30 blocks) | sea depth, coast, inland rise; folded so seas form a web | ocean, shore and inland columns |
| E erosion | 4 km | 8 (about 30 blocks) | `|E|` → range spines; high `|E|` → lowland styles | flat, hilly, mountain roles |
| W ridges | 1.2 km | 3 (about 300 blocks) | `|W|` → valley network; peaks and valleys | valley, slope and peak roles |
| Q region | 6 km | 3 | lowland style: flat, rolling, terraced, plateau (Tectonic's suits) | sub-biome flavour |
| T temperature | latitude, about 10 km pole to equator, plus an 8 km noise of ±5 °C | 2 | shape gates only | rows |
| H humidity | 6 km noise plus a regional rain shadow | 2 | shape gates only | rows |
| A aether | 4 km | 2 | fantastic shapes | fantasy claims |
| R rarity | 7 km | 2 | regions of rare shapes | rare biomes in the tails |

T and H stay physical (°C, precipitation over evaporation) for gameplay. The rain shadow is
kept as an idea but computed over a smoothed copy of the land (512-block steps upwind over
the spline offset), so the dry lee of a range is a region kilometres wide, not a stripe
along each slope. **Altitude cooling moves out of biome selection**: `Site::temperature`
still cools with the real ground height for snow, ice, tree lines, crops and comfort
(vanilla does exactly this), but the biome is chosen by the regional temperature. Alpine
zones come from the landform roles (slopes, peaks) in the cold rows, and snow caps from the
surface rules.

### 6.2 Splines as data

A new content file, `land.ron`, holds the router: the parameter noises (wavelength, the
weight of each octave, spread, warp) and the derived fields as small expressions, mostly
nested cubic Hermite splines in the vanilla form (location, value or spline, derivative;
linear beyond the ends). A later package's `land.ron` replaces an earlier one's. Knot
locations can name the breakpoints in `climate.ron`, so moving a band edge moves the biome
and its landform together. A sketch (the built file keeps Tectonic's units, 128 blocks per
unit, so its values carry over):

```ron
offset: (on: C, points: [
    (-1.0, -96.0), (-0.45, -58.0), (-0.2, -18.0), (coast, 1.0, 12.0),
    (near_inland, (on: E_fold, points: [
        (0.0, 120.0), (0.15, 70.0, -190.0), (0.3, 30.0),
        (0.375, (on: W_fold, points: [(0.0, -12.0), (0.15, 18.0, 380.0), (0.3, 30.0)])),
        (0.45, (on: Q, points: [ /* plateau, rolling, terraced lowlands */ ])),
    ])),
    // …
]),
```

Our world has vanilla's vertical layout (128 blocks of ground under the sea, 256 above), so
vanilla's and Tectonic's offsets map one to one: surface = 128 + 128 × offset. Tectonic is
MIT, so we can start from its splines directly, credited in `THIRD_PARTY_NOTICES.md`, and
tune from there. Vanilla's own spline values are Mojang's and are used only as a reference.

`factor`, `jaggedness` and a `roughness` spline (how much 3D noise) are authored the same way.
World dials scale them: ocean fraction shifts C, continent size scales C, tectonic activity
raises the spine knots, erosion age lowers jaggedness and roughness, mountain height sets
the elevation boost (`h · (1 + b·h²)` above the sea, eased under the ceiling as today).

### 6.3 3D density

In blocks:

```
surface(x, z) = sea + offset(p) + jaggedness(p) · J(x, z) + Σ 2D shaper heights
d(x, y, z)    = slope(p) · quarter_negative(surface − y) + roughness(p) · N(x, y, z)
              + Σ gated 3D shapers (max to add, min to cut)
solid  ⇔  d > 0, after slides at the top and bottom of the world
```

`slope(p)` comes from the factor spline: about 0.2 per block on plains (3D noise moves the
ground by a block or two) down to 0.02 on shattered peaks (by tens of blocks, with
overhangs, since above the surface density falls four times more slowly). `N` is our SIMD
OpenSimplex2 at 3 octaves, smoother vertically than horizontally as vanilla's is, so steep
ground breaks into walls and buttresses.

**Lattice and band.** `d` is sampled at the corners of 4 × 8 × 4 cells and interpolated
trilinearly, as in vanilla and as our caves already are (`lattice.rs`). Unlike vanilla, we
evaluate noise only where it can matter: from the surface and the bound on `N`, each
lattice column knows the band in which `d` can change sign; above it is air, below it rock,
and only the band's corners sample `N`. That band is a few cells on plains and a dozen in
the mountains.

**The preliminary surface** is `surface(x, z)` without `N` and without 3D shapers: an
analytic 2D height, cheaper than today's `Geology::land`, sampled four points at a time.
Rivers, lakes, trails, the rain shadow and the aquifers' near-surface test read it, through
a batched call like today's `lands`. The final top block differs from it only where the 3D
noise or a 3D shaper moves the ground.

### 6.4 Shapers: gated density terms

Every terrain shape becomes a term of the router with a gate: a product of smooth ramps
over parameter-space ranges (temperature and humidity bands from `climate.ron`, C, E, W, Q,
A, R, rock province). The gate is evaluated on the 2D lattice; a chunk whose 25 corners all
have gate 0 skips the shape entirely. Nothing is placed around a centre unless the landform
really is one.

| Today | Becomes |
| --- | --- |
| rolling_hills | `|n| · k` in the offset where E is high and Q picks rolling (Tectonic's heart) |
| dunes | the existing wind-aligned asymmetric dunes as a 2D offset term, gated hot and arid |
| mesas, canyons | terrace tiers stepping down to the `|W|` valleys, tier heights from `|n|` (Tectonic's spade), clay bands on the risers; canyons are the steps down to the rivers |
| karst_towers | pillars from a thresholded high-frequency noise (Tectonic's jungle pillars) with low factor for undercuts, gated warm and wet on karst; kept out of climate height |
| frost_spires, shattered_spires | 3D leaning spires: `|n(x + a(y), z + b(y))|²` against a falling gradient, so they lean and taper (Terralith's technique, our own code) |
| volcanoes | only inside a volcanic region of parameter space (a rare tail of R with low erosion), so a volcanic field is a place and a biome; cones with several vents, a breached crater and lava lobes, their distance warped by up to a third of the radius |
| lone_mountain | removed: massifs come from a rare tail of the erosion spline (very low `|E|` far inland) |
| arches | 3D ribbons along a line noise, gated hot uplands, no per-cell search |
| fjords | the offset spline: cold rows × coast × valleys give a drowned U-shaped trench |
| sea_cliffs | a cut where the offset is re-evaluated at a slightly seaward C (Terralith's technique), gated on hilly coasts, with a wave-cut platform at sea level |
| plateaus, broad valleys (geology) | Q regions with escarpments from spline steps (3D noise breaks the risers), and the `|W|` valleys |

### 6.5 Water, gorges and caves

- **Rivers and lakes** keep the two-tier drainage. It reads the preliminary surface
  instead of `Geology::land`. Because the offset dips along `|W| → 0`, trunks find a valley
  network that is part of the terrain, as in vanilla and Tectonic. A river or lake no
  longer edits a height; it adds a **cap** (no rock above its carved valley surface) and,
  under the water, a **floor** (rock up to its bed), so a river cuts cleanly through
  overhangs and its water is always sealed.
- **Gorges come from water.** Where a breach cuts deep through high ground, the channel's
  walls steepen into a slot gorge with a monotone floor and a stream in it. Canyon country
  is the terrace system stepping down to the rivers. The random-walk ravines go: a buried
  canyon may remain as a rare cave feature at depth, never open to the sky.
- **Caves** (`caves.rs`) stay as they are and combine with `min`. Their "depth below the
  surface" reads the column's density top instead of `geo.height`, entrances cut only where
  the density is near the surface (vanilla's 1.5625 test), and the aquifers' near-surface
  cells read the preliminary surface.
- **The sea** fills air below sea level wherever it is open to the sea, as today.

### 6.6 Biomes from the same parameters

- **Axes:** regional T and H (rows), C and a landform role from E, W and Q (columns),
  R for rare biomes in its tails, A for fantasy claims. The table machinery in `biome.rs`
  (bands, claims, tails, remaps, edge distance, blending) stays; what feeds it changes.
- **Roles** come from the same spline breakpoints as the terrain: deep ocean, ocean, shore
  (beach or cliff by E), lowland (flat or rolling), plateau or terraces (Q), slopes, peaks,
  valley floor in the mountains (low `|W|` where E is low), river and lake (from water). A
  plateau biome covers the plateau because both are the same region of parameter space.
- **Coasts:** the shore role is a thin strip by distance to the sea, not a continentalness
  band 0..0.15 that paints a broad stripe inside every coast.
- **Variants** come from Q or R at three kilometres and more, not from a 450-block field.
  Ecotones stay as slices along T and H borders; with smooth T and H they are hundreds of
  blocks wide, which is what a transition zone should be.
- **Targets**, checked by `map_preview` on several seeds: median region at least 1.5 km
  across, at least 70% of land in regions 1 km or more across, 15–25 biomes within 5 km of
  spawn, no desert beside snow.

### 6.7 Surface rules in 3D

The fill knows each block's depth below the nearest air above it (vanilla's
`stone_depth`). A biome's surface rule is evaluated once per column at its top, as now, and
its layers apply at the top and at every exposed ledge within a few blocks below it;
undersides get a ceiling rule (no sand or gravel hangs); exposed 3D faces show the biome's
cliff block; cave floors far below keep cave biomes' rules. Slope keeps coming from the
neighbouring columns' tops, so the cliff, snow and dither rules keep working.

### 6.8 What stays the same for everyone else

- `Overworld::sampler`, `Sampler::site`, `Sampler::surface` and `surface_sample` keep their
  signatures. The parameter grid replaces `ClimateGrid` inside them.
- `Overworld::height(x, z)` returns the exact top block the fill makes, from the same
  lattice and the same arithmetic (four lattice columns and the band), with river, lake and
  trail caps applied. Distant terrain keeps calling it.
- `Site` keeps its fields and meanings. `erosion` becomes the landform role, `weirdness` the
  rarity field, `mountain` comes from `|E|`. New fields (raw C, peaks and valleys) are
  additions.
- Rock provinces, fertility, stands, clearings, decoration, trees, trails and cave biomes
  read the same things as before.
- Generation stays deterministic; `Overworld::VERSION` is bumped and the golden hashes in
  `perf.rs` updated at every step.

## 7. Performance budget

**Where the time goes today** (after the second speed pass, `main` at 64642b9; release, one
core):

- *Warm, per column:* caves 1.1 ms, fill 0.8, cave biomes 0.3, rivers about 0.4 (including
  the land under them), climate 0.14; about 3 ms of generation, plus 1.2 ms of light.
- *The first chunk:* river tiles 36–43% of its CPU, the climate lattice 15–20%, trail grids
  15–19%, basins 12–15%, lake floods 5–9%. Most of that is **terrain sampling**: the land
  height asked for on the rivers' 64- and 8-block grids, the trails' grids and the climate's
  upwind walk, through the batched `Geology::lands` and `reliefs`.

So the design splits terrain into two products with very different costs:

1. **The 2D preliminary surface**: the parameter fields C, E, W and Q plus the offset
   spline (and the 2D shapers: dunes, terraces, fjords). No 3D noise. This is all that
   rivers, lakes, trails, the rain shadow and the aquifers' near-surface test ever ask for.
   It keeps a **batched API** in the shape of today's `lands` and `reliefs` (four points per
   call, one per lane of `noise::get_2d_x4` and `noise::warp_2d_x4`), so river and trail
   region builds keep their 4-wide sampling; going back to one point at a time would make
   them about twice as slow.
2. **The 3D density**: only inside generated columns and for `height()`, on the lattice,
   only in the band around the surface, 4-wide through `noise::noise_3d`.

| Work | Budget | How |
| --- | --- | --- |
| Preliminary surface, per point | ≤ today's `lands` per point | about 25 octaves of 2D noise 4-wide (C 9, E 8, W 3, Q 3, warp 2) plus one nested spline; replaces ten noises, the shapes' climate weighing and the regional lookup today's `land` makes |
| Climate lattice point | ≤ half of today's | the upwind walk samples a coarse 512-block copy of the offset instead of the full relief at nine distances |
| Parameter fields per column | ≤ 15 µs | 16 new 4-block lattice points (corners shared), 4-wide; T, H, A on the 64-block lattice |
| Splines per column | ≤ 10 µs | three nested Hermite splines per lattice point |
| 3D density per column | ≤ 30 µs | band-limited: 4–12 cells of 8 blocks × 16 corners, 3-octave 3D noise 4-wide; gated shapers only inside their gates |
| Fill and surface per column | ≤ 0.8 ms | as today: interpolation is one add per block in y |
| `height(x, z)` | ≤ 2 µs | four lattice columns, band only |

**Targets.** Warm generation stays at about 3 ms per column or better (caves, cave biomes
and light are unchanged by this work, so the terrain itself must not add to today's fill
and rivers time). The first chunk gets faster, because river tiles, trail grids, basins and
the climate lattice all sample the cheaper preliminary surface. No step lands with a
regression in warm or cold generation, measured on a quiet machine (QUIET flag, nothing
else running, no playtest) and reported before and after.

**Measured at step 1** (release, Low Power Mode, instructions from `/usr/bin/time -l`, which
load does not change; main at ee09ba4 against the branch, back to back):

| | main | step 1 |
| --- | --- | --- |
| A land sample on the rivers' 8-block grid (`land_samples`) | 4675 instructions, 940 cycles | 5457, 1040 (+11% cycles) |
| `first_column` (three new worlds) | 141.9 G instructions | 138.4 G (−2.5%) |
| `generation_stages` (960 columns, cold then warm) | 350.9 G | 368.2 G (+4.9%) |
| Warm generation per column (CPU, loaded machine) | 4.6–4.9 ms | 4.2–5.5 ms |

A land sample costs more because the router interprets its splines four lanes at a time
(about 45% of a sample, against 24% for the noise and 21% for the terrain shapes); the
first column is cheaper anyway, because the climate's lattice and the rivers' sealing now
share the lattice points they read. Most of the remaining cold difference is trail
planning, whose grids sample the land the most. The next
savings belong to step 3 (fields and splines worked out once per lattice point for both
the 2D surface and the density) and step 4 (the shapes' 21% moves into the router).

**Measured at step 2:** `first_column` 141.0 G instructions (+1.9% on step 1),
`generation_stages` 369.7 G (+0.4%). The biome picker reads the land's continentalness at
each 4-block cell, batched over a sampler's area, so the biome stage stays under 0.1 ms per
column.

## 8. Migration plan

Each step builds, passes the tests, looks right in captures and previews, bumps
`Overworld::VERSION`, updates the golden hashes, and lands on `main`.

1. **Router and splines.** `router.rs`: parameter fields and nested Hermite splines loaded
   from `land.ron`, evaluated four positions at a time on a 4-block lattice and
   interpolated. The climate's decoupling from local relief moved into this step, since the
   land's regions read the climate: a coarse base lattice (sea-level temperature,
   precipitation from continents and ranges) that the land reads, with climate zones about
   10 km from pole to equator.
   `Geology::relief`, `reliefs`, `land` and `lands` are rewritten as the preliminary surface
   from the splines, starting from Tectonic's (credited), keeping the 4-wide batched calls.
   Still 2D, so rivers, climate, shapes and the fill are untouched. *Done when:* fractal
   coasts at the 2 km preview, ranges as chains with valleys, plateaus with natural
   outlines, land heights comparable to today, the first chunk and river regions no slower
   (faster expected); captures of coasts, ranges and plains.

   *As built:* `land.ron` holds Tectonic 3.0.25's fields and splines (credited) with these
   changes: continents 10.4 km (seas about 1.5× the old size), the coast at the quantile of
   the world's own continents noise for its ocean fraction (so the dial means what it says
   with a noise that is not quite normal), ranges about 8 km apart with an erosion spread of
   1.3 (Tectonic's is about 0.64; Mariano asked for slightly fewer, slightly lower
   mountains), no channels below the sea (our rivers cut the valleys), lowland lifted a few
   blocks with rolling hills up to about 25 blocks high where Tectonic's 3D noise would
   roughen it, and a fine texture until step 3. Over 20 km squares of six worlds
   (`land_heights`): land 54% of the world (the dials ask for 57%), its median 16 blocks
   above the sea (17 before), 5.0% of it above 100 blocks (5.4%), 0.8% above 160 (1.1%),
   the highest 227. Biome regions grew from a median of about 0.96 km across to 1.24 km on
   seeds 1, 3, 7 and 42, before step 2 touches the biomes.
2. **Biomes on the same parameters.** Regional T and H without altitude coupling, the rain
   shadow at regional scale, roles from the router, `climate.ron` and biome claims
   re-authored on shared breakpoints, variants by Q or R. *Done when:* the section 6.6
   targets hold on seeds 1, 7 and 42, captures across borders.

   *As built:* measuring first (`biome_spread`, with `AXES=1` for each climate axis alone)
   showed what kept regions small: each axis alone makes regions 1.4–4 km across, but the
   table's cells, where four bands meet, only 0.4–0.85 km, and moisture changed with
   latitude as fast as temperature (evaporation by the local warmth, the circulation's
   belts), so its borders ran beside temperature's and cut each zone into thin bands.
   Biomes now go by a humidity (`humidity` in `climate.ron`) that weighs a slow 16 km
   wetness field most and the wind's rain and the belts little, and leaves evaporation out,
   as vanilla's humidity is independent of temperature; plants and the land's regions (the
   router's `H`) read the same. Variants follow an 8 km field and aether a 12 km one, and
   the scales of all of the climate's own fields are data (`scales`). Continentalness is the
   land's own at each cell, kept to the dry side where the cell is land, so coasts no longer
   carry strips of sea biomes. Altitude coupling, the ecotones' widths and the circulation
   alone each changed little and stay as they were. Over twelve worlds (8 km maps, mean):
   the median block of land lies in a region 1.63 km across (1.26 before), 71% of land in
   regions at least 1 km across (59%), 31 biomes within 5 km of the middle (35), 25
   counting variants and ecotones with their parents. Seed 7 holds every size target
   (2.37 km, 75%); seeds 1 and 42 fall short (1.38 and 1.12 km, 66% and 63%), as much of
   their land is islands and peninsulas narrower than a kilometre. The count of biomes
   within 5 km cannot reach 15–25 with climate zones 10 km from pole to equator: every 5 km
   radius holds every zone. It needs either zones about twice as wide or fewer kinds of
   biome per zone; that is Mariano's call.

   Bare rock in the same step (decor's colour-cohesion rule, `crate::hue`): each rock
   province's exposed stones are the family of its main stone's colour, differing in
   shade rather than hue, laid in beds one to four blocks thick that are the same all over
   the province (`Province::bands`, `Fill::Rock`), on cliffs and wherever slope rules
   place rock, whose patches are now tens of blocks across. `Overworld::strata` lists the
   provinces' beds too, for distant terrain.
3. **3D density.** Band-limited lattice density with factor and roughness; the fill from
   density; surface rules at exposed tops and undersides; rivers, lakes and trails as caps
   and floors; caves combined with `min`; exact `height()`. *Done when:* overhangs and
   ledges in mountains, clean river cuts, `fluids_are_contained` and the purity tests pass,
   the budget in section 7 holds.

   *As built* (`crate::density`): the router's `crag` field gives each column an amplitude
   in blocks (up to nine on the spines of ranges, a few on foothills, none on lowland), and
   the density is `(s − y)·k + a·n` around the preliminary surface `s`, with `k` 1 below it
   and `above` (0.5) over it and `n` a 3D noise (56 blocks, two octaves, faster up than
   across) on a lattice 4 blocks across and 8 up. A column's height is its highest solid
   block, the same for single samples, areas and generation (batched, sharing lattice
   points), and generation hollows the band under it; ledges under an overhang take the
   surface's top, undersides are rock. The density is zero near water, in river tunnels,
   under trails and at or below the water beside a column, so rivers, lakes and the sea keep
   their banks; trails plan over the preliminary surface. Caves carve as before. The result
   is tors, crags, ledges and some overhangs along the crests rather than a sheer 3D
   fill everywhere: the factor and roughness splines and 3D surface rules beyond ledges and
   undersides are left for step 6, where captures beside Tectonic's will say how far to go.
   Cost: +2% instructions on main 03811fb for the first column and for generation.
4. **Shapes into the router.** Every shape as a gated term (section 6.4); `Piece`, `sculpt`
   and the standalone radial code are deleted. *Done when:* the `SHAPES=1` preview shows no
   circular outline, and each shape has a capture next to its reference.

   *As built:* the 3D pieces are gone (`Piece`, `Anchors`, `sculpt`, `Overworld::pieces`,
   leaning spires, the lone mountain), and every shape is a term of the land. Towers, spires
   and sea stacks rise where a warped noise passes a threshold that falls in clusters, so
   each outline is ragged and some merge into ridges. Arches are fins along the zero line
   of a noise, full height from halfway into their climate, with a window under them where
   a second noise rises (`Shaped::window`): generation opens it from the fin's foot and
   always leaves four blocks of fin above it, as the land eased the fin, and never beside
   water. Volcanoes are the one shape still placed around a centre, as a volcano is: on a
   jittered grid, only on land, gathered into volcanic fields (regions of the same kind of
   mask the rarer shapes use, about one cone per fifteen square kilometres overall in a
   moderately volcanic world), with breached craters and shallow gullies. Their distance
   from the vent is measured through a domain warp a third of their radius deep, which
   bends the outline into irregular lobes. (The old outline wandered with the angle, which
   drew a starburst on the relief map.) Fields of cones on terrain shaped by the router's
   rarity fields (R in section 6.1) are left for step 6.

   Surfaces in the same step, after the step-3 captures showed gentle range slopes as a
   checkerboard of pale rock, grass and dirt: slope rules read `Site::steepness`, the
   gradient over four blocks averaged across three rows, instead of the largest single step,
   so a stepped slope reads as the even slope it is; the per-block share of the dither and
   mix values fell from 45% and 20% to 4% and 3%, so materials meet along ragged edges of
   patches tens of blocks across; soil slopes break into coarse dirt and rock only past
   steepness 1.3 and 1.8; and the risers of steep, stepped soil show the province's rock
   beds (`Fill::Beds`) under a lip of the biome's ground. Cliffs go by steepness too.
   Measured with `surface_patches` on gentle ground (steepness under 0.7) of the three
   step-3 range views of seed 7, the share of top blocks unlike three or four of their
   neighbours fell from 22.5% to 2.8% (over the ridge), 18.3% to 2.6% (along the spine) and
   13.2% to 7.9% (below the crest), and of neighbours that differ from 22–34% to 10–15%.
   The speckle left is mostly the biomes' own mixed grounds (podzol with coarse or rooted
   dirt), which is content.
   Cost, in instructions against main 80eb200 on the same world: +3.9% for generation
   (`generation_stages`) and +6.0% for the first column (`first_column`). Most of it is
   steepness, which reads a two-block border around each column, and the exact 5 × 5
   neighbourhood `Sampler::site` now samples, which the spawn search calls thousands of
   times.
5. **Gorges and caves.** Slot gorges from rivers, random-walk ravines retired, cave
   entrances and aquifers on the density. *Done when:* gorge captures look like the
   reference creek gorge (`0740_rocky-gorge-creek.png`), caves stay contained.

   *As built:* each river span records how far the ground at its two cells stands over
   its water beyond the tier's usual incision (`Span::cut`): next to nothing along most
   rivers, several blocks where a breach cuts through a rim. From 3 to 8 blocks of that,
   the valley closes into a slot gorge (`water::channel::GORGE`): a ledge a block and a half
   wide beside the water, then walls climbing 3.3 blocks per block out, wavering by under a
   block, which the slope rules turn to the province's bare rock. Along most rivers nothing
   changes (the profile is the same to the bit where there is no cut). On seeds 7 and 1,
   about one trunk span in eight and one stream span in fifteen runs partly in a gorge: in
   16 km², 15 trunk spans (about a kilometre of canyon) and 30 stream spans. Trunk gorges
   are canyons twenty to thirty blocks deep; creek gorges are the small kind in the
   reference, five to ten. `/locate gorge` finds the nearest (`Overworld::locate_gorge`,
   which reads the rivers region by region, nearest first), `Overworld::water_sights` lists
   one among the other sights (`WaterSight::Gorge`), and the landing gate goes to the
   nearest and checks the server's world has water there between walls six blocks high.

   The random-walk ravines are buried, all of them: a lens-shaped canyon under the lowest
   ground along its path, never open to the sky. Caves already read the density's top as
   the surface (a column's `height` is its highest solid block since step 3), so entrances
   and the water tables needed no change.

   Two notes from step 4 went in too. Arches are short, thick spans (10 to 25 blocks long,
   6 to 10 thick, steep-sided) with a window through the middle
   of the higher ones, a lintel of a third of their height and legs at both ends, and no
   window under a tower an arch runs into. The spawn search checks that the ground is
   level from the neighbours' heights before it samples a site.
6. **Comparison and tuning pass.** Captures beside Tectonic and Terralith screenshots and
   the hiking frames, before and after, for mountains, coasts, plains, plateaus, valleys,
   gorges and every biome; a performance pass.

Steps 1 and 3 are the big ones. Step 2 touches `biomes.ron` claims only (not palettes), to
stay clear of the decor track's colour work.

## 9. Decisions for Mariano

1. **Latitude.** Keep ordered climate zones by latitude (pole to equator about 10 km, so
   every zone is large), plus noise? Or go fully noise-driven like vanilla? I recommend
   keeping latitude.
2. **Seas.** Keep Tectonic's folded continents (land as large islands between connected
   seaways) at a larger scale, so most land lies kilometres from the sea? Or vanilla's
   unfolded continents with true interiors? I recommend folded, at about 1.5× today's size.
3. **Tectonic's splines.** Start from Tectonic's MIT splines and density graph (credited),
   or author ours from scratch? Starting from Tectonic's is faster and proven.
4. **Terralith.** Agree to keep not reading its files (licence, section 3.3) and rely on
   the earlier notes?
5. **Biome size.** Confirm the targets in section 6.6 (median region 1.5 km or more).

## 10. Sources and licences

Fetched into `~/Projects/voxmar-wt/research/terrain-source/`:

| Source | What | Licence | How we may use it |
| --- | --- | --- | --- |
| `tectonic/` (github.com/Apollounknowndev/tectonic, 3.0.25, 34241bd) | the whole density graph (245 JSON files), config and density-function Java | MIT (declared in `build.gradle.kts`; the repo has no LICENSE file) | adapt directly, credited in `THIRD_PARTY_NOTICES.md` and `INSPIRATIONS.md`; not the vanilla data it copies |
| `vanilla-data/` (github.com/misode/mcmeta, `data` branch, 26.4-snapshot-2) | noise settings, density functions, noises, carvers | Mojang's | techniques only |
| Minecraft Wiki: Noise router, Density function | definitions (`squeeze`, `quarter_negative`, `interpolated` cell sizes, router fields) | CC BY-NC-SA | techniques only |
| `research/terralith-tectonic.md` | the earlier study of Terralith 2.5.1 and Tectonic | our notes | as notes |
| Terralith (github.com/Stardust-Labs-MC/Terralith) | not read in this study | Stardust Labs licence: no redistribution; no use with AI or generative systems | nothing from its files |

`tools/df.py` in the same folder prints density-function JSON as readable expressions
(`python3 tools/df.py tectonic/src/common/main/resources/resourcepacks/tectonic
tectonic:sloped_cheese --inline`). The experiment toggles behind section 2's table are in
the session scratchpad (`terrain2/experiment-toggles.patch`), not in the repository.
