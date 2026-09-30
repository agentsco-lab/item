#!/bin/sh
# fetch-sysroot.sh [HOST]: the phone's shared libraries a cross build links
# against (smithay needs libxkbcommon even with its fewest features), into
# sysroot/lib, with the unversioned names the linker looks for.
H=${1:-root@172.16.42.1}
D=$(dirname "$0")/../sysroot/lib
mkdir -p "$D"
for l in libxkbcommon.so.0; do
    scp -q "$H:/usr/lib/aarch64-linux-gnu/$l" "$D/" && ln -sf "$l" "$D/${l%%.so*}.so"
done
ls -la "$D"
