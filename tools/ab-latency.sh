#!/bin/sh
# ab-latency.sh STACK SECONDS: touch to present under either stack, timed
# below both compositors: a uprobe on libhybris' hwc2_compat_display_present,
# which phoc and item-compositor both call to hand hwcomposer a frame. The
# finger (touch-script.py) taps the probe (ab-probe.sh) and notes each touch
# down; the probe notes each commit; each tap's latency is the first
# present after the commit it caused (the frame goes
# on screen at the vsync after that, the same for both). The tracer is left
# as it was found. Run on the phone as root, with the tools in /tmp.
STACK=$1; T=${2:-30}
TR=/sys/kernel/tracing
LIB=/usr/lib/aarch64-linux-gnu/libhwc2.so.1.0.0
CLOCK=$(sed 's/.*\[\(.*\)\].*/\1/' $TR/trace_clock)
echo mono > $TR/trace_clock
echo > $TR/trace
echo "p:ab/present $LIB:0x15c0" >> $TR/uprobe_events
echo 1 > $TR/events/ab/present/enable
echo 1 > $TR/tracing_on
sh /tmp/ab-probe.sh $STACK tap $T > /tmp/ab-probe.out
echo 0 > $TR/tracing_on
echo 0 > $TR/events/ab/present/enable
grep " ab/present\|present:" $TR/trace | grep -v "^#" > /tmp/ab-presents.txt
echo "-:ab/present" >> $TR/uprobe_events
echo > $TR/trace
echo $CLOCK > $TR/trace_clock
python3 - <<'PY'
import re, statistics
downs = [float(l.split()[1]) for l in open("/tmp/tap.out") if l.startswith("down ")]
commits = [float(l.split()[1]) for l in open("/tmp/ab-probe.out") if l.startswith("commit ")]
pres = sorted(float(m.group(1)) for l in open("/tmp/ab-presents.txt") if (m := re.search(r"\s([\d.]+): present", l)))
# Each touch, the probe's commit it caused, and the first present after
# that commit: the frame that shows it.
lat, to_commit = [], []
for d in downs:
    c = next((c for c in commits if d < c < d + 0.2), None)
    if c is None:
        continue
    p = next((p for p in pres if p > c), None)
    if p is not None and p - d < 0.2:
        lat.append((p - d) * 1000)
        to_commit.append((c - d) * 1000)
lat.sort()
print(f"taps {len(downs)}, presents {len(pres)}, answered {len(lat)}")
if lat:
    q = lambda k: lat[min(len(lat) - 1, int(len(lat) * k))]
    print(f"touch to the probe's commit: median {statistics.median(to_commit):.1f} ms")
    print(f"touch to present: min {lat[0]:.1f}  median {statistics.median(lat):.1f}  p90 {q(0.9):.1f}  max {lat[-1]:.1f} ms")
PY
grep -c "^tap [0-9]" /tmp/ab-probe.out | sed 's/^/probe saw taps: /'
