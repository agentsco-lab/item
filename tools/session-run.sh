#!/bin/sh
# session-run.sh SECONDS [ARGS...]: item-compositor (from /tmp/item-compositor)
# as the user in a login session on tty7, as phosh.service runs phosh: the
# shell stopped first, and back after, whatever happens. Nothing is installed.
# The compositor's log is /tmp/item-compositor.log.
T=${1:-60}; shift
USER_NAME=droidian
UID_N=$(id -u $USER_NAME)
SVC='(vendor.hwcomposer-.*|vendor.qti.hardware.display.composer)'
log() { echo "[$(date +%T.%N | cut -c1-12)] $*"; }
composers() { ps -eo comm | grep -c "composer@"; }
hwc() { ANDROID_SERVICE_FORCE_KILL=yes ANDROID_SERVICE="$SVC" /usr/lib/halium-wrappers/android-service.sh hwcomposer $1 >/dev/null 2>&1; }

# The user's systemd and D-Bus activation environment: what the portals and
# other session services start with. phosh's session leaves its desktop name
# and its (then dead) Wayland socket there; ours goes in for the run, and the
# old values come back after. The portals' configuration lives in /tmp only.
UB="runuser -u $USER_NAME -- env XDG_RUNTIME_DIR=/run/user/$UID_N DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$UID_N/bus"
SESSION_VARS="XDG_CURRENT_DESKTOP XDG_CONFIG_DIRS WAYLAND_DISPLAY"
PORTALS="xdg-desktop-portal.service xdg-desktop-portal-gtk.service xdg-desktop-portal-phosh.service xdg-desktop-portal-phrosh.service xdg-desktop-portal-gnome.service"
SAVED=$($UB systemctl --user show-environment | grep -E "^(XDG_CURRENT_DESKTOP|XDG_CONFIG_DIRS|WAYLAND_DISPLAY)=")

session_env() {
    mkdir -p /tmp/item-session/xdg/xdg-desktop-portal
    printf '[preferred]\ndefault=gtk\n' > /tmp/item-session/xdg/xdg-desktop-portal/item-portals.conf
    chmod -R a+rX /tmp/item-session
    $UB systemctl --user stop $PORTALS 2>/dev/null
    $UB systemctl --user set-environment XDG_CURRENT_DESKTOP=item WAYLAND_DISPLAY=wayland-item \
        XDG_CONFIG_DIRS=/tmp/item-session/xdg:/etc/xdg
    $UB dbus-update-activation-environment XDG_CURRENT_DESKTOP=item WAYLAND_DISPLAY=wayland-item \
        XDG_CONFIG_DIRS=/tmp/item-session/xdg:/etc/xdg
    log "session environment: item, wayland-item, portals: gtk"
}

session_env_restore() {
    $UB systemctl --user stop $PORTALS 2>/dev/null
    $UB systemctl --user unset-environment $SESSION_VARS
    [ -n "$SAVED" ] && $UB systemctl --user set-environment $SAVED
    [ -n "$SAVED" ] && $UB dbus-update-activation-environment $SAVED
    log "session environment restored: $(echo $SAVED)"
}

rollback() {
    log "rollback"
    systemctl stop item-compositor 2>/dev/null
    pkill -9 -x item-compositor 2>/dev/null
    session_env_restore
    sleep 1
    hwc stop
    systemctl unmask --runtime phosh >/dev/null 2>&1
    systemctl start phosh sfduo-composer-watchdog
    sleep 8
    log "phosh $(systemctl is-active phosh)"
}
trap rollback EXIT

systemctl stop sfduo-composer-watchdog phosh
systemctl mask --runtime phosh >/dev/null
hwc stop
for i in $(seq 1 30); do [ "$(composers)" = 0 ] && break; sleep 0.5; done
hwc start
for i in $(seq 1 40); do [ "$(composers)" -gt 0 ] && break; sleep 0.25; done
log "composer up: $(composers)"
session_env

chmod 755 /tmp/item-compositor
systemd-run --wait --collect --unit=item-compositor \
    -p User=$USER_NAME -p PAMName=phosh -p TTYPath=/dev/tty7 -p StandardInput=tty-fail \
    -p StandardOutput=file:/tmp/item-compositor.log -p StandardError=file:/tmp/item-compositor.log \
    -p RuntimeMaxSec=$T -p WorkingDirectory=/tmp -p ExecStartPre=+/usr/bin/chvt\ 7 \
    -p Environment=EGL_PLATFORM=hwcomposer \
    -p Environment=__EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/10_libhybris.json \
    -p Environment=XDG_RUNTIME_DIR=/run/user/$UID_N \
    -p Environment=DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$UID_N/bus \
    -p Environment=XDG_SESSION_TYPE=wayland -p Environment=XDG_CURRENT_DESKTOP=item \
    ${CLIENT_ENV:+-p "Environment=CLIENT_ENV=$CLIENT_ENV"} \
    /tmp/item-compositor "$@"
log "session ended"
