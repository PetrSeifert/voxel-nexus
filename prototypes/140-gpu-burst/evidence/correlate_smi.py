# Prints nvidia-smi samples around each burst and around each segment start, aligned by wall clock.
import json, sys, datetime
frames_path, smi_path = sys.argv[1], sys.argv[2]
smi = []
for line in open(smi_path):
    parts = [p.strip() for p in line.split(',')]
    if len(parts) < 10: continue
    t = datetime.datetime.strptime(parts[0], '%Y/%m/%d %H:%M:%S.%f').timestamp()
    smi.append((t, parts[1:]))
rows = [json.loads(l) for l in open(frames_path)]
segments = {}
for r in rows:
    if r['kind'] == 'frame': segments.setdefault(r['segment'], []).append(r)
for segment, frames in sorted(segments.items()):
    gpu = sorted(f['gpu_frame_ms'] for f in frames); median = gpu[len(gpu) // 2]
    burst = [f for f in frames if f['gpu_frame_ms'] > 1.8 * median]
    init_unix = frames[0]['unix_s'] - frames[0]['since_init_s']
    print(f'-- seg {segment} init at +0; burst since_init: ' + (f"{burst[0]['since_init_s']:.2f}-{burst[-1]['since_init_s']:.2f}" if burst else 'none'))
    for t, values in smi:
        offset = t - init_unix
        if -0.3 <= offset <= 3.5:
            mark = ''
            if burst and burst[0]['unix_s'] - 0.05 <= t <= burst[-1]['unix_s'] + 0.05: mark = '  <== BURST'
            print(f'   {offset:6.2f}s ' + ' '.join(values) + mark)
    if segment >= int(sys.argv[3]) if len(sys.argv) > 3 else False: break
