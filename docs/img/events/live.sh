#!/bin/bash
# Events live check on the desktopproto sandbox (bella). Phases, state in ~/.cache/events-live:
#   setup     config backup + temp model creds, the sandbox daemon, sessions ev-main and ev-side
#   order     the frame order of an idle wake and a mid-turn event (frames.py, an Observe attach)
#   record    the desk on ev-main, 1920 nord, lossless: events low/medium/high/critical, a channel,
#             expects_response, mid-turn (a bash sleep) and idle, one to ev-side (the rail/tab mark)
#   reload    the desk again on the same session (the history form): stills, 1920/1280 x nord/paper
#   stream    round 2 (Shady #22 H1): the desk on ev-main, 1920 nord, lossless; two events sent while
#             the model streams its reply (frames.py logs where they land among the deltas), then a
#             local_only one; `reload` after it shows the history's rows
#   teardown  daemon stopped, config restored (sha checked), settings.json mtime logged
# Every app run: XDG_CONFIG_HOME absolute under ~/.cache (capture-linux.sh). Kills only its own PIDs.
set -uo pipefail
W=~/.cache/events-live; mkdir -p $W/live $W/raw $W/stills; cd $W
export XDG_RUNTIME_DIR=/run/user/$(id -u)
P=desktopproto; cfg=~/.synaps-cli/$P/config; log=$W/live/live.log; RUN=~/.synaps-cli/run
KIT=~/Projects/.worktrees/synaps-desktop-events/docs/img/events
cap=~/Projects/.worktrees/synaps-desktop-events/scripts/capture-linux.sh
BIN=$W/bin/synaps-desktop
say() { echo "$(date +%T) $*" | tee -a $log; }
sid_of() { grep -oE "[0-9]{8}-[0-9]{6}-[0-9a-z]+" <(synaps daemon sessions --profile $P 2>/dev/null | grep -- "$1") | head -1; }
send() { synaps send --profile $P --session "$@" 2>&1 | tee -a $log; }

case "${1:-}" in
setup)
  [ -f live/config.bak ] && { say "already set up"; exit 1; }
  sha0=$(sha256sum $cfg | cut -c1-64); cp -p $cfg live/config.bak; echo $sha0 > live/config.sha0
  say "config sha before $sha0; settings.json mtime $(stat -c %y ~/.config/synaps-desktop/settings.json)"
  { echo; grep '^auth\.' ~/.synaps-cli/config; } >> $cfg
  say "temp auth lines added: $(grep -c '^auth\.' $cfg)"
  setsid synaps daemon --profile $P > live/daemon.log 2>&1 < /dev/null &
  echo $! > live/daemon.pid; sleep 3; say "daemon pid $(cat live/daemon.pid)"
  for n in ev-main ev-side; do
    tmux new-session -d -s evlive -x 150 -y 40 "synaps attach --create --name $n-$(date +%H%M%S) --profile $P"
    sleep 4; tmux send-keys -t evlive "Reply in one short sentence: what is a lighthouse for?" Enter; sleep 14
    tmux capture-pane -p -t evlive > live/tui-$n.txt; tmux kill-session -t evlive; sleep 2
  done
  synaps daemon sessions --profile $P > live/sessions.txt 2>&1
  say "main $(sid_of ev-main) side $(sid_of ev-side)"
  ;;
