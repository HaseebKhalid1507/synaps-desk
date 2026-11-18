#!/bin/bash
# Q3 r3: main 9121dba (Q2's release build of the identical tree c81ab14) vs Q3 r3, alternating, Q3 first.
# --bench-motion three times each (gates 3, 4; mem_end after the 5x5 = gate 5), then --bench three times each
# (mem after 2,000 rows). Swap in use logged before and after every run; a run whose RSS at the end is under
# its idle RSS was paged out (skipped in the tables).
q=~/.cache/q3; cd $q; mkdir -p bench6 bin
[ -n "${Q3BIN:-}" ] && cp "$Q3BIN" bin/q3r3-bench
cp ~/.local/bin/synaps-desktop-hybrid.q2 bin/main6-bench
export XDG_RUNTIME_DIR=/run/user/$(id -u); unset DISPLAY WAYLAND_DISPLAY
quiet() { for _ in $(seq 1 240); do awk '{exit !($1 < 6)}' /proc/loadavg && break; sleep 1; done; }
swap() { free -m | awk '/Swap/ {print $3}'; }
run() { local bin=$1 out=$2; shift 2
  cfg=$(mktemp -d -p $q); mkdir -p $cfg/synaps-desktop; echo '{"text_size": 16, "theme": "nord"}' > $cfg/synaps-desktop/settings.json
  quiet; local s0; s0=$(swap)
  ENABLE_GAMESCOPE_WSI=0 timeout 330 gamescope --backend headless -r 144 -W 1920 -H 1080 -w 1920 -h 1080 -- \
    env XDG_CONFIG_HOME=$cfg ./bin/$bin "$@" --refresh-hz 144 --size 1920x1080 > bench6/$out.json 2> bench6/$out.err
  echo "$(date +%T) $bin $out load $(cut -d' ' -f1 /proc/loadavg) swap_mb $s0 -> $(swap) gamescopes $(pgrep -c gamescope)" >> bench6/log
  rm -rf $cfg; }
for k in ${RUNS:-1 2 3}; do
  run q3r3-bench q3r3-motion-$k --bench-motion
  [ "${ONLY_Q3:-}" = 1 ] && continue
  run main6-bench main-motion-$k --bench-motion
done
if [ "${ONLY_Q3:-}" != 1 ]; then
  for k in 1 2 3; do run q3r3-bench q3r3-mem-$k --bench; run main6-bench main-mem-$k --bench; done
fi
echo BENCH6-DONE >> bench6/log
