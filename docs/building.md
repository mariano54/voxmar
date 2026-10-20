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
| Hip, valley | `thatch_corner`, `thatch_inner_corner`, `<timber>_shingles_corner`, `<timber>_shingles_inner_corner` | a roof's courses stepping in from the north and the west (a hip, where two eaves meet), or back from both (a valley, where two slopes meet in an L) | eaves towards the placer and their right | 2 roof pieces side by side make 2 hips, one over the other 2 valleys (thatch 1, shingles 5) |
| Ridge | `thatch_ridge`, `<timber>_shingle_ridge` | courses rising from both sides to a cap | ridge across the placer's view | thatch: 3 fibre make 2 (1); shingles: 3 slabs make 2 (5) |
| Hide lean-to | `hide_lean_to` | hide in four thin courses on two rafters, eave towards the placer | | 3 hide, 2 sticks make 2 (1) |
| Wattle | `wattle_panel` | woven sticks across the middle of the cell, three stakes | faces the placer | 8 sticks round a twine make 4 (1) |
| Daub | `daub` | a whole block | | 2 clay, a twine and a wattle panel make 2 (2) |
| Half-timber | `half_timber` | a whole block: daub in a braced timber frame | | 2 beams, 2 daub make 4 (4) |
| Shutters | `wooden_shutters` (`_open`) | two plank leaves across the middle; open, folded back to the jambs and stopping nothing | face the placer; a right click opens and shuts them | 4 planks, 2 sticks make 2 (2) |
| Chimney | `chimney_stack`, `chimney_pot` | a brick flue 3/4 of a block across; a clay pot with a rim | | 4 stone bricks make 2 (6); 4 fired clay round a flue make 2 (3) |
| Stone flue, hood | `stone_flue`, `chimney_hood` | a cobble flue as the brick one; a brick skirt round a hearth narrowing to a flue | | 4 cobblestone and clay make 2 (2); 5 stone bricks make 1 (6) |
| Jetty beam | `<timber>_jetty_beam` | a joist along the top of the cell, out past the wall, a brace stepping down to the wall | overhang towards the placer | 2 beams, a post make 2 (4) |
| Door frame | `<timber>_door_frame` (`_top`) | jambs either side and a lintel, two blocks high; stops nothing | placed whole, as a door; faces the placer | 2 posts, a beam make 1 (3) |
| Window frame | `<timber>_window_frame` | jambs, a sill and a lintel round an opening, one block high; stops nothing | faces the placer | 2 posts, 2 beams make 2 (3) |
| Shuttered window | `<timber>_shuttered_window` (`_shut`) | a lattice window with two leaves on its face, standing out open, across it shut | faces the placer; a right click shuts and opens them | a lattice window and shutters make 1 (3) |
| Railing | `<timber>_railing`, `<timber>_railing_corner` | a handrail and a bottom rail along the back of the cell with balusters between, a block high; the corner turns along the east side too | the rail runs along the side away from the placer | 3 beams, 3 sticks make 3 (3); 2 railings make a corner (3) |
| Fence, gate | `<timber>_fence`, `<timber>_fence_corner`, `<timber>_fence_gate` | half a post at each end and two rails across the middle of the cell, a block high; the corner turns from the west to the north; the gate a frame of rails and uprights, swung out north against its west post when open | runs across the placer's view | 4 posts, 2 sticks make 6 (2); 2 fences make a corner; 2 fences round a slab make a gate (2) |
| Newel post | `<timber>_newel_post` | a post five texels square with a cap | | a slab on a post makes 1 (3) |
| Landing | `<timber>_landing` | boards across the top of the cell on two joists, level with a stair's top | joists run away from the placer | 3 slabs on 2 beams make 3 (3) |
| Balcony door | `balcony_door` (`_top`, `_open`, `_open_top`) | a plank door whose upper half is a lattice | placed whole, as a door; a right click opens and shuts it | a lattice window on a wooden door makes 1 (4) |
| Arrow slit | `stone_arrow_slit` | dressed stone with an upright slit three texels wide through it | the slit runs away from the placer | 2 stone bricks, a brick slab make 2 (6) |

