#!/bin/bash
# composer.sh: the bench's --bench-panels scene captured (Full, 1280x720 nord, 60 fps lossless) → clips/r2-bench-panels.mp4:
# the rail and the right panel fold and unfold over a 400-turn transcript, then the draft is set to two lines (the
# composer grows upward) and cleared (it shrinks).
set -uo pipefail
cd ~/.cache/q5r2; mkdir -p clips raw
out=raw/bench-panels; rm -rf $out
cap=~/Projects/.worktrees/synaps-desktop-q5/scripts/capture-linux.sh
CAPTURE_HOVER=0 CAPTURE_FPS=60 CAPTURE_SIZE=1280x720 CAPTURE_SETTINGS='{"text_size": 16, "theme": "nord"}' \
  bash $cap bin/q5 $out 20 --bench-panels --refresh-hz 144 > $out.log 2>&1
ffmpeg -loglevel error -y -i $out/clip.mkv -c:v libx264rgb -qp 0 -preset veryslow -pix_fmt gbrp clips/r2-bench-panels.mp4
a=$(ffmpeg -loglevel error -i $out/clip.mkv -f framemd5 -pix_fmt gbrp - | grep -v '^#' | md5sum | cut -c1-12)
b=$(ffmpeg -loglevel error -i clips/r2-bench-panels.mp4 -f framemd5 -pix_fmt gbrp - | grep -v '^#' | md5sum | cut -c1-12)
hdr=$(ffprobe -v error -select_streams v:0 -show_entries stream=codec_name,profile,pix_fmt,width,height,r_frame_rate,nb_frames -of csv=p=0 clips/r2-bench-panels.mp4)
echo "clip r2-bench-panels frames-md5 mkv=$a mp4=$b header=$hdr $(date +%T)"
rm -f $out/clip.mkv
