#!/bin/sh
# bench.sh SECONDS [SESSION-RUN ENV...]: item-compositor with GNOME Calculator
# on the left panel and GNOME Clocks on the right, and a virtual finger
# instead of a person's, for a benchmark that needs no one at the phone:
# DRIVER=tap (the default) taps the calculator's digits, 4 taps a second
# (tools/tap.py); DRIVER=swipe pulls the shades (tools/swipe.py). Prints the
# compositor's per-second reports and each missed frame. Run on the phone as
# root, with the driver and session-run.sh in /tmp.
T=${1:-25}; shift
DRIVER=${DRIVER:-tap}
python3 /tmp/$DRIVER.py $((T - 6)) > /tmp/tap.out 2>&1 &
TAP=$!
for i in $(seq 1 20); do [ -s /tmp/tap.out ] && break; sleep 0.1; done
DEV=$(head -1 /tmp/tap.out)
chgrp android_input "$DEV" 2>/dev/null; chmod 660 "$DEV"
env "$@" TOUCHSCREEN=$DEV sh /tmp/session-run.sh $T --spawn gnome-calculator --spawn gnome-clocks > /tmp/bench-session.out 2>&1
wait $TAP
sed "s/\x1b\[[0-9;]*m//g" /tmp/item-compositor.log | grep -E "pacing:|touch: |warm-up|drawn|missed:" | sed "s/^[0-9T:.-]*Z *INFO item_compositor[a-z:]*: //"
