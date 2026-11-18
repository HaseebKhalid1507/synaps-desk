#!/bin/bash
# Q5 evidence: main 9b0d263 | Q5 at rest (pixel diff), and Q5 150 ms into a rail fold; four themes, 1920 and 1280.
set -uo pipefail
cd ~/.cache/q5; mkdir -p side raw
quiet() { for _ in $(seq 1 120); do awk '{exit !($1 < 8)}' /proc/loadavg && break; sleep 1; done; }
for t in nord night-city default paper; do
  for s in 1920x1080 1280x720; do
    quiet; bash shot.sh main main-$t-$s $t $s >/dev/null
    quiet; bash shot.sh q5 rest-$t-$s $t $s >/dev/null
    quiet; bash fly.sh q5 fly-$t-$s $t $s
    ffmpeg -loglevel error -y -i side/main-$t-$s.png -i side/fly-$t-$s.png -filter_complex hstack side/side-fold-$t-$s.png
    d=$(compare -metric AE side/main-$t-$s.png side/rest-$t-$s.png null: 2>&1)
    echo "rest-diff $t $s $d px $(date +%T)"
  done
done
echo EV-DONE
