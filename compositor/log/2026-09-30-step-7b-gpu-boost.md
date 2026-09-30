# Step 7b: the GPU boost, and when clients hear their frame was taken

## The boost

`compositor/src/boost.rs`. On any touch (down, motion, up), and while the
shade or the dock moves, the GPU's minimum clock goes to its maximum
(`/sys/class/kgsl/kgsl-3d0/devfreq/min_freq`, 257 -> 585 MHz). It comes
back 1.5 s after the last, since a kinetic scroll outlasts the finger. The
file is root's: `session-run.sh` hands it to the session's user for the
run, and after it puts back the file's user and the old value. Checked after
runs: `min 257000000, user root`. `GPU_BOOST=0` turns it off. The report
gives the share of each second boosted.

## A surprise: taps got slower

| tap to screen (calculator) | ms |
|---|---|
| no boost | 28.1 |
| boost | 35.8, 37.3 |

The report shows why. With the GPU boosted, the calculator answers in 4-5
ms. It hears its frame was taken as ours is drawn, draws, and commits
while ours still waits in hwcomposer for its vsync. The rule against
queueing (6b) holds it to the next vsync's frame: 1-9 frames a second were
drawn at once, not 22-33. Its answer shows a vsync later.

A second way was tried: frame callbacks at the vsync that puts the frame
on screen (`CALLBACKS=vsync`). Then the client draws right after that
vsync, and its frame is drawn at once for the next.

| callbacks | boost | tap to screen | Settings scrolling, fps |
|---|---|---|---|
| as the frame is drawn | no | 28 ms | 43 |
| as the frame is drawn | yes | 36-37 ms | 60 |
| at the vsync | yes | 34 ms | 39-48 |
| at the vsync | no | 42 ms | 26 |

Settings needs 12-14 ms a frame even boosted. Heard at the vsync, that
leaves too little of the frame, and it drops frames. Heard as the frame
is drawn, it draws its next while ours waits, as under phoc.

Chosen: callbacks as the frame is drawn, and the boost. A scroll at 60 fps
shows more than 8 ms on a tap. `CALLBACKS=vsync` and `CALLBACKS=after`
stay for tests.

With the boost, the shade benchmark gives 26.0 ms from finger to screen
and 4 missed frames in 1318.
