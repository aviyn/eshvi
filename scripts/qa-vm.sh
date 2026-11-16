#!/bin/sh
# Hands-on QA in a macOS VM (Tart), driven over VNC from this Mac: real
# mouse clicks and keys, screenshots, nothing on the host's own screen.
#   scripts/qa-vm.sh start          boot the VM (headless; VNC on localhost)
#   scripts/qa-vm.sh deploy [BIN]   bundle a build and install it in the VM
#   scripts/qa-vm.sh launch [ARGS]  start Eshvi in the VM (nvim ARGS after --)
#   scripts/qa-vm.sh shot FILE      screenshot of the VM's screen
#   scripts/qa-vm.sh vnc CMDS...    move X Y, click 1|3, key K, type TEXT,
#                                   pause S, capture FILE (scripts/qa-vnc.py)
#   scripts/qa-vm.sh type TEXT      type text (shifted characters too)
#   scripts/qa-vm.sh exec CMD...    run a command in the VM (user admin)
#   scripts/qa-vm.sh stop
# Needs: tart (~/.local/bin), vncdotool in ~/.cache/eshnv-qa-tools (a venv),
# the VM "eshnv-qa" (tart clone ghcr.io/cirruslabs/macos-tahoe-base:latest
# eshnv-qa) with neovim (brew) inside.
set -eu
vm=${QA_VM:-eshnv-qa}
tools=$HOME/.cache/eshnv-qa-tools
share=$HOME/.cache/eshnv-qa
log=$tools/tart-run.log
root=$(cd "$(dirname "$0")/.." && pwd)

sock=$tools/vnc.sock

vnc() {
    # Through the one connection qa-vnc.py keeps (started with the VM).
    [ -S "$sock" ] || { echo "no VNC helper: start the VM first" >&2; exit 1; }
    "$tools/bin/python3" -W ignore "$root/scripts/qa-vnc.py" send "$sock" "$@"
}

cmd=${1:-}
[ $# -gt 0 ] && shift
case $cmd in
start)
    mkdir -p "$share"
    nohup tart run "$vm" --no-graphics --vnc-experimental --dir="qa:$share" >"$log" 2>&1 &
    for _ in $(seq 1 60); do tart exec "$vm" true 2>/dev/null && break; sleep 3; done
    tart exec "$vm" true || { echo "the VM didn't come up" >&2; exit 1; }
    # The VNC server's address and one-time password, from Tart's log.
    url=$(sed -n 's#.*\(vnc://[^ ]*\).*#\1#p' "$log" | tail -1)
    nohup "$tools/bin/python3" -W ignore "$root/scripts/qa-vnc.py" serve "$url" "$sock" >"$tools/vnc.log" 2>&1 &
    for _ in $(seq 1 20); do grep -q ready "$tools/vnc.log" 2>/dev/null && exit 0; sleep 1; done
    echo "the VNC helper didn't connect: $tools/vnc.log" >&2; exit 1 ;;
deploy)
    bin=${1:-${CARGO_TARGET_DIR:-$root/target}/release/eshvi}
    "$root/scripts/bundle-macos.sh" "$bin" "$share" >/dev/null 2>&1
    # Through the exec channel: the shared folder goes stale in the VM when
    # the bundle is replaced on this side.
    tar -C "$share" -cf - Eshvi.app | tart exec -i "$vm" sh -c \
        'pkill -x eshvi; rm -rf ~/Applications/Eshvi.app; mkdir -p ~/Applications && tar -C ~/Applications -xf -'
    echo "deployed $(date +%H:%M:%S)" ;;
launch)
    tart exec "$vm" sh -c 'pkill -x eshvi; true'
    # Through `open`, like a click in the Finder: launchd's PATH and the
    # app's login-shell PATH lookup, as users get it.
    tart exec "$vm" sh -c 'open -n ~/Applications/Eshvi.app ${1+--args -- "$@"}' sh "$@" ;;
shot)
    vnc capture "${1:?file}" ;;
vnc)
    vnc "$@" ;;
type)
    vnc type "${1:?text}" ;;
exec)
    tart exec -i "$vm" "$@" ;;
stop)
    pkill -f "qa-vnc.py serve" || true
    tart stop "$vm" ;;
*)
    sed -n '2,15p' "$0"; exit 2 ;;
esac
