#!/bin/bash
# clip.sh <level>: Q5 driven at <level>, 1280x720 nord, 60 fps lossless → clips/panels-<level>.mp4. The drive:
# Ctrl+B fold and unfold; Ctrl+Alt+B fold and unfold; the rail's edge dragged 20 px out and let go (the 312 detent);
# a draft typed past a line, then cleared; the transcript wheeled up (the card away: the stub) and Ctrl+J back;
# the rail wheeled to HISTORY, NAMED folded and unfolded.
set -uo pipefail
cd ~/.cache/q5; mkdir -p clips raw
lv=$1; out=raw/clip-$lv; rm -rf $out
cap=~/Projects/.worktrees/synaps-desktop-q5/scripts/capture-linux.sh
CAPTURE_HOVER=0 CAPTURE_FPS=60 CAPTURE_SIZE=1280x720 CAPTURE_SETTINGS='{"text_size": 16, "theme": "nord"}' \
  bash $cap bin/q5 $out 27 --offline --motion $lv > $out.log 2>&1 &
cpid=$!
for _ in $(seq 1 100); do [ -s $out/display ] && break; sleep 0.1; done
export DISPLAY=$(cat $out/display)
for _ in $(seq 1 100); do wid=$(xdotool search --classname '^synaps-desktop$' 2>/dev/null | tail -n1); [ -n "$wid" ] && break; sleep 0.1; done
sleep 5
log=$out-drive.log; : > $log
t0=$(date +%s.%N); echo "t0 $t0" >> $log
k() { echo "$(awk "BEGIN{print $(date +%s.%N) - $t0}") $*" >> $log; xdotool "$@"; }
k mousemove 640 300; sleep 0.5
k key ctrl+b; sleep 1.3
k key ctrl+b; sleep 1.3
k key ctrl+alt+b; sleep 1.3
k key ctrl+alt+b; sleep 1.3
k mousemove 311 400; sleep 0.2
k mousedown 1; for x in 314 318 322 326 330 332; do xdotool mousemove $x 400; sleep 0.03; done
k mouseup 1; sleep 1.2
k mousemove 640 625; k click 1; sleep 0.3
k type --delay 6 "A draft that runs past the end of its first line and onto a second one, so the composer grows by a line upward"
sleep 1.0
k key ctrl+a BackSpace; sleep 1.2
k mousemove 640 300; for _ in 1 2 3 4 5 6; do xdotool click 4; sleep 0.04; done; sleep 1.6
k key ctrl+j; sleep 1.6
k mousemove 150 500; for _ in $(seq 1 12); do xdotool click 5; sleep 0.04; done; k mousemove 70 350; sleep 0.6
k click 1; sleep 1.3
k click 1; sleep 1.5
wait $cpid
ffmpeg -loglevel error -y -i $out/clip.mkv -c:v libx264rgb -qp 0 -preset veryslow -pix_fmt gbrp clips/panels-$lv.mp4
a=$(ffmpeg -loglevel error -i $out/clip.mkv -f framemd5 -pix_fmt gbrp - | grep -v '^#' | md5sum | cut -c1-12)
b=$(ffmpeg -loglevel error -i clips/panels-$lv.mp4 -f framemd5 -pix_fmt gbrp - | grep -v '^#' | md5sum | cut -c1-12)
hdr=$(ffprobe -v error -select_streams v:0 -show_entries stream=codec_name,profile,pix_fmt,width,height,r_frame_rate,nb_frames -of csv=p=0 clips/panels-$lv.mp4)
echo "clip $lv frames-md5 mkv=$a mp4=$b header=$hdr $(date +%T)"
rm -f $out/clip.mkv
