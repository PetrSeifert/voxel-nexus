import json,sys,glob,os
for path in sorted(glob.glob(os.path.join(os.path.dirname(__file__),'*.jsonl'))  # unzip frames.zip first):
    rows=[json.loads(l) for l in open(path)]
    seg=[r for r in rows if r['kind']=='segment']; fr=[r for r in rows if r['kind']=='frame']
    def p(v,q): v=sorted(v); return v[min(len(v)-1,int(len(v)*q))]
    m=[max(r['cpu_frame_ms'],r['gpu_frame_ms']) for r in fr]
    cpu=[r['cpu_frame_ms'] for r in fr]; gpu=[r['gpu_frame_ms'] for r in fr]
    tail=sum(1 for v in m if v>40)
    print(os.path.basename(path), 'segments',len(seg),'frames',len(fr),
      '| p99 %.2f max %.2f | cpu p50 %.2f p99 %.2f | gpu p50 %.2f p99 %.2f | frames>40ms %d'%(
      p(m,.99),max(m),p(cpu,.5),p(cpu,.99),p(gpu,.5),p(gpu,.99),tail))
