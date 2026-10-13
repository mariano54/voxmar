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

## Rooms (side branches)

Beside the ladder, a home can grow a use. They come on at home beside the ladder's step, each
announced in chat with a **Next:** line, and the better one hides the other:

| Room | Needs (tags) | Gives while at home |
| --- | --- | --- |
| Workbench | crafting table, a seat, a light | crafting and construction checks count 1 level higher (`boosts`) |
| Workshop | + roof, 2 lights, a loom, spindle stand or quern, 2 storage | crafting and construction +2, engineering +1 |
| Pantry | 2 storage, a hearth or any heat block | the good effects of a meal last x1.25 (`meal_time`) |
| Larder | + roof, 4 storage, a stove or cauldron, a table | meals also heal x1.2 (`meal_heal`) and last x1.5 |

`boosts` are the effect vocabulary's own (they stop at +6 and recipes marked firm ignore
them); `meal_heal` and `meal_time` are two new modifiers, applied as the meal is eaten from
the modifiers of the last second. Raw meat's queasiness is not lengthened.

## Reactions (what a home does with what is around it)

A home also answers other content, as plain effects with a condition, no code of their own:

| Reaction | Needs | Gives while at home |
| --- | --- | --- |
| Sheltered | indoors, a fire (hearth or heat block), and rain, snow, hail, dust or a storm falling outside | heal x1.2, learn x1.05 |
| Company | 1 animal the player keeps within the home's reach | heal x1.1 |
| Full house | 4 such animals (hides Company) | heal x1.2, learn x1.05 |
| Kitchen garden | 4 crop plants (any stage) within reach | meals heal x1.1 (`meal_heal`, stacks with a larder) |

How they work, for mod authors: `Condition::Sheltered(inner)` holds under a roof while `inner`
(a weather condition, which is normally true only out in the open) holds in the open, so the
weather is judged as the sky sees it, not as the player's roof lets them feel it. The home tag
`kept` is not a block tag: the survey counts the player's own kept animals (`owner_id`) whose
position is inside the home box (`survey_around(anchor, keeper)`); a mod can ask
`HomeHas("kept", n)` like any other tag, and content load accepts it. `crop` is the block tag
every crop stage carries, so a garden needs no new tag. Other players' animals never count.

Ideas left: species rules (`kept:hen` for eggs, a cat for fewer mice), wandering visitors
who stop at a lit home (needs hooks from the wandering events work), a herb nook.

## `/home`

`/home` tells where the home is and, for each chain of steps (the ladder, and with the rooms
the workbench and the larder), the highest step reached and what the next one lacks, judged
as if you stood in it: "Camp reached. Next, Hearthside: it needs hearth 0/1." What a step
lacks is read off its own condition (`Condition::lacks`), so a mod's steps are covered with no
extra work; a block tag shows by `home.tag.<tag>` in the language files, else as the tag.
It is a console command, so it needs no protocol change.

## Sleeping

Right clicking a bed used to only set where you wake. It now also gives a **rest** (the
`sleeps` rules): Rested (10 minutes) anywhere, Well rested (15) if the bed is in a Snug home,
Deep rest (20) in a Cosy one (a game day is 20 minutes). Rest gives a little heal, xp,
mining and speed wherever you go, and a bed always works as a bed (it sets where you wake and your home) but gives no new rest
while one lasts, so the buff is a once-a-day habit, not a button to hold. Timed effects count down
online only.

## Numbers and tuning

All of it is data. Multipliers stack with food buffs; xp stops at `MAX_XP_BOOST` (1.25).
Home tiers use `heal`, `warmth` and `xp` only: a location effect that changed maximum health
would cost health every time the player stepped out.

## Quest hooks (for the quests thread)

`Built("home_camp", 1)` ... as above. Suggested chapters: Nomad "Make Camp" (sleep in a bag
with a torch beside it), "Hearthside" (light the hearth, already `Home`), "Snug" (roof it);
Farm "Homely", "Cosy", "Heart of the Home", each a multi-part quest (craft the pieces, then
reach the step).

## Not built (ideas)

- A herb nook (hanging herbs, jars) boosting Alchemy and Farming: `hanging_herbs` is only a
  village prop today, with no recipe.
- Real storage (extra pack slots or a home chest) as a tier perk: needs the inventory and
  protocol to take a slot count.
- Passing the night by sleeping, and a visible Rested icon (effects use the plain buff icon).
- Sleeping, and the ladder, for villagers' houses.
