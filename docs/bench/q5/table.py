# table.py <dir>: the README's tables from the raw JSON (means over the three runs).
import json, os, statistics as st, sys
os.chdir(sys.argv[1] if len(sys.argv) > 1 else '.')
acts = ["rail_fold", "rail_unfold", "right_fold", "right_unfold", "composer_grow", "composer_shrink"]
log = {l.split()[2]: l.split() for l in open('log') if len(l.split()) > 5}
def runs(who, size):
    return [json.load(open(f"{who}-panels-{size}-{i}.json")) for i in (1, 2, 3)]
m = lambda xs: round(st.mean(xs), 2)
for size in ["1280x720", "1920x1080"]:
    print(f"\n**{size}** (means of 3; draw = the app's draw time per presented frame, ms)\n")
    print("| action | main presents | Q5 presents | main draw p95 / max | Q5 draw p95 / max | over budget, dropped (3 runs) main / Q5 | main CPU ms | Q5 CPU ms | after (presents, app frames) main / Q5 |")
    print("|---|---|---|---|---|---|---|---|---|")
    R = {w: runs(w, size) for w in ("main", "q5")}
    for a in acts:
        c = {w: [r[a] for r in R[w]] for w in R}
        pres = {w: m([s['frames_presented'] for s in c[w]]) for w in c}
        p95 = {w: m([s['draw_ms']['p95'] for s in c[w]]) for w in c}
        mx = {w: m([s['draw_ms']['max'] for s in c[w]]) for w in c}
        cpu = {w: m([s['cpu_ms'] for s in c[w]]) for w in c}
        aft = {w: f"{sum(s['presents_after'] for s in c[w])}, {sum(s['app_frames_after'] for s in c[w])}" for w in c}
        ob = {w: f"{sum(s['draws_over_budget'] for s in c[w])}, {sum(s['dropped_frames'] for s in c[w])}" for w in c}
        print(f"| {a} | {pres['main']} | {pres['q5']} | {p95['main']} / {mx['main']} | {p95['q5']} / {mx['q5']} | {ob['main']} / {ob['q5']} | {cpu['main']} | {cpu['q5']} | {aft['main']} / {aft['q5']} |")
print("\n**--bench-motion** (1920×1080)\n")
print("| run | swap-ins | idle_30s_prompt presents / app frames / layer paints / cpu ms/s | pending, demoted | mem_idle anon / USS | after 5×5 anon / USS | mem_end anon / USS |")
print("|---|---|---|---|---|---|---|")
for i in (1, 2, 3):
    for who in ("main", "q5"):
        n = f"{who}-motion-{i}"
        try: d = json.load(open(n + ".json"))
        except Exception: print(f"| {n} | | (no result) | | | | |"); continue
        i30 = d['idle_30s_prompt']; e = d['mem_end']['excluding_gpu_driver']; idle = d['mem_idle']['excluding_gpu_driver']
        l5 = d['mem_after_load_5x5']['excluding_gpu_driver']
        yn = lambda b: 'yes' if b else 'no'
        print(f"| {n} | {log.get(n, ['?']*7)[6]} | {i30['frames_presented']} / {i30['app_frames']} / {i30['layer_paints']} / {i30['cpu_ms_per_s']} | {yn(i30['prompt_pending'])}, {yn(i30['demoted_worker'])} | {idle['anon_kb']:,} / {idle['uss_kb']:,} | {l5['anon_kb']:,} / {l5['uss_kb']:,} | {e['anon_kb']:,} / {e['uss_kb']:,} |")
def mean_mem(who, key, f, ns):
    xs = [json.load(open(f"{who}-motion-{i}.json"))[key]['excluding_gpu_driver'][f] for i in ns]
    return st.mean(xs)
for ns in ((1, 2, 3), (2, 3)):
    print(f"\nruns {ns}:")
    for key in ("mem_after_load_5x5", "mem_end"):
        for f in ("anon_kb", "uss_kb"):
            a, b = mean_mem("main", key, f, ns), mean_mem("q5", key, f, ns)
            print(f"- {key} {f}: main {a:,.0f}, Q5 {b:,.0f}: {100 * (b - a) / a:+.2f} %")
