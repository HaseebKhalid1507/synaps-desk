#!/bin/bash
# Q3 evidence on bella: side by side at rest (main vs Q3), the life clips at Full/Reduced/Off, packed lossless.
set -uo pipefail
q=~/.cache/q3; cd $q; mkdir -p bin side clips raw
cp ~/.local/bin/synaps-desktop-hybrid.q4 bin/main-ev
cp ~/.cache/cargo-target-synaps-desktop/synaps-desktop-q3/release/synaps-desktop bin/q3-ev
cap=~/Projects/.worktrees/synaps-desktop-q3/scripts/capture-linux.sh
quiet() { for _ in $(seq 1 120); do awk '{exit !($1 < 8)}' /proc/loadavg && break; sleep 1; done; }
if [ "${1:-all}" != clips ]; then
for t in nord night-city default paper; do
  for s in 1920x1080 1280x720; do
    for b in main q3; do
      quiet
      CAPTURE_HOVER=0 CAPTURE_SIZE=$s CAPTURE_SETTINGS="{\"text_size\": 16, \"theme\": \"$t\"}" bash $cap bin/$b-ev raw/$b-$t-$s 2 --offline > raw/$b-$t-$s.log 2>&1
      ffmpeg -loglevel error -y -i raw/$b-$t-$s/clip.mkv -vf "select=eq(n\,45)" -vframes 1 side/$b-$t-$s.png
    done
    ffmpeg -loglevel error -y -i side/main-$t-$s.png -i side/q3-$t-$s.png -filter_complex hstack side/side-$t-$s.png
    echo "side $t $s $(date +%T)"
  done
done
fi
for lv in full reduced off; do
  quiet
  SYNAPS_BENCH_LIFE=4 SYNAPS_BENCH_LIFE_LEVEL=$lv CAPTURE_HOVER=0 bash $cap bin/q3-ev raw/life-$lv 34 --bench-motion > raw/life-$lv.log 2>&1
  ffmpeg -loglevel error -y -i raw/life-$lv/clip.mkv -c:v libx264rgb -qp 0 -preset veryslow -pix_fmt gbrp clips/life-$lv.mp4
  a=$(ffmpeg -loglevel error -i raw/life-$lv/clip.mkv -f framemd5 -pix_fmt gbrp - | grep -v '^#' | md5sum | cut -c1-12)
  b=$(ffmpeg -loglevel error -i clips/life-$lv.mp4 -f framemd5 -pix_fmt gbrp - | grep -v '^#' | md5sum | cut -c1-12)
  echo "life $lv frames-md5 mkv=$a mp4=$b $(date +%T)"
done
echo EVIDENCE-DONE
