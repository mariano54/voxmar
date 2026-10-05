# How the systems connect

Oct 5, 2026. The links between creatures, breeding, farming, food, cooking, drops, skills
and the world, and the loops they make. Mariano asked for each system to connect to as
many others as possible. This is the map; the system docs hold the detail:
[creatures.md](creatures.md), [creature-ai.md](creature-ai.md),
[breeding.md](breeding.md), [food-and-cooking.md](food-and-cooking.md),
[drops.md](drops.md), [skills.md](skills.md), [creature-data.md](creature-data.md).

## The rule that makes it work

Nothing links to anything by name. The design philosophy's mechanism ("Everything
connects", "Fields decouple systems", "Modifier stages replace load order") carries all of
it:

- **Tags** say what things are: `meat`, `warming`, `alchemy:healing`, `aether`, `knife`.
  A recipe asks for `meat`; any creature's meat works. A tool tagged `knife` improves
  every loot entry that names knives.
- **Fields** say what places are like: temperature, moisture, light, wind, aether, soil
  fertility, (later) season and pollen. Every system reads them; none names the others.
- **Modifiers at stages** combine effects from different systems without order: a crop's
  growth is climate × soil × water × pollination; a drop's chance is base × tool × level.
- **Actions** are the only way things change, so a player, a tamed wolf, a machine and
  (later) an NPC farmer all shear, milk, plant and cook the same way.

So a mod that adds a creature with `meat` drops, a `warming` diet and a comfort range
appears in fitting biomes, feeds stews, keeps players warm, and breeds, with no edits to
anything else.

## The links

Each row reads "this system → feeds or changes → that system".

| From | To | Link |
| --- | --- | --- |
| Climate and biomes | Creatures | spawning by comfort and biome selectors; behaviour by temperature (nocturnal in heat, huddling in cold); coat genes (cold coats in the north) |
| Climate and biomes | Breeding | condition depends on comfort; a misplaced animal needs a heated or shaded barn |
| Climate and biomes | Cooking | drying needs dry air, fermenting a cool place; warm meals matter where it is cold |
| Aether | Creatures | supernatural species and variants live where it is high; wisps follow it |
| Aether | Breeding | mutation chance and supernatural births (rabbits become moonhares) |
| Aether | Food | aether ingredients strengthen dishes; Aether sight shows the field |
| Weather | Creatures | rain sends them to shelter; storms spook herds into stampedes; wind carries scent (approach from downwind) |
| Weather | Vitals | cold and heat bands; food counters them |
| Seasons (when added) | Creatures, breeding | breeding seasons, rut, migration, antler shedding, hibernation |
| Plants and trees | Creatures | diets: grazers eat grass and flowers, browsers eat leaves and fruit, bears raid berry bushes and hives, pigs root truffles |
| Plants and trees | Cooking | most ingredients; herbs and spices carry flavour and effects |
| Creatures | Plants | seed dispersal (diet seeds in drops and dung), grazing wears meadows, bees pollinate, crows and foxes raid crops |
| Creatures | Farming | husbandry and breeding are Farming; manure and guano make compost; pests pressure fields |
| Creatures | Cooking | meat, fish, eggs, milk, fat, honey; supernatural ingredients |
| Creatures | Combat | monsters to fight; hides, scales, pelts and feathers make armour (Defence); fangs and talons make weapons (Attack) |
| Creatures | Mining | cave creatures guard deep places; crystal tortoise shells are mined; bat guano lies in caves; salt licks (Mining) draw deer |
| Creatures | Wear | herds trample game trails through grass, as players' feet do (`server::wear`) |
| Farming crops | Breeding | feed and breeding food; whisper oats raise mutation |
| Farming crops | Cooking | grains, roots, vegetables, fruit |
| Cooking | Breeding | feed and treats (condition, breeding, taming) |
| Cooking | Hunter | bait for traps and lures; Quiet steps stew |
| Cooking | Combat | cooked food heals at once; Steady hand, Hearty |
| Cooking | Mining and exploring | Dark sight for caves, warm meals for mountains, Deep breath for water, preserved food for long trips |
| Combat | Farming | monsters' rare seeds; protecting herds from wolves and fields from pests |
| Hunter | Breeding | taming brings new blood; inbreeding makes it needed |
| Drops | Everything | one loot system; tools and levels shift chances; conditions read genes, weather, time, aether |
| Skills | Everything | levels gate and refine; combat level decides which monsters bother you |

## The loops

Loops are what make the links matter: each one is a reason to care about two systems at
once.

