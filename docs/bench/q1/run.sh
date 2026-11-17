#!/bin/bash
# main vs Q1: --bench-motion (30 s idle over rows at rest, a tool's beat, 25 glyphs, Off) and --bench
# (streaming, scroll, RSS/PSS), alternating, each in a headless gamescope at 144 Hz, 1920x1080.
cd ~/.cache/q1-shots; mkdir -p bench; rm -f bench/*.json bench/*.err
export XDG_RUNTIME_DIR=/run/user/$(id -u); unset DISPLAY WAYLAND_DISPLAY
quiet() { for _ in $(seq 1 120); do awk '{exit !($1 < 6)}' /proc/loadavg && break; sleep 1; done; }
run() { # bin out args...
  local bin=$1 out=$2; shift 2
  cfg=$(mktemp -d -p ~/.cache/q1-shots); mkdir -p $cfg/synaps-desktop; echo '{"text_size": 16, "theme": "nord"}' > $cfg/synaps-desktop/settings.json
  quiet
  echo "$(date +%T) $bin $out load $(cut -d' ' -f1 /proc/loadavg)" >> bench/log
  ENABLE_GAMESCOPE_WSI=0 timeout 150 gamescope --backend headless -r 144 -W 1920 -H 1080 -w 1920 -h 1080 -- \
    env XDG_CONFIG_HOME=$cfg SYNAPS_BENCH_IDLE_SECS=30 ./$bin "$@" --refresh-hz 144 --size 1920x1080 > bench/$out.json 2> bench/$out.err
  rm -rf $cfg
}
for r in 1 2; do
  for b in main q1; do run $b-bench $b-motion-$r --bench-motion; done
done
for r in 1 2; do
  for b in main q1; do run $b-bench $b-$r --bench; done
done
echo BENCH-DONE >> bench/log
