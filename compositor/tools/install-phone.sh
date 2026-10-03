#!/bin/sh
# install-phone.sh [HOST]: item installed on the phone for good (item-tracker
# #94), from this tree's aarch64 build - in place of phosh, which stays as the
# fallback (item-fallback.service) and the other choice (item-switch).
#   - /usr/libexec/item: item-compositor, item-session, item-face, camera-warm
#     (built on the phone from tools/camera-warm.c)
#   - /usr/share/item/cvid: the models (tools/cvid-models.sh)
#   - the gnome-session session and its target's drop-in; item's XDG overlay
#     (the portals' choice; the port's old autostarts marked NotShowIn=item)
#   - item.service, item-fallback.service, item-face.service, item-face's
#     D-Bus policy, /usr/bin/item-switch
# The files written are listed in /usr/share/item/installed, for removal.
# It installs; `item-switch item` on the phone makes item the shell.
set -e
HOST=${1:-root@172.16.42.1}
HERE=$(cd "$(dirname "$0")/.." && pwd)
R=$HERE/target/aarch64-unknown-linux-gnu/release
STAGE=$(mktemp -d)
trap 'rm -rf $STAGE' EXIT
SSH="ssh -o UserKnownHostsFile=/dev/null -o StrictHostKeyChecking=no -o LogLevel=ERROR"

aarch64-linux-gnu-strip -o $STAGE/item-compositor $R/item-compositor
aarch64-linux-gnu-strip -o $STAGE/item-face $R/item-face
cp $HERE/tools/camera-warm.c $HERE/session/item-session $HERE/session/item-switch $HERE/session/item-composer-fresh \
   $HERE/session/item.session $HERE/session/item-portals.conf $HERE/session/org.sfduo.Face.conf \
   $HERE/session/item.service $HERE/session/item-fallback.service $HERE/session/item-face.service $STAGE/
cp $HERE/session/gnome-session@item.target.d/session.conf $STAGE/target-session.conf
cp $HERE/crates/cvid/models/*.onnx $STAGE/
cat > $STAGE/install.sh <<'INNER'
#!/bin/sh
set -e
cd "$(dirname "$0")"
USER_NAME=droidian
UID_N=$(id -u $USER_NAME)
LIST=/usr/share/item/installed
mkdir -p /usr/share/item
: > $LIST.new
put() { install -D -m "$1" "$2" "$3"; echo "$3" >> $LIST.new; }

put 755 item-compositor /usr/libexec/item/item-compositor
put 755 item-session /usr/libexec/item/item-session
put 755 item-composer-fresh /usr/libexec/item/item-composer-fresh
put 755 item-face /usr/libexec/item/item-face
gcc -O2 -o camera-warm camera-warm.c -lpthread -l:libgstreamer-1.0.so.0 -l:libgobject-2.0.so.0
put 755 camera-warm /usr/libexec/item/camera-warm
for m in *.onnx; do put 644 $m /usr/share/item/cvid/$m; done
put 644 item.session /usr/share/gnome-session/sessions/item.session
put 644 target-session.conf /usr/lib/systemd/user/gnome-session@item.target.d/session.conf
put 644 item-portals.conf /usr/share/item/xdg/xdg-desktop-portal/item-portals.conf
for a in sfduo-dock sfduo-system-screen sfduo-pen-screen sfduo-fingerprint sfduo-brightness; do
    [ -f /etc/xdg/autostart/$a.desktop ] || continue
    sed '/^NotShowIn=/d; /^\[Desktop Entry\]/a NotShowIn=item;' /etc/xdg/autostart/$a.desktop > $a.desktop
    put 644 $a.desktop /usr/share/item/xdg/autostart/$a.desktop
done
put 644 org.sfduo.Face.conf /usr/share/dbus-1/system.d/org.sfduo.Face.conf
sed "s/@USER@/$USER_NAME/g; s/@UID@/$UID_N/g" item.service > item.service.out
put 644 item.service.out /usr/lib/systemd/system/item.service
put 644 item-fallback.service /usr/lib/systemd/system/item-fallback.service
put 644 item-face.service /usr/lib/systemd/system/item-face.service
put 755 item-switch /usr/bin/item-switch
# The vendor composer: the port binds it to phosh.service (20-phosh.conf);
# with item it goes with whichever shell runs.
mkdir -p /etc/systemd/system/android-service@hwcomposer.service.d
cat > hwc.conf <<'CONF'
# item (item-switch): the vendor composer bound to no shell - the port's
# 20-phosh.conf (BindsTo=phosh.service) replaced. Stopped by systemd with a
# shell (BindsTo, then PartOf), it reset the phone one restart in two
# (2026-10-03); the shell's own start puts in a fresh one instead, as the
# test runs always did (item-composer-fresh), which never did.
[Unit]
Before=phosh.service item.service
CONF
put 644 hwc.conf /etc/systemd/system/android-service@hwcomposer.service.d/20-phosh.conf
mv $LIST.new $LIST
# On disk before anything restarts: a reset just after left the new files
# empty (2026-10-03), and an empty unit is a masked one.
sync
systemctl daemon-reload
busctl call org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus ReloadConfig >/dev/null
echo "installed: $(wc -l < $LIST) files; item-switch item makes item the shell"
INNER
chmod +x $STAGE/install.sh
$SSH $HOST 'rm -rf /tmp/item-install; mkdir -p /tmp/item-install'
scp -q -o UserKnownHostsFile=/dev/null -o StrictHostKeyChecking=no -o LogLevel=ERROR $STAGE/* $HOST:/tmp/item-install/
$SSH $HOST 'sh /tmp/item-install/install.sh && rm -rf /tmp/item-install'
