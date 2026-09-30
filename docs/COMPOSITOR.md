# item's own compositor

**Decided 2026-10-01: item-shell's base becomes a compositor of its own**,
`item-compositor` (Rust, smithay), drawing through Android's hwcomposer on
the Surface Duo 1 with the shell inside it, in place of item on the port's
phosh and phoc. Until it has what is listed under "Before it replaces
phosh", item-shell 0.1.x on phosh stays the shell in use, and phosh stays
the way back after.

The work so far, step by step with its measurements, is in
`compositor/log/` (steps 1-22, 2026-09-30 to 2026-10-01); the probes that
chose the path are in agentsco-lab/duo-lindroid (private).

## Why a compositor of our own

Smoothness and speed matter most: every part of the shell
should move as the shade does. Where they come from:

1. **The frame's path.** Nothing between the GPU's frame and hwcomposer.
2. **Where the shell is drawn.** item on phosh is a set of GTK processes
   beside the compositor: the dock, the system screen, the pen's sheet each
   go touch → phoc → the client → its buffer → phoc → a frame, and phosh's
   shade and grid are phosh's. Drawn inside the compositor, in the same pass
   as the windows, the dock, the shades, the grid and the screens at the
   edges follow the finger with no round trip.
3. **When a frame is started** against the vsync: the compositor's own
   pacing, not a patch to phoc's.

And freedom: item on phosh is 36 patches to phoc, phosh and the
keyboard, moved by hand at every release of theirs. item-compositor
changes nobody's code.

## Why not the Lindroid chain

Droidian's "lindroid" display stack (a virtual DRM device, `create-disp`
handing its frames to hwcomposer) would let stock compositors run: sway,
SwayFX, KWin, phoc 0.57. It was brought up on the Duo's 4.14 kernel
(agentsco-lab/duo-lindroid (private), 2026-09-30), and measured:

| | the chain | hwcomposer directly (item-compositor) |
|---|---|---|
| each frame | copied by the CPU, 9-13 ms, 0.76 W at 60 fps | not copied |
| both panels animating | 30 fps (cage) | 60 fps |
| GPU sync | none through the chain: half-drawn frames (SwayFX) | EGL's swap and hwcomposer's fences |
| latency | a frame more (a copy, a second present) | the base |
| our code in the stack | a kernel module (6 patches), create-disp (6), libhybris (1), scenefx (1) | the compositor |

Its freedom to pick any compositor is paid for in exactly what matters
most here (duo-lindroid's `log/2026-09-30-choosing-a-base.md`).

## Measured against item on phosh

The same phone, the same night, timed below both compositors: a uprobe on
libhybris' `hwc2_compat_display_present`, the call both make to hand
hwcomposer a frame, and a probe client both serve alike
(`compositor/log/2026-10-01-step-21-ab.md`, `-step-22-canvas.md`).

| | item on phosh | item-compositor |
|---|---|---|
| the shade pulled back up | 6 gaps over 25 ms each time, up to 54 ms | 0-1, up to 29 ms |
| the app grid going down | a 235-243 ms freeze each time | up to 34 ms |
| the shade down, the grid up | at the vsync, 1-2 gaps | at the vsync, 1-3 gaps |
| a tap, touch to the present with its content | 17.6 ms (p90 24.4) | 6.2 ms (p90 7.9) |
| the shell at rest: memory (PSS) | 461 MB (phoc, phosh, dock, system screen, pen) | 173 MB |
| the shell at rest: CPU | 3.5 % of a core | 2.6 % |

Each of the 20 steps was also tried by hand on the phone.

A tap is timed from the touch to the present that holds the client's new
frame. phoc presents twice to each tap: a frame of its own on the touch,
then the one with the client's content; item-compositor presents once
(`compositor/log/2026-10-01-step-22-canvas.md`).

## What it has (2026-10-01)

The dock in two halves with item's motion (contact, bump, the neck across
the hinge), windows tiled to a panel and moved through the hinge, put away
with a swipe up and called back, the launch curtain, the app grid, the
desktop clock, back from the edge, the two shades with their settings,
windows and notifications (a notification server of its own), the
on-screen keyboard (the port's stevia), the power key, blanking, the lock
screen with the PIN (PAM, as phosh), the volume keys and bar, the lid, the
pen as a finger, the system screen, the pen's sheet, the GPU boost on touch,
`wp_presentation`.

## Before it replaces phosh

- The polkit agent, and the prompts phosh gives today: Wi-Fi passwords
  (NetworkManager's secret agent), the keyring (gcr's system prompter).
- Calls: an incoming call over the lock screen, the call's own screen.
- item's services: posture (folded, flat, one panel), fingerprint unlock,
  automatic brightness.
- Suspend and wake; the screen's idle timeout.
- The weather under the clock, the status icons in the shade's head, the
  grid's long press (to the dock and off it), running apps in the dock, the
  privacy dot.
- No vsync while at rest; launches in an app scope of their own (GTK is
  refused its portal now).
- Installing as a session of its own beside phosh, with the way back: a
  package, a greeter entry or a switch like `sfduo-shell`, and a fall back
  to phosh if it does not start.

Each goes in as a step in `compositor/log/`, measured and tried by
hand, as the first 22 were.
