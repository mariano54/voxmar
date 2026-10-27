# Quality: the landing gate, the performance gate and play builds

Many threads land on main every hour, and each ran its own checks; still, in one day these
reached Mariano's play builds:

- a crash on the first particle drawn: a pipeline missing three bindings failed wgpu
  validation, which only happens when the pipeline is first used;
- no sun, moon or stars, after views started declaring GPU preprocessing;
- a play binary from an old worktree that could not read the new `trees.ron`;
- broken blocks that came back and placed blocks that never appeared: the client showed
  a break when its own timer ran out and started on the next block, and a server running
  behind (a busy Mac) took that as cancelling the first. Unit tests covered the client and
  the server each alone, and the smoke test broke blocks on the client only;
- lag spikes while he played, mostly from other threads' builds running on every core.

Each slipped between checks that tested one side at a time, in quiet conditions. The
answer is one gate that every landing passes, which plays the real game, client and
server together, as he does, under load, and judges it by what reaches the screen.

## The landing bar (Mariano, 2026-10-13)

Speed is a feature: a landing that takes hours costs more than the bugs its ceremony
catches. The bar is:

- **A green `tools/gate` is the landing bar.** `tools/land` runs it on the merged tip, and a
  change that passes it lands. No evidence bundles, no mutation proofs, no hash-pinned
  status files, no separate review chat per phase, no lock queue in front of `tools/land`
  (the gate's own one-at-a-time rules in this file are enough).
- **One review pass, by one Sonnet session, only for a change that touches the server
  tick, saves, the network protocol or balance numbers** (anything that can corrupt a
  world, desync two players or move the economy). It reads the diff and the tests and
  answers GO or a list of blockers; findings are fixed on the branch, and the gate runs
  again. Every other change lands on the gate alone.
- A visible change adds a gate step (see below), not a screenshot bundle: the step is the
  evidence, and it runs at every landing from then on.
- **A failing check is never a flake.** A check that failed once and passed on a retry has
  a race; the fix is a wait on server ticks or a state, never a retry loop or a wider bar.
- A cloud thread pushes its branch and opens a draft PR; the Mac landing thread lands it
  after the gate. A cloud thread can run `tools/gate` itself, on software Vulkan (below),
  to hand over a branch that is already green.

## The gates: `tools/gate`

Two gates share one script. Forced checks must be quick, or they slow every thread down,
so the one every landing passes runs only what the change can reach; the full one, which
every play build passes, runs everything.

`tools/gate`, before every landing. Land with `tools/land`, from the root of your worktree
with everything committed: it fetches origin and merges origin/main and main into your
branch (main moves on GitHub too: the GPU box pushes straight to origin, so the local main
can be behind), runs `tools/gate` on the result, fast-forwards main to it, pushes main and
fetches it back to check that origin has the commit. When main moved meanwhile, here or on origin, it merges
again, up to three times; it never forces. Only its last line, `land: <commit> landed`,
means landed; anything else, the push included, failed and says why. A tree that already
passed costs nothing; otherwise a landing takes the build and the tests of the crates the
change reaches, played side by side with the session (about 6 to 10 minutes on the Mac).

What a change reaches (`crates_for` in `tools/gate`, checked by `reach_self_check`): code
reaches its crate and what depends on it; assets reach the crates that read them. Music,
sounds and shader packs are the client's alone; texture packs and fonts are texgen's and the
client's; a content file that nothing in worldgen reads (quests, recipes, skills, combat,
lang: anything worldgen names in its sources, the blocks and the creatures are the rest)
skips worldgen's tests, which are most of the suite's time. Add a file worldgen starts to
read by naming it in worldgen's sources, and the gate follows.

