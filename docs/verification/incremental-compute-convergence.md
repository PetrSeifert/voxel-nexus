# Incremental compute convergence

Issue #89 replaces full-scene preparation for adjacent edits with immutable, shared CPU pages and changed-word uploads. A binary tree limits page copying to the edited paths and leaves of at most 1,024 words. Metadata is shared between bundles. Inspection methods `voxel_words()` and `storage_words()` now return materialized vectors; convergence does not call them on the incremental path.

Preparation reads the changed regions from the newest authoritative view. It validates the complete chain from Visible to Required, including scene identity, adjacent revisions, and compatible volume/material metadata. Missing, mismatched, or discarded chains use the full rebuild. History is bounded to 64 change sets while visibility is stalled.

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
