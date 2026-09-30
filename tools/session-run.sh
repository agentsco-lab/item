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

rollback() {
    log "rollback"
    systemctl stop item-compositor 2>/dev/null
    pkill -9 -x item-compositor 2>/dev/null
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

chmod 755 /tmp/item-compositor
systemd-run --wait --collect --unit=item-compositor \
    -p User=$USER_NAME -p PAMName=phosh -p TTYPath=/dev/tty7 -p StandardInput=tty-fail \
    -p StandardOutput=file:/tmp/item-compositor.log -p StandardError=file:/tmp/item-compositor.log \
    -p RuntimeMaxSec=$T -p WorkingDirectory=/tmp -p ExecStartPre=+/usr/bin/chvt\ 7 \
    -p Environment=EGL_PLATFORM=hwcomposer \
    -p Environment=__EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/10_libhybris.json \
    -p Environment=XDG_RUNTIME_DIR=/run/user/$UID_N \
    -p Environment=DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$UID_N/bus \
    -p Environment=XDG_SESSION_TYPE=wayland \
    ${CLIENT_ENV:+-p "Environment=CLIENT_ENV=$CLIENT_ENV"} \
    /tmp/item-compositor "$@"
log "session ended"
