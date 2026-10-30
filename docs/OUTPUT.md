# item's own output

item draws each frame on the GPU and, by default, hands it to Android's
hwcomposer (through libhybris), which owns the display. Since 0.4 it can
also show the frame itself: the panels through the kernel's DRM, with no
hwcomposer at all. The two give the same picture, but the own output adds
the panels' factory colour and a shorter path to the screen.

```
sudo item-switch output kms     # item's own output, from now and after boots
sudo item-switch output hwc     # hwcomposer's again
sudo item-switch output         # which one is in use, which one is chosen
```

The choice is kept in `/etc/item/output` (`kms` or `hwc`, no file: `hwc`).

## What it gives

- **Colour.** The Duo's panels left the factory measured: their
  calibration is on the phone's `sfpd` partition (Qualcomm's
  `disp_user_calib_data_*` files, an sRGB and a Vivid mode a panel). Only
  the display's owner can load it into the colour hardware (IGC, PCC and
  GC, all three - without the gamma the greys lift), and under hwcomposer
  that owner is hwcomposer, which never did on the port: the panels ran
  raw, over-saturated, each its own white. The own output loads Vivid at
  start (`ITEM_COLOUR=srgb|vivid|off` to try the others), the two panels
  matched.
- **A shorter path.** The frame goes to the panels in one atomic commit
  for both, without blocking the compositor: present takes no time (it was
  hwcomposer's validate and present, 2.7 ms a frame and more with a second
  layer), the newest frame wins when one is late, and each buffer goes back
  to the GPU on the panels' own release fence.
- **The panels' power** on the lid: lit with their first frame in about
  0.76 s.

Measured on the owner's Duo 1: 60 frames a second steady, the same use as
under hwcomposer with a quarter of the missed vsyncs (2026-10-29); a night
asleep with the lid shut at about 1.0 % of the battery an hour, against
hwcomposer's 1.27 (2026-10-30).

## When it fails

hwcomposer stays the way back, every time:

- Before item starts, `item-composer-fresh` stops hwcomposer (Android's
  stop, forced: systemd's stop of it reset the phone) and waits until it is
  gone and the card is free.
- If the own output cannot open, refuses the first frame three times, or
  has a second of frames refused, item marks it failed for this boot
  (`$XDG_RUNTIME_DIR/item-kms-failed`) and leaves; systemd starts item
  again, through hwcomposer, which is started afresh.
- `item-switch output` clears the mark.

## Which phone

The device is read from the device tree at start (`/proc/device-tree/model`
and `compatible`), and each has a profile of its own (crates/hybris-hwc,
`profile.rs`): the gap between the panels, which calibration file is which
panel, the colour hardware's names, the bandwidth votes. Only the Duo 1
(SM8150) has one so far; anything else gets hwcomposer. The Duo 2 (SM8350,
two 1344×1892 panels up to 90 Hz) is laid for and needs its own reading.

## Not yet checked

- Long everyday use: days, not hours.

## Next

The votes for bandwidth computed from the frame rather than fixed; no
commit when nothing changed and partial updates (the panels keep their
picture); drawing as late as the next transfer allows; the window on an
overlay plane of its own; 3D LUTs (`.cube`) over the calibration; the
brightness in nits from the factory curves; the Duo 2's profile.

## How it is made

`crates/hybris-hwc`: `kms.rs` (the output: the card, the gralloc buffers
imported once each by their dmabuf, with UBWC compression as gralloc
flags it; the commit thread; the event thread, the DRM events' times taken
from CLOCK_BOOTTIME to the compositor's clock), `drm.rs` (DRM's ioctls by
hand, no libdrm), `colour.rs` (the calibration's files into the colour
hardware's blobs), `profile.rs` (the phones). What it learnt on the way,
each a dark screen or a refused commit until found:

- SDE waits on its own `input_fence` plane property, not on the standard
  `IN_FENCE_FD`; its release fence is the CRTC's `output_fence` (written
  back as 64 bits), about 8 ms after the transfer starts.
- A stopped hwcomposer leaves each CRTC voting the bus's whole bandwidth
  (22.8 GB/s): every commit after the first was refused ("exceeds
  bandwidth") until the votes were set to hwcomposer's running ones.
- Command-mode panels give no period in their timings: the mode's refresh
  does.
- The kernel holds a dead hwcomposer's mastership of the card a moment.
- The planes must be opaque.
- The panels on again by the CRTC's ACTIVE alone flip every frame and stay
  black: SDE assigns its blocks in the mode set, which ACTIVE alone does
  not run. Off, each CRTC is let go whole; on, the next frame sets the
  modes as the first one does.
- The DRM events' times are mostly CLOCK_BOOTTIME's, some
  CLOCK_MONOTONIC's: each is taken as the clock it is nearer to. Taking the
  sleep off them all, after a night (more slept than awake) some came to 0,
  and frames timed at the clock's start drew a call screen's fade-out over
  everything.
- One vblank asked for at a time: a second asked while one is on its way
  starts a chain of its own.
- hwcomposer, stopped, leaves the rotator's votes at the maximum too
  (460 MHz, 22.8 GB/s): set to 0.

The probe that found the way is `compositor/probes/kms-frame`
(`tools/kms-run.sh`).
