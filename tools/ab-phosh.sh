#!/bin/sh
# ab-phosh.sh: the phosh side of the A/B, in one go, on the user's running
# phosh session, unlocked, with the screen on and no one touching it (about
# 3 minutes): the shell at rest (ab-idle.py), the probe animating and tapped
# (ab-probe.sh), and item's own sfduo-perfcheck without its lock scenario.
# Nothing of the user's is changed; the probe's window closes by itself.
S=$(loginctl list-sessions --no-legend | awk '$4 == "seat0" {print $1}')
if [ "$(loginctl show-session $S -p LockedHint --value)" != no ]; then echo "locked: unlock first"; exit 1; fi
echo "== idle"
python3 /tmp/ab-idle.py 60 phoc phosh sfduo-dock sfduo-system sfduo-pen sfduo-posture sfduo-fingerpri sfduo-brightnes phosh-osk-stevi
echo "== anim"; sh /tmp/ab-probe.sh phosh anim 25 | tail -22
echo "== tap"; sh /tmp/ab-probe.sh phosh tap 30
echo "== perfcheck"; sfduo-perfcheck --no-lock 2>&1 | sed "s/\x1b\[[0-9;]*m//g"
