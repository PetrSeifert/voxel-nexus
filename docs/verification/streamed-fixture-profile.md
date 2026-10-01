# Streamed fixture production qualification: passed

The production streamed qualification passes on the NVIDIA GeForce RTX 4070, driver `0x94d84000`. All CPU and GPU modes were recaptured on 2026-10-01 from the clean committed source revision `44b7ec252f571012e7d65f344d91e76adf0d98c7`. Since [#134](https://github.com/PetrSeifert/voxel-nexus/issues/134), every streamed residency limit derives from a caller-chosen maximum selection size N, and since [#135](https://github.com/PetrSeifert/voxel-nexus/issues/135) the interactive streamed demo uses a 7x7 neighbourhood, distance fog and voxel edge shading. That revision also enables demote-to-helper-invocation on the Vulkan device and adds a Render Backend ledger of Render Path GPU memory for the demo overlay ([#133](https://github.com/PetrSeifert/voxel-nexus/issues/133)). The ledger reserves its table when the backend is created, so it adds no allocations inside Render Path CPU scopes. The qualification still selects its 3x3 neighbourhood and configures N = 9, so the nineteen-copy, eighteen-copy and nine-volume limits are unchanged, and its cameras keep presentation styling off. Every category peak matches the previous capture. The [production context](../evidence/streamed-fixture/production/residency-context.json) records the revision, toolchain, device, executable hash and source hashes. The [production summary](../evidence/streamed-fixture/production/residency-summary.json) records `PASS` under the complete verifier contract from [#131](https://github.com/PetrSeifert/voxel-nexus/issues/131).

The byte-cap formulas and numeric caps below were ratified on 2026-09-30 in [#118](https://github.com/PetrSeifert/voxel-nexus/issues/118). The constants, formulas, fixture, route, edit script, crossing clock, coverage rule and probe definitions are unchanged. The verifier checks every live and peak sample against those caps and the plain bounds. The first production capture completed [#132](https://github.com/PetrSeifert/voxel-nexus/issues/132) from revision `03cee4fdc0575d5308c603017c75cc1c94a8acc9`. Later source changes invalidated its recorded source hashes, so it was replaced by this recapture.

[#117](https://github.com/PetrSeifert/voxel-nexus/issues/117) first recorded a failure: after the Raster-to-Brickmap handoff, GPU frame progress stopped and the host required a restart. #118 traced that to the Brickmap traversal loop. Near an empty-brick corner, a coarse skip could move an untied axis back into the previous cell, so two cells alternated forever in an unbounded loop. Untied axes can no longer retreat, and a per-volume step cap turns any future violation into a miss; see [the compute DDA regression](compute-dda-regression.md). No budget was enlarged, no proof was reduced and no hysteresis was added. `host-failure.json` preserves the original incident.

## Frozen fixture

Recipe `streamed-qualification-v1` declares a finite 16x16 grid of 64x64x64 volumes, one volume tall, voxel size 1, origins `[64*x,0,64*z]`. The clipped 3x3 selection settles at nine volumes. The local inclusive surface height is `24 + ((x/8 + z/8) % 9)`. Occupied cells below it are stone, the surface is grass, and `[16,24) x [8,16) x [16,24)` is an empty cavity. Coordinates above the surface or outside the finite scene are empty. Linear RGBA values are stone `[0.35,0.30,0.25,1]` and grass `[0.12,0.55,0.18,1]`.

Sparse publication emits column fills and surface details. The three-coordinate atomic edit sets `[4,40,4]` to stone, `[32,20,32]` to empty and `[20,12,20]` to grass. Restoration uses generated values. FNV-1a visits the complete volume in x-fastest/y/z order with byte codes air 0, stone 1, grass 2. Generated/restored fingerprint is `9660d9308d69ede5`; edited fingerprint is `478ee4a9737e1ad7`.

The script retains revisions 1 and 2, edits `(3,3)`, evicts `(2,2)` at center `(4,3)`, edits that non-resident volume, reloads it, verifies historical reads, restores both volumes, drops history and repeats travel. Bounds are six edited coordinates and two historical views. Sources retain immutable recipe/compact versioned edits rather than voxel payloads. Shared copies use one fixed scene identity plus volume coordinate and content version. One synchronous generation admission reserves inside the nineteen-copy cap; a superseded target can complete its current volume, then its unneeded copy is released before the newest target's admission.

Raster uses 16-cubed regions and caches immutable per-volume geometry by the same key. Brickmap is explicit; the qualification streamed constructor rejects Dense with `ComputeSceneBuildError::StreamedDense`. Both paths participate in construction, installation and retirement. The qualification runner exercises the production streamed publication, residency, edit and Render Path APIs. Only the independent whole-scene oracle input remains qualification-only. Complete packed compute replacements are rebuilt and timed rather than assumed incremental.

## Projection and clock

Drawable is 1920x1080, vertical FOV 60 degrees, near 0.1, far 34. Eye height is 48. At 4 units/s the route follows horizontal points `[224,224]`, `[352,224]`, `[480,352]`, `[352,224]`, `[224,224]`, aiming 16 horizontal units along its current segment and 24 down. A lap takes 154.50966799187808 seconds.

Crossings occur at 8, 24, 43.31370849898476, 65.94112549695429, 88.5685424949238, 111.19595949289332, 130.50966799187808 and 146.50966799187808 seconds. They include axial and simultaneous diagonal changes and return/reload; consecutive crossing travel is at least 64 units. The first diagonal crossing requests a switch and crossing index 5 requests the reverse switch. The route runs once starting on Raster and once starting on Brickmap, so each lap exercises both switch directions.

The unresolved clock starts at the analytic crossing time before admission and uses `get_or_insert` so supersession cannot restart it. Installation and handoff run inside the backend's frame-boundary hook after the preceding-frame fence wait. Handoff compares selection, scene, visible/required revision, camera revision, presentation configuration and readiness. It includes complete candidate and replacement preparation and actual configuration costs. Retirement consumes owner slots until shutdown succeeds. Maximum owners are three, including hidden candidates and retiring paths.

Coverage uses the complete frustum's eight-corner AABB clipped to finite scene bounds, not sampled rays. Far-corner reach is about 52.53 units, leaving 11.47 inside the guaranteed 64-unit neighborhood reach. Accepted camera states and rendered/oracle comparisons require installed coverage. The oracle uses an independent whole-scene recipe view containing all 256 volumes; non-resident contents are never treated as empty.

## Accounting and caps

The tagged allocator counts unique successful System allocation layouts, capacity, Arc headers and its aligned category header. Allocation tags survive deallocation on another phase. It includes faces, face-to-quad mappings and scratch, and does not sum shared `storage_bytes`. It measures allocated layout sizes, not allocator internals, process RSS or private driver host memory.

Vulkan tracing records actual `VkMemoryAllocateInfo::allocation_size` and individual live/free ownership across configuration and retirement. Raster buffers and Brickmap scene buffers are separated from depth/output images and camera/probe controls. The fully resident GPU baseline actually configured and drew the paths in a separate process, including a three-owner overlap. It is not a sum of hypothetical requested buffer lengths.

Counts were fixed independently of total observed peaks: nineteen CPU copies including one query-only copy, one generation worker, one derivation worker, three renderer owners, at most eighteen representation copies per owner. Thus the conservative representation allowance is `3 * 18 = 54`. The single raster worker derives one volume at a time. Complete compute construction has scratch covering nine volumes. Six nine-volume containers cover old/new assemblies across three owners.

| Constant | Bytes | Source |
| --- | ---: | --- |
| S, worst retained materialization including moved recipe allocations | 413,448 | CPU per-volume generated/edited/restored calibration |
| P, conservative per-volume publication peak | 2,858,304 | Sum of materialization and recipe category peaks |
| R, worst configured raster CPU ownership | 1,046,656 | One edited volume with actual configuration |
| R_peak | 1,176,992 | Worst one-volume raster derivation |
| H, nine-volume container/bookkeeping overhead | 91,072 | Matched selection minus eight generated and one edited per-volume owners |
| B, worst configured Brickmap CPU ownership | 100,856 | One edited volume with actual configuration |
| B_peak | 385,368 | Worst one-volume complete construction |
| G_raster | 18,160 | Actual per-volume Vulkan allocations, 120 buffers |
| G_brickmap | 88,160 | Actual edited scene-buffer allocation |

| Category | Formula | Cap | Fully resident 256-volume baseline |
| --- | --- | ---: | ---: |
| CPU materialization/generation | `19*S + (P-S)` | 10,300,368 | 101,568,512 |
| Raster CPU artifacts/construction | `54*R + 6*H + R_peak` | 58,242,848 | 262,029,440 |
| Brickmap CPU artifacts/construction | `54*B + 9*B_peak` | 8,914,536 | 22,547,240 |
| Raster GPU data | `54*G_raster` | 980,640 | 4,382,720 |
| Brickmap GPU data | `54*G_brickmap` | 4,760,640 | 21,504,064 |

The baseline exceeds every residency-scaled category, even using its unconfigured CPU representation bytes as a lower bound. Coefficients bound generated, edited and restored states. They are derived from per-volume costs and measured assembly overhead, not fitted to the residency peak. The raster cap is 811,200 bytes below the #117 value: the final source charges audit bookkeeping to Control, not the calibrated renderer.

Plain bounds are Control heap 16,777,216 bytes, metadata heap 262,144 bytes, source/edit/history heap 16,384 bytes, and application-owned fixed GPU memory `3 * 8,847,968 = 26,543,904` bytes. Metadata is bounded to 256 entries, sources to one recipe/two materials, live historical views to two and edited coordinates to six. Fixed Vulkan objects have at most three renderer pipeline owners, nine owner camera/probe/output allocations, three depth/output images and nine framebuffers over three swapchain images. Driver-owned swapchain images and private pipeline/driver allocations have object-count bounds; their private memory is not claimed as measured application allocation bytes. Qualification audit bookkeeping is bounded to 65,536 entries and charged to Control.

## Production results and limits

The production capture runs `cpu-calibration`, `cpu-matched-8`, `cpu-matched-16`, `cpu-lifecycle`, `matched-8`, `matched-16`, `raster` and `brickmap`. The CPU modes create no window or Vulkan instance. They preserve generated, edited and restored fingerprints, replay non-resident edits and historical reads, verify unrelated-edit reuse and compaction, and reach nineteen shared copies with one query copy. Both CPU travel laps settle to identical allocation bytes and counts with nine copies.

Each GPU route starts on its named Render Path and completes two laps. Each records 16/16 fence-safe installations, four switches covering both directions on each lap, zero coverage stalls and 64 matching covered rendered/oracle route probes. Revision replacement has its own covered probe batch. Both routes exercise nineteen-copy admission, rejection of a second query copy, upload-failure recovery that preserves presentation, failed-candidate cleanup and twelve boundary-churn installations without hysteresis. Their settled lap allocation bytes and counts match. All four GPU modes report zero Vulkan validation warnings/errors, zero residency cleanup debt, zero GPU allocations/objects and zero workers after shutdown.

| Route | Worst crossing, s | Worst switch, s | Raster GPU peak, bytes | Brickmap GPU peak, bytes | Fixed GPU peak, bytes |
| --- | ---: | ---: | ---: | ---: | ---: |
| Starting on Raster | 0.238438607 | 0.2332729 | 207,520 | 1,512,128 | 17,695,344 |
| Starting on Brickmap | 0.243349603 | 0.2218772 | 239,680 | 1,524,416 | 17,695,344 |

The verifier checks all 987 CPU/GPU residency samples. The following maxima cover every captured mode. Materialization/generation conservatively sums both category peaks in each sample. The original ratified constants and caps remain unchanged.

| Category | Production peak, bytes | Cap or plain bound, bytes |
| --- | ---: | ---: |
| CPU materialization/generation | 9,967,496 | 10,300,368 |
| Raster CPU artifacts/construction | 15,059,736 | 58,242,848 |
| Brickmap CPU artifacts/construction | 4,049,600 | 8,914,536 |
| Raster GPU data | 239,680 | 980,640 |
| Brickmap GPU data | 1,524,416 | 4,760,640 |
| Control heap | 8,666,360 | 16,777,216 |
| Metadata heap | 183,592 | 262,144 |
| Source/edit/history heap | 1,184 | 16,384 |
| Fixed GPU memory | 17,695,344 | 26,543,904 |

An initial capture exposed 56 bytes of standard-library blocking-channel state first allocated inside Raster accounting. That state is cached until the event-loop thread exits. The committed runner initializes it under Control before rendering, where the same fixed Control bound still applies. The ignored GPU cleanup regression failed before this change and passes across three fresh matched-scene processes after it. No cleanup check, cap, deadline or frozen definition changed.

## Archived prototype results and limits

The numbers in this section describe the archived [#121](https://github.com/PetrSeifert/voxel-nexus/issues/121) prototype capture in `docs/evidence/streamed-fixture/development-machine`, from revision `f7f6be45d8b67de324ebed7b96dd4528f35ffc39`. Those evidence files remain unmodified. Its `residency-summary.json` still says ratification is pending because it describes the evidence at capture time.

CPU-only replay reaches nineteen copies with disjoint old/new selections and a query, rejects a second global query, preserves version reuse during unrelated edits, reconstructs historical fingerprints, restores generated values and drops all tracked residency/source/metadata allocations. A query-to-selection ownership transfer generates no duplicate copy. Two simulated travel laps return to identical allocation bytes and counts, with nine settled copies. These are CPU lifecycle checks, not rendered motion or latency evidence.

| CPU replay category | Peak bytes |
| --- | ---: |
| Materialization plus generation, conservative sum of category peaks | 9,983,264 |
| Raster CPU | 18,609,768 |
| Brickmap CPU | 4,020,448 |
| Retained world metadata | 22,584 |
| Source/edit/history peak | 928 |

Matched 8x8 and 16x16 owners have identical materialization and both derived representation allocations for the same edited neighborhood. Metadata is reported separately and grows with the catalog; history remains identical. The rendered matched runs also configured both strategies and returned zero Vulkan validation warnings/errors.

Each rendered route completed two laps: 16 of 16 crossings installed fence-safe, 4 switches, zero coverage stalls and 64 covered rendered/oracle probes. Both laps settled to nine copies with identical CPU and GPU allocation bytes and counts. Both routes reached nineteen CPU copies and three owners. They exercised disjoint query and renderer overlap, revision replacement, an injected raster upload failure that preserved presentation, a cleaned failed candidate and twelve boundary-churn crossings without hysteresis. Vulkan validation reported zero warnings and errors, and every mode released all tracked CPU residency and GPU memory at shutdown.

| Route | Worst crossing, s | Worst switch, s | Raster GPU peak, bytes | Brickmap GPU peak, bytes | Fixed GPU peak, bytes |
| --- | ---: | ---: | ---: | ---: | ---: |
| Starting on Raster | 0.283269707 | 0.2797441 | 311,280 | 1,512,128 | 26,543,296 |
| Starting on Brickmap | 0.288112307 | 0.2859619 | 308,160 | 1,524,416 | 26,543,296 |

Every sample is within the category caps, the 2.5-second deadline and the fixed GPU bound.

The earlier isolated-allocation calibration remains as archived `calibration.jsonl`, `baseline.jsonl`, `summary.json` and `context.json`. Its two findings still explain the prototype choice: ordinary published historical views pin every volume, and staging all nine recipe inputs breaches the original proposed CPU envelope. It is not the final residency verdict.

## Verify and recapture

The recorded capture command was `pwsh -NoProfile -File scripts/verify-streamed-residency.ps1 -RunCpu -RunGpu -EvidenceDirectory artifacts/streamed-production-133`. Its verified outputs are committed separately in `docs/evidence/streamed-fixture/production`. Recheck those files without dispatching graphics with `pwsh -NoProfile -File scripts/verify-streamed-residency.ps1`.

The production verifier defaults to `docs/evidence/streamed-fixture/production`; the prototype evidence above remains archived. Run `pwsh -NoProfile -File scripts/verify-streamed-residency.ps1 -RunCpu -EvidenceDirectory artifacts/streamed-cpu` to generate and verify CPU residency, fixed caps and the edit lifecycle without creating a window or Vulkan instance. The `cpu-baseline` runner mode remains available as a separate fully resident comparison.

`-RunGpu` requires a clean committed tree and captures both route starts, two laps each, plus matched scenes. Every GPU invocation requires `--allow-gpu`. The verifier checks each live and peak allocation category against the ratified caps, along with fixed GPU memory, object counts, complete covered probe batches, clock origins, switch directions, replacement recovery and repeated allocation plateaus. `-DryRun` verifies local uncommitted GPU runs with a distinct verdict; it cannot be combined with recorded capture.

Run `pwsh -NoProfile -File scripts/test-streamed-residency-verifier.ps1` for the positive and negative verifier fixtures. They create JSON evidence only and dispatch no graphics work. The qualification integration suite also runs them on Windows.

Validation for this production recapture: `cargo fmt --all`, zero-warning `cargo clippy --workspace --all-targets --all-features`, `cargo test --locked --workspace --features qualification`, the explicit ignored matched GPU cleanup regression and the complete recorded CPU/GPU verifier. The workspace suite includes the negative verifier fixtures and unchanged canonical, dense/sparse, switching and large sparse terrain regressions. The GPU cleanup regression is ignored by ordinary test runs; CPU qualification and negative verifier fixtures dispatch no graphics work.

The observations qualify this fixture on this device under the fixed caps and deadline. They do not establish cross-machine performance. Durable persistence, networking, gameplay, dense streamed compute and large-terrain raster remain deferred.
