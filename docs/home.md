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

Each step's chip has its own icon, so the HUD shows the home climbing at a glance: a tent
(Camp), a hearth fire (Hearthside), a quilt (Snug), a house (Homely), its window lit and its
chimney smoking (Cosy), a heart at its middle (Heart of the Home); and a moon for Rested, a
moon and star for Well Rested, a full moon for Deep Rest (default pack
`recipes/effects.ron`, `ui/effect/<name>`; test `the_homes_steps_and_rest_have_their_own_icons`;
the gate's Home step, "home: icons", shot `home-chips`).

| Step | Age | Needs (tags) | Gives while at home |
| --- | --- | --- | --- |
| Camp | Nomad | a bed (sleeping bag) slept in, 1 light | heal x1.2 |
| Hearthside | Nomad end | the lit hearth | heal x1.33, warmth +4 |
| Snug | Nomad end / Farm start | roof, hearth, bed, 2 lights | heal x1.5, warmth +6 |
| Homely | Farm | + 2 seats, 1 table, 2 storage, 3 lights | + xp x1.05 |
| Cosy | Farm | + 2 rugs, 2 curtains, 1 cushion, 3 crockery, 4 lights | heal x1.75, xp x1.1 |
| Heart of the Home | Farm end | + books, cooking, workshop, banner | heal x2, xp x1.125 |

Seats are stools, chairs, benches and cushions; tables include desks and counters; storage is
whatever a home keeps things in or on: chests, cupboards and wardrobes, shelves (pot shelves
and bookshelves too), crate stacks, barrel racks, jars and baskets, and a village's barrels
and crates (every store a player opens counts but the feed trough, which is the pen's: test
`every_store_shelf_and_cupboard_counts_as_storage`; the gate's Nest step finds a Pantry made
of a chest and a barrel by a campfire, "home: storage"); cooking is the cottage stove and cauldron;
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
| Herb nook | 2 hanging herbs or hung drying racks (`drying`), 1 storage | farming and alchemy count 1 level higher, a meal's good lasts x1.1 |

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
| Door shut | indoors at home with a shut door in it | heal x1.1, 1° warmer |
| Kitchen garden | 4 crop plants (any stage) within reach | meals heal x1.1 (`meal_heal`, stacks with a larder) |
| Bee garden | a skep of bees (`hive`) and 3 flowers within reach | heal x1.1; the hive's bees make crops within 8 blocks grow x1.25 (docs/bees.md) |

Stations that work while you are away (a drying rack, a compost heap) are in docs/stations.md; a hung rack counts toward the herb nook.

How they work, for mod authors: `Condition::Sheltered(inner)` holds under a roof while `inner`
(a weather condition, which is normally true only out in the open) holds in the open, so the
weather is judged as the sky sees it, not as the player's roof lets them feel it. The home tag
`kept` is not a block tag: the survey counts the player's own kept animals (`owner_id`) whose
position is inside the home box (`survey_around(anchor, keeper)`); a mod can ask
`HomeHas("kept", n)` like any other tag, and content load accepts it. `crop` is the block tag
every crop stage carries, so a garden needs no new tag. Other players' animals never count.

Ideas left: a cat for fewer mice. Hens lay into the home's stores (below, "Eggs in the stores").

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
turns until it is in the middle and walks. The mark's icon is the item a package names as
`home_icon` (base: the hearthstone). The server sends home in the player's vitals
(`Vitals::home`, protocol 38) once a second when it changes, so the way home needs no new
protocol. With no hearth lit and no bed slept in, there is no mark.

**When it shows (Oct 14, thirteenth pass).** The first nights are where a player gets lost.
They wander off at dusk, the hearth is a dot of light among dark trees, and nights stay dark
with no torches before coal. More light is not the answer; the mark is. It used to show
whenever the player was over 24 blocks from home, all day long. Now:

- **Never inside the home.** Within `home_hidden_within` (8) blocks either way (a square, as
  the home's own box is, `home_reach` 7, and a step), nothing shows, corners included.
- **From dusk to dawn** (`home_dusk` 16500 to `home_dawn` 23500 ticks after sunrise, the
  times `/time set dusk` and `dawn` name) it shows everywhere past that. A night can wrap
  past midnight (a dusk later than its dawn).
- **By day** it shows only farther than `home_shown_beyond` (48) blocks. A day spent working
  round the home is not cluttered, and a long trip still has its way back.
- It **fades** in and out over `home_fade` (0.5) seconds instead of popping.
- The **"Way Home Marker"** setting (Interface tab, on by default, en+ja) turns it off.

All five numbers live in core `interface.ron`, where a later package changes what it gives.
They are checked as they load: the distances are 0 or more and the day distance is not under
the inside one; both times are under a day and differ; the fade is 0 to 5 s. The rule is
`hud::home::wanted`, with its own test (`the_way_home_shows_at_night_or_far_off`).

**Gate.** The Home step checks the mark itself after the camp: named home, its distance, the
edge, and gone near home. It does this with the distances set to 0 so it shows at any hour.
The **WayHome** step then plays the rule at dusk. It sets `/time set dusk` and stands on a
pad 20 blocks out, past the box and short of the day distance. It waits, in server and game
seconds both, for the mark to fade in. It turns home 20° to one side and reads back the
bearing the mark draws from where it sits, which must be within 3° of home's
("way home: dusk"). Back at the camp it waits for the mark to fade out ("way home:
inside"). It then sets the time back and leaves the camp to the Nest step, which takes it down.

## Sleeping

Right clicking a bed used to only set where you wake. It now also gives a **rest** (the
`sleeps` rules): Rested (10 minutes) anywhere, Well rested (15) if the bed is in a Snug home,
Deep rest (20) in a Cosy one (a game day is 20 minutes). Rest gives a little heal, xp,
mining and speed wherever you go, and a bed always works as a bed (it sets where you wake and your home) but gives no new rest
while one lasts, so the buff is a once-a-day habit, not a button to hold. Timed effects count down
online only.

A proper bed rests better than a bag on the ground: a sleep rule can name a `bed` tag the bed
slept in must carry, and a bed of furniture (`bed: "furniture"`) is Well rested anywhere,
where a sleeping bag needs a Snug home or a lit corner. Every sleep also mends: Healing
Sleep (heal x4) for a minute, two in a proper bed, so coming home hurt and sleeping heals.
It is no way to heal in a fight: a bed refuses sleep with a creature that hunts players
(wild, or kept by another player) within `sleep_threat_reach` blocks (12), or within
`sleep_after_hurt` seconds (15) of a wound (a blow, a fall, a sting; not the cold or heat
wearing at one) (`core/survival.ron`). A refused sleep still claims the bed. And it stays slower than eating, since food is the heal (design
draft 63): however many heal buffs and prayers are on, healing by itself is held at
`heal_most` (1 a second, `survival.ron`), which loading checks is below what the slowest
food heals a second, a 0.6 s bite at a time; time past a point is never banked to pay out
after. No night skipping. Loading refuses a sleep rule whose `bed` tag no bed carries.
Tests: the engine's `a_proper_bed_rests_well_and_every_sleep_mends_slower_than_food` and
`a_sleep_rule_for_a_bed_nobody_made_is_refused_at_load`, the server's
`a_proper_bed_rests_well_and_mends_longer_than_a_bag` and
`no_sleep_with_a_hunter_near_or_wounds_fresh`; the gate's Home step clicks the bag with a
shambler near and expects the refusal, then waits on every chip the engine says the bag
gives there.

## Drying off

Rain soaks you (Soaked, Drenched in a downpour), and a roof alone no longer dries you: wet
through, you stay so for two minutes under cover (three if drenched), colder and, drenched,
slower to heal, unless you dry off within three blocks of a fire or a hearth (or warmed on a
cold day). So coming in from the rain, you dry by your own hearth. It is data: an effect
that `lingers` can name what `ends` it early (core effects.ron's `ends: Some(Any([Warmed,
Near("fire", 3), Near("hearth", 3)]))`), and loading refuses `ends` on one that does not
linger. Tests: the engine's `wet_through_you_dry_off_by_a_fire_and_not_before`, the
server's `health_is_twice_hitpoints_and_a_downpour_drenches_those_out_in_it`; the gate's
Weather step dries the player by a campfire after its storm ("weather: dries by a fire").

## Sitting by the fire

Seats were only props the ladder counted. Now a right click on one (a stool, chair, bench or
cushion: anything tagged `seat`), with no block to place in hand and room above it, sits the
player on it (`base:sit`, `Requirement::Seat`, `Effect::Sit`): the server sets them in it
(base's seats are not solid, so one under a roof two blocks up is sat on; a mod's solid seat
sets them on top, and wants two free cells over it), and their eye drops to
`vitals::SEATED_EYE` (1.3) over their feet, on the server and the client alike (`vitals::eye`; `Vitals::seated`, protocol
40). They sit until they move off it or it goes (`vitals::still_seated`, checked once a
second). Effects can ask `Seated`; base's **By the Fire** (`fireside`: heal x1.5, 4° warmer)
holds sat with a fire or the hearth within 3, so a Nomad's stool at the campfire and a chair
at home both rest the player after a day out. Tests: server `seats.rs`. The gate's Seat step
sits on a stool under a block two up, by a campfire (shot `seat-fire`), and steps off it.

Sat with a table within 2, the player is **At the Table** (`at_table`): a meal eaten there
heals a quarter more and its good lasts a quarter longer (`meal_heal`, `meal_time`, as the
pantry and larder rooms give; they multiply, up to core `survival.ron`'s `meal_most`, 2: with
every meal effect on, the heal is 1.82 times and the good's length 2.27, held to 2). A supper at the kitchen table is worth sitting
down for. Test: `a_meal_at_the_table_does_more`. The gate's Seat step sets a table by the
stool ("seat: at the table").

## Numbers and tuning

All of it is data. Multipliers stack with food buffs; xp stops at `MAX_XP_BOOST` (1.25).
Home tiers use `heal`, `warmth` and `xp` only: a location effect that changed maximum health
would cost health every time the player stepped out.

## A door to shut

A home keeps the night out with a **wooden door** (Construction 2, six planks of any wood at
a table). It is two blocks tall, and one item places both halves, facing whoever sets it.
Breaking either half takes the whole door and gives it back. A right click opens or shuts
it. Shut, it is solid to players and creatures alike, and no creature opens one. Open, it
stands against its jamb and lets anyone by. It will not shut on anyone standing in the
doorway. Indoors at home behind a shut door, the **Door shut** reaction gives heal x1.1 and
1° of warmth.

How it works, for mod authors: it is all data on the blocks (blocks_furnishings.ron).
`upper` names the block placed above with it, which rests on it (`support: Below`) and goes
with it either way (`supported_by`). `swaps` names what using it turns it into, in the same
facing, its other half with it (`action::swapping`, the `open` action, `Effect::Swap`).
Shut and open are two blocks with their own `solid`, and only the server turns them; the
client is told, never guesses, so nobody walks through a door the server still holds shut.
A mod's gate, hatch or portcullis is the same four lines. The gate's Door step places one,
opens it and shuts it ("door: placed", "door: open", "door: shut").

## Crafting from home's stores

The recipe viewer (U or R over an item) has a **Lay** button on every crafting recipe while
the inventory is open, as NEI's "+" does: pressed, it fills the crafting grid with what that
recipe takes (shift: as many sets as will go), putting what the grid held back in the pack
first. It takes from the pack, and, while the player stands in their home (the box `/home`
counts, `home_reach` across), from every store in it, nearest first: the barrel of planks
by the door and the cupboard of wool are a woodshed and a larder to craft from, with no
trips to fetch. A recipe that needs a table counts only at one; one above the player's
level is passed over; with nothing to lay from, the chat says what is short. Server side in
server/src/laying.rs (`core:lay`, an `ActionTarget::Item`, protocol 39), the grid choice in
`Recipe::lay` (an alternative with enough for every alike cell). Laid with a store open,
the store's screen closes, since its slots stand where the grid is drawn. The gate's Home
step puts four planks in a barrel beside the bag, shoots the table's recipe with its Lay
button ("home-lay-recipe"), presses it, and shoots the grid laid from the barrel
("home-lay"; check "home: lay").

## Eggs in the stores

A kept hen (any bird whose product `nests`) that lays inside its keeper's home box lays into
the home's stores instead of onto the ground: the nearest store that takes an egg and has
room (a barrel, a crate, a jar; not a basket, which keeps only what is foraged). What finds
no room, or is laid away from home, falls on the ground as before, where it lasts 30
minutes. So a pen by the house fills the larder while the keeper is out: no egg hunt round
the yard, nothing rotting while you travel, and the eggs are there for **Lay**, stations and
Put Away to draw on like anything else stored.

The keeper is told once home ("Your animals laid into your home's stores: 3 Egg."), so the
first egg in a barrel is never a mystery; it is told once, then the tally starts again. It
is data: `nests: true` on a product in creatures.ron (base sets it on every egg), refused at
launch on a product taken with a tool (wool, milk). The home box is the survival rules' own
(`home_reach`, `home_up`, `home_down`). Server side in server/src/husbandry.rs (the laying
pass) with `Server::stores_around`; the keeper must be in the world (logged out, it falls). The
gate's Nest step keeps a junglefowl at the Home step's bag, opens a barrel, makes an egg
due, and checks the egg in the barrel and the chat line (shot `home-nest`, check
"home: nest").

## A well by the field

Crops grow best with water within four blocks, and rice not at all without it, so a home
far from a river had no field worth the name. A **Well** (Construction 8, Farm age: a ring of
cobbles about a pail, under sticks and a plank) is one block that waters as water does: any
block tagged `waters` within `WATER_REACH` of a crop, standing on the ground by the field or
sunk level with it, keeps its ground moist (farm.rs `crop_conditions`), and a station's
`Water` spot counts it too, so a compost heap beside a well rots as fast as one by a stream.
A mod's rain barrel or cistern is the same tag. Tests: `a_well_waters_the_field_about_it`,
`a_heap_rots_greens_four_to_one`. The gate's Refusal step sets one down from the hand by its
dry plot ("well", shot "well"). The Farm quest A Well by the Field (after A Field of Gold)
asks for one made and set down where it keeps tilled ground moist (`Watering`).

## The hearth is a fire

Lit, the Homestead Hearth cooks what a campfire cooks: raw meat, fish or an egg in hand, a right
click on the standing hearth (its ring and chimney whole, `station_stands`) cooks one, with an
empty hand still lighting it. The Cottage Stove (Farm, five cobblestone round a campfire) is
the campfire built in: it cooks the same, holds a cooking pot (`hob`), and warms and lights the
room as a fire does (`heat`, `fire`, light 12, which survival.ron's `heat_emission` counts),
smoking from its flue. So the home's own fire is where supper is made, and nobody keeps a
campfire on the floor. Data only: the `campfire` tag the cooking recipes ask for, on the
hearthstone and the stove (blocks_tools.ron, blocks_furnishings.ron); a loose hearthstone
does not cook, and says what the hearth lacks. Cooking at either counts as campfire cooking
for quests (First Night). Test `the_hearth_and_the_stove_cook_as_a_campfire_does`; the gate's
Cook step rings its hearthstone with stone and cooks small game at it ("hearth: cooks"), then at a
stove ("stove: cooks").

## Fire in the thatch

Thatch and wattle burn. A torch, a campfire or a brazier one step from a thatch roof or a
wattle panel (out a face), standing there eight seconds, sets it alight; a lit hearth or kiln
does the same within two steps. A step goes only through open cells, never through a solid
block, so reach is the shortest way round: stone between a flame and the thatch shields it,
and a roof's opening over a hearth lined in stone keeps its fire from the roof (the nearest
thatch is then two blocks off as the crow flies but three round the stone). What catches
becomes a **blaze**, a flame itself, so the fire runs on through the thatch and wattle beside
it, and ten seconds later it is gone, nothing left. Take the torch away before it catches and
nothing happens; break a blaze (it breaks at a touch) and it is out. Eight seconds is time
enough to see the smoke, step over and break the thatch or panel by bare hand (about a
second: hardness 0.8 or 1).

The wait is shown. While a block waits to catch it **smoulders**: grey smoke curls off it
with the odd ember (`smoke` in fire.ron, the client's emitters) and it crackles (ambience.ron's
`smoulder` group, `smouldering: true`, and its `smoulder_crackle` sound). The server tells
every player which blocks smoulder when that changes (`ServerMessage::Smoulders`, protocol 50),
since only it knows what changed (a village's thatch by its own torches does not smoulder).
The first time in a session a player's own change sets something smouldering (a torch set, a
hearth lit, thatch laid by a flame), they are told why in one line (`fire.smoulders`: "Thatch
and wattle by a flame smoulder, then catch: move the flame or break them in time.").

There is no chance in it: the same fire burns the same way every time (catching before burning
out, in order of place). Only block changes are watched (a player's, a command's, a flood's in
the one pass over a tick's flow, the fire's own), never a column as the world made it, so a
village stands by its own torches until someone sets a flame to it. Nothing is hurt by a
blaze; it only takes blocks. What waits to catch or burn out is timed in ticks of game time,
looked at once a second, and saved with the world (`blazes` in the save); a pass of no time
changes nothing.

For mod authors, it is all data: `fire.ron` names the tags that catch (`thatch`, `wattle`) and
those that are flames (`fire`), the reaches (`reach` 1 to 4, `hearth_reach` 0 to 4, for every
lit hearth or kiln), the seconds, the burning block (`blaze`, which must be a flame), what is
left (`ash`, here air) and the smoke a smouldering block gives off (non-empty), load-checked
(a block cannot both catch and be a flame; what burns and what is left cannot catch). Code:
`crates/engine/src/fire.rs`, `crates/server/src/blaze.rs`. Tests:
`a_torch_sets_wattle_alight_and_the_fire_runs_on` (the fire runs on to the next panel, burns
away, and two worlds burn alike), `a_flame_taken_away_or_put_out_burns_nothing` (and a lit
hearth two off sets it alight), `a_hearth_lit_under_thatch_smoulders_then_catches` (the hint
once, the smoulder told, caught only on time), `a_stone_lined_flue_keeps_a_hearths_fire_from_the_roof`,
`fires_are_saved_with_the_world`. The gate's Blaze step sets a torch by a wattle panel and
counts the smoke off it before it catches ("fire: smoulders", "fire: catches", "fire: burns out").

## Curing hides

A beast's hide comes off **raw**. The drying rack is the one place it dries (stations.ron,
the only work that makes hide): hung there (up to eight), it cures to hide in a
minute, half that with a fire near, and taking it down pays Crafting (stations.ron; test
`a_raw_hide_hung_on_a_rack_is_hide_a_minute_later`). Everything made of hide (pouches, hide
armour, the hide lean-to, the wool bag) takes the cured hide; only the first night's bed of
feathers and hide takes a raw one too, so the first bag does not wait on the rack. A wandering
party's goods are already cured. Hide and Seek (Nomad) asks for four raw hides, and Hide
Wraps after it for the rack's cured hide.

## Smoke over home

The Homestead Hearth's chimney smokes from its top while the hearth below it is lit, a
thick column seen from 96 blocks, so a player out at dusk sees where home is from across a
valley. It is data on the chimney block (blocks_tools.ron): a smoke emission with
`range: Some(96.0)` (how far off it is seen, up to 128, against the 24 every other block's
particles keep) and `over: Some(("hearth", 3))` (given off only with a `hearth` block up to
3 below and one across, and only from the top of a stack, with open air above). A mod's
stove pipe or a forge's flue can do the same. The client searches farther only for chunks
whose palette holds such a block, and counts nearness as a share of each emission's range,
so a far chimney takes its place among the nearest torches
(`a_chimney_smokes_over_a_hearth_from_afar`). The gate's Cook step builds a hearth beside
its campfire, checks its chimney gives nothing off cold, lights it and counts the puffs
("chimney smoke").

A cold hearth is quiet. Lighting a hearth keeps its fire until a piece of it is taken away
(server/src/hearth.rs; a kiln's burns for its fuel charge), and the server tells every
player which fires burn when that changes (`ServerMessage::Fires`, protocol 41). Over a
structure's core (a hearthstone) an emission is `over` it only while it burns, and the
ambience's `fire` group (`lit: true`) hears a hearth or a Crucible Kiln crackle and snap only
then; a campfire or a pit kiln always. A home hearth lit in a world saved before this is lit
again when its player joins.

## Light in the windows

A home shows its lit rooms at night: a window glows warm by the light of the room behind it.
Before glass there is the Hide Window (Farm, Construction 3: a hide in a frame of eight sticks,
two windows), oiled hide in a stick frame. Every block tagged `window` (the hide window, glass,
settlements' old glass) passes block light as air does (a `Cutout` cube), so a candle or the
hearth behind it lights the ground outside, and is `lit_through` (a block field, any mod's
window can say it): its bright texels glow by the block light reaching its face, which came
through it, as a lantern's glow by its own light (client `render/mod.rs` `layer_table`, the
glow byte's `THROUGH` bit; `voxel.wgsl`). It shows at any hour, as a lantern's glow does, and
stands out after dark. A dark room's window stays dark; a wall keeps the light in
(`a_window_lets_a_lit_rooms_light_out`, engine tests/windows.rs). The gate's Door step then
sets a hide window in a candle-lit stone nook at midnight and judges the shot: the pane warm
and bright against the night ("window: glows", shot `window-night`).

## A trough for the pen

Kept animals graze within their leash of home (7 blocks), and grazing wears the land: now and
then a tuft goes for good, so a full pen strips its grass and goes hungry, and a hungry animal
loses condition and gives less. A Feed Trough (Farm, Construction 4: five planks in a low U) is
a store of nine (machines.ron, opened like a barrel) tagged `trough`. A kept animal that grazes,
unfed past a day (`creatures.ron`'s condition `fed`) with nothing to graze near, walks to a
trough within its leash of home holding one of its foods (what it grazes, its favourites, what
tames or readies it, any `breeding_feed`) and eats one, head down, as it grazes; it is fed
for the day. So the trough feeds the pen while the keeper is away, an item an animal a day,
and wheat from the field has a use at home. Hunters (a kept fox) still take meals by hand.
Server `troughs.rs` (`trough_for`, `eat_from_trough`; the brain's food is a trough when it has
no grazing); tests `a_hungry_kept_animal_eats_its_food_from_a_trough`,
`a_kept_grazer_left_alone_feeds_itself_at_the_trough`. The gate's Tame step sets a trough
and opens it ("trough: opens", shot `trough`).

A kept animal also needs water within its leash of home, and that need reads one tag:
any block tagged `drinking_water` (water itself, a well, a feed trough) meets it (server
`condition.rs` `water_near`; the engine's `condition::DRINK`). So a pen away from the river
is watered by the trough that feeds it, a Construction 4 piece any age can make, and a mod's
water station is a tag on its block. Loading refuses creatures when no block carries
`drinking_water` (`check_drink`). Test: `a_pen_drinks_from_water_a_well_or_a_trough`.

## Moving the furniture

A home is rearranged, not rebuilt: whatever a player placed to furnish it (the wood
furniture, the dyed cloth, the crockery, a candle) comes back whole when broken, so a bed
moved to the other wall, or a whole home moved to a better spot, costs nothing. It is one
tag: a block tagged `furnishing` gives its drop only from a cell a player set
(`Server::placed`, kept in the save), and one a village placed gives nothing, as its props
do (design draft 80), so a settlement is still no free store of goods. Since a piece drops
itself, breaking one is no way to make planks or dye, and a placed cell teaches no skill
when broken. Loading refuses a `furnishing` that drops nothing. Tests: the engine's
`a_placed_piece_gives_itself_back_and_a_village_one_nothing`, the server's
`a_village_rug_gives_nothing_but_a_players_comes_back_whole_across_restarts` (and the crate
stack's). The gate's Nest step places a rug and a stool, breaks them and finds both back in
the pack ("furnishings: come back").

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
