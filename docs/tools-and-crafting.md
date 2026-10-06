# Tools and crafting

Mariano's design: RuneScape crossed with Minecraft, deeper. Better tools work faster and
find more, levels gate what you may do and raise what you find, and nothing makes the world
grow faster. Everything is content a mod adds the same way.

## Tools

A tool is an item (`base/blocks_tools.ron`) tagged `tool`, its kind (`pickaxe`, `axe`,
`shovel`, `hoe`, `knife`, `shears`), a `tier:N`, a `speed:X` and maybe a `yield:Y`. Five
kinds come in four tiers: wooden (1), stone (2), copper (3) and iron (4), speed 2, 4, 6
and 8, yield 0 to 0.15.

`base/tools.ron` (engine/src/tool.rs) says what each kind is made for, by block tags (a
pickaxe for stone and ore, an axe for logs and planks, a shovel for soil, sand, gravel,
clay and snow, a hoe for crops and leaves, shears for leaves and wool, a knife for plants).
Over a block it suits, a tool mines `speed` times as fast as a bare hand, and its `yield`
adds to the chance of an extra drop. A knife already takes more meat and better hides from
creatures (their loot's `tools`), and shears wool.

Block rules, first match wins, say what breaking a block takes and teaches: a level in a
skill, a kind and tier of tool, and the experience. Ores ask for Mining levels as in
RuneScape (copper 5, tin 10, iron 15, cobalt 25, silver 30, gold 35, diamond 45, aetherite
50) and pickaxe tiers as in Minecraft (iron a stone pickaxe, gold and silver copper,
diamond iron). A click on a block that refuses says why: "Needs Mining 15", "Needs a stone
pickaxe or better". Blocks players placed teach nothing (the world keeps where they put
them, `WorldMeta::placed`).

Mining time is the block's hardness divided by the tool's speed (when it suits) and the
player's effects' `mining`; client and server work it out the same way.

## Crafting

Recipes (`base/recipes.ron`, engine/src/craft.rs) are shaped (up to three rows of three)
or shapeless (up to nine ingredients). The inventory's grid is two by two; a **crafting
table** (four planks) gives three by three: right click one and the inventory opens with
the bigger grid, for as long as you are within reach of it and the screen is open.
Anything bigger than two by two, or with more than four ingredients, needs a table; the
server checks.

Every recipe trains a skill, Crafting unless it names another, may need a `level` in it,
and gives `xp` for every one made: tools, the table, torches, shears, pails and smelting
train Engineering, bread and animal feed Cooking. A recipe whose level is above yours
shows "Needs Engineering 15" where its result would be.

Smelting is by hand for now (ore over coal at the grid makes an ingot); a furnace comes
with Engineering 30.

## Farming, taming and breeding with tools

- A **hoe** tills soil into **farmland** (right click); crops are sown on farmland only
  (`ground` in crops.ron says otherwise for a crop that wants something else). A hoe over a
  ripe crop adds its yield to rarer finds.
- A **taming treat** (Cooking 27) is taken by any animal you are skilled enough to tame,
  and wins its trust twice as fast as its own food (`taming_treat`, `trust:2`).
- **Seed mix** (Cooking 8) readies any kept animal to breed (`breeding_feed`).
- A creature too wild for your Hunter level, or a kept one your Farming cannot breed yet,
  turns food down instead of eating it for nothing.

## Fishing

`base/fishing.ron` (engine/src/fishing.rs) lists what bites: an item, the Fishing level it
takes, its experience, a weight, salt or fresh water (salt is a biome tagged `ocean`),
biome tags, and when (day, night, rain). A cast (`base:fish`, five seconds, a rod in hand,
water within nine blocks with open air above) lands something with a chance that starts at
about a third for the water's best fish at its level and grows with each level above it
and with the rod's tier, up to nine in ten; which fish is a weighted draw among those that
can bite, rarer ones a little more often with a better rod's yield. Fish cook at a
campfire (cooking.ron). Fishing tops out at level 75 for now (glimmer koi), with the cape
perk at 80.

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
except on a herb planter (planks and dirt at a crafting table, Engineering 5) for a player
whose Farming level reaches the plant's (1, 15, 40, 60, 80 by rarity); picking one there
teaches Farming. Waiting plants are saved with the world (`WorldMeta::regrowing`), and
`/grow` brings back those near you at once.

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
