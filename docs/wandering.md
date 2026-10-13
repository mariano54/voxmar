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
  every player (12 animals, the monster cap), so a party can never crowd the land. A chance
  start checks room for the whole cast first, and every member needs a place to stand and
  must appear unseen: the whole party comes or none of it, never a pedlar without his hound.
  An admin's `/wander NAME` brings the whole cast whatever the budgets. A species with
  `habitat: (party_only: true)` is never spawned naturally.
- **Cast and roles.** A cast is groups, each a species, a count range and a role: `Lead`
  (the first group's first member leads everything), `Follow`, `Flank` (alternating sides),
  `Rear`, `Ahead` (scouts), `Herd` (loosely about the leader), `Captive`, `Pet(group)` (keeps to that group's first member), `Free` (its
  own brain, only kept in the party's books), `Chaser` (runs the leader down, in a `Chase`) and
  `Lurk` (lies in wait about the leader, in an `Ambush`).
- **Scripts.** The script decides where the leader goes; every other member gets a formation
  slot from its role. The server hands each member an `Errand`, the same handoff villagers
  use, so the creature brain still decides how to walk it (and still fights or looks around
  when its modules say so). Until a player turns the party, its errand is `steady`: the
  brain's own `Flee` waits while it holds one, so a member runs only when its script says
  (a chase, a lure hurrying on), and a wild stag or a hurt traveller does not bolt from the
  player the script is meant to meet. Once a player turns it (a scatter, a sprung ambush)
  the errand is no longer steady and each member's fear may run it; a freed or released
  member has no errand at all, and its brain is wholly its own.
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
    Reactions) for twenty, and they march `away` blocks off (a paid toll or a won fight the same).
  - `Lure(keep, length, pace, patience, linger)`: the leader stands watching where it appeared
    until a player comes within `keep.1` blocks, then leads off along a line `length` blocks
    long, away from them, hurrying if they come nearer than `keep.0`, waiting (alert) if they
    fall behind, and giving up after `patience` seconds with no follower. At the end the
    event's `reveal` (items, XP, an effect) goes to the nearest player within 18 blocks, with
    the line `arrived`, and the lure goes `linger` seconds later.
  - `Gather(reach, work, visits)`: everyone who walks works. Each goes to the nearest block
    the event `seek`s (block names or `tag:`s) within `reach` of where the party began, one
    block each, stands at it crouched for `work` seconds, then takes the next; after
    `visits` works between them, or with nothing left, they go. A block that is gone (the
    player picked it) sends its worker to another; one not reached in time is passed over.
    A gathering kind comes only where something it seeks is within `reach` of the spot.
    With `carry: Some((set, tend))`, each worker then takes what it worked to the nearest
    block the event `deliver`s to (or, delivering nowhere, back to where the party began,
    its pile), sets it down for `set` seconds, and only then counts the work; with `tend:
    Some((near, stages))`, planted crops within `near` blocks of where it is set down grow
    on `stages` (water poured on a field). A kind that delivers comes only where something
    it delivers to is within `reach` as well.
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
  rewards. A line may `asks` instead of `wants`: a list of items with counts, of which each
  party asks for one, picked by its seed; until the player holds it, a speaker whose line
  otherwise applies says what it asks (`NAME_ask`, with `{thing}` and `{count}`) in place
  of a greeting. The rewards are (items, XP, an effect, healing, `Free`, `Anger`, `Leave`: the party gives up what it
  was about and goes, no grudge; `Point(selector, reach)`: tells the player which way, and
  about how far, the nearest block of that name or `tag:` lies within `reach` of the speaker,
  true to the land (a line that points is said only when something of the kind is near, so
  a once-only line is not spent on nothing); `Hand(group)`: hands one member of that group to
  the player as if tamed (a calf sold), said only when there is one and the player has room
  for another kept animal; `Town(reach)`: names the nearest settlement of the world's plan
  within `reach` blocks of the speaker, which way and about how far (as `/locate
  settlement` finds it; said only when there is one); `Cook(station, most, keep)`: cooks what the line takes by the
  game's own recipes at that station (cooking.ron), never burnt, up to `most` of what is in
  hand, keeping `keep` for the speaker's pot, said only with something it cooks in hand). A
  line can also need `When(stage)` (the party is at that
  stage of its script: `Caught`, `Waiting`, `Sprung`, `Won`, `Leading`, `Arrived`; the stage
  must belong to the event's script) or `Struck(group)` (this player struck a member of that
  group of the cast, and the party did not turn on them for it). A line with only such needs
  and no `wants` is the stage's own greeting. `once` is `Always`, `Player` or `Party`. The speech is the lang key `wander.<package>.<event>.<line>`; the greeting
  (when no line applies) is `.greet`, or with `greets: 3` one of `.greet`, `.greet_2`,
  `.greet_3`, drawn each time; the title is `.title`. Lessons are capped by the player's own
  levels (`Below`): a tutor teaches a beginner, asks more for the next step and has nothing
  for a master, so no lesson skips a step.
- **Guarding.** A party with a `guard(near, warn, calm)` (a sow with her litter) warns a
  player who comes within `near` of any member: they stand and face them, and the event's
  `warn` line is told. One who stays `warn` seconds, or comes within half of `near`, is set
  upon by those that fight while the rest run (as `Avenge`); one who backs off past `calm`
  is let be. A player holding what the party's kind is shy of or drawn to (a `Shy` or `Draw`
  reaction: a torch, food) is not warned at all.
- **Startling.** A party with a `startle(near, least, still, still_for, stalk)` (a herd of
  deer) bolts from a player who comes within `near` blocks of any member, less `stalk.1`
  blocks a level of skill `stalk.0` (Hunter: a stalker comes nearer), no nearer than
  `least`. A player who has not moved for `still_for` seconds is noticed only at `still`, and
  a still one holding what draws the kind (food) not at all. Bolted, it runs off and is done.
  A party that startles does not halt or guard.
- **Shows.** A `Camp` party with a `show(near, seconds, gives)` puts one on while it stays at
  its spot: a player who stands within `near` blocks of its leader for `seconds` together
  is given `gives` (items, XP, effects, healing), once each, with the event's `show`
  line. One who walks off starts the count again; a party struck or startled shows nothing.
- **Young.** A group with `young: true` comes young (piglets, a calf, fawns) and grows up as
  kept young do.
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
  by a player's home becomes a companion: saved in `companions.ron` (species, seed, place,
  health), kept out of the settlements' people budget, never despawned for distance, and put
  back at the next start. At most `companions_most` stay; later ones go their own way, and
  so does anyone let go anywhere else, who is a creature of the land like any other.
