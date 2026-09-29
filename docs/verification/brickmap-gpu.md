# GPU brickmap rendering

Issue #106 adds `ComputeRepresentation::Brickmap { budget_bytes }` to the compute Render Path. Dense remains the default. The desktop demo accepts `--compute-representation brickmap` and `--brickmap-budget-bytes N`, with a default sparse budget of 134217728 bytes.

The scene buffer contains volume headers, material colors, coarse entries and the packed mixed-brick pool. Empty coarse entries skip an 8³ cell; uniform entries return their material; mixed entries decode a 16-bit material index from the pool. Volume headers are ordered by identity to preserve nearest-contact tie breaking. Rendering and Semantic Ray requests share this traversal. Each Required revision rebuilds the complete sparse representation, with cancellation checkpoints during cell enumeration. Installation uses the existing hidden-candidate and atomic revision handoff protocol.

## Mandatory limits

A metadata-only coarse-grid budget check runs before CPU construction. Before GPU allocation, the entire packed scene buffer must fit the configured budget, `maxStorageBufferRange`, and the device's queried Maintenance4 `maxBufferSize`. Because coarse entries and the pool share one buffer, their combined size plus metadata must fit each limit. The budget counts uploaded scene payload bytes per complete representation, not Vulkan allocation padding, output images, or the sum of Visible and hidden candidates. Both complete representations can coexist during convergence. Device type does not participate in qualification.

Sparse construction enforces these inclusive limits independently:

- Each volume-local extent is at most 65536 voxels.
- Voxel size is between 0.125 and 16 scene units.
- Each scene-origin component lies in [-65536, 65536].
- Every final scene-space bound lies in [-131072, 131072], calculated in double precision as origin plus extent times voxel size.

These limits apply only to the GPU brickmap configuration. The standalone CPU brickmap and existing dense compute workloads retain their existing contracts. Camera construction and updates use the existing Camera State validation, with no extra sparse camera envelope. Semantic Rays likewise retain their existing validation. The scene envelope limits geometry arithmetic; it does not promise arbitrary distant-camera or near-tangent floating-point accuracy.

## Envelope qualification

Issue #109 extends `brickmap_gpu_limits` to check acceptance at each limit and rejection immediately beyond it. Coarse-grid and mixed-pool fixtures check the packed buffer at exactly 60 and 2124 bytes, respectively, then reduce each device capability or the configured budget by one byte. The other device capability remains unlimited during each rejection check. Assertions check the reported limit, required bytes, and available bytes. The public validation boundary accepts byte capabilities and budget, with no device-type input; GPU resource creation passes the queried `maxStorageBufferRange` and `maxBufferSize` to that boundary before allocating the scene buffer.

Spatial checks cover every axis, both origin signs, the 65536-voxel extent, both voxel-size endpoints, and the 131072 final bound. Origin and voxel-size rejection cases use the adjacent representable `f32`. A separate final-bound case would round down to the accepted boundary if calculated in `f32`, so it qualifies the existing double-precision check. The negative final bound of -131072 cannot be reached independently: origins cannot be below -65536, and extents and voxel sizes are nonnegative. Every rejected spatial fixture also builds successfully with the dense representation. The existing camera-update test confirms that a camera outside the sparse geometry envelope remains accepted.

`compute_dda_gpu` adds 600 Semantic Ray comparisons with the exhaustive oracle and CPU brickmap traversal. Of these, 456 qualify sparse scenes at origin limits, maximum local extents, and maximum final bounds, at voxel sizes 0.125, 1, and 16. Thin volumes reach the limits on each axis without requiring a cubic oracle allocation. A one-voxel fixture places contacts exactly at the negative origin limit. Face, edge, corner, and nearly axis-aligned probes approach the occupied far-corner voxel in both directions, rotating through all axes. Fixture checks require the oracle to identify that voxel, so matching misses cannot pass. Distance tolerance remains voxel size times 1e-6, and comparisons include scene identity, revision, voxel coordinate, material, and contact classification.

The other 144 comparisons render dense scenes beyond the sparse origin, extent, final-bound, and voxel-size limits. Existing GPU fixtures still run unchanged. Every GPU fixture requires zero Vulkan validation warnings and errors.

On 2026-09-29, the all-features GPU regression passed on the NVIDIA GeForce RTX 4070 with driver `0x94d84000` and Vulkan validation enabled, with zero warnings or errors. The expanded run completed in 91.62 seconds. A temporary mutation from `required > available` to `required >= available` made both packed-buffer boundary tests fail; restoring the existing inclusive comparison made them pass. No production limits or rendering code changed for this qualification.

## Observations and verification

`ComputeTimingEvent::uploaded_bytes()` reports the bytes written for an Upload event. `elapsed_milliseconds()` measures CPU allocation and upload work, not GPU bus time. Events identify the scene, revision and convergence generation. Preparation and installation events report zero uploaded bytes.

On 2026-09-29, the NVIDIA GeForce RTX 4070, driver `0x94d84000`, passed the GPU regression with Vulkan validation enabled. The regression compares GPU observations with the oracle and CPU brickmap for canonical revisions 1–4, a 25×9×9 volume, and the existing dense DDA boundary cases at voxel sizes 0.125, 1 and 16. It checks both validation warning and error counts. Both dense and brickmap validation-enabled milestone qualifications passed 15 Semantic Ray checks and three Render Path switches each, with zero warnings, errors, obsolete observations or remaining owned resources. It also checks revision supersession.

```powershell
cargo test -p compute-ray-render-path --test brickmap_gpu_limits
cargo test -p desktop-demo --test compute_dda_gpu -- --ignored --nocapture
cargo test -p desktop-demo --test compute_dda_gpu --all-features -- --ignored --nocapture
pwsh -NoProfile -File scripts/verify-portable-compute-ray-milestone.ps1 -EvidenceDirectory artifacts/issue-106-brickmap-validation -ComputeRepresentation brickmap
cargo test --workspace --all-features
cargo fmt --all
cargo clippy --workspace --all-targets --all-features
```
