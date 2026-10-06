# The journal's data

Oct 7, 2026. What the server records for the journal screen (Mariano's draft 21): every kind
of thing a player can discover, each entry seen, studied or mastered. The interface draws
the screen from it; anything not found shows as an outline.

## Shape

`voxmar_engine::journal`:

- `Page`: Animals, Monsters, Bosses, Breeds, Plants, Fish, Biomes, Places, Sky, Ores,
  Collection, Music, Books, Discoveries (`Page::ALL` in that order).
- `Entry { page, name, stage, count, found }`: `name` is the content name (`base:deer`,
  `base:poppy`, `base:forest`); `stage` is `Seen`, `Studied` or `Mastered`; `count` is
  kills, catches, picks or blocks mined since; `found` is the world tick it was first found.
- `Journal`: a player's entries, with `get(page, name)`, `found(page)` and `entries()`.

The server sends `ServerMessage::Journal { entries }`: every entry on joining, then each
one that changes. The client keeps them in `LocalPlayer::journal`. Entries are saved in the
player file.

## What finds an entry

| Page | Seen | Counted |
| --- | --- | --- |
| Animals, Monsters (and spirits) | within 16 blocks | each kill |
| Fish | a catch | each catch |
| Plants | a flower, herb or fungus picked, a crop harvested | each pick or harvest |
| Ores | an ore, gem or natural rock mined | each block |
| Biomes | standing in it | |
| Collection | a creature's drop on a kill | each item dropped |

Bosses, breeds, places, sky, music, books and discoveries have pages and rules but no
events yet; `Server::discover(player, page, name, times)` and `Journal::study` are the
hooks for the systems that will fill them.

## Stages

`core/journal.ron` sets the counts at which an entry is studied and mastered per page:
ten and a hundred kills for monsters (the weak style, then the drop table), catches for
fish, picks for plants, 25 and 250 blocks for ores. Pages without counts stay seen until
something studies them (a codex instrument, a book).
