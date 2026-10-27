# Tools and crafting

Mariano's design: RuneScape crossed with Minecraft, deeper. Better tools work faster and
find more, levels gate what you may do and raise what you find, and nothing makes the world
grow faster. Everything is content a mod adds the same way.

## Tools

A tool is an item (`base/blocks_tools.ron`) tagged `tool`, its kind (`pickaxe`, `axe`,
`shovel`, `hoe`, `knife`, `shears`), a `tier:N`, a `speed:X` and maybe a `yield:Y`. Five
kinds come in four tiers: flint (1), stone (2), copper (3) and iron (4), speed 2, 4, 7
and 14, yield 0 to 0.15. The tooltip says it ("Works 7× as fast as a bare hand"). Tier 1
has a flint pickaxe, axe and knife and a digging stick (a shovel), but no hoe: tilling waits for a stone edge. There are no wooden tools (Oct 15:
a plank's edge cuts nothing); an older save's come back as the flint tool, the digging
stick or the stone hoe that does their work now (`renamed` in `blocks_tools.ron`).

**Why those speeds** (Oct 14 tier audit). A block takes its hardness over the speed, and
after each break the hand waits a quarter second before the next (`mine_cooldown`,
Minecraft's five ticks), so what a player feels sweeping a wall is hardness / speed + 0.25 s.
With 2, 4, 6, 8 the copper and iron steps were near that floor: on a log (hardness 2)
0.75 to 0.58 to 0.50 s a block, 1.29x then 1.17x, and on stone 1.25x then 1.14x, under the
1.4x a step needs to be felt. Copper 7 and iron 14 make each step about 1.4x on what tools
are for (log, cobblestone, ore; stone 1.35x and 1.30x):

| Seconds a block (with the 0.25 s wait) | Hand | Flint | Stone | Copper | Iron |
| --- | --- | --- | --- | --- | --- |
| Log (needs an axe), cobblestone (2.0) | 2.25 (cobblestone) | 1.25 | 0.75 | 0.54 | 0.39 |
| Stone (1.5, needs a pickaxe) | | 1.00 | 0.63 | 0.46 | 0.36 |
| Ore (3.0, needs a pickaxe) | | 1.75 | 1.00 | 0.68 | 0.46 |
| Clay, gravel (0.6) | 0.85 | 0.55 | 0.40 | 0.34 | 0.29 |
| Dirt (0.5) | 0.75 | 0.50 | 0.38 | 0.32 | 0.29 |

Soft ground stays near the quarter-second floor whatever the tool (1.2x a step past
stone): it was already quick, and only the wait could change that, which is not a tier's
to change.

Bare hands break neither natural stone nor a standing log: their rules in `base/tools.ron`
ask for a pickaxe and an axe of tier 1 (Mariano, Oct 15: the early game was too easy, and
the first tool must matter). So a new player's first tools are flint ones, made in the
hand's grid from what hands can gather: flint (nodules lie loose in the grass and on the
forest floor, and gravel gives it one break in eight) and sticks (a tree's leaves give one
in five, a dead snag always; leaves a player set down give none). However poor the land
by the spawn, the **spawn kit** (`base/spawn_kit.ron`, server/src/spawn_kit.rs) lays four
to six loose flint and a three-block snag 2 to 6 blocks from it, once per world (new worlds
and older saves alike, `WorldMeta::spawn_kit`), into air on natural ground no player
placed; it is wild, so picked flint grows back (`regrowth.ron`'s `loose`). A flint axe is two flint over a stick, a flint pickaxe two flint over
two sticks, a flint knife a flint over a stick, and a digging stick two sticks end to end. A click on stone or a log by hand says "Needs a Flint
Pickaxe or better": a refusal names the least tool that would do, the lowest tier and then
the cheapest (`Content::least_tool`). Cobblestone, mossy or not, still breaks by hand; a log
set down as a wall wants an axe like a standing one (whoever has logs has felled them).

Stone tools want hard rock. Their heads are **stone heads**, ground at a **grindstone**
(sand over any cobble, on two sticks, in the hand's grid) from **toolstone**: granite,
diorite, andesite and basalt, which only some rock provinces bare (`rocks.ron`), and the
deepslate under every province, so a player in limestone or sandstone country walks to find
it or mines down. Plain stone and cobblestone will not take an edge. A stone pickaxe or axe
takes three heads, a hoe two, a shovel or knife one.

Cobblestone is made too. Plain stone breaks into two **loose stones** (`drops` and
`drop_count` in `blocks.ron`), and four loose stones knock together into two cobblestone in
the hand's grid, so a block mined is still a cobblestone, one craft later. Hard rock
(granite and the other rock families) breaks straight to its own cobbles, which the
grindstone takes whole; cobblestone set down and broken again gives itself back.

`base/tools.ron` (engine/src/tool.rs) says what each kind is made for, by block tags (a
pickaxe for stone and ore, an axe for logs and planks, a shovel for soil, sand, gravel,
clay and snow, a hoe for crops and leaves, shears for leaves and wool, a knife for plants).
Over a block it suits, a tool mines `speed` times as fast as a bare hand, and its `yield`
adds to the chance of an extra drop. A knife already takes more meat and better hides from
creatures (their loot's `tools`), and shears wool.

Block rules, first match wins, say what breaking a block takes and teaches: a level in a
skill, a kind and tier of tool, and the experience. Ores ask for Mining levels as in
RuneScape (copper 5, tin 10, iron 15, cobalt 25, silver 30, gold 35, diamond 45, aetherite
50), teach more the higher they ask (stone 1, coal 13, copper 17.5, tin 25, iron 35 up to
bauxite and aetherite 396, design draft 63, so stone alone no longer trains Mining) and
pickaxe tiers as in Minecraft (coal, copper, tin and iron a stone pickaxe, gold and silver
copper, diamond iron). Coal's and copper's samples on the ground above a seam lift with a
flint pickaxe, so the seam is found before it can be dug. The first sample a player breaks over each vein
teaches its rule's `prospect` as well (three times the ore's experience: coal 39), with a
line saying what lies below; it is kept as a `prospected:x,y,z` tally by the vein's
middle (`Overworld::vein_of_sample`), so breaking more samples of the same vein, or one a
player set down, teaches only the break. A click on a block that refuses says why: "Needs Mining 15", "Needs a stone
pickaxe or better". Blocks players placed teach nothing (the world keeps where they put
them, `WorldMeta::placed`).

Mining time is the block's hardness divided by the tool's speed (when it suits) and the
player's effects' `mining`; client and server work it out the same way.

## Crafting

Recipes (`base/recipes.ron`, engine/src/craft.rs) are shaped (up to three rows of three)
or shapeless (up to nine ingredients). A shape may sit anywhere in the grid and is also
made mirrored left to right (an axe's head on either side), so the load refuses a recipe
whose mirror image is another recipe's pattern that makes something else. Where a shape and
a shapeless recipe take the same things, the shape wins among recipes made in the same place, whichever file
loaded first: a
sling's hide between two twine is no coin purse (`every_recipe_can_be_crafted_as_written`).
The inventory's grid is two by two; a **crafting
table** (four planks) gives three by three: right click one and the inventory opens with
the bigger grid, for as long as you are within reach of it and the screen is open.
Anything bigger than two by two, or with more than four ingredients, needs a table; the
server checks.

A recipe may be made only `near` a block with a tag, within a player's reach
(`craft::tags_near`, the same for the server and the client's prediction): where the grid
holds both that recipe and one made anywhere, the one near the block is made, and away from
it the plain one. The **chopping block** (`blocks_workshop.ron`; flint over any log,
Crafting 2) is the first: a log split by it gives six planks, not the grid's four, so it
pays back the log it took after two. The recipe viewer shows the block beside the grid and
"Near a Chopping Block". A mod adds a workshop block by giving a block a tag and a recipe
`near: "<tag>"`; the load refuses a tag no block has.

A cell or ingredient can accept several items: `tag:log` takes any log, `coal_ore|charcoal`
either (`Recipe::alternatives`; the viewer shows the first). Torches use it.

Every recipe trains a skill, Crafting unless it names another, may need a `level` in it,
and gives `xp` for every one made: tools, the table, torches, shears, pails and smelting
train Engineering, bread and animal feed Cooking. A recipe whose level is above yours
shows "Needs Engineering 15" where its result would be. The viewer's **Lay** button fills
the grid with a recipe from the pack, and at home from its stores too (docs/home.md).

Smelting is by hand for now (ore over coal at the grid makes an ingot); a furnace comes
with Engineering 30.

## Farming, taming and breeding with tools

- A **hoe** tills soil into **farmland** (right click); crops are sown on farmland only
  (`ground` in crops.ron says otherwise for a crop that wants something else). A hoe over a
  ripe crop adds its yield to rarer finds. A stone or copper hoe also **reaps**: cutting a
  ripe crop takes every ripe crop of the same kind in the three by three patch about it, an
  iron hoe the five by five (`reap:N` on the tool, at most 2, checked at load); fruit on
  trees is still picked one leaf at a time. Each crop
  reaped gives its harvest and experience as if cut on its own, so a field goes faster, not
  richer. The tooltip says so ("Reaps ripe crops in a 3×3 patch").
- A **taming treat** (Cooking 27) is taken by any animal you are skilled enough to tame,
  and wins its trust twice as fast as its own food (`taming_treat`, `trust:2`).
- **Seed mix** (Cooking 8) readies any kept animal to breed (`breeding_feed`).
- A creature too wild for your Hunter level, or a kept one your Farming cannot breed yet,
  turns food down instead of eating it for nothing.

## Fishing

`base/fishing.ron` (engine/src/fishing.rs) lists what bites: an item, the Fishing level it
takes, its experience, a weight, salt or fresh water (salt is a biome tagged `ocean`),
biome tags, and when (day, night, rain). The water's climate counts as a biome tag, from
its temperature by `water`'s bands (frozen, cold, cool, temperate, warm, hot): the sea is
one biome from pole to equator, so cod bite in cold water and tuna in warm. A cast (`base:fish`, five seconds, a rod in hand,
water within nine blocks with open air above) lands something with a chance that starts at
about a third for the water's best fish at its level and grows with each level above it
and with the rod's tier, up to nine in ten; which fish is a weighted draw among those that
can bite, rarer ones a little more often with a better rod's yield, and each fish bites
twice as often, against those below it, for every 15 levels it asks (`outlevelled`). A
fish's
experience doubles every 10.5 levels, 10 × 2^((level − 1) ÷ 10.5): perch 18, cod 35, carp
68, pike 131, tuna 254, sturgeon 353 (55), marlin 491 (60, deep ocean by day), deepwater
lanternfish 951 (70, deep ocean at night), koi 1,323 (design draft 63). Fish cook at a
campfire (cooking.ron) in two seconds, teaching what catching them does. Fishing tops out at level 75 for now (glimmer koi), with
the cape perk at 80.

## Pouches

An item tagged `pouch` and `holds:TAG` (blocks_tools.ron: the seed pouch holds `seed`, the
herb pouch `herb`) carries what has that tag: while the player has the pouch in a slot,
those items go into `Inventory::pouched` instead of the slots, up to 1,000 of a kind and 40
kinds in all. A seed pouch in hand sows (`actor_seed`: the first seed in it the player's
Farming level allows) and right click empties it (`base:empty_pouch`). What a pouch holds
stays with the player if the pouch is dropped; a pouch is needed to put more in.

## Wild plants grow back

Flowers, herbs, fungi and berry bushes the world grew come back where they were picked
(`regrowth.ron`, engine `regrowth.rs`, server `regrow.rs`), as long as the block they grew
from is still there: dig out or replace the ground and the spot is gone. Commons take 20
minutes of world time, uncommons an hour, rares half a day, very rares two days and
legendaries four. Herbs cannot be sown, and a plant a player places does not come back,
except on a herb planter (planks and dirt at a crafting table, Construction 3) for a player
whose Farming level reaches the plant's (1, 15, 40, 60, 80 by rarity); picking one there
teaches Farming. Waiting plants are saved with the world (`WorldMeta::regrowing`), and
`/grow` brings back those near you at once.

## Multiblocks and the Homestead Hearth

A multiblock is a structure recognised from a pattern of blocks (`multiblocks.ron`, engine
`multiblock.rs`): layers from the bottom up, a key from characters to items, `tag:TAG` or
`a|b`, `.` for anything and a space for air, and a core block; it counts turned any of four
ways. The first is the Homestead Hearth, the Nomad age's gate: a hearthstone (eight
cobbles at a crafting table, Construction 1) in a ring of stone, with a chimney (cobbles and
clay, Construction 1) two blocks tall at one corner. Right click the hearthstone
(`base:light_hearth`): if the hearth stands, it is your home (`Player::home`, saved) and
you wake on it after dying; if not, you are told what it needs.

## Sleeping bags, kilns and bronze (the Nomad and Farm ages)

A sleeping bag (three wool and two hide, four pelts, or feathers and two hide) is where
you wake after dying until a hearth gives you a home: right click it to sleep
(`base:sleep`, `Player::bed`, saved). A pit kiln (cobbles round a campfire, Construction 1) fires clay into fired clay: loaded with up
to twelve and left, it fires them in twelve seconds while the player does something else
(`stations.ron`, docs/stations.md), and fired clay is what pots, vials and kiln bricks are made
of at the grid. The Crucible Kiln, the Farm age's gate, is a multiblock:
a crucible (kiln bricks round a clay pot) ringed with tempered kiln bricks, with a pillar
on each corner. Tempered kiln bricks are four fired clay and a straw (two, as plain bricks
are); straw comes of grain: one from a wild wheat or rice tuft picked, one or two from a
ripe sown plant (wheat, rye or rice). Straw is fibre too (thatch), three
twist into two twine, and nine bind into a hay bale, which comes apart into the nine when
broken. A crucible smelts only in a standing kiln with an unexpired paid fire:
copper ore into ingots, and three copper ingots held with a tin ore carried into four
bronze (a station recipe may take several of what is held, `uses`, and other items
besides, `with`, and make several, `count`), all Engineering, never burnt (`stop_burning: 0`). To light a cold kiln,
hold 32 charcoal or 24 coal and right-click the crucible. Lighting consumes the charge
and quests count it (`Lit("crucible_kiln", 1)`). Its fire uses the fuel times in
`machines.ron`, survives reload, and goes out when it expires or a required piece is
removed. A cold kiln leaves smelting ingredients untouched.

Multiblocks are lit generically (`base:light` on a block tagged `multiblock_core`): a
multiblock with `home: true` becomes home; a fuelled one must first receive a complete
charge. One that does not stand yet says its `hint`. Wooden chests wait for a container
screen.

## Fewer items, more ways (P27)

Mariano, 2026-10-07: one item per material or part, each with several ways to get it
(gathering, crafting, drops, quest rewards, trade), rather than many items for the same
thing. Every warm pelt is fur (a bear gives two), rabbit hide is hide, duck eggs are eggs,
bear fat is lard, bright feathers are feathers, claws are fangs, glimmer dust is moon dust;
fired clay replaces unfired pots, unfired vials and fired bricks, and tin ore goes straight
into bronze at the crucible (Oct 14: the bronze blend between them is gone). Flint comes from gravel, one break in eight (tools.ron's `instead`;
gravel a player placed gives none). Twine comes from tall grass cut with a knife, one cut in
five, and a better knife finds more (`by: "knife"`: the knife's yield adds to the chance, so
a stone knife is 1 in 4, copper 3 in 10, iron 7 in 20); by hand, tall grass gives nothing.
The recipe book shows both: each `instead` is a way to get its item ("Cut from Tall Grass
with a Wooden Knife or better (20%+)"), and each knife lists twine among its uses.

Raw and cooked rabbit and chicken were the same meat (Cooking 1, the same experience and
healing) and are **small game** now (Oct 14). A package's blocks file lists what it renamed
or merged, old name to new (`renamed: {"raw_rabbit": "raw_small_game"}`), and an older save
loads those items, in slots, pouches, on the ground and on stations, as the new one. Load
refuses a rename to a block that does not exist, or of a name a block still has. Shrimp, which
were minnows in all but name (level, experience, healing, weight), are minnows too, and
minnows bite in any water. Frog, also Cooking 1 and healing 4 but teaching less, is small
game too: a frog drops it and a heron still favours it.

## Dying

Dying costs what you carried, not your skills (Mariano, 2026-10-06). You keep your three most
valuable items, counted one at a time; everything else, pouches' contents too, lies where
you fell for 30 minutes, and only you can pick it up (`Dropped::owner`, the hash of your
name); then it is gone. An item's worth is its rarity (1, 4, 16, 64, 256) times one more than
its tool tier, doubled when it cannot stack. A grave-ward amulet (tag `grave_ward`) is always
kept, keeps a fourth item and holds the rest for an hour; for now it comes only from `/give`.
You wake at `Player::home` (a bed or hearth will set it, and it is saved), or at world spawn.
The server code is `crates/server/src/death.rs`; the gate's workshop step dies with `/kill`
and picks everything back up.
