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
| Snug | Nomad end / Farm start | roof, walled in (9 in 10 faces draughty or better), hearth, bed, 2 lights | heal x1.5, warmth +6 |
| Homely | Farm | + 6 in 10 faces sound, 2 seats, 1 table, 2 storage, 3 lights | warmth +8, xp x1.05 |
| Cosy | Farm | + 9 in 10 faces sound, 2 rugs, 2 curtains, 1 cushion, 3 crockery, 4 lights | heal x1.75, warmth +9, xp x1.1 |
| Heart of the Home | Farm end | + half the faces fine, books, cooking, workshop, banner | heal x2, warmth +10, xp x1.125 |

Seats are stools, chairs, rocking chairs, benches, settles, fireside benches, footstools,
cushions and window seats; tables include desks and counters; storage is whatever a home
keeps things in or on: chests, blanket chests, cupboards, dressers and wardrobes, mantel
shelves, plate racks, spice racks, candle boxes, bread bins, bedside tables, stair
cupboards, window seats, shelves (pot shelves, pantry shelves and bookshelves too), crate
stacks, barrel racks, jars and baskets, and a village's barrels and crates (every store a player opens
counts but the feed trough, which is the pen's: test
`every_store_shelf_and_cupboard_counts_as_storage`; the gate's Nest step finds a Pantry made
of a chest and a barrel by a campfire, "home: storage"); crockery is plates, bowls, jars,
jugs, baskets and a dresser's plates on show; rugs include a rush mat (4 rushes, Crafting 1,
a Nomad's first); cooking is the cottage stove and cauldron; workshop is the loom frame,
spindle stand and quern. All are craftable in the first two ages (Construction 1 to 8,
Crafting 1 to 8), so each step is a few minutes of gathering and crafting, not a grind.

## Warm, never hot

A home's warmth (each step's and room's `home_warmth` in effects.ron) is not plain warmth.
What the air, a fire's heat, food and the weather make of how warm a player feels comes
first; a home then lifts that toward the top of the comfortable band (`comfort_top`, 29 °C
in core survival.ron) and no further, and adds nothing on a day already that warm. A full
home (Heart of the Home, every room, an evening in) is about 20° warmer on a frosty night,
yet never makes its owner Hot (`FeltAbove(30.0)`), and its heals and other bonuses hold
whatever the weather. At load `comfort_top` must be below the coolest a debuff comes on at
as a player feels warmer, and no effect may give a negative `home_warmth` (tests
`a_homes_warmth_tops_out_at_the_comfortable_band`,
`a_full_home_in_warm_air_leaves_its_owner_comfortable_not_hot`).

## Walls (how well a home is built)

Each step from Snug up wants better walls about the room the home stands in, and is warmer
for them: a hide lean-to walled in all about is Snug, a log or plank house Homely, the same
with hardly a draughty board Cosy, and a house half of dressed stone, brick or glass the Heart
of the Home.

A building block says how well it shelters with a tag: `shelter:draughty` (the hide lean-to,
wattle panels, hide windows, cloth, awnings), `shelter:sound` (logs, planks, cobble, cracked
and mossy bricks, daub, half-timber, shut doors and shutters) or `shelter:fine` (stone and
other bricks, polished stone, terracotta, glass, chimney pieces). A better one counts for every
worse. An open door or shutter carries none: it shelters nothing. Roofs are judged as walls:
thatch and shingles are both sound (the trade between them is fire, not draught: thatch is
cheap and cosy but catches, shingles do not), as are timber posts and beams and plank and
cobble slabs and stairs; a lattice window is draughty. Natural rock and the ores in it are
sound, so a home cut into a hillside can be Cosy after the mining it took; earth, gravel, sand
and any other solid block with no tag count as draughty, until daub coats them. Every solid
block tagged `building` or `roof`, full or shaped, that is not furniture carries exactly one
shelter tag, and no block more than one, checked when content loads (server `rules.rs`
`check_shelters`), so a new one cannot slip in unjudged.

