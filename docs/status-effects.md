# Status effects

Buffs and debuffs change how a player heals, feels the cold, moves, mines and learns.
They are content: every package's `effects.ron` lists its own (core's: warmth and bad
weather; base's: what food does), read by `crates/engine/src/effect.rs`.

## What an effect is

- a **kind**, buff or debuff, and an **icon**: `ui/effect/<icon>` in the packs (ten texels
  across, `assets/packs/default/recipes/effects.ron` and the interface's `ui.ron`), the
  effect's own name unless given. Every effect has one of its own: the launch refuses an
  effect no bundled pack (the default pack, or a mod's pack without a `pack.ron`) draws,
  naming it (`Source::unpictured`; test `every_effect_has_an_icon_of_its_own`, which also
  finds no two alike; gate "effects: icons");
- its name and a line on what it does, as language keys `effect.<package>.<name>` and
  `effect.<package>.<name>.description` (English and Japanese, with a hiragana reading
  for a Japanese name with kanji);
- **modifiers**, a fixed vocabulary the engine owns: `heal`, `speed`, `mining`, `xp`
  and `fishing` (how often a cast lands) multiply (1 changes nothing; `heal: 0` stops healing), `finds` adds to
  the chance of an extra piece from natural ground and rock (0.05 is one time in twenty, on
  top of the tool's `yield` and the skill level; `tools.ron`'s rules say which blocks), `warmth` adds degrees to
  how warm you feel, `max_health` adds health points, `hurt_every` hurts a point every so
  many seconds. While several effects are on, rates multiply, warmth and health add, and
  the quickest hurt wins;
- what brings it on: a **condition** the server checks every second (how warm you
  feel, a fire near you in the cold, the weather where you stand, and the biome: `InBiome`
  takes a biome tag or name), or something you do.
  Rain, snow, hail, dust and lightning count only out in them, with nothing solid over
  your head; wind reaches you anywhere. `replaces` hides milder effects while a stronger
  one is on (freezing hides chilly, drenched hides soaked).

**After dark.** `Night` holds from sunset to sunrise (the sun below the horizon,
`calendar::night`); base's `evening_in` (Evening in: heal x1.2, 2° warmer) asks for it
indoors at home by a fire, and a sleep rule marked `night: true` gives its rest only after
dark (docs/home.md "Nights").

Biomes give their own (base effects.ron): waterside (sea, river and marsh: casts land a
fifth more often), forest calm (heal a fifth faster), rich rock (mountains: mine a tenth
faster), aether-touched (fantastical biomes: a twentieth more experience) and mired
(marsh: a little slower), so where you go matters as well as what you eat.

**What is in hand and what is near.** `Holding("tag")` holds while the item in hand carries
the tag, and `Wearing("tag")` while something worn carries it (the charm slot: base's
`tusk_stride`, from Old Tusk's Charge Charm, never from it held in hand); `Near("tag", reach)` while a block carrying it lies within `reach` blocks (1 to 8,
every way) of the player's feet. A tag no block carries is refused at launch. `Seated` holds
while the player sits on a seat (`base:sit`, right click on a stool, chair, bench or cushion
with no block in hand; they sit until they move off it); base's `fireside` (Fireside: heal
x1.5, 4° warmer) asks for it with a fire or the hearth within 3, and `at_table` (At the Table:
a meal heals x1.25 and its good lasts x1.25, as `meal_heal`/`meal_time`) with a table within 2. **Effects that
cling:** `lingers: SECONDS` makes an effect stay that long after its `when` stops holding (it
is kept topped up while it holds, as a timed effect). Base's `smoky`: a torch in hand or a
fire within 2 blocks leaves the player Smoky for 30 s, and bees treat a Smoky player as
smoked (docs/bees.md). Any content can ask about it later, as about any effect.

**Know-how: what a quest taught stays.** The condition `Done("quest")` holds from the moment
the player has finished that quest (its package's name unless written in full; a quest no
package has is refused at launch). It is how finishing an early gathering quest becomes an
upgrade the player feels at once, with no new item: base's ladder `steady_hands` (Stone in
Hand: every tool a tenth faster), `practised_hands` (A Full Stone Kit: a fifth, and
extras three times in a hundred), `copper_hands` (Copper Kit: nearly a third, extras twice as
often: 6 in a hundred); each replaces the one below, says so in chat once with a **Next:** line, and counts
for quests as `Built("knack_NAME", 1)` (`ladder: "knack"` on the effect; the ladder name is
`home` unless given). It combines with every other condition, so what was learnt can pair
with where it is used: `stonewise` (A Pile of Stone, in mountain biomes: extra finds) and
`anglers_knack` (A Fish Supper: casts land a tenth more often) are two. A mod adds a perk by
adding an effect that waits on a quest, and its chat line, icon and Japanese come with it.

Warmth from food and weather counts before the bands of warmth are judged: soaked through
on a mild day, you feel chilly.

## Food

`foods` rules give an effect for so many seconds to anything eaten with all of its tags
(cooked meat makes you hearty, raw meat queasy); a food may also carry its own
`effect:NAME:SECONDS` tag. Eating again tops the time up. Timed effects are saved with the
player.

## How the client learns them

`ServerMessage::PlayerState { vitals }`: `Vitals.effects` lists `ActiveEffect { effect,
seconds_left }`, buffs first; `seconds_left` is none for an effect that lasts while its
condition holds. The client resolves each id through `content.effects` (both sides load
the same content), applies `speed` to its own movement and `mining` to its mining time, and
the interface draws the icons, names and descriptions.

## Health

Most health is twice the Hitpoints level (which starts at 10, so 20), plus what effects
add. When it rises, the new health comes with it; when it falls, what is over goes.
