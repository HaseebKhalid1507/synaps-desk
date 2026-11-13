#!/bin/bash
# Run one `synaps-desktop --bench` on the logged-in X11/Wayland session of this
# machine, memory-capped, and append its JSON to bench-results/.
# usage: scripts/bench-linux.sh <binary> [extra synaps-desktop args...]
set -euo pipefail
bin="$1"; shift
# Borrow the graphical session's display env from its window manager / shell.
for name in i3 sway Hyprland kwin_wayland kwin_x11 plasmashell gnome-shell; do
  pid=$(pgrep -u "$USER" -x "$name" | head -n1 || true)
  [ -n "$pid" ] && break
done
[ -n "${pid:-}" ] || { echo "no graphical session found" >&2; exit 1; }
while IFS= read -r -d '' kv; do
  case "$kv" in DISPLAY=*|WAYLAND_DISPLAY=*|XDG_RUNTIME_DIR=*|XAUTHORITY=*|XDG_SESSION_TYPE=*|DBUS_SESSION_BUS_ADDRESS=*) export "$kv" ;; esac
done < "/proc/$pid/environ"
out="${BENCH_OUT:-$HOME/.cache/synaps-desktop-bench}"; mkdir -p "$out"
# Optionally wait (up to BENCH_QUIET_WAIT s) for the 1-minute load average to drop under BENCH_QUIET_LOAD.
if [ -n "${BENCH_QUIET_LOAD:-}" ]; then
  for _ in $(seq 1 "${BENCH_QUIET_WAIT:-600}"); do
    awk -v max="$BENCH_QUIET_LOAD" '{exit !($1 < max)}' /proc/loadavg && break
    sleep 1
  done
fi
load_before=$(cut -d' ' -f1-3 /proc/loadavg); rustc_before=$(pgrep -c rustc || true)
stamp=$(date +%Y%m%d-%H%M%S)
launch_ns=$(date +%s%N)
# Record whether the WM floated the window (i3: dialogs float, nothing re-tiles).
( sleep 4; command -v i3-msg >/dev/null && i3-msg -t get_tree 2>/dev/null | python3 -c '
import json,sys
def walk(n, fl=False):
    fl = fl or n.get("type") == "floating_con"
    if (n.get("window_properties") or {}).get("class") == "synaps-desktop" or n.get("name") == "Synaps":
        print("floating" if fl else "tiled")
    for c in n.get("nodes", []) + n.get("floating_nodes", []): walk(c, fl)
walk(json.load(sys.stdin))' > "$out/.wm" 2>/dev/null ) &
systemd-run --user --scope --quiet -p MemoryMax=2G -p MemorySwapMax=0 \
  timeout 35 "$bin" --bench "$@" > "$out/$stamp.json" 2> "$out/$stamp.err" || echo "exit=$?" >> "$out/$stamp.err"
wait
load_after=$(cut -d' ' -f1-3 /proc/loadavg)
python3 - "$out/$stamp.json" "$launch_ns" "$load_before" "$load_after" "${rustc_before:-0}" "$(cat "$out/.wm" 2>/dev/null)" <<'PY'
import json, sys
p, launch = sys.argv[1], int(sys.argv[2]) / 1e6
try:
    d = json.load(open(p))
except Exception as e:
    print("no JSON:", e); sys.exit(1)
if d.get("first_frame_epoch_ms"):
    d["launch_to_first_frame_ms"] = round(d["first_frame_epoch_ms"] - launch, 1)
d["host"] = {"loadavg_before": sys.argv[3], "loadavg_after": sys.argv[4], "rustc_procs_before": int(sys.argv[5] or 0), "wm_placement": sys.argv[6] or None}
json.dump(d, open(p, "w"), indent=1)
print(p)
PY
tail -n 5 "$out/$stamp.err" >&2 || true
