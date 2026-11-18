#!/bin/bash
# The palette open and left alone: app frames over 30 s (SYNAPS_DESKTOP_LOG frame lines), main at rest vs Q6 with the palette, --offline.
q=~/.cache/q6; cd $q; mkdir -p idle
export XDG_RUNTIME_DIR=/run/user/$(id -u); unset DISPLAY WAYLAND_DISPLAY
run() { local bin=$1 out=$2; shift 2
  cfg=$(mktemp -d -p $q); mkdir -p $cfg/synaps-desktop; echo '{"text_size": 16, "theme": "nord"}' > $cfg/synaps-desktop/settings.json
  ENABLE_GAMESCOPE_WSI=0 timeout 45 gamescope --backend headless -r 144 -W 1920 -H 1080 -w 1920 -h 1080 -- \
    env XDG_CONFIG_HOME=$cfg SYNAPS_DESKTOP_LOG=debug ./bin/$bin --offline --size 1920x1080 "$@" > idle/$out.log 2>&1
  rm -rf $cfg; }
run main-ev main
run q6-ev q6-goto --palette goto
run q6-ev q6-rest
for f in main q6-goto q6-rest; do echo "$f $(grep "synaps_desktop::frame\] frame" idle/$f.log | awk -F"[[ ]+" "{t=\$2} t>=10000 && t<40000" | wc -l) app frames in 10-40 s"; done
