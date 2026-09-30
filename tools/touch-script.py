#!/usr/bin/env python3
"""touch-script.py STEP...: a virtual touchscreen playing a script of touches,
for tests with no one at the phone. Each STEP is one of

    tap:X,Y@T            a tap at physical px (X, Y), T seconds after start
    drag:X,Y0,Y1,S@T     a drag at x=X from y=Y0 to Y1 over S seconds, from T
    hdrag:Y,X0,X1,S@T    a drag at y=Y from x=X0 to X1 over S seconds, from T

(physical px of the 2784x1800 output). It prints its device node first, for
the compositor's TOUCHSCREEN, and stays a while after the last step."""
import sys, time
from evdev import UInput, AbsInfo, ecodes as e

X, Y = 17709, 11411
caps = {
    e.EV_KEY: [e.BTN_TOUCH],
    e.EV_ABS: [
        (e.ABS_X, AbsInfo(0, 0, X, 0, 0, 0)),
        (e.ABS_Y, AbsInfo(0, 0, Y, 0, 0, 0)),
        (e.ABS_MT_SLOT, AbsInfo(0, 0, 9, 0, 0, 0)),
        (e.ABS_MT_TRACKING_ID, AbsInfo(0, 0, 65535, 0, 0, 0)),
        (e.ABS_MT_POSITION_X, AbsInfo(0, 0, X, 0, 0, 0)),
        (e.ABS_MT_POSITION_Y, AbsInfo(0, 0, Y, 0, 0, 0)),
    ],
}
ui = UInput(caps, name="item script", input_props=[e.INPUT_PROP_DIRECT])
print(ui.device.path, flush=True)
start = time.time()
tid = 0

def at(x, y, down=False):
    tx, ty = int(x * X / 2784), int(y * Y / 1800)
    ui.write(e.EV_ABS, e.ABS_MT_SLOT, 0)
    if down:
        ui.write(e.EV_ABS, e.ABS_MT_TRACKING_ID, tid)
    ui.write(e.EV_ABS, e.ABS_MT_POSITION_X, tx)
    ui.write(e.EV_ABS, e.ABS_MT_POSITION_Y, ty)
    if down:
        ui.write(e.EV_KEY, e.BTN_TOUCH, 1)
    ui.write(e.EV_ABS, e.ABS_X, tx)
    ui.write(e.EV_ABS, e.ABS_Y, ty)
    ui.syn()

def lift():
    ui.write(e.EV_ABS, e.ABS_MT_TRACKING_ID, -1)
    ui.write(e.EV_KEY, e.BTN_TOUCH, 0)
    ui.syn()

steps = []
for arg in sys.argv[1:]:
    what, rest = arg.split(":", 1)
    params, t = rest.split("@")
    steps.append((float(t), what, [float(v) for v in params.split(",")]))
for t, what, p in sorted(steps):
    time.sleep(max(0.0, start + t - time.time()))
    tid += 1
    if what == "tap":
        at(p[0], p[1], down=True)
        time.sleep(0.06)
        lift()
    elif what == "drag":
        x, y0, y1, secs = p
        at(x, y0, down=True)
        n = max(1, int(secs / 0.008))
        for i in range(1, n + 1):
            time.sleep(0.008)
            at(x, y0 + (y1 - y0) * i / n)
        lift()
    elif what == "hdrag":
        y, x0, x1, secs = p
        at(x0, y, down=True)
        n = max(1, int(secs / 0.008))
        for i in range(1, n + 1):
            time.sleep(0.008)
            at(x0 + (x1 - x0) * i / n, y)
        lift()
time.sleep(30)
ui.close()
