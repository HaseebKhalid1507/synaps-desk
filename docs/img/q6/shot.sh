#!/bin/bash
# shot.sh <bin> <name> <theme> <size> [args...]: frame 45 of a 2 s capture → side/<name>.png
set -uo pipefail
cd ~/.cache/q6
cap=~/Projects/.worktrees/synaps-desktop-q6/scripts/capture-linux.sh
b=$1 n=$2 t=$3 s=$4; shift 4
CAPTURE_HOVER=0 CAPTURE_SIZE=$s CAPTURE_SETTINGS="{\"text_size\": 16, \"theme\": \"$t\"}" bash $cap bin/$b raw/$n 2 --offline "$@" > raw/$n.log 2>&1
ffmpeg -loglevel error -y -i raw/$n/clip.mkv -vf "select=eq(n\,45)" -vframes 1 side/$n.png
echo "side/$n.png"
