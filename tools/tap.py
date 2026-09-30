#!/usr/bin/env python3
"""tap.py SECONDS [RATE]: a virtual touchscreen tapping for a benchmark.

A uinput device like the Duo's (X 0-17709, Y 0-11411 over both panels and
the hinge, multitouch protocol B) that taps GNOME Calculator's digit keys on
the left panel, RATE taps a second (default 4), for SECONDS. It prints its
device node first, for the compositor's TOUCHSCREEN, then waits 3 s before
tapping.
"""
import sys, time
from evdev import UInput, AbsInfo, ecodes as e

seconds = float(sys.argv[1]) if len(sys.argv) > 1 else 20
rate = float(sys.argv[2]) if len(sys.argv) > 2 else 4
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
ui = UInput(caps, name="item tap", input_props=[e.INPUT_PROP_DIRECT])
print(ui.device.path, flush=True)
time.sleep(3)

def px(x, y):  # physical px of the 2784x1800 output to touchscreen units
    return int(x * X / 2784), int(y * Y / 1800)

# GNOME Calculator's digits 7 8 9 4 5 6 on the left panel, at scale 2
# (logical 0-675 x 0-900): rows at about y 708 and 756 in half-size shots,
# i.e. physical y about 1416 and 1512; columns about x 160, 400, 640 physical.
keys = [px(x, y) for y in (1416, 1512) for x in (160, 400, 640)]
tid = 0
end = time.time() + seconds
i = 0
while time.time() < end:
    x, y = keys[i % len(keys)]
    i += 1
    tid += 1
    ui.write(e.EV_ABS, e.ABS_MT_SLOT, 0)
    ui.write(e.EV_ABS, e.ABS_MT_TRACKING_ID, tid)
    ui.write(e.EV_ABS, e.ABS_MT_POSITION_X, x)
    ui.write(e.EV_ABS, e.ABS_MT_POSITION_Y, y)
    ui.write(e.EV_KEY, e.BTN_TOUCH, 1)
    ui.write(e.EV_ABS, e.ABS_X, x)
    ui.write(e.EV_ABS, e.ABS_Y, y)
    ui.syn()
    time.sleep(0.05)
    ui.write(e.EV_ABS, e.ABS_MT_TRACKING_ID, -1)
    ui.write(e.EV_KEY, e.BTN_TOUCH, 0)
    ui.syn()
    time.sleep(max(0.0, 1.0 / rate - 0.05))
ui.close()
