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
| Bedside | a bed, a seat, 2 lights (no roof needed) | 2° warmer, and sleeping there is a Well rested sleep |
| Herb nook | 2 hanging herbs (`drying`), 1 storage | farming and alchemy count 1 level higher, a meal's good lasts x1.1 |

Bedside is a Nomad's first comfort: a stool (Construction 1) and two candles (Crafting 3)
beside the sleeping bag. Before any roof or hearth it is the warmer night and the better rest
(Well rested, 15 minutes, instead of Rested's 10); a snug home gives the same rest, and two
ways to one rest give it once (`given_by_sleep` keeps each effect once, its longest rule).
The herb nook is the Farm age's: two herbs tied with twine hang to dry (Crafting 4, two
bunches); two bunches and a jar, basket or crate make it. Its farming level counts where it
is felt, sowing the kitchen garden at the door (a crop a level early) and harvesting it, and
its alchemy level at the bench.

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
| Bee garden | a skep of bees (`hive`) and 3 flowers within reach | heal x1.1; the hive's bees make crops within 8 blocks grow x1.25 (docs/bees.md) |

How they work, for mod authors: `Condition::Sheltered(inner)` holds under a roof while `inner`
(a weather condition, which is normally true only out in the open) holds in the open, so the
weather is judged as the sky sees it, not as the player's roof lets them feel it. The home tag
`kept` is not a block tag: the survey counts the player's own kept animals (`owner_id`) whose
position is inside the home box (`survey_around(anchor, keeper)`); a mod can ask
`HomeHas("kept", n)` like any other tag, and content load accepts it. `crop` is the block tag
every crop stage carries, so a garden needs no new tag. Other players' animals never count.

Ideas left: species rules (`kept:hen` for eggs, a cat for fewer mice).

## Callers (wandering parties that come to a home)

A wandering kind can ask how far a player's home has come: `needs.home_is` names a home
step (any effect with a condition), judged at the home as `/home` judges it (its survey, its
roof), so it works wherever the player stands near home. With `near_home` it brings callers:

| Kind | Comes to | What it brings |
| --- | --- | --- |
| Neighbours | any lit hearth (batch 9) | a plate for the house; seed for a cooked meal |
| Fireside guests | a Homely home, of an evening | a long talk (Focused, 15 minutes); for a cooked supper, two candles toward Cosy |
| A hedgehog at the door | a Cosy home, by night | a wild hedgehog after the crumbs: tame it (Hunter 3) and it is one of the animals about the house (Company) |
| Beekeeper | a home with a Bee garden, by day | walks the garden (crops near grow a stage, Farming XP); a spare skep for three honey |

They are wandering kinds like any other (docs/wandering.md), rare, one at a time.

## Home perks (know-how kept at home)

A finished home quest teaches something that stays, through `Done("quest")` (see
`docs/early-upgrades.md`), and it works where it is used: `AtHome` and the furnishing it is
about. They are effects with `ladder: "homecraft"` (announced once in chat, countable by
quests as `Built("homecraft_NAME", 1)`, and left out of `/home`, which tells only the home's
own steps):

| Perk | Quest | Where it works | Gives |
| --- | --- | --- | --- |
| Banked fire | Make Camp | at home, a hearth or heat block | 3° warmer |
| Snowed in | Snug for Winter | indoors at home while snow or hail falls outside | heal x1.15, learn x1.1 |
| Everything in its place | A Homely Home | at home with 2 storage | construction +1 level |
| Home cooking | A Stove and a Quern | at home with a stove or cauldron | meals heal x1.1, last x1.1 |
| Studious | Books and Banners | indoors at home with a bookshelf | learn x1.05 |
| Hearthkeeper | Heart of the Home | at home | crafting and cooking +1 level |

They stack with the ladder, the rooms and the reactions: a Snug home in snow with Snug for
Winter done gets the ladder's heal, Snowed in's and, with a fire, Sheltered's. Rooms are
steps too: `Built("home_workbench", 1)`, `home_larder`, `home_company`, `home_kitchen_garden`
and the rest work as quest goals, written short and tallied in full (`base:home_larder`).

## `/home`

`/home` tells where the home is and, for each chain of steps (the ladder, and with the rooms
the workbench and the larder), the highest step reached and what the next one lacks, judged
as if you stood in it: "Camp reached. Next, Hearthside: it needs hearth 0/1." What a step
lacks is read off its own condition (`Condition::lacks`), so a mod's steps are covered with no
extra work; a block tag shows by `home.tag.<tag>` in the language files, else as the tag.
It is a console command, so it needs no protocol change.

## The way home

Away from home (the lit hearth, else the bed last slept in), a small hearth mark at the top
of the screen sits over where home lies, with how far it is ("Home 140 m"). Turned away from
it, the mark waits at the screen's edge on the side to turn to, so a player lost at night
turns until it is in the middle and walks. Within `home_hidden_within` blocks (core
`interface.ron`, 24) nothing shows; the mark's icon is the item a package names as
`home_icon` (base: the hearthstone). The server sends home in the player's vitals
(`Vitals::home`, protocol 38) once a second when it changes. With no hearth lit and no bed
slept in, there is no mark. The gate's Home step checks it after the camp: the bag named as
home and the mark showing its distance.

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

## Smoke over home

The Homestead Hearth's chimney smokes from its top while the hearthstone stands below it, a
thick column seen from 96 blocks, so a player out at dusk sees where home is from across a
valley. It is data on the chimney block (blocks_tools.ron): a smoke emission with
`range: Some(96.0)` (how far off it is seen, up to 128, against the 24 every other block's
particles keep) and `over: Some(("hearth", 3))` (given off only with a `hearth` block up to
3 below and one across, and only from the top of a stack, with open air above). A mod's
stove pipe or a forge's flue can do the same. The client searches farther only for chunks
whose palette holds such a block, and counts nearness as a share of each emission's range,
so a far chimney takes its place among the nearest torches
(`a_chimney_smokes_over_a_hearth_from_afar`). The gate's Cook step sets a hearthstone and a
chimney beside its campfire and counts the chimney's puffs ("chimney smoke").

## Quest hooks (written, in base/quests.ron)

`Built("home_camp", 1)` and the other steps are real goals (a step is tallied as
`base:home_NAME`, and quests write it short). The home branch: Make Camp (Nomad: sleep in a
bag with a torch beside it); Snug for Winter, A Homely Home, A Cosy Home and Heart of the
Home (Farm), each with routes that also accept any higher step, because a player who first
reaches a higher step never gets the lower one announced; they come after the craft quests
that make the pieces (seats, a table, shelves, rugs, crockery, candles) and A Stove and a
Quern, Books and Banners, the two parts the last step needs. Hearthside is the Homestead
Hearth quest itself. The rooms have their own: A Bench to Work At, Pantry Shelves,
Company at Camp and A Lit Corner (Nomad), A Proper Workshop, A Larder, A Kitchen Garden,
A Full House and A Herb Nook (Farm). Callers at the Door (Farm, after A Homely Home) asks
for a supper or a talk with fireside guests.

## Not built (ideas)

- Real storage (extra pack slots or a home chest) as a tier perk: needs the inventory and
  protocol to take a slot count.
- Passing the night by sleeping, and a visible Rested icon (effects use the plain buff icon).
- Sleeping, and the ladder, for villagers' houses.