- **Ending.** An event ends when no player is within `leave`, after its `lifetime`, or when its
  march is walked and nobody is within 40 blocks. Its people are taken away without a death.
  Killed members leave the party and the party goes on: when the leader is killed, the next
  member who can lead (not a pet, a flock, a captive or a `Free` one) takes over, and when
  nobody who can lead is left (a hound whose pedlar was killed, a shepherd's flock) the rest
  are let go as creatures of the land. Party members are never despawned for distance like
  wild creatures; the event takes them away. A freed captive, or a befriended one, follows
  on a clock of its own from the moment it is freed or befriended.
- **Speech.** A speaker answers a right click only within `speak_reach`. A line that frees a
  captive (`Free`) is offered only while the party still holds one, so a bribe is never taken
  for nobody. Short skill names in lines mean the event's own package's skill first, then
  core's. Everyone who came with a party, freed or let go, stays outside the settlements'
  people budget.
- **Admin.** `/wander` lists the parties about, `/wander list` the kinds, `/wander start
  NAME` begins one ahead of you (nearer, down to your feet, where there is no standing room
  ahead), `/wander end` sends them all away, and `/wander hold` keeps any from setting out
  by chance until `/wander allow` (the gate holds them through its session).

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

## Batch 4 (implemented, seven kinds): the early land

The Nomad and Farm ages: folk at work a new player can walk up to with bare hands, and wild
families that answer how they are approached. Every kind is data; the new pieces (the
`Gather` script, `seek`, `guard`, `startle`, `young`, and the `Point`, `Hand` and `Cook`
rewards) are each used by several kinds and the ones to come.

| kind | script | what it adds |
| --- | --- | --- |
| berry pickers | Gather | Villagers and a child pick a patch of berry bushes, one bush each. Pick the bush one is at and they move on. They point out where mushrooms grow (true to the land, once), and eight berries buy a loaf. Only where berry bushes are. |
| herb gatherer | Gather | An elder works the herbs of a meadow. Show a herb and a beginner learns Alchemy (once); Alchemy 5 is pointed to roots, Alchemy 20 to the glowing wonders, if any grow near. |
| fisher folk | Gather | Fishers stand crouched at the water's edge. Three sticks from a beginner (Fishing below 5) buy twine enough for a rod; a rod held out gets a lesson (below 12); four fish buy line; the hurt are given a fish. Only by water. |
| boar sounder | Drift, Herd, guard | A wild sow with two to four young piglets. Come within nine blocks and she grunts a warning; stay three seconds, or come within half that, and she charges while the piglets run. Hold up a torch and they go; hold out food and she lets you near (a piglet can then be tamed the usual way). |
| deer herd | Drift, Herd, startle | A stag, hinds and a fawn or two. They bolt from a walker at 22 blocks, a still player at 6, and a hunter's step nearer (0.35 a Hunter level, to 8). Stand still holding food and they come to it; move and they are gone. |
| travelling cook | Camp | A cook camps at dusk with a pot on. Raw meat or fish handed over comes back cooked, one kept for the pot; cooked food shown teaches a beginner (Cooking below 10); the hurt are fed. |
| cowherd | Drift, Herd, Hand | A herder drives aurochs and a calf with a dog at heel, to players with a home. A pail held out is filled (once); to a player with the Hunter level that taming an aurochs takes (12), four coins buy the calf, kept from then on. |

| the player | gatherers | sow and litter | deer herd |
| --- | --- | --- | --- |
| walks past | they halt if close, then work on | warned at 9 blocks | the herd bolts |
| stands still | they work on | warned, then charged if close | they graze on round you |
| picks what they work | the worker moves to the next | n/a | n/a |
| holds a torch | nothing | the family goes off | they keep away |
| holds food | trades and lessons as above | the sow comes to it, no warning | come to a still hand |
| strikes one | scatter and will not talk | the sow fights, the piglets run | scatter |
| is a skilled hunter | nothing | nothing | comes nearer unseen |

## Batch 5 (implemented, six kinds): carrying and the crop ladder

More of the Nomad and Farm ages: folk who carry their work somewhere, and traders whose
goods follow a player's farming level. The new piece is `carry` on `Gather` (with
`deliver` and `tend`), used by two kinds here and by the haulers to come; a `Point` line
now waits until there is something to point to.

