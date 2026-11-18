#!/bin/bash
# tip.sh: offline demo 1280x720 nord: Ctrl+K /workers Enter mid-turn (the note), then the pointer on the note: its tooltip.
set -uo pipefail
cd ~/.cache/q6r3; mkdir -p raw stills
export XDG_RUNTIME_DIR=/run/user/$(id -u)
n=tip; out=raw/$n; rm -rf $out
cap=~/Projects/.worktrees/synaps-desktop-q6/scripts/capture-linux.sh
CAPTURE_HOVER=0 CAPTURE_SIZE=1280x720 CAPTURE_SETTINGS='{"text_size": 16, "theme": "nord"}' bash $cap bin/r3 $out 14 --offline > $out.log 2>&1 &
cpid=$!
for _ in $(seq 1 100); do [ -s $out/display ] && break; sleep 0.1; done
export DISPLAY=$(cat $out/display)
for _ in $(seq 1 100); do wid=$(xdotool search --classname '^synaps-desktop$' 2>/dev/null | tail -n1); [ -n "$wid" ] && break; sleep 0.1; done
sleep 4
log=$out-drive.log; : > $log; t0=$(date +%s.%N); echo "t0 $t0" >> $log
mark() { echo "$(awk "BEGIN{print $(date +%s.%N) - $t0}") still $1" >> $log; import -window $wid stills/$n-$1.png; }
k() { echo "$(awk "BEGIN{print $(date +%s.%N) - $t0}") $*" >> $log; xdotool "$@"; }
k mousemove 640 300; sleep 0.3
k key ctrl+k; sleep 0.6; k type --delay 80 "workers"; sleep 0.6; mark palette
k key Return; sleep 0.8; mark note
for x in 640 700 760; do k mousemove $x 672; sleep 1.5; mark hover-$x; done
wait $cpid
echo "tip done $(date +%T)"
