# Wandering events

Oct 13, 2026. Mariano's brief: random villager groups walking (some with mage tutors),
villagers with pets, patrolling mobs, and a hundred kinds of wandering event, each with its
own AI and mechanics, to keep the world interesting.

An event is a small party (people, animals, monsters) that appears at the edge of a
player's view, does one thing with its own AI, can be talked to or fought, and goes. It is
all data: a mod adds a kind in its own `wandering.ron`, and a later package replaces the
rule numbers field by field (`Setting`, design draft 91). The engine half is
`crates/engine/src/wander.rs`, the server half `crates/server/src/wandering.rs`.

## How it runs

- **Spawning.** Every `scan_every` seconds each joined player gets one try at `chance`. A
  try needs: past `first_days`, out of the `gap` after the last event began near them, fewer
  than `most_near` parties within `near`, fewer than `most_total` on the server. The spot is
  a standable cell at `appear_near` to `appear_far` blocks, out of every player's view. The
  kind is drawn by weight from the kinds that suit the biome (name or tag), the hour
  (`daylight`), and the player (`needs`: days, home or not, combat level); a kind met in the
  last `repeat_memory` seconds weighs `repeat_weight` of itself.
- **Budgets.** People of a party count against `people_most`, not against the settlements'
  villager cap. Animals and monsters of a party count against the usual land caps near
  every player (12 animals, the monster cap), so a party can never crowd the land. A species
  with `habitat: (party_only: true)` is never spawned naturally.
