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

## The landing gate: `tools/gate`

Run it from the root of your worktree, after merging main and before
`git -C ~/Projects/voxmar merge --ff-only <branch>`. It takes about two minutes when the
build is warm (most of it the workspace's tests and loading the world), and runs every
heavy step through `memcap`.

1. Hygiene: `git diff main --check` and no conflict markers.
2. Builds the workspace, tests included (`--profile iter`); any warning fails.
3. Runs every test in the workspace.
4. Plays a scripted session in the real game (`voxmar --gate DIR`, crates/client/src/
   gate.rs), headless and silent, with the integrated server made to run behind
   (`--server-lag`, 60 ms more a tick, as on a busy machine), and fails on any failed
   check, crash, panic, `ERROR` line (wgpu reports validation errors as these, and Bevy
   quits on them) or `WARN` line not listed in the script's `ALLOWED_WARNINGS`.

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
| sky | looks at the morning sun, then the midnight moon, then away | a bright disc where each should be, a sky that darkens upwards, stars |
| gorge | `/locate gorge`, then goes there | the server's world has water there between walls standing six blocks over it on both sides |
| leave | quits to the title screen | the world closes cleanly |

"The server" is a second player the gate connects to the same integrated server, floating
out of the way: its copy of the world is what the server sends everyone. The shots, the
game's log and the report stay in `target/gate/`; look at them when something fails.

On a clean tree a pass is recorded against the commit (in the repository's common git
directory); `tools/gate --passed [REV]` tells whether a commit passed.

### Adding to it

A change a player can see or do needs a step in the gate, written the same way: drive the
game only through its inputs (keys, buttons, where the player looks, console commands),
and judge it by what the player would see and what the server holds, never by reaching
into the code under test. Each check gives a reason when it fails. Keep the whole session
under a minute or so: threads run it before every landing only if it is quick.

## The performance gate: `tools/bench`

For changes that can cost frame time (anything under `client/src/render`, meshing,
lighting, worldgen, streaming, per-frame client code), and after any change the landing
gate's numbers make suspicious. It builds this tree and a base (main unless given) with
`--release`, waits for a calm machine, holds `~/Projects/voxmar-wt/QUIET` while it times,
and runs each view alternately, base then this, for two rounds, at Mariano's settings
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

Mariano plays only commits that passed the gate: `tools/gate --passed <commit>` must
succeed before a play build is made, and the play binary is built fresh from that commit
in `~/Projects/voxmar-wt/play` (`cargo build --release`), never copied from another
worktree, so the binary and its `assets/` always match.

## Staying out of his way

While he plays (his voxmar game, any game run that is not a capture or gate, or
Minecraft), `memcap` holds game runs and captures until he stops, pausing any that are
running, and moves builds and tests to background priority on the efficiency cores, back
to normal when he stops. It also waits while `QUIET` exists (younger than 20 minutes);
the timing run that holds `QUIET` sets `VOXMAR_TIMING=1` for its own commands. The
version in `tools/memcap` is what `~/Projects/voxmar-wt/memcap` should be.
