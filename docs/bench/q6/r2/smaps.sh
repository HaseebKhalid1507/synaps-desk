#!/bin/bash
# smaps.sh: each binary at idle (offline demo, 1920x1080, headless gamescope, 10 s), its USS split by mapping:
# the binary's own file, the GPU driver's mappings, other shared libraries, anonymous (heap and the rest).
cd ~/.cache/q6r2; mkdir -p mem
export XDG_RUNTIME_DIR=/run/user/$(id -u); unset DISPLAY WAYLAND_DISPLAY
quiet() { for _ in $(seq 1 120); do awk '{exit !($1 < 6)}' /proc/loadavg && break; sleep 1; done; }
for run in 1 2 3; do for b in main r2; do
  quiet
  cfg=$(mktemp -d -p ~/.cache/q6r2); mkdir -p $cfg/synaps-desktop; echo '{"text_size": 16, "theme": "nord"}' > $cfg/synaps-desktop/settings.json
  timeout 40 gamescope --backend headless -r 144 -W 1920 -H 1080 -w 1920 -h 1080 -- env XDG_CONFIG_HOME=$cfg ./bin/$b --offline --size 1920x1080 > /dev/null 2>&1 &
  for _ in $(seq 1 100); do pid=$(pgrep -f "^./bin/$b --offline" | head -1); [ -n "$pid" ] && break; sleep 0.1; done
  sleep 12
  python3 - "$pid" "$(realpath bin/$b)" > mem/$b-$run.txt <<'PY'
import sys, re
pid, binpath = sys.argv[1], sys.argv[2]
groups = {}
cur = None
gpu = re.compile(r'(libvulkan|libdrm|libgallium|libLLVM|mesa|/dri/|libnvidia|libigc|radeon|intel|libVkLayer|libgbm|libglapi|libEGL|libGL)', re.I)
for line in open(f'/proc/{pid}/smaps'):
    m = re.match(r'^[0-9a-f]+-[0-9a-f]+ \S+ \S+ \S+ \S+\s*(.*)$', line)
    if m:
        path = m.group(1).strip()
        if path == binpath or path.endswith('/bin/' + binpath.split('/')[-1]): cur = 'binary'
        elif path.startswith('/dev/dri') or gpu.search(path): cur = 'gpu driver'
        elif path.startswith('/') : cur = 'other files (libs, fonts)'
        else: cur = 'anon (heap, stacks, [..])'
        continue
    k = line.split(':')[0]
    if k in ('Private_Clean', 'Private_Dirty', 'Rss') and cur:
        g = groups.setdefault(cur, {'Private_Clean': 0, 'Private_Dirty': 0, 'Rss': 0})
        g[k] += int(line.split()[1])
tot = {k: sum(g[k] for g in groups.values()) for k in ('Private_Clean', 'Private_Dirty', 'Rss')}
for name, g in sorted(groups.items()):
    print(f"{name}\tUSS {g['Private_Clean'] + g['Private_Dirty']}\tclean {g['Private_Clean']}\tdirty {g['Private_Dirty']}\tRSS {g['Rss']}")
print(f"total\tUSS {tot['Private_Clean'] + tot['Private_Dirty']}\tclean {tot['Private_Clean']}\tdirty {tot['Private_Dirty']}\tRSS {tot['Rss']}")
PY
  kill $pid 2>/dev/null; wait 2>/dev/null; rm -rf $cfg
  echo "$(date +%T) $b $run" >> mem/log
done; done
echo MEM-DONE >> mem/log
