# Incremental compute convergence

Issue #89 replaces full-scene preparation for adjacent edits with immutable, shared CPU pages and changed-word uploads. A binary tree limits page copying to the edited paths and leaves of at most 1,024 words. Metadata is shared between bundles. Inspection methods `voxel_words()` and `storage_words()` now return materialized vectors; convergence does not call them on the incremental path.

Preparation reads the changed regions from the newest authoritative view. It validates the complete chain from Visible to Required, including scene identity, adjacent revisions, and compatible volume/material metadata. For dense scenes, missing, mismatched, or discarded chains use the full rebuild. History is bounded to 64 change sets while visibility is stalled.

An incremental hidden candidate retains CPU patches without changing the installed GPU buffer. The existing candidate-upload checkpoint means the candidate is staged and may be held or rejected there. The actual range writes happen at installation, after the backend waits for the preceding frame and after supersession, hold, and injected failure checks. All ranges are validated before a single memory mapping, so a mapping failure cannot partially update Visible data. CPU installation immediately follows the writes. Installed bundles release transient patches. Full rebuilds still use separate candidate GPU buffers.

## Preparation and upload measurements

Measured on the development machine on 2026-09-26, in release mode, with 20 alternating single-voxel edits per size. Scene generation and frontend edits are outside the preparation timer. Before uses commit `ff68885c1cc7160a0cf4d55051aae0c4728881ac` in a detached worktree with the same fixture and full-builder timer. After uses the incremental implementation. Byte counts are the exact payload passed to the upload code, not GPU timing or bus-traffic measurements.

| Volume | Before preparation, mean ms | After preparation, mean ms | Before bytes/edit | After bytes/edit |
| --- | ---: | ---: | ---: | ---: |
| 32³ | 0.425485 | 0.001230 | 131,136 | 4 |
| 64³ | 3.585650 | 0.001995 | 1,048,640 | 4 |
| 128³ | 33.869145 | 0.009540 | 8,388,672 | 4 |

These are single-run observations, not latency guarantees. Tree depth and cache effects still vary with scene size. Unchanged voxel payload is shared, and only changed words are uploaded.

Reproduce the after measurement and its full-rebuild comparison:

```powershell
cargo test --locked --features qualification --release -p compute-ray-render-path measure_incremental_preparation -- --ignored --nocapture
```

## Verification

- Unit coverage checks immutable predecessors, repeated edits across a chain, missing/truncated chains, broad-edit patch release, shared tree branches, cancellation, retries, and newest-only candidate installation.
- The installed GPU DDA regression passes on the NVIDIA GeForce RTX 4070, driver `0x94d84000`, with Vulkan validation enabled.
- The validation-enabled portable compute-ray milestone demo passes 15 Semantic Ray qualifications and three Render Path switches. The edit burst advances Required through 1, 2, 3, 4 while Visible and installed compute revisions are only 1 and 4. No obsolete frames or observations occur. Vulkan reports zero warnings and zero errors; shutdown reports zero owned objects, allocations, workers, and views.

Commands:

```powershell
cargo test --locked --features qualification --workspace --release
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --features qualification -p desktop-demo --test compute_dda_gpu -- --ignored --nocapture
pwsh -NoProfile -File scripts/verify-portable-compute-ray-milestone.ps1 -EvidenceDirectory artifacts/issue-89-validation
```

Local runtime logs and the summary are in `artifacts/issue-89-validation`. The verification script was copied locally with its process launch set to `-WindowStyle Hidden`; qualification logic was unchanged. The run occurred before committing, so its recorded repository revision is the parent commit.


## Brickmap convergence, issue #107

Brickmap preparation maps every changed region to its intersecting 8� cells and deduplicates them before classification. Missing revisions conservatively invalidate the entire cell grid. Both cases produce patches against the installed allocation, with no rebuild fallback. The packed CPU words use the existing persistent page tree, so a small edit does not clone a scene-sized grid.

The convergence owner retains pool state in its installed bundle and private candidate snapshots. A candidate removes reserved slots only from its private free set and records retired slots separately. Dropping or cancelling the candidate discards those reservations without changing installed ownership. Candidates may name the same free slot because their payloads stay in CPU memory until installation. The owner releases retired slots only when installation commits after the preceding frame has finished. Revision and allocation identity must still match before any GPU write.

Initial construction reserves up to one spare slot per coarse cell, capped at 64 spare slots and the remaining byte budget. Every slot occupies 1,024 bytes. The material table includes the scene's currently unused materials so later edits can reference them without moving the grid or pool. Pool growth is not implemented. Exhaustion reports a preparation failure while the installed revision continues presenting.

Hidden candidates expose dirty-cell, reserved-slot, retired-slot, and upload-byte counts through `ComputeConvergenceStatus::hidden_patch`. The installed counts remain available through `installed_patch` and `ComputeSceneBundle::brickmap_patch_observations`. Upload timings continue to report the actual patch payload size. An error during mutation permanently stops frame recording and convergence for that Render Path; the qualification hook deterministically writes the first range and then reports failure.

Verified on 2026-09-29:

- In-crate owner tests cover structural transitions, deduplication, supersession, skipped revisions, cancellation, stale revision/allocation bases, capacity exhaustion, and deferred retirement.
- The Windows Vulkan adapter regression covers all four structural transitions against the Semantic Ray oracle, held candidates preserving visible observations, one-slot retirement/reuse, capacity failure preserving presentation, and partial-write failure preventing further frames. Vulkan validation reports zero errors and warnings.
- The existing brickmap milestone proof passes edit-burst newest-only convergence and the switching round trip. Evidence is in `artifacts/issue-107-brickmap`.

Commands:

```powershell
cargo test --workspace --all-features
cargo test -p desktop-demo --features qualification --test compute_dda_gpu -- --ignored --nocapture
pwsh -NoProfile -File scripts/verify-portable-compute-ray-milestone.ps1 -EvidenceDirectory artifacts/issue-107-brickmap -ComputeRepresentation brickmap
cargo fmt --all
cargo clippy --workspace --all-targets --all-features
```
