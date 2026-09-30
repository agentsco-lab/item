#!/usr/bin/env python3
"""swipe.py SECONDS [scroll]: a virtual finger pulling the shade, or with
`scroll` scrolling the window on the right panel, for a benchmark.

The same kind of uinput touchscreen as tap.py (X 0-17709, Y 0-11411 over
both panels and the hinge, multitouch protocol B), sending a position every
8 ms as a finger would. It takes turns on the left and the right panel:
- a quick pull from the top edge to about two thirds down, let go (a fling:
  the sheet runs open), then a tap on the sheet (it runs closed);
- a slow pull to half way and a bit, held still, let go (it runs open), then
  a swipe up (it runs closed).
With `scroll` it drags up and down the middle of the right panel instead,
0.8 s each way, between y 1500 and 500 (physical px).
It prints its device node first, for the compositor's TOUCHSCREEN, then
waits 3 s before it starts.
"""
import sys, time
from evdev import UInput, AbsInfo, ecodes as e

seconds = float(sys.argv[1]) if len(sys.argv) > 1 else 20
scroll = len(sys.argv) > 2 and sys.argv[2] == "scroll"
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
ui = UInput(caps, name="item swipe", input_props=[e.INPUT_PROP_DIRECT])
print(ui.device.path, flush=True)
time.sleep(3)

STEP = 0.008
tid = 0

def at(x, y, down=None):
    """The finger at physical px (x, y) of the 2784x1800 output."""
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

def drag(x, y0, y1, seconds, hold=0.0):
    """Down at y0, to y1 in `seconds`, held still for `hold`, then up."""
    global tid
    tid += 1
    at(x, y0, down=True)
    n = max(1, int(seconds / STEP))
    for i in range(1, n + 1):
        time.sleep(STEP)
        at(x, y0 + (y1 - y0) * i / n)
    t = 0.0
    while t < hold:
        time.sleep(STEP)
        at(x, y1)
        t += STEP
    lift()

end = time.time() + seconds
while scroll and time.time() < end:
    drag(2109, 1500, 500, 0.8)
    time.sleep(0.3)
    drag(2109, 500, 1500, 0.8)
    time.sleep(0.3)
panel = 0
while time.time() < end:
    x = [675, 2109][panel]
    drag(x, 4, 1200, 0.3)            # a fling open
    time.sleep(0.8)
    drag(x, 600, 600, 0.05)          # a tap: closed
    time.sleep(0.8)
    drag(x, 4, 1000, 1.2, hold=0.3)  # a slow pull past half, let go: open
    time.sleep(0.8)
    drag(x, 1400, 300, 0.25)         # a swipe up: closed
    time.sleep(0.8)
    panel ^= 1
ui.close()
