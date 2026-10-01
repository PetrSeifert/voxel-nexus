# PROTOTYPE (issue #138): front-to-back scene traversal, 2026-10-01, RTX 3060 Laptop

Throwaway. Branch `prototype/138-front-to-back-traversal`, built on `prototype/137-whole-world-detail`.

## What changed

`trace_scene` in `dense_dda.comp` walks the fixture's 16x16 volume grid front to back with a 2D DDA (cells of 64 scene units, one layer). It traces only the volumes the ray crosses, in order, and returns at the first hit. Volumes are disjoint half-open boxes and cells are visited in increasing ray distance, so the first hit is the nearest. The prototype hardcodes the fixture layout (identity order `volume-XX-ZZ`, so index = x·16 + z). It checks each visited header against that layout and falls back to the old linear scan on a mismatch; the `NO_FALLBACK` build proves the guard never fires on this fixture. Correctness was argued, not tested: no Semantic Ray comparison was run.

Build variants via `PROTOTYPE_TRAVERSAL=<LINEAR|NO_FALLBACK>` (shader macro), and limit runs with `PROTOTYPE_SEGMENTS=<n>`.

## Environment gotcha

New executable paths ran on the **Intel Iris Xe**: the backend takes the first qualifying Vulkan device, and Windows orders devices by per-app GPU preference. The first DDA lap recorded `"device": "Intel(R) Iris(R) Xe Graphics"` and was discarded (DDA 26 ms vs LINEAR 83 ms GPU p50 there). The binaries in `target/release/variants/` were then forced to the RTX 3060 with `HKCU\Software\Microsoft\DirectX\UserGpuPreferences` = `GpuPreference=2;`. Every row below records `NVIDIA GeForce RTX 3060 Laptop GPU`. Qualification runs should check the recorded `device` field.

Context: mains power, Balanced scheme, validation off, IMMEDIATE, 1920x1080, `PresentationStyle::SCENIC`, frozen #137 route, demo bands, whole world.

## Results (full lap, `evidence/*.jsonl`, `evidence/burst.py`)

| Traversal | Frames | p99 all | Steady p50 | Steady p99 | Burst frames |
| --- | ---: | ---: | ---: | ---: | ---: |
| Linear scan (same-session control) | 10,237 | 24.22 | 19.54 | 24.22 | 0 |
| **Front-to-back grid DDA** | 30,272 | **15.17** | **6.29** | **8.54** | 485 (1.6%) |

The linear control reproduces #137's steady GPU p50 (19.0 vs 18.8 ms).

## The burst (unattributed, independent of traversal)

In most segments, about 2.2–2.5 s after the segment's backend is initialized, GPU time rises about 4x (5.5 → 23 ms, sometimes 2x → 12.5 ms) for exactly 8 frames, then returns to normal. #137's 80 ms linear bursts are the same 4x of 19 ms. It is not traversal cost, since it scales with whatever the frame costs. It is not backend teardown either: a 5 s off-clock settle pause still showed it, including in segment 0 (`evidence/brickmap-12-segments-dda-settle-5s.jsonl`). Graphics clocks stay at about 1.8 GHz with no P-state change during it. No timer in the engine matches it. Its frequency varies between runs: none in this run's linear lap, 3% in #137.

## Verdict (proposed)

- Grid DDA: steady p99 8.54 ms ≤ 10 ms. The scan over all 256 volumes was the steady cost; a front-to-back walk over the existing volume grid removes it without a new traversal structure.
- The gate is still failed by the burst: 1.6% of frames is above the 1% the p99 tolerates. It needs attribution before qualification.
