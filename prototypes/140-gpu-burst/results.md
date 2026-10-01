# PROTOTYPE (issue #140): the 8-frame GPU burst after Render Backend initialization, 2026-10-01/02, RTX 3060 Laptop

Throwaway. Branch `prototype/140-gpu-burst`, built on `prototype/138-front-to-back-traversal` (grid DDA build).

Context: mains power, Balanced scheme, validation off, IMMEDIATE unless noted, 1920x1080, frozen #137 route, demo bands, whole world. Every segment recorded `NVIDIA GeForce RTX 3060 Laptop GPU`: the probe executable was forced to the dGPU through `UserGpuPreferences`.

## Probes added to `whole-world-detail-frames`

- `PROBE_HOLD_CENTRE`: one backend for the whole lap; the selection centre never moves, so the scene never changes.
- `PROBE_SWITCH_IN_PLACE`: one backend for the whole lap; each segment's scene is installed through `request_render_path_switch` (Brickmap to Brickmap, a constant scene identity) instead of a new backend.
- `PROBE_REFRESH_IN_PLACE`: one backend and one scene; `refresh_render_path` releases and reconfigures every path resource at each would-be segment boundary.
- `PROBE_FIFO`: FIFO instead of IMMEDIATE. `PROTOTYPE_VISIBLE`: visible window.
- Every frame now records `since_init_s` (time since the presenting backend was initialized) and `unix_s`.

`evidence/bursts.py` lists each burst (frames above 1.8x the segment median GPU time) with its `since_init_s`. `evidence/gate.py` gives p99 of `max(cpu_frame_ms, gpu_frame_ms)`. Raw evidence is in `evidence/frames.zip`.

## Results: full laps

| Backend lifetime | Frames | Burst frames | p99 | p99.9 | Max |
| --- | ---: | ---: | ---: | ---: | ---: |
| New backend per segment (baseline lap 1) | 35,914 | 243 (0.68%) | 7.33 | 24.90 | 26.35 |
| New backend per segment (baseline lap 2) | 31,429 | 439 (1.40%) | **14.85** | 27.41 | 30.22 |
| One backend, centre held | 35,558 | 11 (0.03%) | 6.58 | 7.11 | 22.39 |
| **One backend, scene switched in place per segment** | 34,274 | 17 (0.05%) | **6.67** | 7.44 | 25.40 |
| One backend, path resources refreshed per segment | 34,303 | 11 (0.03%) | 6.99 | 7.64 | 23.31 |

With a new backend per segment, 41 of 48 segments in lap 2 burst, almost always at 2.5–2.6 s after initialization (sometimes 1–3 s later). With one long-lived backend, the 10–12-frame burst happens **once**, 6–11 s after initialization. It does not recur at path switches, path refreshes or scene changes.

## Results: 12-segment A/B (new backend per segment)

| Variant | Burst frames | Burst onset |
| --- | ---: | --- |
| Baseline, hidden, IMMEDIATE | 80 (1.01%) | 2.48–2.58 s |
| FIFO | 75 (1.03%) | 2.58–2.62 s |
| Visible window | 115 (1.59%) | 2.55–2.66 s |
| Visible window, FIFO | 84 (1.17%) | 2.59–2.62 s |
| Brickmap, full detail only | 130 (2.16%) | 2.64–2.70 s |
| **Raster, whole world** | **0** | none |

## Ruled out

- **Presentation mode and window visibility:** unchanged across IMMEDIATE/FIFO and hidden/visible.
- **Device creation elsewhere on the system:** `vulkaninfo --summary` every ~4.4 s beside a long-lived backend caused single-frame blips at that period, never the 8-frame burst (`hold-centre-external-init.jsonl`, `external-init-times.txt`).
- **Path resource re-creation:** releasing and reconfiguring the Brickmap path 47 times in one backend produced no burst after the first.
- **Clocks and P-state:** nvidia-smi sampled every 20 ms (it updates every ~0.5 s) shows P0, 7001 MHz memory and 1.7–1.8 GHz graphics throughout, with only a software thermal slowdown (reason `0x20`, ~5%) that cannot make 4x (`smi-12seg.csv`).
- **Implicit Vulkan layers:** the EOS and Steam overlay layers are installed but need enable variables that are not set.

## Attribution

The burst is a one-time cost that follows each Render Backend initialization in this process, about 2.5 s later (6–11 s for the first). It appears only under the Brickmap compute workload. A long-lived backend pays it once at startup and never again for path switches, refreshes or scene changes. The #137/#138 harness creates a new backend for every route segment, so it paid that startup cost about 48 times per lap. **It is a harness artifact.**

Not attributed: what inside the driver/OS runs at that moment. Getting that needs an elevated WPR GPU trace (GPUView), which this session could not take. The decision does not depend on it.

## Verdict (proposed)

- Measure whole-world frame time with one long-lived backend per lap, installing each segment's scene in place, which is what the production Render Backend does. With that harness the DDA lap's p99 is 6.67 ms (≤ 10 ms).
- The single startup burst (10–12 frames, ~0.03%) stays inside the route and is recorded in the maximum frame time, which is not gated.
