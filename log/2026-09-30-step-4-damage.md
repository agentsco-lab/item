# Step 4a: drawing only what changed (2026-09-30)

`Screen::render` now asks the EGL surface for its buffer age and gives it to
smithay's damage tracker, which redraws only what changed since that buffer
was last drawn; a frame with nothing damaged is not swapped. The report
gives the buffer ages, the share of the screen redrawn, and the draw, swap
and present times (`2026-09-30-step-4-damage.log`, drawn at the vsync, a
finger tapping GNOME Calculator (GL) and GNOME Clocks (shm) for 30 s).

- **The buffer age works**: always 2 on libhybris' hwcomposer window.
- **One panel redrawn, not the screen**: 48.5% of the screen, which is one
  1350x1800 window. GTK damages its whole window each frame, so a frame never
  gets smaller than the window that changed.
- **GL clients are cheap, shm clients are not**: frames from GNOME
  Calculator (GL, EGL buffers) drew in about 2 ms; frames from GNOME Clocks
  drawing by the CPU (`GSK_RENDERER=cairo`, shm) in 9-11 ms, with missed
  vsyncs. The same share of the screen: the difference is uploading the shm
  window whole each frame, about 10 MB. Step 3's guess holds.
- hwcomposer's present: still 2.5-3.5 ms on the loop's thread.
- Touch to screen: mostly 25-60 ms.

## So

For a GL client the compositor's frame is about 2 ms of drawing and 3 ms of
present. Next, the present leaves the loop's thread. Uploading shm windows
cheaper (asynchronously, or only the damage a client really changed) waits:
GTK4 apps draw through GL by default.
