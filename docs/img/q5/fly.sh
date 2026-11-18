#!/bin/bash
# fly.sh <bin> <name> <theme> <size>: Ctrl+B 1 s into a 3 s, 60 fps capture; the frame 9 frames (150 ms) after the
# rail first moved (the first frame whose SESSIONS fold button, 60×40 at 250,66, changed) → side/<name>.png
set -uo pipefail
cd ~/.cache/q5
cap=~/Projects/.worktrees/synaps-desktop-q5/scripts/capture-linux.sh
b=$1 n=$2 t=$3 s=$4; out=raw/$n; rm -rf $out
CAPTURE_HOVER=0 CAPTURE_FPS=60 CAPTURE_SIZE=$s CAPTURE_SETTINGS="{\"text_size\": 16, \"theme\": \"$t\"}" bash $cap bin/$b $out 3 --offline > $out.log 2>&1 &
cpid=$!
for _ in $(seq 1 100); do [ -s $out/display ] && break; sleep 0.1; done
export DISPLAY=$(cat $out/display)
sleep 4.6
xdotool key ctrl+b
wait $cpid
first=$(ffmpeg -loglevel error -i $out/clip.mkv -vf crop=60:40:250:66 -f framemd5 - | grep -v '^#' | awk -F', ' 'NR==1{m=$6} $6!=m{print NR-1; exit}')
ffmpeg -loglevel error -y -i $out/clip.mkv -vf "select=eq(n\,$((first + 9)))" -vframes 1 side/$n.png
rm -f $out/clip.mkv
echo "side/$n.png first-moved f$first"
