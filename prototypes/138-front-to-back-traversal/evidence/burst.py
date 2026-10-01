# Separates the per-segment GPU burst (a run of frames whose GPU time exceeds 1.8x the segment
# median) from steady frames, to isolate traversal cost from the unattributed burst.
import json,sys,glob,os
for path in sorted(glob.glob(os.path.join(os.path.dirname(__file__),'brickmap-whole-world-*.jsonl'))  # unzip frames.zip first):
    rows=[json.loads(l) for l in open(path)]
    by={}
    for r in rows:
        if r['kind']=='frame': by.setdefault(r['segment'],[]).append(r)
    steady=[];burst=0;allm=[]
    for s,fr in by.items():
        g=sorted(r['gpu_frame_ms'] for r in fr); med=g[len(g)//2]
        for r in fr:
            v=max(r['cpu_frame_ms'],r['gpu_frame_ms']); allm.append(v)
            if r['gpu_frame_ms']>1.8*med: burst+=1
            else: steady.append(v)
    def p(v,q): v=sorted(v); return v[min(len(v)-1,int(len(v)*q))]
    print(os.path.basename(path),'frames',len(allm),'burst',burst,'(%.1f%%)'%(100*burst/len(allm)),
      '| all p99 %.2f | steady p50 %.2f p99 %.2f p99.9 %.2f max %.2f'%(p(allm,.99),p(steady,.5),p(steady,.99),p(steady,.999),max(steady)))
