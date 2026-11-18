#!/bin/bash
# Q7 r2 evidence on bella (--offline demo, headless gamescope at 144 Hz, x11grab, MESA_VK_WSI_DEBUG=sw).
# Every run gets a fresh settings.json under an ABSOLUTE XDG_CONFIG_HOME in ~/.cache/q7r2 ({"text_size": 16,
# "theme": "nord"}: the right pane as main's default, open), and bella's real settings.json is checked unchanged.
# - park: 1280x720, Full: one wheel notch up every 2.4 s from the tail to the top, so every position is held
#   over 2 s; Q7 r1 (3e2093d) then r2 (7bdac0c), the same drive.
# - card: the authorize card under the band (short windows: the card is taller than the view), r2.
# - day: the idle session (--session, its last turn the next morning under a day line), notch by notch at
#   Reduced, 1280x720 (band) and 1920x1080 (gutter), r2.
set -uo pipefail
q=~/.cache/q7r2; cd $q; mkdir -p raw clips stills
export XDG_RUNTIME_DIR=/run/user/$(id -u); unset DISPLAY WAYLAND_DISPLAY
real=~/.config/synaps-desktop/settings.json; before=$(stat -c %Y.%s $real; sha256sum $real)
quiet() { for _ in $(seq 1 120); do awk '{exit !($1 < 8)}' /proc/loadavg && break; sleep 1; done; }
park() { sleep 1.5; for _ in $(seq 1 26); do xdotool click 4; sleep 2.4; done; }
notches() { sleep 1; for _ in $(seq 1 30); do xdotool click 4; sleep 0.8; done; }
# rec <bin> <out> <size> <secs> <drive: none|park|notches> [app args]
rec() { local bin=$1 out=$q/$2 size=$3 secs=$4 drive=$5; shift 5
  local w=${size%x*} h=${size#*x}
  quiet; rm -rf $out; mkdir -p $out/config/synaps-desktop
  echo '{"text_size": 16, "theme": "nord"}' > $out/config/synaps-desktop/settings.json
  ENABLE_GAMESCOPE_WSI=0 setsid gamescope --backend headless -r 144 -W $w -H $h -w $w -h $h -- \
    bash -c "echo \$DISPLAY > $out/display; exec env XDG_CONFIG_HOME=$out/config MESA_VK_WSI_DEBUG=sw $q/bin/$bin --offline $* --size $size > $out/app.log 2>&1" \
    > $out/gs.log 2>&1 < /dev/null &
  local gs=$! wid=""
  for _ in $(seq 1 100); do [ -s $out/display ] && break; sleep 0.1; done
  export DISPLAY=$(cat $out/display)
  for _ in $(seq 1 100); do wid=$(xdotool search --classname '^synaps-desktop$' 2>/dev/null | tail -n1); [ -n "$wid" ] && break; sleep 0.1; done
  sleep 3; xdotool mousemove $((w / 2)) $((h / 2))
  ffmpeg -loglevel error -f x11grab -window_id "$wid" -framerate 30 -i "$DISPLAY" -t $secs \
    -c:v libx264rgb -qp 0 -preset ultrafast -y $out/clip.mkv &
  local ff=$!
  case $drive in park) park ;; notches) notches ;; esac
  wait $ff
  pkill -f -- "$q/bin/$bin" 2>/dev/null; kill $gs 2>/dev/null; sleep 1; kill -9 $gs 2>/dev/null; unset DISPLAY
  echo "$(date +%T) $2 $(grep -c . $out/app.log) app-log-lines"; }
pack() { local raw=$1 out=$2
  ffmpeg -loglevel error -y -i $raw -c:v libx264rgb -qp 0 -preset veryslow -pix_fmt gbrp $out
  a=$(ffmpeg -loglevel error -i $raw -f framemd5 -pix_fmt gbrp - | grep -v '^#' | md5sum | cut -c1-12)
  b=$(ffmpeg -loglevel error -i $out -f framemd5 -pix_fmt gbrp - | grep -v '^#' | md5sum | cut -c1-12)
  echo "$(basename $out) frames-md5 mkv=$a mp4=$b $(ffprobe -v error -select_streams v -show_entries stream=codec_name,profile,pix_fmt,width,height,r_frame_rate,nb_frames -of csv=p=0 $out)"; }
for b in q7-3e2093d q7-7bdac0c; do
  rec $b raw/park-$b 1280x720 68 park --motion full
  pack raw/park-$b/clip.mkv clips/park-full-1280-$b.mp4
done
for s in 1280x400 1280x440 1280x480; do
  rec q7-7bdac0c raw/card-$s $s 3 none --motion full
  ffmpeg -loglevel error -y -i raw/card-$s/clip.mkv -vf "select=eq(n\,60)" -vframes 1 stills/card-$s.png
done
for s in 1280x720 1920x1080; do
  rec q7-7bdac0c raw/day-$s $s 28 notches --motion reduced --session 20261113-161706-a1b9
done
after=$(stat -c %Y.%s $real; sha256sum $real)
[ "$before" = "$after" ] && echo "REAL-SETTINGS-UNCHANGED $after" || echo "REAL-SETTINGS-CHANGED before=$before after=$after"
echo EVIDENCE-DONE
