#!/bin/bash
# design.sh <bin> <theme> <size> <name>: the offline demo, the palette driven by keys (opened at launch,
# GO TO); the drive log's times → stills. 30 fps lossless.
set -uo pipefail
cd ~/.cache/q6r3; mkdir -p raw stills
b=$1 t=$2 s=$3 n=$4; out=raw/$n; rm -rf $out
cap=~/Projects/.worktrees/synaps-desktop-q6/scripts/capture-linux.sh
CAPTURE_HOVER=0 CAPTURE_SIZE=$s CAPTURE_SETTINGS="{\"text_size\": 16, \"theme\": \"$t\"}" \
  bash $cap bin/$b $out 20 --offline --palette goto > $out.log 2>&1 &
cpid=$!
for _ in $(seq 1 100); do [ -s $out/display ] && break; sleep 0.1; done
export DISPLAY=$(cat $out/display)
for _ in $(seq 1 100); do wid=$(xdotool search --classname '^synaps-desktop$' 2>/dev/null | tail -n1); [ -n "$wid" ] && break; sleep 0.1; done
sleep 4
log=$out-drive.log; : > $log
t0=$(date +%s.%N); echo "t0 $t0" >> $log
# A still: the window's pixmap as drawn (MESA_VK_WSI_DEBUG=sw: the whole frame), 0.8 s after the key.
mark() { echo "$(awk "BEGIN{print $(date +%s.%N) - $t0}") still $1" >> $log; import -window $wid stills/$n-$1.png; }
k() { echo "$(awk "BEGIN{print $(date +%s.%N) - $t0}") $*" >> $log; xdotool "$@"; }
sleep 0.5; mark goto-open; sleep 0.7
k type --delay 100 "led"; sleep 0.8; mark goto-led; sleep 0.7
k key BackSpace BackSpace BackSpace; sleep 0.3
k type "f"; sleep 0.8; mark goto-f; sleep 0.7
k key BackSpace; sleep 0.3
k key Tab; sleep 0.8; mark cmds-open; sleep 0.7
k type "f"; sleep 0.8; mark cmds-f; sleep 0.7
k type --delay 100 "old"; sleep 0.8; mark cmds-fold; sleep 0.7
k key BackSpace BackSpace BackSpace BackSpace; sleep 0.3
k type --delay 100 "led"; sleep 0.8; mark cmds-led; sleep 0.7
k key Escape
wait $cpid
echo "design $n $(date +%T)"
