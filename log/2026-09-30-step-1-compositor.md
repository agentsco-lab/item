# Step 1: the probe becomes the compositor (2026-09-30)

`compositor/` (the `item-compositor` binary) is `probes/wl-panels` with its
parts in modules - `layout` (the panels and the hinge), `state` (the
Wayland protocol handlers), `input` (libinput and touch), `output`
(hwcomposer, EGL, the renderer, the output clients see) - and it runs in the
user's session, as phosh does: `tools/session-run.sh SECONDS ARGS` stops
the shell, restarts the Android composer clean, runs `/tmp/item-compositor`
through `systemd-run` as droidian with `PAMName=phosh` on tty7 and the
session's `XDG_RUNTIME_DIR` and bus, and brings phosh back whatever happens.
Nothing is installed. `--spawn COMMAND` starts a client, `--seconds N` stops
it.

## As the user it works

hwcomposer (binder is open to all), EGL, the touchscreen (droidian is in
android_input) and 60 fps, as the probes had it as root. GNOME Calculator
drawing through GL came up on the left panel as an EGL buffer
(`2026-09-30-session.log`).

## Clients start slowly in this session

GTK apps took 5-27 s to map, and GNOME Text Editor lost its bus connection
and quit. `dbus-monitor` on the session bus during a start: the app asks
D-Bus to start `org.freedesktop.portal.Desktop` (GLib does, so
`GDK_DEBUG=no-portals` does not stop it), and xdg-desktop-portal takes 8 to
25 s to answer - most likely waiting on backends picked for the phosh/GNOME
desktop the user's systemd still has in `XDG_CURRENT_DESKTOP`. The
accessibility bus held apps too (`GTK_A11Y=none` took one start from 26.6 to
5.4 s).

The compositor now gives its clients `EGL_PLATFORM=wayland` (not its own
hwcomposer platform), `GDK_DEBUG=no-portals`, `GTK_A11Y=none` and
`NO_AT_BRIDGE=1`, as a stopgap. The real fix is the session's: a desktop
name of its own with a portals configuration that picks a backend which
answers (gtk), and the accessibility bus up.
