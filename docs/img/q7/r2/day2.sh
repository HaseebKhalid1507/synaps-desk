#!/bin/bash
# Q7 r2: a day line pushing a pinned label, at rest. The idle session (--session: its last turn the next morning,
# under a day line) at the tail, in a window short enough that the day line rests inside the pinned AGENT's push
# range (labels hang): 1920x536 (kept) and 1920x540.
# Fresh settings under an ABSOLUTE XDG_CONFIG_HOME in ~/.cache/q7r2; bella's real settings.json checked unchanged.
set -uo pipefail
q=~/.cache/q7r2; cd $q; mkdir -p raw stills
export XDG_RUNTIME_DIR=/run/user/$(id -u); unset DISPLAY WAYLAND_DISPLAY
real=~/.config/synaps-desktop/settings.json; before=$(stat -c %Y.%s $real; sha256sum $real)
for spec in "1920x536 0" "1920x540 0"; do
  sleep 3
  set -- $spec; size=$1 n=$2; w=${size%x*} h=${size#*x} out=$q/raw/day2-$size
  rm -rf $out; mkdir -p $out/config/synaps-desktop
  echo '{"text_size": 16, "theme": "nord"}' > $out/config/synaps-desktop/settings.json
  ENABLE_GAMESCOPE_WSI=0 setsid gamescope --backend headless -r 144 -W $w -H $h -w $w -h $h -- \
    bash -c "echo \$DISPLAY > $out/display; exec env XDG_CONFIG_HOME=$out/config MESA_VK_WSI_DEBUG=sw $q/bin/q7-7bdac0c --offline --motion reduced --session 20261113-161706-a1b9 --size $size > $out/app.log 2>&1" \
    > $out/gs.log 2>&1 < /dev/null &
  gs=$!; wid=""
  for _ in $(seq 1 100); do [ -s $out/display ] && break; sleep 0.1; done
  export DISPLAY=$(cat $out/display)
  for _ in $(seq 1 100); do wid=$(xdotool search --classname '^synaps-desktop$' 2>/dev/null | tail -n1); [ -n "$wid" ] && break; sleep 0.1; done
  sleep 3; xdotool mousemove $((w / 2)) $((h / 3))
  for _ in $(seq 1 $n); do xdotool click 4; sleep 0.5; done; sleep 1
  ffmpeg -loglevel error -f x11grab -window_id "$wid" -framerate 30 -i "$DISPLAY" -t 1 -c:v libx264rgb -qp 0 -preset ultrafast -y $out/clip.mkv
  ffmpeg -loglevel error -y -i $out/clip.mkv -vf "select=eq(n\,15)" -vframes 1 stills/day-$size.png
  pkill -f -- "$q/bin/q7-7bdac0c" 2>/dev/null; kill $gs 2>/dev/null; sleep 1; kill -9 $gs 2>/dev/null; unset DISPLAY
  echo "$(date +%T) day2 $size"
done
after=$(stat -c %Y.%s $real; sha256sum $real)
[ "$before" = "$after" ] && echo "REAL-SETTINGS-UNCHANGED $after" || echo "REAL-SETTINGS-CHANGED before=$before after=$after"
echo DAY2-DONE
