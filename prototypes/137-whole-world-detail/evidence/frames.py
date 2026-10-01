import json,sys,glob
for path in sorted(glob.glob('artifacts/prototype-137/frames-*.jsonl')):
    rows=[json.loads(l) for l in open(path)]
    seg=[r for r in rows if r['kind']=='segment']; fr=[r for r in rows if r['kind']=='frame']
    def p(v,q): v=sorted(v); return v[min(len(v)-1,int(len(v)*q))]
    m=[max(r['cpu_frame_ms'],r['gpu_frame_ms']) for r in fr]
    cpu=[r['cpu_frame_ms'] for r in fr]; gpu=[r['gpu_frame_ms'] for r in fr]
    dom=sum(1 for r in fr if r['gpu_frame_ms']>=r['cpu_frame_ms'])/len(fr)
    prep=[s['preparation_ms'] for s in seg]
    print(path.split('frames-')[1][:-6], 'segments',len(seg),'frames',len(fr),'route_s %.1f'%fr[-1]['route_seconds'],
      '| p99 %.2f max %.2f | cpu p50 %.2f p99 %.2f | gpu p50 %.2f p99 %.2f | gpu>=cpu %.0f%% | rebuild p50 %.0f max %.0f ms'%(
      p(m,.99),max(m),p(cpu,.5),p(cpu,.99),p(gpu,.5),p(gpu,.99),dom*100,p(prep,.5),max(prep)))
