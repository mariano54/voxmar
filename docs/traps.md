# Traps

Traps are blocks a player sets where small animals run, and comes back to (`traps.ron`,
`engine/src/trap.rs`, `server/src/traps.rs`). Each trap is two blocks: `set`, waiting, and
`sprung`, holding what it caught.

| Rule | What happens | Test |
| --- | --- | --- |
| Catch | A wild creature (not one a player keeps) of a species the trap `catches` whose feet stand in a set trap's cell is caught. It leaves the world, and the trap shows sprung. Traps look every 4 ticks, and catch one creature each. | `a_rabbit_steps_into_a_snare_and_is_taken_from_it`, `a_snare_holds_only_small_wild_game_in_its_cell` |
| Take | A right click on a sprung trap (`base:take_from_trap`) takes the catch. Its loot is rolled for the taker as their own kill (their Hunter level, the tool in hand, the night). It goes to the inventory, and what does not fit drops at the trap. The trap's share of the species' kill experience trains its skill, and the journal counts it. The trap is set again, unless it wore out. Taking needs the trap's `skill` at `level`. | `a_rabbit_steps_into_a_snare_and_is_taken_from_it` |
| Keep | A catch is saved with the world. Breaking a sprung trap loses its catch. | `a_catch_keeps_across_a_save_and_is_lost_with_the_snare` |
| Chance | A creature the trap catches is held by its `chance` (or its own in `chances`), else it slips free and bolts, frightened (`Mind::hurt`). It is rolled for once as it steps in, not again while it stays in the cell. | `a_snare_holds_a_rabbit_by_its_chance_and_one_that_slips_it_bolts` |
| Tear | A wild creature with more health than the trap `holds`, or one that fights (Defend or Hunt), tears the trap by `tear` as it steps in. The trap is gone, leaving `leaves` (an item of each). Kept animals never step into a trap at all. | `a_deer_or_a_fox_tears_a_snare_by_its_chance_and_a_rabbit_never` |
| Wear | Taking a catch wears the trap out by `wear`: gone, leaving `leaves`, rather than set again. | `a_snare_wears_out_by_its_wear_and_a_catch_gives_its_share_of_a_kill` |
| Experience | A catch gives the trap's `xp` share of the species' kill experience. | the same |
| Shy | Within `shy_reach` blocks of a block tagged one of `shy_of`, nothing is caught or rolled for: game keeps off fire. | `no_game_is_caught_by_a_fire` |
| Rolls | Every roll (held, torn, worn out) is the trap's own: by the world's draw key, the trap's cell and its count of rolls there, saved (`trap_rolls` in `world.ron`, none in older worlds) and kept when the trap goes, so one set again in the same place never draws a roll twice. Never the tick. | `a_snares_rolls_go_by_its_cell_and_count_not_the_tick` |
| Load checks | The set and sprung blocks differ, belong to one trap only, and are not solid. A trap catches at least one known species, each small enough for it to hold and none that fights. The skill is known, and the level is 1 to the top level. Chances and `xp` are over 0 and at most 1; `chances` names only species it catches; `tear` and `wear` are 0 to 1; `leaves` are known items; `shy_reach` is 0 to 16, with `shy_of` tags exactly when it is over 0. | `bad_traps_are_refused` |

## Base

**Snare** (Nomad, Hunter 1): a loop of twine pegged low between two sticks (two twine over
two sticks, Hunter 1). It catches rabbits, hares, field mice and junglefowl. Set one on
the ground where they graze, go about the day, and come back to the sprung one for raw
meat, hide and half the hunt's experience. A rabbit is held three times in five (a hare
one in two, a field mouse 0.7, a junglefowl 0.4). Anything over 6 health, or that fights
(a deer, a boar, a fox, a wolf), tears it six times in ten. It wears out after about five
catches (`wear` 0.2), leaving one of its two sticks. Gate step: Snare, after Dry: rabbits
are put in until one is held (at most 8; the gate's draw key is fixed, so the same ones
slip every run), and the snare taken from is set again or worn out.

## Snares as a hunt

Oct 15, 2026. Snares are the Nomad's third way to hunt, beside melee and the bow (docs/combat.md, "Nomad hunting"). Before this pass a snare was sure, gave the whole kill's experience and never wore out. Now it costs and is unsure, so a line of them pays for the walk and no more than the hunt.

`engine/tests/nomad_snares.rs` measures it, seeded, in game time. Six rabbits wander a 48-block meadow, each taken replaced at its edge 90 s later, so both ways of hunting draw on the same supply. A rabbit that slips a snare bolts at a run while its fear lasts. The snare line is walked every ten minutes. A hunter on the same meadow sprints to the nearest rabbit and takes it with a short bow in the 8.1 s `nomad_hunting.rs` measures. Experience an hour, 16 runs each:

| Snares | Before (sure, whole kill, never worn) | After | Snares made in the hour |
| --- | --- | --- | --- |
| 4 | 75.0 | 28.9 | 7.2 |
| 16 | 248.0 | 103.8 | 26.6 |
| 64 | 558.2 | 265.5 | 91.8 |
| 256 | 832.2 | 414.4 | 296.9 |
| Hunter, short bow | 879.8 | 879.8 | |

The test pins that no line of snares, up to 256, earns as much as the hunter, and that more snares catch more.

How the snare meets the rest of the world, each a general rule:

- **Pens and villages.** Kept animals never step into a trap: a snare in a pen catches nothing and is never torn by the herd. A village's own animals are kept. Its fires and hearths keep game off (below), so a snare among the houses catches little.
- **In a home, or by a fire.** Nothing is caught within 6 blocks of a block tagged `fire`, `heat` or `hearth` (a campfire, a torch, a kiln, the home's hearth), and nothing is rolled for there. A snare line belongs out on the meadow, a walk from home.
- **Big game passing.** A deer, a boar or an aurochs crossing a snare line tears snares as it goes. Set snares where small game runs and big game does not.
- **The boar's flee.** A fleeing boar tears what it runs through, and once it fights back (PR 302) it is torn by fighting too. A snare is never a way to take a boar.
- **No prey, no catch.** Every catch is a real creature stepping into the snare's cell, and creatures live only in loaded land, so a snare in empty or unloaded land catches nothing.
- **Rolls.** A rabbit standing in a snare is rolled for once. Breaking and setting a snare again in the same place goes on from its count, so no one can replay a lucky roll.

Later: bird nets (Hunter 3) and box traps (Hunter 8) are more entries in `traps.ron`, and bait that draws game to a trap is left for later.

