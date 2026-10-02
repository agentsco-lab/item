#!/bin/sh
# session-run.sh SECONDS [ARGS...]: item-compositor (from /tmp/item-compositor)
# as the user in a login session on tty7, as phosh.service runs phosh: the
# shell stopped first, and back after, whatever happens. Nothing is installed.
# The compositor's log is /tmp/item-compositor.log.
#
# SESSION=1: the whole session as item.service will run it - session/item-
# session (from /tmp) starting gnome-session --session=item - with its files
# in /tmp and the user's runtime systemd directory, all gone after: the
# session file, its target's drop-in, and copies of the old GTK dock's,
# system screen's and pen sheet's autostart files, and sfduo-fingerprint's,
# marked NotShowIn=item (the compositor has its own of each; the package
# marks the originals so).
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
    $UB systemctl --user set-environment XDG_CURRENT_DESKTOP=item:GNOME WAYLAND_DISPLAY=wayland-item \
        XDG_CONFIG_DIRS=/tmp/item-session/xdg:/etc/xdg
    $UB dbus-update-activation-environment XDG_CURRENT_DESKTOP=item:GNOME WAYLAND_DISPLAY=wayland-item \
        XDG_CONFIG_DIRS=/tmp/item-session/xdg:/etc/xdg
    log "session environment: item, wayland-item, portals: gtk"
}

RUNTIME_UNITS=/run/user/$UID_N/systemd/user
AUTOSTART_OLD="sfduo-dock sfduo-system-screen sfduo-pen-screen sfduo-fingerprint"

session_files() {
    mkdir -p /tmp/item-session/share/gnome-session/sessions $RUNTIME_UNITS/gnome-session@item.target.d
    cp /tmp/item.session /tmp/item-session/share/gnome-session/sessions/
    cp /tmp/item-session.conf $RUNTIME_UNITS/gnome-session@item.target.d/session.conf
    cp /tmp/item-portals.conf /tmp/item-session/xdg/xdg-desktop-portal/item-portals.conf
    chown -R $USER_NAME $RUNTIME_UNITS; chmod -R a+rX /tmp/item-session
    $UB systemctl --user daemon-reload
    mkdir -p /tmp/item-session/xdg/autostart
    for a in $AUTOSTART_OLD; do
        sed '/^NotShowIn=/d; /^\[Desktop Entry\]/a NotShowIn=item;' /etc/xdg/autostart/$a.desktop > /tmp/item-session/xdg/autostart/$a.desktop
    done
    chmod -R a+rX /tmp/item-session
    log "session files: item.session, gnome-session@item.target, old autostarts not shown in item"
}

session_files_restore() {
    $UB systemctl --user stop gnome-session@item.target 2>/dev/null
    rm -rf $RUNTIME_UNITS/gnome-session@item.target.d
    $UB systemctl --user daemon-reload
    log "session files removed"
}

session_env_restore() {
    $UB systemctl --user stop $PORTALS 2>/dev/null
    $UB systemctl --user unset-environment $SESSION_VARS
    [ -n "$SAVED" ] && $UB systemctl --user set-environment $SAVED
    [ -n "$SAVED" ] && $UB dbus-update-activation-environment $SAVED
    log "session environment restored: $(echo $SAVED)"
}

# The GPU's minimum clock: the compositor's boost writes it (boost.rs). The
# user may for the run; the file's user and the old value come back after.
KG=/sys/class/kgsl/kgsl-3d0/devfreq/min_freq
KG_OLD=$(cat $KG 2>/dev/null)
# The big cores' minimum clocks, as the GPU's (boost.rs).
CPU_MINS="/sys/devices/system/cpu/cpufreq/policy4/scaling_min_freq /sys/devices/system/cpu/cpufreq/policy7/scaling_min_freq"
CPU_OLD=""
for f in $CPU_MINS; do CPU_OLD="$CPU_OLD $(cat $f 2>/dev/null)"; done

