#!/bin/bash
# clip.sh: Full, 1280x720 nord, 60 fps lossless: the process's first Ctrl+P, Down x12, PageDown, Esc, Ctrl+P again.
set -uo pipefail
cd ~/.cache/q6r3; mkdir -p clips raw
out=raw/clip-full; rm -rf $out
cap=~/Projects/.worktrees/synaps-desktop-q6/scripts/capture-linux.sh
CAPTURE_HOVER=0 CAPTURE_FPS=60 CAPTURE_SIZE=1280x720 CAPTURE_SETTINGS='{"text_size": 16, "theme": "nord"}' \
  bash $cap bin/r3 $out 14 --offline --motion full > $out.log 2>&1 &
cpid=$!
for _ in $(seq 1 100); do [ -s $out/display ] && break; sleep 0.1; done
export DISPLAY=$(cat $out/display)
for _ in $(seq 1 100); do wid=$(xdotool search --classname '^synaps-desktop$' 2>/dev/null | tail -n1); [ -n "$wid" ] && break; sleep 0.1; done
sleep 4
log=$out-drive.log; : > $log; t0=$(date +%s.%N); echo "t0 $t0" >> $log
k() { echo "$(awk "BEGIN{print $(date +%s.%N) - $t0}") $*" >> $log; xdotool "$@"; }
sleep 0.5; k key ctrl+p; sleep 1.0
for i in $(seq 1 12); do k key Down; sleep 0.35; done
sleep 0.5; k key Next; sleep 1.0
k key Escape; sleep 1.0
k key ctrl+p; sleep 1.5
k key Escape
wait $cpid
ffmpeg -loglevel error -y -i $out/clip.mkv -c:v libx264rgb -qp 0 -preset veryslow -pix_fmt gbrp clips/palette-scroll-full.mp4
a=$(ffmpeg -loglevel error -i $out/clip.mkv -f framemd5 -pix_fmt gbrp - | grep -v '^#' | md5sum | cut -c1-12)
b=$(ffmpeg -loglevel error -i clips/palette-scroll-full.mp4 -f framemd5 -pix_fmt gbrp - | grep -v '^#' | md5sum | cut -c1-12)
echo "clip frames-md5 mkv=$a mp4=$b $(date +%T)"
