#!/usr/bin/env python3
"""Slow frames in a Chrome trace of a client run, and what ran during them.

Build with tracing and fly at the player's settings, from the repository root (the client
finds assets/ relative to it); the trace lands there as trace-*.json, about 0.3-0.6 GB a
second:

    CARGO_TARGET_DIR=target/trace cargo build --release -p voxmar_client --features chrome
    VOXMAR_MUTE=1 VOXMAR_CONFIG=<dir with settings.ron> target/trace/release/voxmar \
        --capture out.png --seed 1 --size 2268x1473 --weather clear --time 0.2 --flight 30

Then, with START_US the trace time the flight began, or flight:SECONDS for the last "ts" in
the file less the flight's seconds:

    python3 tools/perf/flight_spikes.py trace-*.json START_US
    python3 tools/perf/flight_spikes.py trace-*.json flight:30

(tools/perf/play_frames does both steps for a traced build, SCENES=flight.)

prints frame-time quantiles (main-thread update to update) and, for the twelve slowest
frames, the longest spans that began during them on any thread. Spans of a few
microseconds' work taking milliseconds mean the thread was preempted: time on a quiet
machine (see docs/rendering.md, "Measuring").
"""
import sys, re, statistics
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


if start.startswith('flight:'):
    start = last_ts(path) - float(start[len('flight:'):]) * 1e6
else:
    start = float(start)
name_re = re.compile(r'"name":"((?:[^"\\]|\\.)*)"')
stacks = {}
frames = []          # start ts of each frame
spans = []           # (begin, dur, tid, name)
SKIP = ('schedule: ', 'main app', 'update: ', 'multithreaded executor', 'sub app', 'par_for_each', 'system_commands', 'check_conditions', 'RenderContextState')
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
            name = m.group(1) if m else '?'
            st.append((ts, name))
            if tid == 0 and name == 'update: ' and ts >= start:
                frames.append(ts)
        else:
            if not st:
                continue
            b, name = st.pop()
            if b >= start and not name.startswith(SKIP):
                spans.append((b, ts - b, tid, name))
d = [b - a for a, b in zip(frames, frames[1:])]
med = statistics.median(d)
print(f'{len(d)} frames, median {med/1000:.2f} ms, p99 {sorted(d)[int(len(d)*0.99)]/1000:.2f}, max {max(d)/1000:.2f}')
spans.sort()
import bisect
starts = [s[0] for s in spans]
worst = sorted(range(len(d)), key=lambda i: -d[i])[:12]
for i in sorted(worst):
    a, b = frames[i], frames[i + 1]
    lo, hi = bisect.bisect_left(starts, a), bisect.bisect_left(starts, b)
    top = sorted(spans[lo:hi], key=lambda s: -s[1])[:7]
    print(f'\nframe {i} at {(a-start)/1e6:.2f}s: {d[i]/1000:.1f} ms')
    for s in top:
        print(f'  {s[1]/1000:7.2f} ms tid {s[2]:3d} {s[3][:150]}')
# Which span names are most often the longest in slow frames.