| kind | script | what it adds |
| --- | --- | --- |
| woodcutters | Gather, carry | Woodcutters work the trees about a clearing, one each, and carry every load back to their pile. Eight logs buy four coins, once; an axe in a beginner's hand gets a word on felling (Construction below 10); they say where fruit trees grow, if any are near. |
| water carriers | Gather, carry, tend | Farmers fill pails at the water and carry them to the nearest tilled field, to players with a home: the crops within a few blocks of where each pail is poured grow on a stage. Only where water and a field lie together. A beginner gets a farming tip; the hurt a drink. |
| harvest family | March | A farming family with what they grew. Each crop buys the next a farmer can sow, once each and only at the level to sow it: six wheat buy rye seed at Farming 3, six rye buy carrots at 5, six carrots buy chickpeas at 7. A beginner with seed in hand learns to sow. |
| charcoal burners | Camp | Burners camp by their mound, day or night. A dozen logs buy two charcoal, once; they say where coal shows at the surface (an ore sample) if any is near. |
| wandering healer | March | An elder who mends the hurt once, and makes a salve of three herbs: Soothed, and a little Alchemy. |
| story teller | Camp | Camps at dusk with listeners. The tale is of something strange and near, true to the land, once, and a listener leaves Focused. |

| the player | woodcutters | water carriers | the traders |
| --- | --- | --- | --- |
| walks past | they halt if close, then work on | the same | they halt if close |
| picks or cuts what they work | the worker moves to the next | n/a | n/a |
| has a field near water | nothing | they water it: it grows | nothing |
| holds what they want | logs buy coins, once | n/a | crops buy the next crop, by level |
| strikes one | scatter and will not talk | the same | the same |

## Batch 6 (implemented, six kinds): asks, and the rites of the road

The farm age's people: a hermit and a travelling alchemist who each want something
different, a goose girl, a prospector, and the processions of village life. The new piece
is `asks` on a line, so two meetings with one kind can want different things and say so.

