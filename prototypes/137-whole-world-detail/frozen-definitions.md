# PROTOTYPE (issue #137): frozen definitions draft

## 1. Voxel Detail Selection

- **Centre:** per horizontal axis, a Schmitt trigger on the camera's volume index. Let `nominal = clamp(floor(c / 64), 0, 15)`. Keep the previous centre `a` while `a*64 - 16 <= c < (a+1)*64 + 16`. Otherwise set it to `nominal`. Height is ignored because the world is one volume tall.
- **Resets:** on initialization, and when an accepted camera state moves more than 64 scene units from the previous one (a teleport), the centre is `nominal` on both axes.
- **Distance rule:** Chebyshev distance in volume indices from the centre, `d = max(|x - ax|, |z - az|)`.
- **Bands (reach is the outer `d` of each level, inclusive):**

  | Configuration | 64³ | 32³ | 16³ | 8³ |
  | --- | --- | --- | --- | --- |
  | Demo | d ≤ 3 | d ≤ 6 | d ≤ 12 | rest |
  | Qualification | d ≤ 1 | d ≤ 2 | d ≤ 4 | rest |

  Each reach doubles as the voxel size doubles, so the largest projected voxel size at every transition is the same.
- **Hysteresis:** the 32-unit deadband sits on the centre, so every band shifts together and keeps the same shape. Unlike per-volume deadbands, this never grows a band.
- **Guaranteed full-detail reach** from the eye: `reach*64 - 16` units (176 demo, 48 qualification).

### Proof of maxima

Every selection is a set of clipped square annuli around one centre volume, whatever path led there. So the maximum count at each level is the largest clipped annulus over all 256 centres. Reversals, diagonals and teleports can change only which centre is current. They cannot change a selection's shape. Moving a centre back to the index it just left requires 32 units of travel on that axis, so wobbling cannot churn the selection.

| Configuration | 64³ | 32³ | 16³ | 8³ | N (= 64³ max) | Copies `2N+1` |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Demo | 49 | 120 | 156 | 87 | 49 | 99 |
| Qualification | 9 | 16 | 56 | 231 | 9 | 19 (unchanged from #118) |

8³ is resident for all 256 volumes regardless of band. The 8³ column counts volumes whose *selected* level is 8³.

Evidence: `check_selection.cjs` ran 2,000 random walks of 2,000 steps per configuration, with boundary wobbles, diagonals and 1% teleports (about 800k centre moves). It found zero violations, observed exactly the analytic maxima, and measured a minimum reversal travel of 33.2 units. `selection.html` is the clickable model.

## 2. Measurement definitions (approved 2026-10-01)

- **Scene bounds:** `[0,1024] x [0,64] x [0,1024]`.
- **Far plane:** `D = ` the largest distance from the eye to the 8 corners of the scene bounds. `far = D + 1`, near 0.1. Content that cannot be seen is never cut off, from anywhere and in any orientation.
- **Fog:** the existing `smoothstep(start, end, distance)` toward `DISTANCE_FOG_LINEAR_COLOR`, with `start = 0.5 D` and `end = 1.6 D`. At distance `D` the fog factor is 0.432, so the farthest content keeps 56.8% ≥ 50% of its linear difference from the fog colour.
- **Route** (demo configuration, eye height 48, 16 units/s). The camera aims at the world centre `[512, 24, 512]`, which keeps the most terrain in view: the worst case for whole-world cost. Within 64 horizontal units of the centre, it aims 64 units ahead along its travel and 16 down:
  `[160,160] → [864,160] → [864,500] → [864,420] (reversal) → [864,864] → teleport to [160,864] → [864,160] (diagonal) → [160,160]`.
  That is 3,267.6 units, about 204.2 s per lap. Two laps for each starting Render Path.
- **Switches:** the first centre move on the diagonal requests the switch to the other Render Path. The first centre move on the final `[864,160] → [160,160]` segment requests the switch back. Every lap exercises both directions.
- **Warmup:** the startup sweep runs, then both Render Paths' initial installation at the start position, then a 5 s hold at `[160,160]`. Frames before the route clock starts are excluded.
- **Crossing window:** frames whose CPU start falls in `[t, t + 2.5 s]`, where `t` is the accepted camera state that moved the centre (including the teleport). **Switch window:** `[t_request, t_request + 2.5 s]`.
- **GPU scope:** `TOP_OF_PIPE → BOTTOM_OF_PIPE` timestamps around the frame's single command buffer. That buffer holds all of the frame's uploads, rendering, compute and composite. Any extra submission a Render Path makes for a frame is bracketed the same way and added to that frame.
- **CPU scope:** from the start of the frame thread's redraw handling, before the camera route update, selection, admission and scenario hooks, until `vkQueuePresentKHR` returns. This covers the preceding-frame fence wait, frame-boundary installation, acquire, recording and submission. Worker threads are excluded, except for any time the frame thread blocks on them.
- **Gate value per frame:** `max(cpu_frame_ms, gpu_frame_ms)`, paired by submitted frame sequence. Each frame is attributed to the Render Path presenting it. p99 is computed over the whole route, the union of crossing windows and the union of switch windows, separately for each Render Path.
