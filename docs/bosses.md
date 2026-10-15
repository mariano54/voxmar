# Bosses

A boss is a creature with `boss: true` that lives in a lair and nowhere else. The first is
the Rootmother, late in the Farm age (Oct 14). Everything below is data: a pack can add a
boss with a creature, a lair and a quest, and replace hers.

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
