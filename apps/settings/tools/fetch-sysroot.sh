#!/bin/sh
# fetch-sysroot.sh: the phone's userland to cross-build item Settings
# against - Debian trixie's arm64 GTK 4, libadwaita and glibc, into
# sysroot/ (not tracked). No phone needed: an amd64 trixie container
# installs the :arm64 packages through multiarch (natively, ~2 min; apt
# under qemu took over 10) and hands back /usr/lib/aarch64-linux-gnu,
# the pkg-config files and the headers.
set -e
D=$(cd "$(dirname "$0")/.." && pwd)/sysroot
rm -rf "$D" && mkdir -p "$D"
docker run --rm --platform linux/amd64 -v "$D:/out" debian:trixie sh -c '
    set -e
    dpkg --add-architecture arm64
    apt-get update -qq
    DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends \
        libgtk-4-dev:arm64 libadwaita-1-dev:arm64 libc6-dev:arm64 >/dev/null 2>&1
    mkdir -p /out/usr/lib /out/usr/share
    cp -a /usr/lib/aarch64-linux-gnu /out/usr/lib/
    cp -a /usr/share/pkgconfig /out/usr/share/
    cp -a /usr/include /out/usr/
    ln -s usr/lib /out/lib
    # libc.so names the loader at /lib, where arm64 itself keeps a link.
    ln -s aarch64-linux-gnu/ld-linux-aarch64.so.1 /out/usr/lib/ld-linux-aarch64.so.1
    # One pkg-config directory (cargo cannot put two relative paths in a
    # variable): the arch-independent files beside the arm64 ones.
    for f in /out/usr/share/pkgconfig/*.pc; do
        [ -e /out/usr/lib/aarch64-linux-gnu/pkgconfig/${f##*/} ] || ln -s ../../../share/pkgconfig/${f##*/} /out/usr/lib/aarch64-linux-gnu/pkgconfig/
    done
    dpkg-query -W -f "\${Package} \${Version}\n" libgtk-4-1:arm64 libadwaita-1-0:arm64 libc6:arm64 > /out/VERSIONS
    chown -R '"$(id -u):$(id -g)"' /out'
cat "$D/VERSIONS"
du -sh "$D"
