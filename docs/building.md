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
| Ridge end, peak | `thatch_ridge_end`, `thatch_peak`, `<timber>_shingle_ridge_end`, `<timber>_shingle_peak` | a ridge whose end slopes down as its sides do, where a hipped roof's end meets it; a ridge sloping on all four sides, where four hips meet over a square | ridge end: its end towards the placer | a ridge over a roof piece makes 2 ridge ends, 2 ridges side by side make 2 peaks (thatch 1, shingles 5) |
| Hide lean-to | `hide_lean_to` | hide in four thin courses on two rafters, eave towards the placer | | 3 hide, 2 sticks make 2 (1) |
| Wattle | `wattle_panel` | woven sticks across the middle of the cell, three stakes | faces the placer | 8 sticks round a twine make 4 (1) |
| Daub | `daub` | a whole block | | 2 clay, a twine and a wattle panel make 2 (2) |
| Half-timber | `half_timber` | a whole block: daub in a braced timber frame | | 2 beams, 2 daub make 4 (4) |
| Shutters | `wooden_shutters` (`_open`) | two plank leaves across the middle; open, folded back to the jambs and stopping nothing | face the placer; a right click opens and shuts them | 4 planks, 2 sticks make 2 (2) |
| Chimney | `chimney_stack`, `chimney_pot` | a brick flue 3/4 of a block across; a clay pot with a rim | | 4 stone bricks make 2 (6); 4 fired clay round a flue make 2 (3) |
| Stone flue, hood | `stone_flue`, `chimney_hood` | a cobble flue as the brick one; a brick skirt round a hearth narrowing to a flue | | 4 cobblestone and clay make 2 (2); 5 stone bricks make 1 (6) |
| Jetty beam | `<timber>_jetty_beam` | a joist along the top of the cell, out past the wall, a brace stepping down to the wall | overhang towards the placer | 2 beams, a post make 2 (4) |
| Brace | `<timber>_brace` (`_top`) | a knee brace, a timber five texels wide stepped corner to corner from its foot against the post behind it up to the front of the cell's top; turned over, it braces a beam from below | faces the placer, its foot to the wall; flips like a slab | 2 posts laid corner to corner make 4 (3) |
| Door frame | `<timber>_door_frame` (`_top`) | jambs either side and a lintel, two blocks high; stops nothing | placed whole, as a door; faces the placer | 2 posts, a beam make 1 (3) |
| Window frame | `<timber>_window_frame` | jambs, a sill and a lintel round an opening, one block high; stops nothing | faces the placer | 2 posts, 2 beams make 2 (3) |
| Shuttered window | `<timber>_shuttered_window` (`_shut`) | a lattice window with two leaves on its face, standing out open, across it shut | faces the placer; a right click shuts and opens them | a lattice window and shutters make 1 (3) |
| Railing | `<timber>_railing`, `<timber>_railing_corner` | a handrail and a bottom rail along the back of the cell with balusters between, a block high; the corner turns along the east side too | the rail runs along the side away from the placer | 3 beams, 3 sticks make 3 (3); 2 railings make a corner (3) |
| Fence, gate | `<timber>_fence`, `<timber>_fence_corner`, `<timber>_fence_gate` | half a post at each end and two rails across the middle of the cell, a block high; the corner turns from the west to the north; the gate a frame of rails and uprights, swung out north against its west post when open | runs across the placer's view | 4 posts, 2 sticks make 6 (2); 2 fences make a corner; 2 fences round a slab make a gate (2) |
| Double gate | `<timber>_double_gate` (`_open`, and `_leaf`, `_leaf_open` for its second leaf) | two leaves two cells wide between a half post at each outer end, meeting in the middle; open, each swung out north against its own post | runs across the placer's view; the second leaf goes in to the placer's right with it, and the two open, shut and break together (`beside`); broad cattle (an aurochs, a bison) can be led through it, never a gate one wide | 2 gates make a double gate (2) |
| Ladder | `<timber>_ladder` | two rails and four rungs at the back of the cell, against the wall behind | faces the placer, its back to the wall | 6 posts, 3 sticks make 4 (2) |
| Stair cupboard | `<timber>_stair_cupboard` | a stair (the step facing north) with panelled doors in its riser: walked up as a stair, counted as storage | faces the placer | a stair, a plank and a stick (5) |
| Built-ins | `<wood>_window_seat`, `<wood>_sill`, `<wood>_pantry_shelf` (furnishings) | a panelled box under a cushion against the wall (a seat and storage); a deep board on two brackets at the top of its cell (decor); a shelf with two crocks and a keg (storage) | face the placer, their backs to the wall | a bench, 2 planks, wool (5); a slab and a stick make 2 (3); a shelf and 2 clay pots (5) |
| Hearth corner | `<wood>_settle`, `<wood>_woodpile`, `<wood>_peg_rail` (furnishings) | a high-backed bench with arms and a cushion, sat in as a seat (`settle`); logs stacked two, two and one against the wall (`woodpile`); a rail high on the wall with three pegs, a cloak and a pot hung on them (decor; within two of it, wet things hung up dry you off as by a fire, `pegs`). A settle and a woodpile by the hearth under the roof make a Hearth corner (docs/home.md) | face the placer, their backs to the wall | a bench and 3 planks (4); 3 logs and 2 sticks (1); a plank and 3 sticks make 2 (2) |
| Bedchamber | `<wood>_blanket_chest`, `<wood>_washstand` (furnishings) | a panelled box under an overhanging lid, for the bed's foot (storage, `blanket_chest`); a top on four legs with a low shelf, a basin and a jug on it (`washstand`). With the bed and a rug under the roof they make a Bedchamber (docs/home.md) | face the placer | 4 planks and wool (3); a table and 2 clay pots (4) |
| Rushlight, window box | `<wood>_rushlight` (furnishings), `window_box` | a peeled rush burning in a clip on a little wooden stand, light 7, a home's `light`, burnt down in a game day to its bare holder (`<wood>_rushlight_spent`), which any fresh reed (a rush, cattail, papyrus or sugar cane) lights again; a trough of flowers on two brackets against the wall outside under a window, in its own colours, a `yard` piece out under the sky | the window box faces the placer, its back to the wall | a plank, a stick and 2 rushes make 2 (Crafting 2; taught by the Nomad home quest By Rushlight, before A Lit Corner); 3 planks, dirt, a poppy and a dandelion (3) |
| Kitchen wall | `<wood>_dresser`, `<wood>_pot_crane` (furnishings), `rush_mat` | a panelled cupboard under a plate rack with five plates on show (9 slots, storage and crockery); a post on the wall with an arm and a pot hung from its hook (decor, `pot_crane`; with the dresser by the hearth under the roof, a Kitchen room, docs/home.md); rushes woven flat on the floor (a rug) | the dresser and crane face the placer, their backs to the wall | a cupboard, 2 planks and 2 crockery (6); 2 planks, 2 sticks and a clay pot (3); 4 rushes (Crafting 1) |
| By the fire | `<wood>_rocking_chair`, `<wood>_cradle` (furnishings) | a chair on two curved rockers with an arm each side (a seat); a crib of boards on two rockers with a rush bed and a pillow in it, its headboard higher (decor) | both face the placer | a chair and 2 planks (4); 3 planks, 2 sticks and 2 rushes (4) |
| Lived in | `<wood>_mantel_shelf`, `<wood>_fireside_bench` (furnishings) | a deep board on two brackets high on the wall, a lip along its front, showing the three things put in it on the board (a store of 3 slots, `shows`); a low bench with a fleece thrown over it (a seat). Both by the hearth under the roof make a Lived-in hearth room (docs/home.md) | both face the placer, the shelf its back to the wall | a shelf and 2 sticks (3); a bench and a hide (2) |
| Plate rack | `<wood>_plate_rack` (furnishings) | a post at each end, a low ledge and a front rail on the wall, showing the four things stood on its ledge (a store of 4 slots, `shows`) | faces the placer, its back to the wall | 2 planks and 4 sticks (3) |
| Bedside and chair | `<wood>_bedside_table`, `<wood>_footstool`, `<wood>_spice_rack` (furnishings) | a little table on four legs with a drawer, showing the two things stood on its top (a store of 2, `shows`); a low footstool, its top woven of rushes (a seat); a back board, ledge and rail on the wall that keeps only herbs and shows the three in it (a store of 3, holds `tag:herb`) | the table and rack face the placer, the rack its back to the wall | a stool and a plank (3); a stool and 2 rushes (2); a plank and 2 sticks (2) |
| By the hearth | `<wood>_candle_box`, `<wood>_bread_bin`, `<wood>_kindling_box` (furnishings) | an open box with a low front on a back board on the wall that keeps only candles and shows the two standing in it (a store of 2, holds `tag:candle`), and at home sets one out in place of a candle burning out (`spare_lights`); a box on the floor under a lid with a knob, keeping the loaves under it (a store of 4, holds `tag:baked`); an open box on the floor, its front lower than its back, keeping only what burns and showing the top two things in it (a store of 4, holds `tag:fuel`), and at home lights again from what it keeps a torch or lamp burning out (`relights_lights`) | all face the placer, the candle box its back to the wall | a slab and a plank (2); 3 planks and a slab (2); 2 planks and a slab (2) |
| Grain sack | `sack` (furnishing) | a sack of grain with its neck tied off, as a village's farmers and millers keep; a store of 4 that holds only `tag:grain`, home storage; given back whole where a player set it, nothing where a village did | stands on the floor | 6 twine (Crafting 2) |
| Fishing net | `fishing_net` (furnishing) | a net with cork floats hung to dry from the block above, as a village hangs one on a frame by the water; given back whole where a player hung it, nothing where a village did | hangs from the block above | 4 twine and a stick (Crafting 3) |
| Log bench | `<wood>_log_bench` (furnishing) | a split log laid flat on two short stumps, as a village sets one by its lanes; a seat; a village's gives nothing when broken, a player's comes back | faces the placer | 2 stripped logs make 2 (1) |
| Barrel, crate | `barrel`, `crate` (furnishings) | staves round two hoops, as a village keeps its stores in; boards with gaps between them, braced at the corners; each a store of 9; a village's gives nothing when broken, a player's comes back | | 6 planks round an open middle with a twine hoop at top and bottom (4); 4 planks with a stick at each corner (2) |
| Hatch | `<timber>_hatch` (`_open`) | boards across the top of the cell over two battens; open, hung against the back of the cell no deeper than a ladder's rungs | faces the placer; a right click opens and shuts it | 2 slabs on 2 sticks make 2 (2) |
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
room Cosy, which asks nine tenths sound. A lattice window, and a shuttered one while it
stands open, is tagged `window` as glass and hide are: it glows by the light through it
and a home counts it among its windows. It draws with its own `lattice_grain`, since a
glow is a drawing's: on the grain posts and chairs share, every one would glow (refused at
load). Shut, the shutters are a sound wall, keep the light out and are no window; a bare
frame (an opening), plain shutters folded back over one, and an arrow slit are none either.
Every window lets light in: a block tagged `window`, `lattice`, `window_frame`,
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

