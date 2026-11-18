# memsum.py <bench dir>: medians of 3 runs, main vs q2, excluding GPU-driver mappings (kB), and gate 3.
import json, glob, os, statistics as st, sys
d = sys.argv[1]
def load(b, kind):
    return [json.load(open(f)) for f in sorted(glob.glob(f'{d}/{b}-{kind}-*.json')) if os.path.getsize(f) > 0]
rows = [('motion', 'mem_idle', '`--bench-motion` idle'), ('motion', 'mem_end', '`--bench-motion` end (after 5×5 load)'),
        ('bench', 'mem_idle', '`--bench` idle'), ('bench', 'mem_long_transcript_loaded', '`--bench` long transcript'),
        ('bench', 'mem_end', '`--bench` end')]
print('| point | n | main RSS | Q2 RSS | Δ | main PSS | Q2 PSS | Δ | main anon | Q2 anon | Δ |')
print('|---|---|---|---|---|---|---|---|---|---|---|')
for kind, pt, label in rows:
    m, q = load('main', kind), load('q2', kind)
    out = [label, f'{len(m)}/{len(q)}']
    for k in ('rss_kb', 'pss_kb', 'anon_kb'):
        a = st.median(x[pt]['excluding_gpu_driver'][k] for x in m); b = st.median(x[pt]['excluding_gpu_driver'][k] for x in q)
        out += [f'{a:,.0f}', f'{b:,.0f}', f'{(b - a) / a * 100:+.1f} %']
    print('| ' + ' | '.join(out) + ' |')
print()
print('per run (PSS kB, excluding GPU driver):')
for kind, pt, label in rows:
    for b in ('main', 'q2'):
        print(f'  {label:40s} {b:4s}', [x[pt]['excluding_gpu_driver']['pss_kb'] for x in load(b, kind)], 'anon', [x[pt]['excluding_gpu_driver']['anon_kb'] for x in load(b, kind)])
print()
for b in ('main', 'q2'):
    r = [x['idle_30s_prompt'] for x in load(b, 'motion')]
    print(f'gate 3 idle_30s_prompt {b}: frames_presented {[x["frames_presented"] for x in r]}, app_frames {[x["app_frames"] for x in r]}, cpu ms/s {[x["cpu_ms_per_s"] for x in r]}, prompt_pending {[x.get("prompt_pending") for x in r]}')
