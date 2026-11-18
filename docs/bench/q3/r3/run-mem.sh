#!/bin/bash
# Q3 r3 at f72f5ad: --bench-motion three times for mem_after_load_5x5 (gate 5 after the 5x5, like main bench6 runs mem_end).
q=~/.cache/q3; cd $q; mkdir -p bench7
export XDG_RUNTIME_DIR=/run/user/$(id -u); unset DISPLAY WAYLAND_DISPLAY
quiet() { for _ in $(seq 1 240); do awk "{exit !(\$1 < 6)}" /proc/loadavg && break; sleep 1; done; }
swap() { free -m | awk "/Swap/ {print \$3}"; }
for k in 1 2 3; do
  cfg=$(mktemp -d -p $q); mkdir -p $cfg/synaps-desktop; echo "{\"text_size\": 16, \"theme\": \"nord\"}" > $cfg/synaps-desktop/settings.json
  quiet; s0=$(swap)
  ENABLE_GAMESCOPE_WSI=0 timeout 330 gamescope --backend headless -r 144 -W 1920 -H 1080 -w 1920 -h 1080 -- env XDG_CONFIG_HOME=$cfg ./bin/q3r3g --bench-motion --refresh-hz 144 --size 1920x1080 > bench7/q3r3g-motion-$k.json 2> bench7/q3r3g-motion-$k.err
  echo "$(date +%T) q3r3g q3r3g-motion-$k load $(cut -d" " -f1 /proc/loadavg) swap_mb $s0 -> $(swap)" >> bench7/log
  rm -rf $cfg
done
echo BENCH7-DONE >> bench7/log
