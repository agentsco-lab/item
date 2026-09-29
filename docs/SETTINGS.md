# Settings

The device came with three settings programs: GNOME Settings (gnome-control-
center 48), Mobile Settings (phosh-mobile-settings) and Droidian's
mobile-settings service, which has no window. Neither of the two with a
window carried any of the port's own settings, and GNOME Settings offered
pages for hardware this phone does not have.

`sfduo-settings` ("Settings" in the grid) is the one list now, grouped for
this device: Connections, Screen, Sound and Notifications, Surface Duo,
Security, Apps and Accounts, System, For Developers. Each row opens its page
in whichever program has it, or a page of its own. The page is asked for on
the session bus - GNOME Settings' `launch-panel` action, Mobile Settings'
`set-panel`, both taking the panel and its arguments - not by running the
program again. Everything stays on the panel Settings was opened on: before
a row opens its page, Settings asks the dock to put that program's window on
its own panel (`org.sfduo.Dock.Follow(app_id, leader)`), so the page comes
up over the list and the other panel is left as it was; a window of that
program already open on the other panel is moved across. The Surface Duo
page says what is installed; its switches are #20.

## A page in a tenth of a second

A page used to cost over a second, every time. Settings ran
`gnome-control-center <panel>`: a whole GTK process started to hand a panel
name to the instance already running, and then exit. Measured on 2026-09-20,
from the tap on a row to the page's first frame: 1180 ms that way, 1070 ms
for the same page asked for on the bus with the program not running, and
35-60 ms for the call with the program running, the page drawn 120 ms after
the tap. So the programs are asked on the bus, and they are kept running
while this window is open - closing it tells them to quit, which matters
because GNOME Settings is 145 MB.

Back from a page puts its window away rather than closing it (the wrapper
takes Alt+Left before the program's own navigation sees it, so the window is
put away on the page it is showing, not on the program's first screen), and
the program waits there for the next page.

Bringing that window back is the compositor's (`org.sfduo.Phoc.Present`,
phoc-patches/0009), and it is asked for before the page is: a window put
away is told it is suspended and its client stops drawing, so it reaches the
page only once it is on the screen - the eye would catch the page it was put
away on while the new one is built. Present wakes the client with nothing to
see and shows the window once it has stopped drawing. From the tap to the
page, one motion: 320-430 ms, or 190 ms when the page asked for is the one
it already has.

A cold start is still a second and a half, and that is what the stand-in is
for: Settings answers the tap in its own motion - a page titled as the one
coming, with a spinner if the wait passes half a second - and the program's
window lands on it, drawn for the first time already on its panel: the
compositor holds a new window until it is placed and settled
(phoc-patches/0010), so the dock covers nothing for a page any more. If the
window never comes, the stand-in goes by itself after 8 s.

The list comes up with nothing lit. A window that opens hands the keyboard
to the first thing that will take it, and a row holding the keyboard is
drawn as chosen - Wi-Fi came up highlighted and the highlight went out at
the first touch - so the window drops the focus when it is shown and
whenever it is the active window again.

Left out, on purpose, and why:

| Page | Why |
|---|---|
| Displays | phosh applies a scale set there live, over phoc.ini's 2, and the dock and the shell's CSS are made for phoc.ini's |
| Printers, Remote desktop, Thunderbolt, Device security | nothing to do on this phone |
| NFC | `nfcd` does not start and there is no NFC device |
| Waydroid | not installed (#21) |
| Encryption | not tried with this boot chain; a device that cannot unlock its root cannot boot |
| Mobile Settings: alerts, convergence | alerts are broken here (its schema is missing), convergence is an external display, never tested |

GNOME Settings and Mobile Settings stay installed and leave the grid by
`NoDisplay` overrides in `/usr/local/share/applications`, not `Hidden` ones -
a hidden entry is gone for launching by id too, and phosh and the dock
launch Settings by id. Settings' SSH page cannot be reached from its command
line in 48 (`system secure-shell` lands on System), so that row opens System.

Both old programs are split views that show the list and the page side by
side above a width (550sp and 500sp); a window on one panel is 675 logical
px, so both did. `sfduo-one-column`, installed as
`/usr/local/bin/gnome-control-center` and `/usr/local/bin/phosh-mobile-settings`
and named by D-Bus service files in `/usr/local/share/dbus-1/services` (both
are D-Bus activated), takes the window's `.ui` from the installed binary at
launch, raises the line to 900sp and serves it through `G_RESOURCE_OVERLAYS`:
list, then page, on one panel; two columns spanned across both. It
re-extracts when either the binary or this script changes - the cache is
stamped with both, followed through the symlink it is started by - and runs
the program untouched if the line is not in the file any more. A session bus that started before the
service directory existed needs `org.freedesktop.DBus.ReloadConfig` or a new
login.

Their own lists are not the way in any more, Settings is. Back puts the
window away and leaves Settings showing underneath: the rewritten `.ui`
takes Alt+Left - what the dock's swipe from the edge sends - on a shortcut
controller in the capture phase and answers it with `window.minimize`,
before the program's own navigation sees it. So the window is put away on
the page it is showing, and the program stays: the next page it is asked
for is a switch inside it, and asked for the page it already has there is
nothing to change at all. Going back through the program's first screen
instead - an empty column here - meant coming back on that screen, with the
page arriving after. The column still holds a single "‹ Settings" button
(`window.minimize`) for the moment it is on screen. The list is still in the
`.ui`, hidden (the programs' code holds on to it), with the search and menu
buttons above it. The same launch-time rewrite does it, with Python's XML
parser rather than sed; a file of an unexpected shape gets the width change
only.

While Settings runs, the dock shows no button of its own for a window that
follows it: GNOME Settings over Settings was a second gear beside the
first.

