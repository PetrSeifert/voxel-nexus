# Lists every burst (a run of frames whose GPU time exceeds 1.8x the segment median) with its
# time since the presenting backend was initialized, per evidence file given on the command line.
import json, sys
for path in sys.argv[1:]:
    rows = [json.loads(line) for line in open(path)]
    devices = {r['device'] for r in rows if r['kind'] == 'segment'}
    segments = {}
    for r in rows:
        if r['kind'] == 'frame':
            segments.setdefault(r['segment'], []).append(r)
    total = sum(len(f) for f in segments.values()); burst_total = 0
    print(f'== {path} devices={devices} frames={total}')
    for segment, frames in sorted(segments.items()):
        gpu = sorted(f['gpu_frame_ms'] for f in frames); median = gpu[len(gpu) // 2]
        runs = []; current = None
        for f in frames:
            if f['gpu_frame_ms'] > 1.8 * median:
                if current is None: current = [f['since_init_s'], 0, 0.0]; runs.append(current)
                current[1] += 1; current[2] = max(current[2], f['gpu_frame_ms'])
            else:
                current = None
        burst_total += sum(r[1] for r in runs)
        span = (frames[0]['since_init_s'], frames[-1]['since_init_s'])
        print(f'  seg {segment:3} span {span[0]:6.2f}-{span[1]:6.2f}s median {median:5.2f} bursts ' +
              ', '.join(f'@{r[0]:.2f}s x{r[1]} max {r[2]:.1f}' for r in runs))
    print(f'  burst frames {burst_total} ({100 * burst_total / max(total, 1):.2f}%)')
