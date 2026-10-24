#!/bin/sh
# package-deb.sh: item as a Debian package, item_<version>_arm64.deb in
# target/deb - the files tools/install-phone.sh puts on the phone, for the
# release image (item-tracker #153) and updates by apt (#150). It installs;
# `item-switch item` makes item the shell, as after install-phone.sh.
#   - binaries built here for aarch64; camera-warm cross-built against a
#     stub of GStreamer (the phone's real one is loaded at run time)
#   - item.service for Droidian's user (droidian, uid 32011)
#   - the port's autostarts, marked NotShowIn=item, made by postinst from the
#     port's own files (they are the port's, and may change with it)
# A phone that had item from install-phone.sh moves to the package cleanly:
# preinst drops that install's list and the one file that moved.
set -e
HERE=$(cd "$(dirname "$0")/.." && pwd)
cd "$HERE"
TARGET=aarch64-unknown-linux-gnu
CARGO=${CARGO:-$(command -v cargo || echo "$HOME/.cargo/bin/cargo")}
"$CARGO" build --release --target $TARGET -p item-compositor -p item-face -p pen-split
# item Settings (#160), its own crate cross-built against a sysroot of
# trixie's GTK and libadwaita (apps/settings/tools/fetch-sysroot.sh).
SETTINGS="$HERE/../apps/settings"
[ -d "$SETTINGS/sysroot" ] || sh "$SETTINGS/tools/fetch-sysroot.sh"
(cd "$SETTINGS" && "$CARGO" build --release --target $TARGET)
R=target/$TARGET/release

BASE=$(sed -n 's/^version = "\(.*\)"/\1/p' compositor/Cargo.toml | head -1)
# The commit's time to the second first: a later build always sorts
# higher (the bare date and hash did not: 84d12ac "lower" than 88d8dd9).
VERSION="$BASE~git$(git log -1 --format=%cd.%h --date=format:%Y%m%d%H%M%S)"
[ -z "$(git status --porcelain -- compositor crates session tools ../apps/settings)" ] || VERSION="$VERSION.dirty"
# A release: the commit tagged item-v<version>, the tree clean - the bare
# version (dev builds after it want the next version in Cargo.toml: 0.2.0~git
# sorts below 0.2.0).
if [ "$(git tag --points-at HEAD -l "item-v$BASE")" = "item-v$BASE" ] && [ -z "$(git status --porcelain -- compositor crates session tools ../apps/settings)" ]; then
    VERSION=$BASE
fi
USER_NAME=droidian
UID_N=32011

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
S=$WORK/item
lib=$S/usr/libexec/item
mkdir -p "$lib" "$S/usr/bin" "$S/usr/share/item/cvid" "$S/DEBIAN"

for b in item-compositor item-face pen-split; do
    aarch64-linux-gnu-strip -o "$lib/$b" "$R/$b"
