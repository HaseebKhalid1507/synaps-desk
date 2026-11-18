#!/bin/bash
cd ~/.cache/q6; mkdir -p sheets
sheet() { # clip first last step cols out
  local c=$1 a=$2 b=$3 st=$4 cols=$5 o=$6
  local n=$(( (b - a) / st + 1 )); local rows=$(( (n + cols - 1) / cols ))
  ffmpeg -loglevel error -y -i clips/palette-$c.mp4 -vf "select='between(n\,$a\,$b)*not(mod(n-$a\,$st))',crop=640:540:330:60,drawtext=text='f%{n}':x=8:y=8:fontsize=18:fontcolor=white:box=1:boxcolor=black,scale=320:-1,tile=${cols}x${rows}" -frames:v 1 -fps_mode passthrough sheets/$o.png 2>&1 | tail -2
  echo sheets/$o.png
}
sheet full 85 90 1 6 full-open-f85-90
sheet full 161 174 1 7 full-quad-f161-174
sheet full 333 361 2 8 full-height-f333-361
sheet reduced 160 165 1 6 reduced-quad-f160-165
sheet off 160 165 1 6 off-quad-f160-165
sheet reduced 333 338 1 6 reduced-height-f333-338
sheet off 333 338 1 6 off-height-f333-338