| kind | script | what it adds |
| --- | --- | --- |
| hermit | Camp | Camps far from home (80 blocks or more). Each asks for its own thing (three feathers, two bones, two flints, two eggs, two mint, two nettles) and says so; bring it and the hermit points to a wonder near and teaches a little Magic, once. Only where a wonder is near. |
| goose girl | Drift, Herd, Hand | A child drives ducks and a duckling to water. Four wheat seeds earn feathers, once; at Hunter 3 (taming a duck), two coins buy the duckling. |
| prospector | Gather | Works the ore that shows at the surface. A pickaxe in a beginner's hand (Mining below 10) gets a lesson; at Mining 5 they point to more surface ore, if any is near. Only where surface ore is. |
| travelling alchemist | March | Each wants two of one herb (lavender, chamomile, yarrow, sage, thyme or sorrel) and says which; bring it for Alchemy and Focused, once. Another herb is not taken. |
| wedding | March | A priest, the couple and guests walk to the shrine. A flower given returns bread and Refreshed, once. |
| funeral | March | A slow procession that does not stop for a player. A flower laid with them is a little Magic, once. |

## Batch 7 (implemented, eight kinds): shows, towns, and folk of the road

More of the Nomad and Farm ages. The new pieces are `show` on a `Camp` (stand and watch,
no talking needed) and the `Town` reward (the way to the nearest settlement), and a new
species, the scarecrow.

| kind | script | what it adds |
| --- | --- | --- |
| travelling smith | Camp | A smith camps a while. A coin and every edge you carry is whetted: Keen edge, a tenth faster at every tool for twenty minutes, once. A tool in a beginner's hand (Crafting below 10) gets a word on its care. |
| truffle hunter | Gather | Works the mushrooms of a wood with a boar on a rope. Show a mushroom to a beginner cook (Cooking below 10) and learn which are good to eat; a coin buys where a truffle grows, said only when one is near. |
| wandering monk | March | A monk under a vow of silence, slow through high and quiet land. Answers in gestures: binds the hurt, and a mote of aether buys a blessing (Focused, a little Magic). |
| refugees | Roam (sitting) | A family fled from a raid, to players with a home. A meal and one of them follows you home and stays there as one of your people; the seed they carried is yours. |
| puppeteers | Camp, show | Set up a little stage by day, children gathered. Watch the play through (20 s within 8 blocks) and you go off Refreshed; a coin in the hat is Focused. |
| dance circle | Camp, show | Villagers turn round a drummer at nightfall. Stand in the ring 15 s: Ember-warmed and mended a little. Dance a round (talk) for Hitpoints XP. |
| cartographer | Camp, Town | Camps far from where the world began. Tells the way to the nearest town within 600 blocks, once; teaches a beginner builder to pace out ground. |
| scarecrow | Shadow | A straw figure walks the grass at dusk near a player's home, keeps off and stands stock still when looked at. Whoever reaches it finds seed in its pockets, once. |

| the player | camp with a show | refugees | cartographer |
| --- | --- | --- | --- |
| walks past | nothing | they sit on | nothing |
| stands near | the show, once, after its seconds | nothing | nothing |
| walks off mid-show | the count starts again | n/a | n/a |
| holds what they want | a coin in the hat | food: one follows home | n/a |
| strikes one | scatter, no show | scatter | scatter |

## Catalogue of 100

