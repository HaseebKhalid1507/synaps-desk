#!/bin/bash
# clip.sh <level>: Q5 r4 driven at <level>, 1280x720 nord, 60 fps lossless → clips/r4-<level>.mp4.
# Three favourites (the one that needs you in the middle, kept when folded): FAVOURITES folded and unfolded; a double
# click on its head (a fold turned round into an unfold); folded, then a double click (an unfold turned round into a
# fold) and one more click (open again); the rail's edge dragged out and let go (the thumb holds through the drag and
# the spring-back); the transcript wheeled up (the stub) and Ctrl+J. NAMED and the Ctrl+B / Ctrl+Alt+B flights:
# r2drive.sh.
set -uo pipefail
cd ~/.cache/q5r4; mkdir -p clips raw
lv=$1; size=1280x720; tag=$lv
out=raw/clip-$tag; rm -rf $out
# Three live favourites (the demo's sessions have no journal ids: provisional, by their live ids, as Ctrl+D makes
# them): two idle, the one that needs you (relay-refresh) between them.
fav() { echo "{\"id\": \"$1\", \"label\": \"$2\", \"provisional\": true, \"added_at\": $3}"; }
favs="[$(fav 20261112-151010-5c21 docs-sync 1), $(fav 20261113-041207-a1f0 relay-refresh 2), $(fav 20261112-120455-e7d3 ci-cache 3)]"
cap=~/Projects/.worktrees/synaps-desktop-q5/scripts/capture-linux.sh
CAPTURE_HOVER=0 CAPTURE_FPS=60 CAPTURE_SIZE=$size CAPTURE_SETTINGS="{\"text_size\": 16, \"theme\": \"nord\", \"favourites\": {\"offline\": $favs}}" \
  bash $cap bin/q5 $out 27 --offline --motion $lv > $out.log 2>&1 &
cpid=$!
for _ in $(seq 1 100); do [ -s $out/display ] && break; sleep 0.1; done
export DISPLAY=$(cat $out/display)
for _ in $(seq 1 100); do wid=$(xdotool search --classname '^synaps-desktop$' 2>/dev/null | tail -n1); [ -n "$wid" ] && break; sleep 0.1; done
sleep 5
log=$out-drive.log; : > $log
t0=$(date +%s.%N); echo "t0 $t0" >> $log
k() { echo "$(awk "BEGIN{print $(date +%s.%N) - $t0}") $*" >> $log; xdotool "$@"; }
k mousemove 61 213; sleep 0.4
k click 1; sleep 1.3
k click 1; sleep 1.3
k click 1; sleep 0.12
k click 1; sleep 1.3
k click 1; sleep 1.3
k click 1; sleep 0.12
k click 1; sleep 1.3
k click 1; sleep 1.3
k mousemove 311 400; sleep 0.2
k mousedown 1; for x in 314 318 322 326 330 332; do xdotool mousemove $x 400; sleep 0.03; done
k mouseup 1; sleep 1.2
k mousemove 640 300; for _ in 1 2 3 4 5 6; do xdotool click 4; sleep 0.04; done; sleep 1.6
k key ctrl+j; sleep 1.6
wait $cpid
ffmpeg -loglevel error -y -i $out/clip.mkv -c:v libx264rgb -qp 0 -preset veryslow -pix_fmt gbrp clips/r4-$tag.mp4
a=$(ffmpeg -loglevel error -i $out/clip.mkv -f framemd5 -pix_fmt gbrp - | grep -v '^#' | md5sum | cut -c1-12)
b=$(ffmpeg -loglevel error -i clips/r4-$tag.mp4 -f framemd5 -pix_fmt gbrp - | grep -v '^#' | md5sum | cut -c1-12)
hdr=$(ffprobe -v error -select_streams v:0 -show_entries stream=codec_name,profile,pix_fmt,width,height,r_frame_rate,nb_frames -of csv=p=0 clips/r4-$tag.mp4)
echo "clip r4-$tag frames-md5 mkv=$a mp4=$b header=$hdr $(date +%T)"
[ "$a" = "$b" ] && rm -f $out/clip.mkv
