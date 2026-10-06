# Status effects

Buffs and debuffs change how a player heals, hungers, feels the cold, moves, mines and
learns. They are content: every package's `effects.ron` lists its own (core's: hunger,
warmth and bad weather; base's: what food does), read by `crates/engine/src/effect.rs`.

## What an effect is

- a **kind**, buff or debuff, and an **icon**: `ui/effect/<icon>` in the packs (ten texels
  across, `assets/packs/default/recipes/effects.ron`), the effect's own name unless given;
- its name and a line on what it does, as language keys `effect.<package>.<name>` and
  `effect.<package>.<name>.description` (English and Japanese, with a hiragana reading
  for a Japanese name with kanji);
- **modifiers**, a fixed vocabulary the engine owns: `heal`, `hunger`, `speed`, `mining`
  and `xp` multiply (1 changes nothing; `heal: 0` stops healing), `warmth` adds degrees to
  how warm you feel, `max_health` adds health points, `hurt_every` hurts a point every so
  many seconds. While several effects are on, rates multiply, warmth and health add, and
  the quickest hurt wins;
- what brings it on: a **condition** the server checks every second (hunger, how warm you
  feel, a fire near you in the cold, and the weather where you stand), or something you do.
  Rain, snow, hail, dust and lightning count only out in them, with nothing solid over
  your head; wind reaches you anywhere. `replaces` hides milder effects while a stronger
  one is on (freezing hides chilly, drenched hides soaked).

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