On a machine with no display (a cloud container) the gate builds and runs the tests, says so,
and records no pass: the Mac's gate is the landing bar. `VOXMAR_GATE_SOFTWARE=1` also plays
the session there on a virtual display with Mesa's software Vulkan (`apt install xvfb
mesa-vulkan-drivers`), as a look: frames take half a second, so the steps that time the game
(natural births, perfect block, dodge) fail on speed and the run stops at the first.

1. Hygiene: `git diff main --check`, no conflict markers, and no new wall-clock wait in a
   test (`tools/test-waits`). A test that waits until `Instant::now()` passes a deadline,
   or ends a `pump_until` by the clock, passes on a quick machine and fails on a loaded one
   (main once went red from an 800 ms wait racing the once-a-second quest check): wait for
   what the server says (`settle`, a message, its ticks) and space a player's actions with
   arrival stamps (`propose_at`). Only watchdogs that can never let a check pass
   (`SERVER_WAIT`, `SERVER_SILENT`, `assert!(Instant::now() < ...)`) and the timing tests
   that sleep to stand for a player's real time are allowed, by name, in the script.
2. An incremental build of the workspace, tests included (`--profile iter`); any warning
   fails.
3. The tests of the crates the change can reach, worked out from the files it touches
   against main (a client change runs the client's; an engine change or any asset
   everything's, for the engine's and texgen's tests read assets too),
   less the few slow ones listed in the script's `SLOW_TESTS`, and meanwhile
4. a scripted session in the real game (`voxmar --gate DIR`, crates/client/src/gate.rs) in
   a small world, headless and silent, with the integrated server made to run behind as
   on a busy machine (`gate::SERVER_LAG`: 60 ms more a tick, a 400 ms stall every ninth
   with messages waiting, and a 600 ms stall as every other action is proposed) in the
   steps that play against it: placing, digging, breaking and the fight's guard and dodge
   (`Gate::lagging`: the fight lags from the bear's coming through its tries, not its summons
   and sweep, and provokes no blow until `FEEL_LAG_SETTLE` pings have been answered under the
   lag, a lagging server pinging every tick, so the round trip the server judges guards by has
   settled before the block is searched); the
   rest run at full speed, three times sooner. It also
   fails when placing or digging hitches (a frame over 150 ms, or six times the median: the
   first break once compiled three pipelines on the spot); on a machine asked for more
   than all its cores (the load now over the core count: the one-minute average, or the
   load a rising average comes from, `Gate::load_now`, since the average lags a gate's
   start by most of a minute) the bar rises by how far over it is, since the machine is rarely calm enough to wait for, and the pipelines check catches
   compiles at any load. A check a session cannot judge is reported as SKIPPED, and is not
   a pass: the gate waits for the load to fall under the core count (up to 20 minutes),
   plays the session again on its own and judges that run (the first run's log and shots
   kept in `game-skipped.txt` and `first-run/`), and fails if a check is skipped again.
   The gorge, a fresh area to generate, waits up to four minutes on an overloaded machine
   instead of one, and fails, saying what was missing, rather than being skipped.

   Steps judge what the server does by its own clock (`ServerHandle::ticks`), not the
   wall's or the game's: a server running behind takes longer, not too long. The tests run
   at the lowest priority (`nice 19`), below the session, so they cannot starve its server
   or its frames; once they did, and nine checks failed together, and at `nice 10` a loaded
   Mac still drew the fight-feel step's frames past the bear's wind-up. A step that fails lets go of every key and
   button, so it cannot fail the steps after it. The session must never kill its player:
every loss of health is logged with where and among what, and a death fails `alive` at
once, naming the step (the workshop kills the player on purpose, and the text-fit steps show the death screen), and the
player respawns, for a dead player fails every step after (nine checks at a time). `VOXMAR_GATE_STALL=1` (with
   `--server-lag 150`) stalls every frame and slows the server as a machine loaded far
   past its cores does, to show a step passes only on an idle one. It fails on any failed
   check, crash, panic, `ERROR` line (wgpu reports validation errors as these, and Bevy
   quits on them) or `WARN` line not listed in the script's `ALLOWED_WARNINGS`.

A change that touches only docs or tools skips 2, 3 and 4. A tree that already passed is not
gated again, so merging or rebasing without changing anything costs nothing.

`tools/gate --full`, before every play build: every test and the session at the normal
view distance, with the workshop's longer machine phases (`VOXMAR_GATE_FULL=1`), a few minutes. A scheduled full run on main also reports any failure to
the coordinator, so a miss of the quick gate is caught within hours, and always before
Mariano plays.

The session builds its own test ground high above spawn with commands, so terrain
changes don't move it, and checks:

| step | what the player does | what must follow |
| --- | --- | --- |
| load | | the world around spawn loads and is meshed, and the weather is ready (`weather::WeatherReady`), showing the server's report from its first frame (no change from the menus' sky into it) |
| arrive | clicks into the game | the mouse is grabbed |
| place | three right clicks on the ground | the planks are on the client and the server, the two agree on every block around, the renderer has every column as it now is, the middle of the screen changed, the hand holds three fewer |
| dig | holds the left button over the ground | a hole three blocks deep, with the same four checks |
| drops | walks into the hole | every dug block's drop is picked up |
| breaks | holds the left button along a row of glass, dirt, stone, snow, gravel and cobblestone, with the bare hand, the starting planks (throwing one with Q halfway) and every item tagged `tool`, while busy threads contend for the cores and the game's own frames stall 80 to 320 ms every fifth frame or so | every block ends broken on the client and the server, none comes back, the two agree on every block around. The server judges breaks by when the player's messages arrived, so a server held up decides as one that kept up (crates/server/src/actions.rs) |
| workshop | crafts sticks and a crafting table in the two-by-two grid, places the table and right clicks it, crafts a stone pickaxe in its three-by-three grid, clicks an iron ore at Mining 1, then at 15 holds the button on it, right clicks dirt with a hoe, is given three loaves, casts a rod into the pool, fires a boiler with coal and puts copper ore in the steam crusher beside it; in the full gate only, then an MV ore washer on a turbine and transformer, a Bellows Bloomery worked by a kept aurochs on a treadwheel with a hopper line into a chest, and a Coke Oven and Blast Furnace | sticks and a table come back with Engineering experience; the table is on both sides and its grid shows nine cells; the ore refuses with "Needs Mining 15", then breaks on both sides with Mining experience; the dirt turns to farmland on both sides; three loaves given take three slots; holding right click with a rod over the pool catches a fish; the crusher gives two dust for the ore; (full) the dry washer says it needs water and gives three dust once wet, the bloomery gives an iron bloom and the chest gets four dust, the oven says it is making coke and the furnace iron (shot `iron_age`) |
| swim | holds forward and jump in a pool against a step | climbs out onto the step; the server has the player where the client does |
| step up | holds forward, never jump, along a slab, a stair, two whole blocks and a slab on one, then a whole block a level up, each picked from the loaded blocks by its boxes | reaches the top never inside a block's boxes, never moving upwards, never put back; the server has the player on the top within 0.25 |
| menu, inventory | Esc, Esc, E, E | each opens, is drawn and closes, giving the mouse back |
| music | Esc, clicks the audio tab, clicks the first category of music's switch, then clicks it again, Esc; then M, M (the gate's music is picked and started though silent, `music::Rehearse`) | the audio page shows one switch for each category in `music.ron` the player can hear (Voxmar's own; Minecraft's too with Minecraft assets on), all on; the first click turns that category off in the settings and its switch reads Off, the second turns it back on; after M turns music back on a track starts and the top right reads "Now playing: <title> (<category>)", with one of the switches' names, and the line fades away by itself |
| overlay | F3, then F3 again | the debug overlay opens and closes, and PostUpdate (UI layout among it) takes no longer with it open than closed: within 0.5 ms in the median |
| creatures | summons a lumen snail, mouflon, a rabbit, a shambler and a goldfinch on a walled pad, and releases a cabbage white butterfly over it | they reach the client and draw (shot `creatures`), the shambler's front eyes painted; the goldfinch, a small bird, reaches the client like any creature; the butterfly, the client's own, is drawn as two wing quads; the voices load and the creature pointed at shows its level |
| look | on the creatures' pad, looks at a plank block beside them, then at a lumen snail seven blocks off, then at the snail with a stone wall put between | the top-left label names the planks as the language does (shot `look-block`); names the snail with its level from further than a blow or a tending reaches (shot `look-creature`); with the wall between, it no longer names the snail, and names the wall |
| weather | `/weather storm`, `/weather snow`, `/weather clear` | each arrives and changes the view (rain, snow, their particles), and out in it the player gets a weather effect (soaked, snowblown…); from joining to here, no weather fraction shown changed in a frame by more than easing allows (`weather::Easing::fastest`: a full swing over six seconds at the quickest, after a command) |
| depths | `/weather fog` over the test ground, then down a shaft eleven deep and open to the sky; then, in a sealed room deep in stone, mines its wall just until its cracks show (letting go at once, or after 3 s without them), sets a glowstone beside it and mines the same block again | the air has ground fog or the weather's fog on the open ground, and neither at the foot of the shaft (its sky light is full, so only the share of sky in view tells it from open land); the cracks in the dark room are lit by nothing (sky 0, block 0) and their light edges are gone, with the glowstone beside them the face has block light 10 or more and the edges show in full; shots `mist-open`, `mist-shaft`, `crack-dark`, `crack-lit` to look at |
| sky | looks at the morning sun, flies above the world's top, looks towards and away from the setting sun, then at the midnight moon, then away | a bright disc where each should be, a sky that darkens upwards, a lit sky from above the world, a sunset sky warmer towards the sun than away, stars |
| tree | at noon, a small tree with an apple among its leaves is filled in | it reaches the client and draws, leaves, leaf cards and fruit, changing the view |
| prayers | C, the Prayers button, a click on the Bark Skin row, then shuts the screen and presses the first quick key | the book has a row for each prayer in the content; Bark Skin is on as the server says; the strip over the hotbar names it; the quick key turns it off again |
| held | looks up at the open sky and takes an empty hand, an iron sword, a stone block and the empty hand again in turn, a shot each; then seals itself in a stone box and shoots the sword, and the empty hand, again | the sword and the block each change the lower right of the view by at least 1 level (and differ from each other by as much), and the sky by under 0.5 beyond its own drift between the two empty-hand shots (each held shot judged against the nearer of them); in the dark box the sword changes the lower right by under half what it did under the sky (the hand takes the light where the player stands) |
| cook | a campfire on the pad, raw rabbit used on it, then counts the fire's particles for 1.5 s | cooked (or burnt) food comes back two seconds later, within 15 server seconds; at least 4 particles (smoke, embers, sparks) rise from the fire, a count that holds at any frame rate |
| kiln | a pit kiln inside the creatures pad, twelve clay in hand, the kiln right clicked | the server sends back the firing kiln, and the fired one twelve server seconds on (within 15), seen drawn; right clicks then take the twelve fired clay into hand and leave the empty kiln (within 10); the kiln and the clay go after |
| quern | quern stones inside the creatures pad, on its stone floor, a wheat in hand, the quern right clicked until flour comes | a flour comes back and the wheat is gone; the quern goes after, leaving the pad's stone floor and air as they were |
| home | a sleeping bag and a torch beside them, slept in | the Camp chip comes on (the bed is their home and has a light), the chat names it with what to add next, the sleep gives a rest (Rested); the blocks and the rest go after |
| party | `/wander start tinker_camp` ahead of the pad, then goes to its trader, right clicks it with nothing to trade, again with four hides in hand, and `/wander end` | the trader and a hound reach the client and draw within 20 server seconds; the click is answered in chat by one of the camp's greetings; the hides are taken and loaves come back with the trader's swap line (the trade read from the hand, through the real inspect path); the party is gone from the client within 10 s of the end. The first run on the Mac is this step's first run anywhere (the cloud has no GPU) |
| ore | `/locate ore`, then goes there and looks down | the server's world has a sample of ore there on solid ground, with ore of the same kind below it within seven blocks across (its vein), the client has the sample, and no more than one in thirty of the ore blocks around lies alone (no other ore within two blocks) |
| gorge | `/locate gorge`, then goes there | the server's world has water there between walls standing six blocks over it on both sides, within eight blocks |
| tarns | finds a visible pool filled by the tarn rule near the highlands, then visits it on both peers | at least twenty connected source-water or ice surface columns match on the client and observer; an ordinary high lake cannot replace a missing tarn (shot `tarn`) |
| quests | crafts ten twine to finish Twine and Thong, opens the quest book (L), holds W and Esc with it open, pins the torch quest and crafts torches for it, pins the finished Crafting Table quest, then opens the book on Twine and Thong and clicks its Claim button | the book opens on a quest; W does not walk with it open and walks with it shut; the tracker counts torches as the server does and reads "Done! Claim it." for a finished quest that pays, then drops it after `done_stays`; the finished quest is waiting to be claimed (a Claim button shows), and after the click the server says it is claimed and its Crafting experience came (shots `quests-en`, `quests-ja`, and `quests-claim`: the book filling most of the window, the picked quest's page with its Claim button and rewards in the sidebar at the right) |
| meanwhile | marks A Pit Kiln done, gives two charcoal and pins Charcoal, then opens the book on Charcoal and presses its "While you wait" line | the tracker reads "While you wait: A Place to Pray" under Charcoal begun; the book's page for Charcoal shows the line, and pressed it turns the book to A Place to Pray |
| pipelines | everything from the world loading to leaving it | no pipeline compiles meanwhile: on Metal each is a hitch in the frame it is first needed, a few hundred ms on a fresh build. What shows only now and then is warmed at load (`render::warm`: other players, creatures, breaking, the menu blur); something new drawn on first use fails this until it is warmed too |
| leave | quits to the title screen | the world closes cleanly |

"The server" is a second player the gate connects to the same integrated server, floating
out of the way: its copy of the world is what the server sends everyone. The shots, the
game's log and the report stay in `target/gate/`; look at them when something fails.

`tools/gate stop` stops this worktree's run and what it started, and nobody else's;
never stop runs by pattern (`pkill -f tools/gate`), which stops every thread's.

On a clean tree a pass is recorded against the tree (in the repository's common git
directory); `tools/gate --passed [--full] [REV]` tells whether a commit passed (the full
gate).

### Adding to it

A change a player can see or do needs a step in the gate, written the same way: drive the
game only through its inputs (keys, buttons, where the player looks, console commands),
and judge it by what the player would see and what the server holds, never by reaching
into the code under test. Each check gives a reason when it fails. Keep the session quick
(about half a minute now): poll for what a step waits on rather than sleeping, and never
add a fixed wait of more than a second.

The server runs behind on purpose (`SERVER_LAG`, in the steps `Step::lagged` names, as `Gate::lagging` narrows them; a new
step that times actions against a busy server joins them), and the machine is usually
loaded, so:

- Wait on the server by its own clock, `gate.server_now` and `gate.server_in_phase()`
  (its ticks, `ServerHandle::ticks`), never by the game's seconds: anything the server
  does once a second (effects, quests, vitals, cooking) takes longer on a busy machine.
  Fail on no progress for a while by that clock, not on a total. Give up by it too: a
  wait on anything the server does (an item, a block, a reply, an effect) gives up by
  `gate.server_in_phase()`, and each step's own limit (`guard`) is in the server's
  seconds. Given up by the game's seconds, the cook step failed on a stalled server
  before the server had run the ticks it waited for. Only waits on the client's own work
  (drawing, shots, screens, movement it predicts) count the game's seconds. A server
  that stops is caught all the same: the session fails "the server stopped" once its
  ticks stand still for `SERVER_SILENT` (60 s) of the wall clock, and on `WALL_LIMIT`
  (15 min) of it in all, with frames longer than 10 s (memcap stopping the run while
  Mariano plays) counted as 10 s. In the full gate the session's limits (`TIME_LIMIT`,
  `WALL_LIMIT`) grow by the seconds its full-only workshop phases take
  (`FULL_ONLY_PHASES` in gate.rs, 600 s for the four machine phases now).
- Propose actions (clicks that ask the server for something) at most every few server
  seconds while waiting, or once and then wait: `SERVER_LAG` stalls 600 ms on every other
  proposal, so a step proposing every 30 frames starves the server of ticks and the
  thing it waits for never comes (the kiln check once hung this way).
- Judge an event by its state, not by catching a single frame: a check that needed the
  one frame the player was on the ground missed it under load, and the player hopped off
  the test ground and died.
- Let go of every key and button a step holds before it ends, pass or fail, and leave the
  player standing on the test ground; the health watch fails the session if the player
  dies (the workshop's own `/kill` sets `killed_on_purpose`).
- `VOXMAR_GATE_STALL=1` with `--server-lag 150` runs the session as a machine loaded far
  past its cores would: a new step should pass that way too.

## Fast tests

Fast tests are a standing rule: the suites run on every landing and every edit, so a slow
test costs the whole project. Nothing is made fast by weakening it: no test is skipped,
shrunk below what proves its claim or stripped of its negative side to save time.

`tools/test-budget.toml` holds the numbers and `tools/test-times DIR` checks them against a
run's output (`DIR/test-*.txt`, the test binaries run with `-Z unstable-options
--report-time`, which needs `RUSTC_BOOTSTRAP=1` off nightly): a test may take
`per_test_seconds` (5 now, measured while the gate runs every binary side by side, times how
much busier than idle the machine is), the slowest test binary `suite_seconds` (`--full`:
`full_suite_seconds`, with the slow tests the default gate skips), and a test
over its budget is listed in `[[allow]]` with its own `seconds` and the reason. It prints the
slowest tests every time. It fails on a test over its budget and on a binary over the suite
budget; with `--strict`, run on the Linux box the budgets were set on, also on an allowance a
test no longer needs (it ran back under the default), so the list only shrinks as tests get
cheaper (a faster machine would find many unneeded, so the gate only notes them). The scale is the machine's slowdown against the Linux box the budgets were set on (`tools/gate`: 5 on the Mac, whose gate ran the tests at nice 10 and utility QoS beside the session when measured, mostly on its efficiency cores, 3 to 8 times slower on a calm day (nice 19 now, without the QoS); `VOXMAR_TEST_SLOWDOWN` for another machine); past the core count, the load averaged while the tests ran, the gate lists the times and does not judge them (what else the machine runs must never block a landing; a calm machine holds the budgets). A new test over the default is made cheaper first;
an entry is the last resort, with a reason a reader can check.

How the tests are kept cheap:

- Time in a test is work or waiting. Waiting is judged on the server's ticks, never on the
  clock (no `sleep`, no wall-clock asserts; poll with `after_ticks` and `pump_until`). The
  gameplay tests start their servers with `ServerConfig.tick_speedup` (20), so a wait of
  twenty ticks takes a tick's worth of time; the hello limit and every tick-judged system
  scale with it. What the server judges by the wall clock (combat, timed actions, the
  movement hover grace, idle and chat limits: arrival stamps, `Connection::send_at`) cannot
  speed up, and a test of one calls `real_time("why")` first to get a normal server. Such a
  test is slow by nature until the server reads a clock the test can drive; each one is in
  the allow-list under that reason.
- The gameplay tests share one generated world per seed (`keep_world`), because planning the
  spawn village costs more than a second the first time a world is asked where spawn is.
  The settlements tests do the same for the tests that only read a world (`shared_world`);
  a test that counts what a world plans, or compares a fresh world with another, builds its
  own (`base_world`).
- A loop that asks the generator for the same answer on every block asks once per column
  and keeps it (`decorations_rest_on_the_ground` asked `height` 440 000 times).
- The generator's own cost (a column 13 to 18 ms, `site()` 0.15 to 0.5 ms, a village a
  second of planning) is most of what is left in the worldgen suite: that is where to win more.

## The FPS benchmark: `tools/bench fps`

For optimising frame rate, and for anyone (Mariano included) who wants to know where it
stands: one command, silent, through memcap and its own quiet window.

```
tools/bench fps [--repeats N] [--only flight75,dig]
tools/bench history [SCENE]
```

It builds this tree with `--release` and runs six fixed scenes, seed 1 at Mariano's
settings (32 chunks, SMAA, VSync off, 2268×1473: 75% of his 3024×1964 screen), each N
times (default 3), reporting the median of the runs: `flight75` (a 40 s flight), `flightfast` (the same at his 400% fly
speed, the world streaming in faster than it is made), `flighthigh` (the same at y=260 looking
down at 40°, the whole view of about 36k chunks in sight, where his slow frames gathered),
`forest`, `mountains`,
`night_rain` (still views measured for 13 s), and `dig` (15 s digging a shaft, holding the
button: breaking, particles, drops, remeshing). For each it prints the average FPS, the 1%
low (the mean of the slowest 1% of frames, as FPS), the p99 and worst frame in ms (by the
wall clock), and the spread of the runs' FPS around their mean, which says how far to
trust a difference. Every result goes to `~/Projects/voxmar-wt/bench-history.jsonl` with
the commit, the machine (platform, and on a Mac the power mode and AC), so `tools/bench
history` shows each scene over time. Compare only rows of the same machine and power mode.

Where benchmarks run (Mariano, 2026-10-05): until the Linux GPU box is set up, on his Mac
whatever else it is doing: by default the benchmark holds no quiet window, so nobody's
work pauses, records the load in every history row, and judges by the median of its runs
(`--quiet` holds a window when an idle machine matters). Once the box is up, all benchmarks run there and its rows are the reference
baseline; Mac rows stay only for absolute sign-off on his own hardware.

Frame times come from captures (`--flight`, `--dig`, and for still views the last
seconds before the shot), which now log `N frames, median, p99, max, …, mean (fps), 1% low`.



For changes that can cost frame time (anything under `client/src/render`, meshing,
lighting, worldgen, streaming, per-frame client code), and after any change the landing
gate's numbers make suspicious. It builds this tree and a base (main unless given) with
`--release`, then holds a quiet window (see below) while it runs each view alternately,
base then this, for two rounds, at Mariano's settings
(32 chunks, SMAA, VSync off; 75% and full resolution of his 3024×1964 screen). It judges
ratios, never absolute numbers, as the graphics thread does: a flight fails if its p99
frame is over 1.5× the base's in every round or any frame takes over 25 ms on AC power;
a static view fails if over 8% slower in every round. The power mode (`pmset`), the load
and every number go to `target/bench/report.txt`. It takes about a quarter of an hour;
`--only flight75 --rounds 1` is a quick look.

Quiet AC numbers on 2026-10-05, at his settings: flight median 3.0–3.5 ms, p99 8.5–9.3 ms,
worst 13–18 ms. With other threads building (load around 50): p99 37–46 ms, worst
63–143 ms, which is what he felt as lag spikes.

## Play builds

Mariano plays only commits that passed the full gate: `tools/gate --passed --full
<commit>` must succeed before a play build is made (run `tools/gate --full` on it first),
and the play binary is built fresh from that commit in `~/Projects/voxmar-wt/play`
(`cargo build --release`), never copied from another worktree, so the binary and its
`assets/` always match. Build it with `VOXMAR_FOREGROUND=1` before memcap: he is waiting
for it, and at memcap's background priority the last crate alone once took half an hour.

## Disk: `tools/tidy`

Every worktree keeps its own target dir (cargo trusts the first tree's fingerprints, so a
shared one silently reuses another tree's binaries), and a built one holds about 30 GB:
on 2026-10-20 the voxmar worktrees held over 900 GB, nearly all of it build output. Most
of a busy tree's is rustc's incremental cache for crate versions that have since changed
(15 of the landing tree's 25 GB had not been touched for a day).

`tools/tidy` frees it, every hour on Mariano's Mac (`tools/tidy --install`, a launchd
agent running the copy on main; `--dry-run` says what it would do and changes nothing). It
looks only at the trees directly in `~/Projects/voxmar-wt`. A tree goes, its branch kept,
once it is clean (nothing modified, untracked, or ignored but precious such as saves,
worlds or `.env`), every worktree inside it (bench baselines) is clean and on origin too,
and its commit has landed on origin/main and it has been idle for 6 hours, or is on another
origin branch and idle for 7 days; unpushed work never goes. Otherwise its target dir goes
after 2 days without a build, a profile dir (`target/debug`, ...) the same, and an
incremental crate cache after a day untouched.

It never touches `play` or `landing` at all, build output included, nor the checkout of
main, locked worktrees (`git worktree lock` keeps one), or a tree git fails in. Right
before every delete it checks again that PLAYING and QUIET are down and that no running
process works in the tree or names it, and when it cannot tell, it keeps the tree; git's
refusals are logged, never forced. Every removal is logged, with its size, to
`~/Projects/voxmar-wt/tidy.log`. `tools/tidy --self-check` runs it against scratch repos
covering each of those cases; the gate runs it whenever `tools/tidy` changes, since what
lands on main runs on the Mac within the hour.

## Staying out of his way, and quiet windows

Every heavy command goes through `memcap`, which gets out of the way. The copy every thread
runs is `~/Projects/voxmar-wt/memcap`; install a new version only by copying `tools/memcap`
next to it and renaming it over the old one (`cp tools/memcap ~/Projects/voxmar-wt/memcap.new
&& mv -f ~/Projects/voxmar-wt/memcap.new ~/Projects/voxmar-wt/memcap`). Bash reads a running
script from its file as it goes, so writing over the file in place breaks every copy that is
running: on 2026-10-06 that turned the play binary and a thread's build into six-byte files.


- When Mariano says he will play, the coordinator writes `~/Projects/voxmar-wt/PLAYING`.
  While it exists nothing new starts and whatever is running pauses, builds, tests and
  game runs alike, until it is removed when he is done. (Our builds on every core gave
  him 40–140 ms frames and under 60 fps while flying.) He wants work to go on otherwise,
  game open or not, so his game alone holds nothing. memcap removes the flag a minute
  after his game (voxmar not as a capture or gate, or Minecraft) exits, so everything
  waiting starts as soon as he quits; if his game never appears, the flag holds two hours
  after it was written.
- Everything memcap runs a step below his game: on a Mac, utility QoS, which may use the
  performance cores his game leaves free (6 of the 18); elsewhere `nice 10`.
  `VOXMAR_QOS=background` keeps a job to the efficiency cores (`nice 19`). That was the
  default until 2026-10-13: at his look-down pose a full build alongside gave a 326 ms
  worst frame at normal priority and 57 ms at background QoS (p99 43 against 27 ms), but a
  landing's build took 25 minutes on the efficiency cores alone, and the clamp bought
  nothing while he is not playing, since PLAYING stops every job and is written only when
  he plays. Timing runs (`VOXMAR_TIMING=1`) and the benchmark's timed runs
  (`VOXMAR_FOREGROUND=1`) run as his game does, and a timing run takes no slot. So does
  the gate's played session, whose checks time the game's frames: its tests beside it run
  at `nice 19` (`tools/gate`), with no QoS clamp, which kept them to the efficiency cores
  (the worldgen tests alone 115 s there against 33 s without it, T7b).
- Everything memcap runs is muted (`VOXMAR_MUTE=1`): no run of ours ever plays a sound.
- While `~/Projects/voxmar-wt/QUIET` exists (and is younger than 20 minutes), nothing new
  starts and running builds pause; tests and game runs, which finish within a minute or
  two, finish. A paused command leaves `/tmp/voxmar-heavy-N.lock.paused`.

So a quiet window needs no announcement: `tools/bench --quiet` waits until his game is closed
(numbers taken while it runs would be wrong, and it stops if he starts one), claims
`QUIET` (waiting only for another timing run's), waits for the running tests and game
runs to finish and up to a minute for the machine to idle (under three cores busy, three
samples in a row; past that it times anyway and records the load), and keeps the flag
fresh until it is done.
It also fails when it cannot compute a ratio, and reports INCONCLUSIVE (exit 3) when the
base's own rounds disagree by more than 1.5× (flights) or 15% (static views). A thread
timing something by hand does the same: write `QUIET`, run its own commands with
`VOXMAR_TIMING=1`, and delete the flag when done.

### HV and treasure evidence

The full gate also builds a standing Turbine Hall with its boiler above, HV cable, MV
transformer and HV bank, a cased Processing Array, and an Induction Smelter. It supplies
fuel and ore through player actions and requires the hall's generating reply and both
processors' making replies with power or progress above zero. Each reply names the
machine being checked. A copper pick opens a Bronze treasure chest, consumes exactly
one pick, and reveals a non-empty saved chest whose finds are then taken into inventory.

`hv-hall`, `hv-array`, `hv-smelter`, `hv-power`, `treasure-row` and `treasure-open` shots
are captured in matching poses with `-en` and `-ja` suffixes. All four picks appear in
the hotbar and their translated selected-item names appear across the factory shots.
The default pack is the baseline; Voxmar HD falls back to its HV machine textures
until that pack provides them. The later reviewed art phases update these same shots.
