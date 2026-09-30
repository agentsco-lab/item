# Step 5c: a benchmark for the shade, and the missed frames it found

## The benchmark

`tools/swipe.py` is a virtual finger like `tap.py`, sending a position every
8 ms. It takes turns on the left and the right panel:
- a fling open, then a tap on the sheet (closed);
- a slow pull to 1000 of 1800 px, held still for 0.3 s, then let go (open);
- a swipe up (closed).

`DRIVER=swipe sh /tmp/bench.sh 40` runs it with GNOME Calculator and Clocks
under the sheets. `bench.sh` takes the driver from `DRIVER` (`tap` by
default). No one needs to be at the phone.

The compositor now logs each missed frame with where its time went: how
long before the vsync it was drawn, the draw, the swap, and hwcomposer's
validate and present. It also logs whether the GPU had finished the frame
when it was handed over (the acquire fence, polled), the time since the
last present, and the sheets' state (hybris-hwc's `last_present_detail`).

Raw output: `log/step-5c/swipe-N.out`, 2-6 (the first run, before the fixes, was not saved; its numbers are quoted below).

## What it found

**Chains of missed frames.** The first run gave 40-odd misses in runs of 8
and 22. Each run started with the first frame after a pause: a
vsync with no frame, a GPU that had slowed down, the frame handed over
unfinished, presented 1-3 ms after its vsync. From then on each frame was
handed over while the one before still waited in hwcomposer for the vsync.
hwcomposer's `present` blocked until that vsync (11-14 ms). Every frame
showed a frame late: 60 fps still, but 40 ms from finger to screen. The
chain lasted until the sheet stopped. The blocked present also went into
the render-time estimate, which raised the budget to 20 ms and drew frames
earlier into the blocked present again.

**Fixes** (`compositor/src/main.rs`, `Pacing`):
- After a missed frame, skip a vsync (`drain`), so hwcomposer's queue
  empties. The report counts these as `skipped`.
- The first frame after a pause (more than 1.5 periods since the last swap)
  is drawn at the vsync, not late: a GPU waking up needs the whole frame.
- The render-time estimate takes only frames that went to hwcomposer. A
  frame with nothing damaged costs nothing: in a session with a
  finger such frames pulled the budget down to 6.6 ms and gave 7 misses in
  a row. A slower frame raises the estimate at once (weight 0.5), faster
  ones lower it slowly (0.1).

**After** (swipe-6): 6 single misses in 1258 frames, none in a row, budget
11-19 ms. Finger to screen 21-27 ms as before. The rest are frames after a
single empty vsync that take 13 ms instead of 5 (draw 6, present 4). The
clocks are likely down after even one idle frame. Left as is.

**The first pull** stalled in step 5b. The labels' textures are now
uploaded at start (`warm-up: 3 textures in 0.7 ms`). No stall at a first
pull since.

## The sheet's run

A slow pull by hand showed a jerk a little below the centre on both
panels. Two causes:
- The run after a release eased out: it started at full speed whatever the
  finger did. From a finger held still, the sheet jumped from rest. Now it
  is a cubic (Hermite) that leaves at the finger's velocity and ends at
  rest. From a finger held still it starts from rest, in 150-350 ms,
  shorter for a faster release, never overshooting.
- The rest was the benchmark itself. Its slow pull stops dead at 1000 of
  1800 px, just below the centre, for 0.3 s. A slow pull by hand
  through the centre is smooth.
