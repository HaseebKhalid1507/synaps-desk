#!/bin/bash
# Idle CPU of a synaps-desktop window: launch it on this machine's logged-in X11
# session (as a floating dialog), let it settle, then count the CPU time it
# uses while nobody touches it. Closes the window on exit, also on failure.
# usage: scripts/idle-cpu-linux.sh <binary> <settle-s> <measure-s> [synaps-desktop args...]
set -euo pipefail
bin="$1"; settle="$2"; measure="$3"; shift 3
for name in i3 sway Hyprland kwin_wayland kwin_x11 plasmashell gnome-shell; do
  wm=$(pgrep -u "$USER" -x "$name" | head -n1 || true)
  [ -n "$wm" ] && break
done
[ -n "${wm:-}" ] || { echo "no graphical session found" >&2; exit 1; }
while IFS= read -r -d '' kv; do
  case "$kv" in DISPLAY=*|WAYLAND_DISPLAY=*|XDG_RUNTIME_DIR=*|XAUTHORITY=*|XDG_SESSION_TYPE=*|DBUS_SESSION_BUS_ADDRESS=*) export "$kv" ;; esac
done < "/proc/$wm/environ"
SYNAPS_DESKTOP_FLOAT=1 "$bin" "$@" > /dev/null 2>&1 &
pid=$!
trap 'kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true' EXIT
sleep "$settle"
ticks() { awk '{print $14 + $15}' "/proc/$pid/stat"; }
hz=$(getconf CLK_TCK)
t0=$(ticks); w0=$(date +%s.%N)
sleep "$measure"
t1=$(ticks); w1=$(date +%s.%N)
rss=$(awk '/^VmRSS/ {print $2}' "/proc/$pid/status")
threads=$(ls "/proc/$pid/task" | wc -l)
python3 -c "import sys; t=(int(sys.argv[2])-int(sys.argv[1]))/int(sys.argv[3]); w=float(sys.argv[5])-float(sys.argv[4]); print(f'idle_cpu_pct={100*t/w:.3f} cpu_s={t:.2f} wall_s={w:.1f} rss_kb={sys.argv[6]} threads={sys.argv[7]} args={sys.argv[8]!r}')" "$t0" "$t1" "$hz" "$w0" "$w1" "$rss" "$threads" "$*"
