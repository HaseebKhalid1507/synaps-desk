#!/bin/bash
# clip.sh <level>: the palette driven by keys at <level>, 1280x720 nord, 60 fps lossless → clips/palette-<level>.mp4
set -uo pipefail
cd ~/.cache/q6; mkdir -p clips raw
lv=$1; out=raw/clip-$lv; rm -rf $out
cap=~/Projects/.worktrees/synaps-desktop-q6/scripts/capture-linux.sh
CAPTURE_HOVER=0 CAPTURE_FPS=60 CAPTURE_SIZE=1280x720 CAPTURE_SETTINGS='{"text_size": 16, "theme": "nord"}' \
  bash $cap bin/q6-ev $out 16 --offline --motion $lv > $out.log 2>&1 &
cpid=$!
for _ in $(seq 1 100); do [ -s $out/display ] && break; sleep 0.1; done
export DISPLAY=$(cat $out/display)
for _ in $(seq 1 100); do wid=$(xdotool search --classname '^synaps-desktop$' 2>/dev/null | tail -n1); [ -n "$wid" ] && break; sleep 0.1; done
sleep 5
log=$out-drive.log; : > $log
t0=$(date +%s.%N); echo "t0 $t0" >> $log
k() { echo "$(awk "BEGIN{print $(date +%s.%N) - $t0}") $*" >> $log; xdotool "$@"; }
k key ctrl+k; sleep 1.2
k key Down; sleep 0.6
k key Down; sleep 0.6
k key Down; sleep 0.6
k key Up; sleep 1.0
k type --delay 120 "fold"; sleep 1.2
k key BackSpace BackSpace BackSpace BackSpace; sleep 1.2
k key Tab; sleep 1.0
k key Down; sleep 0.6
k key Down; sleep 0.6
k type --delay 120 "ledger"; sleep 1.2
k key Escape; sleep 1.0
k key ctrl+p; sleep 1.0
k key Escape
wait $cpid
ffmpeg -loglevel error -y -i $out/clip.mkv -c:v libx264rgb -qp 0 -preset veryslow -pix_fmt gbrp clips/palette-$lv.mp4
a=$(ffmpeg -loglevel error -i $out/clip.mkv -f framemd5 -pix_fmt gbrp - | grep -v '^#' | md5sum | cut -c1-12)
b=$(ffmpeg -loglevel error -i clips/palette-$lv.mp4 -f framemd5 -pix_fmt gbrp - | grep -v '^#' | md5sum | cut -c1-12)
echo "clip $lv frames-md5 mkv=$a mp4=$b $(date +%T)"
