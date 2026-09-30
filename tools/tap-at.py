#!/usr/bin/env python3
"""tap-at.py X Y [DELAY] [HOLD]: one tap by a virtual touchscreen at physical
px (X, Y) of the 2784x1800 output, DELAY seconds after it starts (default
5), held HOLD seconds (default 0.08). Prints its device node first, for the
compositor's TOUCHSCREEN, and stays a while after the tap."""
import sys, time
from evdev import UInput, AbsInfo, ecodes as e

x, y = float(sys.argv[1]), float(sys.argv[2])
delay = float(sys.argv[3]) if len(sys.argv) > 3 else 5
hold = float(sys.argv[4]) if len(sys.argv) > 4 else 0.08
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
ui = UInput(caps, name="item tap-at", input_props=[e.INPUT_PROP_DIRECT])
print(ui.device.path, flush=True)
time.sleep(delay)
tx, ty = int(x * X / 2784), int(y * Y / 1800)
for code, v in ((e.ABS_MT_SLOT, 0), (e.ABS_MT_TRACKING_ID, 1), (e.ABS_MT_POSITION_X, tx), (e.ABS_MT_POSITION_Y, ty), (e.ABS_X, tx), (e.ABS_Y, ty)):
    ui.write(e.EV_ABS, code, v)
ui.write(e.EV_KEY, e.BTN_TOUCH, 1)
ui.syn()
time.sleep(hold)
ui.write(e.EV_ABS, e.ABS_MT_TRACKING_ID, -1)
ui.write(e.EV_KEY, e.BTN_TOUCH, 0)
ui.syn()
time.sleep(60)
ui.close()
