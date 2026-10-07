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
| 6 | Timing | Each class has a cooldown: a blow a full cooldown after the last lands at full strength, the quickest at half the cooldown at 45%, so spam-clicking deals about 90% of timed damage per second (Mariano, 6 Oct) with every weapon. |
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
| Iron sword | sword (slash) | Attack 20 | 9 | two iron ingots and a stick, Engineering 15 |
| Iron axe | axe (slash) | Attack 20 | 11 | the tool |

Still to come in phase 4, in order: bronze weapons and spears once bronze lands (P30);
ranged weapons, ammunition and projectiles; armour slots and sets with resistances; the
weapons' specials (bleed, stun, brace); then modifications (P42).
