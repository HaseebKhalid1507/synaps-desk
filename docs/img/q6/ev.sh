#!/bin/bash
# Q6 evidence on bella: side by side (main at rest | Q6 with the palette), Q6 at rest vs main (pixel diff).
set -uo pipefail
cd ~/.cache/q6; mkdir -p side raw
quiet() { for _ in $(seq 1 120); do awk '{exit !($1 < 8)}' /proc/loadavg && break; sleep 1; done; }
for t in nord night-city default paper; do
  for s in 1920x1080 1280x720; do
    quiet; bash shot.sh main-ev main-$t-$s $t $s >/dev/null
    quiet; bash shot.sh q6-ev rest-$t-$s $t $s >/dev/null
    quiet; bash shot.sh q6-ev goto-$t-$s $t $s --palette goto >/dev/null
    quiet; bash shot.sh q6-ev cmds-$t-$s $t $s --palette commands >/dev/null
    ffmpeg -loglevel error -y -i side/main-$t-$s.png -i side/goto-$t-$s.png -filter_complex hstack side/side-goto-$t-$s.png
    ffmpeg -loglevel error -y -i side/main-$t-$s.png -i side/cmds-$t-$s.png -filter_complex hstack side/side-cmds-$t-$s.png
    d=$(compare -metric AE side/main-$t-$s.png side/rest-$t-$s.png null: 2>&1)
    echo "rest-diff $t $s $d px $(date +%T)"
  done
done
echo EV-DONE