done
# The keyring's secrets in and out (secrets.rs), for the services the shell speaks to.
aarch64-linux-gnu-strip -o "$S/usr/bin/item-secret" "$R/item-secret"
# camera-warm declares the four GStreamer calls it makes itself: a stub
# with GStreamer's soname is all the link needs.
printf 'void gst_init(void){}\nvoid *gst_parse_launch(void){return 0;}\nint gst_element_set_state(void){return 0;}\nint gst_element_get_state(void){return 0;}\n' > "$WORK/gst.c"
aarch64-linux-gnu-gcc -shared -fPIC -Wl,-soname,libgstreamer-1.0.so.0 -o "$WORK/libgstreamer-1.0.so.0" "$WORK/gst.c"
aarch64-linux-gnu-gcc -O2 -o "$lib/camera-warm" tools/camera-warm.c -lpthread -L"$WORK" -l:libgstreamer-1.0.so.0
aarch64-linux-gnu-strip "$lib/camera-warm"
install -m755 session/item-session session/item-composer-fresh "$lib/"
install -m755 session/item-switch "$S/usr/bin/item-switch"
# The Agent entry: Claude Code in a terminal, the keyboard's vibe layer under it (kbd.rs).
install -m755 session/item-agent "$S/usr/bin/item-agent"
install -m755 session/item-agents "$S/usr/bin/item-agents"
install -m755 session/item-sync "$S/usr/bin/item-sync"
install -Dm644 session/item-agent-home.service "$S/usr/lib/systemd/user/item-agent-home.service"
install -d "$S/usr/lib/systemd/user/default.target.wants"
ln -s ../item-agent-home.service "$S/usr/lib/systemd/user/default.target.wants/item-agent-home.service"
# The dock's apps read into the page cache after boot (session/item-warm),
# on by default for every user through the wants link.
install -m755 session/item-warm "$S/usr/bin/item-warm"
install -Dm644 session/item-warm.service "$S/usr/lib/systemd/user/item-warm.service"
install -d "$S/usr/lib/systemd/user/default.target.wants"
ln -s ../item-warm.service "$S/usr/lib/systemd/user/default.target.wants/item-warm.service"
# Chromium kept resident, no window (session/item-chromium.service): the
# dock asks it for a window over its socket (state.rs's resident_chromium).
install -Dm644 session/item-chromium.service "$S/usr/lib/systemd/user/item-chromium.service"
ln -s ../item-chromium.service "$S/usr/lib/systemd/user/default.target.wants/item-chromium.service"
# Two idle burners off (draft 47): mtp-server masked for every user (ssh is
# how files move), cnss_diag stopped by a root unit after the container is up.
install -d "$S/etc/systemd/user"
ln -s /dev/null "$S/etc/systemd/user/mtp-server.service"
install -Dm644 session/item-quiet.service "$S/usr/lib/systemd/system/item-quiet.service"
install -d "$S/usr/lib/systemd/system/graphical.target.wants"
ln -s ../item-quiet.service "$S/usr/lib/systemd/system/graphical.target.wants/item-quiet.service"
install -Dm644 session/agent-home.md "$S/usr/share/item/agent/CLAUDE.md"
# The browser for links under item, Chromium: item-mimeapps.list, and its
# flags for Debian's wrapper (the Wayland socket, text-input v3).
install -Dm644 session/item-mimeapps.list "$S/etc/xdg/item-mimeapps.list"
install -Dm644 session/item-chromium "$S/etc/chromium.d/item"
install -Dm644 session/item-agent.desktop "$S/usr/share/applications/item-agent.desktop"
install -Dm644 session/item-agent.svg "$S/usr/share/icons/hicolor/scalable/apps/item-agent.svg"
install -Dm644 session/item-vibration-symbolic.svg "$S/usr/share/item/icons/vibration-symbolic.svg"
aarch64-linux-gnu-strip -o "$S/usr/bin/item-settings" "$SETTINGS/target/$TARGET/release/item-settings"
install -Dm644 "$SETTINGS/lab.agentsco.item.Settings.desktop" "$S/usr/share/applications/lab.agentsco.item.Settings.desktop"
install -Dm644 "$SETTINGS/lab.agentsco.item.Settings.svg" "$S/usr/share/icons/hicolor/scalable/apps/lab.agentsco.item.Settings.svg"
install -m644 crates/cvid/models/*.onnx "$S/usr/share/item/cvid/"
# Debian's copyright and changelog for the package (lintian: a copyright
# file, a changelog): the repository's LICENSE as the copyright, the tag's
# version as the changelog's one entry.
install -Dm644 ../LICENSE "$S/usr/share/doc/item/copyright"
printf 'item (%s) unstable; urgency=medium\n\n  * See https://github.com/agentsco-lab/item/releases for what is in it.\n\n -- Ivan Verbovoy <145046797+iverbovoy@users.noreply.github.com>  %s\n' "$VERSION" "$(date -R)" | gzip -9n > "$S/usr/share/doc/item/changelog.gz"
# The wallpapers with their credits (notes/drafts/30): the owner's set from
# WALLS (NAME.jpg with NAME.txt beside it, Unsplash's "Photo by ..." lines),
# each brought to 12 Mpx at most (walls.rs's SOURCE_PX: the picker's zoom
# and the memory want no more), its metadata dropped. None there: a phone
# starts with Aurora, drawn by the compositor.
WALLS="${WALLS:-$HOME/Desktop/walls/ready}"
if [ -d "$WALLS" ]; then
  mkdir -p "$S/usr/share/item/walls"
  for jpg in "$WALLS"/*.jpg; do
    n=$(basename "$jpg")
    case "$n" in thumb-*) continue ;; esac
    convert "$jpg" -strip -resize '12000000@>' -quality 90 "$S/usr/share/item/walls/$n"
    [ -f "${jpg%.jpg}.txt" ] && install -m644 "${jpg%.jpg}.txt" "$S/usr/share/item/walls/"
  done
  echo "wallpapers: $(ls "$S/usr/share/item/walls"/*.jpg | wc -l) pictures from $WALLS"
fi

install -Dm644 session/item.session "$S/usr/share/gnome-session/sessions/item.session"
install -Dm644 session/gnome-session@item.target.d/session.conf "$S/usr/lib/systemd/user/gnome-session@item.target.d/session.conf"
install -Dm644 session/item-portals.conf "$S/usr/share/item/xdg/xdg-desktop-portal/item-portals.conf"
install -Dm644 session/org.sfduo.Face.conf "$S/usr/share/dbus-1/system.d/org.sfduo.Face.conf"
mkdir -p "$S/usr/lib/systemd/system"
sed "s/@USER@/$USER_NAME/g; s/@UID@/$UID_N/g" session/item.service > "$S/usr/lib/systemd/system/item.service"
install -m644 session/item-fallback.service session/item-face.service "$S/usr/lib/systemd/system/"
# The digitizer split by item's pen-split (Rust) in place of the port's
# Python one, at a real-time priority (install-phone.sh had it in /etc).
mkdir -p "$S/usr/lib/systemd/system/sfduo-pen-split.service.d"
cat > "$S/usr/lib/systemd/system/sfduo-pen-split.service.d/50-item-rust.conf" <<'CONF'
[Service]
ExecStart=
ExecStart=/usr/libexec/item/pen-split
Environment=PEN_SOCK_OWNER=@UID@
CPUSchedulingPolicy=fifo
CPUSchedulingPriority=50
CONF
sed -i "s/@UID@/$UID_N/" "$S/usr/lib/systemd/system/sfduo-pen-split.service.d/50-item-rust.conf"
# The vendor composer bound to no shell: the port's 20-phosh.conf
# (BindsTo=phosh.service, from phosh-config-hwcomposer) replaced by the same
# name in /etc - see install-phone.sh for why.
mkdir -p "$S/etc/systemd/system/android-service@hwcomposer.service.d"
cat > "$S/etc/systemd/system/android-service@hwcomposer.service.d/20-phosh.conf" <<'CONF'
# item: the vendor composer bound to no shell - phosh-config-hwcomposer's
# 20-phosh.conf (BindsTo=phosh.service) replaced. Stopped by systemd with a
# shell (BindsTo, then PartOf), it reset the phone one restart in two
# (2026-10-03); the shell's own start puts in a fresh one instead
# (item-composer-fresh).
[Unit]
Before=phosh.service item.service
CONF
printf '%s\n' /etc/systemd/system/android-service@hwcomposer.service.d/20-phosh.conf /etc/xdg/item-mimeapps.list /etc/chromium.d/item > "$S/DEBIAN/conffiles"

INSTALLED=$(du -sk "$S" --exclude=DEBIAN | cut -f1)
cat > "$S/DEBIAN/control" <<CONTROL
Package: item
Version: $VERSION
Architecture: arm64
Maintainer: Ivan Verbovoy <145046797+iverbovoy@users.noreply.github.com>
Section: x11
Priority: optional
Installed-Size: $INSTALLED
Depends: libc6, libgcc-s1, libinput10, libxkbcommon0, libgstreamer1.0-0, libgtk-4-1, libadwaita-1-0, gnome-session-bin, gnome-settings-daemon, phosh, chromium, gnome-console, tmux, sudo, python3, wget, curl, pulseaudio-utils, libglib2.0-bin, adaptation-droidian-surfaceduo (>= 0.21)
Recommends: rsync, openssh-client, gnome-calls, chatty, gnome-weather
Homepage: https://github.com/agentsco-lab/item
Description: item, the two-panel shell for the Surface Duo
 item-compositor and its session in place of phosh on Droidian: a dock
 across both panels, windows by panel, the system screen, the pen's sheet,
 face unlock (item-face), the phone's sleep. phosh stays as the fallback;
 item-switch changes between them.
CONTROL

cat > "$S/DEBIAN/preinst" <<'SCRIPT'
#!/bin/sh
set -e
# From install-phone.sh to the package: its list goes, and the one file the
# package keeps elsewhere.
if [ "$1" = install ] || [ "$1" = upgrade ]; then
    if [ -f /usr/share/item/installed ]; then
        # The composer drop-in was item's own file there: without it here,
        # dpkg would ask about a changed conffile (and fail with no terminal).
        rm -f /usr/share/item/installed /etc/systemd/system/sfduo-pen-split.service.d/50-item-rust.conf \
            /etc/systemd/system/android-service@hwcomposer.service.d/20-phosh.conf
        rmdir /etc/systemd/system/sfduo-pen-split.service.d 2>/dev/null || true
    fi
fi
SCRIPT

cat > "$S/DEBIAN/postinst" <<'SCRIPT'
#!/bin/sh
set -e
if [ "$1" = configure ]; then
    # The port's autostarts, kept out of item's session (NotShowIn=item) by
    # copies in item's XDG overlay, made from the port's current files.
    mkdir -p /usr/share/item/xdg/autostart
    : > /var/lib/item-autostart.list.new
    for a in sfduo-posture sfduo-brightness sfduo-fingerprint sfduo-dock sfduo-system-screen sfduo-pen-screen; do
        [ -f /etc/xdg/autostart/$a.desktop ] || continue
        sed '/^NotShowIn=/d; /^\[Desktop Entry\]/a NotShowIn=item;' /etc/xdg/autostart/$a.desktop > /usr/share/item/xdg/autostart/$a.desktop
        echo /usr/share/item/xdg/autostart/$a.desktop >> /var/lib/item-autostart.list.new
    done
    mv /var/lib/item-autostart.list.new /var/lib/item-autostart.list
    # On disk before anything restarts (an empty unit is a masked one).
    sync
    if [ -d /run/systemd/system ]; then
        systemctl daemon-reload || true
        # Already the shell (an upgrade): what item-switch item masks, masked.
        if systemctl is-enabled -q item.service 2>/dev/null; then
            systemctl mask --now iio-sensor-proxy.service 2>/dev/null || true
        fi
        busctl call org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus ReloadConfig >/dev/null 2>&1 || true
    fi
fi
SCRIPT

cat > "$S/DEBIAN/prerm" <<'SCRIPT'
#!/bin/sh
set -e
# Removed while it is the shell: phosh back first, so the phone has one.
if [ "$1" = remove ] && [ -d /run/systemd/system ] && systemctl is-enabled -q item.service 2>/dev/null; then
    item-switch phosh || true
fi
# The user's own units of item's stopped (Chromium resident, the warm-up,
# the agent's session), where a user manager runs.
if [ "$1" = remove ] && [ -d /run/systemd/system ]; then
    for u in $(loginctl list-users --no-legend 2>/dev/null | awk '$2 != "root" {print $1}'); do
        runuser -u "#$u" -- env XDG_RUNTIME_DIR=/run/user/$u systemctl --user stop item-chromium.service item-warm.service item-agent-home.service 2>/dev/null || true
    done
fi
SCRIPT

cat > "$S/DEBIAN/postrm" <<'SCRIPT'
#!/bin/sh
set -e
if [ "$1" = remove ] || [ "$1" = purge ]; then
    if [ -f /var/lib/item-autostart.list ]; then
        xargs rm -f < /var/lib/item-autostart.list
        rm -f /var/lib/item-autostart.list
    fi
    rmdir --ignore-fail-on-non-empty -p /usr/share/item/xdg/autostart 2>/dev/null || true
    [ -d /run/systemd/system ] && systemctl daemon-reload || true
fi
SCRIPT
chmod 755 "$S/DEBIAN/preinst" "$S/DEBIAN/postinst" "$S/DEBIAN/prerm" "$S/DEBIAN/postrm"

mkdir -p target/deb
OUT=target/deb/item_${VERSION}_arm64.deb
dpkg-deb --root-owner-group -Zxz --build "$S" "$OUT" >/dev/null
echo "$OUT"