How it is judged (`Server::room_walls`, beside the home survey): from where home stands, a
scan goes out over the open cells within the home's reach (survival.ron's `home_reach`,
`home_up`, `home_down`), starting from the first open cell over a hearthstone. Every face it meets sideways or overhead is a face of the room: a
wall, or the roof. A wall is any solid block or any with a shelter tag, so a shut door or a
window closes the room, and an open door lets the scan out. Furniture is not a wall: a block
with a tag in survival.ron's `home_furniture` (machine, station, furnishing, storage, decor,
multiblock_core, farm, waters) is looked past to the wall behind it, so a chest or a table
by the wall leaves the room as it was, and a barrel in a doorway is no door. Furniture is a
furniture tag and no shelter tag: a shelter tag always wins, so cloth hung for a wall, a hay
bale and a stone column (`shelter:draughty`, `draughty`, `sound`) wall a room, while machines,
stations, the chest, the hearthstone, the altar and the planter carry no shelter tag and are
looked past, so a small station never plugs a gap in a wall. The scan never steps down into a
solid block or furniture: under the room is floor, so a hearthstone, a barrel or a well set in
it is floor like the stone round it. A face leading out of the reach
(or into an unloaded column) is an opening, of no shelter, and counts against the room; the
floor is not counted. `HomeWalls(Sound, 0.6)` holds when 6 in 10 faces are sound or better
(effect.rs, `Condition::HomeWalls`, `Walls::share`; load-checked: the share is above 0 and at
most 1). The scan runs only when an effect asks after walls (`EffectRegistry::home_walls`) and
is bounded by the home box, so a home out in the open costs no more than the box. `/home`
names what is short: "sound or better walls and roof all about 52%/60%".

Tests: `a_room_is_judged_by_its_walls_and_roof` (planks sound, a brick fine, a hole or an open
door an opening, a shut door a wall, a hide window draughty),
`a_home_shows_the_highest_step_it_has_and_a_rest_to_match` (each step walled for the one below
is the one below, and each step up is warmer), and the ladder's climb test (each step asks
walls at least as good as the one below). The gate's Home step walls a camp in and checks it
("home: walls").

## Rooms (side branches)

Beside the ladder, a home can grow a use. They come on at home beside the ladder's step, each
announced in chat with a **Next:** line, and the better one hides the other:

| Room | Needs (tags) | Gives while at home |
| --- | --- | --- |
| Workbench | crafting table, a seat, a light | crafting and construction checks count 1 level higher (`boosts`) |
| Workshop | + roof, 2 lights, a loom, spindle stand or quern, 2 storage | crafting and construction +2, engineering +1 |
| Pantry | 2 storage, a hearth or any heat block | the good effects of a meal last x1.25 (`meal_time`) |
| Larder | + roof, 4 storage, a stove or cauldron, a table | meals also heal x1.2 (`meal_heal`) and last x1.5 |
| Bedside | a bed, a seat, 2 lights (no roof needed) | 2° warmer, and a night's sleep there is Well rested |
| Herb nook | 2 hanging herbs or hung drying racks (`drying`), 1 storage | farming and alchemy count 1 level higher, a meal's good lasts x1.1 |
| Bright room | a roof, 2 windows (`window`: hide window, glass, lattice window, open shuttered window) | heal x1.05, learn x1.05 |
| Soft furnishings | a roof, a rug and a drape (`drape`: a wall hanging or a curtain) | 1° warmer |
| Hearth corner | a roof, the hearth, a settle (`settle`) and a woodpile (`woodpile`) | 1° warmer, heal x1.05 (test `a_settle_and_a_woodpile_by_the_hearth_make_a_hearth_corner`) |
| Bedchamber | a roof, a bed, a blanket chest (`blanket_chest`), a washstand (`washstand`) and a rug | 1° warmer, heal x1.1 (test `a_blanket_chest_and_a_washstand_by_the_bed_make_a_bedchamber`) |
| Dooryard | out under the sky, an awning and 2 yard pieces in the open (`yard`: a bench, bunting, a hay bale) | heal x1.05 |

