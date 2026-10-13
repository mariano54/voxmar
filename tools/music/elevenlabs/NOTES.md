# ElevenLabs music for Voxmar

Notes on how the tracks in `assets/music/elevenlabs/` were made, from the session that wrote
`forest_gen.py` (12-13 October 2026). No credentials are kept here; the generator reads the
key from the `ELEVENLABS_API_KEY` environment variable.

## Model and call

- ElevenLabs Music, model `music_v2_5`, `POST /v1/music?output_format=mp3_44100_128`, with
  `force_instrumental: true`. Each call returns one mp3 of roughly 2:30 to 3:00 (boss tracks
  about 2:05).
- Next to every `.mp3` is a `.json` with the exact `prompt`, plus the lead, backing
  instruments, vibes, scene, form, BPM, key, seed and model that built it. Re-generating a
  track from its json: `forest_gen.py --respec <json>`.

## How prompts are built

`forest_gen.py` samples a track spec and turns it into a prompt (`build_prompt` for calm,
`build_boss_prompt` for boss tracks):

- Instrument and vibe weights come from the calm part of an analysed game soundtrack (vocal,
  loud, beat-driven and aggressive tracks removed): piano, violin and cello lead; harp,
  guitar, zither, flute, mallets, bells, singing bowl add colour; pads sit underneath.
- Piano and violin may lead at most half of a batch each (Rhodes counts as piano, pizzicato as
  violin); other leads are flute, harp, cello, clarinet, French horn, duduk or the biome's
  signature instrument.
- About 40% of tracks get one "Teldrassil" touch: an extra flute, clarinet, French horn or
  harp, or one gently mystical, moonlit or reverent vibe.
- A musical form per track (returning melody, call and response, waltz, music-box figure, solo
  build, theme and variation, sparse phrases with rests), a clear singable melody, 50-62 BPM.
- Fixed "never" lists keep every prompt calm: no vocals or choir, no nature or ambient
  sounds (the game layers its own), no loud crescendos. At most one calm track per biome gets
  soft percussion.
- Biomes: `forest`, `snow`, `fantasy`, `desert`, `general`. Each has its own scenes plus a few
  extra weighted instruments and vibes. `general` has no place and no time-of-day words, so it
  fits anywhere at any time, and has percussion turned off.
- `--mode boss`: intense but tasteful orchestral or folk-fusion, 128-150 BPM, each biome keeping
  its signature instruments, with a `BOSS_NEVER` list (no vocals, choir, nature sounds, EDM or
  metal harshness).

## Commands

```
ELEVENLABS_API_KEY=... python3 tools/music/elevenlabs/forest_gen.py --biome forest --count 3 --seed 7
python3 tools/music/elevenlabs/forest_gen.py --biome snow --count 3 --dry-run    # print prompts only
python3 tools/music/elevenlabs/forest_gen.py --biome general --count 5
python3 tools/music/elevenlabs/forest_gen.py --biome desert --mode boss --count 1
```

`--out DIR` sets where it writes (`<DIR>/<biome>/`); `--model` defaults to `music_v2_5`.

## Rounds

- **round4**: calm tracks for forest, snow, desert and fantasy (3 per biome were generated; 11
  are kept here). This round added the 50% piano and violin cap, the Teldrassil touch, the
  varied forms and openings, and the "no ambient sound" rule. Earlier rounds (not kept) had
  too much pad wash, a track that was too fast, and nature sounds baked in.
- **round5**: four calm `general` tracks (a fifth, "hero's home base", was generated but
  came out as strummed guitar with jingle bells and very quiet, and is not kept) and one boss
  track per biome (forest, snow, desert, fantasy).

## Decisions about where they play (Mariano, 13 October 2026)

- Everything goes in an ElevenLabs music category.
- Biome tracks (round4, and the round5 biome folders other than boss) play only in their
  biome. Boss tracks play only in boss fights or boss rooms. The `general` tracks can play at
  any moment.
- The game code for this (category, biome and boss selection) is not written here.
