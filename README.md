# item-compositor

A Wayland compositor of our own for the Surface Duo 1, in Rust on
[smithay](https://github.com/Smithay/smithay), meant to become the base of
item-shell (agentsco-lab/item).

Why, from a day of probes on the Lindroid chain
(agentsco-lab/duo-lindroid, `log/2026-09-30-choosing-a-base.md`):

- **Speed.** It draws straight into Android's hwcomposer through libhybris'
  hwc2 API, as Droidian's phoc does in C: no Lindroid chain, no CPU copy of
  each frame, vsync from hwcomposer. On the chain the copy halves the frame
  rate when both panels animate, and nothing syncs the GPU with it.
- **Smoothness.** item's UI - the dock across both panels, a shade per half,
  windows tiled to their panel, a window's slide between panels - is drawn
  inside the compositor, in the same GPU pass as the windows, with gestures
  that follow the finger and our own frame scheduling. No round trips to
  shell clients.
- **Freedom.** Our own code, not patches on phoc and phosh.

It is an experiment beside the port (iverbovoy/surfaceduo-droidian) and
item, which keep running the phone day to day.

## Where it stands (2026-09-30)

| | | |
|---|---|---|
| the frame's path | Rust → libhybris hwc2 → hwcomposer, no copy, fences through | 60 fps across both panels, 16.7 ms between frames |
| the renderer | smithay's `GlesRenderer` on EGL without GBM (the Android platform, hwcomposer's window) | 1-1.5 ms to render a frame |
| clients | xdg-shell; shm, and GL over libhybris' `android_wlegl` | a window per panel, 60 fps with two |
| touch | libinput to `wl_touch`, the touchscreen one to one with the output, hinge and all | both panels |
| the session | the user's, on tty7 as phosh runs; our desktop name, a gtk portals configuration | GTK apps map in under 2 s |
| the shade | a sheet per panel, pulled from the top edge by the finger, drawn in the compositor's own pass; the time, the date, the battery as text (fontdue, the system's fonts) | finger to screen 22-26 ms, no frames missed |
| the dock | item's two halves at the panels' outer corners, apps from item's dock.json, a tap launches onto the panel tapped; the halves cross the hinge to the free panel and join, as item's do | launches in under 2 s; frames drawn whole (partial redraws left garbage on Adreno) |
| the GPU's clock | raised to the top on touch and while the shell moves, 1.5 s after (the governor leaves it at 257 of 585 MHz) | Settings scrolls at 60 fps (43) |
| the launch curtain | the panel black with the app's icon from the tap to the window's first frame, as item's | up in the next frame; apps draw 1.5-2.5 s later |
| putting a window away | a swipe up from a panel's bottom edge, the window following the finger, as phoc's minimize; back with a tap on its app in the dock | |
| the app grid | every app in five columns on black, up with a swipe on an empty panel, following the finger, as item's | |
| the shades' contents | left: brightness, volume, six quick settings; right: the open windows with close buttons, as item's split | brightness through logind from the session |
| notifications | the session's notification server; a banner on the right panel, the list in the right shade | |
| the on-screen keyboard | the port's stevia (item's build) through layer-shell, input-method, text-input and virtual-keyboard; windows shorten under it | types |
| locking | the power key blanks (hwc2 power off) and locks; the lock screen and its PIN pad, the PIN checked by PAM as phosh does; the volume keys | |
| the system screen | left of the left panel, a swipe right on its desktop: greeting, battery card with 24 h graph, as item's #109 | |
| the lid, the pen | the lid shut locks and darkens; the pen touches as a finger | |
| the pen's sheet | right of the right panel, a swipe left on its desktop: the pen draws with its pressure, its other end or button erases; clear, save as PNG, as item's sfduo-pen-screen | a stroke uploads only what it touched |
| frame pacing | hwcomposer's vsync wakes the loop; a frame only when something changed, a client's frame drawn at once, the shell's motion late in the frame; never two frames waiting in hwcomposer; drawn whole | 0 frames when still; a GL client's frame about 5 ms; touch to screen about 39 ms through GTK |

What it does not do yet: installing as a session of its own, the pen's own sheet, cheaper shm frames, `wp_presentation`, the portals' Settings interface.

## How it was reached

