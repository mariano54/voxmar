# voxmar

A moddable, open source voxel sandbox with an infinite procedurally generated world.
Rust, Bevy 0.19, wgpu (Metal on macOS, Vulkan/DX12 elsewhere).

## Running

```bash
cargo run --release                         # title screen: singleplayer, multiplayer, settings
cargo run --release -- --world NAME         # play (or create) the saved world NAME right away
cargo run --release -- --connect HOST:PORT  # join a server right away
cargo run --release -- --host               # also open singleplayer worlds on the LAN
cargo run --release -p voxmar_server        # dedicated server on port 25665, world in ./world
```

macOS and Windows need only Rust. Linux also needs the development packages for ALSA
(sound), udev (gamepads), Wayland and xkbcommon, and a Vulkan driver; on Debian or Ubuntu:

```bash
sudo apt install pkg-config libasound2-dev libudev-dev libwayland-dev libxkbcommon-dev mesa-vulkan-drivers
```

The dedicated server needs none of them.

While developing, use `--profile iter` in place of `--release` (for `run`, `build` and `test`):
it rebuilds in seconds rather than minutes after an edit and runs nearly as fast. Keep
`--release` for performance measurements.

For a bounded renderer diagnosis, set `VOXMAR_PROFILE_FRAMES=1` together with
`VOXMAR_RECORD_SLOW_FRAMES=1` for a capture. Slow-frame records then include wall and
thread CPU time for streaming, compaction and buffer operations, with queue counts and
bytes. Storage stays fixed at eleven measurements per render frame in the existing
three-frame ring; only slow frames write JSON. `render_local_frame` identifies a render
record; the shared process-local monotonic intervals join it to main-frame intervals.
Parent measurements include their nested operations, so their times must not be added
together. Thread CPU excludes sleep and descheduling, and neither clock measures GPU
execution. Unsupported or failed CPU clocks are `null`. `receive.input_bytes` counts
payloads of applied messages. Child `written_bytes` counters cover the targeted page,
arena and compaction queue writes; origins, wind, coverage and blade-index setup writes
are outside these counters. `buffer_bytes_created` counts allocated capacity. These are
different quantities. Owner `original_writes` and `original_bytes` describe assignments
on pages that survive to the end-of-receive flush; `unique_dirty_words` counts distinct
assigned positions, including same-value assignments. `compact.reused_after_unrelated_changes`
counts view reuse that the previous whole-terrain revision check would have invalidated.

Client flags: `--seed N` (for worlds `--world` creates, and captures), `--name NAME`,
`--view-distance N` (columns of 32 blocks, default 16), `--fov DEGREES`, `--no-vsync` (uncapped
frame rate), `--language CODE` (`en` or `ja`; captures are in English without it), `--weather KIND[:INTENSITY]` (show a kind of weather from `weather.ron`, such as
`cumulus`, `storm`, `rain:0.3` or `snow:heavy`, instead of the server's), `--command LINE` (run a console line once in game, such as
`--command "/time set noon"`; repeatable, and handy with captures), and
`--capture out.png [--pose dx,dy,dz,yaw,pitch | --flight SECONDS | --dig SECONDS] [--hold ITEM] [--repeat] [--face-creature] [--time FRACTION] [--mine PROGRESS] [--flicker | --flicker-still] [--menu | --inventory | --character | --loading | --screen title|worlds|create|join|settings]`,
which runs headless, waits for the world (an unsaved one unless `--world` or `--connect` is
given) or the menu backdrop to load, logs frame time and saves a screenshot; `--time` holds the
time of day (a fraction of the day, 0 at sunrise), `--hold` selects the hotbar slot holding an item once it is there (after a `/give`), `--repeat` clicks `--press`'s button every 0.4 s through the clip instead of holding it, `--face-creature` keeps the eye on the nearest creature, `--dig` digs straight down for that many seconds
holding the button (reporting frame times, as `--flight` does), and `--mine` mines the block in the middle of
the view that far (0..1, cracks and chips showing; 1 breaks it just before the screenshot).
`--clip SECONDS` saves every frame of that much game time at 30 fps instead; `--clip-join` starts
it the moment the player joins (to see what joining shows) rather than once the world settled, and
`--clip-command LINE` runs a console line as it starts (`--clip-command "/weather clear"` to watch
the weather change). Still shots also wait for the weather and the place's air to show.
`--flicker` measures shimmer: with the clock stopped it also takes `OUT-moved.png` after
stepping the camera 4 cm sideways, logs the share of pixels that changed by more than 60
levels, and marks them in red in `OUT-flicker.png`.
`--flicker-still` instead keeps the camera still and lets the game run on one 60 fps frame
between the shots (the sun and wind moving), as shadows and leaves flicker over time.
`--gate DIR` plays a scripted session headless and checks it, saving its shots and report in
DIR, and `--server-lag MS` makes every tick of the integrated server take that much longer,
as on a busy machine (see docs/quality.md).

Land on main with `tools/land`: it merges origin's main and the local one in, runs
`tools/gate` (a quick gate: the build, the tests the change can reach and that session in a
small world), fast-forwards main and pushes it, and checks that origin has it. Play builds
pass `tools/gate --full`. For changes
that can cost frame time, `tools/bench` compares frame times with main's on a quiet
machine. All are described in docs/quality.md.

The title screen, over a slowly turning view of a demo world, leads to Singleplayer (saved
worlds, most recently played first, to play, create or delete), Multiplayer (join by address;
the last few servers are remembered) and Settings. A new world takes a name, an optional seed
(a number, or any text, which is hashed; empty means random) and a game mode: Creative allows
flight, Survival does not. Either way you are the world's admin, who can run commands.

Controls: WASD move, Space jump, Ctrl (or W tapped twice) sprint, Shift descend, F toggle flight, left mouse break
(hold), right mouse place (on an animal: feed or tame it; Shift and right click one of yours twice to let it go), middle mouse pick the block looked at, 1-9 or wheel select hotbar slot, E
inventory, C character screen, T chat, / command, F3 debug overlay (frame rate, and the worst frame, p99 and frames over 25 ms of the last 5 s), Esc menu, R respawn, Z (hold) zoom in
creative: the wheel changes the magnification while zoomed, and mouse look slows to match.
Movement feels like Minecraft's: a sprint lasts while forward is held, until you run head-on
into a wall, and widens the view a little; sprint-jumping is fastest. Swimming up against a
bank and holding jump hops you out onto it, up to a block above the water. The speeds, jump
and gravity are content (`content/core/movement.ron`), which client and server both read.

The inventory works as Minecraft's: click picks up, puts down, merges or swaps a stack;
right click (or Control-click) takes half or puts down one; dragging a held stack spreads it
evenly over the slots crossed (right button: one each); a double click gathers the item onto
the pointer; Shift-click moves a stack between the hotbar and the pack (Shift double click:
every stack of it); 1-9 over a slot swaps it with that hotbar slot; Q over a slot throws one
item (Ctrl+Q the stack), and clicking outside the panel throws the held stack (right click:
one item). Hovering an item names it, coloured by rarity, with what it is good for. Esc or E
closes it and puts back what you hold, and what is left in the crafting grid.

Above the pack is a two-by-two crafting grid: put items in it as in any slot, and click
what it makes to craft one onto the pointer (Shift-click: as many as fit into the
inventory). Recipes are content (`recipes.ron`): a log makes four of its planks, two planks
four sticks, four planks a crafting table, four stone stone bricks, and two wheat a loaf of
bread. Right click a crafting table to work at its three-by-three grid, for tools (wooden,
stone, copper and iron pickaxes, axes, shovels, hoes and knives) and anything else bigger
than two by two. Each recipe trains a skill (Crafting, Engineering for tools and stations,
Cooking for food) and may need a level in it. Tools mine what they are made for faster and
find more there; ores ask for a Mining level and a pickaxe tier (`tools.ron`). A hoe tills
soil into farmland, where crops are sown. See docs/tools-and-crafting.md. Cooked food, drinks and potions take a slot each, as in
RuneScape, so what you carry is a choice (`stacking.ron`). A seed pouch or herb pouch (a hide and
twine, Crafting 3) carries up to a thousand of each kind of seed or herb in one slot: what
you pick up goes into it while you carry it, hovering it lists what it holds, a seed pouch
in hand sows its seeds (the first your Farming level allows), and right click empties it.

Fishing: twine (three wheat) and sticks make a fishing rod at a crafting table. Hold right
click with it over water to cast, again each time the line comes up: after a few seconds a
fish bites or nothing does. What bites depends on your Fishing level, salt or fresh water,
the biome, day or night and rain (`fishing.ron`): minnows at 1, trout in cold
rivers at 15, eels at night at 45, tuna in warm seas at 50, and glimmer koi in fantastical
waters on rainy nights at 75. Levels and a fine rod land more often; fish cook on a
campfire.

The recipe viewer works as NEI does: over any item in the inventory (or in the viewer),
R shows the recipes that make it and U everything it goes into, drawn as they happen (the
crafting grid, a campfire, shears on a mouflon, a creature that drops it); arrows turn the
pages, clicking an item looks up its recipes (right-click, its uses) and Back retraces the
steps. Both keys can be changed in Settings, Keys. It reads the same content the game plays
by, so a package's recipes show up in it on their own.

