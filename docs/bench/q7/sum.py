import json,glob,os,re
os.chdir(os.path.expanduser('~/.cache/q7/bench'))
log={l.split()[2]:l.split() for l in open('log') if len(l.split())>5}
for f in sorted(glob.glob('*.json'), key=lambda f:(f.split('-')[-1], f)):
    n=f[:-5]
    try: d=json.load(open(f))
    except Exception: print(n,'(no result)'); continue
    sw=log.get(n,['?']*7)[6]; ld=log.get(n,['?']*7)[4]
    if 'motion' in n:
        i=d['idle_30s_prompt']
        print(n, 'swapins',sw,'load',ld,'| idle_30s_prompt presents',i['frames_presented'],'app_frames',i['app_frames'],'layer_paints',i['layer_paints'],'cpu_ms/s',i['cpu_ms_per_s'],'prompt',i['prompt_pending'],'demoted',i['demoted_worker'])
    else:
        m=d['mem_end']['excluding_gpu_driver']
        print(n, 'swapins',sw,'load',ld,'| mem_end anon',m['anon_kb'],'uss',m['uss_kb'])
