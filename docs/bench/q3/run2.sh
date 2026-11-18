#!/bin/bash
# r2: Q3 at the 30 Hz alignment fix, --bench-motion twice and --bench once; main's runs are r1's (same harness).
q=~/.cache/q3; cd $q; mkdir -p bench
cp ~/.cache/cargo-target-synaps-desktop/synaps-desktop-q3/release/synaps-desktop $q/q3b-bench
export XDG_RUNTIME_DIR=/run/user/$(id -u); unset DISPLAY WAYLAND_DISPLAY
quiet() { for _ in $(seq 1 180); do awk '{exit !($1 < 6)}' /proc/loadavg && break; sleep 1; done; }
run() { local bin=$1 out=$2; shift 2
  cfg=$(mktemp -d -p $q); mkdir -p $cfg/synaps-desktop; echo '{"text_size": 16, "theme": "nord"}' > $cfg/synaps-desktop/settings.json
  quiet; echo "$(date +%T) $bin $out load $(cut -d' ' -f1 /proc/loadavg)" >> bench/log
  ENABLE_GAMESCOPE_WSI=0 timeout 260 gamescope --backend headless -r 144 -W 1920 -H 1080 -w 1920 -h 1080 -- \
    env XDG_CONFIG_HOME=$cfg ./$bin "$@" --refresh-hz 144 --size 1920x1080 > bench/$out.json 2> bench/$out.err
  rm -rf $cfg; }
run q3b-bench q3b-motion-1 --bench-motion
run q3b-bench q3b-motion-2 --bench-motion
run q3b-bench q3b-1 --bench
echo BENCH2-DONE >> bench/log
