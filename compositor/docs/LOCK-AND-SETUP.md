# The lock, the first setup, and asking for secrets

How the item session asks who you are, decided 2026-10-01 after the first
runs of the full session showed three parts that knew nothing of each
other: the lock screen, gnome-keyring's prompt opening as an app's window on
a panel, and the keyboard coming up for it, or with no field at all. A
finger unlocked the session and left the keyring locked, so a password was
asked for after the person was already in, in a window of its own.

## Principles

- **The interface is in English**, every label, message and date.

- **Only the shell asks for a secret**: the lock screen or a system dialog
  drawn by the compositor, never a stray app window.
- **Ask once**: after a boot the PIN opens the session and the login
  keyring together (PAM's `phosh` service has `pam_gnome_keyring`).
- **The finger first, the PIN in reserve**, except the first unlock after a
  boot, which needs the PIN (a finger gives the keyring no secret).
- **Two panels, two roles**: the left one says who and what is going on,
  the right one is where you act (the PIN, the reader, the keyboard).

## After a boot

- The table, as for the lock (below). Left: the time as a title, the date,
  the battery; a greeting by the time of day and the name ("good morning,
  alex"); low down "pin after a restart".
- Right: the PIN pad, already open, its keys the drop's water on the
  squares.
- The right PIN unlocks, and the lock screen goes as the doors say
  (door.rs), the desktop under it, the dock rising. A wrong one: the pad
  shakes, its dots red, a buzz, "not this one".

## The lock

Since 2026-10-31 in the first setup's style (the owner's): the setup's
table over both panels, its squares the wallpaper's colour, dimmed; the
words a letter a square, lowercase; the setup's drop and its lamp.

- Left: the time as a title, the date and the battery under it (not the
  network's name: it says where the phone is); what is
  playing, its buttons as words a tap takes (`‹  play  ›`); the last
  notifications, app and title. Low down what is said: "swipe up" (no
  finger known), a passing "not this finger", "it's you, swipe up" after a
  known face. The words are there at once as the screen lights, typed only
  as they change.
- Right: the drop at the right edge, across from the power key (the reader
  is in it), a green lamp under it, breathing a while after the screen
  lights.
- A known finger: the drop flashes, the lock goes. An unknown one: the drop
  shakes, coral a moment, a buzz. After five failures, or a swipe up at
  any time, the drop goes to the panel's middle and becomes the PIN pad, as
  in the setup; a swipe down gathers the keys back into it, and it goes
  back to the reader.
- Unlocked, as the setup ends: the words typed away, the drop gone, the
  squares the wallpaper's whole colour; the view pulls back from where it
  was touched until each square is a pixel - the wallpaper, the desktop's
  own (`$XDG_RUNTIME_DIR/item-doors` tries the others: `fly`, `wave`,
  `slide`, `saloon`, `book`).

## The first setup (once)

1. **Welcome**: a motion across both panels (item's mark assembling over the
   hinge). Left "Welcome", right "Get started".
2. **The PIN**: left, why a PIN (after a boot, and in reserve); right, the
   pad. A new PIN twice, or the one there is, once.
3. **A finger**: left, a large fingerprint filling with each touch
   (`EnrollProgressChanged`); right, "Touch the power key, moving your
   finger a little each time", an arrow to the key. "Later" skips it.
4. **The tour**: a lesson per gesture. Left, a ghost finger shows it; right,
   "Try it"; it counts when done for real: the shade, the app grid, an
   app from the dock, putting a window away, the system screen, the pen's
   sheet, back from the edge. "Skip" at any time.
5. **"All set"**, and the doors open on the desktop.

## Asking while the session runs

The keyring's prompts (`org.gnome.keyring.SystemPrompter`), later the Wi-Fi
passwords and polkit's: a dialog of the compositor's. Left, who asks and
why; right, the field and the keyboard. The keyboard comes up only with a
field that takes text.

## Order

1. The lock, again: the two panels, the PIN on the right after a boot, the
   fingerprint mark, the doors, the count of failures.
2. The system dialog for the keyring; the keyboard only with a field.
3. The first setup: welcome, PIN, a finger. Changing the PIN and enrolling
   a finger change the phone's data: tried with its owner, or in a mode
   that changes nothing.
4. The tour.