Bedside is a Nomad's first comfort: a stool (Construction 1) and two candles (Crafting 3)
beside the sleeping bag. Before any roof or hearth it is the warmer night and the better rest
(Well rested, 15 minutes, instead of Rested's 10); a snug home gives the same rest, and two
ways to one rest give it once (`given_by_sleep` keeps each effect once, its longest rule).
The herb nook is the Farm age's: two herbs tied with twine hang to dry (Crafting 4, two
bunches); two bunches and a jar, basket or crate make it. Its farming level counts where it
is felt, sowing the kitchen garden at the door (a crop a level early) and harvesting it, and
its alchemy level at the bench.
Soft furnishings keep the draught off: a rug underfoot and cloth hung on a wall or at a window
(`drape`: a wall hanging, two cloths over two sticks, Crafting 4, any dye, or a curtain) under the home's roof, 1°
warmer; two rugs are not enough
(`a_rug_and_a_hanging_under_a_roof_are_soft_furnishings`, the server's
`a_rug_and_a_hanging_make_soft_furnishings`, the gate's walls step: "home: soft furnishings").
The dooryard is the Farm age's too, the home's outside (home audit 2, item 10: outdoor decor
used to count for nothing): an awning at the door and two more things set out under the sky
about it. A yard piece counts only where it stands under the open sky (effects.ron
`outdoors`, a tag list load-checked against what `HomeHas` asks), so a bench or bales kept
indoors are no yard; only blocks still standing count, so a bale taken away or broken takes
its part with it; and it holds only out in the yard, never under the roof, which `/home`
names ("you out under the open sky", `Lack::Sky`)
(`an_awning_and_yard_pieces_out_of_doors_make_a_dooryard`, the server's
`an_awning_a_bench_and_a_bale_make_a_dooryard`, the gate's walls step: "home: dooryard").

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

**No ghost home.** A home used to be only ever set: break the hearth or the bed and you woke
at the hole, the mark pointing there. Now every block change asks `Server::home_changed`:
a player whose bed stood where the new block is no bed forgets the bed ("Your bed is gone:
you will not wake there again."), and one whose home hearth's core it was forgets the hearth
("Your hearth is gone: it is your home no more."), en+ja. They wake at what is left (the
hearth, the bed, else the world's spawn) and `Vitals::home` changes at once, so the mark
moves or goes. A bed set down where the bed was (a bag swapped for a proper bed) is still
the bed. Test: `hearth::tests::a_broken_bed_or_hearth_is_forgotten`. Gate: the Nest step's
last phase breaks the bag that was home and waits for no home, no mark and the chat line
("home: no ghost"). A bed broken while its sleeper is offline is not seen (their save keeps it
until they sleep elsewhere).

## Sleeping

Right clicking a bed used to only set where you wake. It now also gives a **rest** (the
`sleeps` rules): Rested (10 minutes) anywhere, Well rested (15) if the bed is in a Snug home,
Deep rest (20) in a Cosy one (a game day is 20 minutes). Rest gives a little heal, xp,
mining and speed wherever you go, and a bed always works as a bed (it sets where you wake and your home) but gives no new rest
(save a bed a village furnished a home with, where its plan put one, while that very bed
stands there: it is its resident's, and sleeping in it is refused with `bed.someones`, outside
creative; any other bed works, placed, another player's, or set by command)
while one lasts, so the buff is a once-a-day habit, not a button to hold. Timed effects count down
online only.

A proper bed rests better than a bag on the ground: a sleep rule can name a `bed` tag the bed
slept in must carry, and a bed of furniture (`bed: "furniture"`) is Well rested anywhere,
where a sleeping bag needs a Snug home or a lit corner. Every sleep also mends: Healing
Sleep (heal x4) for a minute, two in a proper bed, so coming home hurt and sleeping heals.
It is no way to heal in a fight: a bed refuses sleep with a creature that hunts players
(wild, or kept by another player) within `sleep_threat_reach` blocks (12), or within
`sleep_after_hurt` seconds (15) of a wound (a blow, a fall, a sting; not the cold or heat
wearing at one) (`core/survival.ron`). A sleep refused for a threat or a wound still claims the bed; a
village's bed is never claimed. And it stays slower than eating, since food is the heal (design
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

## Nights

Nights are told apart (home audit 2, item 6). An effect can ask for `Night`: from sunset to
sunrise (`calendar::night`, by the same day the sky draws), the last third of the day, 16000
up to 24000 ticks after sunrise: `/time set sunset` is night, `/time set sunrise` day.

- **The better rests are a night's.** A sleep rule marked `night: true` gives its rest only
  after dark: Well rested (a proper bed, a snug home, a lit corner) and Deep rest (a cosy
  home). A nap by day still gives Rested and its mend, and says what a night's sleep there
  would give ("A night's sleep here would rest you better: Well rested").
- **A nap does not keep a night's rest away.** A bed holds its rest back while that rest, or a
  better one, is on the player, or while the last sleep's mend (Healing Sleep) still runs
  (`EffectRegistry::rested`): a second nap rests no more, and a bed after a bag in one night
  mends no more, but once the nap's mend is over, sleeping that night gives the better rest
  though the nap's Rested is still on.
- **An evening in.** After dark, indoors at home within four blocks of a fire to sit by
  (`campfire`: a campfire, the hearth, the stove; a torch is no fireside), the home's own
  fire (`HomeHas("campfire", 1)`, not a neighbour's through a wall): Evening in, heal
  x1.2 and 2° warmer, announced. Morning ends it. A time of day, not a step to build toward:
  it is on its own ladder (`night`), so `/home` never names it as the next room.

Tests: the engine's `a_night_sleep_rule_holds_only_after_dark` and
`sleep_gives_the_rest_the_home_allows_once` (a nap does not keep a better
rest away), the calendar's night; the server's
`a_nap_by_day_is_no_nights_rest_and_an_evening_in_is_by_the_fire_after_dark`. The gate's walls
step sets midnight in the walled room by the hearth and waits on the Evening in chip, with its
line seen at any point in the run, since the server tells an effect once a session and the
motifs step can be where it first came on ("home: evening in"), then sets noon.

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
second). Effects can ask `Seated`; base's **Fireside** (`fireside`: heal x1.5, 4° warmer)
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
with it either way (`supported_by`). `beside` names a block placed with it in the cell to
the right of whoever sets it (a double gate's second leaf), in the same facing; the two
are broken and turned together, and load refuses a pair whose `swaps` do not make a pair.
`swaps` names what using it turns it into, in the same
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

**Stores count everywhere a home crafts (Oct 14).** The eleventh pass carries the same rule
to the rest of the home, so the barrel is a larder from the first night:

- **Always on, not a tier perk.** The rule holds wherever the player stands in their own home,
  at any home step from a bed and a torch up. Tying it to a tier would hide it until the
  home is Homely, when the first days are already over; the tiers instead grow how much the
  stores hold. Only the player's own home counts, and only the stores they placed (or that
  nobody did, as a village's): a guest sleeping in a friend's bed crafts from their own pack
  and their own barrels, since a friend's barrel is not theirs to empty.
- **The home bounds** are the box `/home` counts: `home_reach` across, `home_up` and
  `home_down`, around the lit hearth, else the bed last slept in. The player must stand
  in it, and a station must stand in it too.
- **Which stations count.** Crafting (the 2×2 grid and the crafting table) already lays from
  the stores. Home stations (stations.ron) now count too: loading one tops the load up to
  what it `holds`, with more of what is in hand. It takes from the hand first, then the
  rest of the pack, then the stores, nearest the hearth or bed first. Unlike Lay, a station
  never reaches into a pouch. This is data per station, `from_stores` (true unless said
  otherwise). The drying rack and the cooking pot take from the stores. The compost heap
  does not, because grain and seed kept in a barrel are for eating and sowing, and a heap
  that ate them would punish keeping a larder. The chat line says how many came from the stores
  (`station.loaded_from_stores`, en and ja). Cooking at a fire (cooking.ron) and machines
  are left out. A fire cooks the one item in hand and a machine takes items into its own
  slots, so there is no load to top up.
- **What the player sees.** The Can Make list under the inventory, and what the recipe
  viewer says a recipe still wants, count the home's stores while the player stands in
  their home. The server sends the tally with the vitals (`Vitals::home_stores`, protocol
  44). It is recounted on the once-a-second survival pass, which already surveys the same
  box, and resent only when it changes. With four planks in the barrel and none in the
  pack, the crafting table shows as ready. What a fire cooks counts the pack alone, since a
  fire cooks what is in hand: raw meat only in the barrel is not shown cooked.
- **Tests.** In server/src/laying.rs: `at_home_a_rack_tops_its_load_up_from_the_stores`,
  `at_home_with_no_store_a_rack_tops_up_from_the_pack`,
  `a_compost_heap_leaves_the_stores_alone`, `a_guest_draws_on_no_one_elses_stores`,
  `a_station_and_its_loader_must_both_be_in_the_home` and
  `the_home_stores_are_counted_only_at_home`. In client/src/hud/makeable.rs:
  `the_home_stores_count_toward_what_can_be_made`, which also checks a cooked thing is not
  shown from the stores alone. In the gate's Home step, before Lay, the check
  "home: can make" asks that the server counted the barrel's planks (as many as the crafting
  table's recipe takes) and that the Can Make plan, drawn under the open barrel, has the
  crafting table ready from them alone.

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
with the odd ember (`smoke` in fire.ron, the client's emitters) and it hisses and crackles
(ambience.ron's `smoulder` group, `smouldering: true`, its `smoulder_hiss` bed and
`smoulder_crackle` sound). As it catches, a soft whoomph comes from it (sounds.ron's `fire`
`catch`, played on the server's block change to the blaze; a fire that can burn without one
does not start). Burning, it crackles crisply (the `blaze` group, `blaze_crackle` bed and
`blaze_pop` sound), as loud as there are burning cells near, so the crackle thins as the fire
burns out and stops with the last of it. All of it follows the server's word, never a timer
of the client's. The server tells
every player which blocks smoulder when that changes (`ServerMessage::Smoulders`, protocol 48),
since only it knows what changed (a village's thatch by its own torches does not smoulder).
The first time in a session a player's own change sets something smouldering (a torch set, a
hearth lit, thatch laid by a flame), they are told why in one line, with the ways round it
(`fire.smoulders`: "Thatch and wattle by a flame smoulder, then catch: move the flame or break
them in time. Daub the wattle, build in stone, or light with candles."). A candle lights a home
(its `light` tag counts toward snug's two lights and up) with no flame in it, and is made by
hand at Crafting 3 from twine and hide, in the Nomad age. A player joining is told at once what
smoulders, so its smoke shows from their first frame.

An older world's home hearth, relit for its player as they join (`kindle_home`), sets nothing
smouldering: they changed nothing, and a roof should not go up under someone who just logged
in. Instead they are told once of the nearest thatch or wattle its fire would reach, and how
many steps off (`fire.near_hearth`). Their next lighting by hand does set it smouldering.

**Homes catch only from their owner** (fire.ron `homes_catch_only_from_owner`, on unless a pack
turns it off). Every fire remembers whose it is: the player whose change started it (a torch set,
a hearth lit, thatch laid by a flame), carried on as it spreads and saved with it. A block in a
player's home box (`home_reach` across, `home_up` up and `home_down` down from the cell their
home is at: over a home hearth lit by them, or where an online player's home is; the same box
they feel at home in) only smoulders or catches from that player's fire. Where two homes overlap
it is kept from both owners' fires, so lighting a hearth next door, or making your home at
someone else's hearth, never lets your fire into their home. A home hearth lit before fires were
kept by owner belongs to no one and keeps out every fire, so it stays protected while its player
is away and no one can take it over by touching it. A home hearth stays whose it was first lit by
while its fire is out, while a piece of it is broken and whoever relights it, until its owner
breaks its hearthstone or lights a home hearth elsewhere; its box keeps out others' fire all that
while. Breaking and mending a stranger's hearth, its hearthstone too, never opens their home to
your fire nor makes the hearth yours. (A world saved before this kept no owner for a cold home
hearth: it comes back as no one's.) A hearth no one owns is never forgotten, so its spot keeps out
every fire for good; that is the safe way round. Only the owner's own hand breaking the hearthstone
forgets a hearth, not a fire of theirs spreading over it. Lit by someone else, a hearth's fire is
the lighter's as it catches what is near, so relighting a stranger's hearth never sets their home
alight. Another player's torch beside your wattle wall fizzles, and a neighbour's burning shed
stops at your home's edge; your own careless torch still burns your own roof. A fire a command
or the world started belongs to no one, so it never takes a home either.

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
`fires_are_saved_with_the_world`. The gate's Kindle step sets a torch by a wattle panel,
twenty blocks off the home's steps, and counts the smoke off it ("fire: smoulders"); the fire
burns on through the home's steps, each frame noting when the panel shows a blaze and when it
is gone, and the Blaze step, before the catalogue, judges those by fire.ron's times ("fire:
catches", "fire: burns out"), so its 18 s of waiting run under other steps.

## Daubing in place

A wattle panel already standing is daubed where it stands: clay in hand, right click it, and
it is a daub block, one clay used. Daub is a sound wall (`shelter:sound`) and does not burn,
so a wattle home grown up round its hearth is made safe without being pulled down, and a panel
smouldering by a torch, daubed in time, is saved. It is data on the block (`coat: Some((with:
"clay", into: "daub"))` in blocks_building.ron, load-checked: both blocks exist, and what it
becomes is not coated again), and asks the level of the easiest recipe that makes what it
becomes (Construction 2 for daub), teaching that recipe's experience a piece; below it, it is
refused with the level it takes. Action `base:coat` (requirement `Coats`, silent: it only says
the click meant it), server `coat.rs`, test `clay_daubs_a_placed_wattle_panel_in_place`; the
gate's Kindle step daubs a panel by its torch ("daub: coats").

Earth walls daub the same way. Dirt, coarse dirt, gravel, sand and red sand carry the same
`coat` (blocks.ron), so a room dug into a bank or walled in earth turns from draughty to sound
one clay at a time. Clay coats only when aimed at a block's side, a wall's face: clay to the
top of a dirt floor or a panel is laid there as a block as before (test
`clay_to_the_side_of_an_earth_wall_daubs_it_and_to_the_top_does_not`).

A panel never daubs shut on anyone. The thin panel turns into a whole daub block, so clay held
to one a player or a creature leans into is refused with placing's words ("Someone is standing there":
`CoatClear`, refusals.ron), as placing a block on them and shutting a door on them are. The
dauber's client sees no other body, so the server sends those words itself; and `coat` keeps
the cell clear whatever action asks for it. Once they step off, the same clay daubs it (test
`a_panel_someone_leans_on_is_not_daubed_around_them`).

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
(`a_chimney_smokes_over_a_hearth_from_afar`). Chimney pieces (`chimney_piece`) stacked
straight over the chimney carry its smoke up and out of their top, so a chimney carried up
through a roof smokes from its pot (docs/building.md). The gate's Cook step builds a hearth
beside its campfire, its chimney carried up a brick flue to a pot, checks nothing smokes
cold, lights it and counts the pot's puffs ("chimney smoke").

A cold hearth is quiet. Lighting a hearth keeps its fire until a piece of it is taken away
(server/src/hearth.rs; a kiln's burns for its fuel charge), and the server tells every
player which fires burn when that changes (`ServerMessage::Fires`, protocol 41). Over a
structure's core (a hearthstone) an emission is `over` it only while it burns, and the
ambience's `fire` group (`lit: true`) hears a hearth or a Crucible Kiln crackle and snap only
then; a campfire or a pit kiln always. A home hearth lit in a world saved before this is lit
again when its player joins.

## Lights burn down

Every light set down burns by the world's clock (engine `flame.rs`, server `flames.rs`): a
torch a game day, a candle three, an oil lamp five on a fill of fat. A home's light steps
(Camp, Snug, Homely, Cosy, Bedside, Workbench) therefore need tending, not just building.

- **Guttering.** In the last tenth of its burn (`gutter_share`, core/survival.ron) a torch
  turns into its guttering block (`gutters_to:`): dimmer, its flame low and smoky, burning on
  to the same end. That is the cue to light it again: anything that kindles lights a
  guttering torch whole (`relights_to:`).
- **Telling the owner.** When lights going out leave a home short of a light a step it had
  asks for, the home's owner (if here) is told once per burn pass, naming the highest step
  lost and how many lights it wants (`flame.home_short`). The steps a home meets are kept on
  the player (`home_steps` in their save); one who was away is told on return, once the land
  round their home has loaded (`flame.home_short_away`).
- **Picked up burning** a light keeps what it has burnt as its item's wear (`WEAR_PER_DAY`,
  1200 a game day), shown as a wear bar and in its tooltip, and set down again burns on from
  there: carrying a light never refreshes it. A lamp picked up lit keeps its fat.
- **Stubs.** A torch's stub breaks into an unlit stub item. Set down it is a stub, lit again
  where it stands by anything that kindles; held in any fire it catches into a torch.
- **Tended.** Inside a settlement, within `tend_height` blocks of its height (so not in a mine
  under it), its people tend what is tagged `tended`: one set down there never goes out, and
  keeps what it had burnt. The item's tooltip says so.
- **Old lights.** A light with no burn (set down before lights burnt out, or placed by the
  world outside a settlement) is given a full burn the first time its column loads, once a
  column (`flame_columns` in the world's save).

## Light in the windows

A home shows its lit rooms at night: a window glows warm by the light of the room behind it.
Before glass there is the Hide Window (Farm, Construction 3: a hide in a frame of eight sticks,
two windows), oiled hide in a stick frame. Every block tagged `window` (the hide window, glass,
settlements' old glass, and the lattice window and open shuttered window, which are shapes,
not cubes) passes block light as air does (a `Cutout` cube, or light through a lattice's
squares), so a candle or the hearth behind it lights the ground outside, and is
`lit_through` (a block field, any mod's window can say it): its bright texels glow by the
block light reaching its face, which came
through it, as a lantern's glow by its own light (client `render/mod.rs` `layer_table`, the
glow byte's `THROUGH` bit; `voxel.wgsl`). It shows at any hour, as a lantern's glow does, and
stands out after dark. Two windows in a home's walls under its roof make it a **Bright
room** (Rooms above; home audit 2, item 10: windows used to count for nothing), whatever pane
a package tags `window` (`two_windows_at_home_make_a_bright_room`, the server's
`windows_at_home_make_a_bright_room`, the gate's walls step: "home: bright room"). A dark
room's window stays dark; a wall keeps the light in
(`a_window_lets_a_lit_rooms_light_out`, engine tests/windows.rs). A curtain hung inside a
window draws across and back with a right click (`swaps`, as a door; drawn back it is
`{name}_curtain_open` and gives the curtain): drawn across, it keeps most of the room's
light in (`light_filter: 8`), so a lit home's window glows low at night, and drawn back
the light goes through as air (`a_drawn_curtain_keeps_the_light_in`; the gate's walls step
hangs one inside a pane and draws it back: "home: curtain"). Either way it is a curtain for
the ladder's steps and a drape (`drape`, which Soft furnishings asks beside a rug). The gate's Door step then
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
- Passing the night by sleeping (sleep stays a rest, not a skip).
- Sleeping, and the ladder, for villagers' houses.
