"""PROTOTYPE (issue #139): byte caps from fixed counts times worst measured per-volume cost
(ownership.jsonl), following the #117/#118 structure: 3 Render Path owners x old/new = 6
assemblies, one raster worker deriving one volume at a time, complete Brickmap construction
scratch over every selected volume."""
import json, os

here = os.path.dirname(os.path.abspath(__file__))
rows = [json.loads(line) for line in open(os.path.join(here, "ownership.jsonl"))]
levels = [row for row in rows if row["kind"] == "level"]
whole = [row for row in rows if row["kind"] == "whole-world"]
EDGES = [64, 32, 16, 8]
COUNTS = {"demo": [49, 120, 156, 87], "qualification": [9, 16, 56, 231]}
ASSEMBLIES = 6
GRID_TABLE = (6 + 256) * 4

def worst(path, key, edge):
    return max(row[key] for row in levels if row["path"] == path and row["edge"] == edge)

R = [worst("Raster", "cpu_configured_live_bytes", e) for e in EDGES]
R_PEAK = worst("Raster", "cpu_peak_bytes", 64)
B = [worst("Brickmap", "cpu_configured_live_bytes", e) for e in EDGES]
B_PEAK = [worst("Brickmap", "cpu_peak_bytes", e) for e in EDGES]
G = [worst("Raster", "gpu_live_bytes", e) for e in EDGES]
BG = [worst("Brickmap", "gpu_live_bytes", e) for e in EDGES]
S = {e: max(row["summary_retained_bytes"] for row in levels if row["edge"] == e) for e in EDGES[1:]}

def generated(path, key, edge):
    return next(row[key] for row in levels
                if row["path"] == path and row["edge"] == edge and row["state"] == "generated")

def overhead(path, key):
    """Largest measured whole-world assembly minus its generated per-volume sum, floored at 0."""
    return max(0, max(row[key] - sum(n * generated(path, key, e) for n, e in zip(row["counts"], EDGES))
                      for row in whole if row["path"] == path))

H_R = overhead("Raster", "cpu_configured_live_bytes")
H_G = overhead("Raster", "gpu_live_bytes")
H_B = max(GRID_TABLE, overhead("Brickmap", "cpu_configured_live_bytes"))
H_BG = max(GRID_TABLE, overhead("Brickmap", "gpu_live_bytes"))
dot = lambda counts, costs: sum(n * c for n, c in zip(counts, costs))
print("per-level worst: R", R, "R_peak", R_PEAK, "B", B, "B_peak", B_PEAK, "G", G, "BG", BG, "S", S)
print("overheads: H_R", H_R, "H_G", H_G, "H_B", H_B, "H_BG", H_BG)
for name, M in COUNTS.items():
    caps = {
        "summaries": 256 * S[8] + 2 * M[1] * S[32] + 2 * M[2] * S[16],
        "raster_cpu": ASSEMBLIES * (dot(M, R) + H_R) + R_PEAK,
        "brickmap_cpu": ASSEMBLIES * (dot(M, B) + H_B) + dot(M, B_PEAK),
        "raster_gpu": ASSEMBLIES * (dot(M, G) + H_G),
        "brickmap_gpu": ASSEMBLIES * (dot(M, BG) + H_BG),
    }
    bands = [row for row in whole if row["bands"] == name]
    observed = {
        "summaries": max(row["summary_resident_retained_bytes"] + row["summary_streamed_retained_bytes"] for row in bands),
        "raster_cpu": max(row["cpu_peak_bytes"] for row in bands if row["path"] == "Raster"),
        "brickmap_cpu": max(row["cpu_peak_bytes"] for row in bands if row["path"] == "Brickmap"),
        "raster_gpu": max(row["gpu_live_bytes"] for row in bands if row["path"] == "Raster"),
        "brickmap_gpu": max(row["gpu_live_bytes"] for row in bands if row["path"] == "Brickmap"),
    }
    print(f"== {name} M={M}")
    for key, cap in caps.items():
        print(f"   {key:13} cap {cap:>12,}  one-assembly observed {observed[key]:>12,}")

# Alternative: the worst achievable selection over every one of the 256 centres, instead of
# per-level maxima that can never occur together (their counts sum to more than 256 volumes).
def edge(bands, centre, x, z):
    distance = max(abs(x - centre[0]), abs(z - centre[1]))
    for reach, level in zip(bands, EDGES):
        if distance <= reach:
            return level
    return 8

BANDS = {"demo": [3, 6, 12], "qualification": [1, 2, 4]}
for name, bands in BANDS.items():
    selections = []
    for cx in range(16):
        for cz in range(16):
            counts = [0, 0, 0, 0]
            for x in range(16):
                for z in range(16):
                    counts[EDGES.index(edge(bands, (cx, cz), x, z))] += 1
            selections.append(counts)
    worst_sel = lambda costs: max(dot(c, costs) for c in selections)
    alt = {
        "summaries": 256 * S[8] + 2 * max(c[1] * S[32] + c[2] * S[16] for c in selections),
        "raster_cpu": ASSEMBLIES * (worst_sel(R) + H_R) + R_PEAK,
        "brickmap_cpu": ASSEMBLIES * (worst_sel(B) + H_B) + worst_sel(B_PEAK),
        "raster_gpu": ASSEMBLIES * (worst_sel(G) + H_G),
        "brickmap_gpu": ASSEMBLIES * (worst_sel(BG) + H_BG),
    }
    print(f"== {name} worst achievable selection")
    for key, cap in alt.items():
        print(f"   {key:13} cap {cap:>12,}")
    print("   argmax raster selection", max(selections, key=lambda c: dot(c, R)))