In survival, the inventory has a recipe search beside it: type part of an item's English
or Japanese name, then click it for recipes or right-click it for uses. The quest book
shows recipes under crafting, cooking and carried-item goals; click any goal, ingredient
or reward item to look it up, and Back returns to the book. The Nomad quest "Look It Up"
teaches this with the stone pickaxe and awards 40 Crafting xp once. For captures,
`--catalogue SEARCH` opens the search in either mode, and `--quests QUEST` opens that
quest's page.

The character screen (C) shows health, how warm you feel and the effects on you, and
every skill with its icon, level and progress; click a skill for its guide, what each level
opens up. There is no hunger: health comes back slowly by itself, and food heals (right-click
food in hand to eat it when hurt), a tenth less each time you eat the same thing in a row,
down to half, so a varied pack is worth carrying. How warm you feel follows the air, water
and any fire or torch nearby, and gives effects (Chilly, Freezing, Hot, Overheated, By the
fire) that change how fast you heal and move, or hurt you. The HUD shows health as a bar
and the effects as chips above it. Every in-game screen lists the keys to
the others under it. A creature under the crosshair is named at the top of the screen with
its level, coloured RuneScape-style against your combat level (green below, gold even, red
above), and once you strike at it, a thin bar of its health; the one you are fighting stays
there a few seconds after you look away.

Combat is Minecraft's without the knockback: a blow lands at full strength 0.6 s after the
last, and quicker ones (down to 0.3 s) land lighter, so well-timed blows deal about a tenth
more damage a second than clicking as fast as you can (`engine/src/combat.rs`).

Farming, with RuneScape-style skills: right-click soil with seeds to sow them (carrots and
potatoes are sown from themselves). Each crop needs a farming level, from wheat at 1 to
saffron at 60 and the night-growing moonbloom at 75, and planting and harvesting earn
experience, shown rising at the right of the screen, with a small plate above it when a
level is reached; the character screen lists every skill's level (levels 1 to 99 on RuneScape's curve). Crops grow through three stages to
ripe over real minutes, fastest inside their climate, in light, on fertile ground with water
within four blocks; break a ripe one to harvest it, or right-click a ripe tomato, strawberry
or chili to pick it and let it fruit again. Higher levels yield a little more, never grow
faster. Most crops are rare; their seeds come first from wild plants (wild wheat and carrot,
chili, moonbloom) and from wild crops. Crops and skills are content (`content/base/crops.ron`,
`content/core/skills.ron`), and the crops players plant are kept with the world (`crops.ron`).
`/grow [STAGES|ripe]` grows the crops near you. Fruit grows on trees: apples (and, rarely,
golden apples) on oaks, cherries on cherry trees, oranges on jungle trees, coconuts on palms
and emberfruit on emberwoods, as leaves that blossom, set fruit and ripen; right-click ripe
fruit leaves to pick them, at any level, and they bear again.

Broken blocks drop items, as in Minecraft: what a block gives pops out as a tiny copy of it
(a plant as a flat sprite with thickness) that turns and bobs where it lands, floats in
water, merges with nearby stacks of the same thing, and lasts half an hour of world time. After half a
second, walking near draws it in with a pop. Q throws one item from the hand, Ctrl+Q the
whole stack; thrown items wait two seconds before they can be picked up again. Dropped items
are kept with the world (`items.ron`).

The interface (menus, HUD, inventory) has one look: a pixel font drawn in
`assets/ui/font.txt` and pixel-art pieces that are `ui/*` textures of the default pack
(`recipes/ui.ron`), so a texture pack can reskin it. Menus blur the world behind them.

### Languages

