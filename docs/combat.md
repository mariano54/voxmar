# Combat

Oct 7, 2026. How blows land and what they are struck with: the three styles, weapon
classes and weapons. Phase 4 of the build plan (P40 onward, the design wiki's "Build plan"
and "Items" tabs); the code is `engine/src/combat.rs`, the data each package's
`combat.ron`.

## Decisions

| # | Question | Decision |
| --- | --- | --- |
| 1 | Styles | Melee (stab, slash, crush), Ranged and Magic. A weapon fights in one; a creature attacks in one (`attack.style`) and may be weak to one (`weak`). |
| 2 | Hit rolls | RuneScape's: an attack roll `(level + 8) × (accuracy + 64)` against a defence roll `(level + 8) × (armour + 64)`. With nothing held or worn both bonuses are 0, which is the creatures' roll that was already in use. |
| 3 | Weakness | A creature's defence level counts half against the style it is weak to. |
| 4 | Max hit | Each weapon's max hit is the wiki's at the level that wields it, and rises a point for every eight levels above it. A bare fist is a weapon of max hit 1 at level 0: 1 + Attack / 8, as before. A blow does 1 to the max hit, then the timing scales it. |
| 5 | Wielding | A weapon needs its level in its style's skill (Attack for melee, Ranged, Magic), the player's own: boosts never help to wield. Held below its level it strikes as a fist; the tooltip says the level in red. |
| 6 | Timing | Each class has a cooldown, and the end of it is the beat (see "Feel" below). A click on the beat lands at full strength; the quickest, at half the cooldown, at 45%, so spam-clicking deals about 90% of timed damage per second (Mariano, 6 Oct) with every weapon. |
| 7 | Speeds | The wiki's class speeds divided by three, so a sword keeps the fist's 0.6 s that play was tuned on: dagger 0.4 s, sword 0.6, spear 0.73, axe 0.8, mace 0.87. |
| 8 | Reach | A spear reaches a block further. |
| 9 | Accuracy | Twice the max hit unless a weapon says otherwise. |
| 10 | Experience | As before: four a point of damage in the weapon's skill, a third of that in Hitpoints. |

## Weapons so far

| Weapon | Class | Level | Max hit | Made from |
| --- | --- | --- | --- | --- |
| Bone club | mace (crush) | Attack 1 | 3 | two bones and a stick, Crafting 1 |
| Stone sword | sword (slash) | Attack 1 | 3 | two cobblestone and a stick, Crafting 1 |
| Wooden axe | axe (slash) | Attack 1 | 2 | the tool |
| Stone axe | axe (slash) | Attack 1 | 3 | the tool |
| Copper sword | sword (slash) | Attack 5 | 5 | two copper ingots and a stick, Engineering 5 |
| Copper axe | axe (slash) | Attack 5 | 6 | the tool |
| Fang dagger | dagger (stab) | Attack 10 | 5 | a fang and a stick, Crafting 10 |
| Bronze sword | sword (slash) | Attack 15 | 7 | two bronze ingots and a stick, Engineering 12 |
| Bronze spear | spear (stab, reach) | Attack 15 | 8 | a bronze ingot on two sticks, Engineering 12 |
| Iron sword | sword (slash) | Attack 20 | 9 | two iron ingots and a stick, Engineering 15 |
| Iron axe | axe (slash) | Attack 20 | 11 | the tool |

Creatures' styles and weaknesses (systems agreed them, 7 Oct): wolves, foxes, cave crawlers
and crocodiles bite (stab) and are weak to crush; bears and snow leopards claw (slash),
weak to stab; lynxes and badgers claw, weak to crush; aurochs and bison charge (crush),
weak to stab; the frost stag crushes, weak to slash; the shambler crushes, weak to slash;
the ember salamander burns (magic), weak to crush; cave bats bite, weak to slash. The
armoured tortoise, shore crab and crystal beetle are weak to crush.

## Feel