A ladder (`<timber>_ladder`, tagged `climbable`) is climbed as in Minecraft: with your feet
in its cell, pressing against it (or the wall behind it) or holding jump takes you up at
`climb_speed` (core `movement.ron`, 3 blocks a second, at most `jump_velocity`), and letting
go slides you down no faster than that. You climb off its top: past the last rung your feet
leave its cell, and pressing on steps you onto the wall or floor it leads to. The server
counts a ladder your feet are in as something under you, so a climb taller than a jump is
not put back, though nobody rises on one faster than `climb_speed`; and a slide down one is
no fall, so it neither hurts nor lands a falling blow (`sliding_down_a_ladder_is_no_fall`).
A ladder is furniture to a home's survey (survival.ron's `home_furniture` names `ladder`),
so a ladder up to a loft leaves the room as walled as it was (engine `movement.rs` `on_ladder`; tests
`a_ladder_is_climbed_onto_the_wall_behind_and_slid_down_when_let_go` and gameplay
`an_honest_climb_up_a_ladder_is_never_put_back`; the gate's Pieces step judges one). At load
a climbable block must be solid, so there is something to press against. Creatures do not
climb ladders.

A hatch (`<timber>_hatch`) closes a hole in a loft's floor: shut, its boards are level with
the floor round it, walked on, and a sound bit of ceiling to the room below. A right click
swings it open (`swaps`); open, it hangs against the back of its cell and is climbed as a
ladder is (`climbable`, a draughty face to the room below), so a ladder under it, set with its
back the same way, climbs on up through it and out onto the loft floor
(`a_ladder_climbs_through_an_open_hatch_into_a_loft_and_a_shut_one_stops_it`; the gate's
Pieces step judges one shut and one open).
The home's load check (`Rules::check_shelters`) holds every solid building or roof block,
and every solid shaped piece, but furniture to exactly one grade.

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
