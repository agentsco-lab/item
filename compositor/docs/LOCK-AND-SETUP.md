# The lock, the first setup, and asking for secrets

How the item session asks who you are, decided 2026-10-01 after the first
runs of the full session showed three parts that knew nothing of each
other: the lock screen, gnome-keyring's prompt opening as an app's window on
a panel, and the keyboard coming up for it, or with no field at all. A
finger unlocked the session and left the keyring locked, so a password was
asked for after the person was already in, in a window of its own.

## Principles

- **Only the shell asks for a secret**: the lock screen or a system dialog
  drawn by the compositor, never a stray app window.
- **Ask once**: after a boot the PIN opens the session and the login
  keyring together (PAM's `phosh` service has `pam_gnome_keyring`).
- **The finger first, the PIN in reserve**, except the first unlock after a
  boot, which needs the PIN (a finger gives the keyring no secret).
- **Two panels, two roles**: the left one says who and what is going on,
  the right one is where you act (the PIN, the reader, the keyboard).

## After a boot

- Left: the time, large; the date; a greeting by the time of day and the
  name ("Доброе утро, Иван"); "После перезагрузки нужен PIN".
- Right: the PIN pad, already open.
- The right PIN unlocks, and the two halves open as doors: the left panel's
  half slides out to the left, the right one's to the right, the desktop
  under them, the dock rising. A wrong one: the pad shakes, a buzz.

## The lock

- Left: the time and the date.
- Right: a fingerprint mark at the right edge, across from the power key
  (the reader is in it), pulsing softly while the reader listens, and
  "Коснитесь кнопки питания".
- A known finger: the mark flashes, the doors open. An unknown one: the
  mark shakes red, a buzz, "Не узнан". After five failures the PIN pad
  slides in on the right; a swipe up brings it at any time.

## The first setup (once)

1. **Welcome**: a motion across both panels (item's mark assembling over the
   hinge). Left "Добро пожаловать", right "Начать".
2. **The PIN**: left, why a PIN (after a boot, and in reserve); right, the
   pad. A new PIN twice, or the one there is, once.
3. **A finger**: left, a large fingerprint filling with each touch
   (`EnrollProgressChanged`); right, "Касайтесь кнопки питания, меняя
   положение пальца", an arrow to the key. "Позже" skips it.
4. **The tour**: a lesson per gesture. Left, a ghost finger shows it; right,
   "Попробуйте"; it counts when done for real: the shade, the app grid, an
   app from the dock, putting a window away, the system screen, the pen's
   sheet, back from the edge. "Пропустить" at any time.
5. **"Всё готово"**, and the doors open on the desktop.

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
