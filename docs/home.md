# Home: the ladder of comfort

Oct 13, 2026. Mariano's ask for the first two ages: upgrades that make the player's home feel
cosy and more useful over time, so progress is felt quickly. (The other half, quick gathering
upgrades, belongs to the early-ages quests thread.)

## What it is

A player's **home** is their lit Homestead Hearth, else the bed they last slept in (so a
Nomad's camp counts before the hearth is lit). Effects can ask about it (`effects.ron`, the
engine's `effect.rs`):

| Condition | Holds when |
| --- | --- |
| `AtHome` | the player stands within `home_reach` blocks of the home (and `home_up`/`home_down`) |
| `Indoors` | something solid is overhead |
| `HomeHas(tag, n)` | at home, with at least n blocks carrying the tag around it |

The server counts, once a second and only for a player at home, the tags effects ask for
(`EffectRegistry::home_tags`, read off the effects themselves) in the box around the home
(`core/survival.ron`: `home_reach` 7, `home_up` 4, `home_down` 2). Nothing is hard-coded: a
mod adds a step or a tag by adding an effect, and a tag no block carries is refused at launch.

## The ladder (base/effects.ron)

Each step asks for all the one below does, replaces it in the HUD, and says so in chat when
first reached in a session (with a **Next:** line). Reaching one counts as
`Built("home_NAME", 1)` for quests (`home_camp`, `home_hearthside`, `home_snug`, `home_homely`,
`home_cosy`, `home_heart_of_the_home`).

| Step | Age | Needs (tags) | Gives while at home |
| --- | --- | --- | --- |
| Camp | Nomad | a bed (sleeping bag) slept in, 1 light | heal x1.2 |
| Hearthside | Nomad end | the lit hearth | heal x1.33, warmth +4 |
| Snug | Nomad end / Farm start | roof, hearth, bed, 2 lights | heal x1.5, warmth +6 |
| Homely | Farm | + 2 seats, 1 table, 2 storage, 3 lights | + xp x1.05 |
| Cosy | Farm | + 2 rugs, 2 curtains, 1 cushion, 3 crockery, 4 lights | heal x1.75, xp x1.1 |
| Heart of the Home | Farm end | + books, cooking, workshop, banner | heal x2, xp x1.125 |

Seats are stools, chairs, benches and cushions; tables include desks and counters; storage is
barrels, crates, barrel racks, jars and baskets; cooking is the cottage stove and cauldron;
workshop is the loom frame, spindle stand and quern. All are craftable in the first two ages
(Construction 1 to 8, Crafting 2 to 8), so each step is a few minutes of gathering and
crafting, not a grind.

## Sleeping

Right clicking a bed used to only set where you wake. It now also gives a **rest** (the
`sleeps` rules): Rested (10 minutes) anywhere, Well rested (15) if the bed is in a Snug home,
Deep rest (20) in a Cosy one (a game day is 20 minutes). Rest gives a little heal, xp,
mining and speed wherever you go, and you cannot sleep again until it wears off ("You are not
tired yet"), so it is a once-a-day habit, not a button to hold. Timed effects count down
online only.

## Numbers and tuning

All of it is data. Multipliers stack with food buffs; xp stops at `MAX_XP_BOOST` (1.25).
Home tiers use `heal`, `warmth` and `xp` only: a location effect that changed maximum health
would cost health every time the player stepped out.

## Quest hooks (written, in base/quests.ron)

`Built("home_camp", 1)` and the other steps are real goals (a step is tallied as
`base:home_NAME`, and quests write it short). The home branch: Make Camp (Nomad: sleep in a
bag with a torch beside it); Snug for Winter, A Homely Home, A Cosy Home and Heart of the
Home (Farm), each with routes that also accept any higher step, because a player who first
reaches a higher step never gets the lower one announced; they come after the craft quests
that make the pieces (seats, a table, shelves, rugs, crockery, candles) and A Stove and a
Quern, Books and Banners, the two parts the last step needs. Hearthside is the Homestead
Hearth quest itself.

## Not built (ideas)

- A `/home` command listing what is still missing for the next step.
- Real storage (extra pack slots or a home chest) as a tier perk: needs the inventory and
  protocol to take a slot count.
- Passing the night by sleeping, and a visible Rested icon (effects use the plain buff icon).
- Sleeping, and the ladder, for villagers' houses.