The home's ladder runs from a hide lean-to through wattle and daub and timber to stone and
brick. Every piece on it carries a shelter tag for how well it keeps the weather out of a
room: `shelter:draughty` (hide, wattle), `shelter:sound` (daub, half-timber, shutters, the
wooden door, roofs) or `shelter:fine` (brick); the home's wall scan reads them. Their
recipes start from worked materials (sticks, twine, hide, fired clay, bricks, beams), never
from blocks straight out of the ground; daub also takes clay, smoothed onto a wattle panel,
and the stone flue cobbles laid in clay.

Timber posts, beams and jetty beams are sound, lattices draughty; thatch and shingles are
both sound (the trade between them is fire); the stone flue and the hood are sound. A door
or window frame is an opening, not a wall, and carries no grade, as an open door or shutter
does not: a room with an empty frame is open there.

A window walls a room in but lets the weather through: a lattice window, a shuttered one
left open and an arrow slit are draughty, and carry `draught`, the tag of a wall a draught
comes through (which a draughty home's whistle can listen for). `draught` means an open
gap: an oiled-hide window or old glass is a draughty wall with no gap in it, so it
carries no `draught`. A room with them is as
closed as without, and as Snug, but each is a face short of sound: enough of them cost a
room Cosy, which asks nine tenths sound. Shut, the shutters are a sound wall and keep the
light out. Every window lets light in: a block tagged `window`, `lattice`, `window_frame`,
`slit` or `draught`, or lit through, is refused at load if it is a whole cube or dims light
by more than 2 (`light_filter`); a misspelt `shelter:` tag, or `draught` on anything but a
draughty wall, is refused too.

Stairs and upper floors: the woods' and the cut stones' stairs climb in half-block steps
without a jump to a landing level with the top stair, and a railing along a balcony, a
gallery or a landing keeps anyone from walking off it. A railing stands a block high, so
nobody steps onto it, and to a creature it stands two high, as a fence does, so nothing
hops over it: no animal walks on its top or on air over it, even from a step beside it.
A boss, or a fighter after a player, climbs it as the block-high step it is, so a ring of
railings is no pen to fight from. A field fence and its gate are railings too (tagged
`railing`), so a ring of fences pens a farm's animals; a right click swings the gate open
(`swaps`, as a door), and open it stops nobody, so a mouflon led by wheat follows you out
through it (`a_fenced_pen_holds_a_led_mouflon_until_its_gate_opens`; the gate's Pieces step
judges a fence, a corner and a gate). It walls no room: a home's survey looks past it (survival.ron's
`home_furniture` names `railing`), so a gallery round a hall leaves the hall as walled as it
was. A shut balcony door is a sound lower half and a draughty lattice over it; open, it is
a way through, as an empty frame is. At load a block tagged `railing` must be solid and a
block high, and one tagged `landing` solid across the whole top of its cell.
The home's load check (`Rules::check_shelters`) holds every solid building or roof block
but furniture to exactly one grade.

Chimney pieces (`chimney_piece`: the stone and brick flues, the hood, the pot) carry a
hearth's smoke up through a roof: stacked straight over the hearth's chimney (or over any
block that smokes), its smoke comes out of the top of the stack while flames, embers and
sparks stay at the fire, so a chimney carried up past the eaves smokes from its pot and not
under the roof (client `emitters.rs` `flue_top`; a stack of any height smokes from its top
piece; tests `only_smoke_goes_up_a_flue` and
`a_flue_over_a_chimney_carries_its_smoke_up_to_the_pot`; the gate's Cook step builds its
hearth's chimney with a brick flue and a pot and counts the pot's puffs, "chimney smoke").
The stone flue comes at Construction 2, before brick, so a thatched house can have one, and
the home's fire rule keeps a chimney's roof from catching (solid pieces line the flue, as
stone does).
A jetty beam sticks out of the wall under an upper floor that overhangs the street; a door
frame finishes a doorway with no door, and stops nothing, so villagers and animals walk
through it as through any open way.

Cut stone keeps its stone's tags (a cobblestone slab is `stone`) but never stands in for a
whole block: a recipe's or a multiblock's `tag:` cell that whole blocks answer takes no piece
(`BlockRegistry::answers_whole`; the hearth's ring of stone takes no slabs, headstones or columns), since three
blocks cut into six would count double. A tag only pieces carry (`tag:beam`) takes them.
Cobblestone pieces are not `cobbled` (sling ammo, cobble recipes).

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