The game is in English and Japanese (Settings › Interface › Language; the first run follows
the system's language). Every word comes from a content package's `lang/<code>.ron`, a map
from keys to text (`"menu.settings": "設定"`): the interface's words are `core`'s
(`assets/content/core/lang/en.ron` explains the format), and a mod ships its own files for
its blocks and skills, or adds a whole language. Definitions are named by key from their ids
(`block.base.oak_log`, `skill.core.farming`), and a template's members by pattern, so
`block.base.{name}_log` with `family.base.oak` names オークの原木 and a new wood needs one
line per language; anything a language leaves out shows in English, then as a name made from
the id. A shader pack is named by `shader_pack.<key>`, its key being its name in lower case
with anything but a letter or a digit made `_` (Verdant is `shader_pack.verdant`, 新緑
`shader_pack.新緑`, "R&D v1.2" `shader_pack.r_d_v1_2`); a pack no language names shows its own
name. The packs the game ships (`assets/shaderpacks/shipped.ron`) load first; a pack with a
name already loaded is refused, and one whose key is already taken ("soft_light" after "Soft
Light") loads but keeps its own name. Text names its arguments (`"{crop}を植える"`) so each language orders them its own
way. Kanji and kana come from Fusion Pixel 12px, a Japanese pixel font (listed in
`assets/ui/fonts/fallback.ron`) whose glyphs are read at startup and merged under the chosen
interface font, its pixels drawn at three quarters of ours; BIZ UDGothic, a smooth font,
covers any character it lacks. Every Japanese name with kanji has a
reading in `lang/readings/ja.ron` (オークのげんぼく for オークの原木) that the interface can show with it. Tests in `crates/client/src/lang.rs` hold every
language complete, every key the code names defined, and every character drawn.

### Console and commands

T opens the console to chat with everyone on the server; / opens it with a command. Up and down
recall earlier lines, Tab completes a command's name, Enter sends and Esc closes. Replies and
chat show above the hotbar and fade after ten seconds. The server checks and runs commands,
like actions: admins (the singleplayer host, players named with `--admin` on a dedicated
server who play on its machine, players who join with the server's admin token, and anyone an
admin makes one with `/op NAME` for the session) may use them all, everyone else `/help`,
`/seed`, `/biome` and `/list`. Chat (and a non-admin's commands) is limited to a burst of eight
lines and two a second after it, by the clock; lines past that are dropped, and the first of
them says so.

- `/time` tells the time; `/time set sunrise|day|morning|noon|afternoon|sunset|dusk|night|midnight|dawn|TICKS`
  moves forward to that time of day (a day is 24000 ticks, 0 is sunrise), `/time add 100|30s|2h|1d`
  moves it on, `/time speed 10` runs days ten times as fast, `/time freeze` stops the sun and
  `/time resume` lets it run normally. `/day` and `/night` are shortcuts. The time of day and its
  speed are saved with the world; the weather keeps its own pace.
- `/weather` describes the weather where you are. `/weather KIND [INTENSITY] [SECONDS]` brings
  that kind (`/weather list`) to the land within about a kilometre of you, as intense as you say
  (`light`, `moderate`, `heavy`, `max` or 0 to 1; each kind has its usual intensity, and 1 is its
  fiercest), for five minutes unless you say; then the natural weather takes over again, slowly.
  `/weather natural` ends it at once. What you see and hear changes over within several seconds
  after a command, and over tens of seconds otherwise; it never jumps.
- `/wind` describes the wind; `/wind calm|breeze|strong|gale [FROM] [SECONDS]` brings that
  wind (from a compass point, `n` to `nw`, or degrees from north) on top of any forced
  weather.
- `/tp X Y Z`, `/tp X Z` (onto the ground), `/tp PLAYER`, `/tp PLAYER X Y Z`, `/tp PLAYER PLAYER`;
  `~` means where the player is, so `/tp ~ ~10 ~` goes ten blocks up.
- `/gamemode survival|creative [PLAYER]` switches a player's mode, which is kept with them.
- `/setblock X Y Z BLOCK` puts a block somewhere (`~` again means where you are), and
  `/fill X1 Y1 Z1 X2 Y2 Z2 BLOCK` fills a box (at most 32768 blocks and 128 across; `air`
  empties it). Both load the columns they touch first. `/give BLOCK [COUNT]` hands you blocks
  (64 unless you say; what does not fit drops at your feet), so `/give water` lets you pour
  water. `/clear [ITEM]` empties your inventory, or takes every one of an item out of it.
  `/kill` kills you, to try what dying costs.
  `/drop ITEM [COUNT] [X Y Z]` drops a stack into the world, at your feet unless you
  say where. Block names may leave out the
  package (`water` for `base:water`).
- `/biome` names the biome you stand in; `/locate biome NAME [RADIUS]` finds the nearest one
  (within 4000 blocks unless you say); `/locate ore [NAME] [RADIUS]` the nearest sample of ore
  lying on the ground over a vein (of an ore or vein named so, such as `iron`; within 1000).
- `/seed`, `/list` (who is online), `/help [COMMAND]`.

`/locate settlement [hamlet|village] [RADIUS]` finds a settlement and gives its teleport
coordinates; the default radius is 4000 blocks.

Commands are data: each package's `commands.ron` names its commands, who may use them, their
usage and help, and what they run: one of the engine's built-in commands, or other command
lines with the arguments filled in (`$1`, `$*`), which is how a package adds its own (`/day`
is `time set day`).

Esc opens the settings menu, in tabs (Video, Graphics, Audio, Controls, Interface) with a
line explaining the setting under the pointer; a choice steps forward on click and back on
right click, and the wheel moves a slider. The settings: render distance, far detail (Balanced, Fast or Full: how soon
distant chunks merge two or four columns into one, which draws far terrain much faster),
resolution (75%, 50% or Full: how many pixels the world is drawn at before it is stretched over
the window; menus and text stay sharp), field of view, FOV effects (the view widening while
sprinting), shader pack, anti-aliasing, shadows (off, low, medium or high), light shafts
(volumetric light), water quality, particles (All, Fewer or Off: the pieces that fly from
broken blocks and the chips while mining, and the smoke, embers, sparks and flames fires
and torches give off), how far the hotbar sits above the bottom edge (Raise
HUD), VSync, debug overlay, music, sound and ambience volume, mouse sensitivity, the sprint key and double-tap sprint, the zoom key
(click it, then press a key), zoom magnification, cinematic zoom (the view follows the mouse
smoothly while zoomed), zoom in survival, toggle guard (a right-click raises the guard and the next
lowers it, instead of holding the button), damage numbers (the hitsplats a fight shows), and fly speed
for admins, with Resume, Save and quit
to title (Disconnect on a server) and Quit game. Settings are saved to `voxmar/settings.ron` in
the OS config directory (`~/Library/Application Support` on macOS, `$XDG_CONFIG_HOME` or
`~/.config` on Linux, `%APPDATA%` on Windows; `$VOXMAR_CONFIG` replaces `…/voxmar`);
`--view-distance` and `--no-vsync` override them. On a dedicated server, name admins with
`--admin NAME` (repeatable; names are not checked, so it counts only for players on the server's
own machine; a tunnel or proxy that forwards players from elsewhere makes them all look local, so
leave `--admin` out behind one). An admin token makes anyone who joins with it an admin from
anywhere: the server reads it from `$VOXMAR_ADMIN_TOKEN` or the first line of
`--admin-token-file FILE` (never the command line, which others on the machine can see), and a
player gives it by starting the game with `VOXMAR_ADMIN_TOKENS=host:port=TOKEN` (several
separated by spaces), so each token goes to its own server only. It travels unencrypted, so
use it on networks you trust. `op NAME` typed at the server (or by an admin in game) makes
anyone else an admin until they leave, and `deop NAME` undoes it (an admin by name or token is
one again when they come back). Cap render distance with `--view-distance N` (default 32) and players with
`--max-players N` (default 64). Each name plays once at a time, whatever its case (Ada and ada
are one player, with one save), except that a player whose game froze or lost its network
comes back from the same address once that connection has been silent 6 seconds; at most 16 connections may be waiting to say hello (4 from
one address), and a player from elsewhere whose connection sends nothing for 15 seconds is
let go (`--idle-seconds N`; the game's network thread says it is there every 5 seconds, even
while the game is minimized). `--limits FILE` changes how much one connection may ask of the
server, from a RON file naming any of `messages_per_tick` (64), `chat_burst` (8 lines) and
`chat_per_second` (2), `hello_seconds` (3), `max_pending` (16), `max_pending_per_address`
(4), `name_length` (32), `ipv6_prefix` (64 bits counted as one address), `rejoin_silence`
(6 seconds, over the 5-second keepalive), `players_seen` (160 blocks within which others are
shown) and `players_seen_margin` (8 blocks further one shown must go to be hidden), as in `(chat_burst: 4.0, max_pending_per_address: 2)`.

### Saved worlds

Singleplayer worlds live in `voxmar/worlds/<name>/` next to the settings:

- `world.ron`: name, seed, game mode, created and last played times, the generator id and
  version that made it, and the time of day.
- `columns/<x>.<z>.bin`: each column a player has edited, as zlib-compressed postcard with the
  names of the blocks it uses, so a content change that renumbers blocks does not break saves.
  Untouched columns are not stored; generation recreates them exactly.
- `players/<name>.ron`: each player's position, health and inventory, restored when they join,
  under the name in lower case (a file saved under the name as typed is still read). The
  file keeps the player's canonical spelling, so Bob reconnecting as BOB keeps animal,
  machine and death-drop ownership. Older saves recover spellings from their kept animals,
  machines and actual player filenames and fold matching aliases. An older death drop
  with only an owner hash and no surviving original spelling remains protected; reconnect
  with the original spelling to collect it.

The server saves edited columns, players and the time every 30 seconds, when a player leaves
and when it stops (leaving to the title screen or quitting). Every file is written to a
temporary file and renamed, so a crash never leaves half a file, and a file that cannot be read
is never written over. One that is not what it should be is moved aside to
`<file>.corrupt-<unix time>` and the error logged (a player whose file it was starts afresh and
is told so); one that could not be read at all (no permission, a failing disk) or moved aside
is left alone: its player is turned away, its column is not saved over, and a world whose
items, crops or creatures file it is does not open. When a player leaves, the items they left
are written before they are, so a crash in between can double things but never lose them. Saved columns are unloaded
like any other. The dedicated server keeps its world in `--world DIR` (default `./world`),
created with `--seed N` and, given `--no-flight`, in survival mode if it does not exist yet;
type `stop` to save and quit. Anything else typed there runs as a command, as an admin
standing at spawn (`time set noon`, `weather storm 600`, `gamemode creative Ada`), and
`say …` tells everyone.

### Textures

Textures come from a stack of packs, each a folder, zip or jar; every texture is taken from the
first pack that has it:

1. the pack chosen with `VOXMAR_TEXTURES` (a resource pack, a Minecraft jar, or
   `assets/packs/default` to see only ours), which overrides everything below;
2. with Settings → Video → Minecraft assets on and Textures at "Minecraft first", the newest
   Minecraft client jar found (official launcher or Prism Launcher);
3. our default pack, `assets/packs/default`, which draws every texture the game uses: by
   default everything, and with Minecraft first whatever Minecraft lacks (our stones, woods,
   plants and wonders);
4. any other pack in `assets/packs`, in name order: mods ship art this way, adding textures but
   never replacing ours.

A new player gets Voxmar's own textures and sounds: the game reads nothing from a Minecraft
install unless Settings → Video → Minecraft assets is on (a settings file from before that
setting reads it as on when a jar is found, so an existing player's game is unchanged). With it
on, the jar also fills names our pack lacks. Settings → Video → Textures picks "Voxmar",
"Voxmar HD", our own 32×32 look (`assets/packs/voxmar_hd`, style guide in
[docs/voxmar-hd.md](docs/voxmar-hd.md)), or, with Minecraft assets on, "Minecraft first"; it
takes effect the next time the game starts. A pack folder with a `pack.ron` is such a whole
look: it names the packs it builds on (`extends`), whose images and recipes it takes for everything it
does not draw itself, and the resolution its block and item textures are drawn at (recipes
written smaller are drawn finer, keeping their shapes), and it is used only when picked.
Anything still missing gets a generated placeholder. The startup log says how many textures
each pack provided. A pack keeps images at `textures/<path>.png` (`textures/block/stone.png`),
or at `assets/minecraft/textures/<path>.png` as Minecraft and its resource packs do. No
Minecraft assets are shipped.

A pack can also describe textures as text, in recipe books (`recipes/*.ron`), which the game
renders as it loads, so a pack or mod can ship its art with no image files and no build step.
A recipe is a texture program (palettes, noise, patterns such as cells, bricks, boards and
rings, edge shading, highlights and cut-outs) or a text sprite (a grid of characters, each a
colour or palette shade); recipes derive from each other, and templates make a whole family
(a wood's log, planks, leaves and sapling) from one entry. See the `voxmar_texgen` crate docs.
A PNG beats a recipe of the same name. The `texgen` tool helps author them:

```bash
cargo run --release -p voxmar_texgen -- check                  # render every recipe, report errors
cargo run --release -p voxmar_texgen -- sheet --only stone,ore --compare PACK_OR_JAR
cargo run --release -p voxmar_texgen -- export --out DIR       # write every recipe as a PNG
```

A pack's books can replace the books of the pack it extends by name: a palette (recolouring
everything drawn with it), a texture, or single members of a template (redrawing them for
every family). `--pack assets/packs/voxmar_hd` renders Voxmar HD as the game does.
`sheet` writes `target/texgen/sheet.png`: each texture large, tiled 3×3 and at one pixel per
texel, optionally beside the same name from another pack (`--pack DIR` picks the pack,
`--columns`, `--zoom` and `--out` the layout).

Code and assets adapted from other open source projects are listed, with their licences, in
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md). Every mod, shader pack and project whose
ideas we used is credited in [INSPIRATIONS.md](INSPIRATIONS.md).

## Layout

| crate | role |
| --- | --- |
| `engine` | world data and rules, no renderer: palette chunks, column lighting, binary greedy meshing, visibility graph, collision, raycast, content packages, actions, scheduler, traits, fields |
| `worldgen` | staged generator: world dials → geology, rivers → climate fields → ecology → resources → terrain, caves, trees, plants |
| `protocol` | client/server messages and transports (in-process channels, TCP) |
| `server` | authoritative simulation (fixed 20 Hz tick), validation, streaming, world saves; `voxmar-server` binary |
| `client` | Bevy app: rendering, input, prediction, HUD, menus; the only crate that depends on Bevy |
| `texgen` | texture recipes and text sprites rendered to pixel art, and the `texgen` review tool |

Wandering events (travelling parties, pets, patrols, camps) are data too: a package's `wandering.ron` lists casts, scripts and what they say, and species that only come with a party say `party_only`. See [docs/wandering.md](docs/wandering.md) for how they run and the catalogue of 100 kinds.

Content is data: blocks are defined in `assets/content/base/blocks.ron` and loaded through the
same registry a mod will use. Client and server compare a content fingerprint on join. Templates
make whole families from one line (a wood: log, stripped log, wood, planks, leaves, sapling; a
rock: raw, cobbled, polished, bricks; a flower in three looks), and templates are shared across
packages, so a mod's new wood is one line. Blocks carry tags from a shared vocabulary (listed at
the top of `blocks.ron`: what a block is, where it grows, what it is good for, such as `meadow`,
`edible`, `alchemy:calming`, `dye:blue`, `wonder`), a rarity and an optional inventory icon, so
biomes, food and alchemy select blocks without naming them. A plant's `variants` are other looks
of it, each its own block that drops the plant itself, as many as its size;
`BlockRegistry::looks` lists them for generators. A block's `support` says what it rests on:
plants stand on solid ground (the default for plants), hanging ones (`support: Above`: glow
berries, hanging roots, stalactites) need a solid block above; either can only be placed with
that support, and goes, with the column resting on it, when the support is broken. The inventory shows plants as their sprite and
full blocks as a shaded cube. Templates
make whole families from one line (a wood's log, stripped log, wood, planks, leaves and sapling;
a rock's raw, cobbled, polished and brick blocks), and templates are shared across packages, so
a mod's new wood is one line too. A plant can list variants, other looks of itself that become
their own blocks and drop the plant, as many as their size. Blocks carry tags (`flower`,
`wetland`, `edible`, `alchemy:calming`, `dye:blue`; the vocabulary is at the top of
`blocks.ron`) and a rarity, so biomes, food and alchemy select them without naming them.

Colour variants cost no extra art. A block may say `tint: Fixed(r, g, b)`: its textures are
multiplied by that colour when the texture array is built (one layer per texture and colour),
so a greyscale drawing makes a family of woods or dyes and the renderer sees an untinted
block. A `blocks*.ron` file may list `dyes: [(name: "indigo", rgb: (52, 64, 150)), ...]`
(one palette across packages, each dye named by `family.<package>.<name>` in the language
files), and a family line `(template: "dyed", dyes: true)` makes the template once for every
dye, with the dye's name for `{name}` and its colour as the tint. A family recipe may take the
tag of its own name (`tag:dye:{name}`), and a fill whose tag nothing has is skipped. The decor
blocks (`blocks_furnishings.ron`: furniture in three woods, cloth things in twelve dyes,
crockery in three glazes, lights, graves, columns; their boxes are in `shapes.ron`, their
greyscale drawings in the default pack's `decor.ron`, written by `tools/art/decor.py`) are
the first users; Voxmar HD shows the default drawings at twice the fineness until the pack
thread redraws them.

**Packages setting the same data.** Packages load in name order. Names (blocks, recipes,
creatures) are their own package's, so two packages never clash. Data with a single value
(how players move, what things are worth, the weather's tuning, survival's numbers, a
journal page's thresholds, creature limits, the level curve, treasure settings) is the
other case, and one rule covers it (Design draft 91): **a later package replaces an earlier
one's value, field by field, and the load log names both packages**. A later package
writes only the fields it changes (a `movement.ron` of `(walk_speed: 5.0, checks: (burst:
3.0))` tunes two numbers and keeps the rest), and a list is replaced whole. The first
package that sets `movement.ron` gives every field, and the laid-together result is checked
as a whole, so numbers that would hang the server are refused at launch with the package
named. Each replacement is one line (`zz_tuned/movement.ron replaces the
movement core set (walk_speed, checks.burst)`) in the server log and in `Content::notes`.
In code the rule goes through `Laid` (`crates/engine/src/content.rs`): `Setting` and
`settable!` for a struct of numbers, `Laid::set` for anything else; the world generator's
`layered!` is the same rule for climate, land and trail data.

How the design primitives map to the code today:

- **Definition**: blocks (`content/base/blocks.ron`), actions (`content/core/actions.ron`,
  `content/base/actions.ron`), crops (`content/base/crops.ron`), skills
  (`content/core/skills.ron`),
  console commands (`content/core/commands.ron`), kinds of weather (`content/base/weather.ron`)
  and how players move (`content/core/movement.ron`),
  namespaced per package (`base:stone`, `core:mine`; `core:air` is built in).
- **Action**: the only way discrete changes happen. Every action goes through one lifecycle on
  the server (propose, validate, schedule one completion, re-validate, commit or cancel) and
  compiles down to a fixed set of engine effects. Clients validate with the same rules to
  predict. Mining, placing, moving items, throwing them, sowing and picking crops and
  respawning are all actions.
- **Process**: continuous server rules at a scheduling tier (`High` every tick, `Low` once a
  second): falling damage, regeneration, which heals slower where the temperature field
  is freezing, the weather (below), footsteps, which wear paths (below), dropped items, and
  crop growth, which pops only the crops whose next stage is due.
- **Trait**: typed data with declared replication and persistence (`core:health`,
  `core:inventory`, `core:skills`, `core:position`); owner-replicated traits are sent when
  they change.
- **Field**: coarse grids (16 blocks per cell unless a field chooses otherwise), generated
  baseline plus emitted differences: `core:temperature`, in °C, from the climate generator,
  and `core:humidity`, `core:clouds` and `core:precipitation` at 128 blocks, which the weather
  process evolves.
- **Agent**: players, whose controller is input. Machines and creatures come later.
- **Generator**: the `Generator` trait; `Overworld` is the first implementation. Random
  streams are seeded from world seed + generator id and version + stream + position, so
  generation is pure and order-independent. Distant terrain is sampled from the same functions.

## World generation

The world is 384 blocks tall (columns of 12 chunks) with the sea surface at 128: 128 blocks
of ground and caves below it, 256 of land, mountains and sky above. Each world draws a few
physically meaningful dials from its seed (`Overworld::dials`: mean temperature,
equator-to-pole temperature difference and distance, ocean fraction, continent size,
humidity, prevailing wind, tectonic activity, erosion age, river density, vegetation, fantasy,
volcanism, cave density, mountain height, biodiversity), with sensible correlations, and a
featured set: a few rare biomes and other things it boosts and common ones it suppresses, so
no two worlds share a rarity table. Everything else is a continuous field:

- **Land** (`land.ron`, `worldgen/src/router.rs` and `geology.rs`) is built the way
  Minecraft's noise router and Tectonic build it (`docs/terrain-foundation.md`): a few large
  noise fields (continentalness, erosion, ridges, a region selector, and the base climate)
  define a point at every position, and nested cubic splines, adapted from Tectonic's and
  written as data, turn it into the height of the land. Continents about 10 km across are
  separated by a connected web of seaways, with shores fractal down to a few dozen blocks;
  ranges run in long chains along the erosion noise's zero lines, with crests whose detail is
  dragged down their slopes; valleys follow the ridge noise's zero lines, and the lowlands
  between take the style their region and climate call for: plateaus and valleys, rolling
  hills, two-tier terraces, badlands ridges, wetland, with gentle hills a few hundred blocks
  apart over most of them. Heights over the sea are scaled with the
  world's mountains, stretched by their square and eased under a ceiling; the deep sea floor
  has trench and ridge lines. The relief is worked out on a 4-block lattice and interpolated.
  On the spines of ranges a 3D density (`crag` and `density` in `land.ron`,
  `worldgen/src/density.rs`) moves the ground by up to nine blocks, so rock breaks into tors,
  crags, ledges and overhangs; it never hollows the ground beside water, under trails or at
  the sea, and a column's height is the top of its density however it is asked for.
- **Terrain shapes** (`terrain.ron`, `worldgen/src/shapes.rs`) rise where the climate calls
  for them, on the same bands as the biome table and never by asking which biome won:
  wind-aligned dunes with steep lee faces, banded mesas stepping down to the valleys,
  stepped canyons, karst towers on limestone, clusters of spires, volcanoes with breached
  craters gathered in volcanic fields on land, natural arches (a rare find: short, thick spans of
  rock with a window through the middle, one at a site, sometimes two or three), sea cliffs notched at the waterline with stacks offshore, and fjords
  where valleys meet cold coasts. Every outline follows noise, so none is a circle; only
  volcanoes and arch sites stand around a centre, volcanoes' outlines warped into
  irregular lobes. Rarer shapes
  take only some regions of their climate.
  Towers and spires do not cool the climate on their tops.
- **Temperature** falls towards the poles (latitude repeats along z every 9–11 km from pole
  to equator, `zones` in `climate.ron`, colder to the north, so hot, temperate and cold land
  always lie in that order, with anomalies kilometres wide) and with altitude.
- **Precipitation** follows moisture the prevailing wind carries in from the sea, worked out
  over continents and ranges alone so climate zones are regions kilometres wide rather than
  stripes along every slope: wet coasts, drier interiors, rain shadows behind ranges, wet
  tropics and dry subtropics.
- **Humidity**, what biomes and plants go by, weighs a slow wetness field (16 km) most and
  the wind's rain and the latitude's belts a little (`humidity` in `climate.ron`), and leaves
  out how much the local warmth evaporates: humidity that followed latitude as fast as
  temperature does would cut every climate zone into thin bands of biomes.
- **Rivers and lakes** (`worldgen/src/water/`) run where water would. Trunk rivers are routed on
  a 64-block grid over the land's broad shape and cut their valleys (and, rarely, tunnels
  through a ridge) into it; creeks and streams are routed on an 8-block grid from the hills to a
  trunk, a lake or the sea, joining as they go. Each river is as big as the rain on its whole
  catchment. Shallow pits are breached; deeper ones fill as lakes at their own height, lower
  and without outlet in dry climates, down to salt flats. Where a breach cuts deep through
  high ground, the valley closes into a slot gorge: a ledge beside the water and steep,
  ragged rock walls. Water steps down in whole blocks,
  falling as waterfalls onto plunge pools where the drop is three blocks or more. Flow is
  summed region by region through records of where water crosses each 1 km region's border,
  so rivers can be any length and nothing depends on generation order.
- **Settlements** have deterministic residents with homes, daily routines and guards.
  Ordinary villagers give no loot or combat XP; deaths persist until a newcomer arrives.
- **Trails** (`worldgen/src/trails/`, materials in `trails.ron`; laid only near settlements,
  with `laid: NearSettlements`) join the places worth walking
  to: summits that stand above the land around, lake shores, the banks of waterfalls, the edges
  of long drops and flat trailheads in the valleys, a few per 512-block region. Nearby places
  are joined into a sparse network (a Gabriel graph, with no two trails setting off side by
  side), each trail routed by A* over the land with costs adapted from RoadWeaver: walking
  along the contour is cheaper, grades past soft and hard limits cost more, and so does the
  fall line, so trails traverse slopes and switch back up them. Trails that run within 18
  blocks of a longer one join it, making junctions. Each is drawn as a smooth curve cut into
  hillsides and raised on low banks, its tread worn to packed earth, gravel or mud by biome
  with trodden grass at its edges, across shallow water on stepping stones, across creeks on
  logs and across rivers on plank bridges with piers (crossings over 100 blocks are not built).
  Cairns mark trails above the treeline and stand on summits, old waymarkers stand at
  trailheads, and summits and viewpoints keep a clearing. Everything is a pure function of the
  seed and the land, planned region by region in caches bounded by bytes, so nothing depends
  on generation order. Coarse sampling (`Overworld::height`, `site`, `surface_sample`, used by
  distant terrain and the weather) leaves trails out; generated columns have them.
- **Caves** (`worldgen/src/caves.rs`) come in kinds of very different rarity: winding tunnels
  everywhere, more tangled with depth, opening to the surface as entrances in some places;
  cavern regions with pillared cheese caverns; thin noodle tunnels; rare multi-floor
  labyrinths, buried ravines (30–70 deep, never open to the sky, a few a kilometre long) and
  sinkholes; and very rare giant cavern systems, most holding one great lake. Water tables
  per 16×16×12 cell (as in Minecraft 1.18) flood some caves at their own levels, with sea
  level near the surface so caves under the sea are flooded rather than draining it, and lava
  below y 10, with rock left wherever a fluid would otherwise touch air. The cave density
  dial, the world's featured rarities and the seed set how much of each kind a world has
  (`Overworld::cave_style`).
- **Selection fields**: continentalness, a landform class (erosion), and seeded weirdness,
  aether (how fantastical the land is), fertility, variant and rock provinces 2-4 km across.

**Biomes** are data in content packages (`assets/content/base/biomes.ron`, with the climate
bands in `climate.ron` and rock provinces in `rocks.ron`), loaded like blocks, so a mod adds
one the same way. The temperature, humidity, continentalness and landform bands form a table
in which every cell belongs to some biome (checked at load); the picker sees temperature and
landform averaged over a few hundred blocks, so a range, hill country or plain is one place
rather than a scatter of hills and summits, and the land's own continentalness at each spot,
so a coast's biomes keep to their side of the water line; commons share a cell along the slow weirdness
field in the same slices wherever they meet, and rarer biomes take its extremes, so they
appear in whole regions of some worlds and not in others;
variants take shares of their parent along a field 8 km across; ecotones sit on borders;
fantastical biomes claim cells where aether (12 km across) runs high. The scales of these
fields are data (`scales` in `climate.ron`). Borders blend over 16-48 blocks. Each biome's ground comes from
ordered surface rules (slope, altitude, climate, rock province, noise patches and contour
lines such as game trails, which no biome uses for now; shared sets in `surfaces.ron` break steeper soil slopes up into
patches of coarse dirt, gravel and rock) over the stones of its rock province, while snow, ice and bare
rock follow the climate everywhere. Slope rules go by the ground's steepness over a few
blocks, not single steps, and materials meet in patches tens of blocks across with ragged
edges rather than interleaving block by block; the risers of steep, stepped ground show the
province's rock beds under a lip of the biome's ground. Trees (`trees.ron`: procedurally shaped giant spruce, pine,
birch, oak, dark oak, cherry, willow, jungle giants, acacia, palm, mangrove, shrubs, cacti)
and plants (`plants.ron`, with nutrition, alchemy and dye properties) are a shared pool:
each says what climate, ground and habitat it likes, biomes select from the pool by tag or
name, and the climate at each spot weighs the candidates, so a new plant with the right
tags grows in every biome that fits. Biomes, trees, plants and rock provinces carry rarity
tiers from common to legendary, and wonders (anything tagged `wonder`) grow where aether
runs high: wonder plants scattered, wonder trees as a find (a lone tree or a tiny grove every
few kilometres) except in the fantasy biomes that name them. Each biome keeps to a palette
(`crates/worldgen/src/hue.rs`): every candidate's colour is worked out at load (its block's
colour, or the grass or foliage colour where tinted) and weighed by how near it lies to the
biome's foliage, grass or ground colour (or a `palette` colour the biome sets, as maple woods
and cherry groves do), so many kinds of similar colour grow together while a clashing one is
a rare accent; flowers grow a few neighbouring hues per place, mixed ground blocks keep to
their main block's colour, and ground patches unlike the ground grow fewer and smaller.
Forests grow in stands: a slow noise picks each stand's kind, with a few of the others among
it, and stands of different biomes meet along the border rather than mixing across it.
Forests have structure: one field grades them from dense old-growth cores with the
giant trees through ordinary woodland to scrubby edges and gaps, and a global field opens
clearings with a soft edge. Ground decoration (`decorations.ron`, another shared pool biomes
select by tag) is placed in a fixed order of stages: patches of dirt, gravel and moss; boulders
and scattered stones of the rock province, mossy on their north side in damp woods; bushes of
the trees growing there; ornaments each tree makes of its own blocks (needle and leaf litter,
fallen leaves, fungi at the trunk's foot); fallen logs (some thick and hollow, with branches and
the stump they broke from) and stumps, each laid only where the ground holds it; then flowers
(sparse, a few kinds at a time, thicker at forest edges and along creeks), fungi and a carpet
of grasses and ferns over most open ground; and last a snow blanket on every level open to the
sky where snow lies, using the same snowy blocks as snow that falls in play. Each place also has an atmosphere (`Overworld::atmosphere_at`: haze, sky and
colour-grading nudges, and ambient particle sets from `ambient.ron` such as pollen,
fireflies, spores, dust, drifting snow and aether motes), taken from the climate unless a
biome sets parts of it, and blended across borders.

**Cave biomes** (`cave_biomes.ron`) are biomes of the `Underground` layer, placed by the same
picker in 3D: the climate above, plus a depth, height and rock province a claim may ask for,
and a weirdness field that also varies with depth, so cave biomes are wide regions stacked in
layers. Plain caves fill most of the underground; dripstone caves lie under dry land and in
karst, lush and fungal caves under wet land, frozen caves under cold land, molten caves near
the lava and under volcanic land, deep dark caves far down; painted (bands of fired clay) and
crystal caves are rare, and two fantasy caves, glowroot hollows and the starlit grotto, grow
only where aether runs high. Each lines its floors and ceilings through its surface rules,
replaces the rock around it with its own stones or strata, and grows decorations from floors
and ceilings (`cave_decoration`): plants and mushrooms, stalagmites and stalactites, icicles,
hanging roots and glowing berries, crystals and lichen, which light the caves; hanging growth
always hangs under rock. Frozen caves freeze their pools, and lush caves and glowroot hollows
have pools that glow from below. Wherever daylight falls straight down a shaft to a cave
floor, a small garden of turf, grass, ferns and flowers grows in the light. Buried in the rock
are geodes (hollows lined with crystal, calcite and basalt, glowing clusters on their floors)
and fossil skeletons, found by digging or where a cave cuts through them. Lava burns (4 health
every half second). `Overworld::cave_biome_at` gives the cave biome at a block.

`Overworld::site(x, z)` returns every parameter at a position for gameplay to use, including
its biome, the biome next door and the distance to their border; `Overworld::biome_at` and
`Overworld::locate_biome` serve commands.
`cargo test --release -p voxmar_worldgen map_preview -- --ignored --nocapture` (optionally with
`SEED=n` or `SEED=1,2,3`, `BIOMES=1` to colour by biome, `RELIEF=1` by height, `SHAPES=1`
tinted by terrain shape, `FIELD=erosion` and so on, `X`, `Z`
and `SCALE` to move and zoom) writes a shaded 8 km map to `target/map_preview.ppm`, with lakes,
rivers and creeks in their own blues, trails over them (brown, with fords cyan, bridges red,
and the places they join marked; `TRAILS=0` leaves them out), and prints each biome's share of
it, how large biome regions are (variants counted with their parent, the shore the sea's
biome reaches left out) and which picker axis draws the borders, with the pairs of biomes
along each. `trail_patch` (with `SEED`, `X`, `Z`, and `LINES=1` for trail centrelines) draws 160×160
generated blocks from above to `target/trail_patch.ppm`; `trail_sights` prints `--pose`
arguments for fords, bridges and switchbacks near spawn; `trail_stats` counts places, trails
and routes that failed. `river_sights` prints
`--pose` arguments for captures of the places water makes near spawn (highland lakes,
tarns, waterfalls, confluences, river mouths, tunnels, islands, hot springs), which
`Overworld::water_sights` finds; `tarn_census` (with `SEEDS` and `REGIONS`) counts the tarns
around mountains and alpine meadows in and out of tarn country, their visible water area,
and the share of the land that is lake bank. `LOTUS=n` also generates the connected shore
of up to n visible pools per seed to count actual lotus plants and visible clusters. Existing high lakes and snowy peaks are counted
separately; `Overworld::locate_tarn` finds a visible pool filled by the tarn rule.
`rare_herb_census` (with `SEEDS`, `CHUNKS`, `PLACES` (up to three far places of each biome a
world), or `WILD=a,b` or `WILDTAG=tag` with `WILDBIOMES=a,b` for other plants instead, `AT=1`
for where the biggest patch of each is) counts the
rare herbs as generation places them, per million columns of each biome naming them;
`shape_sights` prints a `/tp` command and `--pose` for the best example of each
terrain shape and the highest peak.
`ore_preview` (with `SEED`, a centre `X`, `Z` and `COLUMNS`) writes an x-ray of the ore veins
from above and a cut through the richest row to `target/ores_*.ppm`, with the surface samples
in white, and prints each ore's blocks per column and the share lying alone.
`cave_preview` (with `SEED`, and a centre `X`, `Z` and two heights `Y1`, `Y2` for level cuts)
writes an x-ray map, sections, level cuts and a cave biome map of the caves over 1.5 km to
`target/caves_*.ppm`; `cave_sights` prints `--pose` arguments for captures of caverns,
labyrinths, ravines, sinkholes, entrances, geodes, bare fossils and the best view of each cave
biome near spawn (or near `X`, `Z`, or the nearest surface biome `NEAR=name`, with the
`--command "/tp X Z"` that takes a capture there), with each cave biome's share of the caves
there; `decor_sights` prints `--pose` arguments for captures of old-growth forest, a forest
edge, a clearing, a meadow, a slope, a cold forest, dry grassland and snow near spawn
(or, with `BIOMES=a,b`, the nearest place well inside each named biome); `tree_preview`
(optionally `TREES=oak,birch`, `SAMPLES=n`, `VIGOUR=0..1`) draws every tree species a few times
over, at the ages they happen to draw, in isometric views to `target/trees_N.ppm`;
`tree_sights` (with `SEED` and `BIOMES=forest,taiga`) prints the `/tp` command and `--pose`
of views of the biggest tree near the middle of each biome, from under its crown, from a
rise eighteen blocks off, from above and from far off, so the same trees can be compared
however they are grown; `column_timing` reports generation time per column, and `river_speed` the cost of building a
region's rivers apart from the columns generated after (all `-- --ignored --nocapture`).
`generation_stages` breaks generation time per column down by stage (climate, biomes, rivers
and their region builds, trails and their planning, surface, caves, cave biomes, fill, ores,
trees, decoration, packing), cold and warm, and reports lighting and meshing per column;
`first_column` times a new world's spawn point, spawn column and first 9×9 columns, building
regions on one core and on all, and `land_samples` the cost of one sample of the land on the
rivers' grids. `biome_spread` (with `SEED=1,2,3`; `SCALE=16` for 16 km maps, `AXES=1` for
the regions of each climate axis alone, `SMALL=1` for `target/small_N.ppm`, a picture of the
land in regions under a kilometre across) prints the biome statistics of `map_preview` quickly,
without rivers or a picture. `land_heights` (with `SEED=1,2,3`) prints how high the land is, its share above
a few heights, and `climate_bands` how temperature and moisture spread over it, for tuning
`land.ron` and `climate.ron`. `output_is_unchanged` and `light_is_unchanged` (ordinary tests)
hash generated columns and their light on three seeds, so performance work cannot change a
block; when a change is meant, they print the hashes to put in `crates/worldgen/src/perf.rs`.

## Worn paths

Feet wear the ground (`crates/server/src/wear.rs`, rules in `assets/content/base/wear.ron`).
Each step onto a block wears it by a step, the blocks to either side by half and the one ahead
by a quarter, so paths form about three blocks wide. Plants go first: tall grass is trampled to
short grass, short grass, ferns and flowers to nothing. Then the ground passes through stages,
grass to worn grass to packed earth to coarse dirt, each block drawing how many steps each stage
takes from a hash of the seed and its position, so paths fray irregularly at their edges. The
steps are kept sparsely per surface column in the column (`Column::wear`), so they save and
load with it; only the changed blocks are sent, and only to players who have the column. Worn
ground heals a stage every five days nobody walks on it (twice as fast for a lone patch),
applied when a saved column loads, and grass coming back sometimes sprouts a tuft. Healing only
undoes what feet did, so generated trails keep their tread.

## Weather

The server simulates the weather; clients only draw it. What drives it is a pure function of
the seed, place and time: moist and dry air masses a few kilometres across that drift with the
world's prevailing wind and slowly change shape, and smaller, shorter-lived updrafts that are
strongest on warm afternoons. Near players, a `Low` tier process evolves humidity, cloud,
precipitation, ground wetness and lying snow cell by cell (128 blocks): humidity follows the
passing air mass away from its baseline (the usual humidity of the climate's annual rainfall),
cloud condenses as the air nears saturation, sooner in rising or cold air, and evaporates as it
dries, and thick cloud rains, which dries the air again. Typical land rains 8-20% of the time,
rainforests 30-50%, deserts almost never. Rain wets the ground, which dries over minutes, and
snow lies where it is cold enough and melts above freezing; as it deepens, grass turns snowy
block by block and turns back as it melts. The air is colder at night and warmer in the
afternoon, more so in dry climates and under clear skies, and it snows below about 0 °C. Only a
heavy downpour under strong updrafts is a thunderstorm: each second it may, rarely, raise a
lightning strike somewhere in the cell (a few flashes a minute in the heart of the fiercest
storm), which nearby clients see as a flash and a bolt and hear as thunder once the sound has
travelled: a crack, boom and roll close by, a muffled rip and roll farther off, only a low
roll from far away. What the weather would do to machines and buildings left out in it is
worked out in the same cells as hazard fields (hail's impact, the wind's load, corrosion,
frost, heat and recent strikes) with lightning strikes kept as events, for gameplay to come
(docs/weather-effects.md, "Hazards"). A cell coming into range first catches up on its last ten minutes, so
the weather at a place does not depend on when anyone arrived, and cells left behind return to
their baseline. The process uses only exact arithmetic on seeded hash streams, so it is
deterministic on every machine. It runs on the simulation clock, so it carries on when the
time of day is frozen or sped up; only what follows the sun reads the time of day.

The wind is part of it: light under settled skies, freshening and veering as fronts pass,
strong in storms, livelier in the afternoon, stronger on open heights and over water and
calmer in forests and valleys. Gusts sweep across the land with it, from one deterministic
function server and clients share (`voxmar_engine::weather::gust`), so grass, particles and
later gameplay agree.

Effects come on top, as data in `assets/content/base/weather.ron`: each is a product of
smooth conditions on the weather, the local climate and land (coast, valley, altitude,
latitude, moisture, vegetation, aether) and the time of day, raises channels of the weather
clients draw, and has a rarity. There are drizzle and downpours, thunderstorms (and the
towering supercells of hot, wet country, common there and rare elsewhere), heat lightning on
humid summer nights, sleet, freezing rain, hail, blizzards, thundersnow, squalls, drifting
snow, dust and sandstorms, mist after rain, dawn mist, mist off lakes on cool mornings, steam
off hot ground after rain, valley fog, sea fog, frost, diamond dust in bitter cold, heat haze,
rainbows (and rarely double ones), halos and sun dogs, the aurora near the poles (and rarely
far from them), shooting stars, meteor showers, ball lightning, marsh lights, and where the
land's aether is high glowing motes, the very rare aether storm and the legendary starfall.
Light rain and cloud are everyday; heavy rain is uncommon outside the tropics; wind is calm
to a light breeze most of the time, strong now and then, and a gale only in storms. Rarity is drawn per stretch of land (1.5 km) and time
(the effect's typical length) from the seed, and each world features a few rare effects and
suppresses a common one, so drizzle and mist are everyday sights and a meteor shower is an
event. `cargo test -p voxmar_server effect_spread -- --ignored --nocapture` prints how often
each shows across kinds of country, and `climate_spread` how often each kind of country is
cloudy, rains or snows at each intensity and how hard the wind blows.

Each player is sent `ServerMessage::Weather` for where they stand when it changes or every five
seconds, with the effects showing, and the client eases towards it: cloud builds and clears
over tens of seconds, rain sets in over a few, fog gathers slowly. The low cloud is a layer in
the world that drifts with the wind; its shape follows the weather, its base comes down onto the
hills in rain, and from a mountain it passes around and below you. Rain, drizzle, snow, sleet, hail, dust and motes
fall or drift through a box around the camera, slanted by the gusting wind and stopped by a map
of the highest block around the camera, so nothing falls indoors, under trees or in caves.
Ambient particles from the land's air (pollen by day, fireflies on some warm nights, spores in
damp forests, drifting snow in the cold) come from `ClientAtmosphere`, which samples
`Overworld::atmosphere_at` around the player and applies the weather and time of day, and
also tints the haze, sky and grading. Fog, mist, whiteout and dust close in the view; valley
fog, dawn mist and sea fog fill the land under the weather's fog top and thin above it, uneven
and drifting near the eye, so they look like volumes from inside and above; rain wets the ground
and fills puddles; frost whitens it; clouds cast moving shadows. Rainbows, halos, aurora,
shooting stars and heat haze are in the weather the client receives and wait on the renderer
(docs/weather-effects.md). F3 shows the weather and the effects showing.

Blocks give off particles near the player as their data says (`particles` in blocks.ron, each a
`style` of Smoke, Ember, Spark or Flame with its rate, burst count, place, spread, colour, size
and how high it rises; `engine::block::Emission`): a campfire's flames lick a block and more
above it, its smoke rises a few blocks, thinning, embers float as high, and now and then it
crackles with sparks; a torch's flame flickers. The nearest 32 within 24 blocks emit, those
beyond 10 blocks fewer (client/src/emitters.rs).

An admin's `/weather KIND` holds the cells within about a kilometre at that kind of weather
(kinds are data in `weather.ron`; what a kind leaves out stays natural, so `/weather rain`
snows where it is freezing) and its effects at full strength. Players there are told at once
and change over in a second or two. When its time is up it fades over a minute, and the
released cells drift back to the natural weather over the next few minutes. `/wind` does the
same for the wind alone, on top of any forced weather.

## Water

Water and lava flow as in Minecraft (`engine/src/fluid.rs`): a source spreads seven blocks over
flat ground (lava three), heading for the nearest edge within four blocks (lava two), falls as
far as it can, and dries up once nothing feeds it. Two sources make a third between them, so
water taken out of a pool comes back, and water that fills a hollow it cannot run out of stays
there as a pond. Lava cools: where a flow stops, its thin edge crusts over with basalt after
twenty seconds, and a flow left without its source hardens where it lay. Water quenches it: a
lava source turns to obsidian, flowing lava to basalt, and lava pouring into water turns the
water to broken basalt (`cobbled_basalt`). Every state is its own block (`water_flow_1` to
`water_flow_7`, `water_falling`; rivers also have pinned water that carries a current), so
chunks, saves and the network treat them like any block. Flowing water and river currents
carry swimmers along, and they look it: flows slope down from their source, waves run with
the flow and rapids froth, and falling water is streaked white (see docs/rendering.md).

The server's kernel is event-driven: a block change wakes only the fluid beside it, each cell
moves on its fluid's tick (water every five ticks, lava every thirty, with a little hashed jitter
so fronts do not move in lockstep), and water that has settled costs nothing. Generated water is
already settled, so loading the world wakes nothing. A tick runs at most a budget of updates
that adapts to what they cost and how busy the server is (aiming at 4 ms), and the rest wait;
each player is sent one `Blocks` message per tick with the changes in columns they have. A lake
48 blocks across pouring 60 blocks into a cave (`a_large_flood_is_fast`: 312 thousand updates
and 154 thousand changed cells over 413 ticks) takes about 0.1 s of work in all, 0.3 µs per
update, and never more than 2 ms in one tick.

## Sound

Music is in `assets/music`, a folder for each category, and plays now and then: in a world, 10 to
20 minutes of quiet between tracks, as in Minecraft. As a track starts, its title and category show
small at the top right for a few seconds. `M` toggles it, and the audio settings switch each
category on and off. The categories
are listed in `assets/content/base/music.ron`: `suno`, `lyria` and `elevenlabs` are our own tracks
(ElevenLabs' general ones play anywhere, its biomes' only in their biomes, its boss tracks only in
a boss fight), and with
Minecraft assets on, the tracks of the player's own Prism Launcher instance (read where they are,
never copied) join `minecraft_menu`, `minecraft_world`, `minecraft_deep` or `minecraft_records` by
the rules there. A mod adds a category in its own `music.ron`. All of our own music is
AI-generated, from two sources (recorded 7 October 2026 from the commits that added the tracks
and the tags inside the files):

