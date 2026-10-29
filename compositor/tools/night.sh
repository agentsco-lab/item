#!/bin/sh
# night.sh [FROM_EPOCH [TO_EPOCH]]: the phone's drain over a night, from
# UPower's own history on the phone (not a logger on the computer: the
# phone's sleep is what is measured). Without FROM, the last stretch of
# discharging. Says %/h, the mean power, the output in use and the suspends.
#   ssh root@172.16.42.1 sh -s < tools/night.sh
#   ssh root@172.16.42.1 sh -s 1793312600 < tools/night.sh
FROM=${1:-0}
TO=${2:-9999999999}
H=${UPOWER_HISTORY:-/var/lib/upower}
awk -v from="$FROM" -v to="$TO" '
    # A stretch of discharging ends where another state starts (UPower
    # writes the charge only as its percent changes: an hour apart, at night).
    $1 < from || $1 > to { next }
    $3 != "discharging" { on = 0; next }
    {
        if (!on) { on = 1; n = 0; t0 = $1; p0 = $2 }
        n++; t1 = $1; p1 = $2
    }
    END {
        if (n < 2 || t1 - t0 < 3600) { print "night: under an hour of discharging - nothing to tell"; exit 1 }
        h = (t1 - t0) / 3600
        printf "night: %s to %s, %.1f h, %.0f%% -> %.0f%%: %.2f %%/h\n", strftime("%F %R", t0), strftime("%F %R", t1), h, p0, p1, (p0 - p1) / h
        print t0, t1 > "/tmp/night-span"
    }' $H/history-charge-*[0-9].dat || exit 1
read T0 T1 < /tmp/night-span
awk -v a="$T0" -v b="$T1" '$3 == "discharging" && $1 >= a && $1 <= b { s += $2; n++ } END { if (n) printf "night: %.3f W on average (%d samples)\n", s / n, n }' $H/history-rate-*[0-9].dat
echo "night: output $(cat /run/item-output 2>/dev/null || echo hwc), up $(uptime -p)"
journalctl --since "@$T0" --until "@$T1" --no-pager -q 2>/dev/null | grep -c "PM: suspend entry" | sed 's/^/night: suspends /'
