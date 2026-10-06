# Voxmar HD: style guide

Voxmar HD is our own 32×32 texture pack, an alternative to the Minecraft look that players
pick in Settings → Video → Textures. It lives in `assets/packs/voxmar_hd`, written as
recipe books like the default pack, and builds on that pack (`pack.ron`: `extends:
["default"]`, `resolution: 32`): every texture it does not redraw itself is the default
pack's recipe drawn twice as fine, so it covers every block, item and particle texture the
game has from the first day, and everything it does redraw replaces ours by name.

This guide is what every texture in the pack follows, so a hundred textures drawn months
apart still read as one hand.

## The look

- **Slightly fantasy, our own.** Warm, lived-in materials, soft vegetation, a little
  storybook. A player who knows Minecraft should know what each block is at a glance, but
  never see Minecraft's drawing of it: our cobbles have our stones, our ores our crystals,
  our planks our grain.
- **Muted and warm.** Colours sit a little below full saturation and lean warm in the light.
  Nothing natural is pure grey: stone is a grey with a breath of violet in the
  shadows, dirt a soft umber, sand a pale straw. Saturation is kept for the things a player
  looks for: flowers, ores, fruit and magic.
- **Big shapes, drawn finely.** Stylised, not noisy: each texture is a few large, clear
  shapes (eight or nine cobbles, a handful of rock plates, a couple of dozen leaves, four
  boards) that read from across a valley, and 32×32 is spent on drawing those shapes well,
  with smooth shading inside each, crisp one-texel edges and a deliberate highlight, rather
  than on fine noise. Mariano's verdict on the first fine-grained draft (2026-10-06): "too
  uniform, the features are too tiny; features should be larger, but high res, more
  stylised".
- **Readable at a distance.** The big shapes are the block's silhouette of value, so they
  survive mipmapping; the detail lives in their shading and edges.

These goals come from Mariano's notes on the screenshots he liked (fine soft grass with
light yellow-green speckle and gentle variation, thin pale grass blades, flowers as dense
clumps of tiny blossoms, muted warm tones) and from the public descriptions of
[Alacrity](../INSPIRATIONS.md#alacrity) (warmer building textures, hand-crafted vegetation,
atmosphere without losing readability). No image of any other pack was copied, traced or
sampled.

## Palette ramps

Every material is one ramp of **7 to 9 shades**, darkest first (the default pack uses 4 to
6; more shades give the soft gradients 32×32 has room for).

- **Hue shift.** Shadows turn towards blue-violet, highlights towards yellow, about 15–25°
  across the ramp (a `Ramp` with `hue: 20`), the way a painter would light it. This is what
  makes a stone warm instead of grey.
- **Chroma.** Highest in the middle shades; highlights pale off and shadows deepen
  (`chroma: -0.3` to `-0.4`). The lightest shade of a natural material is never white and
  the darkest never black.
- **Lightness span.** About 0.30–0.38 in OKLab across a ramp for natural blocks (`light`),
  wider (0.45) only for things that must glint: ores, glass, ice.
- **Tinted textures stay grey.** Grass tops, the grass fringe on a block's side, leaves and
  water are tinted by the climate's colour in the shader, so their ramps are neutral greys
  with a slight warm lift in the highlights; only value is drawn.
- **Key colours.** The middle shade of each family, for reference:

  | family | key | notes |
  | --- | --- | --- |
  | stone | `#82827f` | grey with the barest warmth, violet shadows |
  | deepslate | `#4b4a54` | cool, the one cold rock |
  | dirt | `#7a5c46` | umber, never orange |
  | sand | `#d9c79b` | straw |
  | gravel | `#8a8682` | mixed stones, a few warm |
  | oak | `#a27b52` | honey planks, grey-brown bark |
  | spruce | `#6e4f36` | dark warm |
  | birch | `#d6c9a6` | cream |
  | leaves | `#8e8e8e` | grey, tinted |
  | water | `#9a9a9a` | grey, tinted, translucent |

## Light direction

Light comes from the **top left**, as in the default pack and Minecraft: the top and left
edge of a raised shape (a cobble, a board, a brick, a crystal) take the lighter shades and
its bottom and right edge the darker ones; a dent is the opposite. Top faces are lit from
straight above, so they get no directional rim on the block's own edges, only on the
shapes within. Never bake a cast shadow longer than one or two texels: the game draws real
shadows.

## Shapes and detail