Oct 7, 2026 (Mariano: "a mix between osrs, action combat and minecraft, focused on food
consumption, prayers, dodging and blocking, and attack rhythm"; numbers from the design
wiki's #feel tab, draft 49). The server's `fight.rs` decides what lands; the client's
`fight.rs` keeps the same rhythm for the meter and sends the guard. Every number below is
in `combat.ron` (`feel:`, each class's times, `dodges:`) or the creature's `attack:`, so a
pack can retune or replace all of it.

| # | Question | Decision |
| --- | --- | --- |
| 1 | Rhythm | A swing winds up (`windup`), lands, and the cooldown runs from its start; the cooldown's end is the beat. A click in the `buffer` before the beat is held and fires on it; a click within `beat` after it is on the beat too. On the beat a blow rolls with +5% accuracy (`beat_accuracy`). Earlier clicks fire at part strength (45% rising to full), earlier than half the cooldown they are ignored. One swing waits at most. |
| 2 | Pips | Each blow on the beat adds a pip; at three, the next one is a flourish (+20% max hit and the class's effect: dagger bleed, sword cleave, spear pin, axe sunder, mace daze), and the pips start over. An off-beat blow, an unguarded hit taken, a weapon switch or two seconds idle loses them. |
| 3 | Guard | Holding the use button with a weapon that guards raises it (a right click that opens a station or eats does not). It stops `stop` of a melee blow from its `arc` in front, half that against crush; never magic, never a heavy blow. Moving at half speed, no sprint. Raising it during your own wind-up cancels the swing, and the meter restarts from half. |
| 4 | Perfect block | Raised in the last `perfect` seconds before a blow lands: it takes nothing, staggers the creature 0.8 s (bosses 0.3) and the next click within a second is a counter (certain to hit, +25% max hit, full strength, a pip). A guard raised again within 0.4 s of lowering it opens no perfect window, so flicking it is no use. |
| 5 | Dodge | Left Alt with a direction dashes that way, alone steps back: 3 blocks in a quarter second, safe from blows from 0.04 to 0.22 s in, once a second; heavier armour dodges shorter, safe shorter and recovers longer (`dodges:`). The dash stops at walls and ledges. A blow dodged makes the next click within 0.8 s a riposte (on the beat by itself, a pip, full strength). |
| 6 | Telegraphs | A creature winds up before its blow (`attack.windup`, default 0.5 s; quick ones 0.35, big ones 0.7) standing still in its attack pose, and stands still to recover after (`attack.recovery`, default 0.6 s): the moment to strike it. A blow lands only if the player is still in reach then, so stepping back out of it works too. `attack.heavy` blows cannot be blocked. |
| 7 | Knockback | None (Mariano): a hit creature flinches for 0.15 s instead, drawn by the client. |
| 8 | Bodies | The player cannot walk into a creature's body: the client pushes them 0.2 blocks clear, so the camera never ends up inside a model. |
| 9 | Falling blows | A blow struck while falling, as Minecraft's critical hit, has +15% max hit (`falling_max_hit`). |

## Armour and shields

Oct 7, 2026 (the wiki's "Items" tab for sets, weights and Defence levels, its material tiers
for "Armour ×", and "Combat feel" for shields and dodges by weight). Data in `combat.ron`
(`sets:`, `shields:`), items in `blocks_armour.ron`, recipes in `recipes_armour.ron`.

| # | Question | Decision |
| --- | --- | --- |
| 1 | Slots | Head, body, legs, hands, feet, and the off-hand for a shield: six worn slots after the crafting grid (`inventory::WORN`). Each takes only the piece made for it. Right click with a piece in hand puts it on, swapped for what was there; shift-click in the inventory too. |
| 2 | Armour | A piece gives its slot's share of a bronze set's 36 (head 6, body 14, legs 10, hands 3, feet 3) times its set's material: hide 0.4, copper 0.7, bronze 1.0, iron 1.2. It adds to the defence roll as RuneScape's armour does. |
| 3 | Weight | Light (hide, leather) counts 0.8 against melee and full against arrows and spells; medium full against all; heavy (plate) 1.2 against melee and arrows, 0.6 against spells; robes 1.5 against spells, 0.7 arrows, 0.5 melee. A set's `resists` count half again (chainmail against slash). |
| 4 | Levels | Worn below its Defence level a piece gives nothing, and a shield does not guard: your own level, boosts never count. |
| 5 | Shields | In the off-hand a shield guards in place of the weapon: hide buckler 45% over 120° with a 0.28 s perfect window; wooden 55%, copper 60%, bronze 65%, iron 70%, each over 150° with 0.20 s. Shields stop arrows and thrown things too, never spells. A spear (two-handed) guards with itself, shield or not. A shield's own armour (1 to 7) adds to the defence roll. |
| 6 | Dodge | The heaviest armour worn picks the dodge: light or none 3.0 blocks, safe 0.04 to 0.22 s, every 1.0 s; medium 2.6, to 0.20, 1.2 s; heavy 2.2, to 0.18, 1.5 s. |
| 7 | Death | What is worn counts with the rest: the three most valuable things carried are kept. |

| Set | Weight | Defence | Material × | Whole set | Made from |
| --- | --- | --- | --- | --- | --- |
| Hide wraps (hood, tunic, trousers, gloves, boots) | Light | 1 | 0.4 | 14 | 24 hide, Crafting 1 |
| Copper (helmet, chestplate, leggings, gauntlets, boots) | Medium | 10 | 0.7 | 25 | 24 copper ingots, Engineering 5 |
| Bronze scale (helmet, scale shirt, leggings, gauntlets, boots) | Medium | 15 | 1.0 | 36 | 24 bronze ingots, Engineering 12 |
| Chainmail (coif, shirt, leggings, gloves, boots) | Medium, resists slash | 20 | 1.2 | 44 | 24 iron ingots, Engineering 15 |

Against a brown bear (level 28, slash) a Defence 15 player is hit about 68% of the time bare,
50% in bronze scale; a Defence 20 player in chainmail (66 against slash) about 32%.

Still to come in phase 4, in order: ranged weapons, ammunition and projectiles; armour slots and sets with resistances; the
weapons' specials (bleed, stun, brace); then modifications (P42).
