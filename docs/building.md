# Building pieces

Shaped pieces a home or a village is built with, beyond whole blocks. Each is a block drawn
as its shape (`render: Model`, boxes in `assets/content/base/shapes.ron`) and collides as
those boxes (`BlockRegistry::collision`): a post is walked round, a beam stepped over, a
roof walked up course by course. A walker on the ground steps up to `step_up` (half a block
and a pixel, `core/movement.ron`) without jumping; a whole block still takes a jump.

| Piece | Blocks | Shape | Placed | Recipe (crafting table) |
|---|---|---|---|---|
| Slab | `<wood>_slab`, `cobblestone_slab`, `stone_brick_slab` | lower half; under a ceiling the upper (`flips`) | | 3 in a row make 6 (Construction 1, stone 2, bricks 6) |
| Stairs | `<wood>_stairs`, `cobblestone_stairs`, `stone_brick_stairs` | a slab and the back half | step towards the placer; upside down under a ceiling | 6 in a stair make 4 (2, 3, 7) |
| Post | `birch_post`, `oak_post`, `dark_oak_post` | 6 texels square, full height | | 3 logs in a column make 6 (2) |
| Beam | `<timber>_beam` | 6 texels square along the cell, on its floor; under a ceiling at its top (a joist) | runs away from the placer | 3 logs in a row make 6 (2) |
| Lattice window | `<timber>_lattice_window` | a frame and laths, light through the squares | faces the placer | 4 planks, 5 sticks make 4 (3) |
| Roof | `thatch_roof`, `<timber>_shingles` | four courses a quarter block high, eave towards the placer | | thatch: 6 fibre make 4 (1); shingles: 6 slabs make 4 (5) |
| Ridge | `thatch_ridge`, `<timber>_shingle_ridge` | courses rising from both sides to a cap | ridge across the placer's view | thatch: 3 fibre make 2 (1); shingles: 3 slabs make 2 (5) |

Timber comes in the furniture's three woods (birch, oak, dark oak), its greyscale drawings
tinted the wood's colour (`blocks_building.ron` family lines). Thatch is the first roof:
fibre is wheat or grass twine, so it is there from the first days.

A roof stops light (`light_filter: 14`): a roofed room is dark until it is lit, and the
courses' faces inside the roof's cell take the light of the open side they face
(`mesh.rs` `emit_model`). Every roof is solid, so it keeps rain off, a drying rack under it
does not dry, and a player under it is indoors for a home.

A gable: roofs facing out on each side, one course of cells higher each step in, and a
ridge along the top where an odd width leaves a middle row (an even width meets back to
back).
