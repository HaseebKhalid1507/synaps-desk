import json, glob, os, re
q = os.path.expanduser('~/.cache/q6/bench')
rows = []
for f in sorted(glob.glob(q + '/*-motion-*.json'), key=lambda p: (p.split('-')[-1], p)):
    try:
        d = json.load(open(f))
    except Exception as e:
        print(os.path.basename(f), 'unreadable', e); continue
    i = d['idle_30s_prompt']
    ex = lambda k: d.get(k, {}).get('excluding_gpu_driver', {})
    mi, ma, me = ex('mem_idle'), ex('mem_after_load_5x5'), ex('mem_end')
    rows.append((os.path.basename(f)[:-5], i['frames_presented'], i['app_frames'], i['layer_paints'], i['cpu_ms_per_s'], i['prompt_pending'], i['demoted_worker'],
                 mi.get('anon_kb'), mi.get('uss_kb'), ma.get('anon_kb'), ma.get('uss_kb'), me.get('anon_kb'), me.get('uss_kb'),
                 d['mem_idle']['rss_kb'], d.get('mem_end', {}).get('rss_kb'), d.get('total_s')))
print('| run | idle_30s_prompt presents / app frames / layer paints / cpu ms/s | pending, demoted | mem_idle anon / USS | after 5×5 anon / USS | mem_end anon / USS | RSS idle → end | total s |')
print('|---|---|---|---|---|---|---|---|')
for r in rows:
    print(f'| {r[0]} | {r[1]} / {r[2]} / {r[3]} / {r[4]} | {r[5]}, {r[6]} | {r[7]:,} / {r[8]:,} | {r[9]:,} / {r[10]:,} | {r[11]:,} / {r[12]:,} | {r[13]:,} → {r[14]:,} | {r[15]} |')
def mean(xs): return sum(xs) / len(xs)
for k, name in [(9, 'after 5x5 anon'), (10, 'after 5x5 USS'), (11, 'mem_end anon'), (12, 'mem_end USS')]:
    m = [r[k] for r in rows if r[0].startswith('main')]; s = [r[k] for r in rows if r[0].startswith('q6')]
    if m and s and None not in m + s:
        print(f'{name}: main {mean(m):,.0f}  q6 {mean(s):,.0f}  {100 * (mean(s) - mean(m)) / mean(m):+.2f} %')
