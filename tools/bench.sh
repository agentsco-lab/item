#!/bin/sh
# bench.sh SECONDS [SESSION-RUN ENV...]: item-compositor with GNOME Calculator
# on the left panel and GNOME Clocks on the right (started after it, so the
# calculator has the left), and a virtual finger
# instead of a person's, for a benchmark that needs no one at the phone:
# DRIVER=tap (the default) taps the calculator's digits, 4 taps a second
# (tools/tap.py); DRIVER=swipe pulls the shades (tools/swipe.py);
# DRIVER=scroll scrolls item's Settings on the right panel instead of Clocks
# (SCROLL_APP another command). Prints the
# compositor's per-second reports and each missed frame. Run on the phone as
# root, with the driver and session-run.sh in /tmp.
T=${1:-25}; shift
DRIVER=${DRIVER:-tap}
case $DRIVER in
    scroll) CMD="python3 /tmp/swipe.py $((T - 6)) scroll"; APPS="--spawn gnome-calculator --spawn \"sleep 1.5; ${SCROLL_APP:-/usr/local/bin/sfduo-settings}\"" ;;
    *) CMD="python3 /tmp/$DRIVER.py $((T - 6))"; APPS="--spawn gnome-calculator --spawn \"sleep 1.5; gnome-clocks\"" ;;
esac
$CMD > /tmp/tap.out 2>&1 &
TAP=$!
for i in $(seq 1 20); do [ -s /tmp/tap.out ] && break; sleep 0.1; done
DEV=$(head -1 /tmp/tap.out)
chgrp android_input "$DEV" 2>/dev/null; chmod 660 "$DEV"
for kv in "$@"; do export "$kv"; done
export TOUCHSCREEN=$DEV
eval sh /tmp/session-run.sh $T $APPS > /tmp/bench-session.out 2>&1
wait $TAP
sed "s/\x1b\[[0-9;]*m//g" /tmp/item-compositor.log | grep -E "pacing:|touch: |warm-up|drawn|missed:" | sed "s/^[0-9T:.-]*Z *INFO item_compositor[a-z:]*: //"