Status: `done` is in a batch above, `next` is the second batch (ages 1 and 2: Nomad and Farm
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
| 13 | cowherd | see batch 4 | Herd | 2 | done |
| 14 | goose girl | see batch 6 | Herd | 2 | done |
| 15 | berry pickers | see batch 4 | Forage | 1 | done |
| 16 | fisher folk | see batch 4 | Gather | 1 | done |
| 17 | woodcutters | see batch 5 | Carry | 2 | done |
| 18 | charcoal burners | see batch 5 | Camp | 2 | done |
| 19 | water carriers | see batch 5 | Carry | 2 | done |
| 20 | harvest family | see batch 5 | | 2 | done |
| 21 | herb gatherer | see batch 4 | Forage | 1 | done |
| 22 | wandering healer | see batch 5 | | 1 | done |
| 23 | hermit | see batch 6 | Asks | 1 | done |
| 24 | travelling smith | see batch 7 | Camp | 2 | done |
| 25 | travelling cook | see batch 4 | Camp | 1 | done |
| 26 | story teller | see batch 5 | Camp | 1 | done |
| 27 | stray hound | see batch 2 | Roam | 1 | done |
| 28 | deer herd | see batch 4 | Herd | 1 | done |
| 29 | boar sounder | see batch 4 | Herd | 1 | done |
| 30 | fox and farmer | see batch 3 (a fox runs, a farmer chases) | Chase | 2 | done |
| 31 | thief and constable | see batch 3 | Chase | 2 | done |
| 32 | poacher | hunts deer; reporting them to a guard pays | Hunt | 2 | later |
| 33 | hunting party | villagers with hounds track a boar | Hunt | 2 | later |
| 34 | scarecrow walker | see batch 7 | Shadow | 2 | done |
| 35 | night watchman | walks a village ring with a lantern | Patrol | 2 | later |
| 36 | village guard patrol | guards walk a loop between settlements | Patrol | 2 | later |
| 37 | tax collector | asks coin from a player with a home; refusing makes guards hostile for a day | Meeting | 2 | later |
| 38 | wedding procession | see batch 6 | | 2 | done |
| 39 | funeral procession | see batch 6 | | 2 | done |
| 40 | festival drummers | drums and a dance ring; a refreshed effect | Dance | 2 | later |
| 41 | dance circle | see batch 7 | Show | 2 | done |
| 42 | caravan | pack animals, guards and a merchant on a long road | March | 3 | later |
| 43 | refugees | see batch 7 | Guide | 2 | done |
| 44 | soldiers on the march | two columns that do not stop for a player | March | 3 | later |
| 45 | knight errant | duels for a coin; wins a favour | Duel | 3 | later |
| 46 | wandering monk | see batch 7 | Meeting | 2 | done |
| 47 | travelling alchemist | see batch 6 | Asks | 2 | done |
| 48 | rat catcher | a piper followed by rats | Lure | 2 | later |
| 49 | gypsy wagon | a wagon camps; fortunes told (a journal hint) | Camp | 2 | later |
| 50 | puppeteers | see batch 7 | Show | 2 | done |
| 51 | circus | acrobats, a bear on a chain; scares animals | Camp | 3 | later |
| 52 | messenger | runs between settlements; a letter for a coin | Errand | 2 | later |
| 53 | postman with mule | delivers to a player's home mailbox | Errand | 3 | later |
| 54 | cartographer | see batch 7 | Town | 2 | done |
| 55 | prospector | see batch 6 | Gather | 2 | done |
| 56 | miners' shift | a row of miners with lamps walk to a mine mouth | March | 3 | later |
| 57 | stonecutters | haul a block; leave a cut stone | Haul | 3 | later |
| 58 | lamplighter | at dusk lights lamps along a road | Errand | 3 | later |
| 59 | beekeeper | smoke and bees; honey for a comb | Camp | 2 | later |
| 60 | truffle hunter | see batch 7 | Forage | 2 | done |
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
- **Forage / Gather**: done (batch 4): members work the blocks of a name or tag about them.
- **Carry**: done (batch 5): a `Gather` worker takes each load to a pile or a field.
- **Haul**: two members carry a load along a line; kill one and it drops the load.
- **Escort**: the party follows the player toward a settlement; arriving pays out, losing
  them (distance, death) ends it.
- **Chase**: done (batch 3): one cast member is the quarry, another the pursuer; stopping the quarry pays.
- **Ambush**: done (batch 3): lies in wait until a player is within a distance, then rushes.
- **Lure**: done (batch 3): leads the player on if followed; the end is a reward. A trap end is a
  later `Lure` whose lurkers spring at the end.
- **Show**: done (batch 7): a camped party puts on something to watch; staying pays.
- **Duel / Dance / Crossing / Vigil**: fixed arrangements of the formation code.

## Notes

- Nothing here is added to the world: parties exist only near players and are taken away
  when none is near, so a save holds none and a restart loses nothing.
- Everything seeded: the same seed and tick pick the same party, pass distance and stops.
- Players cannot be pushed around by any of it (no knockback).
- Every spoken line is a lang key in English and Japanese, and every number is data.
