#!/bin/bash
# chips.sh: the worker chips (SYNAPS_BENCH_CHIPS=N in bench.rs) with 1, 3 and 6 workers, nord and paper, 1920 and
# 1280 → stills/chips-<theme>-<size>-<n>.png (3.5 s in: spawned, steps changing). (6 s: the scene's JSON, printed at
# its end, is chipclip.sh's.)
set -uo pipefail
cd ~/.cache/q5r2; mkdir -p stills raw
cap=~/Projects/.worktrees/synaps-desktop-q5/scripts/capture-linux.sh
for t in nord paper; do for s in 1920x1080 1280x720; do for n in 1 3 6; do
  out=raw/chips-$t-$s-$n; rm -rf $out
  SYNAPS_BENCH_CHIPS=$n CAPTURE_HOVER=0 CAPTURE_SIZE=$s CAPTURE_SETTINGS="{\"text_size\": 16, \"theme\": \"$t\"}" \
    bash $cap bin/q5 $out 6 --bench-motion --refresh-hz 144 > $out.log 2>&1
  ffmpeg -loglevel error -y -i $out/clip.mkv -vf "select=eq(n\,105)" -vframes 1 stills/chips-$t-$s-$n.png
  rm -f $out/clip.mkv
  echo "stills/chips-$t-$s-$n.png $(date +%T)"
done; done; done
