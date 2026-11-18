#!/bin/bash
# Q7 bench on bella: main 0a78bb7 and Q7 built the same way (dbuild release), alternating, 3 runs each:
# --bench-motion (gate 3: idle_30s_prompt) and --bench (gate 5: mem_end after 2,000 rows, anon / USS).
# Each run logs the load and the swap-ins during it (pswpin delta): a run that paged is skipped.
q=~/.cache/q7; cd $q; mkdir -p bench
export XDG_RUNTIME_DIR=/run/user/$(id -u); unset DISPLAY WAYLAND_DISPLAY
quiet() { for _ in $(seq 1 180); do awk '{exit !($1 < 6)}' /proc/loadavg && break; sleep 1; done; }
pswpin() { awk '/^pswpin/{print $2}' /proc/vmstat; }
run() { local bin=$1 out=$2; shift 2
  cfg=$(mktemp -d -p $q); mkdir -p $cfg/synaps-desktop; echo '{"text_size": 16, "theme": "nord"}' > $cfg/synaps-desktop/settings.json
  quiet; local s0; s0=$(pswpin); local load; load=$(cut -d' ' -f1 /proc/loadavg)
  ENABLE_GAMESCOPE_WSI=0 timeout 300 gamescope --backend headless -r 144 -W 1920 -H 1080 -w 1920 -h 1080 -- \
    env XDG_CONFIG_HOME=$cfg ./bin/$bin "$@" --refresh-hz 144 --size 1920x1080 > bench/$out.json 2> bench/$out.err
  echo "$(date +%T) $bin $out load $load swapins $(( $(pswpin) - s0 ))" >> bench/log
  rm -rf $cfg; }
for i in 1 2 3; do
  run main-0a78bb7 main-motion-$i --bench-motion
  run q7-3e2093d q7-motion-$i --bench-motion
  run main-0a78bb7 main-mem-$i --bench
  run q7-3e2093d q7-mem-$i --bench
done
echo BENCH-DONE >> bench/log
