#!/bin/bash
# Build item-shell_<ver>_arm64.deb into out/: the two-panel shell for the
# Surface Duo 1, on top of the port (adaptation-droidian-surfaceduo, from
# agentsco-lab/surfaceduo-droidian), which runs Droidian's own phosh on the two
# panels by itself.
#
# What this adds: the dock, the system screen, the pen's sheet, the port's
# Settings, the shell's CSS, and builds of phoc, phosh and the keyboard with
# the port's patches and the shell's (patches/). The port's install scripts
# put those builds in place: they take /usr/lib/item/<what>/<version>/ over
# the port's own while it is there, and fall back to the port's builds when
# this package is removed.
#
# The builds themselves are made on the phone (docs/SHELL.md) and copied to
# out/<what>/ before this runs; a missing one is left out with a note, and
# the port's build of it stays in place.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
VER="${1:-0.1.0}"
PORT_MIN=0.21.0
OUT="$ROOT/out"
PKG="$OUT/pkgroot"

rm -rf "$PKG"
mkdir -p "$PKG/DEBIAN" "$PKG/usr/local/bin" "$PKG/usr/local/sbin" \
         "$PKG/etc/xdg/autostart"

# ---- the shell's own programs -----------------------------------------------

# The dock: a strip across both panels that launches onto the panel touched
# and tiles windows to a panel. It autostarts with the session.
install -m755  "$ROOT/shell/sfduo-dock"                 "$PKG/usr/local/bin/"
install -m644  "$ROOT/shell/sfduo-dock.desktop"         "$PKG/etc/xdg/autostart/"
install -Dm644 "$ROOT/shell/dock.json"                  "$PKG/usr/share/item/dock.json.example"
# The system screen: the page left of the left panel, brought by a swipe
# right on its desktop (the dock catches it). The pen's sheet likewise.
install -m755  "$ROOT/shell/sfduo-system-screen"         "$PKG/usr/local/bin/"
install -m644  "$ROOT/shell/sfduo-system-screen.desktop" "$PKG/etc/xdg/autostart/"
install -m755  "$ROOT/shell/sfduo-pen-screen"            "$PKG/usr/local/bin/"
install -m644  "$ROOT/shell/sfduo-pen-screen.desktop"    "$PKG/etc/xdg/autostart/"
# sfduo-shell: the shell and the output scale as two switches; Settings runs
# it through pkexec, which the policy names.
install -m755  "$ROOT/shell/sfduo-shell"                 "$PKG/usr/local/sbin/"
install -Dm644 "$ROOT/shell/org.sfduo.shell.policy"      "$PKG/usr/share/polkit-1/actions/org.sfduo.shell.policy"

# The shell's frame times, measured the same way every time: it drives the
# dock and moves the port's synthetic finger (sfduo-touch); the port's docs/PERF.md.
install -m755  "$ROOT/tools/sfduo-perfcheck"             "$PKG/usr/local/sbin/"

# ---- Settings -------------------------------------------------------------

# sfduo-settings is the one Settings in the grid: the pages of GNOME Settings
# and Mobile Settings grouped for this device, one column on one panel. The
# two are still there under their own ids, launched through
# sfduo-one-column, first in PATH and in the D-Bus service directory.
install -m755  "$ROOT/apps/sfduo-settings"               "$PKG/usr/local/bin/"
install -Dm644 "$ROOT/apps/org.sfduo.Settings.desktop"   "$PKG/usr/local/share/applications/org.sfduo.Settings.desktop"
install -Dm644 "$ROOT/apps/org.gnome.Settings.desktop"   "$PKG/usr/local/share/applications/org.gnome.Settings.desktop"
install -Dm644 "$ROOT/apps/mobi.phosh.MobileSettings.desktop" "$PKG/usr/local/share/applications/mobi.phosh.MobileSettings.desktop"
install -Dm755 "$ROOT/apps/sfduo-one-column"             "$PKG/usr/local/lib/item/sfduo-one-column"
ln -sf /usr/local/lib/item/sfduo-one-column "$PKG/usr/local/bin/gnome-control-center"
ln -sf /usr/local/lib/item/sfduo-one-column "$PKG/usr/local/bin/phosh-mobile-settings"
install -Dm644 "$ROOT/apps/org.gnome.Settings.service"   "$PKG/usr/local/share/dbus-1/services/org.gnome.Settings.service"
install -Dm644 "$ROOT/apps/mobi.phosh.MobileSettings.service" "$PKG/usr/local/share/dbus-1/services/mobi.phosh.MobileSettings.service"

# ---- phoc: windows tile, they do not fill the display --------------------

