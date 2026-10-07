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
means landed; anything else, the push included, failed and says why. On a typical change
it takes well under a minute.

1. Hygiene: `git diff main --check` and no conflict markers.
2. An incremental build of the workspace, tests included (`--profile iter`); any warning
   fails.
3. The tests of the crates the change can reach, worked out from the files it touches
   against main (a client change runs the client's; an engine change everything's),
   less the few slow ones listed in the script's `SLOW_TESTS`, and meanwhile
4. a scripted session in the real game (`voxmar --gate DIR`, crates/client/src/gate.rs) in
   a small world, headless and silent, with the integrated server made to run behind as
   on a busy machine (`gate::SERVER_LAG`: 60 ms more a tick, a 400 ms stall every ninth
   with messages waiting, and a 600 ms stall as every other action is proposed). It also
   fails when placing or digging hitches (a frame over 150 ms, or six times the median: the
   first break once compiled three pipelines on the spot); on a machine asked for more
   than all its cores (load average over the core count) the bar rises by how far over it
   is, since the machine is rarely calm enough to wait for, and the pipelines check catches
   compiles at any load. A check a session cannot judge is reported as SKIPPED, and is not
   a pass: the gate waits for the load to fall under the core count (up to 20 minutes),
   plays the session again on its own and judges that run (the first run's log and shots
   kept in `game-skipped.txt` and `first-run/`), and fails if a check is skipped again.
   The gorge, a fresh area to generate, waits up to four minutes on an overloaded machine
   instead of one, and fails, saying what was missing, rather than being skipped.

   Steps judge what the server does by its own clock (`ServerHandle::ticks`), not the
   wall's or the game's: a server running behind takes longer, not too long. The tests run
   a step below the session (`nice 10`), so they cannot starve its server; once they
   did, and nine checks failed together. A step that fails lets go of every key and
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
view distance, a few minutes. A scheduled full run on main also reports any failure to
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
| workshop | crafts sticks and a crafting table in the two-by-two grid, places the table and right clicks it, crafts a stone pickaxe in its three-by-three grid, clicks an iron ore at Mining 1, then at 15 holds the button on it, right clicks dirt with a hoe, is given three loaves, casts a rod into the pool | sticks and a table come back with Engineering experience; the table is on both sides and its grid shows nine cells; the ore refuses with "Needs Mining 15", then breaks on both sides with Mining experience; the dirt turns to farmland on both sides; three loaves given take three slots; holding right click with a rod over the pool catches a fish |
| swim | holds forward and jump in a pool against a step | climbs out onto the step; the server has the player where the client does |
| menu, inventory | Esc, Esc, E, E | each opens, is drawn and closes, giving the mouse back |
| overlay | F3, then F3 again | the debug overlay opens and closes, and PostUpdate (UI layout among it) takes no longer with it open than closed: within 0.5 ms in the median |
| weather | `/weather storm`, `/weather snow`, `/weather clear` | each arrives and changes the view (rain, snow, their particles), and out in it the player gets a weather effect (soaked, snowblown…); from joining to here, no weather fraction shown changed in a frame by more than easing allows (`weather::Easing::fastest`: a full swing over six seconds at the quickest, after a command) |
| sky | looks at the morning sun, flies above the world's top, looks towards and away from the setting sun, then at the midnight moon, then away | a bright disc where each should be, a sky that darkens upwards, a lit sky from above the world, a sunset sky warmer towards the sun than away, stars |
| tree | at noon, a small tree with an apple among its leaves is filled in | it reaches the client and draws, leaves, leaf cards and fruit, changing the view |
| cook | a campfire on the pad, raw rabbit used on it, then counts the fire's particles for 1.5 s | cooked (or burnt) food comes back; at least 4 particles (smoke, embers, sparks) rise from the fire, a count that holds at any frame rate |
| ore | `/locate ore`, then goes there and looks down | the server's world has a sample of ore there on solid ground, with ore of the same kind below it within seven blocks across (its vein), the client has the sample, and no more than one in thirty of the ore blocks around lies alone (no other ore within two blocks) |
| gorge | `/locate gorge`, then goes there | the server's world has water there between walls standing six blocks over it on both sides, within eight blocks |
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

The server runs behind on purpose (`SERVER_LAG`), and the machine is usually loaded, so:

- Wait on the server by its own clock, `gate.server_now` and `gate.server_in_phase()`
  (its ticks, `ServerHandle::ticks`), never by the game's seconds: anything the server
  does once a second (effects, quests, vitals, cooking) takes longer on a busy machine.
  Fail on no progress for a while by that clock, not on a total. Give up by it too: a
  wait on anything the server does (an item, a block, a reply, an effect) gives up by
  `gate.server_in_phase()`, and each step's own limit (`guard`) is in the server's
  seconds. Given up by the game's seconds, the cook step failed on a stalled server
  before the server had run the ticks it waited for. Only waits on the client's own work
  (drawing, shots, screens, movement it predicts) count the game's seconds.
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
- Everything memcap runs is at background priority: on a Mac, background QoS, which keeps
  it to the efficiency cores (6 of the 18 are performance cores); elsewhere `nice 19`. At
  his look-down pose, a full build alongside gave a 326 ms worst frame at normal priority
  and 57 ms at background QoS (p99 43 against 27 ms), and the build still went at about
  nine tenths of its speed. Timing runs (`VOXMAR_TIMING=1`) and the benchmark's timed runs
  (`VOXMAR_FOREGROUND=1`) run as his game does, and a timing run takes no slot.
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
