#!/bin/bash
# main (4f87411's tree) vs Q2 4475cdf: --bench-motion and --bench, alternating, each in a headless gamescope
# at 144 Hz, 1920x1080, after the 1-minute load average falls under 6 (as docs/bench/q4/r4/run.sh).
cd ~/.cache/q2-shots; mkdir -p bench; rm -f bench/*
export XDG_RUNTIME_DIR=/run/user/$(id -u); unset DISPLAY WAYLAND_DISPLAY
quiet() { for _ in $(seq 1 180); do awk '{exit !($1 < 6)}' /proc/loadavg && break; sleep 1; done; }
run() { local bin=$1 out=$2; shift 2
  cfg=$(mktemp -d -p ~/.cache/q2-shots); mkdir -p $cfg/synaps-desktop; echo '{"text_size": 16, "theme": "nord"}' > $cfg/synaps-desktop/settings.json
  quiet; echo "$(date +%T) $bin $out load $(cut -d' ' -f1 /proc/loadavg)" >> bench/log
  ENABLE_GAMESCOPE_WSI=0 timeout 260 gamescope --backend headless -r 144 -W 1920 -H 1080 -w 1920 -h 1080 -- \
    env XDG_CONFIG_HOME=$cfg ./$bin "$@" --refresh-hz 144 --size 1920x1080 > bench/$out.json 2> bench/$out.err
  rm -rf $cfg; }
for b in main q2; do run $b $b-motion --bench-motion; done
for b in main q2; do run $b $b-bench --bench; done
echo BENCH-DONE >> bench/log
