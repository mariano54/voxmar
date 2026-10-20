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
| Flint axe | axe (slash) | Attack 1 | 2 | the tool |
| Stone axe | axe (slash) | Attack 1 | 3 | the tool |
| Flint spear | spear (stab, reach, braces) | Attack 1 | 3 | a flint on two sticks, Crafting 2; 1 in 9 from Old Tusk |
| Tusk club | mace (crush) | Attack 5 | 5 | Old Tusk's great tusk and a stick, Crafting 3 |
| Copper sword | sword (slash) | Attack 5 | 5 | two copper ingots and a stick, Engineering 5 |
| Copper axe | axe (slash) | Attack 5 | 6 | the tool |
| Fang dagger | dagger (stab) | Attack 10 | 5 | a fang and a stick, Crafting 10 |
| Bronze sword | sword (slash) | Attack 15 | 7 | two bronze ingots and a stick, Engineering 12 |
| Bronze spear | spear (stab, reach) | Attack 15 | 8 | a bronze ingot on two sticks, Engineering 12 |
| Iron sword | sword (slash) | Attack 20 | 9 | two iron ingots and a stick, Engineering 15 |
| Iron axe | axe (slash) | Attack 20 | 11 | the tool |
| Sling | sling (ranged, reach 12, shoots any cobbled stone) | Ranged 1 | 2 | twine, hide, twine, Crafting 1 |
| Short bow | bow (ranged, reach 20, two hands, shoots arrows) | Ranged 3 | 4 | three sticks and three twine, Crafting 3 |

A ranged weapon is a class with `ammo` (an item or `tag:`): each shot spends one when it
lands, hit or miss, and with none the server starts no swing and says what it needs. A shot
needs a clear line from the eye to the middle of the target as well as reach; otherwise it
is the same blow, rhythm and experience as any other. A shot that hits may be left lying
where the target stood to pick up again, the shot that kills it too: `recover` is that
chance (sling stones 0.25, arrows 0.5). A miss lands to pick up again by `recover_miss`
(stones 0.5, arrows 0.8): by the target if it was in reach, else as far as the weapon
reaches toward it. Either stops short of anything solid on the flight's line and backs off
until its own box is clear, then falls to the ground. The rest break or fly out of sight. Both are rolled by the player's own count
of hits and of misses (`Draw::Recover`, `Draw::Stray`), never the tick. Arrows: four from
flint or cobblestone, a stick and a feather, Crafting 5.

A class may also say what drawing costs (`combat.ron`, load-checked; only a class with
ammo may): `spoiled_by_hits` lets a draw go when a creature's blow hurts the player, as
raising the guard does (the meter starts again from `cancel_to`, and the shot spends
nothing), and `drawing_speed` is the share of their pace the player keeps while a shot is
drawn (bow 0.4, sling 0.7). The server spoils the draw and says so on the `Hurt` it
sends (`spoiled`, protocol 56); the client lets its draw go only then. The slowing is the client's own, as the guard's is (gate check `sling draw`).
A draw's last moments cannot be spoiled across a slow connection: the server lands a swing
as its wind-up ends, and a creature's blow is judged up to the player's round trip later
(`lag_allowance`, at most 0.2 s), and a tick at most after that, so the last round trip of a
draw, never more than 0.2 s and a tick, has already loosed. Since the client follows the
server's `spoiled`, the two never disagree on a spoil.

Creatures' styles and weaknesses (systems agreed them, 7 Oct): wolves, foxes, cave crawlers
and crocodiles bite (stab) and are weak to crush; bears and snow leopards claw (slash),
weak to stab; lynxes and badgers claw, weak to crush; aurochs and bison charge (crush),
weak to stab; the frost stag crushes, weak to slash; the shambler crushes, weak to slash;
the ember salamander burns (magic), weak to crush; cave bats bite, weak to slash. The
armoured tortoise, shore crab and crystal beetle are weak to crush.

## Feel

