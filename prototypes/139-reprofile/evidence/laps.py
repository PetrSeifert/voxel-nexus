"""PROTOTYPE (issue #139): gate p99 of max(cpu, gpu) per presenting Render Path, whole route and
2.5 s crossing/switch windows, for every lap-*.jsonl beside this script.
Unzip laps.zip here first."""
import glob, json, math, os

WINDOW = 2.5

def p99(values):
    if not values:
        return None
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, math.ceil(0.99 * len(ordered)) - 1)]

def inside(seconds, starts):
    return any(start <= seconds <= start + WINDOW for start in starts)

here = os.path.dirname(os.path.abspath(__file__))
for path in sorted(glob.glob(os.path.join(here, "lap-[rb]*.jsonl"))):
    rows = [json.loads(line) for line in open(path)]
    frames = [row for row in rows if row["kind"] == "frame"]
    crossings = [row["route_seconds"] for row in rows if row["kind"] == "crossing"]
    switches = [row["route_seconds"] for row in rows if row["kind"] == "switch-request"]
    lap = next(row for row in rows if row["kind"] == "lap")
    end = next((row for row in rows if row["kind"] == "end"), {})
    devices = {row.get("device") for row in rows if row.get("device")}
    installs = [row for row in rows if row["kind"] == "installed"]
    install_s = [row["route_seconds"] - row["requested_s"] for row in installs]
    preparation = [row["preparation_ms"] for row in rows if row["kind"] == "crossing"]
    print(f"== {os.path.basename(path)} start={lap['start']} devices={devices}")
    print(f"   segments={end.get('segments')} stalls={end.get('coverage_stalls')} "
          f"switches={len(switches)} installs={len(installs)} "
          f"install p50/max s={sorted(install_s)[len(install_s)//2]:.3f}/{max(install_s):.3f} "
          f"preparation max ms={max(preparation):.0f}")
    for presenting in sorted({frame["presenting"] for frame in frames}):
        own = [frame for frame in frames if frame["presenting"] == presenting]
        gate = lambda selected: [max(f["cpu_frame_ms"], f["gpu_frame_ms"]) for f in selected]
        whole = gate(own)
        crossing = gate([f for f in own if inside(f["route_seconds"], crossings)])
        switch = gate([f for f in own if inside(f["route_seconds"], switches)])
        fmt = lambda value: "n/a" if value is None else f"{value:.2f}"
        print(f"   {presenting}: frames={len(own)} p99 whole={fmt(p99(whole))} "
              f"crossing={fmt(p99(crossing))} ({len(crossing)}) switch={fmt(p99(switch))} "
              f"({len(switch)}) max={max(whole):.2f} gpu p50="
              f"{sorted(f['gpu_frame_ms'] for f in own)[len(own)//2]:.2f}")
