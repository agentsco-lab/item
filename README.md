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

## First, two probes

| | | |
|---|---|---|
| A | smithay's anvil on the Lindroid chain (duo-lindroid's test image): do smithay's EGL and GLES run on libhybris? | |
| B | a minimal Rust program that puts a GL frame on the Duo's panels through libhybris' hwc2, at vsync: the core of the backend | |

What each must answer - the seams between smithay and libhybris, not the
language:

- **The EGL display.** On the chain (A) libhybris has GBM (libgbm-hybris),
  as KWin and wlroots used. Without the chain (B) there is no GBM: smithay
  needs its own `EGLNativeDisplay` (the Android platform or
  `EGL_DEFAULT_DISPLAY`) and `EGLNativeSurface` over libhybris' hwcomposer
  native window.
- **Clients' buffers.** On the port's own path GL apps hand buffers over
  `android_wlegl`, which libhybris' EGL serves after
  `eglBindWaylandDisplayWL`: smithay's `bind_wl_display` (feature
  `use_system_lib`) must work with it, and the buffers must import as
  textures (`EGL_WAYLAND_BUFFER_WL`), or GL apps will not show. On the chain
  clients use linux-dmabuf with libhybris' metadata fd instead
  (duo-lindroid, `log/2026-09-30-probe-plasma-mobile.md`).
- **Sync.** On the port's path a frame goes to hwcomposer through EGL's swap
  with its fence; the chain has none (duo-lindroid idea 20).

If either fails, or the size turns out too large, the fallback is SwayFX
with item as a Rust daemon over sway's IPC and layer-shell clients
(duo-lindroid, `log/2026-09-30-probe-swayfx.md`).

## What the Duo gives a compositor (known from duo-lindroid)

- One hwcomposer display, 2784x1800: two 1350x1800 panels and 84 columns
  under the hinge between them. The composer takes only a client target
  (device layers came back changed to client composition).
- One touchscreen over both panels and the hinge: X 0-17709, Y 0-11411.
- Adreno 640 through libhybris: GLES 3.2, EGL 1.5. Its GLSL ES compiler is
  strict (an undefined name in `#if` is an error).
- The port's scale is 2.
