#!/bin/bash
cd ~/.cache/q6r3
export XDG_RUNTIME_DIR=/run/user/$(id -u)
quiet() { for _ in $(seq 1 120); do awk '{exit !($1 < 8)}' /proc/loadavg && break; sleep 1; done; }
for t in nord paper; do for s in 1280x720 1920x1080; do
  n=r3-$t-${s%x*}; [ -f stills/$n-goto-open.png ] && continue
  quiet; bash design.sh r3 $t $s $n
done; done
echo DESIGNS-DONE
