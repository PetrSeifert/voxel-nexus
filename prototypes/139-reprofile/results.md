# PROTOTYPE (issue #139): whole-world re-profile and detail-level byte caps, 2026-10-02, RTX 3060 Laptop

Throwaway. Branch `prototype/139-reprofile`, built on `prototype/140-gpu-burst` (grid DDA build).

Context: NVIDIA GeForce RTX 3060 Laptop GPU, driver 595.79. Mains power, and Windows "Best performance" was confirmed as the active AC overlay (`ded574b5-…`) before every lap (`evidence/lap-context.jsonl`). Validation was off, presentation unthrottled (IMMEDIATE), 1920x1080, fog and edge shading on. The frozen #137 route and demo bands were used. Every record in every run names the RTX 3060 Laptop: each executable was forced to the dGPU through `UserGpuPreferences`, and the entry was removed afterwards.

Rerun: `cargo build --release --features qualification -p desktop-demo --bin whole-world-laps --bin whole-world-ownership`, then `whole-world-laps <raster|brickmap|semantic> OUT.jsonl` and `whole-world-ownership OUT.jsonl`. Analysis is in `evidence/laps.py` and `evidence/caps.py`; their outputs are `laps.txt` and `caps.txt`.

## Grid-to-volume table in the scene words

`compute_scene.rs` writes a grid block between the volume headers and the materials when every volume occupies one cell of a single-layer regular grid. The block holds the origin, the cell size, the x and z cell counts, and then one volume index per cell (`u32::MAX` for an empty cell). The shader detects the block from the gap before the material offset and walks it with the #138 front-to-back 2D DDA. Scenes without the block, which includes every existing test scene that is not such a grid, keep the linear scan. For 256 volumes the table is (6 + 256) × 4 = **1,048 bytes** per assembly.

**Correctness** (`whole-world-laps semantic`): every one of the 48 route scenes was installed in place in one backend, and a 6x6 camera-ray lattice was compared against `observe_along_ray` (tolerance 0.002). **1,728 of 1,728 rays agree; 897 are contacts.** The existing `installed_gpu_dda_matches_semantic_oracle` GPU test and the compute crate suites also pass.

## Configured ownership per detail level (bytes)

`whole-world-ownership` builds each Render Path under its own allocation category and GPU owner, configures it in a fresh Render Backend and draws until installed. It then reads the category's live and peak CPU bytes and the actual Vulkan allocations attributed to the path. CPU and GPU cleanup debt after shutdown was 0 for all 152 measurements. Edited contents are the worst case at every level (generated is equal or lower). This table shows the worst values.

| Level | Summary retained | Raster CPU configured / peak | Raster Vulkan | Brickmap CPU configured / peak | Brickmap Vulkan |
| --- | ---: | ---: | ---: | ---: | ---: |
| 64³ | n/a | 1,042,136 / 1,176,568 | 18,160 | 102,192 / 385,048 | 88,176 |
| 32³ | 133,016 | 266,176 / 495,680 | 9,936 | 44,912 / 127,832 | 33,136 |
| 16³ | 18,160 | 81,088 / 320,880 | 7,312 | 20,720 / 60,408 | 10,384 |
| 8³ | 3,824 | 27,104 / 77,456 | 4,288 | 12,128 / 51,816 | 2,176 |

- Each single-volume figure includes the path's fixed per-assembly state (for example, most of the 12,128 bytes for Brickmap 8³), so the per-volume costs are conservative.
- The #137 summary figures were 3,736–3,744 bytes higher at every level. Its "retained" delta also freed the profile's own JSON record. These figures exclude it.
- Brickmap Vulkan is 16–32 bytes above #137 because of the one-cell grid table.

## Whole-world assembly overhead

All 34 distinct route centres were measured as 256-volume assemblies under both demo and qualification bands. Overhead is the assembly minus its generated per-volume sum:

| | Raster CPU | Raster Vulkan | Brickmap CPU | Brickmap Vulkan | Summaries (8³ set / streamed set) |
| --- | ---: | ---: | ---: | ---: | ---: |
| Demo max | **+303,840** | 0 | −2,394,376 | −40,880 | −270,440 / −206,496 |
| Qualification max | −740,640 | 0 | −2,701,904 | −169,952 | −270,440 / −42,408 |

