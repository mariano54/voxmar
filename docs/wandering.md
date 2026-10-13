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
  try needs: past `first_days`, out of the `gap` (ten minutes) after the last event began
  near them, fewer than `most_near` parties within `near`, fewer than `most_total` on the
  server. The spot is
  a standable cell at `appear_near` to `appear_far` blocks, out of every player's view. The
  kind is drawn by weight from the kinds that suit the biome (name or tag), the hour
  (`daylight`), and the player (`needs`: days, home or not, combat level, and `away`: how far from where the world began,
  so the tutors and the convoys are for those who go exploring); a kind met in the
  last `repeat_memory` seconds weighs `repeat_weight` of itself.
- **Budgets.** People of a party count against `people_most`, not against the settlements'
  villager cap. Animals and monsters of a party count against the usual land caps near
  every player (12 animals, the monster cap), so a party can never crowd the land. A species
  with `habitat: (party_only: true)` is never spawned naturally.
- **Cast and roles.** A cast is groups, each a species, a count range and a role: `Lead`
  (the first group's first member leads everything), `Follow`, `Flank` (alternating sides),
  `Rear`, `Ahead` (scouts), `Herd` (loosely about the leader), `Captive`, `Pet(group)` (keeps to that group's first member), `Free` (its
  own brain, only kept in the party's books), `Chaser` (runs the leader down, in a `Chase`) and
  `Lurk` (lies in wait about the leader, in an `Ambush`).
- **Scripts.** The script decides where the leader goes; every other member gets a formation
  slot from its role. The server hands each member an `Errand`, the same handoff villagers
  use, so the creature brain still decides how to walk it (and still flees, fights or looks
  around when its modules say so).
  - `March(pass, length, pace)`: a line that passes the player at `pass` blocks and walks on.
  - `Patrol(radius, stops, pause, laps)`: a ring of stops, each paused at, then marches off.
  - `Camp(near, stay, mill, away)`: settles near the player, mills about (rests at night),
    leaves.
  - `Shadow(keep, seconds)`: keeps `keep` blocks from the player, stands still when looked at.
  - `Roam(radius, pause, rest, seconds)`: wanders about the place it began, pausing (sitting
    if `rest`), then goes.
  - `Drift(leg, pause, seconds)`: grazes along in short legs of a slowly turning heading;
    the `Herd` role keeps the rest loosely round the leader.
  - `Chase(pass, length, pace, gain, hold)`: the leader is the quarry and runs a line like a
    march at `pace`; its `Chaser`s run at the quarry at `gain` times that. One within 2.5
    blocks has run it down: the stage is `Caught`, everyone stands about it for `hold` seconds
    (a talk, a reward), then they march off together. No chasers left, or the quarry dead:
    the chase is off.
  - `Ambush(near, trigger, delay, patience, away, hold)`: the leader is the bait and settles
    `near` blocks short of the player; the `Lurk`ers lie crouched to its sides. The stage is
    `Waiting`. A player within `trigger` blocks of the bait or any lurker springs it `delay`
    seconds later (time to talk, pay, or run): the lurkers turn on that player (`Sprung`,
    `Avenge`), the bait is afraid. Every lurker gone is `Won`: the bait stays `hold` seconds
    to be thanked. Nobody comes in `patience` seconds, or a `Wary` player stands by (see
    Reactions) for twenty, and they go `away`.
  - `Lure(keep, length, pace, patience, linger)`: the leader stands watching where it appeared
    until a player comes within `keep.1` blocks, then leads off along a line `length` blocks
    long, away from them, hurrying if they come nearer than `keep.0`, waiting (alert) if they
    fall behind, and giving up after `patience` seconds with no follower. At the end the
    event's `reveal` (items, XP, an effect) goes to the nearest player within 18 blocks, with
    the line `arrived`, and the lure goes `linger` seconds later.
  The leader strides in 6-block steps and turns aside where the ground will not do; with no
  headway for six seconds it detours, and after four detours the party gives up and leaves.
- **Stopping.** A party with a `halt` stops for a player who comes within `near` of its
  leader: it stands facing them for a few seconds, a talk keeps it standing, then it goes
  on and does not stop again for `cooldown` seconds. Parties that hunt (bandits, wolves)
  never halt.
- **Meetings.** Right-clicking a speaker with nothing the click wants runs `inspect`, which
  first asks the party. Each speaking group has a meeting. The first line whose needs hold
  (`Skill` at least, `Below` a level, `Home`, `Hurt`, `Holds` an item with a tag, which is
  not taken) and whose `wants` the player holds is said, takes what it wants and gives its
  rewards (items, XP, an effect, healing, `Free`, `Anger`, `Leave`: the party gives up what it
  was about and goes, no grudge). A line can also need `When(stage)` (the party is at that
  stage of its script: `Caught`, `Waiting`, `Sprung`, `Won`, `Leading`, `Arrived`; the stage
  must belong to the event's script) or `Struck(group)` (this player struck a member of that
  group of the cast, and the party did not turn on them for it). A line with only such needs
  and no `wants` is the stage's own greeting. `once` is `Always`, `Player` or `Party`. The speech is the lang key `wander.<package>.<event>.<line>`; the greeting
  (when no line applies) is `.greet`, or with `greets: 3` one of `.greet`, `.greet_2`,
  `.greet_3`, drawn each time; the title is `.title`. Lessons are capped by the player's own
  levels (`Below`): a tutor teaches a beginner, asks more for the next step and has nothing
  for a master, so no lesson skips a step.
- **Being struck.** `hurt` says what a party does when a player strikes one of its own:
  `Scatter` (the party runs and breaks up, and will not talk), `Avenge` (those who can
  fight turn on the player while they stay near, the rest run) or `Ignore` (already hostile).
- **Captives.** A cast may hold one `Captive` that walks with its captors. When every
  captor is gone, or a line gives `Free`, the captive is free: it thanks whoever freed it
  (`thanks` and some coins, as the event says), then, with chance `follow`, follows that
  player, and settles by their home if they have one; otherwise it goes its own way, a
  peaceful person of the land for as long as anyone is near. The same cast can be solved
  by killing the jailers, by a bribe, or by a knife (which the jailers see).
- **Persistence.** A party is a passing thing and is not saved. A freed prisoner who settles
  stays while players are near like any creature of the land; a restart does not keep them.
- **Ending.** An event ends when no player is within `leave`, after its `lifetime`, when its
  march is walked and nobody is within 40 blocks, or when its leader is gone. Its people are
  taken away without a death. Killed members leave the party; the party goes on.
- **Admin.** `/wander` lists the parties about, `/wander list` the kinds, `/wander start
  NAME` begins one ahead of you, `/wander end` sends them all away.

## Reactions

`reactions` in `wandering.ron` are rules every kind inherits: how a *kind* of creature
(`Animal`, `Monster`, `Spirit`, `Villager`) answers a player who holds an item with a *tag*
within `range` blocks, optionally only for a `combat` level range. The response is one of:

- `Wary`: it does not stop, and an ambush does not spring (and gives up after twenty seconds
  of the player standing by).
- `Shy`: an ambling animal walks away from the player, and a lure hangs back (alert, waiting)
  until the tag is put away.
- `Draw`: an ambling animal comes to the player and stands near.

An ambush answers as its lurkers' kind, anything else as its leader's. The tag must be one
some item has (checked at load), and a later package adds reactions, never rewrites one. The
shipped three: a weapon in a hand of combat 14 or more makes monsters in wait let them by; a
`fire` (torch) makes animals and spirits shy; food (`edible`) draws animals. Adding a kind
that wants a new response to something held is a line in this list, not code in the kind.

## The bar every kind meets

Mariano's brief for the whole set, which a new kind is checked against:

- Fresh, not a reskin: each kind has its own cast, script and mechanic, and the same
  framework piece is reused only where the behaviour really is the same.
- Modular: a kind is data (`wandering.ron`, lang keys); a new mechanic is a script, role or
  reward added once and used by many kinds, never a special case for one kind.
- Skills and progression: lessons, trades and rewards are gated on the player's levels
  (`Skill`, `Below`, `Holds`), pay small amounts, and never skip an age's steps.
- Exploration: the rare kinds need `away` from where the world began; biome, hour, home and
  combat level decide what comes.
- Rare: a gap of ten minutes between parties near a player, one at a time, weights below 1
  for the interesting ones, and a lesser chance for one met lately.
- They stop for you: peaceful parties halt when a player comes near and stay for a talk.
- What a player might do is thought through for each kind: watch, approach, wait, talk,
  trade, give a gift, follow, strike, steal a march, bring a knife or coins, flee; the
  table below says what each does.

| the player | peaceful party | prison convoy | bandits, wolves |
| --- | --- | --- | --- |
| walks past | halts if close, then goes on | halts a few seconds, watches | hunts if within sight |
| talks, empty-handed | a greeting, one of several | a warning | n/a |
| offers what they want | trades, or teaches by level | three coins buy the prisoner | n/a |
| strikes one | scatters and will not talk | jailers turn on them, prisoner runs | fights as it did |
| kills the captors | n/a | prisoner thanks, follows or goes alone | n/a |
| cuts the ropes (knife) | n/a | prisoner free, jailers angry | n/a |
| flees | they go on | they stop the grudge when far | give up past the leash |

## Batch 1 (implemented, ten kinds)

| # | kind | script | what it adds |
| --- | --- | --- | --- |
| 1 | mage tutors | March | A wandering mage and apprentices, far from towns. Motes of aether buy lessons by level: one or two for a beginner, three past level 15, nothing for a master. Magic XP and a Focused effect, once per player. |
| 2 | pedlar and hound | March | A pedlar with a pack hound. Swaps what the player holds (hide, bone, feather) for twine, seed mix or bread. |
| 3 | pilgrims | March | A priest with pilgrims and a child. Mends wounds, and gives a blessing once. |
| 4 | stroll with a dog | March | A villager and their dog; a bone from the player's hand earns twine. The dog circles its person. |
| 5 | bandit patrol | Patrol | Raiders walk a ring of stops, two laps, and hunt anyone within sight. Only for players with a home and combat level 3 to 40, after day 1.5. |
| 6 | wolf prowl | Patrol | A pack circles a hunting ground at night in forests and cold land. |
| 7 | hooded stranger | Shadow | A shy figure that keeps 12 to 20 blocks off and freezes when looked at. Gives a torch, once, to whoever reaches it. |
| 8 | tinker camp | Camp | A trader, villagers and a hound settle near the player for a while. Three swaps. |
| 9 | bard band | March | Musicians pass by; a song refreshes the player. |
| 10 | prison convoy | March | Two or three jailers march a prisoner. They leave you be until struck. Kill them, bribe them (three gold coins), or cut the ropes with a knife and the jailers turn on you. The freed prisoner follows you half the time and settles by your home, or goes off alone. |

## Batch 2 (implemented, four kinds)

| kind | script | what it adds |
| --- | --- | --- |
| shepherd and flock | Drift | A shepherd grazes a flock of mouflon along, with a dog at heel. Stand still and the flock comes round you; any food teaches a beginner Farming once; two loaves buy wool. Tame one of the flock with the usual food and it is yours, and leaves the flock. |
| lost child | Roam, then Follow | A child who followed a butterfly. Feed them and they follow you to the nearest settlement, where the family pays in coins and bread. Strike them and they run, lost again. |
| stray hound | Roam | A hound with no one. Feed it meat with Hunter 2 and enough patience and it is yours (the usual taming), and leaves its party. |
| wounded hunter | Roam (sitting) | Share food and they teach Hunter (to level 15) and give flints; nothing for the skilled. |

New in the framework: the `Roam` and `Drift` scripts, the `Herd` role, a line can `wants`
any item with a tag (`"tag:edible"`), the `Befriend` reward with an event `guide` (follow the
player for so long, to `Home` or `Town`, thank them with rewards, stay), and a member a
player has taken in (owner set) leaves its party. These are general rules in data: taming,
feeding and trading already in the game are what the player combines with a wandering
kind, and the kind needs no code of its own.

## Batch 3 (implemented, five kinds, three scripts)

| kind | script | what it adds |
| --- | --- | --- |
| thief on the run | Chase | A cutpurse runs, the town watch (one or two constables) on his heels. They run him down in well under a minute and hold him a while. A blow on him as he passes earns the constables' bounty (coins, Attack XP) once; kill him and his purse drops; strike the watch and only the one struck fights. |
| fox and farmer | Chase | A fox with feathers on its whiskers, a farmer after it. A blow that turns the fox earns eggs and Farming XP. |
| wounded traveller | Ambush | A traveller sits hurt by the road, raiders lie in the rocks beside him. Going to him springs it (after six to ten seconds): fight, or slip a lurker three coins in time and they go. A combat 14 fighter with a weapon in hand is let by. Win and the traveller thanks you once. Only for players with a home and combat 4 to 60, far from the start. |
| white stag | Lure | A white stag watches from the wood's edge, then leads off if you come near. Follow it to its glade for an antler, Hunter XP and a Refreshed effect. Strike it and it bolts; it waits for stragglers and gives you up if you do not come. |
| wisp trail | Lure | A pale light over the marsh by night, shy of torches: hold one up and it hangs back until you put it away. At the end of its path, aether motes and Magic XP. |

| the player | chase | ambush | lure |
| --- | --- | --- | --- |
| watches | the watch catch the quarry and march it off | nothing: it waits until `patience` | nothing: it gives you up |
| walks into it | may block the quarry's way and slow it | springs after a moment; fight, flee or pay | it leads on |
| talks | the held quarry pleads, the watch speak | a lurker takes three coins and goes; the bait thanks the winner | n/a (no speaker) |
| strikes | the quarry: the watch pay you; the watch: that one fights | a lurker or the bait: it springs at once | it bolts, scattered |
| holds a weapon | nothing | a seasoned fighter (combat 14) is let by | nothing |
| holds a torch | nothing | nothing | a wisp hangs back (animals and spirits are shy of flame) |
| holds food | nothing | nothing | animals ambling about come to it |
| kills | the quarry's purse drops | the lurkers: the bait thanks you | only a deer |

New in the framework: the `Chase`, `Ambush` and `Lure` scripts, the `Chaser` and `Lurk`
roles, line stages (`When`) and `Struck`, the `Leave` reward, an event's `reveal`, and
Reactions. The settled-companions save, the next batch, comes with a save change of its own.

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
| 10 | prison convoy | see batch 1 | Captive | 2 | done |
| 11 | lost child | see batch 2 | Roam, Guide | 1 | done |
| 12 | shepherd with flock | see batch 2 | Drift, Herd | 2 | done |
| 13 | cowherd and cows | milk for a bucket held out | Herd | 2 | next |
| 14 | goose girl | geese on a march to water | Herd | 2 | next |
| 15 | berry pickers | villagers gather and drop forage in a basket; trade a share | Forage | 1 | next |
| 16 | fishers' file | villagers with rods walk to a shore and fish | Gather | 1 | next |
| 17 | woodcutters | two villagers fell a lone tree and haul the logs | Haul | 2 | next |
| 18 | charcoal burners | a small camp with a smoking mound | Camp, Haul | 2 | next |
| 19 | water carriers | villagers with yokes go from well to field | Haul | 2 | next |
| 20 | harvest cart | a cart-pushing family in the farm age; swaps seeds | Haul | 2 | next |
| 21 | herb woman | a forager who names a herb patch nearby for a coin | Forage | 1 | next |
| 22 | wandering healer | mends and sells a salve | | 1 | next |
| 23 | hermit | lives off the path; a riddle gives a recipe | Vigil | 1 | next |
| 24 | travelling smith | repairs held tools for coin | Camp | 2 | next |
| 25 | travelling cook | a pot on the fire; a portion for an ingredient | Camp | 1 | next |
| 26 | story teller | camps at dusk; players near get a journal page | Camp | 1 | next |
| 27 | stray hound | see batch 2 | Roam | 1 | done |
| 28 | deer herd | drifts through; flees when seen | Herd | 1 | next |
| 29 | boar sounder | sow with piglets; charges what comes close | Herd | 1 | next |
| 30 | fox and farmer | see batch 3 (a fox runs, a farmer chases) | Chase | 2 | done |
| 31 | thief and constable | see batch 3 | Chase | 2 | done |
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
| 65 | ghost lantern | a lantern drifting over a marsh; follow it to a chest (the wisp trail's cousin: a Lure whose reveal is a chest) | Lure | 2 | later |
| 66 | wisp trail | see batch 3 (a light leads off the path, shy of torches) | Lure | 3 | done |
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
| 95 | wounded hunter | see batch 2 (a hunter hurt by a boar) | Roam | 3 | done |
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
- **Chase**: done (batch 3): one cast member is the quarry, another the pursuer; stopping the quarry pays.
- **Ambush**: done (batch 3): lies in wait until a player is within a distance, then rushes.
- **Lure**: done (batch 3): leads the player on if followed; the end is a reward. A trap end is a
  later `Lure` whose lurkers spring at the end.
- **Duel / Dance / Crossing / Vigil**: fixed arrangements of the formation code.

## Notes

- Nothing here is added to the world: parties exist only near players and are taken away
  when none is near, so a save holds none and a restart loses nothing.
- Everything seeded: the same seed and tick pick the same party, pass distance and stops.
- Players cannot be pushed around by any of it (no knockback).
- Every spoken line is a lang key in English and Japanese, and every number is data.
