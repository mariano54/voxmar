# Trailer

A 30-second trailer made from the game's own headless captures: every shot is a
`--capture … --clip` run on seed 2026, so re-recording after the game changes is one command.

```bash
cargo build --profile iter -p voxmar_client
python3 tools/trailer/scout.py target/iter/voxmar OUT 2026     # where the biomes and villages are
python3 tools/trailer/shoot.py target/iter/voxmar OUT          # every shot in shots.py (or name some)
python3 tools/trailer/sheet.py OUT [SHOT ...]                  # contact sheet of their middle frames
uv run tools/trailer/cards.py CARDS                            # title cards, in the game's pixel font
python3 tools/trailer/edit.py OUT CARDS trailer.mp4            # cut, cards and music
```

- `shots.py`: each shot's place, camera (`--pose`, and `--dolly` to move it through the clip),
  time of day, weather and console commands. Creatures are summoned with `--clip-command` so
  they are where they were put when recording starts; the factory is built with `/fill` and
  `/setblock`.
- `cards.py`: the headline numbers. Keep them true to the content: 14 skills
  (`core/skills.ron`), 50 creature species (`base/creatures.ron`), 56 surface and 12 cave
  biomes.
- `edit.py`: the timeline (which shots, where in each, how long) and the music, a stretch of
  `assets/music/old_forest.mp3` that ends with the track, normalised to -14 LUFS.

The capture flags it relies on: `--cinematic` (no HUD, console or block outline),
`--no-console` (no command replies or tips over menus and fights) and `--dolly`.
