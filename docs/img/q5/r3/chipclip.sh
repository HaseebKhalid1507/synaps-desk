#!/bin/bash
# chipclip.sh: 3 workers' steps changing every 0.4 s, their clocks ticking, one done and one
# failed, the pointer on a chip for 0.5 s (its hover) — 1280x720 nord, 60 fps lossless → clips/r3-chips.mp4.
set -uo pipefail
cd ~/.cache/q5r3; mkdir -p clips raw
out=raw/chipclip; rm -rf $out
cap=~/Projects/.worktrees/synaps-desktop-q5/scripts/capture-linux.sh
SYNAPS_BENCH_CHIPS=3 CAPTURE_HOVER=0 CAPTURE_FPS=60 CAPTURE_SIZE=1280x720 CAPTURE_SETTINGS='{"text_size": 16, "theme": "nord"}' \
  bash $cap bin/q5 $out 16 --bench-motion --refresh-hz 144 > $out.log 2>&1 &
cpid=$!
for _ in $(seq 1 100); do [ -s $out/display ] && break; sleep 0.1; done
export DISPLAY=$(cat $out/display)
sleep 4; xdotool mousemove ${HOVER_X:-500} ${HOVER_Y:-537}; echo "hover $(date +%s.%N)" > $out-drive.log
sleep 0.5; xdotool mousemove 640 300; echo "away $(date +%s.%N)" >> $out-drive.log
wait $cpid
cp $out/app.log clips/r3-chips.json
ffmpeg -loglevel error -y -i $out/clip.mkv -c:v libx264rgb -qp 0 -preset veryslow -pix_fmt gbrp clips/r3-chips.mp4
a=$(ffmpeg -loglevel error -i $out/clip.mkv -f framemd5 -pix_fmt gbrp - | grep -v '^#' | md5sum | cut -c1-12)
b=$(ffmpeg -loglevel error -i clips/r3-chips.mp4 -f framemd5 -pix_fmt gbrp - | grep -v '^#' | md5sum | cut -c1-12)
hdr=$(ffprobe -v error -select_streams v:0 -show_entries stream=codec_name,profile,pix_fmt,width,height,r_frame_rate,nb_frames -of csv=p=0 clips/r3-chips.mp4)
echo "clip r3-chips frames-md5 mkv=$a mp4=$b header=$hdr $(date +%T)"
rm -f $out/clip.mkv
