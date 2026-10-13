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
| wander | idle | strolls about home; pause length by style and energy (cats and bears pause long, snails barely) |
| look_around | idle | stands and turns its head; tires of it |
| stand_to_look | social | rears up or lifts its head to watch a player; nervous ones more |
| graze | drive | goes to its food and eats, head down; now and then a tuft is gone for good |
| flee | danger | runs from a threat; hoppers zigzag; raises the alarm for kin |
| freeze | danger | holds still and watches a threat just beyond fleeing distance, for as long as its nerve holds |
| blink | reflex | supernatural: vanishes a few blocks away from a threat that comes too close |
| herd | social | closes on its group when it drifts apart; spacing by sociability |
| sentinel | social | stands watch while kin graze |
| tempted | drive | follows a player holding its favourite food; a grudge outweighs the food |
| hunt | danger | chases and strikes, by RuneScape's rules; stalkers creep crouched, then pounce |
| defend | danger | fights back when hurt (the boar, the bull, the bear) |
| curious | social | comes to look from a distance, backs off if you come closer |
| return_home | drive | every creature: goes home when past its leash, until well inside |

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
- **Warnings** (planned): many creatures warn before attacking (an adder rattles at 5
  blocks and strikes at 2; a bear stands up), so players can back away.

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

Now: steering straight at goals with box collision, stepping up single blocks, giving up a
goal after two stuck seconds. Next, from the deep dive:

- A* over standable cells with a per-species movement profile (step, jump, safe fall,
  media) and sparse overrides of one block-cost table (water, fire, powder snow, berry
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
