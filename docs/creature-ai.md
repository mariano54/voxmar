# Creature AI and behaviour generation

Oct 6, 2026. How creatures sense, decide and move, how each species and each individual
gets its own way of behaving, and how behaviours are generated from data. Part of the
creatures, food and breeding design (see [creatures.md](creatures.md)).

Mariano's brief: complex AI that is very different for each creature, unpredictable,
varied and partly random, shaped by climate and everything else, with a system that
generates behaviours; mix and match the best ideas from every game and mod researched.
Models and animations stay simple; behaviour is where the depth goes.

Sources: the creature AI deep dive (vanilla Minecraft and Bedrock, 14 mob mods read from
source, 17 other games, 30+ techniques; `research/creature-ai.html`, published at
https://claude.ai/artifact/9mXqfLW6wwsXMTuv7SJLPZ, notes in `research/creature-ai/`) and
the mob survey (`research/mob-mods.html`). Techniques only; credited in INSPIRATIONS.md,
"Creatures and their behaviour".

## Decisions

| # | Question | Decision | From |
| --- | --- | --- | --- |
| 1 | Chooser | Dual utility: modules wish to run in rank bands (reflex, danger, drive, social, idle); the top band with any wish competes; inside it, the running module keeps going while within a tenth of the best, otherwise one is drawn at random among those within a tenth, weighted by score. | Zoo Tycoon 2, Dave Mark's IAUS, Guild Wars 2: Heart of Thorns |
| 2 | What a behaviour is | A module: a small parameterised behaviour with its own considerations, from an engine library. Species list modules with weights in data; mods add modules (WASM later). | Heart of Thorns' decision library, Rain World's modules, mobkit |
| 3 | Shared state | Drives (hunger, fear, anger) plus memories with a time to live: who hurt it, who scared it, who fed it, an alarm. Cooldowns are memories that run out. | Minecraft's Brain, Crytek's decaying stimuli |
| 4 | Individuals | Six seeded personality axes (aggression, bravery, sociability, curiosity, energy, nervousness), spread about the species' values and pushed away from the middle so individuals read clearly. Each axis changes something visible. Heritable through breeding. | Rain World, Dwarf Fortress facets, Minecraft pandas |
| 5 | Unpredictability | Near-tie draws in the chooser, per-individual personality, randomised pauses and wander targets, regional modulation, learned regional traits, rare quirks. All seeded: a replay repeats it. | |
| 6 | Aggression | RuneScape's rules: hunters attack players up to twice their combat level (plus one), leave alone players who have stayed ten minutes in one small area, and anger overrides both. | Old School RuneScape |
| 7 | Food web | Relations by tag (prey, rival, afraid) plus one power number from health, attack and group size; hunt lower, flee higher. Mod creatures join it with no pairwise rules. | Untamed Wilds' eco level, Rain World's relationship table |
| 8 | Movement | Species character through movement style, gait timing (bursts, pauses, pounce), turn rate and per-species overrides of one block-cost table. | Minecraft path costs, the fox's pounce, Vintage Story turn rates |
| 9 | Generation | A generator writes a species' modules, quirks and numbers from its ecology, then perturbs them by seed; regions modulate them by climate; individuals roll personalities. Hand-written species override any part. | Dwarf Fortress raws, Bedrock component groups |
| 10 | Cost | Think 5 times a second near players, once a second further out; sensors throttled and jittered; events wake creatures at once; far creatures are numbers in the regional population. | Minecraft sensors, Veloren rtsim, Rain World's realised rooms |

## The loop

```
fields ─┐                                    ┌─▶ actions (eat, strike, call, blink)
events ─┼─▶ senses ─▶ memories ─┐            │
drives ─┘   (throttled)         ├─▶ chooser ─┤
personality ───────────────────┘   (bands)  └─▶ locomotion (path, steering, gait) ─▶ world
```

1. **Senses** turn the world into stimuli; events (a blow, a noise, food dropped, a herd's
   alarm) arrive at once, sight and hearing are polled on a throttled timer.
2. **Memories** hold what was sensed for a while (Minecraft keeps revenge 15 s; here a
   grudge lasts 45 s and fades by bravery).
3. **Drives** rise and fall over time and with events: hunger, fear, anger; later fatigue,
   thirst, social, comfort and mating in season.
4. **The chooser** asks every module how much it wants to run, and in which band.
5. **The chosen module** asks the body for a goal, a speed, something to face, and acts
   on the world through actions: strike, eat, blink, pounce, raise the alarm.
6. **Locomotion** moves the body with the same box collision as players
   (`engine::creature::motion`), at the module's speed times the individual's pace.

## Rank bands

| Band | Modules | Why first |
| --- | --- | --- |
| Reflex | blink, (later) escape water and fire, unstick | survival in the next second |
| Danger | flee, freeze, hunt, defend, (later) play dead, burrow, defend young | threats beat appetite: a rabbit never finishes its meal with a fox at its tail |
| Drive | graze, tempted, return home, (later) drink, sleep, breed, hide from rain | needs |
| Social | herd, sentinel, curious, stand to look, (later) groom, play, beg, follow owner | company and interest |
| Idle | wander, look around, (later) bask, sun, wallow, tells | what it does with nothing to do |

## Personality

Six axes per individual, rolled from the species' temperament with a seed of the world and
the creature, spread by the species' `spread`, and pushed away from the middle:

| Axis | What a player sees |
| --- | --- |
| Aggression | how far a hunter notices prey, how eagerly it hunts |
| Bravery | how close you get before it flees; whether it fights back or runs when hurt |
| Sociability | how tightly it keeps to its group |
| Curiosity | whether it comes to look at you |
| Energy | its pace (0.8 to 1.2 of its species'), how long it pauses |
| Nervousness | how often it freezes, stands to look, stands watch; how fast its fear fades |

The inspect action names the stand-out axes in words ("bold, curious, lazy"). Breeding
passes them on (see [breeding.md](breeding.md)).

## The module library

Built now (`engine::creature::brain::modules`):

| Module | Band | What it does |
| --- | --- | --- |
| wander | idle | strolls about home, and with kin about, shorter and toward them by sociability; pause length by style and energy (cats and bears pause long, snails barely) |
| look_around | idle | stands and turns its head; tires of it |
| stand_to_look | social | rears up or lifts its head to watch a player; nervous ones more |
| graze | drive | goes to its food and eats, head down; now and then a tuft is gone for good |
| flee | danger | runs from a threat; hoppers zigzag; raises the alarm for kin (an alarmed one runs home, unless a player in sight who never hurt it holds its favourite food) |
| freeze | danger | holds still and watches a threat just beyond fleeing distance, for as long as its nerve holds |
| blink | reflex | supernatural: vanishes a few blocks away from a threat that comes too close |
| herd | social | closes on its group when it drifts well apart (its spacing past it), to its companion's near side, the same companion until there; spacing by sociability |
| sentinel | social | stands watch while kin graze |
| tempted | drive | follows a player holding its favourite food, hurrying as they draw ahead, keeping after them to 1.5 times the radius it noticed them at, and stepping back when too close; its home goes with it, so it stays where it was led; a grudge outweighs the food |
| hunt | danger | chases and strikes, by RuneScape's rules; strikes up to its height and 1.5 blocks over its feet and down at a head 1.5 under them, from a pit's rim (`strike: (above, below)` on the creature), and only along a way through the air from its body to the player's head or middle (`blow_open`: never through a roof or a wall, and standing at a target it has no such way to, it gives up at once; a charge, which runs at the target first, needs instead a lane along the ground from one side of its body or the other, `charge_open`: a wall across its run shuts it off, a trunk on the line does not); stalkers creep crouched, then pounce; gives up past its leash (unless struck there in the last 3 s: it fights back), or after balking twice running (a search finding no way to its target brings a balk on sooner; standing over or under its target out of that reach, a player down a hole too deep, it has no way at once), then lopes home and leaves players be for 45 s (strolling the side away from them) unless struck again; back at a chase within a minute, it gives up at its first balk |
| defend | danger | fights back when hurt (the boar, the bull, the bear); gives up as hunt does |
| curious | social | comes to look from a distance, backs off if you come closer |
| warn | danger | every creature whose kind has a `warn`: stands its ground and warns a player before it attacks them (its stance and `warn` sound): a hunter those it would hunt, one that only defends itself those it sees within its radius, never one holding its food unless it hunts; goes for one who stays its seconds or comes within `near`, as if struck; lets be one who backs off first (see "Warnings" below) |
| fire | danger | every creature whose kind has a `fire`: backs away from a player holding a fire at it (a torch), facing them, if it is one of its kind's `fear` share; at night walks out of a lit fire's ward; not once provoked, kept or on an errand (see "Fire" below) |
| hide | danger | every creature whose kind has a `flush`: crouches still while a noticed player is beyond its `hold` and within the distance it would otherwise flee at; not once they flushed it, it is hurt, its kin are alarmed, they hold its favourite food, or it is kept or on an errand (see "Flush" below) |
| return_home | drive | every creature: goes home when past its leash, until well inside; having given up a chase, lopes all the way home |

Planned (from the deep dive's library): cache food, drink, sleep, bask, seek shade, huddle,
hibernate, migrate, school, flock, ambush, pack hunt, scavenge, play dead, burrow, defend
young, defend territory, mark territory, court, nest, raid crops, raid stores, pollinate,
steal item, beg, follow owner, fetch, gift, play, investigate noise, call, groom, wash
food. Supernatural: phase through leaves, gaze trigger, mimic, light feeder, moon cycle,
curse spread, aura, molt rebirth, dream walk.

## Senses

| Sense | Range | What changes it |
| --- | --- | --- |
| Sight | species range | daylight (night halves it, except for night creatures), weather, cover; sneaking players half as visible |
| Hearing | from each sound's loudness | events: sprinting, mining, placing, combat, thunder; rain and wind mask quiet sounds |
| Smell | species range, stretched downwind | the wind (`engine::weather::gust`): ×3 downwind, ×0.3 upwind; blood, food and dung leave scent in a sparse field that rain washes out and cold keeps |
| Vibration | burrowers | running feet; sneaking is silent |
| Aether | supernatural only | the aether field and aether-bearing items |

Where it looks decides what it notices (`senses.field`, `close`, `react` in its data). It sees a
player within `field` degrees either way of where it faces, with nothing opaque between
(`Server::in_view`); a deer's eyes see most of the way round (160), a wolf's to the front (110),
and 180, the default, sees all round. Out of view it notices a player only within `close`
blocks (it hears and smells them), when they hold its food within its hearing, or once they
hurt it; a player it has noticed it keeps in mind for `keep` seconds out of sight (default 10, at most 60), so
turning away does not drop a chase. Just noticing someone, it takes a double take of `react`
seconds, facing them, alert, before it runs, hunts or comes to look: the moment a player has to
back off, or to creep closer behind it. A blow skips the double take.

## Aggression

- **Temperament, not type.** Hostile is high aggression and low fear; neutral is a
  creature with an attack that only defends; passive has no attack.
- **Aggro radius** = the hunt module's radius × daylight (by activity) × hunger ×
  aggression, capped by sight.
- **RuneScape's rules**: hunters ignore players more than twice their combat level, and
  players who have stayed within ten blocks of one spot for ten minutes; anger overrides
  both. Weak areas stay safe for strong players; camping one spot calms down.
- **Provocation**: a blow angers a creature that can fight (if it is brave enough and not
  badly hurt; otherwise it flees) and is remembered. Kin within earshot are alarmed.
- **Warnings** (Oct 20): a creature whose kind has a `warn` (creatures.ron) warns a
  player before it attacks them, so they can back away: it stands its ground facing them
  in its warning stance (a pose, `Pose::Warning`, protocol 58: its `stance` of nod, crouch
  and rear, drawn as a tell is) and makes its `warn` sound (the wolf's snarls slowed
  into a low growl, the bear's deep roar, the badger's alarm lowered to a hiss; a kind
  whose voice has none uses its `alert`, and every kind that warns in the base game has
  one). If they stay its `seconds`, or come within its `near` blocks, it is provoked
  (`Mind::provoked`) and goes for them as if they had struck it, its hunt or defence
  taking over in that same decision. A warning only tells a hunt before it comes, and
  never changes whom a hunter goes for or when: it warns the one its hunt would go for (the
  nearest it would hunt, as far off, kept, frightened or on an errand as much as not) if
  it sees them; one it does not see it has not noticed (unless they are close, hurt it or
  hold its food), so it leaves them be. One that only defends itself (a badger, a bear)
  warns those it sees (in its field of view with a clear line from its eyes, the same
  `in_view` look it notices by) within its `radius`, and then defends
  its ground. No warning goes to a player holding the food that tempts it unless it hunts
  (a hunter's chase outranked the food before, and still does): a lured bear or badger
  follows the food as before. It warns no one creative, of more than twice its level or
  settled long in one place, nor having given up a chase, and one that only defends
  itself not while frightened, kept or on an errand; struck, it fights at once. One clock runs for the creature, whoever it warns
  (a second player stepping nearer does not start it over), and a warning holds over
  decisions spent on anything else for `WARN_LAPSE` (0.5 s); a player must go `WARN_EXIT`
  (1.5) blocks past where it began to warn to be let be, so the edge does not flicker.
  Then the warning is over, with no grudge, and a later one starts afresh. The wolf growls
  head low for 1.5 s (at once within 4 blocks), the badger hunches and hisses for 1.2 s
  within 5 (2), the brown bear stands up and roars for 2 s within 8 (3). Load-checked: an
  attack to follow it, no boss, a `radius` (2 to 16) only for one that does not hunt, near
  0.5 to a block inside the radius, seconds 0.5 to 4, stance 0 to 1. Tests: the engine's
  `a_warning_hunter_warns_then_goes_for_one_who_stays`,
  `a_defender_that_warns_defends_its_ground` and `warnings_out_of_range_are_refused`; the
  server's `a_wolf_warns_before_it_goes_for_a_player`,
  `lured_bears_and_badgers_follow_without_warning` and `a_badger_warns_only_one_it_sees`.
  The gate's Narrow and Feel steps summon their bear held and let it go with the first
  blow, as Climb does its wolf: stepping close is meant to provoke it.
- **Fire** (Oct 21): a creature whose kind has a `fire` (creatures.ron) is afraid of it.
  A player holding a fire (an item tagged `fire`: a torch) at it, one it has noticed
  (`Senses`: seen where it looks, or close by, and kept in mind a while as it turns away),
  within its `give` blocks, it backs away from, facing them (in its warning stance, for one that
  warns), until they are `give` off and a block more (`FIRE_EXIT`); a warning it was giving
  lapses. Its kind's `fear` is the share that give ground, each individual by its own
  roll (`Fire::gives_ground`); the bold rest stand theirs. Held off, one that warns
  warns from beyond the torch as long as it is held and never comes for them by its
  warning's time (only within its `near`, which the torch keeps it out of), nor hunts them
  unwarned when it does not see them; put away, it does. At night, unless `ward: false`, it keeps out of a lit fire's ward: a block's
  `ward` (a campfire, 8 blocks) or a lit multiblock's (the Homestead Hearth 16, the
  Crucible Kiln 10). Standing in one it walks out past its edge, a stroll (roam or
  wander) that would end in one is dropped, and a player standing in one it neither
  hunts nor warns. The server finds the wards each second at night (`find_wards`: the lit
  multiblocks', and warding blocks within 20 blocks across and 5 up or down of each
  player). Provoked, it fights whatever the light; kept animals and those on an errand
  (villagers, guards) pay fire no heed; a boss or a supernatural kind may not have a
  `fire`. Base game: wolf (fear 0.85, give 5), brown bear (0.6, 5), badger (0.9, 3).
  Tests: the brain's `fire_keeps_the_wild_back` and `fire_out_of_range_is_refused`;
  the server's `a_torch_holds_a_wolf_off`, `wolves_keep_out_of_firelight_at_night` and
  `a_player_by_a_lit_hearth_is_let_be_at_night`; `wards_out_of_range_are_refused`.
- **Flush** (Oct 21): a game bird whose kind has `flush` (creatures.ron) sits tight: a
  player it has noticed beyond its `hold` it lets be, crouched still (the `hide` module),
  as far out as it would otherwise flee at (its `Flee` radius by its fear); one further
  off it does not hide from, and lives its day. A player within its `hold` it bursts away
  from (`Flee` takes `hold` as its distance) at `burst` times its run for its first
  `seconds`, raising the alarm so its kin burst
  with it. Flushed, it recalls being scared by them (`Memory::ScaredBy`, 10 s) and runs on
  at its kind's distance rather than settle again at the edge of its hold. Hurt, it flees
  as any prey, with no burst; kept or on an errand it sits tight for no one. A stalker who keeps coming
  gets close, and is startled at the last. Refused at load: hold outside 1 to 6 blocks,
  burst outside 1 to 2.5, seconds outside 0.2 to 3, or a flush on a boss, a supernatural
  kind or one that attacks. Base game: pheasant (hold 3, burst 1.8, 1.2 s), quail (2.5,
  2, 0.8 s), junglefowl (3.5, 1.6, 1 s). Tests: the brain's
  `a_game_bird_sits_tight_then_bursts_away` and the server's of the same name (each bird,
  walked up to in the open, crouches unmoved to half a block outside its hold, and one
  step in it is fleeing faster than its run; with no burst it is not).
- **Winded** (Oct 21): a creature whose kind has `tires` (creatures.ron) runs flat out
  from a threat for its `after` seconds, then winded at its `pace` of its run for `rest`
  seconds, then flat out again; time not spent fleeing gets its wind back a second for a
  second (`Mind::sprinted`, `Mind::winded`). A kept animal or one on an errand runs as
  ever. Hurt, it limps: it runs `hurt` slower at the last of its health, in proportion
  above (a creature does not heal), so each blow a hunter lands brings the catch nearer.
  The base game sets it on the prey faster than a sprinting player (6.1 blocks a second):
  rabbit (3 s, 0.4, 4 s, hurt 0.5), hare (2.5, 0.35, 5, 0.5), red squirrel (2.5, 0.5, 4,
  0.5), red deer (3, 0.4, 5, 0.5), reindeer (4, 0.45, 5, 0.5), wild horse (4, 0.35, 6,
  0.4) and gazelle (3, 0.35, 6, 0.4), each averaging under a sprint over a sprint and a
  rest, so a first hunt on foot ends in a catch: first-days.md's slowest wait was the chase
  for the bed's hides. Tests: the brain's `prey_that_tires_runs_winded_after_a_sprint`;
  the server's `a_sprinting_hunter_runs_down_winded_prey` (a rabbit and a red deer run
  down within 20 s on open ground; without `tires` the rabbit is not) and
  `a_hurt_rabbit_limps` (at a third of its health, from the same seed and spot, it covers
  0.63 of an unhurt one's ground in its sprint, the data saying 0.67; 1.00 without the
  limp).

## Roaming

Every creature has a home (where it appeared; a den, nest or pen later) and a leash
(rabbit 14 blocks, sheep 24, deer 40, wolves 48). Wander targets drift back toward home;
past the leash, return_home takes over until it is well inside. Activity windows (day,
night, crepuscular, any) set how lively it is; out of hours it rests.

## Taming (planned, see breeding.md)

A per-individual trust meter with species ways to raise it (feeding, patience, rescue,
imprinting, gifts), decay scaled by wildness, a backlash chance on failure (RimWorld's
manhunter), and treatment that shapes what the animal becomes at full trust (Don't
Starve's beefalo: fed, ridden or fought). Breeding lowers wildness over generations.

## Movement styles

Numbers in data, read by one locomotion system:

| Style | What makes it |
| --- | --- |
| Walker | walks with pauses (grazers, birds on foot) |
| Hopper | moves in hops; flees in zigzags |
| Stalker | long pauses; creeps crouched toward prey and pounces from about 4 blocks (Minecraft's fox) |
| Trotter | steady trot (canids) |
| Lumberer | slow, heavy, long pauses, short fast charges |
| Creeper | low and slow (snails, crawlers, shamblers) |
| Drifter | floats above the ground, bobbing (wisps) |
| Flier | hops about the ground pecking; takes wing to go further or faster than a walk (fleeing, or now and then across its range), keeps `fly` blocks over the land and the treetops ahead, and flutters down to land when it slows (small birds, `creatures_birds.ron`); spreads its wings and beats them in bursts in flight, and falls when it dies in the air |

The client animates from the same numbers: leg swing from ground speed, a crouch for
stalking, the head down for grazing and up when alert, tails swaying.

## Pathfinding

Now: steering at goals with box collision (`motion::steer`). A walker heads straight
while that way is open (sliding along a wall counts, and so does a single block its jump
really clears); when it is shut, it takes the nearest turn that is open, 40, 80, 120 or
160 degrees off, and keeps to the side it took until it is round, so a creature struck
with its back to a wall runs along it, one in a corner runs out past the player, and one
before a tree runs round the trunk. Its head stops at what its box does not: a turn that
would put it into a wall goes the other way round (only while it still wants that way, and,
standing, only to a heading its head fits at, so it never spins to face a player through a
wall), a step that would is undone, and a head already in a block (one built before its
face) may turn and walk out rather than stand frozen. What its head meets is a step it gets up,
not a wall, when it is judged so at its feet (`Body::head_step`): the first cell its head
comes to at its feet's level is solid, with room above it for its whole body, and its body
raised a block clears it. Then it hops up it, springing on toward it, rather than turn
along it, and its head is let over the step's edge only on that hop. A wall two high (which
a head carried high meets in its upper block), a lintel at head height, or a step under a
low ceiling is no step. It gives up a goal after two stuck
seconds, or four going round, or about one once a search finds no way there. Fliers on
the wing, drifters and swimmers do not steer.

A way open to steer by is also one it can stand on: it looks a body's length ahead along
the slide, and a step down further than `SAFE_DROP` (3 blocks), or into water two deep
for a land animal not already out of its depth, or into lava at any depth (amphibians
too; one already standing in lava may walk on through it to get out, no deeper in), is
as shut as a wall (wading in the shallows too). Unless it is hunting, defending or led by
food a player holds (into a sunken pen), a drop deeper than
it climbs back (a block if it jumps, else none) is shut too, at any pace, unless the ground
below is open: a fill over the squares it could go to down there (`path::way_out`, kept
per landing square for the search, 256 squares of fills a search at most) reaches one it
climbs back up from, or more than 64 of them. That is judged along the way it goes, never
under its own middle: one on a pit's lip walks back off it. So nothing wanders or flees into a hole a player dug two or three deep; a chaser
still drops in. So prey runs along a cliff
edge or a lake shore rather than off it, and a boxed-in creature stands rather than press.
Anything that jumps gets up a single block, however low its jump (the hop is lifted to
clear it), and no higher. Building pieces count by their shapes, not as whole blocks: a
creature walks up a slab, a stair's tread or a roof's rise without a hop, as a player
steps (`Movement::step`, 0.5625 blocks by default, from 0.5 to 1 in data), from the low
side only; the way-finder takes those for open ground too. A hopper (a rabbit) hops up
them instead. A piece that stands in its way and is not walked up (a post, a panel, a
lattice window, a railing) is a whole block to its body, so nothing small slips through the
gaps beside a post or at a corner of panels. A lower slab is a floor: the look ahead judges
a drop from a slab bridge's edge as from any other. A long head held low may lift (up to half a
block, `Body::lift`) to clear a riser as it climbs; its head is never inside a block. A land animal out of its depth (water under its feet and below)
makes for the nearest dry cell within 12 blocks, whatever it had in mind, keeps on through
the shallows, and hauls itself up the bank.

When steering is going round (or already following one), a short A* (`path::find`) looks
for a way over the cells the body fits in: eight directions, no corner cut, up one block
if it jumps, down no further than `SAFE_DROP` in a chase and otherwise no further than
it climbs back unless onto open ground (`Walker::drop`, `way_out`), at most 1500 cells within 24 blocks, again
at most once a second and only when its goal has moved. It walks the cells' centres, still
steering, so a wolf finds the doorway in a wall (a shut door is a wall) and a sheep after
wheat follows the player through it. A creature is made facing open air, so none starts
with its head in a trunk. Next, from the deep dive:

- A per-species movement profile for that search (step, jump, safe fall, media) and sparse
  overrides of one block-cost table (water, fire, powder snow, berry
  bushes), capped per search and per tick.
- A per-chunk connectivity layer so "no path" is answered at once (Factorio).
- Herds share their leader's path; large groups heading one way share a flow field.
- Context steering for the last metres.
- Who is smart and who can be cheesed is a species choice (RuneScape NPCs step straight
  at you, which is what makes safe spots possible).

## Climate, biome, weather and time

Behaviour reads the same fields every system does, so the world shapes it:

| Condition | Pushes toward |
| --- | --- |
| Hot and dry | crepuscular or nocturnal; shade at noon; water at dusk; small ones burrow by day |
| Cold | diurnal; huddle at night; small species hibernate, large ones migrate downhill; dig through snow to forage |
| Wet | amphibious; active in rain; ambush from water |
| Open land, prey | large herds, sentinels, long flight distance, stampedes |
| Forest, prey | small groups, freeze then bolt, short flight distance |
| Caves | no day cycle; hearing and smell over sight; ambush; drawn to or driven off by light |
| Storms | herds spook; birds roost; monsters bolder |
| Seasons (when added) | breeding season, rut, caching in autumn, spring migration |
| Aether | supernatural modules and abilities |

States are named overlays switched by filters on fields and memories (Bedrock's component
groups): `night`, `storm`, `winter`, `angry`, `tame`, `in_heat`, each adding or tuning
modules.

## Behaviour generation

Four layers, all deterministic from the seed and all data:

1. **Species** (once per species): modules, weights and numbers from its ecology. Built
   now for species that list no behaviours (`creature::generated`): wander and look
   around always; graze if it eats something; flee, freeze and stand to look by fear and
   nerves; herd and sentinel by sociability and group size; tempted if it has a favourite;
   hunt if aggressive with an attack, else defend; curious by curiosity. Next: body size
   and diet kind (herbivore, carnivore, omnivore, insectivore) choosing styles, the food
   web's prey and predators, and one or two supernatural modules for supernatural species
   chosen to fit their climate (ember lands: heat auras and fire walking; crystal groves:
   blink steps and light feeding).
2. **Quirks**: 1–3 small, specific behaviours per species, picked by seed and weighted by
   ecology (stands to look, wallows, collects shiny things, plays with dropped items,
   decoys from the nest, washes food, howls in chorus). The mods that feel alive have many
   of these; No Man's Sky felt samey because its bodies shared behaviour scripts.
3. **Regions** (per population cell): hot populations shift nocturnal, cold ones huddle,
   remote ones are bold, hunted ones flee early, and a rare learned trait (foxes here raid
   farms) spreads slowly to neighbouring cells.
4. **Individuals**: the six personality axes above.

**Validation**: each species is simulated in a headless arena for game days and must eat,
flee, not get stuck, keep its decision cost under budget, and (if hostile) reach a player
on open ground. Each gets a behaviour signature (activity curve, flight distance, aggro
radius, group size, home radius, movement style, diet); generated species must differ from
every other species in the same biome by a minimum distance. Every creature feeds some
other system: a drop, a product, a farming role, a threat or a mount.

## Cost and networking

- Creatures within 64 blocks of a player think every 4 ticks (staggered by id), further
  ones every 20; bodies move every tick; creatures with nothing to do cost one scoring.
- Wild creatures further than 128 blocks from every player go (into the population, once
  it exists).
- The server sends a creature's position, velocity, yaw, pose and health when they change,
  at most every other tick; clients carry it on along its velocity and ease toward each
  update.
