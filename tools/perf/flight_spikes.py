#!/usr/bin/env python3
"""Slow frames in a Chrome trace of a client run, and what ran during them.

Build with tracing and fly at the player's settings, from the repository root (the client
finds assets/ relative to it); the trace lands there as trace-*.json, about 0.3-0.6 GB a
second:

    CARGO_TARGET_DIR=target/trace cargo build --release -p voxmar_client --features chrome
    VOXMAR_MUTE=1 VOXMAR_CONFIG=<dir with settings.ron> target/trace/release/voxmar \
        --capture out.png --seed 1 --size 2268x1473 --weather clear --time 0.2 --flight 30

Then, for the frames the capture marked as its flight's (a `capture_flight` span in each),
or with START_US the trace time to start from:

    python3 tools/perf/flight_spikes.py trace-*.json marked:30
    python3 tools/perf/flight_spikes.py trace-*.json START_US

(marked:SECONDS takes the last "ts" in the file less the flight's seconds as the start in a
trace from a build that marks nothing, which is late: tracing goes on through the shot and
the exit, and the flight's seconds are game time.)

(tools/perf/play_frames does both steps for a traced build, SCENES=flight.)

prints frame-time quantiles (main-thread update to update) and, for the twelve slowest
frames, the longest spans that began during them on any thread. Spans of a few
microseconds' work taking milliseconds mean the thread was preempted: time on a quiet
machine (see docs/rendering.md, "Measuring").
"""
import bisect, heapq, re, statistics, sys
path, start = sys.argv[1], sys.argv[2]


def last_ts(path):
    """The last "ts" in the file, read from its tail."""
    with open(path, 'rb') as f:
        f.seek(0, 2)
        size = f.tell()
        back = 1 << 20
        while True:
            f.seek(max(0, size - back))
            tail = f.read().decode('utf-8', 'replace')
            found = re.findall(r'"ts":\s*([0-9.]+)', tail)
            if found or back >= size:
                return float(found[-1]) if found else 0.0
            back *= 4


name_re = re.compile(r'"name":"((?:[^"\\]|\\.)*)"')
SKIP = ('schedule: ', 'main app', 'update: ', 'multithreaded executor', 'sub app', 'par_for_each', 'system_commands', 'check_conditions', 'RenderContextState')


def spans_of(path):
    """Every span that ends in the trace, as (begin, end, tid, name), streamed: a trace runs
    to gigabytes, so nothing but the open spans' stacks is kept."""
    stacks = {}
    with open(path, 'r', buffering=1 << 24) as f:
        for line in f:
            p = line.find('"ph":"')
            if p < 0:
                continue
            ph = line[p + 6]
            if ph not in 'BE':
                continue
            t = line.find('"tid":', p)
            c = line.find(',', t)
            tid = int(line[t + 6:c])
            s = line.find('"ts":', c)
            e = line.find('}', s)
            ts = float(line[s + 5:e])
            st = stacks.setdefault(tid, [])
            if ph == 'B':
                m = name_re.search(line)
                st.append((ts, m.group(1) if m else '?'))
            elif st:
                b, name = st.pop()
                yield b, ts, tid, name


# Two passes over the file: the frames (main-thread update to update), then the longest
# spans begun in the slowest of them.
updates, marks = [], []
for b, _, tid, name in spans_of(path):
    if tid == 0 and name == 'update: ':
        updates.append(b)
    elif name.startswith('capture_flight'):
        marks.append(b)
updates.sort()
if start.startswith('marked:') and marks:
    # From the frame the first mark is in, through the one after the last mark's.
    first = bisect.bisect_right(updates, min(marks)) - 1
    last = bisect.bisect_right(updates, max(marks))
    frames = updates[max(first, 0):last + 1]
    start = frames[0] if frames else 0.0
else:
    if start.startswith('marked:'):
        print('no capture_flight marks: from the trace\'s end less the flight\'s seconds', file=sys.stderr)
        start = last_ts(path) - float(start[len('marked:'):]) * 1e6
    else:
        start = float(start)
    frames = [b for b in updates if b >= start]
d = [b - a for a, b in zip(frames, frames[1:])]
if not d:
    sys.exit('no frames after the start in the trace')
med = statistics.median(d)
print(f'{len(d)} frames, median {med/1000:.2f} ms, p99 {sorted(d)[int(len(d)*0.99)]/1000:.2f}, max {max(d)/1000:.2f}')
worst = sorted(sorted(range(len(d)), key=lambda i: -d[i])[:12])
windows = [frames[i] for i in worst]
tops = {i: [] for i in worst}
for b, end, tid, name in spans_of(path):
    if b < start or name.startswith(SKIP):
        continue
    k = bisect.bisect_right(windows, b) - 1
    if k < 0:
        continue
    i = worst[k]
    if b >= frames[i + 1]:
        continue
    heapq.heappush(tops[i], (end - b, tid, name))
    if len(tops[i]) > 7:
        heapq.heappop(tops[i])
for i in worst:
    a = frames[i]
    print(f'\nframe {i} at {(a-start)/1e6:.2f}s: {d[i]/1000:.1f} ms')
    for dur, tid, name in sorted(tops[i], reverse=True):
        print(f'  {dur/1000:7.2f} ms tid {tid:3d} {name[:150]}')
