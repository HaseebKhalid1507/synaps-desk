#!/bin/bash
# r3: main and Q3 r2 back to back, --bench-motion then --bench, so both see the same neighbours (PSS divides shared pages by their mappers).
q=~/.cache/q3; cd $q
export XDG_RUNTIME_DIR=/run/user/$(id -u); unset DISPLAY WAYLAND_DISPLAY
quiet() { for _ in $(seq 1 180); do awk '{exit !($1 < 6)}' /proc/loadavg && break; sleep 1; done; }
run() { local bin=$1 out=$2; shift 2
  cfg=$(mktemp -d -p $q); mkdir -p $cfg/synaps-desktop; echo '{"text_size": 16, "theme": "nord"}' > $cfg/synaps-desktop/settings.json
  quiet; echo "$(date +%T) $bin $out load $(cut -d' ' -f1 /proc/loadavg) gamescopes $(pgrep -c gamescope)" >> bench/log
  ENABLE_GAMESCOPE_WSI=0 timeout 260 gamescope --backend headless -r 144 -W 1920 -H 1080 -w 1920 -h 1080 -- \
    env XDG_CONFIG_HOME=$cfg ./$bin "$@" --refresh-hz 144 --size 1920x1080 > bench/$out.json 2> bench/$out.err
  rm -rf $cfg; }

run q3b-bench q3b-motion-4 --bench-motion


echo BENCH4-DONE >> bench/log