| | | |
|---|---|---|
| B | a minimal Rust program that puts a GL frame on the panels through hwc2 | 60 fps, 16.7 ms, no copy (`log/2026-09-30-probe-b.md`) |
| C1 | smithay's renderer on our EGL over hwcomposer | 60 fps, 1-1.5 ms a frame; the first frame was wrong, see the log (`log/2026-09-30-probe-c1.md`) |
| C2 | Wayland clients: shm, and GTK4 over `android_wlegl` | both, a window per panel (`log/2026-09-30-probe-c2.md`) |
| C3 | touch | both panels, the port's `sfduo touchscreen` (`log/2026-09-30-probe-c3.md`) |
| 1 | the compositor crate, in the user's session | apps in under 2 s once the session's portals were set up (`log/2026-09-30-step-1-compositor.md`) |
| 2 | frames paced by vsync, drawn only on change | 0 frames when still, touch to screen about 45 ms (`log/2026-09-30-step-2-pacing.md`) |
| 3 | drawing late in the frame | in, but a frame costs 3-13 ms plus hwcomposer's 3 ms present: make it cheaper first (`log/2026-09-30-step-3-late-draw.md`) |
| 4a | buffer age and damage: redraw only what changed | one panel instead of the screen; GL client frames 2 ms, shm 9-11 ms (`log/2026-09-30-step-4-damage.md`) |
| 4b | the present, the buffer age, a tapping benchmark | presentOrValidate crashes; buffer age 3 not EGL's 2 (flicker gone); drawing late takes about 5 ms off, to about 39 ms touch to screen (`log/2026-09-30-step-4b-present.md`) |
| 5 | the shade: a sheet per panel following the finger, a clock in seven segments | finger to screen 22-26 ms against about 39 ms through GTK; a 1 px line from logical rounding fixed (`log/2026-09-30-step-5-shade.md`) |
| 5b | text on the shade: fontdue into a texture, remade only when the text changes | sharp, the right way up, as smooth; two seconds of missed frames to look into (`log/2026-09-30-step-5b-text.md`) |
| 5c | a shade benchmark (a virtual finger), each missed frame logged | chains of missed frames behind hwcomposer's blocking present: now a vsync skipped after a miss, the first frame after a pause drawn at once, the budget from swapped frames only; the run leaves at the finger's speed (`log/2026-09-30-step-5c-shade-bench.md`) |
| 6 | the dock; screenshots and frame runs | first frame not shown by hwcomposer; clients need the port's profile.d environment; partial redraws leave garbage in Adreno's tiles: frames drawn whole; Settings scrolls at 30 fps on its own side (`log/2026-09-30-step-6-dock.md`) |
| 6b | the dock's modes and moves; clients' frames drawn at once | Settings scrolls at 40-43 fps (phosh 38-43, before 28-30), tap to screen 32 ms (before 45-48) (`log/2026-09-30-step-6b-dock-moves-and-pacing.md`) |
| 7 | where a frame's time goes; the minimum clocks raised by hand | the GPU sits at 257 of 585 MHz half busy; at 585 Settings scrolls at 60 fps (43), the shade 23 ms; the CPUs change nothing (`log/2026-09-30-step-7-clocks.md`) |
| 7b | the GPU boost on touch and shell motion (boost.rs) | Settings scrolls at 60 fps; a quick client's tap answer a vsync later (36 ms, 28 without); callbacks kept as the frame is drawn (`log/2026-09-30-step-7b-gpu-boost.md`) |
| 8 | the launch curtain (curtain.rs) | the tap answered at once (`log/2026-09-30-step-8-curtain.md`) |
| 9 | the swipe up that puts a window away (gesture.rs) | the window follows the finger; the dock comes back as it goes (`log/2026-09-30-step-9-put-away.md`) |
| 10 | the app grid (grid.rs, apps.rs) | 18 apps; a tap launches under the curtain or brings a window back (`log/2026-09-30-step-10-grid.md`) |
| 11 | the desktop clock, running dots, back from the edge, an open app called over (clock.rs, back.rs) | all as item's (`log/2026-09-30-step-11-clock-dots-back-call.md`) |
| 12 | the shades' contents (quick.rs) | settings left, open windows right; system commands on a thread (`log/2026-09-30-step-12-shade-contents.md`) |
| 13 | notifications (notify.rs, zbus) | banner and shade list; actions and dismissals signalled (`log/2026-09-30-step-13-notifications.md`) |
| 14 | the on-screen keyboard (layers.rs, protocols.rs) | stevia needs phoc's device state, wlr foreign toplevel and data control to start; the output after xdg-output; one stevia, its user unit (`log/2026-09-30-step-14-keyboard.md`) |
| 15 | the dock's motion: contact, bump and squash, corners, the neck; windows through the hinge | as item's (`log/2026-09-30-step-15-dock-motion.md`) |
| 16 | the power key, blanking, the lock screen, the volume keys (lock.rs) | locks, blanks, unlocks with a swipe (`log/2026-09-30-step-16-lock.md`) |
| 17 | the PIN (pam.rs) | PAM's phosh service on a thread; a wrong one refused (`log/2026-09-30-step-17-pin.md`) |
| 18 | the lid, the pen, the system screen (sysscreen.rs) | (`log/2026-09-30-step-18-lid-pen-system.md`) |
| 19 | a window's close, the volume bar, the dock's rise after an unlock | as item's (`log/2026-10-01-step-19-motion.md`) |
| 20 | the pen's sheet (pensheet.rs) | drawn and saved in a scripted run; the dock now told on the frame after a slide ends (`log/2026-10-01-step-20-pen-sheet.md`) |
| A | smithay's anvil on the Lindroid chain | not needed: the chain is not the path |

