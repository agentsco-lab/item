# Step 4b: hwcomposer's present, the buffer age, and a benchmark with no one at the phone (2026-09-30)

## Not a thread for the present

The plan was to give hwcomposer's present (2.5-3.5 ms) a thread of its own.
libhybris' hwcomposer window (`hwcomposer_window.cpp`) settles it: it has no
queue of free buffers - `dequeueBuffer` takes the next buffer in turn, and
the only thing keeping EGL from drawing into a buffer still on screen is
the release fence the present callback puts on it, under the window's lock.
A present on another thread would put that fence there late and unlocked:
torn frames. It can be made safe, and would free 3 ms of the loop without
showing a frame any sooner (the present still has to be done by the vsync).
Not now.

## presentOrValidate: kills the compositor

hwc2's presentOrValidate presents at once when the layers did not change
(one client layer here, which never does), skipping validate. On the Duo
the compositor died at its first frame (systemd: "Finished with result:
signal"), in the Android side of libhybris' hwc2 layer. It stays in
`hybris-hwc`, off unless `HWC_PRESENT_OR_VALIDATE=1`.

A GL client's frame is about 5 ms: about 2 of drawing and 3 of present.

## The buffer age was wrong: an empty panel flickered

With only GNOME Calculator on the left panel, the empty right panel
flickered. EGL reports a buffer age of 2, but libhybris' hwcomposer
window has 3 buffers and hands them out in strict turn, so a buffer holds the
frame 3 frames back. The damage tracker left undamaged parts with contents
one frame older than it thought; on an empty panel that was whichever of the
old frames. The compositor now counts its swaps: age 0 for the first 3,
then 3. No flicker since.

## A benchmark with no one at the phone

`tools/tap.py` makes a uinput touchscreen like the Duo's (X 0-17709, Y
0-11411, multitouch) and taps GNOME Calculator's digits 4 times a second;
`tools/bench.sh SECONDS ENV...` runs the compositor on it
(`TOUCHSCREEN=`) with the calculator and prints the reports. The same run can
now be repeated for every change.

## Drawing late, measured

`bench.sh 25 LATE=0` against `bench.sh 25 LATE=1`
(`2026-09-30-step-4-bench-late0.log`, `-late1.log`), second by second after
the start:

| | at the vsync | late in the frame |
|---|---|---|
| touch to screen, the seconds' means | about 44 ms | about 39 ms |
| draw / present | about 2 / 2.8 ms | about 2 / 2.8 ms |
| missed vsyncs | 0-1 a second, rarely | 0-1 a second, a little more often |

About 5 ms off the touch's round trip. Late stays the default.

## Open

In both earlier comparisons the finger gave no touches at all in the
late run - each time the second session of two in a row - while the
benchmark's taps reach the late mode fine. Maybe the port's `sfduo
touchscreen` gives nothing to a second session in a row, not the late mode.
To try: the late mode first, with a finger.
