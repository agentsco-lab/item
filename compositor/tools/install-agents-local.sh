#!/bin/sh
# install-agents-local.sh: item-agents and item-sync for this computer
# (~/.local/bin), built here - the computer's Claude Code calls
# `item-agents hook` from its settings, and item-sync sends the work to the
# phone. The phone gets them in item's package.
set -eu
cd "$(dirname "$0")/.."
CARGO=${CARGO:-$(command -v cargo || echo "$HOME/.cargo/bin/cargo")}
"$CARGO" build --release -p item-agents
mkdir -p "$HOME/.local/bin"
for b in item-agents item-sync; do
    # Through a new name and a rename: a running hook is never left with
    # half a file.
    install -m755 "target/release/$b" "$HOME/.local/bin/.$b.new"
    mv -f "$HOME/.local/bin/.$b.new" "$HOME/.local/bin/$b"
done
echo "item-agents and item-sync in ~/.local/bin"
