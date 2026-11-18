#!/bin/bash
# sheets.sh: frame sheets cut from the three clips (run where the clips are; ffmpeg drawtext labels each frame f<n>).
set -uo pipefail
sheet() { local c=$1 a=$2 b=$3 st=$4 cols=$5 crop=$6 sc=$7 o=$8; local n=$(( (b - a) / st + 1 )); local rows=$(( (n + cols - 1) / cols ))
  ffmpeg -loglevel error -y -i panels-$c.mp4 -vf "select='between(n\,$a\,$b)*not(mod(n-$a\,$st))',crop=$crop,drawtext=text='f%{eif\\:$a+n*$st\\:d}':x=6:y=6:fontsize=16:fontcolor=white:box=1:boxcolor=black,scale=$sc:-1,tile=${cols}x${rows}" -frames:v 1 -fps_mode passthrough $o.png; echo $o.png; }
sheet full 118 154 3 13 640:420:0:52 240 full-rail-fold-f118-154
sheet full 198 234 3 13 640:420:0:52 240 full-rail-unfold-f198-234
sheet full 278 314 3 13 640:420:640:52 240 full-right-fold-f278-314
sheet full 450 482 2 17 320:200:160:300 200 full-edge-detent-f450-482
sheet full 744 804 4 8 700:260:300:420 350 full-stub-dock-f744-804
sheet full 884 944 4 8 700:260:300:420 350 full-stub-leaves-f884-944
sheet full 1122 1158 3 13 310:420:0:300 160 full-named-fold-f1122-1158
sheet full 1206 1242 3 13 310:420:0:300 160 full-named-unfold-f1206-1242
sheet reduced 117 125 1 9 640:420:0:52 240 reduced-rail-fold-f117-125
sheet off 117 125 1 9 640:420:0:52 240 off-rail-fold-f117-125
sheet reduced 1122 1130 1 9 310:420:0:300 160 reduced-named-fold-f1122-1130
