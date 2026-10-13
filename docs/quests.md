# Quests

Oct 7, 2026. The quest engine (the progression plan's P20): quests as content, checked
against what a player has done, so they complete retroactively. The quest book screen
(P21) is the interface's; this is its data.

## Rules

- Nothing is locked behind a quest. Recipes stay open; quests point the way and reward
  the walk with experience, items and (later) knowledge.
- A quest completes the moment one of its tasks holds, however long ago the work was
  done: the goals read the player's state, not a log of events since the quest appeared.
- A task is a list of goals that must all hold; a quest offers one task per route (craft
  it, find it, trade for it).
- `after` names the quests that come first; `"a|b"` takes either. Finishing one quest can
  finish others that were already done, in the same moment.

## Content

`base/quests.ron` (engine `quest.rs`): name, age, branch, title, why (the reason and the
reward, shown first), tasks, after, rewards. Goals:

| Goal | Reads |
| --- | --- |
| `Craft(item, n)`, `Cook(item, n)` | the player's tallies of things made and cooked |
| `CookAt(item, station, n)` | successful cooking tallies for a station tag; First Night asks for edible food at a campfire, plus crafting the campfire |
| `Built(structure, n)`, `Lit(structure, n)` | standing construction and successful fuelled lighting, separately; factory construction never supplies lighting credit |
| `Tame(creature, n)` | the tally of animals tamed |
| `Catch(fish, n)`, `Catch("Kinds", n)` | the journal's Fish page |
| `Kill(creature, n)` | the journal's Animals and Monsters pages |
| `Pick(plant, n)`, `Mine(ore, n)` | the journal's Plants and Ores pages |
| `Visit(n)`, `Discover(page, n)` | how many entries the journal has |
| `Level(skill, n)` | the skill's level |
| `Have(item, n)` | what the player carries, pouches too |
| `Home` | a lit Homestead Hearth |
| `LookUp(item)` | the recipe viewer was opened on the item (the client says so once, `ClientMessage::LookedUp`; tally `lookedup:<item>`) |
| `Thriving(n, percent)` | the player keeps n animals right now at a condition of at least that percent (read live from the kept animals, not tallied; a newly tamed one starts at 60) |

Items and creatures may be `any` or `tag:TAG`. Rewards are `Xp(skill, xp)` and
`Item(item, count)`. Titles and reasons are translated as `quest.<package>.<name>` and
`quest.<package>.<name>.why`.

## On the server

Each second the server checks every player's open quests, finishes those whose task
holds, gives their rewards with a chat line, and sends `ServerMessage::Quests { states }`
when any state changed: each quest's name, whether it is done, and each goal's progress
as (have, need). The client keeps them in `LocalPlayer::quests`. Quests done and the
tallies are saved with the player.

The Nomad and Farm chapters (ages 1 and 2) have the quests the game can already do:
Nomad runs from the first campfire to the lit Homestead Hearth, Farm from the first harvest
and the pit kiln to the lit Crucible Kiln. Each has at least thirty quests
(`crates/engine/tests/early_quests.rs` keeps that floor and checks every name they use). The
loop through the first days, and what a new player is and is not told, is in
[first-days.md](first-days.md). The rest of quest book v2 joins as its content lands.

The Crucible Kiln needs a standing pattern and a complete charge held in the hand:
32 charcoal or 24 coal. Right-clicking its core lights it and consumes that charge.
Its burn time uses the fuel times in `machines.ron`, measured by server ticks; the
remaining fire is saved in `world.ron`. Cooking needs both the standing pattern and a
live fire. Removing a required piece quenches it; rebuilding requires a new charge.
Completed quests in older saves stay completed. Older `cooked` and `built` tallies do
not establish which station cooked food or whether fuel was paid, so they cannot
complete these stricter goals for an unfinished quest.

A package's `tracker` may name `start_pins`: quests pinned for a player whose book is new
(nothing done, nothing pinned), once per install (remembered in the settings). A later package replaces the list.

## Multi-part quests

A task is a list of goals that must all hold, so one quest can ask for a gather, a craft
and a cook together. Quests that were a single count are two-part where the second part is
what the player does next anyway (Oct 13, Mariano: "a bit too simple"): Small Game (kill,
then cook one), Hide and Seek (hides and bones), Rod and Line (craft, then land a fish),
A Fish Supper (catch three, cook three), Meat on the Fire (six cooked, Cooking 2), Sharpening
Up (Attack 4 and five kills), First Harvest and Carrot Patch (harvest, keep the crop), A
Field of Gold (24 harvests, Farming 5), Herb Garden (planter and six plants), Seed Mix (the
mix and the wheat to make it from), Fresh Eggs (six eggs, fry one), A Woolly Flock (tame,
and shears made), The Milk Pail (the pail, then the milk), A Silvery Ore (tin and copper from
one seam), Many Kinds (three kinds, six cooked). The main line of each age (the pacing
test's milestones) stays as it was. Rewards are unchanged.
