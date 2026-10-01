# p99 of each frame's max(cpu_frame_ms, gpu_frame_ms) per evidence file, the frozen frame-time statistic.
import json, sys
for path in sys.argv[1:]:
    values = sorted(max(r['cpu_frame_ms'], r['gpu_frame_ms']) for r in map(json.loads, open(path)) if r['kind'] == 'frame')
    pick = lambda q: values[min(len(values) - 1, int(len(values) * q))]
    print(f'{path:40} frames {len(values):6} p50 {pick(.5):6.2f} p99 {pick(.99):6.2f} p99.9 {pick(.999):6.2f} max {values[-1]:6.2f}')
