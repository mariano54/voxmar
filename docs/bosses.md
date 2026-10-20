# Bosses

A boss is a creature with `boss: true`. Most live in a lair and nowhere else: the first is
the Rootmother, late in the Farm age (Oct 14). One roams instead: Old Tusk, the Nomad age's
great boar, placed by habitat like any animal (no `party_only`, no lair). Everything below
is data: a pack can add a boss with a creature, a lair and a quest, and replace either.

## Where they live: lairs

`lairs.ron` (worldgen, `crates/worldgen/src/lairs.rs`) lists lairs. Each names its boss, the
biomes it lies under, and a grid: one cell every `spacing` blocks, a lair in a cell at
`chance`, tried at up to `tries` spots that sit `inside` blocks into one of its biomes, on
dry, fairly level ground, off rivers and settlements. A lair is a dome of air `radius` wide
and `height` high, `depth` below the ground, walled and floored in its own blocks (the root
hollow's are unbreakable), with a passage three wide rising one block in two to the
surface. `Overworld::lairs_near` finds them; the server and tests use it.

| Lair | Boss | Biomes | Grid | Room |
| --- | --- | --- | --- | --- |
| Root hollow | Rootmother | Old Growth Forest, Dark Forest, Redwood Forest, Old Growth Taiga | 640 blocks, 0.8 | radius 7, height 5, 10 deep |

Lairs are sparse: on seed 1, 22 within 3 km of spawn; on seeds where those forests are far
from spawn, none near (`census::lair_census`, ignored). Lairs are generated with their
columns, so a world made before them gets them only in columns generated after.
`Overworld::VERSION` is unchanged.

## Waking and resting

The server's `base:bosses` process (Low, `crates/server/src/lairs.rs`) wakes a lair's boss
at its heart when a player comes within `wakes` blocks and the lair has none. When the boss
dies the lair rests `rest` days (saved in `WorldMeta.lairs`), then wakes again. No wild
creature is born within `quiet` blocks of a lair. A boss is `party_only`, so habitat never
places one.

## The fight

A boss uses the same combat as any creature: wind-ups, guard, perfect block, dodge,
counter, no knockback. What it adds, in `creature.rs` and `creature/moves.rs`:

- `moves`: attacks it chooses among, each with the phases it is used in, a least distance
  (`least`), and a `tell` (nod, crouch, rear) the client draws through the wind-up. It
  takes the next move that fits, in order, so a fight has a pattern to learn; its plain
  `attack` when none fits. Reach counts from its body's edge at its own size, the same
  for its brain and its wind-up (`Creature::edge`): it stops to strike only where a move
  fits, and closes in across a gap between its moves' reaches.
- `phases`: health shares it changes at. It rears for `hold` seconds, its blow on the way
  is forgotten, and players within 32 blocks are told (`boss.NS.NAME.phase.N`).
- What its body shows travels as the creature's `tell` (`CreatureView::tell`): the move it
  winds up (its `moves` from 1, 0 its plain attack), `REARING` into a phase, or
  `STAGGERED` while a stagger holds it (any creature's, a boss's too); neither of the last
  two is a move.
- `mend`: health it regains every `every` seconds once `after` seconds pass without a
  blow, unless a block with a `fire` tag burns within `fire_reach` or, when `daylight` is
  set, the sky over it is open and the day is that bright.
- A boss never flees, a perfect block staggers it for `boss_stagger` (combat.ron), a daze
  does not hold it, and a ward stops only `boss_ward` of its blows (prayers.ron).
- A boss always fights whoever comes near: RuneScape's tolerance (a player settled ten
  minutes in one place) and its level rule (a player of more than twice its level) leave
  other hunters' prey be, never a boss's.