# phosh writes auto-maximize=true on every start. The value is here; the lock
# that keeps it is the port's sfduo-phosh-install's to write, and it writes
# it only while the dock can run - with no dock to put windows on a panel,
# every window would stay at its own size.
install -Dm644 "$ROOT/dconf/50-sfduo-phoc" "$PKG/etc/dconf/db/local.d/50-sfduo-phoc"

# ---- the shell's CSS -------------------------------------------------------

# Appended by the port's sfduo-shell-css after its own rules, with the same
# numbers filled in for the output scale in use.
install -Dm644 "$ROOT/css/item.css.in" "$PKG/usr/share/item/css/item.css.in"

# ---- the patched builds ----------------------------------------------------

# One build per Droidian release, under the exact version of the package it
# stands in for; the port's install scripts take the one that matches what
# is installed and leave anything else alone.
carry() {   # carry DIR BINARY SOURCE VERSION
    if [ -f "$3" ]; then
        install -Dm755 "$3" "$PKG/usr/lib/item/$1/$4/$2"
    else
        echo "NOTE: $3 not found - building without it (the port's build stays in place)"
    fi
}
# Droidian 102
carry phoc phoc "$OUT/phoc/phoc-0.47.0-7e682c6-item" \
    "0.47.0-1~git20260824230949.7e682c6.next.phosh.0.47"
carry phosh phosh "$OUT/phosh/phosh-0.55.0-bee1861-item" \
    "0.55.0+git20260824233009.bee1861.next.phosh.0.55"
carry osk/phosh-osk-stevia phosh-osk-stevia "$OUT/osk/phosh-osk-stevia-0.55.0-cfcbe7a-item" \
    "0.55.0+git20260824233138.cfcbe7a.next.phosh.0.55"

echo "$VER" > "$PKG/usr/share/item/version"

# ---- control -----------------------------------------------------------------

cat > "$PKG/DEBIAN/control" <<EOF
Package: item-shell
Version: $VER
Architecture: arm64
Maintainer: Ivan Verbovoy <145046797+iverbovoy@users.noreply.github.com>
Section: x11
Priority: optional
Depends: adaptation-droidian-surfaceduo (>= $PORT_MIN), python3-gi, python3-gi-cairo, python3-cairo, gir1.2-gtk-3.0, gir1.2-gtklayershell-0.1, wlrctl, wtype, dconf-cli, gir1.2-gtk-4.0, gir1.2-gtk4layershell-1.0, libgtk4-layer-shell0, gir1.2-adw-1, gir1.2-ecal-2.0, gir1.2-edataserver-1.2, pkexec | policykit-1
Description: two-panel shell for the Surface Duo 1 on Droidian
 A dock across both panels that launches onto the panel touched and tiles
 windows to a panel, a system screen, the pen's sheet and a Settings of one
 column, with builds of phoc, phosh and the on-screen keyboard patched for
 them. Runs on top of adaptation-droidian-surfaceduo; removing it gives
 Droidian's own shell on the two panels back.
EOF

# The port's scripts do the placing, both ways: installed, the builds above
# are the ones they put in place; removed, they fall back to the port's own
# and drop the auto-maximize lock, since the dock has gone.
apply='
command -v dconf >/dev/null 2>&1 && dconf update || true
/usr/local/sbin/sfduo-phosh-install || true
/usr/local/sbin/sfduo-phoc-install || true
/usr/local/sbin/sfduo-osk-install || true
/usr/local/sbin/sfduo-shell-css || true
'

cat > "$PKG/DEBIAN/postinst" <<EOF
#!/bin/sh
set -e
$apply
# the dock writes its config as the user; made by root it could not
if id droidian >/dev/null 2>&1; then
    H=\$(getent passwd droidian | cut -d: -f6)
    for d in "\$H/.config" "\$H/.config/sfduo"; do
        [ -d "\$d" ] || install -d -o droidian -g droidian "\$d"
    done
fi
echo "item-shell: restart the shell to start it (sudo systemctl restart phosh)"
EOF
chmod 755 "$PKG/DEBIAN/postinst"

cat > "$PKG/DEBIAN/postrm" <<EOF
#!/bin/sh
set -e
if [ "\$1" = remove ] || [ "\$1" = purge ]; then
$apply
    echo "item-shell: removed - restart the shell for Droidian's own (sudo systemctl restart phosh)"
fi
EOF
chmod 755 "$PKG/DEBIAN/postrm"

dpkg-deb --build --root-owner-group "$PKG" "$OUT/item-shell_${VER}_arm64.deb"
rm -rf "$PKG"
echo "OK: $OUT/item-shell_${VER}_arm64.deb"
