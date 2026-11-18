#!/bin/bash
# Record synaps-desktop's window, lossless at 30 fps, in a headless gamescope (never the real
# display), and stop everything it starts.
# usage: scripts/capture-linux.sh <binary> <outdir> <seconds> [synaps-desktop args...]
#   e.g. scripts/capture-linux.sh target/release/synaps-desktop /tmp/cap 30 --offline
# env:
#   CAPTURE_SIZE=1920x1080       the window (and gamescope's output)
#   CAPTURE_HOVER=1              the pointer between two rail rows 8 times a second, so frames are
#                                drawn (an idle window draws none); 0: left still
#   CAPTURE_SETTINGS='{...}'     the settings file it starts with (default: text 16, nord)
#   CAPTURE_WSI=sw               how the app's frames reach the X server (below); `dri3` is the
#                                driver's own path, for showing the capture artefact
#   CAPTURE_FPS=30               the recording's frame rate (60 for frame-by-frame motion)
#   CAPTURE_HZ=144               gamescope's refresh (the app's display rate)
# Leaves <outdir>/clip.mkv (libx264rgb -qp 0: every pixel as drawn), app.log, gs.log.
# Frame-by-frame: scripts/frame-dips.py <outdir>/clip.mkv.
#
# Why MESA_VK_WSI_DEBUG=sw: x11grab (like `import -window`) reads the window's pixmap through
# Xwayland (XGetImage). On Mesa's Vulkan driver for Intel (ANV, Mesa 26.2) presenting over DRI3
# with explicit sync to Xwayland 24.1, that read can return a frame without its last draw call,
# and keeps returning it until the next frame is presented: on bella, the code blocks' and
# inline code's text (the transcript's last batch of glyphs) vanished for one frame every few
# seconds. The compositor's own output never shows it (headless sway + grim: 0 of 1825 samples),
# and the frame GPUI hands the renderer is the same, byte for byte
# (docs/bench/text-dropout/README.md). `sw` makes Mesa wait for the frame before it copies it to
# the window, so the window's pixmap is always the whole frame. The pixels are the same; the
# present costs more CPU, so never measure performance with it.
set -euo pipefail
bin=$(realpath "$1"); out="$2"; secs="$3"; shift 3
size="${CAPTURE_SIZE:-1920x1080}"; w="${size%x*}"; h="${size#*x}"
mkdir -p "$out"; out=$(realpath "$out")
rm -f "$out/clip.mkv" "$out/display" "$out/app.log" "$out/gs.log"
cfg="$out/config"; rm -rf "$cfg"; mkdir -p "$cfg/synaps-desktop"
default_settings='{"text_size": 16, "theme": "nord"}'
echo "${CAPTURE_SETTINGS:-$default_settings}" > "$cfg/synaps-desktop/settings.json"
wsi=()
case "${CAPTURE_WSI:-sw}" in
  sw) wsi=(MESA_VK_WSI_DEBUG=sw) ;;
  dri3) ;;
  *) echo "CAPTURE_WSI: sw or dri3" >&2; exit 2 ;;
esac
app=$(printf '%q ' env XDG_CONFIG_HOME="$cfg" "${wsi[@]}" "$bin" "$@" --size "$size")
export XDG_RUNTIME_DIR=/run/user/$(id -u)
unset DISPLAY WAYLAND_DISPLAY
ENABLE_GAMESCOPE_WSI=0 setsid gamescope --backend headless -r "${CAPTURE_HZ:-144}" -W "$w" -H "$h" -w "$w" -h "$h" -- \
  bash -c "echo \$DISPLAY > '$out/display'; exec $app > '$out/app.log' 2>&1" > "$out/gs.log" 2>&1 < /dev/null &
gs=$!
hov=""
cleanup() {
  [ -n "$hov" ] && kill "$hov" 2>/dev/null || true
  pkill -f -- "$bin" 2>/dev/null || true
  kill "$gs" 2>/dev/null || true; sleep 1; kill -9 "$gs" 2>/dev/null || true
}
trap cleanup EXIT
for _ in $(seq 1 100); do [ -s "$out/display" ] && break; sleep 0.1; done
export DISPLAY=$(cat "$out/display")
wid=""
for _ in $(seq 1 100); do wid=$(xdotool search --classname '^synaps-desktop$' 2>/dev/null | tail -n1 || true); [ -n "$wid" ] && break; sleep 0.1; done
[ -n "$wid" ] || { echo "no window" >&2; cat "$out/app.log" >&2; exit 1; }
sleep 3
xdotool mousemove $((w - 20)) $((h - 10))
if [ "${CAPTURE_HOVER:-1}" = 1 ]; then
  ( end=$((SECONDS + secs + 1)); while [ $SECONDS -lt $end ]; do
      xdotool mousemove 100 440; sleep 0.125; xdotool mousemove 100 494; sleep 0.125
    done ) &
  hov=$!
fi
sleep 0.5
# The window's own pixels (-window_id): recording the root under gamescope gives black frames.
ffmpeg -loglevel error -f x11grab -window_id "$wid" -framerate "${CAPTURE_FPS:-30}" -i "$DISPLAY" -t "$secs" \
  -c:v libx264rgb -qp 0 -preset ultrafast -y "$out/clip.mkv"
echo "$out/clip.mkv (${CAPTURE_WSI:-sw}, ${secs} s)"
