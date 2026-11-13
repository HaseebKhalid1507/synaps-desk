#!/bin/bash
# Screenshot synaps-desktop on this machine's logged-in X11 session, then close
# the window (always, also on failure). The window opens as a dialog
# (SYNAPS_DESKTOP_FLOAT=1), so a tiling WM floats it instead of re-tiling, and
# the capture is of that window only.
# usage: scripts/shot-linux.sh <binary> <out.png> <delay-seconds> [synaps-desktop args...]
set -euo pipefail
bin="$1"; out="$2"; delay="$3"; shift 3
for name in i3 sway Hyprland kwin_wayland kwin_x11 plasmashell gnome-shell; do
  wm=$(pgrep -u "$USER" -x "$name" | head -n1 || true)
  [ -n "$wm" ] && break
done
[ -n "${wm:-}" ] || { echo "no graphical session found" >&2; exit 1; }
while IFS= read -r -d '' kv; do
  case "$kv" in DISPLAY=*|WAYLAND_DISPLAY=*|XDG_RUNTIME_DIR=*|XAUTHORITY=*|XDG_SESSION_TYPE=*|DBUS_SESSION_BUS_ADDRESS=*) export "$kv" ;; esac
done < "/proc/$wm/environ"
mkdir -p "$(dirname "$out")"
SYNAPS_DESKTOP_FLOAT=1 timeout 60 "$bin" "$@" > "$out.log" 2>&1 &
pid=$!
trap 'kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true' EXIT
wid=""
for _ in $(seq 1 150); do
  app=$(pgrep -P "$pid" | head -n1 || true)
  [ -n "$app" ] && wid=$(xdotool search --pid "$app" 2>/dev/null | tail -n1 || true)
  [ -z "$wid" ] && wid=$(xdotool search --classname '^synaps-desktop$' 2>/dev/null | tail -n1 || true)
  [ -n "$wid" ] && break
  sleep 0.1
done
[ -n "$wid" ] || { echo "no window for pid $pid" >&2; cat "$out.log" >&2; exit 1; }
# SHOT_WORKSPACE=<name> (i3): show it on that workspace instead of the focused
# one, e.g. a visible workspace on another monitor when the focused one is
# covered by a fullscreen window. Focus stays where it was.
if [ -n "${SHOT_WORKSPACE:-}" ] && command -v i3-msg >/dev/null; then
  for _ in $(seq 1 50); do
    i3-msg -q "[id=$wid] move container to workspace $SHOT_WORKSPACE" 2>/dev/null && break
    sleep 0.1
  done
  i3-msg -q "[id=$wid] move position center" 2>/dev/null || true
fi
sleep "$delay"
# The window's own pixels (XGetImage on it), never the screen around it.
for _ in $(seq 1 20); do
  import -window "$wid" "$out" 2>/dev/null && break
  sleep 0.25
done
[ -s "$out" ] || { echo "could not capture window $wid" >&2; exit 1; }
echo "$out $(xdotool getwindowgeometry "$wid" | tr '\n' ' ')"
