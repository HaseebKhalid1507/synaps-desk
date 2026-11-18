#!/bin/bash
# Q3 r2: the settle pass (P3) live on the desktopproto sandbox, 60 fps lossless, Full then Reduced.
# Starts the sandbox daemon only if nobody runs it, and stops it only if it started it.
set -u
q=~/.cache/q3; cd $q; mkdir -p r2/p3
cap=~/Projects/.worktrees/synaps-desktop-q3/scripts/capture-linux.sh
if ! synaps daemon --profile desktopproto status 2>&1 | grep -q 'not running'; then echo "SANDBOX BUSY: not touching it"; exit 3; fi
synaps daemon --profile desktopproto start --detach --idle-exit 180 > r2/p3/daemon-start.log 2>&1
started=1
log=r2/p3/drive.log; : > $log
for lv in full reduced; do
  out=r2/p3/raw-$lv; rm -rf $out; mkdir -p $out
  CAPTURE_HOVER=0 CAPTURE_FPS=60 bash $cap bin/q3r2-ev $out 18 --profile desktopproto --motion $lv > r2/p3/cap-$lv.log 2>&1 &
  cp=$!
  for _ in $(seq 1 100); do [ -s $out/display ] && break; sleep 0.1; done
  export DISPLAY=$(cat $out/display)
  sleep 4.2   # the capture starts 3.5 s after the window
  wid=$(xdotool search --classname '^synaps-desktop$' | tail -n1)
  echo "$lv t0 $(date +%s.%N)" >> $log
  # An empty sandbox: + NEW (the rail's), then the composer.
  xdotool mousemove 234 86 click 1
  echo "$lv new $(date +%s.%N)" >> $log
  sleep 2.0
  xdotool mousemove 966 985 click 1
  sleep 0.4
  xdotool type --delay 15 "Reply in two short sentences about Rust's borrow checker."
  sleep 0.3
  echo "$lv send $(date +%s.%N)" >> $log
  xdotool key Return
  wait $cp
  echo "$lv end $(date +%s.%N)" >> $log
  unset DISPLAY
  ffmpeg -loglevel error -y -i $out/clip.mkv -c:v libx264rgb -qp 0 -preset veryslow -pix_fmt gbrp r2/p3/settle-$lv.mp4
  a=$(ffmpeg -loglevel error -i $out/clip.mkv -f framemd5 -pix_fmt gbrp - | grep -v '^#' | md5sum | cut -c1-12)
  b=$(ffmpeg -loglevel error -i r2/p3/settle-$lv.mp4 -f framemd5 -pix_fmt gbrp - | grep -v '^#' | md5sum | cut -c1-12)
  echo "settle-$lv frames-md5 mkv=$a mp4=$b" >> $log
done
[ -n "${started:-}" ] && synaps daemon --profile desktopproto stop > r2/p3/daemon-stop.log 2>&1
synaps daemon --profile desktopproto status >> r2/p3/daemon-stop.log 2>&1
echo P3-DONE >> $log
