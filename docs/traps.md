# Traps

Traps are blocks a player sets where small animals run, and comes back to (`traps.ron`,
`engine/src/trap.rs`, `server/src/traps.rs`). Each trap is two blocks: `set`, waiting, and
`sprung`, holding what it caught.

| Rule | What happens | Test |
| --- | --- | --- |
| Catch | A wild creature (none a player owns, no companion or villager, and none a village or a wanderer keeps: as `despawn_far` has it) of a species the trap `catches` whose feet stand in a set trap's cell is caught. It leaves the world, and the trap shows sprung. Traps look every 4 ticks, and catch one creature each. | `a_rabbit_steps_into_a_snare_and_is_taken_from_it`, `a_snare_holds_only_small_wild_game_in_its_cell`, `a_companion_or_a_villager_never_springs_or_tears_a_snare` |
| Take | A right click on a sprung trap (`base:take_from_trap`) takes the catch. Its loot is rolled for the taker as their own kill (their Hunter level, the tool in hand, the night). It goes to the inventory, and what does not fit drops at the trap. The trap's share of the species' kill experience trains its skill, and the journal counts it. The trap is set again, unless it wore out. Taking needs the trap's `skill` at `level`. | `a_rabbit_steps_into_a_snare_and_is_taken_from_it` |
| Keep | A catch is saved with the world. Breaking a sprung trap loses its catch. | `a_catch_keeps_across_a_save_and_is_lost_with_the_snare` |
| Chance | A creature the trap catches is held by its `chance` (or its own in `chances`), else it slips free and bolts, frightened, though no player is near (`Mind::hurt`, and the alarm kin raise, `Mind::alarmed`). It is rolled for once as it steps in, not again while it stays in the cell nor on its way out. | `a_snare_holds_a_rabbit_by_its_chance_and_one_that_slips_it_bolts`, `a_rabbit_that_slips_a_snare_runs_from_it_with_no_one_near` |
| Tear | A wild creature with more health than the trap `holds`, or one that fights (Defend or Hunt), tears the trap by `tear` as it steps in, unless the trap is shy there (below). The trap is gone, leaving `leaves` (an item of each). Kept animals never step into a trap at all. Gate step: Snare, "snare torn": a deer held in a fresh snare tears it, and a stick lies by it. | `a_deer_or_a_fox_tears_a_snare_by_its_chance_and_a_rabbit_never` |
| Wear | Taking a catch wears the trap out by `wear`: gone, leaving `leaves`, rather than set again. | `a_snare_wears_out_by_its_wear_and_a_catch_gives_its_share_of_a_kill` |
| Experience | A catch gives the trap's `xp` share of the species' kill experience. | the same |
| Shy | Within `shy_reach` blocks of a block tagged one of `shy_of`, nothing is caught, torn or rolled for: game keeps off fire. Looked for first, once while a creature stays in the cell. | `no_game_is_caught_by_a_fire` |
| Skilled | A better hunter does better (`skilled`): for each level of the trap's skill over its `level`, up to `skilled.levels`, the chance a catch is held rises by `skilled.chance`, and the chances of tearing and wearing out fall by `skilled.tear` and `skilled.wear`. Hold and tear go by the setter's level when they set it (a trap no player set does as its data says), wear by the taker's. | `a_better_hunters_snare_holds_more_and_is_torn_less` |
| Rolls | Every roll (held, torn, worn out) is the trap's own: by the world's draw key, the standing trap's serial and its count of rolls, never the tick. Each trap set where none stands gets the next serial; it is kept while the trap is set or sprung and goes with it, so the saved list (`standing_traps` and `trap_serials` in `world.ron`, none in older worlds) holds only standing traps, and one set again in the same place is a new trap that never draws the old one's rolls. | `a_snares_rolls_go_by_its_cell_and_count_not_the_tick` |
| Load checks | The set and sprung blocks differ, belong to one trap only, and are not solid. A trap catches at least one known species, each small enough for it to hold and none that fights. The skill is known, and the level is 1 to the top level. Chances and `xp` are over 0 and at most 1; `chances` names only species it catches; `tear` and `wear` are 0 to 1; `leaves` are known items; `shy_reach` is 0 to 16, with `shy_of` tags exactly when it is over 0, each one some block is tagged; `skilled` steps are 0 to 0.05 a level, over levels a player can have, and never take a chance past 1 or below 0. | `bad_traps_are_refused` |

