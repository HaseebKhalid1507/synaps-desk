#!/bin/bash
# Q6 r2 live check on the desktopproto sandbox (bella): the command path driving (its replies whole in the
# transcript) and observing (refused, nothing sent). Temp model creds + client_input = exclusive in the
# sandbox config for the run; restored after (sha checked). 1280x720 nord, lossless 30 fps.
set -uo pipefail
cd ~/.cache/q6r2; mkdir -p live raw stills
export XDG_RUNTIME_DIR=/run/user/$(id -u)
P=desktopproto; cfg=~/.synaps-cli/$P/config; log=live/live.log
cap=~/Projects/.worktrees/synaps-desktop-q6/scripts/capture-linux.sh
sha0=$(sha256sum $cfg | cut -c1-64); cp -p $cfg live/config.bak
echo "$(date +%T) config sha before $sha0, settings.json $(stat -c %y ~/.config/synaps-desktop/settings.json)" > $log
{ echo; grep '^auth\.' ~/.synaps-cli/config; echo 'client_input = exclusive'; } >> $cfg
echo "$(date +%T) temp lines added: $(grep -c '^auth\.' $cfg) auth, $(grep -c '^client_input' $cfg) client_input" >> $log
cleanup() {
  tmux kill-session -t q6r2 2>/dev/null
  synaps daemon stop --profile $P >> $log 2>&1; sleep 2
  [ -n "${dpid:-}" ] && kill $dpid 2>/dev/null
  cp -p live/config.bak $cfg && rm -f live/config.bak
  echo "$(date +%T) config restored sha $(sha256sum $cfg | cut -c1-64) (before $sha0), auth lines $(grep -c '^auth\.' $cfg)" >> $log
  echo "$(date +%T) settings.json $(stat -c %y ~/.config/synaps-desktop/settings.json)" >> $log
}
trap cleanup EXIT
setsid synaps daemon --profile $P > live/daemon.log 2>&1 < /dev/null &
dpid=$!; sleep 3
echo "$(date +%T) daemon pid $dpid" >> $log
# A session with one exchange: the thin line client creates it, asks, leaves.
tmux new-session -d -s q6r2 -x 150 -y 40 "synaps attach --create --name q6-palette-$(date +%H%M%S) --profile $P"
sleep 4
tmux send-keys -t q6r2 "Reply with two short sentences about Rust's borrow checker." Enter
sleep 20
tmux capture-pane -p -t q6r2 > live/tui-1.txt
tmux kill-session -t q6r2; sleep 2
synaps daemon sessions --profile $P > live/sessions.txt 2>&1
sid=$(grep -oE "[0-9]{8}-[0-9]{6}-[0-9a-z]+" live/sessions.txt | head -1); [ -n "$sid" ] || { echo "$(date +%T) no session: stop" >> $log; exit 1; }
echo "$(date +%T) session $sid" >> $log
drive() { # drive <name> <secs> [extra app args]: capture, then the keys from the drive function $name
  local n=$1 secs=$2; shift 2; local out=raw/live-$n; rm -rf $out
  CAPTURE_HOVER=0 CAPTURE_SIZE=1280x720 CAPTURE_SETTINGS='{"text_size": 16, "theme": "nord"}' \
    bash $cap bin/r2 $out $secs --profile $P --session $sid "$@" > $out.log 2>&1 &
  cpid=$!
  for _ in $(seq 1 100); do [ -s $out/display ] && break; sleep 0.1; done
  export DISPLAY=$(cat $out/display)
  for _ in $(seq 1 100); do wid=$(xdotool search --classname '^synaps-desktop$' 2>/dev/null | tail -n1); [ -n "$wid" ] && break; sleep 0.1; done
  sleep 4.5
  dlog=$out-drive.log; : > $dlog; t0=$(date +%s.%N); echo "t0 $t0" >> $dlog
  keys_$n
  wait $cpid
}
k() { echo "$(awk "BEGIN{print $(date +%s.%N) - $t0}") $*" >> $dlog; xdotool "$@"; }
mark() { echo "$(awk "BEGIN{print $(date +%s.%N) - $t0}") still $1" >> $dlog; import -window $wid stills/live-$1.png; }
cmd() { k type --delay 60 "$1"; sleep 0.3; k key Return; sleep "$2"; mark "$3"; }
keys_drive() {
  mark drive-open
  k mousemove 640 625 click 1; sleep 0.5
  cmd "/budget" 2.5 budget
  cmd "/context" 2.5 context
  cmd "/workers" 2.0 workers
  cmd "/trace" 2.0 trace
  cmd "/confirms" 2.0 confirms
  k key ctrl+k; sleep 0.6; k type --delay 80 "compact"; sleep 0.6; mark compact-palette
  k key Return; sleep 0.25; mark compact-sent; sleep 1.5; mark compact-asked
  sleep 18; mark compact-after
}
keys_observe() {
  mark observe-open
  k key ctrl+k; sleep 0.6; k type --delay 80 "compact"; sleep 0.8; mark observe-palette
  k key Return; sleep 1.0; mark observe-refused
  k mousemove 640 625 click 1; sleep 0.5
  cmd "/budget" 1.5 observe-composer
}
drive drive 50
# Another client takes the input (exclusive): this window observes.
tmux new-session -d -s q6r2 -x 150 -y 40 "synaps attach $sid --takeover --profile $P"
sleep 4
drive observe 20
tmux capture-pane -p -t q6r2 > live/tui-2.txt
echo "$(date +%T) LIVE-DONE" >> $log
