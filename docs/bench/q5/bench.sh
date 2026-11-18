#!/bin/bash
# Q5 bench on bella: main 9b0d263 and Q5, release builds made the same way (dbuild), alternating, 3 runs each.
# --bench-panels (Q5's scene; main runs it from 9b0d263 + the bench commit alone) at 1280 and 1920, then
# --bench-motion (gate 3 idle_30s_prompt; gate 5 memory). A run's load and swap-ins (pswpin delta) are logged.
q=~/.cache/q5; cd $q; mkdir -p bench
export XDG_RUNTIME_DIR=/run/user/$(id -u); unset DISPLAY WAYLAND_DISPLAY
quiet() { for _ in $(seq 1 180); do awk '{exit !($1 < 6)}' /proc/loadavg && break; sleep 1; done; }
pswpin() { awk '/^pswpin/{print $2}' /proc/vmstat; }
run() { local bin=$1 out=$2 size=$3; shift 3
  cfg=$(mktemp -d -p $q); mkdir -p $cfg/synaps-desktop; echo '{"text_size": 16, "theme": "nord"}' > $cfg/synaps-desktop/settings.json
  local w=${size%x*} h=${size#*x}
  quiet; local s0; s0=$(pswpin); local load; load=$(cut -d' ' -f1 /proc/loadavg)
  ENABLE_GAMESCOPE_WSI=0 timeout 450 gamescope --backend headless -r 144 -W $w -H $h -w $w -h $h -- \
    env XDG_CONFIG_HOME=$cfg ./bin/$bin "$@" --refresh-hz 144 --size $size > bench/$out.json 2> bench/$out.err
  echo "$(date +%T) $bin $out load $load swapins $(( $(pswpin) - s0 ))" >> bench/log
  rm -rf $cfg; }
for i in 1 2 3; do
  for s in 1280x720 1920x1080; do
    run main-bench main-panels-$s-$i $s --bench-panels
    run q5 q5-panels-$s-$i $s --bench-panels
  done
done
for i in 1 2 3; do
  run main main-motion-$i 1920x1080 --bench-motion
  run q5 q5-motion-$i 1920x1080 --bench-motion
done
echo BENCH-DONE >> bench/log