| tracks | source | terms |
| --- | --- | --- |
| `lyria/`: `dawn_meadow`, `highland_wind`, `lantern_village`, `old_forest`, `rain_on_stone`, `starlit_night` | Google Lyria 3 (`google/lyria-3-pro-preview`) through OpenRouter, by `tools/generate_music.py` (c100fc6) | Google does not claim ownership of generated output; use is subject to Google's generative AI terms and Prohibited Use Policy, and the files carry Google's SynthID watermark |
| `suno/`: `moonlit_quest`, `moonlit_quest_2`, `quiet_sky`, `skyward_serenity` | made with Suno, added by Mariano in 5197578 (each file's tags hold its Suno id and creation time, 3 October 2026) | under Suno's terms, songs made on a paid plan (Pro or Premier) may be used commercially, while songs made on the free plan are for non-commercial use only and Suno keeps their rights; which plan made these is not recorded |
| `elevenlabs/`: rounds 4 (forest, snow, desert, fantasy) and 5 (general, and one boss track each for forest, snow, desert and fantasy) | ElevenLabs Music (`music_v2_5`) by `tools/music/elevenlabs/forest_gen.py`, added 13 October 2026; each track's exact prompt and settings are in the `.json` beside it, and `tools/music/elevenlabs/NOTES.md` says how they were made | under ElevenLabs' terms, which depend on the plan of the account that made them; which plan made these is not recorded |

Purely AI-generated audio generally has no copyright of its own, so no source gives us
exclusive rights to these tracks. The loading screen's picture, `assets/ui/loading.png`, is our
own: 36 frames of the game itself, captured along the coast near spawn, then cropped, softened,
tinted and tiled (801915d). Starting a game fades the menu's music out, so a world
opens in quiet; returning to the menus brings it back.

Voxmar ships none of Minecraft's or any modpack's music. Only with the Minecraft assets setting on
does music from the player's own install play: the tracks of their Prism Launcher instance join
Voxmar's (the most recently launched GT New Horizons one, or the instance `prism_instance` names in
`settings.ron`, in the launcher folder `prism_folder` names, empty for the standard one; a named one
that is missing is logged). They are the vanilla music and records the launcher's asset store holds
for its Minecraft version, a music mod's soundtrack zip (HorizonMelodies, with the newer Minecraft
and the StaTech tracks), and music in its resource packs and mod jars. A file found once (the same
size and CRC-32) is not added again, so packs repeating the soundtrack add nothing; the log names
every copy skipped and the track it repeats. Nothing is copied: each track is read from where it is
as it is about to play, and played only once it is checked to decode (one that does not leaves the
rotation). `assets/content/base/music.ron` holds the numbers, where to look, and which moments each
track suits by its path: menu tracks on the title screen, Nether, End, cave and structure tracks
deep underground, the rest in the world; Voxmar's own play everywhere, a quarter of the picks.
`cargo test --profile iter -p voxmar_client prism_music_decodes -- --ignored --nocapture` lists
what this machine's instance has, with every copy skipped from the 1.21 and StaTech resource packs,
and checks every track is placed and decodes.

