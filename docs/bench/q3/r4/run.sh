#!/bin/bash
# Q3 r4: worker rows change in place; load_5x5_fanout_reduced_30s (Full - Reduced = the motion's cost). --bench-motion three times each, Q3 and
# main 9121dba alternating (main has no fan-out case: its runs give the shared cases' baseline).
q=~/.cache/q3; cd $q; mkdir -p bench8 bin
cp "${Q3BIN:?}" bin/q3r4-bench
cp ~/.local/bin/synaps-desktop-hybrid.q2 bin/main8-bench
export XDG_RUNTIME_DIR=/run/user/$(id -u); unset DISPLAY WAYLAND_DISPLAY
quiet() { for _ in $(seq 1 240); do awk '{exit !($1 < 6)}' /proc/loadavg && break; sleep 1; done; }
swap() { free -m | awk '/Swap/ {print $3}'; }
run() { local bin=$1 out=$2; shift 2
  cfg=$(mktemp -d -p $q); mkdir -p $cfg/synaps-desktop; echo '{"text_size": 16, "theme": "nord"}' > $cfg/synaps-desktop/settings.json
  quiet; local s0; s0=$(swap)
  ENABLE_GAMESCOPE_WSI=0 timeout 420 gamescope --backend headless -r 144 -W 1920 -H 1080 -w 1920 -h 1080 -- \
    env XDG_CONFIG_HOME=$cfg ./bin/$bin "$@" --refresh-hz 144 --size 1920x1080 > bench8/$out.json 2> bench8/$out.err
  echo "$(date +%T) $bin $out load $(cut -d' ' -f1 /proc/loadavg) swap_mb $s0 -> $(swap)" >> bench8/log
  rm -rf $cfg; }
for k in 1 2 3; do
  run q3r4-bench q3r4-motion-$k --bench-motion
  run main8-bench main-motion-$k --bench-motion
done
echo BENCH8-DONE >> bench8/log
