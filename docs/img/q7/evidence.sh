#!/bin/bash
# Q7 evidence on bella (--offline demo, headless gamescope at 144 Hz, x11grab, MESA_VK_WSI_DEBUG=sw as
# scripts/capture-linux.sh): side by side at rest, main 0a78bb7 | Q7, 4 themes x 2 sizes; the same after six
# wheel notches up at Reduced (the same scroll position on both); then the wheel clips at Full / Reduced / Off
# (1920), Full (1280) and Full (1024: labels inline, F8's band), packed lossless and checked frame by frame against the capture.
set -uo pipefail
q=~/.cache/q7; cd $q; mkdir -p raw side clips
export XDG_RUNTIME_DIR=/run/user/$(id -u); unset DISPLAY WAYLAND_DISPLAY
quiet() { for _ in $(seq 1 120); do awk '{exit !($1 < 8)}' /proc/loadavg && break; sleep 1; done; }
up6() { sleep 1; for _ in 1 2 3 4 5 6; do xdotool click 4; sleep 0.3; done; }
wheel() { sleep 1; for _ in $(seq 1 24); do xdotool click 4; sleep 0.35; done; sleep 1
  for _ in $(seq 1 12); do xdotool click 5; sleep 0.35; done; }
# rec <bin> <out> <size> <secs> <fps> <theme> <drive: none|up6|wheel> [app args]
# (XDG_CONFIG_HOME must be absolute, or the app reads no settings: the first run's side shots were all one theme.)
rec() { local bin=$1 out=$q/$2 size=$3 secs=$4 fps=$5 theme=$6 drive=$7; shift 7
  local w=${size%x*} h=${size#*x}
  quiet; rm -rf $out; mkdir -p $out/config/synaps-desktop
  echo "{\"text_size\": 16, \"theme\": \"$theme\"}" > $out/config/synaps-desktop/settings.json
  ENABLE_GAMESCOPE_WSI=0 setsid gamescope --backend headless -r 144 -W $w -H $h -w $w -h $h -- \
    bash -c "echo \$DISPLAY > $out/display; exec env XDG_CONFIG_HOME=$out/config MESA_VK_WSI_DEBUG=sw $q/bin/$bin --offline $* --size $size > $out/app.log 2>&1" \
    > $out/gs.log 2>&1 < /dev/null &
  local gs=$! wid=""
  for _ in $(seq 1 100); do [ -s $out/display ] && break; sleep 0.1; done
  export DISPLAY=$(cat $out/display)
  for _ in $(seq 1 100); do wid=$(xdotool search --classname '^synaps-desktop$' 2>/dev/null | tail -n1); [ -n "$wid" ] && break; sleep 0.1; done
  sleep 3; xdotool mousemove $((w / 2)) $((h / 2))
  ffmpeg -loglevel error -f x11grab -window_id "$wid" -framerate $fps -i "$DISPLAY" -t $secs \
    -c:v libx264rgb -qp 0 -preset ultrafast -y $out/clip.mkv &
  local ff=$!
  case $drive in up6) up6 ;; wheel) wheel ;; esac
  wait $ff
  pkill -f -- "$q/bin/$bin" 2>/dev/null; kill $gs 2>/dev/null; sleep 1; kill -9 $gs 2>/dev/null; unset DISPLAY; }
pack() { local raw=$1 out=$2
  ffmpeg -loglevel error -y -i $raw -c:v libx264rgb -qp 0 -preset veryslow -pix_fmt gbrp $out
  a=$(ffmpeg -loglevel error -i $raw -f framemd5 -pix_fmt gbrp - | grep -v '^#' | md5sum | cut -c1-12)
  b=$(ffmpeg -loglevel error -i $out -f framemd5 -pix_fmt gbrp - | grep -v '^#' | md5sum | cut -c1-12)
  echo "$(basename $out) frames-md5 mkv=$a mp4=$b $(ffprobe -v error -select_streams v -show_entries stream=codec_name,profile,pix_fmt,width,height,r_frame_rate,nb_frames -of csv=p=0 $out)"; }
mode=${1:-all}
if [ "$mode" = all ] || [ "$mode" = side ]; then
for t in nord night-city default paper; do for s in 1920x1080 1280x720; do
  for b in main-0a78bb7 q7-3e2093d; do
    rec $b raw/$b-$t-$s $s 2 30 $t none
    ffmpeg -loglevel error -y -i raw/$b-$t-$s/clip.mkv -vf "select=eq(n\,45)" -vframes 1 side/$b-$t-$s.png; rm -rf raw/$b-$t-$s
  done
  ffmpeg -loglevel error -y -i side/main-0a78bb7-$t-$s.png -i side/q7-3e2093d-$t-$s.png -filter_complex hstack side/side-$t-$s.png
  echo "side $t $s $(date +%T)"
done; done
for s in 1920x1080 1280x720 1024x720; do
  for b in main-0a78bb7 q7-3e2093d; do
    rec $b raw/up-$b-$s $s 4 30 nord up6 --motion reduced
    ffmpeg -loglevel error -y -i raw/up-$b-$s/clip.mkv -vf "select=eq(n\,105)" -vframes 1 side/up-$b-$s.png; rm -rf raw/up-$b-$s
  done
  ffmpeg -loglevel error -y -i side/up-main-0a78bb7-$s.png -i side/up-q7-3e2093d-$s.png -filter_complex hstack side/scrolled-nord-$s.png
  echo "scrolled $s $(date +%T)"
done
fi
if [ "$mode" = all ] || [ "$mode" = clips ]; then
for spec in "full 1920x1080" "reduced 1920x1080" "off 1920x1080" "full 1280x720"; do
  set -- $spec; name=sticky-$1-${2%x*}
  rec q7-3e2093d raw/$name $2 16 30 nord wheel --motion $1
  pack raw/$name/clip.mkv clips/$name.mp4
done
fi
if [ "$mode" = all ] || [ "$mode" = band ]; then
# F8: 1280 hangs the labels (the pane is over 872 px without the right panel), so the band is shown at 1024.
rec q7-3e2093d raw/sticky-full-1024 1024x720 16 30 nord wheel --motion full
pack raw/sticky-full-1024/clip.mkv clips/sticky-full-1024.mp4
fi
echo EVIDENCE-DONE
