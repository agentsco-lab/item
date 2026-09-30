#!/bin/sh
# ab-latency.sh STACK SECONDS: touch to present under either stack, timed
# below both compositors: a uprobe on libhybris' hwc2_compat_display_present,
# which phoc and item-compositor both call to hand hwcomposer a frame. The
# finger (touch-script.py) taps the probe (ab-probe.sh) and notes each touch
# down; the probe notes each commit; every present within 50 ms of a
# touch is listed (the frame goes
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
# Each touch: every present within 50 ms of it. A compositor may present a
# frame of its own on the touch before the probe's commit is in (phoc does:
# two presents to each tap, the first 0-3 ms after the commit, too soon to
# hold it); the new content is in the first present that could hold it:
# under item-compositor the only one, under phoc the second.
counts, first, content, to_commit = {}, [], [], []
for d in downs:
    after = [p for p in pres if d < p < d + 0.05]
    counts[len(after)] = counts.get(len(after), 0) + 1
    c = next((c for c in commits if d < c < d + 0.2), None)
    if c is not None:
        to_commit.append((c - d) * 1000)
    if after:
        first.append((after[0] - d) * 1000)
        content.append((after[-1 if len(after) < 2 else 1] - d) * 1000)
q = lambda v, k: sorted(v)[min(len(v) - 1, int(len(v) * k))]
print(f"taps {len(downs)}, presents within 50 ms of a tap: {dict(sorted(counts.items()))}")
if to_commit:
    print(f"touch to the probe's commit: median {statistics.median(to_commit):.1f} ms")
if first:
    print(f"touch to the first present: median {statistics.median(first):.1f} ms")
    print(f"touch to the present with the content (the only, else the second): min {min(content):.1f}  median {statistics.median(content):.1f}  p90 {q(content, 0.9):.1f}  max {max(content):.1f} ms")
PY
grep -c "^tap [0-9]" /tmp/ab-probe.out | sed 's/^/probe saw taps: /'
