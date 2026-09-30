# Step 7: where a frame's time goes, and the GPU's clock

After step 6b: good, with room to improve. Is there room for
optimisation?

## Measured while Settings scrolls (`DRIVER=scroll`, 40-43 fps)

| | |
|---|---|
| the GPU (`msm-adreno-tz`) | at its lowest clock, 257 of 585 MHz, 51-54 % busy |
| the big cores | 710 of 2420 MHz |
| the little cores | 1.2-1.6 of 1.78 GHz; the work runs on these |
| Settings | 32 % of a core, about 7 ms of CPU a frame |
| item-compositor | 31 % of a core, about 7.5 ms a frame: 3-4 ms drawing, 3-4 ms in hwcomposer's present, synchronous |

Nothing is at 100 %. A frame goes through a chain: the client's CPU, the
client's GPU, our GPU, hwcomposer. Every link is slow at the clocks the
governors pick, and `msm-adreno-tz` does not raise the GPU's clock at half
load. Android has an input boost for this; Droidian's clocks show none.

## The minimum clocks raised by hand

`tools/boost-bench.sh` and `tools/boost-one.sh`, run as root on the phone,
with the old minimums put back after whatever happens.

All at their maximum:

| | as the governors leave them | all at max |
|---|---|---|
| Settings' frames a second | 40-43 | 59-61 |
| our draw a frame | 3-4 ms | 2.0 ms |
| the shade, finger to screen | 27.1 ms | 23.0 ms |
| tap to screen (calculator) | 31.7 ms | 33.6 ms |

Tap to screen does not change. The time there is waiting for vsyncs, not
drawing.

One at a time, Settings' frames a second:

| minimum raised | frames a second |
|---|---|
| none (GPU 257 MHz) | 43 |
| GPU 345 MHz | 48 |
| GPU 427 MHz | 55 |
| GPU 499 MHz | 58 |
| GPU 585 MHz (max) | 60 |
| all CPUs at max, GPU left alone | 44 |
| the little cores at max | 44 |

The GPU's clock is the lever; the CPUs' are not.

## What follows

1. A GPU boost while the finger is on the screen, and while the shell
   moves, back down a second after. The compositor sees every touch and
   every animation first, as Android's input boost does. It needs write
   access to `/sys/class/kgsl/kgsl-3d0/devfreq/min_freq`: for the tests
   `session-run.sh` can give it for the run.
2. hwcomposer's present on a thread of its own, safe with the buffers'
   fences: 3-4 ms a frame off the loop.
3. Our frame cheaper on the GPU (the damage into a persistent buffer, a
   whole copy out), if the GPU still limits after 1.
4. `wp_presentation`, which GTK uses under phoc to time its frames.
