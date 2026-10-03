#!/bin/sh
# pen-bench.sh [SECONDS] [RUNS] [HOST]: pen->screen on the phone, the same
# stroke every time - pen-split draws a Lissajous figure through 'sfduo pen'
# (its bench) and item logs the stroke's percentiles. The pen's sheet has to
# be out, and the phone left alone meanwhile. Prints one line a run.
T=${1:-20}; RUNS=${2:-3}; HOST=${3:-root@172.16.42.1}
SSH="ssh -o UserKnownHostsFile=/dev/null -o StrictHostKeyChecking=no -o LogLevel=ERROR $HOST"
for i in $(seq 1 $RUNS); do
    $SSH "python3 -c \"import socket; s=socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM); s.sendto(b'bench $T', '/run/sfduo-pen.sock')\"; sleep $((T + 2)); journalctl -t item -b --no-pager -o cat --since '-$((T + 5))s' | grep 'pen->screen over' | tail -1 | sed 's/.*pen: //'"
    sleep 2
done
