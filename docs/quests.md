# Quests

Oct 7, 2026. The quest engine (the progression plan's P20): quests as content, checked
against what a player has done, so they complete retroactively. The quest book screen
(P21) is the interface's; this is its data.

## Rules

- Nothing is locked behind a quest. Recipes stay open; quests point the way and reward
  the walk with experience, items and (later) knowledge.
- A quest completes the moment one of its tasks holds, however long ago the work was
  done: the goals read the player's state, not a log of events since the quest appeared.
- A task is a list of goals that must all hold; a quest offers one task per route (craft
  it, find it, trade for it).
- `after` names the quests that come first; `"a|b"` takes either. Finishing one quest can
  finish others that were already done, in the same moment.

## Content

`base/quests.ron` (engine `quest.rs`): name, age, branch, title, why (the reason and the
reward, shown first), tasks, after, rewards. Goals:

| Goal | Reads |
| --- | --- |
| `Craft(item, n)`, `Cook(item, n)` | the player's tallies of things made and cooked |
| `CookAt(item, station, n)` | successful cooking tallies for a station tag; First Night asks for edible food at a campfire, plus crafting the campfire |
| `Built(structure, n)`, `Lit(structure, n)` | standing construction and successful fuelled lighting, separately; factory construction never supplies lighting credit. `Built` also takes the announced steps of an effect ladder, written short and tallied in full: `Built("home_snug", 1)`, `Built("knack_practised_hands", 1)` (the effect's `ladder` and name; docs/home.md, docs/status-effects.md) |
| `Tame(creature, n)` | the tally of animals tamed |
| `Catch(fish, n)`, `Catch("Kinds", n)` | the journal's Fish page |
| `Kill(creature, n)` | the journal's Animals and Monsters pages |
| `Pick(plant, n)`, `Mine(ore, n)` | the journal's Plants and Ores pages |
| `Visit(n)`, `Discover(page, n)` | how many entries the journal has (Animals and Monsters entries are only found from close up, see [journal.md](journal.md)) |
| `Kinds(page, which, n)` | how many different entries of a page were done at least once (killed, caught, picked, mined; only seen does not count), among `any`, a `tag:TAG` or one name: Wild Harvest is `Kinds(Plants, "any", 5)` |
| `Level(skill, n)` | the skill's level |
| `Have(item, n)` | what the player carries, pouches too |
| `Home` | a lit Homestead Hearth |
| `LookUp(item)` | the recipe viewer was opened on the item (the client says so once, `ClientMessage::LookedUp`; tally `lookedup:<item>`) |
| `Thriving(n, percent)` | the player keeps n animals right now at a condition of at least that percent (read live from the kept animals, not tallied; a newly tamed one starts at 60) |
| `Met(kind, n)`, `Met("Kinds", n)` | dealings with wandering parties (docs/wandering.md), tallied `met:<kind>`: a line said that gave or took something, a show watched, a share of a hunt, a captive's or a guided one's thanks, a lure followed to its end; a greeting alone is none. `any` counts every kind; `Kinds` counts the kinds dealt with at least once |
| `Prayed(prayer, n)` | prayers turned on (`prayed:<prayer>`); one already on is not turned on again |
| `Offered(item, n)` | what was offered at an altar (`offered:<item>`), an item, `any` or a `tag:TAG` |

Items and creatures may be `any` or `tag:TAG`. Rewards are `Xp(skill, xp)` and
`Item(item, count)`. Titles and reasons are translated as `quest.<package>.<name>` and
`quest.<package>.<name>.why`.

## On the server

Each second the server checks every player's open quests, finishes those whose task
holds, tells the player with a chat line, and sends `ServerMessage::Quests { states }`
when any state changed: each quest's name, whether it is done, whether it is claimed, and
each goal's progress as (have, need). The client keeps them in `LocalPlayer::quests`.
Quests done, quests claimed and the tallies are saved with the player.

## Claiming

Finishing a quest pays nothing by itself. A done quest with rewards waits in the quest
book: it glows gold on the web, its chapter shows a gold "!", the chat says so, and its
page has a Claim button (`ClientMessage::ClaimQuest`) beside the rewards, which list the
experience and the items. The server then gives the items and the experience once. A quest
that pays nothing is claimed as it finishes. If the pack cannot hold every item, nothing
is given, the player is told, and the quest stays claimable; the reward never drops on the
ground. A quest that is not done, was claimed, or does not exist is ignored.

Unclaimed rewards persist: the save lists the claimed quests (`quests_claimed`). A save
from before claiming has no list, and counts everything it finished as claimed, since all of
it was paid then.

The sounds: a chime as a quest is finished and a softer ring as it is claimed, played by the
client as the server's states change (not for the states a session starts with). They are
synthesized (`ui/sound.rs`) unless a package's `sounds.ron` lists `quest_done` or
`quest_claim` recordings, which replace them; they follow the sound volume, not the choice
of click sounds.

The Nomad and Farm chapters (ages 1 and 2) have the quests the game can already do:
Nomad runs from the first campfire to the lit Homestead Hearth, Farm from the first harvest
and the pit kiln to the lit Crucible Kiln. Each has at least thirty quests
(`crates/engine/tests/early_quests.rs` keeps that floor and checks every name they use). The
loop through the first days, and what a new player is and is not told, is in
[first-days.md](first-days.md). The rest of quest book v2 joins as its content lands.

The Crucible Kiln needs a standing pattern and a complete charge held in the hand:
32 charcoal or 24 coal. Right-clicking its core lights it and consumes that charge.
Its burn time uses the fuel times in `machines.ron`, measured by server ticks; the
remaining fire is saved in `world.ron`. Cooking needs both the standing pattern and a
live fire. Removing a required piece quenches it; rebuilding requires a new charge.
Completed quests in older saves stay completed. Older `cooked` and `built` tallies do
not establish which station cooked food or whether fuel was paid, so they cannot
complete these stricter goals for an unfinished quest.

A package's `tracker` may name `start_pins`: quests pinned for a player whose book is new
(nothing done, nothing pinned), once per install (remembered in the settings). A later package replaces the list.

## Multi-part quests

A task is a list of goals that must all hold, so one quest can ask for a gather, a craft
and a cook together. Quests that were a single count are two-part where the second part is
what the player does next anyway (Oct 13, Mariano: "a bit too simple"): Small Game (kill,
then cook one), Hide and Seek (hides and bones), Rod and Line (craft, then land a fish),
A Fish Supper (catch three, cook three), Meat on the Fire (six cooked, Cooking 2), Sharpening
Up (Attack 4 and five kills), First Harvest and Carrot Patch (harvest, keep the crop), A
Field of Gold (24 harvests, Farming 4), Herb Garden (planter and six plants), Seed Mix (the
mix and the wheat to make it from), Fresh Eggs (fry the first egg, then keep five), A Woolly Flock (tame,
and shears made), The Milk Pail (the pail, then the milk), A Silvery Ore (tin and copper from
one seam), Many Kinds (three kinds, six cooked). The main line of each age (the pacing
test's milestones) stays as it was. Rewards are unchanged.

## Quests that meet the rest of the game

Oct 13: twenty-two quests tie the first two ages to what came after the quest book was written,
so that what the game already does is something the book points to:

- **Wanderers** (`Met`). Nomad: Folk of the Road (deal with anyone), A Kind Turn (feed the lost
  child or the hurt hunter), Something for Something (a pedlar, tinker, berry pickers,
  woodcutters or charcoal burners). Farm: Farmers on the Road (the harvest family, goose
  girl, cowherd, water carriers or shepherd), Safe Home (refugees, the prison convoy or the
  hurt traveller) and Many Travellers (five kinds). Each route names a kind, so the hint is
  true; parties are rare, so the routes are alternatives rather than a list.
- **Home rooms** (`Built` on a room's step). Nomad: A Bench to Work At, Pantry Shelves,
  Company at Camp. Farm: A Proper Workshop, A Larder, A Kitchen Garden, A Full House. Each
  room's first step is Nomad's, its second Farm's, as `effects.ron` lays them out.
- **The altar** (`Offered`, `Prayed`). Altar Rites (three bones offered and Bark Skin turned
  on) and A Prayer in the Fight (Ember Arm and Bark Skin on, ten creatures defeated).
- **Cooking**: A Varied Table (meat, fish and bread together).
- **Batch 9 wanderers** (`Met` on one kind each). Nomad: To Market (sell wheat to the
  market folk on the road, or walk with them to town: the arrival counts too, but only for
  those who kept them company) and Caught in the Rain (warm the drenched travellers with a
  torch, or share food). Farm: Neighbourly (the neighbours call once the hearth is lit),
  The Rain Dance (watch the rainmakers or pay them), Filled Again (the shrine bearer) and A
  Share of the Hunt (strike first at a hunting party's, trapper's or wolf hunters' quarry).
- **Wanderer quests in two parts** (the hooks in docs/wandering.md, Oct 13). Each pairs a
  wandering kind with what the player does next anyway, so meeting them is a side step,
  never a wait. Nomad: Coal by Road (the coal carters or the benighted miners, and sixteen
  torches; pays four coal). Farm: Fired Whole (the clay diggers fire your clay, and
  twenty-four fired in all; pays twelve clay), The Last Sheaf (the harvest home procession,
  and twelve wheat brought in; pays eight wheat seed) and Mice in the Stores (follow the
  piper to the end of his tune, and a pantry or larder at home; pays four seed mix).
- **Wanderers that come back** (the Nomad thread's hooks, Oct 13): quests that ask for a
  kind and the kind that follows it, all Farm side branches. The Keepers' Word (the
  poacher, the gamekeepers on his trail and, days later, his gang), Catch Up the Shrine (the
  shrine bearer and the stragglers asking after it), From Wedding to Christening, Those
  Taken In (refugees, then their kin), First Metal (the bloomery smelters before the
  Crucible Kiln, and four copper ore) and A Hen of Your Own (the henwife's hen, kept near
  home). Three more from batch 16: Master and Journeyman (Farm: the apprentice
  who asks to learn your best skill at 15, then the journeyman), The Market Road (Nomad,
  after To Market: the market folk, then beat the highwaymen's ambush; the win is the
  dealing their tally counts) and Found and Thanked (Nomad, after A Kind Turn: the lost
  child brought to town, then the parents who come to your home).

## Quick upgrades

Oct 13 (docs/first-days.md, "Quick upgrades on common resources"): Timber asks for four logs
by hand, the planks for the table, sticks and both wooden tools, and An Axe to It (after
Crafting Table) asks for the wooden axe and eight logs chopped with it, so the first tool
upgrade comes in the first minute. First Night tells the player the stone axe hits three
times as hard as a fist.

## Structure (Oct 13)

A pass on the Nomad and Farm chapters against the way a long modpack's quest book is laid
out: each chapter a spine of main quests to one goal, side branches hanging off it, nothing
on the spine gated by a side quest, and a quest that is several steps shown as several.

| Check | Before | After |
|---|---|---|
| A clear goal per chapter | Nomad's goal Homestead Hearth ("the Farm age opens"); Farm's goal the lit Crucible Kiln ("starts the Bronze age"), with First Bronze after it as the bridge to the next age | Unchanged: both goals are the last spine quest their text names as the age's end |
| Side branches read as optional | No spine quest comes after a side quest (the_potter, brick_by_brick and crucible_kiln come only after spine quests or the hearth); the wanderer quests say "may" | Unchanged |
| Nothing unreachable or a dead end | Every `after` names a quest that exists (load-checked); no Nomad quest waits on a Farm one; no Craft or Level goal asks more than Engineering 10 (The Crucible). A Place to Pray stood in the Farm tab with no `after`, so it could finish before the Farm age opened. Stone in Hand had no `after`, so the spine read as three loose starts | A Place to Pray comes after Homestead Hearth like the rest of its tab; Stone in Hand after Crafting Table, so the Nomad spine reads Crafting Table, Stone in Hand, then First Night's line (First Night stays pinned at the start, with no `after`) |
| Several steps shown as several | Homestead Hearth was one goal (`Home`) for a hearthstone, two chimneys and the lighting | Three parts: the hearthstone, two chimneys, and the lit hearth (`Home`). The book ticks each as it is made |

Left as they are: Timber stays off the spine (branch `start`, with An Axe to It), since the
main-line walk test models only the spine's goals; Crafting Table stays a root so a player
who already has a table finds it done.
