#!/bin/bash
# Q3 r3 evidence on bella, on the merged tree: the life clips (Full 1920 and Reduced 1920 at 60 fps: the settle
# pass, P3; Off 1920 and Full 1280 at 30), packed lossless; then side by side at rest, main 9121dba vs Q3 r3.
set -uo pipefail
q=~/.cache/q3; cd $q; mkdir -p bin r3/raw r3/clips r3/side
cp "${Q3BIN:?}" bin/q3r3-ev
cp ~/.local/bin/synaps-desktop-hybrid.q2 bin/main3-ev
cap=~/Projects/.worktrees/synaps-desktop-q3/scripts/capture-linux.sh
mode=${1:-all}
quiet() { for _ in $(seq 1 120); do awk '{exit !($1 < 8)}' /proc/loadavg && break; sleep 1; done; }
pack() { local raw=$1 out=$2
  ffmpeg -loglevel error -y -i $raw -c:v libx264rgb -qp 0 -preset veryslow -pix_fmt gbrp $out
  a=$(ffmpeg -loglevel error -i $raw -f framemd5 -pix_fmt gbrp - | grep -v '^#' | md5sum | cut -c1-12)
  b=$(ffmpeg -loglevel error -i $out -f framemd5 -pix_fmt gbrp - | grep -v '^#' | md5sum | cut -c1-12)
  echo "$(basename $out) frames-md5 mkv=$a mp4=$b $(date +%T)"; }
if [ "$mode" != side ]; then
for spec in "full 1920x1080 60" "reduced 1920x1080 60" "off 1920x1080 30" "full 1280x720 30"; do
  set -- $spec; lv=$1; size=$2; fps=$3; name=life-$lv-${size%x*}
  [ -n "${ONLY:-}" ] && [ "$ONLY" != "$name" ] && continue
  quiet
  SYNAPS_BENCH_LIFE=4 SYNAPS_BENCH_LIFE_LEVEL=$lv CAPTURE_HOVER=0 CAPTURE_SIZE=$size CAPTURE_FPS=$fps bash $cap bin/q3r3-ev r3/raw/$name 40 --bench-motion > r3/raw/$name.log 2>&1
  pack r3/raw/$name/clip.mkv r3/clips/$name.mp4
  rm -rf r3/raw/$name
done
fi
if [ "$mode" != clips ]; then
for t in nord night-city default paper; do
  for s in 1920x1080 1280x720; do
    for b in main3 q3r3; do
      quiet
      CAPTURE_HOVER=0 CAPTURE_SIZE=$s CAPTURE_SETTINGS="{\"text_size\": 16, \"theme\": \"$t\"}" bash $cap bin/$b-ev r3/raw/$b-$t-$s 2 --offline > r3/raw/$b-$t-$s.log 2>&1
      ffmpeg -loglevel error -y -i r3/raw/$b-$t-$s/clip.mkv -vf "select=eq(n\,45)" -vframes 1 r3/side/$b-$t-$s.png
      rm -rf r3/raw/$b-$t-$s
    done
    ffmpeg -loglevel error -y -i r3/side/main3-$t-$s.png -i r3/side/q3r3-$t-$s.png -filter_complex hstack r3/side/side-$t-$s.png
    echo "side $t $s $(date +%T)"
  done
done
fi
echo EVIDENCE3-DONE
