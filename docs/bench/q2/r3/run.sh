#!/bin/bash
# Gate 5 again: main (4f87411, built through dbuild in the main tree, same profile) vs Q2 (HEAD),
# --bench-motion and --bench three times each, alternating,
# each in a headless gamescope at 144 Hz 1920x1080 after the load average falls under 6 (docs/bench/q4/r4/run.sh).
# Both binaries are read whole into the page cache before every run, so neither starts colder.
# Then an attribution pass: each app idle 10 s (--offline demo), its smaps summed per mapped file.
q=~/.cache/q2-mem; mkdir -p $q/bench; cd $q; rm -f bench/*
cp ~/.cache/cargo-target-synaps-desktop/synaps-desktop/release/synaps-desktop main; cp ~/.cache/cargo-target-synaps-desktop/synaps-desktop-q2/release/synaps-desktop q2
sha256sum main q2 > bench/sums.txt; ls -l main q2 >> bench/sums.txt; size -A main q2 >> bench/sums.txt
export XDG_RUNTIME_DIR=/run/user/$(id -u); unset DISPLAY WAYLAND_DISPLAY
quiet() { for _ in $(seq 1 180); do awk '{exit !($1 < 6)}' /proc/loadavg && break; sleep 1; done; }
warm() { cat main q2 > /dev/null; }
run() { local bin=$1 out=$2; shift 2
  cfg=$(mktemp -d -p $q); mkdir -p $cfg/synaps-desktop; echo '{"text_size": 16, "theme": "nord"}' > $cfg/synaps-desktop/settings.json
  quiet; warm; echo "$(date +%T) $bin $out load $(cut -d' ' -f1 /proc/loadavg)" >> bench/log
  ENABLE_GAMESCOPE_WSI=0 timeout 260 gamescope --backend headless -r 144 -W 1920 -H 1080 -w 1920 -h 1080 -- \
    env XDG_CONFIG_HOME=$cfg ./$bin "$@" --refresh-hz 144 --size 1920x1080 > bench/$out.json 2> bench/$out.err
  rm -rf $cfg; }
for r in 1 2 3; do for b in main q2; do run $b $b-motion-$r --bench-motion; done; done
for r in 1 2 3; do for b in main q2; do run $b $b-bench-$r --bench; done; done
attr() { local bin=$1 out=$2
  cfg=$(mktemp -d -p $q); mkdir -p $cfg/synaps-desktop; echo '{"text_size": 16, "theme": "nord"}' > $cfg/synaps-desktop/settings.json
  quiet; warm
  ENABLE_GAMESCOPE_WSI=0 timeout 40 gamescope --backend headless -r 144 -W 1920 -H 1080 -w 1920 -h 1080 -- \
    env XDG_CONFIG_HOME=$cfg ./$bin --offline --size 1920x1080 > /dev/null 2>&1 &
  local gs=$!; sleep 12
  pid=$(pgrep -n -f "^./$bin --offline")
  python3 - $pid > bench/$out.smaps.txt <<'PY'
import sys, collections
pid = sys.argv[1]; tot = collections.defaultdict(lambda: [0, 0, 0]); name = None
for line in open(f'/proc/{pid}/smaps'):
    f = line.split()
    if '-' in f[0] and len(f) >= 5 and not f[0].endswith(':'):
        name = f[5] if len(f) > 5 else '[anon]'
    elif f[0] in ('Rss:', 'Pss:', 'Anonymous:'):
        tot[name][('Rss:', 'Pss:', 'Anonymous:').index(f[0])] += int(f[1])
for n, (r, p, a) in sorted(tot.items(), key=lambda x: -x[1][0])[:25]:
    print(f'{r:8d} {p:8d} {a:8d}  {n}')
print(f'{sum(v[0] for v in tot.values()):8d} {sum(v[1] for v in tot.values()):8d} {sum(v[2] for v in tot.values()):8d}  TOTAL (rss pss anon kB)')
PY
  kill $pid 2>/dev/null; wait $gs 2>/dev/null; rm -rf $cfg; }
for r in 1 2; do for b in main q2; do attr $b $b-idle-$r; done; done
echo MEM-DONE >> bench/log
