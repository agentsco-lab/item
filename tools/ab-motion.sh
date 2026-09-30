#!/bin/sh
# ab-motion.sh STACK: the shell's motion under either stack, timed below
# both compositors as ab-latency.sh does (a uprobe on hwc2 present): the
# virtual finger pulls the left shade down and pushes it back, opens the
# app grid on the left panel and closes it, twice each; for each gesture,
# the frames presented from its start to 0.6 s after it ends, and the gaps
# between them. STACK item runs item-compositor with nothing open; STACK
# phosh uses the running phosh session, unlocked, no windows open.
STACK=$1
TR=/sys/kernel/tracing
LIB=/usr/lib/aarch64-linux-gnu/libhwc2.so.1.0.0
STEPS="drag:600,4,1300,0.35@9 drag:600,1300,150,0.3@11 drag:600,1760,600,0.3@13 drag:600,400,1200,0.3@15 \
drag:600,4,1300,0.35@17 drag:600,1300,150,0.3@19 drag:600,1760,600,0.3@21 drag:600,400,1200,0.3@23"
NAMES="shade-down shade-up grid-up grid-down shade-down shade-up grid-up grid-down"
CLOCK=$(sed 's/.*\[\(.*\)\].*/\1/' $TR/trace_clock)
echo mono > $TR/trace_clock
echo > $TR/trace
echo "p:ab/present $LIB:0x15c0" >> $TR/uprobe_events
echo 1 > $TR/events/ab/present/enable
echo 1 > $TR/tracing_on
python3 /tmp/touch-script.py $STEPS > /tmp/tap.out 2>&1 &
TP=$!
for i in $(seq 1 20); do [ -s /tmp/tap.out ] && break; sleep 0.1; done
DEV=$(head -1 /tmp/tap.out)
chgrp android_input "$DEV"; chmod 660 "$DEV"
case $STACK in
    item) TOUCHSCREEN=$DEV sh /tmp/session-run.sh 28 > /tmp/bench-session.out 2>&1 ;;
    phosh) sleep 27 ;;
esac
kill $TP 2>/dev/null
echo 0 > $TR/tracing_on
echo 0 > $TR/events/ab/present/enable
grep "present:" $TR/trace > /tmp/ab-presents.txt
echo "-:ab/present" >> $TR/uprobe_events
echo > $TR/trace
echo $CLOCK > $TR/trace_clock
NAMES="$NAMES" python3 - <<'PY'
import os, re, statistics
names = os.environ["NAMES"].split()
drags = [tuple(map(float, l.split()[1:3])) for l in open("/tmp/tap.out") if l.startswith("drag ")]
rows = [(m.group(1), float(m.group(2))) for l in open("/tmp/ab-presents.txt") if (m := re.search(r"-(\d+)\s.*?([\d.]+): present", l))]
for name, (t0, t1) in zip(names, drags):
    inside = [(pid, t) for pid, t in rows if t0 <= t <= t1 + 0.6]
    if not inside:
        print(f"{name:10s} no frames"); continue
    pid = max(set(p for p, _ in inside), key=lambda p: sum(1 for q, _ in inside if q == p))
    ts = [t for p, t in inside if p == pid]
    gaps = [(b - a) * 1000 for a, b in zip(ts, ts[1:])]
    # frames while the finger moved, and after it (the shell's own settling)
    during = sum(1 for t in ts if t <= t1)
    long = sum(1 for g in gaps if g > 25)
    print(f"{name:10s} {len(ts):3d} frames ({during} with the finger, {1000 * (t1 - t0):.0f} ms), "
          f"gap median {statistics.median(gaps) if gaps else 0:.1f} max {max(gaps) if gaps else 0:.1f} ms, {long} gaps over 25 ms")
PY
