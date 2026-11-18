#!/bin/bash
# Q3 r2 evidence on bella: the life clips (Full 1920 at 60 fps; Reduced and Off at 30; Full 1280 at 30), packed lossless.
set -uo pipefail
q=~/.cache/q3; cd $q; mkdir -p bin r2/raw r2/clips
cp ~/.cache/cargo-target-synaps-desktop/synaps-desktop-q3/release/synaps-desktop bin/q3r2-ev
cap=~/Projects/.worktrees/synaps-desktop-q3/scripts/capture-linux.sh
quiet() { for _ in $(seq 1 120); do awk '{exit !($1 < 8)}' /proc/loadavg && break; sleep 1; done; }
for spec in "full 1920x1080 60" "reduced 1920x1080 30" "off 1920x1080 30" "full 1280x720 30"; do
  set -- $spec; lv=$1; size=$2; fps=$3; name=life-$lv-${size%x*}
  [ -n "${ONLY:-}" ] && [ "$ONLY" != "$name" ] && continue
  quiet
  SYNAPS_BENCH_LIFE=4 SYNAPS_BENCH_LIFE_LEVEL=$lv CAPTURE_HOVER=0 CAPTURE_SIZE=$size CAPTURE_FPS=$fps bash $cap bin/q3r2-ev r2/raw/$name 36 --bench-motion > r2/raw/$name.log 2>&1
  ffmpeg -loglevel error -y -i r2/raw/$name/clip.mkv -c:v libx264rgb -qp 0 -preset veryslow -pix_fmt gbrp r2/clips/$name.mp4
  a=$(ffmpeg -loglevel error -i r2/raw/$name/clip.mkv -f framemd5 -pix_fmt gbrp - | grep -v '^#' | md5sum | cut -c1-12)
  b=$(ffmpeg -loglevel error -i r2/clips/$name.mp4 -f framemd5 -pix_fmt gbrp - | grep -v '^#' | md5sum | cut -c1-12)
  echo "$name frames-md5 mkv=$a mp4=$b $(date +%T)"
done
echo EVIDENCE2-DONE
