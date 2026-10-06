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
   than all its cores (load average over the core count), a long frame says nothing about
   the game, so that check is reported as SKIPPED instead. It fails on any failed
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
| load | | the world around spawn loads and is meshed |
| arrive | clicks into the game | the mouse is grabbed |
| place | three right clicks on the ground | the planks are on the client and the server, the two agree on every block around, the renderer has every column as it now is, the middle of the screen changed, the hand holds three fewer |
| dig | holds the left button over the ground | a hole three blocks deep, with the same four checks |
| drops | walks into the hole | every dug block's drop is picked up |
| swim | holds forward and jump in a pool against a step | climbs out onto the step; the server has the player where the client does |
| menu, inventory | Esc, Esc, E, E | each opens, is drawn and closes, giving the mouse back |
| weather | `/weather storm`, `/weather snow` | each arrives and changes the view (rain, snow, their particles) |
| sky | looks at the morning sun, flies above the world's top, looks towards and away from the setting sun, then at the midnight moon, then away | a bright disc where each should be, a sky that darkens upwards, a lit sky from above the world, a sunset sky warmer towards the sun than away, stars |
| tree | at noon, a small tree with an apple among its leaves is filled in | it reaches the client and draws, leaves, leaf cards and fruit, changing the view |
| cook | a campfire on the pad, raw rabbit used on it, then looks at the air above | cooked (or burnt) food comes back; the fire's smoke and embers move the view over it |
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
speed, the world streaming in faster than it is made), `forest`, `mountains`,
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
`assets/` always match.

## Staying out of his way, and quiet windows

Every heavy command goes through `memcap`, which gets out of the way:

- When Mariano says he will play, the coordinator writes `~/Projects/voxmar-wt/PLAYING`.
  While it exists nothing new starts and whatever is running pauses, builds, tests and
  game runs alike, until it is removed when he is done. (Our builds on every core gave
  him 40–140 ms frames and under 60 fps while flying.) He wants work to go on otherwise,
  game open or not, so his game alone holds nothing. So that a forgotten flag cannot stall
  everything, memcap ignores it 30 minutes after his game (voxmar not as a capture or
  gate, or Minecraft) was last seen running, or two hours after it was written if his
  game never appeared.
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
