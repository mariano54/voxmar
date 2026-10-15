# Traps

Traps are blocks a player sets where small animals run, and comes back to (`traps.ron`,
`engine/src/trap.rs`, `server/src/traps.rs`). Each trap is two blocks: `set`, waiting, and
`sprung`, holding what it caught.

| Rule | What happens | Test |
| --- | --- | --- |
| Catch | A wild creature (not one a player keeps) of a species the trap `catches` whose feet stand in a set trap's cell is caught. It leaves the world, and the trap shows sprung. Traps look every 4 ticks, and catch one creature each. | `a_rabbit_steps_into_a_snare_and_is_taken_from_it`, `a_snare_holds_only_small_wild_game_in_its_cell` |
| Take | A right click on a sprung trap (`base:take_from_trap`) takes the catch. Its loot is rolled for the taker as their own kill (their Hunter level, the tool in hand, the night). It goes to the inventory, and what does not fit drops at the trap. The species' kill experience trains its skill, and the journal counts it. The trap is set again. Taking needs the trap's `skill` at `level`. | `a_rabbit_steps_into_a_snare_and_is_taken_from_it` |
| Keep | A catch is saved with the world. Breaking a sprung trap loses its catch. | `a_catch_keeps_across_a_save_and_is_lost_with_the_snare` |
| Load checks | The set and sprung blocks differ, belong to one trap only, and are not solid. A trap catches at least one known species. The skill is known, and the level is 1 to the top level. | `bad_traps_are_refused` |

## Base

**Snare** (Nomad, Hunter 1): a loop of twine pegged low between two sticks (two twine over
two sticks, Hunter 1). It catches rabbits, hares and field mice. Set one on the ground
where they graze, go about the day, and come back to the sprung one for raw meat, hide and
the hunt's experience. Gate step: Snare, after Dry.

Later: bird nets (Hunter 3) and box traps (Hunter 8) are more entries in `traps.ron`. Bait
that draws game to a trap, and catches rolled while nobody is near, are left for later.
