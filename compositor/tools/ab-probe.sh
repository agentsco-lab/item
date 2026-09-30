#!/bin/sh
# ab-probe.sh STACK MODE SECONDS: tools/latency-probe (probes/latency) on
# the left panel of either stack, with a virtual finger tapping it (MODE
# tap: every 0.6 s from 8 s on) or at rest (MODE anim), for the A/B of the
# two stacks. STACK item: item-compositor through session-run.sh, the
# probe its only client. STACK phosh: the user's running phosh session,
# the probe a fullscreen window in it for SECONDS (so the finger lands
# on it wherever phosh puts it), nothing of the user's touched.
# Prints the probe's lines. Run on the phone as root with the probe,
# touch-script.py and session-run.sh in /tmp.
STACK=$1; MODE=$2; T=${3:-30}
USER_NAME=droidian; UID_N=$(id -u $USER_NAME)
rm -f /tmp/probe.out
STEPS=""
[ "$MODE" = tap ] && STEPS=$(python3 -c "print(' '.join(f'tap:500,1200@{8 + 0.5371 * i:.2f}' for i in range(int(($T - 10) / 0.5371))))")
DEV=""
if [ -n "$STEPS" ]; then
    python3 /tmp/touch-script.py $STEPS > /tmp/tap.out 2>&1 &
    TP=$!
    for i in $(seq 1 20); do [ -s /tmp/tap.out ] && break; sleep 0.1; done
    DEV=$(head -1 /tmp/tap.out)
    chgrp android_input "$DEV"; chmod 660 "$DEV"
fi
case $STACK in
    item)
        TOUCHSCREEN=$DEV sh /tmp/session-run.sh $((T + 6)) --spawn "/tmp/latency-probe $MODE $T > /tmp/probe.out 2>&1" > /tmp/bench-session.out 2>&1 ;;
    phosh)
        # The compositor must be up ~6 s after the finger starts, as with item.
        sleep 6
        runuser -u $USER_NAME -- env XDG_RUNTIME_DIR=/run/user/$UID_N WAYLAND_DISPLAY=wayland-0 /tmp/latency-probe $MODE $T full > /tmp/probe.out 2>&1 ;;
esac
[ -n "$TP" ] && kill $TP 2>/dev/null
cat /tmp/probe.out