- The journal's Bosses page lists the bosses seen; the music plays a boss's track while a
  boss is within 32 blocks or a block of a boss's room (`music.ron`'s `rooms`: the block and
  whose room it is) is near. Which track is the boss's own: its creature's `music` tags
  (the Rootmother's `forest`) choose among the boss tracks by their tags, wherever she is
  met, never another biome's; untagged boss tracks (none yet) are every boss's fallback.
  `every_boss_has_a_fight_track_of_its_own` checks each boss has one.

## Charges

Any attack may be a charge (`charge: (speed, distance, stun)` on the attack,
`creature::moves::Charge`). Its wind-up shows as usual; then the creature runs straight
the way it faced, its aim locked, at `speed` blocks a second for up to `distance` blocks,
held from thinking until the run ends (`fight::Charging`, `Server::charge_ran`). A charge
fits only a target level with it (`Creature::level`): at a player down a hole it walks to the
rim and uses its other moves, rather than run over them. It does not
turn and does not look where it goes, so the land answers it, for any charger:

| Rule | Test |
| --- | --- |
| It comes down on the first player in its way, judged as any blow (a dodge passes it), and stops there; one who stepped aside it runs past | `a_charge_runs_straight_and_strikes_the_first_player_in_its_way` |
| A guard with `brace` (combat.ron: the spear's) raised facing it stops it on the point, heavy or not: the bracer's max hit and a counter's share more, sure to land, the player takes nothing, and it stands dazed `stun` seconds | `a_braced_spear_stops_a_charge_on_its_point` |
| A wall two blocks high (or one it makes no way against) dazes it `stun` seconds; a single step it hops | `a_charge_into_a_wall_dazes_the_charger_and_hops_a_step` |
| It runs off edges: a fall past survival's `safe_fall` costs it `feel.charge_fall` (a tenth) of its most health a block; a pit two deep holds it | `a_charge_off_a_cliff_hurts_and_a_pit_holds_the_charger` |

A player up a tree or on a ledge more than its height and a half above it is out of its
reach already (the brain's strike needs the target that close in height).

## Old Tusk

Nomad age. Level 10, health 60, defence 6, weak to stab. Roams Forest, Mixed Forest, Old
Growth Forest and Sagebrush Steppe by day, alone among the boar herds (rarity Rare, as the
spawn factors place it). Passive: he grazes and ignores players until struck, then fights to
the end. His fight plays the forest's boss tracks, but only once he is in a fight (winding
up, charging or hurt: `Creatures::stirred`), never while he grazes near.

| Move | Phase | Damage | Reach | Wind-up | How to answer |
| --- | --- | --- | --- | --- | --- |
| Gore | both | 1-3 stab | 1.2 | 0.55 s, a nod | block |
| Charge | 1 | 2-5 crush, heavy | from 3 to 12 | 1.0 s, a crouch | step aside or dodge; brace a spear; stand before a tree, a wall, a pit or a cliff |
| Frenzy | 2 | 2-5 crush, heavy | from 3 to 14 | 0.75 s, a crouch | as the charge, quicker (11 a second, 20 blocks) |

Below half he is maddened for 1.5 s, and his charges come quicker. Each charge dazes him 3 s
(2.5 in the frenzy) when it meets a braced point or a wall: the moment for free blows.

At the Nomad age's levels (Attack 5 to 10, a flint spear or stone sword at most 3 a blow)
he takes about thirty blows; a braced spear deals 4 a charge and buys three seconds, and a
ten-block cliff takes 42 of his 60. A truffle in hand tempts him, to lead him where you
want him (his `favourite`).

Loot: a great tusk (with a stick, a tusk club: Attack 5, a mace, max hit 5), 3-5 raw pork,
2-3 hide, 1-2 lard, 2-3 bone, 2-5 gold, 1 in 9 a flint spear, and 1 in 65 the Charge Charm
(very rare): worn in the charm slot, Tusk Stride, 5% faster. Hunter XP 60.

The flint spear (Crafting 2: a flint, two sticks) is the Nomad's spear: Attack 1, max hit 3,
reach 3.8, and it braces.

The gate's Tusk step summons him on the pad: drawn, no fight music while he grazes, struck
with a flint spear until he fights back, then from eight blocks off he winds up his charge,
its tell shown, with his fight music on; every exit sweeps him away.

## The Rootmother

Level 22, health 120, defence 14, weak to slash.

| Move | Phase | Damage | Reach | Wind-up | How to answer |
| --- | --- | --- | --- | --- | --- |
| Lash | 1 | 1-4 slash | 2.0 | 0.6 s, a nod | block |
| Grasp | both | 3-7 crush, heavy | 2.6 | 1.1 s, a crouch | dodge |
| Thrash | 2 | 2-5 slash | 2.2 | 0.45 s | block, quicker |
| Volley | 2 | 1-3 ranged | 12, from 3.5 | 0.8 s, she rears | close in, or block |

Each of her blows is told by ear as well as by her body (`combat_fx.ron`'s `tells`, client
only): a cue that swells to its loudest as the blow lands, the music dipping under it. The
lash and the thrash creak and crack like a whip; the grasp groans up from under the ground
and lands with a thud, and draws a ring on the ground as far as it reaches, filling in as
the wind-up runs out (it goes through a guard, so the ring says how far to dodge); the
volley rattles. Every boss blow must have a tell: the client refuses to start without one.

Below half health she rears for 2 s into phase two. She mends 2 health every 2 s, 4 s
after a blow, unless fire burns within 4 blocks or the sun is on her. The answer the quest
hints at: light a campfire or bring torches into the hollow.

At the Farm age's levels (Attack 12, Defence 10, a copper sword at most 5 a blow) she
takes about 40 blows; with a fire lit the fight is about two minutes, and without one her
mending outpaces a careless player. The sling and bow reach her, but keeping away draws
the volley in phase two.

Loot: 3 heartroot (eaten: heals 1, and Heartwood for 10 minutes, a fifth more maximum
health and a quarter faster healing), 6-12 gold, 2-4 bone, a third of the time potatoes and
cabbage seeds, and a 1 in 16 chance each of a copper helmet or copper shield: gear the age
already makes, never a skip ahead. Hunter XP 120.

Quest: The Rootmother (Farm, combat), after Seasoned Fighter and Copper Arms: Attack 12 and
Defence 10, then her death. Its text names the four biomes her hollows lie under
(`the_rootmother_quest_names_every_biome_her_hollows_lie_under`).

## Interactions, each with a test

| Rule | Test |
| --- | --- |
| Fire within 4 blocks stops her mending; so does the sun on open sky | `fire_and_the_sun_stop_the_rootmother_mending`, `the_rootmother_mends_unless_fire_or_the_sun_stops_her` |
| Keeping away in phase two draws the volley (sling, bow) | `the_rootmother_takes_turns_by_phase_and_distance`, `the_rootmother_rears_into_her_second_phase_and_volleys_at_range` |
| A ward prayer stops half a boss's blow | `a_ward_stops_half_a_bosss_blow` |
| No wild creature is born in a lair's quiet | `a_lair_wakes_its_boss_and_rests_once_she_falls` |
| A boss never flees | `a_boss_fights_to_the_end` |
| A boss fights a settled or strong player | `a_boss_never_tolerates_a_player`, `hunters_attack_in_reach_but_leave_strong_and_settled_players_alone_unless_a_boss` |

Pets and companions do not fight yet, so they have no rule here.

The gate's Boss step summons her on the pad: drawn, the boss music (a track of her own,
checked against her tags), a move's tell; every exit sweeps her away.
