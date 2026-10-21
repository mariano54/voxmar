# The item in the hand

Oct 13, 2026. Mariano's playtest: "I'm not seeing the weapon in first person, I'm not
seeing what I am holding." Nothing had ever drawn it. Now the item in the selected hotbar
slot is drawn low in the right of the view.

## What is drawn

| Held | Drawn as |
| --- | --- |
| A tool or weapon, any other item | its icon (the block texture of the item) as a thin sprite, the way a dropped item is |
| A block | a small cube, a corner towards the player |
| Nothing | nothing |

It is the dropped-item renderer (`render/items.rs`, `items.wgsl`) with one more thing an item
can be: *held*, placed in the camera's frame instead of the world. The shader reads the
camera's axes itself, so the hand moves with the very view the frame is drawn with and never
lags a frame behind the head, and it holds the item's depth just inside the near plane, so a
wall the player stands against cannot cut it. It is queued for the camera only, so it casts no
shadow. It takes the sky and block light at the player's eye (measured again as they move and
every 0.2 s, and eased at 8 a second), so it is dark in a dark cave and warm beside a torch.
A light carried (`held_light:N`: the firebrand 8, burning down in the hand; the torch 12,
burning nothing down) lights the hand and what is around it, and keeps monsters from coming
up near its bearer as a torch set down does; load refuses one brighter in hand than set down.

## The empty hand

With nothing held the player sees their own fist and forearm (`bare` in `held.ron`): a row of
cubes, the fist at the pose and each piece of the arm a set spacing further back toward the
elbow, drawn like a held block, in the hand's light. The drawings are `block/held_fist`,
`block/held_arm` and `block/held_sleeve` (`recipes/hand.ron`, drawn for this game); a pack
redraws them by those names, and one it lacks is a skin-coloured placeholder. All of the hand
is drawn at one depth in front of the world, so the pieces go nearest last. It punches (peaking
as the fist's 0.08 s wind-up ends), jabs at a block while digging, dips for a use, raises the
fist when guarding (with a shield worn: bare fists have no guard of their own), and dodges and
flinches as anything held does. A pack without `bare` draws no empty hand.

## What it does

`client/src/held.rs` moves it from what the player does, all timed by the things that time
those:

- A breath at rest and a sway with the walk (the speed along the ground).
- Switching to it brings it up from below in a quarter of a second.
- A **dig** swing, repeated while the left button is held with no creature in the way.
- An **attack** swing when the fight's rhythm starts over (a click that swings, at once or held
  for the beat), not for a bite of food, a raised guard, or a switch of what is held. It peaks
  as the server judges its blow: the wielded class's wind-up (`combat.ron`: dagger 0.06 s,
  sword 0.10, spear 0.12, axe and sling 0.15, mace 0.18, bow 0.2), whatever the stroke's own
  `seconds`, which then pace only the follow-through back to rest. Timed in game seconds, never
  by the frame rate; a landed blow's hit-stop holds it still.
- A **dodge** when a dash begins, the hand left behind on the side dodged to (the data is the
  dodge to the right; to the left mirrors it), and a **flinch** when a blow is taken unguarded.
  The view tips with the dodge (`combat_fx.ron` camera `dodge_lean`, with Screen shake).
- A **place** dip on a right click (placing, using, eating), not with the guard up.
- The **guard** stance while the guard is up, and the **eat** stance, food lifted to the face,
  while eating.

## Data: `held.ron` in a texture pack

`assets/packs/default/held.ron` (Voxmar HD builds on it). A pack's own entries replace those of
the packs below, entry by entry, and the game logs which: the hand's motion, each move, the
pose of a block or of any other item, a kind by its `tag`. A new kind goes ahead of the older
ones. The kinds are matched against the held item's tags in order (`sword`, `dagger`, `spear`,
`weapon`, `pickaxe`, `axe`, `shovel`, `hoe`, `knife`, `shears`, `rod`, `light`, `edible`); no
tag fits, and the block or sprite pose is used. A kind may carry its own `moves`.

Every number is checked at launch (`Book::finish`): all parts present, finite, sizes and
distances in range, strokes 0.05 to 3 s, a kind not listed twice. Anything wrong is a warning
and then nothing is drawn in the hand (the tests hold the shipped file to this). A kind whose
tag no item has is left out with a warning, and a test fails for the shipped file.

## Audit of the player's fight moves (October 2026)

Before: every swing peaked at its stroke's own pace, after the server had judged the blow
(sword 0.17 s against a 0.10 s wind-up, dagger 0.10 against 0.06, spear 0.20 against 0.12, mace
0.23 against 0.18, axe 0.23 against 0.15); the dagger and spear thrusts barely moved; a dodge and
a blow taken did nothing in the hand. After: the peaks meet the wind-ups, the thrusts reach
further, and dodge and flinch moves exist (`/mnt/project-files/combat/player-swing-before-after.png`).
The guard already rose in its 0.14 s stance, well inside the 0.3 s tells. Still open: the empty
hand (fists) drew nothing, so a punch had no first-person move at all; it now has the fist and
arm above (`/mnt/project-files/combat/empty-hand-before-after.png`).

## Tests and the gate

- `held.rs`: the shipped file loads and holds every item tagged `weapon` or `tool` as its own
  kind; every pose rests in the lower right clear of the crosshair, and every move stays in
  frame, at 4:3, 16:9 and 21:9; the raise, strokes (repeat while the button is held, stop when
  let go), stances, swing detection, light easing, and a later pack's replacements; a swing
  peaks within 1/240 s of every wind-up for every kind, the same at 20 and 480 frames a second,
  and holds still while frozen; a dodge leans to its side and a blow taken knocks the hand.
- The gate's `held` step (docs/quality.md): shots of the empty hand, an iron sword and a stone
  block under the open sky, and the sword in a sealed stone box. The `weapon` step's first
  swing at the bear is watched in the hand on a fixed 1/60 s clock (`HeldClock`): it must peak
  within a frame of the sword's wind-up, go all the way out, come back to rest and be
  answered by the server, shot as it connects (`player swing`).
- The gate's `strike` step strikes the snail with the empty hand and watches its first punch
  the same way (`player punch`), and that the hand was drawn with every piece of the arm.
- `VOXMAR_HELD_PREVIEW=DIR cargo test -p voxmar_client bare_hand_strip -- --ignored` writes
  `bare.png`: the empty hand's punch, dig, use, guard, dodge, flinch and raise, frame by frame,
  through the same maths as the shader (cubes, each covering those before it).
- `VOXMAR_HELD_PREVIEW=DIR cargo test -p voxmar_client held_swing_strip -- --ignored
  --nocapture` writes `swings.png`: each weapon's swing, the guard, both dodges and the flinch,
  frame by frame at 1/30 s, the judged frame in red and the peak in yellow.
- To look at the poses without a GPU: `VOXMAR_HELD_PREVIEW=DIR cargo test -p voxmar_client
  held_preview -- --ignored` writes a contact sheet of flat renderings of the poses through the
  same maths.
