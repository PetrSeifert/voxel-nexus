# PROTOTYPE (issue #137): results, 2026-10-01, RTX 3060 Laptop

Context: NVIDIA GeForce RTX 3060 Laptop GPU, driver 595.79, on mains power, power scheme Balanced. The "Best performance" overlay was not confirmed. Validation was off, presentation unthrottled (IMMEDIATE), 1920x1080, fog and edge shading on (`PresentationStyle::SCENIC`). Evidence is in `evidence/`. Rerun with `cargo build --release --features qualification -p desktop-demo --bin whole-world-detail-profile --bin whole-world-detail-frames`, then run the binaries as listed below.

## Item 3: worst per-volume cost per detail level

`whole-world-detail-profile` covered the generated, edited and restored fixture volume. Every fixture volume has the same local contents, so the one edited volume is the worst case. Raster Regions are 16³ voxels, clipped to the level. Vulkan sizes are actual `allocationSize`, summed over unique buffers.

| Level | Summary retained | Raster CPU live / peak | Raster Vulkan (buffers) | Brickmap CPU live / peak | Brickmap Vulkan |
| --- | ---: | ---: | ---: | ---: | ---: |
| 64³ | n/a (materialization, S = 413,448 from #118) | 1,021,376 / 1,176,192 | 18,160 (120) | 92,568 / 384,968 | 88,160 |
| 32³ | 136,752 | 261,544 / 495,304 | 9,936 (24) | 35,344 / 127,808 | 33,120 |
| 16³ | 21,904 | 77,968 / 320,504 | 7,312 (3) | 11,120 / 38,400 | 10,368 |
| 8³ | 7,568 | 23,984 / 77,080 | 4,288 (3) | 2,528 / 9,776 | 2,144 |

The 64³ values reproduce #118's `G_raster` and `G_brickmap` exactly.

The prototype's downsampling peaks at 614,760 bytes and takes 49 ms per volume, because it reads full detail through `read_region`. Its summary is the coarse level published as a `SparsePages` volume. The direct-vote tie uses lexicographic material identity, because `VoxelMaterialId` defines no order yet.

**Finding (skirts):** the raster mesher already treats coordinates outside a volume as empty and emits every boundary face. Every volume mesh is therefore closed, and the costs above already include skirt-equivalent boundary walls. The crack question for spec becomes T-junction sparkle at mixed-level faces, not open gaps.

## Item 4: cap formulas (incomplete)

Counts are fixed independently of observed peaks. Per level L, a single selection holds at most `M_L` volumes: 49, 120, 156 and 87 for 64³, 32³, 16³ and 8³ (selected at 8³). Old and new selections together give `2·M_L`. As in #118, there are 3 Render Path owners.

- Raster GPU = `6·(49·G64 + 120·G32 + 156·G16 + 87·G8)` = 6 × 3,595,888 = **21,575,328** (was 980,640 for the 3x3 qualification)
- Brickmap GPU = `6·(49·B64 + 120·B32 + 156·B16 + 87·B8)` + measured header overhead (not yet measured) ≥ 60,589,056
- Summaries = `256·S8 + 2·120·S32 + 2·156·S16` + overhead (not yet measured) ≥ 41,535,680

Still missing: configured-ownership costs for the coarse levels (the prototype measured derivation only), 256-volume container overhead, and peaks. The Brickmap caps will be re-derived anyway if its traversal structure changes (see the verdict).

## Item 5: whole-world frame time

`whole-world-detail-frames <raster|brickmap> <whole-world|full-detail-only> OUT.jsonl` flies one lap of the frozen route. For each centre segment it publishes all 256 volumes at their selected levels as one static scene, and rebuilds off the route clock. Steady frames only. Crossing and handoff installation are not exercised.

| Render Path | Coverage | Frames | p99 `max(cpu,gpu)` ms | Max ms | GPU p50 / p99 ms |
| --- | --- | ---: | ---: | ---: | ---: |
| Raster | whole world | 124,275 | **2.68** | 7.83 | 0.33 / 0.40 |
| Raster | 7x7 full detail only | 158,213 | 2.08 | 12.10 | 0.14 / 0.18 |
| Brickmap | whole world | 9,332 | **81.52** | 85.90 | 18.76 / 80.27 |
| Brickmap | 7x7 full detail only | 29,324 | 8.24 | 9.56 | 5.56 / 6.94 |

CPU time includes the preceding-frame fence wait, so it always exceeds GPU time. For Brickmap the GPU is the dominant cost.

**Dominant cost (Brickmap):** `trace_scene` in `dense_dda.comp` traces every volume for every ray and keeps the nearest hit. Cost scales with the number of volumes: 49 → 256 raises GPU p50 from 5.6 to 18.8 ms. Clipping each later volume to the nearest hit so far changed nothing (p50 18.6 ms), because volumes are visited in storage order rather than front to back. Even without the tail, steady Brickmap frames have a p99 of 23.2 ms.

**Unexplained tail:** 296 of 9,332 Brickmap frames reach 77–86 ms. They come in bursts at frames 59–74 of 29 of 48 segments, about 1.1–1.4 s after each rebuild. This may be a prototype artifact of the per-segment backend rebuild, and it was not attributed. It does not change the verdict.

## Item 6: latency (descriptive)

A full off-clock rebuild of the whole 256-volume scene for one Render Path takes p50 568 / max 697 ms for Brickmap and p50 979 / max 1,463 ms for Raster. Both are under 2.5 s even without incremental installation. The startup sweep is roughly 256 × (generation + 49 ms prototype downsampling); generation was not measured.

## Verdict

- **Raster: PASS.** Whole-world p99 is 2.68 ms against the 10 ms limit.
- **Brickmap: FAIL.** Whole-world p99 is 81.5 ms, and 23.2 ms without the unexplained tail. The responsible phase is GPU compute traversal: a per-ray linear scan over every volume, with no front-to-back scene-level structure. Shaping reopens. Budgets, gate and proof are unchanged.
