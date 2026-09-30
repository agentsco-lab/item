#!/usr/bin/env python3
"""ab-idle.py SECONDS NAME...: what the shell costs at rest, the same way
under either stack, for the A/B: over SECONDS, the CPU time of the named
processes (exact names, as /proc/PID/comm) and of the whole system, the
GPU's busy share, and at the end the named processes' memory (PSS, the
shared pages split between their users). Run on the phone as root, with
the screen on and nobody touching it."""
import os
import sys
import time

seconds = float(sys.argv[1])
names = set(sys.argv[2:])
HZ = os.sysconf("SC_CLK_TCK")
GPU = "/sys/class/kgsl/kgsl-3d0/gpu_busy_percentage"


def procs():
    out = {}
    for pid in os.listdir("/proc"):
        if not pid.isdigit():
            continue
        try:
            comm = open(f"/proc/{pid}/comm").read().strip()
        except OSError:
            continue
        if comm in names:
            out[int(pid)] = comm
    return out


def cpu(pid):
    try:
        f = open(f"/proc/{pid}/stat").read().rsplit(")", 1)[1].split()
        return (int(f[11]) + int(f[12])) / HZ
    except OSError:
        return 0.0


def system():
    f = open("/proc/stat").readline().split()[1:]
    v = [int(x) for x in f]
    idle = v[3] + v[4]
    return sum(v) / HZ, (sum(v) - idle) / HZ


def pss(pid):
    try:
        for line in open(f"/proc/{pid}/smaps_rollup"):
            if line.startswith("Pss:"):
                return int(line.split()[1]) / 1024
    except OSError:
        pass
    return 0.0


found = procs()
start = {p: cpu(p) for p in found}
total0, busy0 = system()
gpu = []
t_end = time.time() + seconds
while time.time() < t_end:
    try:
        gpu.append(int(open(GPU).read().split()[0]))
    except (OSError, ValueError, IndexError):
        pass
    time.sleep(1)
total1, busy1 = system()
cores = os.cpu_count()
print(f"over {seconds:.0f} s, {cores} cores")
rows = {}
for p, n in found.items():
    c = cpu(p) - start[p]
    m = pss(p)
    r = rows.setdefault(n, [0.0, 0.0])
    r[0] += c
    r[1] += m
for n, (c, m) in sorted(rows.items(), key=lambda kv: -kv[1][1]):
    print(f"  {n:24s} CPU {c / seconds * 100:5.1f} % of a core   PSS {m:6.1f} MB")
print(f"  {'named, together':24s} CPU {sum(r[0] for r in rows.values()) / seconds * 100:5.1f} % of a core   PSS {sum(r[1] for r in rows.values()):6.1f} MB")
print(f"  whole system busy {(busy1 - busy0) / (total1 - total0) * 100:.1f} % of all cores")
if gpu:
    print(f"  GPU busy {sum(gpu) / len(gpu):.1f} % (mean of {len(gpu)} samples)")
missing = names - set(found.values())
if missing:
    print("  not running: " + " ".join(sorted(missing)))
