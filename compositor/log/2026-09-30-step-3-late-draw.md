# Step 3: drawing late in the frame, and what a frame costs (2026-09-30)

## Drawing late

A frame drawn at a vsync shows at the next; anything a client commits after
that vsync waits a frame more. So at each vsync the compositor now aims the
frame at the next vsync and draws it at that vsync less a budget - a running
average of what a frame takes, times 1.5, plus a margin for hwcomposer's
present (`LATE_MARGIN_MS`, default 4) - on a one-shot timer. `LATE=0` draws
at the vsync, as step 2 did. A frame whose present comes after the vsync it
aimed for is counted as missed (it shows a frame later: a stutter). Frames
longer than two periods (the first, a stall) stay out of the average.

## A frame costs more than the probes said

The first comparison of the two modes was lost to the logs (systemd's
`file:` output writes over the old file from its start, not truncating it;
`session-run.sh` removes the log first now) and to the second half having no
touches. So the frame's time was broken down first
(`2026-09-30-step-3-frame-cost.log`, drawn at the vsync, a finger tapping
GNOME Calculator and GNOME Clocks for 30 s):

| part of a frame | time |
|---|---|
| hwcomposer's present (validate, client target, present, fences, inside the swap) | 2.5-3.5 ms, on the loop's thread |
| drawing the scene, first half | about 3 ms |
| drawing the scene, second half | 10-13 ms, and 1-4 missed vsyncs a second |

The jump is most likely GNOME Clocks, which draws by the CPU (shm): each of
its frames goes to the GPU whole, a 1350x1800 window, about 10 MB; GNOME
Calculator's GL buffers cost next to nothing. Not yet proved.

Probe C1's 1-1.5 ms was a frame of two solid rectangles, and probe B's swap
hid hwcomposer's present inside the wait for vsync.

## So

Drawing late pays only when a frame is short and steady; with a budget of
13-25 ms of a 16.7 ms period it draws at once anyway. First the frame gets
cheaper:

1. draw and upload only what changed (the EGL surface's buffer age with the
   damage tracker; shm uploads limited to the buffer's damage);
2. hwcomposer's present on a thread of its own, so its 3 ms leave the loop.

Then the late draw is measured again. It stays on by default; with a large
budget it draws at once.