- **Cast and roles.** A cast is groups, each a species, a count range and a role: `Lead`
  (the first group's first member leads everything), `Follow`, `Flank` (alternating sides),
  `Rear`, `Ahead` (scouts), `Pet(group)` (keeps to that group's first member), `Free` (its
  own brain, only kept in the party's books).
- **Scripts.** The script decides where the leader goes; every other member gets a formation
  slot from its role. The server hands each member an `Errand`, the same handoff villagers
  use, so the creature brain still decides how to walk it (and still flees, fights or looks
  around when its modules say so).
  - `March(pass, length, pace)`: a line that passes the player at `pass` blocks and walks on.
  - `Patrol(radius, stops, pause, laps)`: a ring of stops, each paused at, then marches off.
  - `Camp(near, stay, mill, away)`: settles near the player, mills about (rests at night),
    leaves.
  - `Shadow(keep, seconds)`: keeps `keep` blocks from the player, stands still when looked at.
  The leader strides in 6-block steps and turns aside where the ground will not do; with no
  headway for six seconds it detours, and after four detours the party gives up and leaves.
- **Meetings.** Right-clicking the speaker with nothing the click wants runs `inspect`, which
  first asks the party. The first line whose needs hold (skill level, home, hurt) and whose
  `wants` the player holds is said, takes what it wants and gives its rewards (items, XP,
  an effect, healing). `once` is `Always`, `Player` or `Party`. The speech is the lang key
  `wander.<package>.<event>.<line>`; the greeting is `.greet`; the title `.title`. A line
  that gives XP follows the XP rules already in force (a tutor's lesson trains Magic but
  never pays more than doing it by hand).
- **Ending.** An event ends when no player is within `leave`, after its `lifetime`, when its
  march is walked and nobody is within 40 blocks, or when its leader is gone. Its people are
  taken away without a death. Killed members leave the party; the party goes on.
- **Admin.** `/wander` lists the parties about, `/wander list` the kinds, `/wander start
  NAME` begins one ahead of you, `/wander end` sends them all away.

## Batch 1 (implemented)

| # | kind | script | what it adds |
| --- | --- | --- | --- |
| 1 | mage tutors | March | A wandering mage and apprentices. A mote of aether buys a lesson: Magic XP and a Focused effect. Once per player. |
| 2 | pedlar and hound | March | A pedlar with a pack hound. Swaps what the player holds (hide, bone, feather) for twine, seed mix or bread. |
| 3 | pilgrims | March | A priest with pilgrims and a child. Mends wounds, and gives a blessing once. |
| 4 | stroll with a dog | March | A villager and their dog; a bone from the player's hand earns twine. The dog circles its person. |
| 5 | bandit patrol | Patrol | Raiders walk a ring of stops, two laps, and hunt anyone within sight. Only for players with a home and combat level 3 to 40, after day 1.5. |
| 6 | wolf prowl | Patrol | A pack circles a hunting ground at night in forests and cold land. |
| 7 | hooded stranger | Shadow | A shy figure that keeps 12 to 20 blocks off and freezes when looked at. Gives a torch, once, to whoever reaches it. |
| 8 | tinker camp | Camp | A trader, villagers and a hound settle near the player for a while. Three swaps. |
| 9 | bard band | March | Musicians pass by; a song refreshes the player. |

## Catalogue of 100

Status: `done` is in batch 1, `next` is the second batch (ages 1 and 2: Nomad and Farm
first), `later` the rest. "New" names what the kind needs that the framework lacks; those
are the mechanics later batches add, each used by several kinds.

| # | kind | what happens | new | age | status |
| --- | --- | --- | --- | --- | --- |
| 1 | mage tutors | see batch 1 | | 1 | done |
| 2 | pedlar and hound | see batch 1 | | 1 | done |
| 3 | pilgrims | see batch 1 | | 1 | done |
| 4 | stroll with a dog | see batch 1 | | 1 | done |
| 5 | bandit patrol | see batch 1 | | 2 | done |
| 6 | wolf prowl | see batch 1 | | 1 | done |
| 7 | hooded stranger | see batch 1 | | 1 | done |
| 8 | tinker camp | see batch 1 | | 1 | done |
| 9 | bard band | see batch 1 | | 1 | done |
| 10 | lost child | a child wanders crying; leading it to a settlement pays a coin and standing | Escort | 1 | next |
| 11 | shepherd with flock | a villager and sheep drifting on pasture; lamb for wool | Herd | 2 | next |
| 12 | cowherd and cows | milk for a bucket held out | Herd | 2 | next |
| 13 | goose girl | geese on a march to water | Herd | 2 | next |
| 14 | berry pickers | villagers gather and drop forage in a basket; trade a share | Forage | 1 | next |
| 15 | fishers' file | villagers with rods walk to a shore and fish | Gather | 1 | next |
| 16 | woodcutters | two villagers fell a lone tree and haul the logs | Haul | 2 | next |
| 17 | charcoal burners | a small camp with a smoking mound | Camp, Haul | 2 | next |
| 18 | water carriers | villagers with yokes go from well to field | Haul | 2 | next |
| 19 | harvest cart | a cart-pushing family in the farm age; swaps seeds | Haul | 2 | next |
| 20 | herb woman | a forager who names a herb patch nearby for a coin | Forage | 1 | next |
| 21 | wandering healer | mends and sells a salve | | 1 | next |
| 22 | hermit | lives off the path; a riddle gives a recipe | Vigil | 1 | next |
| 23 | travelling smith | repairs held tools for coin | Camp | 2 | next |
| 24 | travelling cook | a pot on the fire; a portion for an ingredient | Camp | 1 | next |
| 25 | story teller | camps at dusk; players near get a journal page | Camp | 1 | next |
| 26 | stray hound | a lone dog that follows the player until fed, then adopts | Lure | 1 | next |
| 27 | deer herd | drifts through; flees when seen | Herd | 1 | next |
| 28 | boar sounder | sow with piglets; charges what comes close | Herd | 1 | next |
| 29 | fox and hens | a fox stalks a hen that runs | Chase | 2 | next |
| 30 | geese in a vee | pass overhead and call | Ambient | 1 | later |
| 31 | thief and constable | a thief runs from a guard; stop the thief for the loot | Chase | 2 | later |
| 32 | poacher | hunts deer; reporting them to a guard pays | Hunt | 2 | later |
| 33 | hunting party | villagers with hounds track a boar | Hunt | 2 | later |
| 34 | scarecrow walker | a straw figure that stands among crops and moves when unseen | Shadow | 2 | later |
| 35 | night watchman | walks a village ring with a lantern | Patrol | 2 | later |
| 36 | village guard patrol | guards walk a loop between settlements | Patrol | 2 | later |
| 37 | tax collector | asks coin from a player with a home; refusing makes guards hostile for a day | Meeting | 2 | later |
| 38 | wedding procession | villagers walk to the shrine with flowers; a gift is returned in kind | March | 2 | later |
| 39 | funeral procession | a quiet march to a grave | March | 2 | later |
| 40 | festival drummers | drums and a dance ring; a refreshed effect | Dance | 2 | later |
| 41 | dance circle | villagers circle a bonfire at night | Dance | 2 | later |
| 42 | caravan | pack animals, guards and a merchant on a long road | March | 3 | later |
| 43 | refugees | a worn party fleeing a raid; food earns a hearth | Escort | 2 | later |
| 44 | soldiers on the march | two columns that do not stop for a player | March | 3 | later |
| 45 | knight errant | duels for a coin; wins a favour | Duel | 3 | later |
| 46 | wandering monk | a vow of silence; sells a blessing for a mote | Meeting | 2 | later |
| 47 | alchemist's mule | a laden mule; a reagent swap | March | 2 | later |
| 48 | rat catcher | a piper followed by rats | Lure | 2 | later |
| 49 | gypsy wagon | a wagon camps; fortunes told (a journal hint) | Camp | 2 | later |
| 50 | puppeteers | a cart that opens into a stage | Camp | 2 | later |
| 51 | circus | acrobats, a bear on a chain; scares animals | Camp | 3 | later |
| 52 | messenger | runs between settlements; a letter for a coin | Errand | 2 | later |
| 53 | postman with mule | delivers to a player's home mailbox | Errand | 3 | later |
| 54 | cartographer | sells a sketched map of nearby ruins | Meeting | 2 | later |
| 55 | prospector | with a pick and a donkey; points at a vein | Forage | 2 | later |
| 56 | miners' shift | a row of miners with lamps walk to a mine mouth | March | 3 | later |
| 57 | stonecutters | haul a block; leave a cut stone | Haul | 3 | later |
| 58 | lamplighter | at dusk lights lamps along a road | Errand | 3 | later |
| 59 | beekeeper | smoke and bees; honey for a comb | Camp | 2 | later |
| 60 | forager of mushrooms | follows a pig that finds truffles | Forage | 2 | later |
| 61 | goblin raiders | a small band that rushes a farm at dusk | Ambush | 2 | later |
| 62 | goblin scouts | a pair that slips away if seen, then returns with more | Chase | 2 | later |
| 63 | skeleton march | at night, a file of bones along a road | March | 3 | later |
| 64 | wight walk | a single wight on a grave road | Patrol | 3 | later |
| 65 | ghost lantern | a lantern drifting over a marsh; follow it to a chest | Lure | 2 | later |
| 66 | wisp trail | will-o-wisps lead off the path | Lure | 3 | later |
| 67 | troll bridge | a troll under a bridge asks a toll | Meeting | 3 | later |
| 68 | ogre and goats | an ogre drives goats | Herd | 4 | later |
| 69 | giant on the hill | a slow giant strides over a ridge; the ground shakes | March | 5 | later |
| 70 | dragon shadow | a wing's shadow passes; livestock panics | Ambient | 5 | later |
| 71 | werewolf night | a villager turns at moonrise | Chase | 3 | later |
| 72 | vampire carriage | a black coach that avoids day | March | 4 | later |
| 73 | witch and cats | a witch with a cat that sells a hex | Camp | 3 | later |
| 74 | hedge witch circle | herbs for a mote of aether | Camp | 3 | later |
| 75 | necromancer's thralls | skeleton servants fetch a body from a grave | Haul | 4 | later |
| 76 | cultists' procession | masked, singing, to an altar | March | 4 | later |
| 77 | demon summoners | a ring, a flash, one angry guest | Ambush | 5 | later |
| 78 | fae court | small folk who bargain; never hurt them | Meeting | 4 | later |
| 79 | pixie dust | pixies sprinkle a crop for a growth boost | Ambient | 3 | later |
| 80 | treant march | trees walk at dusk, slowly | March | 4 | later |
| 81 | lost mage | a mage whose spell went wrong; fetch three motes | Meeting | 3 | later |
| 82 | mage duel | two mages cast at each other and ignore you | Duel | 4 | later |
| 83 | archmage and pupils | a tutor with five pupils; a tougher lesson | March | 4 | later |
| 84 | star gazers | at night, with a brass tube; shows the sky for a bonus | Camp | 3 | later |
| 85 | rainmakers | a dance before a storm that makes it rain | Dance | 3 | later |
| 86 | storm chasers | follow lightning and gather a charged rock | Chase | 4 | later |
| 87 | frost caravan | in the cold biomes; furs to swap | March | 3 | later |
| 88 | desert nomads | camels, water for a coin | March | 3 | later |
| 89 | sand worm tracks | a ridge of sand rolling by | Ambient | 5 | later |
| 90 | swamp ferryman | a punt on a creek; fares | Crossing | 3 | later |
| 91 | river ferry | a raft for a crossing | Crossing | 3 | later |
| 92 | shipwreck survivors | on a shore; bring food for a map | Escort | 3 | later |
| 93 | pirate landing | a boat, a dozen, one keg | Ambush | 4 | later |
| 94 | sea monster wake | a long shape in the water; a boat is chased | Chase | 5 | later |
| 95 | mine canary | a bird that flies from bad air | Ambient | 3 | later |
| 96 | mole people | emerge at dusk; carry a gem | Haul | 4 | later |
| 97 | golem walk | a stone golem on an old road | Patrol | 4 | later |
| 98 | ancient automaton | wakes at a ruin, rolls a ring, goes still | Patrol | 5 | later |
| 99 | angel passing | a light; heals the hurt | Ambient | 5 | later |
| 100 | wandering god | a veiled figure; a single wish for a price | Meeting | 5 | later |

## Mechanics the next batches add

Each is a script or a role used by many kinds, so every kind stays a few lines of data.

- **Herd**: members drift together in a loose circle with a shepherd; they flee as one.
- **Forage / Gather**: members stop at items or blocks of a tag, work a few seconds, drop
  what they collected for a trade.
- **Haul**: two members carry a load along a line; kill one and it drops the load.
- **Escort**: the party follows the player toward a settlement; arriving pays out, losing
  them (distance, death) ends it.
- **Chase**: one cast member is the quarry, another the pursuer; stopping the quarry pays.
- **Ambush**: hides out of sight until a player is within a distance, then rushes.
- **Lure**: leads the player on if followed; the end is a reward or a trap.
- **Duel / Dance / Crossing / Vigil**: fixed arrangements of the formation code.

## Notes

- Nothing here is added to the world: parties exist only near players and are taken away
  when none is near, so a save holds none and a restart loses nothing.
- Everything seeded: the same seed and tick pick the same party, pass distance and stops.
- Players cannot be pushed around by any of it (no knockback).
- Every spoken line is a lang key in English and Japanese, and every number is data.