order)
  sid=$(sid_of ev-main); sock=$(python3 -c "import json;print(json.load(open('$RUN/daemon-$P.json'))['socket'])")
  python3 $KIT/frames.py "$sock" "$sid" 75 live/frames.log & fp=$!; sleep 2
  echo "== idle: synaps send (high, channel alerts) wakes a turn" >> live/frames.log
  send "$sid" --severity high --channel alerts "Synthetic alert: disk on build-01 at 91%. Acknowledge in one short sentence."
  sleep 22
  echo "== mid-turn: a turn running bash sleep, then synaps send (medium)" >> live/frames.log
  tmux new-session -d -s evlive -x 150 -y 40 "synaps attach $sid --profile $P"; sleep 4
  tmux send-keys -t evlive "Use the bash tool to run exactly: sleep 12; echo slept. Then reply with one word: done." Enter
  sleep 9; tmux capture-pane -p -t evlive > live/tui-order-1.txt
  send "$sid" --severity medium "Synthetic note: the nightly backup finished."
  sleep 30; tmux capture-pane -p -t evlive > live/tui-order-2.txt; tmux kill-session -t evlive
  wait $fp; say "frames logged: $(wc -l < live/frames.log) lines"
  ;;
record|reload|stream)
  sid=$(sid_of ev-main); side=$(sid_of ev-side); phase=$1
  run() { # run <name> <size> <theme> <secs> <keys fn>
    local n=$1 size=$2 theme=$3 secs=$4 fn=$5 out=raw/$1; rm -rf $out
    CAPTURE_HOVER=0 CAPTURE_SIZE=$size CAPTURE_SETTINGS="{\"text_size\": 16, \"theme\": \"$theme\"}" \
      bash $cap $BIN $out $secs --profile $P --session $sid > $out.log 2>&1 &
    cpid=$!
    for _ in $(seq 1 100); do [ -s $out/display ] && break; sleep 0.1; done
    export DISPLAY=$(cat $out/display)
    for _ in $(seq 1 100); do wid=$(xdotool search --classname '^synaps-desktop$' 2>/dev/null | tail -n1); [ -n "$wid" ] && break; sleep 0.1; done
    sleep 4.5; dlog=$out-drive.log; : > $dlog; t0=$(date +%s.%N); echo "t0 $t0" >> $dlog
    $fn "$size" "$theme"
    wait $cpid
  }
  k() { echo "$(awk "BEGIN{print $(date +%s.%N) - $t0}") $*" >> $dlog; xdotool "$@"; }
  mark() { echo "$(awk "BEGIN{print $(date +%s.%N) - $t0}") still $1" >> $dlog; import -window $wid stills/$1.png; }
  ev() { echo "$(awk "BEGIN{print $(date +%s.%N) - $t0}") event $*" >> $dlog; }
  keys_record() {
    mark live-open
    ev "low, synaps send"; send "$sid" --severity low "Synthetic note: cache warmed on build-01 in 42 s."
    sleep 7; mark live-low
    ev "medium, channel"; send "$sid" --severity medium --channel deploys "Synthetic deploy: web-03 is on release 7.4.1."
    sleep 9; mark live-medium-channel
    ev "comms ask, expects_response"
    python3 $KIT/inject.py $RUN "$sid" '{"text": "[comms] #7 ask from kestrel: is the staging build green? one line please.\n↩ comms_send(re=7, text=...)", "channel": {"id": "comms", "name": "comms"}, "sender": {"id": "sid-0001", "name": "kestrel", "sender_type": "agent"}, "expects_response": true}' >> $dlog
    sleep 10; mark live-ask
    ev "high"; send "$sid" --severity high --channel alerts "Synthetic alert: disk on build-01 at 91%."
    sleep 9; mark live-high
    ev "critical, long"
    send "$sid" --severity critical --source uptime-watch "$(printf 'Synthetic outage: api-02 is DOWN (503).\nchecked from 3 regions\nfirst failure 04:05:06Z\nlast success 04:03:11Z\nretries 5 of 5\nowner: platform\nrunbook: restart the api pool\nthis line is past the fold')"
    sleep 10; mark live-critical
    ev "mid-turn: prompt a bash sleep"
    k mousemove 960 985 click 1; sleep 0.4
    k type --delay 30 "Use the bash tool to run exactly: sleep 12; echo slept. Then reply with one word: done."; k key Return
    sleep 6; mark live-midturn-running
    ev "medium during the tool"; send "$sid" --severity medium "Synthetic note: the nightly backup finished."
    sleep 3; mark live-midturn-event
    sleep 20; mark live-midturn-after
    ev "to ev-side (not on screen)"; send "$side" --severity high "Synthetic alert: queue depth on worker-07 is 1200."
    sleep 26; mark live-side-mark
    k mousemove 960 400; k click 4 click 4 click 4 click 4 click 4 click 4; sleep 1.0; mark live-scrolled
    ev "low while scrolled up"; send "$sid" --severity low "Synthetic note: a scrolled-up view stays put."
    sleep 9; mark live-scrolled-after
  }
  keys_stream() {
    mark stream-open
    k mousemove 960 985 click 1; sleep 0.4
    k type --delay 25 "Write 40 numbered lines, one short sentence each, on how a lighthouse lens works. No tools, no preamble."
    k key Return; sleep 2.2
    ev "high, while it streams"; send "$sid" --severity high --channel alerts "Synthetic alert: disk on build-01 at 93%."
    sleep 0.5; mark stream-held-1
    sleep 1.6; ev "medium, while it streams"; send "$sid" --severity medium "Synthetic note: deploy 7.4.2 finished on web-03."
    sleep 0.5; mark stream-held-2
    sleep 1.5; mark stream-held-3
    sleep 3; mark stream-held-4
    sleep 22; mark stream-after
    ev "local_only (inject)"
    python3 $KIT/inject.py $RUN "$sid" '{"text": "Synthetic local note: one-time code 482913 (shown here, never sent to the agent).", "source": {"source_type": "cli", "name": "cli", "callback": null}, "content": {"text": "Synthetic local note: one-time code 482913 (shown here, never sent to the agent).", "content_type": "message", "severity": "Medium", "data": null, "disclosure": "local_only"}}' >> $dlog
    sleep 14; mark stream-local
  }
  keys_reload() {
    sleep 2; mark "reload-$1-$2"; k mousemove 640 300; k click 4 click 4 click 4; sleep 1; mark "reload-$1-$2-up"
    for _ in 1 2 3; do k click 4 click 4 click 4; sleep 0.3; done; sleep 1; mark "reload-$1-$2-up2"
    for _ in 1 2 3; do k click 4 click 4 click 4; sleep 0.3; done; sleep 1; mark "reload-$1-$2-up3"
  }
  if [ $phase = record ]; then
    run live-1920-nord 1920x1080 nord 150 keys_record
  elif [ $phase = stream ]; then
    sock=$(python3 -c "import json;print(json.load(open('$RUN/daemon-$P.json'))['socket'])")
    python3 $KIT/frames.py "$sock" "$sid" 75 live/frames-stream.log & fp=$!; sleep 1
    run stream-1920-nord 1920x1080 nord 66 keys_stream
    wait $fp; say "stream frames logged: $(wc -l < live/frames-stream.log) lines"
  else
    run reload-1920-nord 1920x1080 nord 18 keys_reload
    run reload-1920-paper 1920x1080 paper 18 keys_reload
    run reload-1280-nord 1280x720 nord 18 keys_reload
    run reload-1280-paper 1280x720 paper 18 keys_reload
  fi
  say "$phase done"
  ;;
teardown)
  tmux kill-session -t evlive 2>/dev/null
  synaps daemon stop --profile $P >> $log 2>&1; sleep 2
  [ -f live/daemon.pid ] && kill "$(cat live/daemon.pid)" 2>/dev/null
  [ -f live/config.bak ] && cp -p live/config.bak $cfg && rm -f live/config.bak
  say "config restored sha $(sha256sum $cfg | cut -c1-64) (before $(cat live/config.sha0)), auth lines $(grep -c '^auth\.' $cfg)"
  say "settings.json mtime $(stat -c %y ~/.config/synaps-desktop/settings.json)"
  ;;
*) echo "usage: live.sh setup|order|record|reload|stream|teardown"; exit 2 ;;
esac
