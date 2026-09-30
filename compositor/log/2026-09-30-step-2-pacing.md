# Step 2: frames paced by hwcomposer's vsync (2026-09-30)

Before: a loop that dispatched, drew and swapped every frame, and the swap
waited for vsync (about 15 ms): a frame every vsync whatever changed, and
touches and clients waited while the swap did.

Now (`compositor/src/main.rs`):

- the event loop sleeps until something happens - a client, a touch, a
  vsync, a timer - (`EventLoop::run`); its data is the Wayland state and the
  screen;
- hwcomposer's vsync callback, on its own thread, wakes it through a calloop
  ping (`hybris_hwc::set_vsync_handler`);
- at a vsync a frame is drawn only if something changed since the last one
  (a commit, a new window);
- the swap does not wait (EGL swap interval 0): the frame goes to hwcomposer
  and the loop goes back to serving clients and touches;
- frame callbacks go out after the frame, with the time it will be on screen
  (the vsync after the one that drew it);
- a watchdog draws a pending frame if no vsync came for 50 ms.

## hwcomposer sends no vsync before a frame

The first run drew nothing: 0 vsyncs, black panels. hwcomposer starts its
vsyncs only once it has been given a frame, and the compositor waited for a
vsync to draw its first. The first frame is now drawn at start, and the
watchdog covers vsyncs stopping later.

## The numbers (`2026-09-30-step-2-pacing.log`)

Both panels tapped by hand for 40 s, GNOME Calculator (GL) on the left and
GNOME Clocks (shm) on the right.

- **Frames only when something changed**: frames drawn equal client commits
  every second - 6 to 31 while tapping, 0 of 60 vsyncs when nothing moved.
- **Touch to screen**: from a touch to the vsync that shows the first client
  commit after it, usually 30-65 ms, about 45 ms on average. That includes
  GTK taking the touch and redrawing; the compositor's part is up to one
  frame (from the commit to the next vsync) and the vsync that shows it.
- No hwc errors.

Two readings of 648 ms and 3.1 s were the measure's fault: a touch no client
answered waited for the next unrelated commit. A touch not answered within
half a second is no longer counted.

## Next

Drawing late in the frame (start at the next vsync minus the time a frame
takes, as Android's SurfaceFlinger does) to take up to a frame off; the
presentation-time protocol (`wp_presentation`) with hwcomposer's present
fences; the portals' "Unable to open /proc/PID/root" (they cannot identify
the app).