Block sounds follow each block's material (`sound: Wood` in its definition; the kinds are listed
in `blocks.ron`), so new blocks sound right without new audio. Steps, landings, placing, the taps
while mining and breaking come from the player's own Minecraft install when it has them and the
Minecraft assets setting is on (the official launcher's or Prism Launcher's asset store; nothing
from Minecraft is shipped), with
Minecraft's pitches, levelled to the loudness of our own cues so footsteps are heard. Otherwise
they are synthesized at startup: short, soft and warm, with several variants per cue and a small
random pitch change so nothing repeats exactly. Footsteps come from whatever is underfoot: the
ground, the rustle of plants, flowers and moss carpet walked through (with the ground softer
under them), a splash when wading, strokes when swimming. Every broken block plays its break
sound once, whether the player's own break or one the server reports, fading out over 16 blocks
for others' breaks, panned to the nearer ear.
Rain is a synthesized loop of tiny drops over a soft hiss, as loud as the rain is heavy and
quieter under cover. `cargo test -p voxmar_client sfx_preview -- --ignored` writes every
synthesized cue to `target/sfx_preview.wav`.

Ambience follows where the player is. About once a second a
flood fill from the eye (on a worker, capped at 16,000 cells) measures how much air is around,
how much of it is open to the sky or under leaves, and the leaves, wood, water, grass, rock,
sand and snow it meets, nearer ones counting more. With the climate and biome from the world
generator, height above the land around, depth underground, the weather and the time of day,
that makes an environment vector, and `ambience.ron` in any content package lists what plays
where: beds (breeze, high wind and storm wind, rustling leaves, grasshoppers, crickets,
surf, a fire's crackle by a campfire, hearth or kiln, synthesized; rain by strength and cover, hail, dripping trees, streams, lakes, a
waterfall's roar by falling water, a slight cave air and lava bubbling, recorded) as stereo
segments that crossfade and never repeat back to back, and one-shots at a rate per minute (songbirds and a dawn chorus in the leaves around, a woodpecker on a trunk,
owls, a cricket in the grass, frogs in the water, gusts on heights, creaking trees, drips
near and far and rumbles in caves, lava bursting bubbles, logs snapping in a fire). Recordings are CC0 field recordings in the package
(`assets/content/base/sounds/`, credited in THIRD_PARTY_NOTICES.md), as are thunder near to
far, swimming strokes and splashes (`sounds.ron`). One-shots sound from real blocks the fill found,
panned by direction, with a reverb as wet as the place is enclosed. Every condition is a soft
range with a fade, and rain ducks layers rather than silencing them. Indoors is asked apart
(the fill leaks out of an open door): a roof over the eye and walls met by short rays around it
make the `shelter` axis, and a roof close overhead the `roof` axis. Outdoor layers duck by
shelter and are muffled as far as they duck (one-shots by a low-pass; beds by a darkened twin,
made at startup at a quarter of the rate, played beside them), rain drums on the
roof, a fire's layers never duck, and thunder is quieter (`ambience.ron` `shelter`;
`indoor_preview -- --ignored` writes a wood and a storm heard outside and then from a home by
its hearth to `target/indoor_preview.wav`). Captures run it silently
and log which beds would play. `cargo test -p voxmar_client ambience_preview -- --ignored`
writes about twenty seconds of each of seventeen typical places to `target/ambience_preview.wav`, and
`ambience_survey -- --ignored` prints what the scan makes of real places in a world (`SEED`).
`weather_sound_preview -- --ignored` writes the thunder, swimming and splashes to
`target/sound_preview/`, as the game plays them.

