#!/bin/bash
# Q3 r2: main and Q3 r2 back to back (--bench-motion, then --bench), three --bench-motion runs of each, alternating.
q=~/.cache/q3; cd $q; mkdir -p bench5
cp ~/.cache/cargo-target-synaps-desktop/synaps-desktop-q3/release/synaps-desktop $q/q3r2-bench
cp ~/.local/bin/synaps-desktop-hybrid.q4 $q/main5-bench
export XDG_RUNTIME_DIR=/run/user/$(id -u); unset DISPLAY WAYLAND_DISPLAY
quiet() { for _ in $(seq 1 240); do awk '{exit !($1 < 6)}' /proc/loadavg && break; sleep 1; done; }
run() { local bin=$1 out=$2; shift 2
  cfg=$(mktemp -d -p $q); mkdir -p $cfg/synaps-desktop; echo '{"text_size": 16, "theme": "nord"}' > $cfg/synaps-desktop/settings.json
  quiet; echo "$(date +%T) $bin $out load $(cut -d' ' -f1 /proc/loadavg) gamescopes $(pgrep -c gamescope)" >> bench5/log
  ENABLE_GAMESCOPE_WSI=0 timeout 300 gamescope --backend headless -r 144 -W 1920 -H 1080 -w 1920 -h 1080 -- \
    env XDG_CONFIG_HOME=$cfg ./$bin "$@" --refresh-hz 144 --size 1920x1080 > bench5/$out.json 2> bench5/$out.err
  rm -rf $cfg; }
for k in 1 2 3; do
  run main5-bench main-motion-$k --bench-motion
  run q3r2-bench q3r2-motion-$k --bench-motion
done
run main5-bench main-mem --bench
run q3r2-bench q3r2-mem --bench
echo BENCH5-DONE >> bench5/log
