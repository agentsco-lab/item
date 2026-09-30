#!/bin/sh
# script-run.sh SECONDS FRAMES_AT STEP...: item-compositor for SECONDS with
# nothing spawned, a virtual finger playing the STEPs (tools/touch-script.py),
# and 40 frames saved from FRAMES_AT seconds (as frames-run.sh), packed into
# /tmp/frames.tar. Run on the phone as root, with touch-script.py and
# session-run.sh in /tmp.
T=$1; F=$2; shift 2
rm -f /tmp/item-frame* /tmp/item-shot* /tmp/frames.tar
python3 /tmp/touch-script.py "$@" > /tmp/tap.out 2>&1 &
TP=$!
for i in $(seq 1 20); do [ -s /tmp/tap.out ] && break; sleep 0.1; done
DEV=$(head -1 /tmp/tap.out)
chgrp android_input "$DEV"; chmod 660 "$DEV"
(sleep $F; runuser -u droidian -- touch /tmp/item-frames) &
TOUCHSCREEN=$DEV sh /tmp/session-run.sh $T > /tmp/bench-session.out 2>&1
kill $TP
sed "s/\x1b\[[0-9;]*m//g" /tmp/item-compositor.log | grep -E "spawned|window mapped|put away|brought back|called over|back:|curtain|app id|panic|ERROR" | sed "s/^[0-9T:]*\.[0-9]*Z *[A-Z]* [^ ]* //" | cut -c1-140
cd /tmp && ls item-frame-* >/dev/null 2>&1 && tar cf /tmp/frames.tar item-frame-* && ls item-frame-* | wc -l
