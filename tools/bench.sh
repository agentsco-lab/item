#!/bin/sh
# bench.sh SECONDS [SESSION-RUN ENV...]: item-compositor with GNOME Calculator
# on the left panel, tapped by tools/tap.py (4 taps a second) instead of a
# finger, for a benchmark that needs no one at the phone. Prints the
# compositor's per-second reports. Run on the phone as root, with tap.py and
# session-run.sh in /tmp.
T=${1:-25}; shift
python3 /tmp/tap.py $((T - 6)) 4 > /tmp/tap.out 2>&1 &
TAP=$!
for i in $(seq 1 20); do [ -s /tmp/tap.out ] && break; sleep 0.1; done
DEV=$(head -1 /tmp/tap.out)
chgrp android_input "$DEV" 2>/dev/null; chmod 660 "$DEV"
env "$@" TOUCHSCREEN=$DEV sh /tmp/session-run.sh $T --spawn gnome-calculator > /tmp/bench-session.out 2>&1
wait $TAP
sed "s/\x1b\[[0-9;]*m//g" /tmp/item-compositor.log | grep -E "pacing:|touch: |drawn" | sed "s/^[0-9T:.-]*Z *INFO item_compositor[a-z:]*: //"