What the player does is heard as they do it: eating, crafting at the grid, loading a home
station, lighting a fire or a kiln, opening a chest or a door, putting food on to cook,
lying down to sleep, casting a line, feeding an animal, emptying a pouch, feeding a machine,
picking a lock, praying at an altar, spreading fertiliser and throwing something down; and what
an action brings as it lands in the pack (`brings`, by the item's tags): a fish landed, food off
the fire done, or burnt.
`sounds.ron`'s `actions` gives each action, by name, its clips (one picked at random, at a
slightly different pitch) and a volume, around the loudness of placing a block, or says it is
`silent` on purpose or `heard` elsewhere (by the block touched, the interface or the fight). A
later package replaces an action's sound; the launch fails on a sound for an action the content
lacks, and on an action, a mod's too, that says nothing of how it sounds. The clips, and the pop of an item picked up
when Minecraft's own sounds are off, are our own, synthesized by `tools/generate_sfx.py`.

The interface is heard the same way: `sounds.ron`'s `interface` names what each thing that
happens in it sounds like (a button pressed, the pointer over one, an item picked up or put
down, half a stack split off, a screen opened or closed, a quest picked in the book, something
refused, and the events' jingles): the chosen set's click or tap at a volume and pitch, clips of
its own (a page turned, a soft "no"), a jingle, or `silent`. The launch fails on a name the
interface never makes and on one it makes that says nothing.

What the player does is seen as well (`crates/client/src/fx.rs`): dust settles round a block
set down, crumbs fall as food is bitten, a crop picked springs a few leaves, a hoe or a seed
kicks up soil, fertiliser falls as dust, a table, machine or station worked throws chips, food
on the fire steams, flint sparks, motes rise at an altar, and a glint flashes over what the
grid made. `action_fx.ron` names how every action shows: an `effect` (where it shows, what its
particles look like and how they move, all data), `shown` by breaking or the fight, or `none`;
the launch fails on an action it says nothing of, as for sounds. `fx_contact_sheet -- --ignored`
sketches every effect's motion to `target/fx_contact_sheet.png`.

Creatures are heard: footsteps on whatever block they walk, bleats, grunts and clucks while
they wander, munching, sighs and purrs at rest, barks and snorts of alarm, calls carrying far,
mating calls, snarls when they strike, and cries when hurt or dying. `creature_sounds.ron` in
any content package gives each species a *voice* (several recordings per event, one picked at
random each time at a slightly different pitch); a voice can be `like` another, a missing event
falls back to the nearest it has, and a later package replaces voices and species by name, so a
sound pack can revoice every creature. Each voice says how tall the animal recorded was, and
species of another size play it higher or lower, each individual a little more for its genes,
the young higher still. Sounds come from where the creature is, panned, and fade out over their
event's reach (a call farther than a munch). Supernatural creatures shimmer, ring and echo
slightly. A creature never talks over itself and at most fourteen play at once. The recordings
are Freesound's (CC0 and CC-BY, credited in THIRD_PARTY_NOTICES.md); hisses, slime, wings,
clicks, embers and the wisps' chimes are synthesized. `cargo test -p voxmar_client
creature_sound_preview -- --ignored` writes every species' sounds, as each plays them, to
`target/creature_sounds/`.

The mix (`sounds.ron`'s `mix`, crates/client/src/mix.rs) holds it all together. Each kind of
sound (music, ambience, interface, everyday, combat, jingles, creatures, weather) has a loudness
band, and tests measure every clip we ship, as the game plays it, against its band; Voxmar's own
music tracks are each brought to one loudness by levels `music.ron` lists (a test measures the
files again and says what to write when one is off). The music dips under a jingle and under a
boss's voice, and comes back after. Each kind has a cap on sounds playing at once: past it, the
farthest, then the oldest, fades out to make room. Things the player can walk up to (blocks,
creatures) fade along one curve to silence at their reach; the ambience's distant birds and
thunder fade by their own reach, never quite to nothing.

## Rendering and performance

- 32³ chunks, palette-compressed at 0/1/2/4/8/16 bits per block; uniform chunks store nothing.
- Lighting: 4-bit sky + 4-bit block light, flood-filled per column on worker threads over the
  column plus 17 blocks of padding (light reach + mesh border), which is exact without any
  shared state. Edits relight only the columns within reach.
- Meshing: face culling 64 cells at a time on bit columns, then greedy merging of faces with
  equal texture, tint, AO and smooth light. 8-byte vertices; 16-bit indices when they fit.
- Smooth lighting and ambient occlusion per vertex, AO-aware quad triangulation.
- Cave culling: per-chunk face connectivity and a breadth-first visibility walk
  (Tommaso Checchi's algorithm) hide chunks the camera cannot see.
- Bevy's GPU preprocessing does frustum culling and indirect batched draws.
- Pixel-art texture array with alpha-weighted mipmaps; climate tints from colormaps in the shader.
- Plants (grass, flowers) are two crossed, double-sided quads that sway from their base.
- Leaves draw the faces between them too, like Minecraft's fancy leaves, so crowns show the
  same depth through their holes from every angle: one double-sided face where two leaves
  meet, none between leaves buried on every side, and only a crown's shell casts shadows.
  Faces between two of the same glass, ice or water are hidden; ice is clear blue with a sky
  reflection over still water.
- Water is drawn in its own pass over the finished scene: clear in the shallows and taking
  the biome's colour with depth, from how far light really travels through it; the bed bent
  by wind-driven waves; reflections of the terrain marched through the depth buffer, and of
  the sky elsewhere; sun glints and foam along shores. The Water setting (Low, Medium, High)
  trades waves and reflections for speed, and shader packs scale each part.
- Day/night cycle, sky dome with sun, moon, stars and high cloud, volumetric low cloud in the
  world (lit by the sun through itself, shading the land, reflected in water), rain and snow,
  aerial perspective (haze that thins with height and glows around the sun), ground fog as
  drifting volumes and weather fog, volumetric light (shafts through canopies, the sun's glow
  in haze and mist), HDR with bloom.
  The sun, moon, stars and planets are measured in pixels, so they stay round at any field
  of view. Nights are dark but readable: under a full moon, faces turned to it stand out
  from the shade and the skyline shows; a moonless night is darker, never black; campfires,
  hearths and lit windows are what light it. How bright a night is lives in
  `night_light.ron`, which the game refuses outside the bounds playtests set (black, or far
  too bright).
- Soft cascaded sun shadows whose penumbra widens with distance from the caster, and which
  stand still (the sun's light moves in tenth-of-a-degree steps); sky light that falls off
  steeply into forests and overhangs, ambient occlusion, leaves glowing against the sun,
  moving cloud shadows, grass block sides whose fringe takes the top's biome colour (the pack's
  side overlay, as Minecraft draws it), and blocks that give off light glowing by themselves.
  Shader packs
  (`assets/shaderpacks/*.ron`; Verdant by default, Classic the plain look) scale all of it.
- A sunlit top face gets about as much light as Minecraft's full brightness, and the display
  transform passes colours up to 0.8 through unchanged (rolling off only brighter ones), so
  blocks keep their texture colours in daylight.
- Distant terrain to ~6.6 km: smooth heightfield rings from the generator's surface
  functions, shaded as the block staircase they stand for (see docs/rendering.md).

Measured on an M5 Max at 1920×1080, view distance 16 (1089 columns, ~4.7M quads loaded):
~540 fps at ground level, ~360 fps looking over the landscape from above. Column generation is
~2.5 ms, lighting ~2.5 ms and meshing ~1.2 ms per column on one core; the 33×33 column area
streams in about a second.

Profile with Tracy: `cargo run --release --features tracy` (engine work is instrumented with
`tracing` spans).

See [docs/rendering.md](docs/rendering.md) for measurements, a comparison with GPU-driven
pipeline techniques, and the rendering plan. Code adapted from other projects, and its
licences, is listed in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

## Next steps

- Draw only the face directions that can face the camera (per-direction index ranges).
- A custom GPU-driven terrain pass (see docs/rendering.md).
- Lower-detail meshes for distant chunks.
- Incremental light updates instead of relighting the affected columns.
- Entities, items, tools; the Trait/Field primitives; WASM mods.