The seams between smithay and libhybris, which the probes answered:

- **The EGL display.** There is no GBM on this path: smithay gets an
  `EGLNativeDisplay` on the Android platform and an `EGLNativeSurface` over
  libhybris' hwcomposer window (`crates/smithay-hybris`).
- **Clients' buffers.** GL apps hand buffers over `android_wlegl`, which
  libhybris' EGL serves after `eglBindWaylandDisplayWL`; smithay's
  `bind_wl_display` (`use_system_lib`) imports them.
- **Sync.** A frame goes to hwcomposer through EGL's swap with its fence.
- **Orientation.** GL draws the EGL surface's default framebuffer bottom-up:
  frames are rendered with `Transform::Flipped180`, clients see a normal
  output.

The fallback, if this had failed: SwayFX with item as a Rust daemon over
sway's IPC and layer-shell clients (duo-lindroid,
`log/2026-09-30-probe-swayfx.md`).

## What the Duo gives a compositor

- One hwcomposer display, 2784x1800: two 1350x1800 panels and 84 columns
  under the hinge between them. At scale 2 the left panel is x 0-675, the
  hinge 675-717, the right panel 717-1392. The composer takes only a client
  target (duo-lindroid: device layers came back changed to client
  composition).
- One touchscreen over both panels and the hinge: X 0-17709, Y 0-11411. The
  port's `sfduo-pen-split` grabs it and gives the fingers `sfduo touchscreen`
  and the pen `sfduo pen`.
- Adreno 640 through libhybris: GLES 3.2, EGL 1.5. Its GLSL ES compiler is
  strict (an undefined name in `#if` is an error). No dmabuf import on this
  platform.
- vsync 16.667 ms; `eglSwapBuffers` waits for it.
- The port's scale is 2.

## Building and running

On the PC, once:

    curl https://sh.rustup.rs | sh -s -- --no-modify-path --profile minimal
    ~/.cargo/bin/rustup target add aarch64-unknown-linux-gnu
    tools/fetch-sysroot.sh            # the phone's libxkbcommon and libinput

The cross linker is `aarch64-linux-gnu-gcc` (`.cargo/config.toml`); the
binaries need glibc 2.34, the phone has 2.43. Then:

    ~/.cargo/bin/cargo build --release --target aarch64-unknown-linux-gnu -p item-compositor
    scp target/aarch64-unknown-linux-gnu/release/item-compositor tools/session-run.sh root@172.16.42.1:/tmp/
    ssh root@172.16.42.1 sh /tmp/session-run.sh 60 --spawn gnome-calculator --spawn gnome-clocks

`session-run.sh` stops phosh, restarts the Android composer clean, runs the
compositor as droidian on tty7 for the given seconds, and brings phosh back
whatever happens. Nothing is installed on the phone. The compositor's log is
`/tmp/item-compositor.log` there.

## Layout

- `compositor/` - item-compositor: `layout` (the panels and the hinge),
  `state` (the Wayland protocols), `input` (touch), `shade` (the shade), `dock` (the dock), `boost` (the GPU's clock), `curtain` (the launch curtain), `gesture` (putting windows away), `grid` (the app grid), `apps` (desktop files and icons), `clock` (the desktop clock), `back` (back from the edge), `quick` (the shades' contents), `notify` (notifications), `layers` (layer surfaces, the keyboard), `protocols` (the globals stevia needs), `lock` (locking, blanking, the keys), `pam` (the PIN), `sysscreen` (the system screen), `pensheet` (the pen's sheet), `text` (fonts and labels), `output` (hwcomposer,
  EGL, the renderer)
- `crates/hybris-hwc` - hwcomposer through libhybris: display 0, a client
  layer, a native window presenting with fences
- `crates/smithay-hybris` - smithay's native EGL traits for libhybris without GBM
- `probes/` - the probes: `hwc-frame` (B), `smithay-frame` (C1), `wl-panels` (C2, C3)
- `tools/session-run.sh` - item-compositor in the user's session on the phone
- `tools/probe-run.sh` - a probe on the phone with the shell stopped
- `tools/boost-bench.sh`, `tools/boost-one.sh` - the benchmarks with minimum
  clocks raised, restored after (root on the phone)
- `tools/touch-script.py`, `tools/script-run.sh` - a session driven by a script
  of taps and drags, with frames saved
- `tools/fetch-sysroot.sh` - the phone's libraries a cross build links against
  (into `sysroot/`, not tracked)
- `tools/bench.sh`, `tools/tap.py`, `tools/swipe.py` - benchmarks through a
  virtual touchscreen, with no one at the phone: tapping GNOME Calculator,
  pulling the shades (`DRIVER=swipe`), or scrolling Settings (`DRIVER=scroll`)
- `tools/frames-run.sh`, `tools/tap-at.py` - one tap and the 40 frames after it,
  as they went to hwcomposer; `touch /tmp/item-shot` on the phone saves the next
  frame
- `log/` - what each step found, with the raw output