Oct 7, 2026 (Mariano: "a mix between osrs, action combat and minecraft, focused on food
consumption, prayers, dodging and blocking, and attack rhythm"; numbers from the design
wiki's #feel tab, drafts 49 and 58, and the review of 7 Oct). The server's `fight.rs` decides what lands; the client's
`fight.rs` keeps the same rhythm for the meter and sends the guard. Every number below is
in `combat.ron` (`feel:`, each class's times, `dodges:`) or the creature's `attack:`, so a
pack can retune or replace all of it.

| # | Question | Decision |
| --- | --- | --- |
| 1 | Rhythm | A swing winds up (`windup`), lands, and the cooldown runs from its start; the cooldown's end is the beat. A click in the `buffer` before the beat is held and fires on it; a click within `beat` after it is on the beat too. On the beat a blow rolls with +5% accuracy (`beat_accuracy`). Earlier clicks fire at once at part strength: 45% at half the cooldown (`quickest`, `spam_strength`), rising in a straight line to full at the cooldown, so spamming deals 90% of timed damage per second and no click off the beat beats waiting for it (for every class; tested). Earlier than half the cooldown they are ignored. One swing waits at most, struck with what was in hand when it started; switching what is in hand drops it. |
| 2 | Pips | Each blow on the beat adds a pip; at three, the next one is a flourish (+20% max hit and the class's effect: dagger bleed, sword cleave, spear pin, axe sunder, mace daze), and the pips start over. An off-beat blow, an unguarded hit taken, a weapon switch or two seconds idle loses them. A counter or riposte is on the beat by itself: one pip, like any. The server tells the pips with every blow it answers, so the client's meter shows its count. |
| 3 | Guard | Holding the use button with a weapon that guards raises it (a right click that opens a station or eats does not). It stops `stop` of a melee blow from its `arc` in front, half that against crush (`crush_through`); never magic, never a heavy blow, missiles only with a shield. Whether it took the blow is judged by when the blow landed, not when the server got to it, and only once the server has handled every message that arrived before then (a raise that came in during a stall still counts; the blow is then judged up to a tick late, with where both stand by then). The trip from the client is not made up for: `reach_slack` absorbs it. Moving at half speed, no sprint. Raising it during your own wind-up cancels the swing, and the meter restarts from half. |
| 4 | Perfect block | Raised in the last `perfect` seconds before a blow lands: it takes nothing, staggers the creature 0.8 s after its recovery (bosses 0.3) and the next click within a second is a counter (certain to hit, +25% max hit, full strength, a pip). A guard raised again within 0.4 s of lowering it opens no perfect window (`no_flicker`), so flicking it is no use. Bonuses to a max hit, a sunder's share of defence and a flourish's share of damage round by chance, so small ones count on average. |
| 5 | Dodge | Left Alt with a direction dashes that way, alone steps back: 3 blocks in a quarter second, safe from blows from 0.04 to 0.22 s in, once a second; heavier armour dodges shorter, safe shorter and recovers longer (`dodges:`). The dash moves at exactly its speed, so it covers its distance at any frame rate, and stops at walls and ledges. Only on foot on the ground (or 0.15 s off it, `dodge_grace`), not flying, eating or guarding; the server takes one up to 0.1 s early (`dodge_early`) for a client's clock ahead, and a dodge it refuses puts the player back where it has them. A blow dodged makes the next click within 0.8 s a riposte (on the beat by itself, a pip, full strength). |
| 6 | Telegraphs | A creature winds up before its blow (`attack.windup`, default 0.5 s; quick ones 0.35, big ones 0.7) in its attack pose (sent to players the tick it comes on, not with the every-other-tick movement updates), and stands still to recover after (`attack.recovery`, default 0.6 s): the moment to strike it. It faces its target as the wind-up begins, with that server-chosen yaw in the first attacking update (even with no tracking ticks; a target directly above or below keeps its previous yaw). For the first 60% of the wind-up (`creature_tracking`) it turns to follow its target and runs at it (`tracking_pace`; a creature's `attack.pace` may say walk, as the shambler's does), then its aim locks; as the blow lands it lunges up to `attack.lunge` blocks the way it faces (default 1.2; quick animals 1.5, big ones 1.0), stopping at the player's body or a wall. The blow lands if the player is in its reach from there: its `attack.reach`, half its body and 0.3 for the arm from its centre, and `reach_slack` for lag. So backing off at a walk does not escape it (tested for a bear, frost stag, cave crawler and ember salamander); a dodge does. A bear threatens 1.4 + 0.6 + 0.3 + 0.6 = 2.9 blocks from its centre to the player's, 3.9 with its lunge. `attack.heavy` blows cannot be blocked. |
| 7 | Knockback | None (Mariano): a hit creature flinches for 0.15 s instead, drawn by the client. |
| 8 | Bodies | The player cannot walk into a creature's body: the client pushes them 0.2 blocks clear (`creature_clearance`), so the camera never ends up inside a model. |
| 9 | Falling blows | A blow on the beat struck at least 0.3 blocks into a fall (`falling_drop`), as Minecraft's critical hit, has +15% max hit (`falling_max_hit`). |
| 10 | Reach | From the eye to a creature's nearest edge: 2.8 blocks (`feel.reach`, Minecraft's 3 less a little), a fist or dagger 2.5, a spear 3.8 (each class's `reach`). The server allows 0.6 more (`reach_slack`) for the lag of a view of the fight, and at a moving creature as far as it goes in 0.15 s (`reach_lag`), counted once. A swing that lands out of reach is a miss off the beat: no pip, no flourish spent. Only a spear outreaches a bear. A sword's flourish cleaves the nearest other wild creature within reach and 60 degrees of the look (`cleave_cone`); kept animals are never cleft. Touching a creature (tending it, looking closely at it, letting it go) reaches further whatever is in hand: 3.6 blocks (`feel.touch`) and `reach_slack`; the crosshair picks a creature that far, but a click strikes it only within the weapon's reach. A click at one past the weapon's reach whiffs (Oct 15): the hand swings at the air with a whoosh (`combat_fx.ron`'s `whiff`), nothing is asked of the server and the rhythm is untouched; only from rest, once a cooldown, not while guarding (`Fighting::whiff`; gate line `whiff`). |
| 11 | Defence experience | A blow turned aside (missed by the defence roll, or perfectly blocked) gives Defence experience, four a point of the damage it could do, only while the player fights back (they landed a damaging blow on that creature in the last 10 s, creatures.ron's `defence.fought_within`) and only from a creature of at least half their Defence level (`defence.least_level`; the wiki's draft 63): standing in a cave of bats teaches nothing. |
| 12 | Inspect or guard | A right click at a creature looks closely at it, unless it is fighting the player (winding up, or blows traded within 6 s, `engaged`) and what is held or worn guards: then it guards. Food or a tool it takes tends it first; a ripe crop is picked before armour in hand goes on. |
| 13 | Lag | A guard or dodge is judged as of when the player saw the wind-up: by when their message arrived, less their round trip (the server pings each player every second and smooths what comes back), never more than 0.2 s (`lag_allowance`), so a slower connection earns no wider a window. A creature's blow on a player waits that long before it is judged, so their answer has time to come; counters and ripostes count the same way. Breaking blocks is judged by arrival the same way (`actions`). |
| 14 | Clocks | Oct 15. A player's swings, guard and dodges keep the clock their messages arrive by, as their client keeps them. A creature's times are the server's game time, its ticks, as its moves are: its blow comes down the tick its wind-up runs out, and a bleed, pin or sunder lasts as many ticks, however long a busy server takes over them. The blow is then judged on the players' clock at the moment its wind-up ran out: on the tick it came due, or a wind-up after it began on that clock if that is sooner, as the player's client tells it, so ticks running behind never judge a guard or dodge later than the player saw the blow come (the server test `a_blow_answered_by_its_tell_counts_however_the_ticks_run`: a bear's blow guarded and dodged by its tell is a perfect block and a dodge with ticks on time, at 110 ms each, and after a catch-up burst). After a stall the server runs up to 10 ticks at once to catch up, so a wind-up that falls in them can come down in as little as about 0.25 s of the clock; the client tells it by its own clock all the same, so what it shows and what is judged agree. Before: a wind-up ran out by the clock, so on a server running behind a blow landed in fewer ticks than the creature stood still for, and in tests, whose ticks run ahead of the clock, it hardly landed at all. The server test `a_creatures_blows_keep_their_beat_in_game_time_however_long_the_ticks_take` checks a wolf's blows come on the same ticks with the clock running twice as fast as its ticks. |

## Feel on screen

Oct 14, 2026 (Mariano: "combat sucks ... the sound effects and visual effects are not
satisfactory, animations are bad"). Everything here is drawn and heard on the client from what
the server already says; no rule, time or number of the fight changes. The code is
`client/src/fight_fx.rs` (and `creatures.rs` for bodies, `held.rs` for the hand); every number
and sound is `core/combat_fx.ron`'s, and the hand's strokes are the texture pack's `held.ron`.

| # | Question | Decision |
| --- | --- | --- |
| 1 | A blow landing | A hit-stop: the creature freezes for 0.045 s plus 0.12 s × the share of its health the blow took (at most 0.11 s), shaking a little, the kind of blow scaling it (a beat 1.25×, a flourish 2×, a counter 1.8×). It flashes white, then red, then flinches: pushed back 0.12 of its height and tipped 11° away from the striker, squashed, easing back over 0.22 s. That flinch is weighed (`hit.weigh`): a body of width² × height 0.5 (a wolf) flinches as given, a smaller one more and a bigger one less by the cube root of the ratio (a rabbit 1.8×, a deer 0.8×, a bear 0.63×, held between 0.4× and 1.8×), and a scratch 0.7× up to 1.5× for a blow taking half its health or more. Its position never moves (no knockback, decision 7 above). The swing in hand holds still as long as the creature does, then the hand is knocked back. |
| 2 | Sparks | A burst of glowing sparks where it was struck (on the side facing the eye) and tufts of its own coat; a shelled creature throws chips of its shell's colour. On the beat more, a flourish three times as many. The particle setting scales them. |
| 3 | Sounds | Synthesized at startup from layers (a contact click, a body's falling thump, a band of noise for the surface, crackle, rings, swept air), saturated and given a small room, a few takes each. A hit sounds the weapon's style; a big creature's lower, a small one's higher; a shelled one clacks. On the beat a bright tick plays over the hit, so the rhythm is heard; a flourish crunches and shimmers; a counter clashes and rings; a kill thumps deep. Swings whoosh by class (a dagger quick and high, a mace low and long, a bow's string). A miss is air; a blow that hurts thuds; a block knocks; a perfect block rings like a parry. A package may give recordings for any of them. |
| 4 | Creature strikes | A creature coils through its wind-up and snaps into its strike as the blow lands (at the wind-up's own time, or the server's word if sooner), then settles, by its attack's style: a bite pulls its head back and snaps it forward, claws rear up and swipe their front limbs down, horns and hooves drop the head and drive through, a spell rears and flares. Coiling it plants its feet (it follows where the server has it only `plant` as closely as walking), and the lunge darts the rest of the way (`dart` times as quickly), so the blow comes from where the coil was. After the blow it stands in its `recover` stance, head low and weight back, while the server holds it still for its attack's `recovery`, and is back to standing as that ends: the moment to hit back reads as one. The coil is the tell to block or dodge by. The gate's "combat feel: fights" checks every strike seen planted, coiled deep and, when it stood its recovery out, showing its recover stance. |
| 5 | Stagger | A creature staggered (a perfect block, or a mace's daze cutting its wind-up short) is told to the client as its tell `STAGGERED` for as long as the server holds it. A daze cuts its wind-up short with no strike (an interrupted wind-up no longer reads as the blow landing); a perfect block parries a blow that came, which may show before it reels. It reels where it stands: over `ease` (0.15 s) it eases into its `stagger` stance, head drooped, a slight crouch and its weight back, whatever of its strike still showed giving way to it, and its body sways side to side (0.22 radians, 1.6 times a second); it eases back out as long once the stagger is over. Dizzy sparks (in its coat's colour unless given) circle up over its head and it thuds with a dazed wobble (`stagger` sound), fainter further off. One first seen staggered reels, unheard. The gate's "combat feel: stagger" checks that the bear the perfect block staggers is shown staggered, its stance shown whole (by game time: all the way in after `ease`), and takes the shot `creature-stagger`; client tests judge that a daze shows no blow. |
| 6 | Deaths | A killing blow freezes 0.12 s and dips the view; once the freeze is over the body topples onto its side over its `death.fall` (0.3 s) and lies there a beat, then over the last 12% of its time to its loot sinks and shrinks away, and goes up in a puff of smoke where it lay as its loot bursts out (the server's `death.drops_after`, by the game's clock from the blow, so the freeze never puts the puff behind the loot). It must lie at least 0.3 s past the freeze and its going (`LIES`, checked at launch: `FightLook::unlain`), so a pack whose death `seconds` is under 0.6 with the default 0.3 s fall no longer loads. Gate: the RangedFeel deer is shot lying fallen (`creature-death`), drawn rolled all the way over at its full size and not sinking, and must puff within a step and a half of its loot. |
| 7 | The player | A blow that hurts tips the view 4° toward the side it came from and reddens the screen's edges; a block jolts the hand and shakes the view; a perfect block throws sparks before the eyes. A flourish, counter or kill dips the view. Screen shake in the settings turns every movement of the view off. |
| 8 | The hand | Every attack stroke winds up before it cuts (`wind` in `held.ron`): a sword draws back and slashes across, a dagger and a spear draw back and thrust, a club and an axe rise overhead and come down, a bow draws to the cheek and looses, a sling swings up behind the shoulder and whips forward overhand. |
| 9 | Shots | The server judges a sling or bow shot as its wind-up ends, along a clear line from the eye (no missile exists there). On this side the missile is loosed from the hand as the draw lets go, timed so it reaches the creature as the blow is judged (never before half the draw), and kicks the hand back. It flies with a slight arc (a stone 34 blocks a second, flat; an arrow 52, arcing more), leaving a fading streak, and where it arrives throws a puff (the creature's coat, else dust) with its own sound: a stone smacks a body and clatters on the ground, an arrow thocks into a body and sticks quivering in the ground. A shot the server says missed sails on past the creature to land beyond. A shot whose swing is cancelled before its blow (the guard raised, a dodge, or what is held changed) strikes nothing, as the server judges no blow: one not yet loosed never is, and one flying sails on past and is gone where it lands without a puff or a sound (the gate's "shot cancel", shot `shot-cancel`). Each class that takes ammunition must have its shot (`shots` in `combat_fx.ron`, checked at launch). |
| 10 | Every blow told | Oct 14 (the first fights audit). Every creature's wind-up is told by ear as well as by its coil: its own sound if it has one (a wolf snarls, a badger hisses, a fox yips, an aurochs bellows, a bear roars), else its style's growl, draw or hum. Each swells to its loudest as the blow lands. Only a boss's dips the music; the rest are fainter further off (full within 6 blocks). Every blow must have one, checked at launch, and every wind-up is at least 0.3 s (`fight_fx::READABLE`), with its sound peaking no later than the blow. |
| 11 | Other players | Oct 15 (feel pass gap 6, first half). Other players are drawn as figures, not boxes: a body of boxes from `base/player_look.ron` (the villagers' shapes, a coat and skin picked by a hash of the name from its palettes, so a player looks the same to everyone), skinned and lit like a creature. The figure follows the position the server sends (eased, `follow`), turns to its yaw (`turn`), tilts its head to its look (`look` of the pitch, at most `look_most`), and swings its legs and arms with its pace (`stride`, `swing`, full at `full_pace`). The right arm is carried forward (`carry`) and swings less (`carry_swing`), and what it holds is drawn in that hand (`hand`): the server now sends each player's pitch and held item (protocol 51). When the last other player leaves, the one left is told so once, so no figure stays behind. Their fight shows as the server has it (protocol 54, `PlayerView::fight`: the swing under way with its seconds and wind-up, a count of blows landed, the guard): the right arm is raised through a swing's wind-up to `fight.raise` and, as the count moves on, brought down through `fight.strike` in `fight.follow` seconds; the guard holds it at `fight.guard` with what is in it turned `fight.guard_turn` across; a bow or sling drawing holds both arms out at `fight.draw`, and loosing it strikes nothing down. Nothing is guessed on the client: a swing the server cancels just eases back. Players are told of others only as they change (a view sent again only once it differs, a name only when they come into sight) and only within `players_seen` blocks (the server's limits file, 160 by default), with those gone or out of sight named once; one already shown is hidden only `players_seen_margin` (8) further on, so a player at the edge does not blink. Names go to everyone near or far (`Players::names` for those out of sight, once; a player who leaves the server is named once more with an empty name, and the client forgets them), so a creature kept by a player far off still shows who keeps it; an item picked up by a player out of sight shrinks away where it lay rather than flying to the eye. Gate checks: `others`, `others: fights` (shots `other-player`, `other-player-draw`). |
| 12 | Circling between bites | Oct 15 (feel pass gap 7). A creature whose attack has a `circle` (`creatures.ron`) no longer stands nose to nose and bites again: after its blow and its recovery it backs off and circles the player for `seconds`, to a point `away` blocks past its reach and `round` degrees round them (its side fixed by its seed), at `pace` of its run, facing them all the while; then it closes and strikes. The wolf circles 0.8 s, 0.6 blocks out, 50° round, at 0.7 of its run; the seconds count from the end of its recovery, as it does not think while held. The circle comes on top of its `every`, so its bites come further apart and the player gets an opening between them; its damage went from 1-3 to 2-5 to keep the fight as hard. The bot (`first_age_fights_balanced`, 12 runs each, the cloud) before: open 43% left, 2 of 12 died, 9.0 s; guarding 86%, 7.6 s. After (Oct 20, circling counted once, at 0.7 pace): open 30% left (worst 0%), 1 of 12 died, 8.5 s; guarding 80% (worst 55%), 7.7 s. The engine's reckoning (`first_fights`) puts the open fight at 22% left, within its 15% to 45%. Load-checked: seconds 0.3 to 2, away 0.3 to 2.5, pace 0.2 to 1, round 15 to 120. The server test `a_wolf_circles_between_bites` and the gate's "circling" (after the climb step's bite) check it backs off past its reach and goes 15° or more round before it bites again. |

## Armour and shields

Oct 7, 2026 (the wiki's "Items" tab for sets, weights and Defence levels, its material tiers
for "Armour ×", and "Combat feel" for shields and dodges by weight). Data in `combat.ron`
(`sets:`, `shields:`), items in `blocks_armour.ron`, recipes in `recipes_armour.ron`.

| # | Question | Decision |
| --- | --- | --- |
| 1 | Slots | Head, body, legs, hands, feet, the off-hand for a shield, and a charm: seven worn slots after the crafting grid (`inventory::WORN`). Each takes only the piece made for it; the charm slot takes an item tagged `charm` (never armour too, checked at load), whose effect holds while it is worn (`Wearing`, docs/status-effects.md): Old Tusk's Charge Charm (1 in 65) gives Tusk Stride, 5% faster on foot. Right click with a piece in hand puts it on, swapped for what was there; shift-click in the inventory too. |
| 2 | Armour | A piece gives its slot's share of a bronze set's 36 (head 6, body 14, legs 10, hands 3, feet 3; `armour.slots`) times its set's material: hide 0.5 (tanned leather 0.65 to come), copper 0.7, bronze 1.0, iron 1.2. It adds to the defence roll as RuneScape's armour does. A creature's attack roll adds 0.8 of its level (`creature_accuracy`) unless it sets its own `accuracy` (the wiki's draft 62). |
| 3 | Weight | Light (hide, leather) counts 0.8 against crush and full against the rest; medium full against all; heavy (plate) 1.2 against melee and arrows, 0.6 against spells; robes 1.5 against spells, 0.7 arrows, 0.5 melee, and dodge as light. A set's `resists` count half again (`armour.resisted`; chainmail against slash). All in `combat.ron`'s `armour:`. |
| 4 | Levels | Worn below its Defence level a piece gives nothing, and a shield does not guard: your own level, boosts never count. |
| 5 | Shields | In the off-hand a shield guards in place of the weapon: hide buckler 45% over 120° with a 0.28 s perfect window; wooden 55%, copper 60%, bronze 65%, iron 70%, each over 150° with 0.20 s. Shields stop arrows and thrown things too, never spells. A spear (two-handed) guards with itself, shield or not, and its guard braces (`brace`): held up facing a charge, the charger runs onto the point and is stopped, heavy or not (docs/bosses.md, "Charges"). A shield's own armour (1 to 7) adds to the defence roll. |
| 6 | Dodge | The heaviest armour worn picks the dodge: light or none 3.0 blocks, safe 0.04 to 0.22 s, every 1.0 s; medium 2.6, to 0.20, 1.2 s; heavy 2.2, to 0.18, 1.5 s. |
| 7 | Death | What is worn counts with the rest: the three things carried worth most are kept, counted a unit at a time (a stack or a pouch keeps one), a grave-ward amulet keeping a fourth besides itself. Worth is gold on the wiki's P51 curve (`engine/src/value.rs`, `value.ron`): 2 × 2.5^((level − 1) / 15), to the nearest 5 above 100, × the rarity's 1, 2, 5, 12 or 30, where the level is what wears or wields it or the least recipe level that makes it; a made thing is worth at least 1.25 × what goes into one. An iron sword is 15, an iron pickaxe 24, a chainmail shirt 50, an iron ingot 5. The numbers, and how many are kept and for how long, are `value.ron`'s. |

| Set | Weight | Defence | Material × | Whole set | Made from |
| --- | --- | --- | --- | --- | --- |
| Hide wraps (hood, tunic, trousers, gloves, boots) | Light | 1 | 0.5 | 19 (3 + 7 + 5 + 2 + 2, each piece rounded) | 25 hide, Crafting 1 |
| Copper (helmet, chestplate, leggings, gauntlets, boots) | Medium | 10 | 0.7 | 25 | 25 copper ingots, Engineering 5 |
| Bronze scale (helmet, scale shirt, leggings, gauntlets, boots) | Medium | 15 | 1.0 | 36 | 25 bronze ingots, Engineering 12 |
| Chainmail (coif, shirt, leggings, gloves, boots) | Medium, resists slash | 20 | 1.2 | 44 | 25 iron ingots, Engineering 15 |

Against a brown bear (level 28, slash, accuracy 0.8 × 28 = 22) a Defence 15 player is hit
about 76% of the time bare, 63% in bronze scale; a Defence 20 player in chainmail (66 against
slash) about 43%. A set costs 25 of its material: hood 5, body 8, legs 7, gloves 2, boots 3.

## Prayers

Eighteen prayers (`prayers.ron`, `engine/src/prayer.rs`, `server/src/prayers.rs`), each opening at a Magic level and
draining points a minute while on. Points are `points` + `per_level` × Magic (10 + 1 a level), saved with the player.

| Question | Decision |
| --- | --- |
| Turning on | A quick key (X V B N G, set in the keys tab) or the book: needs the Magic level and a point left. One ward at a time and one boost of each stat: the last turned on puts out the one it shares a kind with. |
| Draining | The prayers on drain together by the tick; at none left every prayer goes out. Prayers off drain nothing (flicking them is allowed: it costs the keys, not points). |
| Points back | With no prayer on and no blow either way for `calm` (10 s), one point every `regain` (20 s). Praying at an altar fills them, and takes an offering in hand (a bone) for Magic experience first. |
| Boosts | Attack, Strength and Defence by a share of the level, rounded by chance so a small one still counts. |
| Wards | A ward stops `ward` (0.8) of a blow of its style from a creature (a boss's blow only `boss_ward`, 0.5, of it: docs/bosses.md; a player's will get its own share when there are player fights), never past what a guard stops: the larger stop counts. |
| Mending | Health comes back faster. |
| Death | Keepsake keeps one more item, never past the worth rule's most (4 with a grave ward); death puts out every prayer. |
| Last Breath | A blow that leaves the player under its share of health heals a share, then costs every point. |

The book is a page of the character screen (its Prayers button, `client/src/hud/prayers.rs`): the points as a meter, then a
row per prayer with the Magic level it opens at, its drain a minute, what it does (written from its effects and the rules'
wards through the `prayer.effect.*` lines, so a mod's prayer reads right with only its name) and its quick key. A click turns
a prayer on or off as its quick key does; the server decides. Over the hotbar a strip shows the points and the names of the
prayers on while any is on or the points are down. A dead player's Pray is ignored, so turning prayers off after dying cannot
lose the Keepsake.

Still to come in phase 4, in order: food timing; ranged weapons, ammunition and projectiles; then modifications (P42). The
vows come with the temple rites.

## First fights audit

Oct 14, 2026. The bot is `first_fights_measured` in `server/tests/gameplay.rs`; run it by hand with `--ignored --nocapture`. It plays a new player with a stone sword at Attack 5 and 60 health, swinging on the sword's 0.6 s cooldown and never guarding or dodging, against each first-age creature until it dies. The cloud run:

| Creature | Wind-up | Kill | Swings | Blows back (damage) | Read 0.3 s or more |
| --- | --- | --- | --- | --- | --- |
| Wild boar | none (it flees) | 21.8 s | 11 | 0 (0) | none to read |
| Badger | 0.35 s | 5.6 s | 10 | 5 (8) | 5 of 5, shortest 0.40 s |
| Fox | 0.35 s | 2.0 s | 4 | 1 (0) | 1 of 1, shortest 0.40 s |
| Aurochs | 0.70 s | 5.0 s | 9 | 3 (6) | 3 of 3, shortest 0.75 s |
| Wolf | 0.50 s | 15.8 s | 27 | 13 (27) | 13 of 13, shortest 0.55 s |
| Brown bear | 0.70 s | 61.5 s | 103 | 35 (85) | 35 of 35, shortest 0.74 s |

Every wind-up was on screen 0.4 s or more before its blow; the server sends a new pose at once. The gap was sound: before this change only bosses' wind-ups made a sound, so a blow from a creature at the edge of the view or behind came with no warning. Now every blow is told (decision 9 above). Wind-up times, and so the server's timing, are unchanged. The wolf (27 damage over 16 s, more than a new player's 20 health) and the bear are not fights for a new player who stands and trades blows. Their numbers are left for balance.

## Nomad hunting

Oct 15, 2026. The short bow came down to Ranged 3 and Crafting 3 (PR 379), within a
Nomad's reach. At its old timing (a 0.6 s cooldown, 0.2 s draw) and with every miss lost
but nothing else asked of it, it beat a spear or club on every Nomad creature: quicker,
safer, and the only way to take a deer. The bow should be safer but slower and costlier
than melee, and better against skittish prey.

`engine/tests/nomad_hunting.rs` measures it on the content's own data, 400 seeded runs a
fight at level 3, never on the clock. The model is one line of ground. The player starts
24 blocks off and closes at a sprint, or at `drawing_speed` of one while drawn. Each
creature acts by its behaviour data: one that `fights()` (Defend or Hunt) charges and
strikes on its attack's timing, and the rest bolt while fear lasts (`FEAR_FADE`) and keep
the player out of their Flee radius. Any blow that reaches it, hit or miss, rouses it as
`land_blow` does (`Mind::hurt`), so a miss gives no free second shot at grazing game. A
hit taken spoils a draw where the class says so. Arrows are kept by `recover` on a hit
(the killing one too) and `recover_miss` on a miss. Times are seconds to the kill, hurt is
the share of health lost, and spent is arrows used up per kill. "Before" is the bow's old
data in the same model:

| Creature | Spear | Club | Bow before | Bow after |
| --- | --- | --- | --- | --- |
| Rabbit | never caught | never caught | 16.4 s, 1.7 spent | 17.1 s, 0.9 spent |
| Junglefowl | 11.0 s | 12.3 s | 2.2 s, 1.7 spent | 12.9 s, 2.3 spent |
| Red deer | never caught | never caught | 57.1 s, 4.0 spent | 58.8 s, 2.2 spent |
| Wild boar | 17.9 s | 21.0 s | 5.9 s, 6.4 spent | 61.1 s, 7.5 spent |
| Aurochs | 12.8 s, 27% hurt | 16.7 s, 39% hurt | 7.6 s, 10% hurt, 8.5 spent | 12.5 s, 23% hurt, 4.2 spent |

Over the game melee can take (junglefowl, boar, aurochs) the bow took 15.6 s against
melee's 41.7 s before, and 86.5 s after. The bow is the tool for skittish game (rabbit,
deer), a little safer against an aurochs, and slower than melee on whatever melee can
catch. The sling takes no skittish game at all (its 12 blocks are inside their ground):
a boar in 90 s and an aurochs in 17.5 s, at 40% of health lost, against 9.8 s and 12.6 s
before.

What changed, in `combat.ron`:

- **Bow**: cooldown 0.6 to 1.0 s and draw 0.2 to 0.5 s; misses kept 0.8; spoiled by
  hits; 0.4 of the pace while drawn.
- **Sling**: cooldown 0.53 to 0.8 s and draw 0.15 to 0.3 s; misses kept 0.5; spoiled by
  hits; 0.7 of the pace while drawn.

Prey reacts as it already did, through its behaviour data: a deer or rabbit that is shot
at is frightened and bolts out of reach (Flee, fear fading at 0.12 a second), and an
aurochs charges (Defend).

**The boar since it fights back** (Oct 20). With PR 302 a wild boar fights back to 40% of
its health. Struck from afar, it charges the shooter, who gets free shots as it comes: the
bow took it in 9.4 s and the sling in 13.3 s against a spear's 15.7 s, which broke the
first pin. Being a fighter, a boar now lets one come near, watching, and edges off only
from one close on it (`Flee` radius 8 to 4). Its hide and the gristle over its shoulders
turn many blows and shots (defence 4 to 9), but a spear's point goes in under them
(`weak: Some(Stab)`, the boar spear). The model now decides fight or flight as the server
does (`Creature::holds`, so `flees_below`). After: spear 12.0 s at 12% hurt, club 18.1 s
at 20%, bow 13.5 s at 11% and 3.6 arrows spent, sling 17.9 s at 20%. Raising the boar's
defence alone could not hold both pins (past about 11 the bow, fighting longer at close
quarters, costs more health than the spear), and raising the bow's cooldown slowed it on
every creature and left it costing more health than melee against an aurochs. The aurochs
pin is the closest: bow 12.4 s against the spear's 12.9 s, 1.1% over the line.

The test pins, so a later change cannot make a shot strictly better than melee:

- on every creature melee kills at least half the time, the bow and the sling each take
  at least 0.95 times as long as the best melee weapon;
- there, the bow costs no more than 2 points of health more than the best melee weapon;
- every kill with the bow uses up at least half an arrow;
- the bow takes a red deer at least 30 points more often than melee.

With the bow's and sling's old data the first pin fails on the junglefowl, boar and
aurochs.

## First-age fight balance

Oct 14, 2026. The first fights audit above played a bot at Attack 5 with 60 health. A brand-new player has 20 health, Attack 1 and Defence 1, and fared much worse. The bot `first_age_fights_balanced` in `server/tests/gameplay.rs` plays that player. It runs on an open floor walled round, chases on foot at a sprint, and swings a stone sword on its cooldown whenever the creature is in reach. It fights each creature twice: once never guarding, and once raising the guard just before every other wind-up lands. Run it by hand with `--ignored --nocapture`; `FIGHT_ONLY`, `FIGHT_RUNS` and `FIGHT_GUARD` narrow it. The cloud runs, averaged over 4 runs before and 6 after. The wolf's open fight after is 8 runs at its final health of 13; its guarding fight was measured at 14, one point harder:

| Creature | Before, open | Before, guarding | After, open | After, guarding |
| --- | --- | --- | --- | --- |
| Wild boar | killed in 6.6 s, chase 1.9 s (worst 7.3 s) | 8.6 s, chase 1.9 s | 7.7 s, chase 1.4 s | 6.1 s, chase 0.1 s |
| Badger | 3 of 4 died, 12% health left | 88% left | 62% left (worst 30%) | 92% left |
| Fox | 88% left | 92% left | 78% left | 97% left |
| Aurochs | 70% left | 100% left | 61% left (worst 45%) | 90% left |
| Wolf | 4 of 4 died | 86% left | 43% left (worst 15%), none died | 93% left |
| Brown bear | 4 of 4 died | 4 of 4 died | 6 of 6 died | 4 of 6 died |

What changed, all in `creatures.ron`:

- **Wolf**: health 16 to 13, defence 8 to 4, damage 2-4 to 1-3. Its level (12), speed, wind-up and pack size are unchanged. A pack at night is still a reason to be home by the fire.
- **Badger**: defence 7 to 3, damage 1-3 to 1-2, and a blow every 1.4 s instead of 1.1 s. Its wind-up is unchanged.
- **Wild boar**: it runs at 5.4 instead of 6.0 blocks a second, under a sprint's 6.1, so a chase closes.
- **Brown bear**: it now lives in the cold pine woods and the mountains, never as warm as where a new player starts. A bear's comfort ends at 8 degrees, and the world's spawn is open temperate grassland at 8 to 20 degrees. Its fight is unchanged, so a bear met later is still a danger to avoid. A player who guards well beat one in 2 of 6 runs.
- The fox and the aurochs are unchanged. Their numbers moved only with the rolls: the server rolls by its tick, and the runs before mostly repeated the same rolls, while the runs after each start a little later.

**The boar fights back** (Oct 15, combat feel pass gap 4). Until then the wild boar had no attack, so a first fight with one was a chase after something that never answered. It still never starts a fight and is still slower than a sprint. Struck while it is hale, it now turns and gores back with a short jab of the tusks: wind-up 0.55 s, told by Old Tusk's gore sound (fainter, never dipping the music), 1 to 2 damage every 1.6 s, stab. Below 40% of its health it drops the fight and runs (`flees_below: 0.4` in `creatures.ron`; a kind without it keeps the bravery rule, 0.5 less 0.35 x bravery). Running, it lets go of its anger, so it keeps running; a kind with no `Flee` behaviour (badger, bear, aurochs) fights on, cornered. Its run is 5.1, so even the liveliest (pace 1.12) runs 5.71, under 0.95 of a 6.1 sprint. The young of a kind that fights back never do unless the kind says `young_fight`: a piglet struck, or a sounder stirred, runs (`Creature::fights_as`, shared by `Mind::hurt_young` and the wandering stir). A wandering party's pet boar scatters with its party. A kept boar struck by its keeper (at slaughter, say) gores back as a kept aurochs does; that is kept. Measured by the same bot over 6 runs each: open, 9.3 s to kill, 3.3 blows back for 2.5 damage, 89% of health left (worst 75%); guarding, 6.1 s, 97% left (worst 90%). Every blow back was told. The bot sprints at 0.95 of a sprint, as fast as the liveliest boar runs, so it never fell out of reach (0.1 s chasing); the run shows as a longer kill instead (9.3 s, against 6.9 s when it fought to the end). That makes it easier than a fox (78% left fighting open) and a badger: a first answer, not a danger. Dropping the fight on running also changes every other kind that fights and has a `Flee`: the fox, the arctic fox, the ember salamander, the frost stag and the cave bat now, badly hurt, drop the fight and run where before the anger of earlier blows could hold them until they died (first_fights.rs checks every such kind). The gate's FirstFights step winds up the first creature in the content that never hunts, fights back and runs once badly hurt (`flees_below`, so the boar) and checks that its own tell, the gore, is asked for without dipping the music (shot `defender-blow`).

Every wind-up is still 0.3 s or more. `engine/tests/first_fights.rs` holds these targets on the content's own rolls:

- a new player beats a wolf without guarding with 15% to 45% of their health left;
- a badger and a fox are easy;
- an aurochs is fair;
- a boar runs at least half a block a second slower than a sprint;
- a wild boar answers a blow but is easy, and gores back when struck while hale but runs when badly hurt;
- no brown bear lives at any temperature a new player starts in, and none can be beaten by a new player.

A new player's kills now take 6 to 12 s; the design lane's level audit assumed 5.4 to 9 s. A wolf has 3 fewer health points, so its kill gives 12 fewer Attack and 4 fewer Hitpoints experience. No experience rate changed.
