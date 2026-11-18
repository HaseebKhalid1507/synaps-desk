#!/bin/bash
# Q6 bench: main 0a78bb7 and Q6, --bench-motion, 3 runs each alternating (gate 3 idle with a pending prompt; gate 5 memory).
q=~/.cache/q6; cd $q; mkdir -p bench
export XDG_RUNTIME_DIR=/run/user/$(id -u); unset DISPLAY WAYLAND_DISPLAY
quiet() { for _ in $(seq 1 180); do awk '{exit !($1 < 6)}' /proc/loadavg && break; sleep 1; done; }
run() { local bin=$1 out=$2; shift 2
  cfg=$(mktemp -d -p $q); mkdir -p $cfg/synaps-desktop; echo '{"text_size": 16, "theme": "nord"}' > $cfg/synaps-desktop/settings.json
  quiet; echo "$(date +%T) $bin $out load $(cut -d' ' -f1 /proc/loadavg) swap $(awk '/SwapFree/{print $2}' /proc/meminfo)" >> bench/log
  ENABLE_GAMESCOPE_WSI=0 timeout 400 gamescope --backend headless -r 144 -W 1920 -H 1080 -w 1920 -h 1080 -- \
    env XDG_CONFIG_HOME=$cfg ./bin/$bin "$@" --refresh-hz 144 --size 1920x1080 > bench/$out.json 2> bench/$out.err
  rm -rf $cfg; }
for i in 1 2 3; do
  run main-ev main-motion-$i --bench-motion
  run q6-ev q6-motion-$i --bench-motion
done
echo BENCH-DONE >> bench/log
