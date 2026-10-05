#!/usr/bin/env python3
"""Duration statistics of spans in a Chrome trace (see flight_spikes.py for making one).

    python3 tools/perf/span_stats.py trace-*.json START_US 'world::cull_chunks"' 'terrain::cull"'

prints, for each name fragment, how many spans begun after START_US contain it and their
median, 90th and 99th percentile, longest and total durations.
"""
import sys, statistics
path, start, names = sys.argv[1], float(sys.argv[2]), sys.argv[3:]
stacks, found = {}, {n: [] for n in names}
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
        ts = float(line[s + 5:line.find('}', s)])
        st = stacks.setdefault(tid, [])
        if ph == 'B':
            hit = None
            for n in names:
                if n in line:
                    hit = n
                    break
            st.append((ts, hit))
        elif st:
            b, hit = st.pop()
            if hit and b >= start:
                found[hit].append(ts - b)
for n, d in found.items():
    if not d:
        print(f'{n}: none'); continue
    d.sort()
    q = lambda x: d[min(int(len(d) * x), len(d) - 1)] / 1000
    print(f'{n}: {len(d)} spans, median {q(0.5):.3f} ms, p90 {q(0.9):.3f}, p99 {q(0.99):.3f}, max {d[-1]/1000:.2f}, total {sum(d)/1000:.0f} ms')
