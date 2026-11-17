#!/bin/bash
# main (4f87411) vs Q3: --bench-motion and --bench, alternating, each in a headless gamescope at 144 Hz,
# 1920x1080, after the 1-minute load average falls under 6. Copied from docs/bench/q4/r4/run.sh.
q=~/.cache/q3; cd $q; mkdir -p bench; rm -f bench/*.json bench/*.err
while ! grep -q EVIDENCE-DONE evidence.log 2>/dev/null; do sleep 5; done
cp ~/.local/bin/synaps-desktop-hybrid.q4 $q/main-bench
cp ~/.cache/cargo-target-synaps-desktop/synaps-desktop-q3/release/synaps-desktop $q/q3-bench
export XDG_RUNTIME_DIR=/run/user/$(id -u); unset DISPLAY WAYLAND_DISPLAY
quiet() { for _ in $(seq 1 180); do awk '{exit !($1 < 6)}' /proc/loadavg && break; sleep 1; done; }
run() { # bin out args...
  local bin=$1 out=$2; shift 2
  cfg=$(mktemp -d -p $q); mkdir -p $cfg/synaps-desktop; echo '{"text_size": 16, "theme": "nord"}' > $cfg/synaps-desktop/settings.json
  quiet
  echo "$(date +%T) $bin $out load $(cut -d' ' -f1 /proc/loadavg)" >> bench/log
  ENABLE_GAMESCOPE_WSI=0 timeout 260 gamescope --backend headless -r 144 -W 1920 -H 1080 -w 1920 -h 1080 -- \
    env XDG_CONFIG_HOME=$cfg ./$bin "$@" --refresh-hz 144 --size 1920x1080 > bench/$out.json 2> bench/$out.err
  rm -rf $cfg
}
for b in main q3; do run $b-bench $b-motion-1 --bench-motion; done
for b in main q3; do run $b-bench $b-1 --bench; done
for b in main q3; do run $b-bench $b-motion-2 --bench-motion; done
echo BENCH-DONE >> bench/log