Shapes come first, shading second, grit last and least:

1. **Shapes** (cells, bricks, boards, strata, sprites) carry the texture. Few and large:
   about two to four across a block for stone plates and bark, three for cobbles, four
   boards, leaves seven to ten texels long, ore crystals half the block tall. Draw sprites
   (leaves, crystals, nuggets, pebbles) as text grids with an outline, a lit face, a shaded
   face and a highlight, as a pixel artist would.
2. **Shading** within each shape: a smooth ramp of three or four shades from a lit top
   left to a darker rim (`Cells` with `bevel` and `round`; `lip: 0` drops the bright line a
   16×16 recipe puts on a cell's edge, which at 32×32 reads as scratches). Gentle macro
   variation across the block (noise of two to four lattice cells, amount 0.15–0.25).
3. **Grit**, if any: per-texel `Grain` at most 0.08 at 32×32, and sparse single-shade
   flecks. Fine noise is what made the first draft look uniform.

Rules of thumb:

- Neighbouring texels differ by at most two shades except on a deliberate edge (an
  outline, a mortar line, a seam, a crack).
- `clean: true` on every paint of a form, so no lone texel floats in a flat area.
- Highlights are few and placed: the tip of a crystal, the crown of a pebble, the lit lip
  of a crack.
- Dark accents (cracks, mortar, furrows) are bold where they are part of the shape (two
  texels of mortar between cobbles, deep bark furrows) and thin where they are detail.

## Edges

- **Natural blocks** (stone, dirt, sand, gravel, grass, ores, leaves) have no border at
  all: they must tile into one continuous surface.
- **Crafted blocks** (planks, bricks, polished stone, tiles) have one-texel seams with a lit
  top-left lip, so a wall reads as courses of laid material. Their block edge is a seam
  like any other, never a frame.
- **Frames** only where the object has one: glass panes, doors, trapdoors, item frames.
  Glass is a thin frame and a clear pane with two or three soft streaks, not a grid.

## Tiling and repetition

- Every pattern wraps (all texgen terms tile by construction); hand-written grids use
  `wrap: true` when they cross an edge.
- Big shapes, but no single one that catches the eye block after block: a lone dark crack
  or knot repeated on every block is the first thing an eye finds in a wall. Make shapes
  many-of-a-kind (nine cobbles, not one boulder), keep any one-off accent faint, and keep
  every shape under half the texture.
- Noise lattices divide 32 evenly (2, 4, 8 cells), so they tile seamlessly.
- Blocks with no up or down (stone, dirt, sand, gravel, grass tops, ores) turn and mirror
  their texture from block to block (`texture_variety` in `blocks.ron`), which removes
  most visible repeats; their textures must therefore look right in any of the eight
  orientations: no directional strata on them.
- Blocks with a direction (logs, planks, bricks, sandstone strata) keep one orientation,
  so their repeats are hidden by the pattern itself: boards with joints at varied places,
  bricks of varied tone, bark plates of varied width.

## Transparency

- **Leaves** look like Minecraft's in spirit: a dense mass of single leaves, here big ones
  (two dozen a block, each outlined, lit along its upper edge), with gaps opening only in
  the dark depths between them, never through a leaf. The game's
  leaf cards make the crown's outline, so the texture stays a regular all-over pattern.
- **Plants and flowers** are cut-outs: thin pale blades, and flowers as dense clumps of
  tiny blossoms. Few flowers in a clump, rather than many large ones. (The biome thread
  draws grass and flowers; the pack uses its art.)
- **Translucent** blocks (water, ice, stained glass) keep alpha even across the texture,
  varying colour more than opacity.

## How the pack is written

- `assets/packs/voxmar_hd/pack.ron` names the pack, the pack it builds on and its
  resolution.
- Its `recipes/*.ron` books replace the default pack's palettes, templates and textures of
  the same name. Replacing a **palette** recolours every texture drawn with it; replacing a
  **template** redraws a whole family (every wood, every rock's cobbled, polished and
  brick forms). Prefer those to redrawing textures one by one.
- Every recipe it writes itself gives `size: (32, 32)`; texel measures in it (brick
  sizes, borders, blob sizes) are in 32×32 texels.
- `texgen sheet --pack assets/packs/voxmar_hd --compare assets/packs/default` shows the
  pack beside ours; `--compare` with a Minecraft jar shows it beside Minecraft's, locally
  (never commit those sheets).
