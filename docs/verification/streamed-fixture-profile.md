# Streamed fixture qualification: failed

[#117](https://github.com/PetrSeifert/voxel-nexus/issues/117) has a negative feasibility result. The bounded prototype completed allocation calibration and CPU residency replay, then the rendered route stalled after switching to Brickmap. The local display froze, process termination did not recover it, and the user restarted the machine. The responsible observed phase is GPU frame progress. The shader/driver root cause is unknown.

The selected qualification is not ready for milestone specification. [#118](https://github.com/PetrSeifert/voxel-nexus/issues/118) explicitly reopens GPU feasibility shaping and the remaining complete proof. The numeric caps below remain provisional. No budgets were raised to accommodate the failure, no acceptance proof was reduced, and no hysteresis was added.

## Frozen fixture

Recipe `streamed-qualification-v1` declares a finite 16x16 grid of 64x64x64 volumes, one volume tall, voxel size 1, origins `[64*x,0,64*z]`. The clipped 3x3 selection settles at nine volumes. The local inclusive surface height is `24 + ((x/8 + z/8) % 9)`. Occupied cells below it are stone, the surface is grass, and `[16,24) x [8,16) x [16,24)` is an empty cavity. Coordinates above the surface or outside the finite scene are empty. Linear RGBA values are stone `[0.35,0.30,0.25,1]` and grass `[0.12,0.55,0.18,1]`.

Sparse publication emits column fills and surface details. The three-coordinate atomic edit sets `[4,40,4]` to stone, `[32,20,32]` to empty and `[20,12,20]` to grass. Restoration uses generated values. FNV-1a visits the complete volume in x-fastest/y/z order with byte codes air 0, stone 1, grass 2. Generated/restored fingerprint is `9660d9308d69ede5`; edited fingerprint is `478ee4a9737e1ad7`.

The script retains revisions 1 and 2, edits `(3,3)`, evicts `(2,2)` at center `(4,3)`, edits that non-resident volume, reloads it, verifies historical reads, restores both volumes, drops history and repeats travel. Bounds are six edited coordinates and two historical views. Sources retain immutable recipe/compact versioned edits rather than voxel payloads. Shared copies use one fixed scene identity plus volume coordinate and content version. One synchronous generation admission reserves inside the nineteen-copy cap; a superseded target can complete its current volume, then its unneeded copy is released before the newest target's admission.

Raster uses 16-cubed regions and caches immutable per-volume geometry by the same key. Brickmap is explicit; the qualification streamed constructor rejects Dense with `ComputeSceneBuildError::StreamedDense`. Both paths participate in construction, installation and retirement. These are qualification adapters, not a production streaming API. Complete packed compute replacements are rebuilt and timed rather than assumed incremental.

## Projection and clock

Drawable is 1920x1080, vertical FOV 60 degrees, near 0.1, far 34. Eye height is 48. At 4 units/s the route follows horizontal points `[224,224]`, `[352,224]`, `[480,352]`, `[352,224]`, `[224,224]`, aiming 16 horizontal units along its current segment and 24 down. A lap takes 154.50966799187808 seconds.

Crossings occur at 8, 24, 43.31370849898476, 65.94112549695429, 88.5685424949238, 111.19595949289332, 130.50966799187808 and 146.50966799187808 seconds. They include axial and simultaneous diagonal changes and return/reload; consecutive crossing travel is at least 64 units. The first diagonal crossing requests a switch. The final prototype also requests the reverse switch at crossing index 5; that extension was not executed before the GPU failure.

The unresolved clock starts at the analytic crossing time before admission and uses `get_or_insert` so supersession cannot restart it. Installation and handoff run inside the backend's frame-boundary hook after the preceding-frame fence wait. Handoff compares selection, scene, visible/required revision, camera revision, presentation configuration and readiness. It includes complete candidate and replacement preparation and actual configuration costs. Retirement consumes owner slots until shutdown succeeds. Maximum owners are three, including hidden candidates and retiring paths.

Coverage uses the complete frustum's eight-corner AABB clipped to finite scene bounds, not sampled rays. Far-corner reach is about 52.53 units, leaving 11.47 inside the guaranteed 64-unit neighborhood reach. Accepted camera states and rendered/oracle comparisons require installed coverage. The oracle uses an independent whole-scene recipe view containing all 256 volumes; non-resident contents are never treated as empty.

## Accounting and provisional caps

The tagged allocator counts unique successful System allocation layouts, capacity, Arc headers and its aligned category header. Allocation tags survive deallocation on another phase. It includes faces, face-to-quad mappings and scratch, and does not sum shared `storage_bytes`. It measures allocated layout sizes, not allocator internals, process RSS or private driver host memory.

Vulkan tracing records actual `VkMemoryAllocateInfo::allocation_size` and individual live/free ownership across configuration and retirement. Raster buffers and Brickmap scene buffers are separated from depth/output images and camera/probe controls. The fully resident GPU baseline actually configured and drew the paths in a separate process, including a three-owner overlap. It is not a sum of hypothetical requested buffer lengths.

Counts were fixed independently of total observed peaks: nineteen CPU copies including one query-only copy, one generation worker, one derivation worker, three renderer owners, at most eighteen representation copies per owner. Thus the conservative representation allowance is `3 * 18 = 54`. The single raster worker derives one volume at a time. Complete compute construction has scratch covering nine volumes. Six nine-volume containers cover old/new assemblies across three owners.

| Constant | Bytes | Source |
| --- | ---: | --- |
| S, worst retained materialization including moved recipe allocations | 413,448 | CPU per-volume generated/edited/restored calibration |
| P, conservative per-volume publication peak | 2,858,304 | Sum of materialization and recipe category peaks |
| R, worst configured raster CPU ownership | 1,055,136 | One edited volume with actual configuration |
| R_peak | 1,176,992 | Worst one-volume raster derivation |
| H, nine-volume container/bookkeeping overhead | 149,952 | Matched selection minus eight generated and one edited per-volume owners |
| B, worst configured Brickmap CPU ownership | 100,856 | One edited volume with actual configuration |
| B_peak | 385,368 | Worst one-volume complete construction |
| G_raster | 18,160 | Actual per-volume Vulkan allocations, 120 buffers |
| G_brickmap | 88,160 | Actual edited scene-buffer allocation |

| Category | Formula | Provisional cap | Fully resident 256-volume baseline |
| --- | --- | ---: | ---: |
| CPU materialization/generation | `19*S + (P-S)` | 10,300,368 | 101,568,512 |
| Raster CPU artifacts/construction | `54*R + 6*H + R_peak` | 59,054,048 | 262,029,440 |
| Brickmap CPU artifacts/construction | `54*B + 9*B_peak` | 8,914,536 | 22,547,240 |
| Raster GPU data | `54*G_raster` | 980,640 | 4,382,720 |
| Brickmap GPU data | `54*G_brickmap` | 4,760,640 | 21,504,064 |

The baseline exceeds every residency-scaled category, even using its unconfigured CPU representation bytes as a lower bound. Coefficients bound generated, edited and restored states. They are derived from per-volume costs and measured assembly overhead, not fitted to the residency peak.

Plain proposed bounds are Control heap 16,777,216 bytes, metadata heap 262,144 bytes, source/edit/history heap 16,384 bytes, and application-owned fixed GPU memory `3 * 8,847,968 = 26,543,904` bytes. Metadata is bounded to 256 entries, sources to one recipe/two materials, live historical views to two and edited coordinates to six. Fixed Vulkan objects have at most three renderer pipeline owners, nine owner camera/probe/output allocations, three depth/output images and nine framebuffers over three swapchain images. Driver-owned swapchain images and private pipeline/driver allocations have object-count bounds; their private memory is not claimed as measured application allocation bytes. Qualification audit bookkeeping is bounded to 65,536 entries and charged to Control in the final prototype.

## Observed results and limits

CPU-only replay reaches nineteen copies with disjoint old/new selections and a query, rejects a second global query, preserves version reuse during unrelated edits, reconstructs historical fingerprints, restores generated values and drops all tracked residency/source/metadata allocations. A query-to-selection ownership transfer generates no duplicate copy. Two simulated travel laps return to identical allocation bytes and counts, with nine settled copies. These are CPU lifecycle checks, not rendered motion or latency evidence.

| CPU replay category | Peak bytes |
| --- | ---: |
| Materialization plus generation, conservative sum of category peaks | 9,983,264 |
| Raster CPU | 18,609,768 |
| Brickmap CPU | 4,020,448 |
| Retained world metadata | 22,584 |
| Source/edit/history peak | 928 |

Matched 8x8 and 16x16 owners have identical materialization and both derived representation allocations for the same edited neighborhood. Metadata is reported separately and grows with the catalog; history remains identical. The rendered matched runs also configured both strategies and returned zero Vulkan validation warnings/errors.

The rendered route completed only three of eight crossings in its first lap. Measured installation times were 0.1647575, 0.1729473 and 0.250649201 seconds. The completed Raster-to-Brickmap handoff took 0.2453271 seconds. These prefix measurements meet 2.5 seconds. Peak captured residency GPU data was Raster 311,280 bytes and Brickmap 756,064 bytes. The prefix reached nineteen CPU copies and three owners, and exercised an injected raster upload failure while preserving presentation.

After handoff, the last installed selection was centered at `(6,4)` with Brickmap presenting. Progress stopped for at least 386.1667223 seconds, far beyond the complete lap duration. The host display froze and required restart. No complete GPU route result exists. Zero coverage stalls for the complete route, reverse handoff, rendered repeated plateaus, later disjoint-renderer/churn extensions and final retirement/cleanup are **unverified**, not passing results. The failure is GPU/frame progress, not a measured byte-cap violation. No shader root cause is asserted.

GPU traces are exploratory captures from before the final CPU-only mode, separate metadata/history tags, explicit GPU opt-in and extended stress cases. Earlier calibration's dynamically growing audit map was charged to the invoking renderer; the route preallocated it as Control. The conservative calibrated CPU coefficients include the earlier audit overhead, while the separate current CPU baseline proves the residency inequalities without that overhead. These provenance differences and the aborted run prevent ratifying the caps. `host-failure.json` records the captured route executable hash and the user-reported recovery. `residency-context.json` identifies the final CPU sources and evidence separately.

The earlier isolated-allocation calibration remains as archived `calibration.jsonl`, `baseline.jsonl`, `summary.json` and `context.json`. Its two findings still explain the prototype choice: ordinary published historical views pin every volume, and staging all nine recipe inputs breaches the original proposed CPU envelope. It is not the final residency verdict.

## Verify without graphics work

PR review fixes reject overdue route crossings before an unfinished target can be replaced, count actual installations separately from crossing requests, and validate handoff stamps before changing presentation. Any preparation or stamp failure shuts down both prepared paths. CPU regression tests cover these checks. These changes postdate the archived source hashes in `residency-context.json`; the captured GPU evidence remains unchanged and was not rerun.

Run `./scripts/verify-streamed-residency.ps1` to verify the captured evidence and regenerate `residency-summary.json`. Exit zero means the recorded evidence is internally consistent; the summary verdict remains **FAIL**. Run with `-RunCpu` to rebuild and repeat CPU-only calibration, matched scenes, separately processed baseline and lifecycle replay. These modes create no Vulkan instance or window and submit no GPU work.

Graphics modes require a separate deliberate invocation with `--allow-gpu`. Do not use the failed route as routine verification on the user's desktop. #118 owns a bounded reproducer on a dedicated device, diagnosis at the actual failing seam and complete recapture from one committed source/device context before accepting the shaping choice.

Validation: new sharing/reconstruction/admission tests, frontend/oracle/raster/compute/backend release regression suites with all features, `cargo fmt --all`, and workspace/all-target/all-feature Clippy. Production streaming architecture, durable persistence, networking, gameplay, dense streamed compute and large-terrain raster remain deferred.