1. **The farm loop.** Crops become feed; fed animals give products and manure; manure
   becomes compost; compost raises soil fertility; fertile soil grows more crops.
2. **Pollination.** Bees need flowers within 16 blocks; crops near hives grow faster
   (a pollination modifier on the growth rate); hives give honey and wax; honey goes into
   cakes, treats and jam.
3. **Predators and prey.** Wolves thin deer; hunting wolves lets deer recover; over-hunting
   empties a valley for days. The population process keeps the numbers; Hunter reads the
   signs.
4. **The aether ladder.** Aether places hold supernatural plants and creatures. Their
   ingredients make the strongest dishes, which let players survive deeper in fantasy
   biomes and caves, where more aether is. Aether feed carried home lets a mundane farm
   breed supernatural animals.
5. **The warmth loop.** Cold weather brings Chilly and Freezing; warm meals need chili,
   ginger or emberfruit, which grow in hot places; wool from cold-coated sheep makes warm
   clothes. Cold and hot regions trade with each other because of it.
6. **The combat loop.** Monsters drop rare seeds, bones (bone meal) and hides; hides become
   armour; cooked food heals mid-fight; stronger fighters go where rarer monsters are.
7. **The cave loop.** Dark sight food from glowcaps; guano from bat roosts for compost;
   crystal tortoises and crawler nests in deep places; Mining levels open those depths.
8. **The pest loop.** Crows, foxes and boars come for crops and eggs; scarecrows, fences
   and a tamed wolf answer them, which pulls farmers into Hunter.
9. **Seeds on the move.** Animals carry seeds in their drops and dung; following a herd is
   a way to find a rare wild crop, and a pasture slowly grows what its animals ate.
10. **New blood.** Inbreeding lowers a herd; taming wild stock (Hunter) refreshes it; wild
    stock is wild, so selective breeding (Farming) starts again: two skills that need each
    other forever.

## Plants wanted

The world has 11 fantastical plants among about 170, and Mariano wants about a fifth of
plants and creatures to be supernatural. These 20 plants and crops close the gap and each
connects to a creature or dish. The biome thread owns where they grow; this list goes to
it through the coordinator.

| Plant | Where (aether high) | Use |
| --- | --- | --- |
| Starwheat | plains, meadows | flour for star bread (Dark sight) |
| Moonmelon | moonlit meadow | cooling fruit; moonhare food |
| Ember pepper | ember wastes | strongest Warm meal and Fire ward |
| Frost turnip | glacier edges, snowy tundra | Cool meal; frost stag food |
| Dreamberry bush | dark forest | calms animals (taming treats); Quiet steps |
| Whisper oats | where whisperfern grows | feed that raises mutation chance |
| Sky lotus | high mountain lakes | Deep breath |
| Bloodroot | dark forest floor | healing (Hearty) |
| Sunpetal | sunspire downs | sunwing food; light |
| Crystal moss | crystal expanse | crystal tortoise food; crystal salt |
| Aether truffle | under high-aether oak and chestnut woods | pigs root it; legendary dishes |
| Ghostcap | ghost forest | brief invisibility to creatures |
| Thunderbloom | mountains, grows where lightning struck | appears after storms; Steady hand |
| Tidekelp | high-aether coasts | fish bait; Deep breath |
| Wyrmroot | dune sea | sand wyrm bait |
| Hearthgrain | cold steppes | bread that is a Warm meal |
| Honeydew lily | high-aether flower meadows | bees make aether honey |
| Lantern gourd | swamps | hollowed, a light; seeds as feed |
| Silvermint | silverwood glade | lifts curses (the silver hart's); befriending silver harts |
| Cinder moss | ember wastes | ember salamander food |

## What other threads need to know

- **Biome thread:** the plants above, and creature habitats use biome tags and names, so
  keep tags stable or tell me when they change.
- **Graphics:** creatures are drawn first as plain Bevy box meshes outside `render/`, as
  other players are (`client/src/net.rs`). A lit, instanced creature pass (boxes of parts,
  lit by chunk light like dropped items) would be the proper home; I will send the exact
  request.
- **Game UI:** the skill guides (lines in skills.ron), a bestiary screen (creatures seen,
  their drops as words), an inspect panel (condition, genes in words), creature health
  bars, and food effect chips beside the vitals ones.
- **Weather:** creatures read the wind (`engine::weather::gust`) for scent and rain and
  storms for shelter and stampedes; seasons, when they come, drive breeding seasons.