rollback() {
    log "rollback"
    systemctl stop item-compositor 2>/dev/null
    pkill -9 -x item-compositor 2>/dev/null
    [ -n "$SESSION" ] && session_files_restore
    session_env_restore
    set -- $CPU_OLD
    for f in $CPU_MINS; do
        [ -n "$1" ] && { chown root $f; echo $1 > $f; }
        shift
    done
    log "CPU min clocks restored: $(cat $CPU_MINS | tr '\n' ' ')"
    if [ -n "$KG_OLD" ]; then
        chown root $KG; echo $KG_OLD > $KG
        log "GPU min clock restored: $(cat $KG)"
    fi
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
[ -n "$SESSION" ] && session_files
[ -n "$KG_OLD" ] && chown $USER_NAME $KG
for f in $CPU_MINS; do chown $USER_NAME $f 2>/dev/null; done

chmod 755 /tmp/item-compositor
START="/tmp/item-compositor"
[ -n "$SESSION" ] && { chmod 755 /tmp/item-session.sh; START="/tmp/item-session.sh"; }
rm -f /tmp/item-compositor.log   # systemd writes a file: output from its start, not truncating
systemd-run --wait --collect --unit=item-compositor \
    -p User=$USER_NAME -p PAMName=phosh -p TTYPath=/dev/tty7 -p StandardInput=tty-fail -p LimitRTPRIO=10 -p "AmbientCapabilities=CAP_WAKE_ALARM CAP_BLOCK_SUSPEND" -p SupplementaryGroups=android_wakelock \
    -p StandardOutput=file:/tmp/item-compositor.log -p StandardError=file:/tmp/item-compositor.log \
    -p RuntimeMaxSec=$T -p WorkingDirectory=/tmp -p ExecStartPre=+/usr/bin/chvt\ 7 \
    -p Environment=EGL_PLATFORM=hwcomposer \
    -p Environment=__EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/10_libhybris.json \
    -p Environment=XDG_RUNTIME_DIR=/run/user/$UID_N \
    -p Environment=DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$UID_N/bus \
    -p Environment=XDG_SESSION_TYPE=wayland -p Environment=XDG_CURRENT_DESKTOP=item:GNOME \
    ${CLIENT_ENV:+-p "Environment=CLIENT_ENV=$CLIENT_ENV"} \
    ${LATE:+-p "Environment=LATE=$LATE"} ${LATE_MARGIN_MS:+-p "Environment=LATE_MARGIN_MS=$LATE_MARGIN_MS"} \
    ${HWC_PRESENT_OR_VALIDATE:+-p "Environment=HWC_PRESENT_OR_VALIDATE=$HWC_PRESENT_OR_VALIDATE"} \
    ${TOUCHSCREEN:+-p "Environment=TOUCHSCREEN=$TOUCHSCREEN"} ${SETUP:+-p "Environment=SETUP=$SETUP"} ${SETUP_DRY:+-p "Environment=SETUP_DRY=$SETUP_DRY"} ${SETUP_AUTO:+-p "Environment=SETUP_AUTO=$SETUP_AUTO"} ${NIGHT:+-p "Environment=NIGHT=$NIGHT"} ${FRAMES_EVERY:+-p "Environment=FRAMES_EVERY=$FRAMES_EVERY"} ${FRAMES_DOCK:+-p "Environment=FRAMES_DOCK=$FRAMES_DOCK"} ${DIALOG_TEST:+-p "Environment=DIALOG_TEST=$DIALOG_TEST"} ${CALL_TEST:+-p "Environment=CALL_TEST=$CALL_TEST"} ${CALL_UNLOCKED:+-p "Environment=CALL_UNLOCKED=1"} ${EDGE_SOFT:+-p "Environment=EDGE_SOFT=$EDGE_SOFT"} ${EDGE_STRENGTH:+-p "Environment=EDGE_STRENGTH=$EDGE_STRENGTH"} ${EDGE_CORNER:+-p "Environment=EDGE_CORNER=$EDGE_CORNER"} ${FRAMES_FULL:+-p "Environment=FRAMES_FULL=1"} ${FRAMES_TOP:+-p "Environment=FRAMES_TOP=1"} ${IDLE_DELAY:+-p "Environment=IDLE_DELAY=$IDLE_DELAY"} ${TOUR:+-p "Environment=TOUR=1"} ${NO_SUSPEND:+-p "Environment=NO_SUSPEND=1"} ${FRAMES_COUNT:+-p "Environment=FRAMES_COUNT=$FRAMES_COUNT"} ${LOG_WAVE:+-p "Environment=LOG_WAVE=$LOG_WAVE"} ${PACE_DEBUG:+-p "Environment=PACE_DEBUG=$PACE_DEBUG"} ${LOG_TIMES:+-p "Environment=LOG_TIMES=$LOG_TIMES"} ${CANVAS:+-p "Environment=CANVAS=$CANVAS"} ${LOG_BUFFERS:+-p "Environment=LOG_BUFFERS=$LOG_BUFFERS"} ${INSTANCING:+-p "Environment=INSTANCING=$INSTANCING"} ${LOG_DAMAGE:+-p "Environment=LOG_DAMAGE=$LOG_DAMAGE"} ${PARTIAL:+-p "Environment=PARTIAL=$PARTIAL"} ${NO_LOGIN_SHELL:+-p "Environment=NO_LOGIN_SHELL=$NO_LOGIN_SHELL"} ${SERVER_DEBUG:+-p "Environment=WAYLAND_DEBUG=server"} \
    ${RUST_LOG:+-p "Environment=RUST_LOG=$RUST_LOG"} ${ASAP:+-p "Environment=ASAP=$ASAP"} ${GPU_BOOST:+-p "Environment=GPU_BOOST=$GPU_BOOST"} ${CALLBACKS:+-p "Environment=CALLBACKS=$CALLBACKS"} \
    ${OSK_DEBUG:+-p "Environment=OSK_DEBUG=$OSK_DEBUG"} ${NO_OSK:+-p "Environment=NO_OSK=$NO_OSK"} ${NO_OSK_RESIZE:+-p "Environment=NO_OSK_RESIZE=$NO_OSK_RESIZE"} \
    ${SESSION:+-p "Environment=ITEM_COMPOSITOR=/tmp/item-compositor"} \
    ${SESSION:+-p "Environment=XDG_DATA_DIRS=/tmp/item-session/share:/usr/local/share:/usr/share"} \
    ${SESSION:+-p "Environment=XDG_CONFIG_DIRS=/tmp/item-session/xdg:/etc/xdg"} \
    $START "$@"
log "session ended"