## Base

**Snare** (Nomad, Hunter 1): a loop of twine pegged low between two sticks (two twine over
two sticks, Hunter 1). It catches rabbits, hares, field mice and junglefowl. Set one on
the ground where they graze, go about the day, and come back to the sprung one for raw
meat, hide and half the hunt's experience. A rabbit is held three times in five (a hare
one in two, a field mouse 0.7, a junglefowl 0.4). Anything over 6 health, or that fights
(a deer, a boar, a fox, a wolf), tears it six times in ten. It wears out after about five
catches (`wear` 0.2), leaving one of its two sticks. A better hunter sets it better: each
Hunter level over 1, up to 30 more, holds 0.008 more of what steps in and tears 0.01 and
wears out 0.004 fewer, so at Hunter 31 a rabbit is held 0.84 of the time, a snare torn 0.3
and worn out 0.08. Gate step: Snare, after Dry: rabbits are put in until one is held (at
most 8; the gate's draw key is fixed, so the same ones slip every run), the snare taken
from is set again or worn out, and a deer put in a fresh one tears it, leaving a stick.

## Snares as a hunt

Oct 15, 2026. Snares are the Nomad's third way to hunt, beside melee and the bow (docs/combat.md, "Nomad hunting"). Before this pass a snare was sure, gave the whole kill's experience and never wore out. Now it costs and is unsure, so a line of them pays for the walk and no more than the hunt.

`engine/tests/nomad_snares.rs` measures it, seeded, in game time. Six rabbits wander a 48-block meadow, each taken replaced at its edge 90 s later, so both ways of hunting draw on the same supply. A rabbit that slips a snare bolts at a run while its fear lasts. The snare line is walked every ten minutes. A hunter on the same meadow sprints to the nearest rabbit and takes it with a short bow in the 8.1 s `nomad_hunting.rs` measures. Experience an hour, 16 runs each:

| Snares | Before (sure, whole kill, never worn) | After, new hunter | Snares made | After, Hunter 31 | Snares made |
| --- | --- | --- | --- | --- | --- |
| 4 | 75.0 | 28.9 | 7.2 | 33.2 | 5.1 |
| 16 | 248.0 | 103.8 | 26.6 | 114.1 | 20.4 |
| 64 | 558.2 | 265.5 | 91.8 | 273.1 | 73.9 |
| 256 | 832.2 | 414.4 | 296.9 | 416.8 | 272.9 |
| Hunter, short bow | 879.8 | 879.8 | | 879.8 | |

A better hunter's snares (`skilled`, by the setter's Hunter level when the snare was set) hold a little more, tear less and wear slower, so they earn more and need fewer snares made; at Hunter 31 the snare is at its best.

The test pins, for a new hunter and for the best, that no line of snares up to 256 earns as much as the hunter, that more snares catch more, and that the best hunter earns more from a line than a new one.

How the snare meets the rest of the world, each a general rule:

- **Pens and villages.** Kept animals never step into a trap: a snare in a pen catches nothing and is never torn by the herd. A village's own animals are kept. Its fires and hearths keep game off (below), so a snare among the houses catches little.
- **In a home, or by a fire.** Nothing is caught within 6 blocks of a block tagged `fire`, `heat` or `hearth` (a campfire, a torch, a kiln, the home's hearth), and nothing is rolled for there. A snare line belongs out on the meadow, a walk from home.
- **Big game passing.** A deer, a boar or an aurochs crossing a snare line tears snares as it goes. Set snares where small game runs and big game does not.
- **The boar's flee.** A fleeing boar tears what it runs through, and once it fights back (PR 302) it is torn by fighting too. A snare is never a way to take a boar.
- **No prey, no catch.** Every catch is a real creature stepping into the snare's cell, and creatures live only in loaded land, so a snare in empty or unloaded land catches nothing.
- **Rolls.** A rabbit standing in a snare is rolled for once. Each standing snare has its own number, and its rolls are drawn from that number and its count, so no one can replay a lucky roll: a snare set again in the same place is a new snare with a new number. Only standing snares are kept in the save.

Later: bird nets (Hunter 3) and box traps (Hunter 8) are more entries in `traps.ron`, and bait that draws game to a trap is left for later.
