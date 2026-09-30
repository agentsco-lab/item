#!/bin/sh
# probe-run.sh SECONDS PROGRAM [ARGS...]: a probe that takes hwcomposer's
# display, run on the phone with the shell stopped, and the shell back after,
# whatever happens. Nothing is installed: the probe runs from where it lies.
T=${1:-20}; shift
SVC='(vendor.hwcomposer-.*|vendor.qti.hardware.display.composer)'
log() { echo "[$(date +%T.%N | cut -c1-12)] $*"; }
composers() { ps -eo comm | grep -c "composer@"; }
hwc() { ANDROID_SERVICE_FORCE_KILL=yes ANDROID_SERVICE="$SVC" /usr/lib/halium-wrappers/android-service.sh hwcomposer $1 >/dev/null 2>&1; }

rollback() {
    log "rollback"
    [ -n "$P" ] && kill -9 $P 2>/dev/null
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
EGL_PLATFORM=hwcomposer \
__EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/10_libhybris.json \
    timeout $((T + 20)) "$@" &
P=$!
wait $P
log "exit $?"
P=