- Brickmap overhead is negative even with the grid table included, because the 256 per-volume terms each carry the fixed state. The table is still charged explicitly: `H_B = H_BG = 1,048`.
- Raster CPU has a single step of +303,840 bytes. It appears only at the largest demo selection (49/120/87/0) and looks like a container-capacity doubling. `H_R = 303,840`.
- Largest single assemblies observed: Raster CPU 89.4 MB live / 89.6 MB peak, Raster Vulkan 2.53 MB, Brickmap CPU 9.2 MB live / 33.2 MB peak (construction scratch), Brickmap Vulkan 8.62 MB.

## Cap formulas (proposed, not ratified)

The structure follows #117/#118: 6 assemblies (3 Render Path owners × old/new), one raster worker deriving one volume at a time, and Brickmap construction scratch over every selected volume. `M` is the frozen per-level maxima.

| Category | Formula | Demo (M = 49/120/156/87) | Qualification (M = 9/16/56/231) |
| --- | --- | ---: | ---: |
| Summaries | `256·S8 + 2·M32·S32 + 2·M16·S16` | 38,568,704 | 7,269,376 |
| Raster CPU | `6·(Σ M_L·R_L + H_R) + R_peak` | 591,080,968 | 149,639,560 |
| Brickmap CPU | `6·(Σ M_L·B_L + H_B) + Σ M_L·Bpeak_L` | 136,250,944 | 54,470,624 |
| Raster GPU | `6·Σ M_L·G_L` | 21,575,328 | 10,334,496 |
| Brickmap GPU | `6·(Σ M_L·BG_L + H_BG)` | 60,643,248 | 14,453,808 |

Every observed single assembly is below its one-assembly term.

**Open for ratification:** the demo Raster CPU cap (591 MB) is larger than the fully resident 256-volume Raster CPU baseline in the streamed-fixture report (262 MB), so it no longer bounds anything. The ×6 multiplier drives it. Replacing per-level maxima with the worst achievable selection over all 256 centres (`caps.py`, lower block) trims it only to 543 MB.

## Frame time: two laps per starting Render Path, one long-lived backend

`whole-world-laps` keeps one backend for the lap. At each centre move the next scene is prepared with the route clock paused (standing in for worker preparation, up to 1.4 s) and installed in place while the clock runs. The frozen switches install the other path the same way. Each frame is attributed to the path presenting it. All four laps: 47 crossings installed, 2 switches, **0 coverage stalls**.

| Lap | Presenting | Frames | p99 whole | p99 crossing | p99 switch | Max | GPU p50 |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Brickmap start 1 | Brickmap | 24,988 | 6.14 | 6.17 | 7.12 | 416.1 | 5.19 |
| | Raster | 49,818 | 1.75 | 4.13 | 1.21 | 804.0 | 0.47 |
| Brickmap start 2 | Brickmap | 21,422 | 7.20 | 7.23 | **8.70** | 423.7 | 6.10 |
| | Raster | 49,679 | 1.81 | 4.14 | 1.20 | 659.2 | 0.47 |
| Raster start 1 | Brickmap | 10,166 | 7.06 | 7.01 | 7.80 | 409.7 | 6.02 |
| | Raster | 102,994 | 1.67 | 1.84 | 1.15 | 965.8 | 0.47 |
| Raster start 2 | Brickmap | 9,685 | **7.48** | 7.54 | 8.49 | 444.9 | 6.30 |
| | Raster | 99,973 | 1.70 | 1.84 | 1.15 | 589.0 | 0.48 |

**Every p99 is ≤ 10 ms. Worst: Brickmap 7.48 ms whole route, 7.54 ms crossings, 8.70 ms switches.**

**Maximum frames (recorded, not gated):** 0.04–0.07% of frames exceed 10 ms.
- **Raster installs:** nearly all of them are Raster in-place installations, where the frame thread spends 500–965 ms installing a complete 256-volume Raster artifact. This is a harness shape: production installs per-volume artifacts incrementally. It belongs in spec as a latency and CPU-scope risk for coarse Raster installation.
- **Raster→Brickmap handoff:** the ~420 ms Brickmap-presenting frame is the single Raster→Brickmap handoff frame in each lap.
- **No startup burst:** the #140 8-frame burst did not appear as a recurring tail.

Detail latency (descriptive, not the frozen gate measurement): on-clock installation p50/max is 0.014/1.36 s for Brickmap and 0.87/1.51 s for Raster. Off-clock preparation adds up to 1.4 s. Preparation plus installation per crossing reaches a maximum of 2.68 s. 5 of 188 crossings exceed 2.5 s, all of them Raster whole-scene rebuilds. This harness rebuilds the whole world at every crossing; production prepares only the changed volumes. That makes incremental coarse preparation a spec requirement for the 2.5 s gate.
