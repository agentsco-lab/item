#!/bin/sh
# frames-run.sh X Y [ENV...]: item-compositor for 22 s with nothing spawned,
# a virtual finger tapping physical px (X, Y) 7 s in (tools/tap-at.py), and
# the 40 frames after 8 s saved as they went to hwcomposer: the screen's
# bottom 600 rows, RGBA, bottom-up, /tmp/item-frame-NNN-MS.rgba, packed into
# /tmp/frames.tar. Run on the phone as root, with tap-at.py and
# session-run.sh in /tmp.
X=$1; Y=$2; shift 2
rm -f /tmp/item-frame* /tmp/item-shot* /tmp/frames.tar
python3 /tmp/tap-at.py $X $Y ${TAP_AT:-7} > /tmp/tap.out 2>&1 &
TP=$!
for i in $(seq 1 20); do [ -s /tmp/tap.out ] && break; sleep 0.1; done
DEV=$(head -1 /tmp/tap.out)
chgrp android_input "$DEV"; chmod 660 "$DEV"
(sleep 8; runuser -u droidian -- touch /tmp/item-frames) &
env "$@" TOUCHSCREEN=$DEV sh /tmp/session-run.sh 22 > /tmp/bench-session.out 2>&1
kill $TP
sed "s/\x1b\[[0-9;]*m//g" /tmp/item-compositor.log | grep -E "spawned|window|panic|ERROR" | cut -c1-160
cd /tmp && tar cf /tmp/frames.tar item-frame-* && ls item-frame-* | wc -l
