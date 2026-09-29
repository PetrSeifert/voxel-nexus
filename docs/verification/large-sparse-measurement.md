# Large sparse workload evidence

Implements [issue #111](https://github.com/PetrSeifert/voxel-nexus/issues/111).
The 2048×256×2048 terrain passes the **1 GiB peak GPU scene budget**, including
one forced pool growth. This is a workload qualification, not a bound on arbitrary
future edits. Timings are descriptive and have no pass/fail threshold.

## Reproduce

On Windows with Vulkan 1.3, the Khronos validation layer, GPU timestamp support,
and a device capable of the workload:

```powershell
pwsh -NoProfile -File scripts/verify-large-sparse.ps1
# Keep another set of evidence without overwriting the checked-in run:
pwsh -NoProfile -File scripts/verify-large-sparse.ps1 -Runs 5 -OutputDirectory .scratch/large-sparse
```

The script builds release binaries, runs the existing large-terrain Vulkan
qualification, checks the owner's deterministic growth-budget rejection test,
then starts at least three fresh measurement processes. Each process writes
JSON Lines, checks semantic probes and Vulkan validation, and shuts down without
retained compute resources. A nonzero exit stops verification. The report tool
requires every record, matching device and workload conditions, all four edit
transitions, successful growth, clean completion, and repeated timing samples.
It rejects measured peaks above 1,073,741,824 bytes, regardless of the predictor's
result. It also checks agreement between the growth and memory records.

The deterministic rejection fixture predicts 8,444 bytes against an 8,443-byte
budget. It verifies rejection and retry without changing visible storage. This
small fixture exercises the owner's budget check cheaply and repeatably. Its
result is recorded separately; it is not evidence of a measured over-budget GPU
allocation. The owner checks the same budget predicate at the demonstrated scale.

The runner has two modes:

```powershell
cargo run --release -p desktop-demo --features qualification --bin large-sparse-measurement -- first-correct-frame .scratch/first.jsonl
cargo run --release -p desktop-demo --features qualification --bin large-sparse-measurement -- complete .scratch/complete.jsonl
cargo run --release -p measurement-evidence --bin large-sparse-report -- docs/evidence/large-sparse/development-machine/run-1.jsonl docs/evidence/large-sparse/development-machine/run-2.jsonl docs/evidence/large-sparse/development-machine/run-3.jsonl
```

Create the output parent directory first. The first mode stops after initial
qualification and its memory record. The complete mode adds edits, steady frames,
and growth. Only the verification script attaches the separately verified
`predicted_rejection` record, so a standalone runner file is intentionally
insufficient for a final report. Raw records are the source of truth; `report.json`
contains count, median, nearest-rank p95 and maximum for every timing.

## Measurement boundaries

- `first_correct_frame` starts immediately before sparse publication and ends
  when GPU timestamps confirm completion of a frame at the initial revision.
  Generation, enumeration replay, and semantic checking are outside this timing.
  Backend setup, shader/pipeline setup and observation overhead are included.
- `publication` includes validation and sparse storage construction. `validation`
  comes from the existing publication counter and is a nested subphase.
- `preparation` is the existing compute preparation event. It includes enumeration,
  brickmap construction and serialization. `enumeration_replay` independently
  traverses the same view with edge 8 and batch capacity 64 after the first frame.
  It isolates enumeration cost without adding instrumentation to owners. It is
  not an in-place timestamp of the preparation traversal. These timings overlap
  in meaning and must not be summed or subtracted to claim an exact decomposition.
- `upload` measures the owner's buffer creation and coherent host-memory write.
  It is a CPU duration, not a DMA timestamp.
- Each edit timing starts before `VoxelFrontend::edit`, includes submission and
  convergence, and ends after a completed frame contains the required revision.
  Each transition is subsequently checked against analytical and oracle probes.
  The command changes one value at the cavity roof or floor. No ordinary edit
  grows the pool. The 1 ms convergence polling interval is included.
- Steady samples follow all four edits, which restore the terrain, and precede
  forced growth. After five seconds of warmup, 60 CPU draw-call durations are
  paired by frame sequence with 60 GPU frame timestamps. The actual Vulkan extent
  must be 1920×1080. The hidden borderless window avoids desktop interference;
  this measures offscreen-window rendering, not display scanout latency. Validation
  and allocation counters remain enabled. The fixed camera is [32.5,88,32.5]
  looking at [40.5,80,40.5], with a 60-degree vertical field of view.
- Growth adds 16,641 isolated occupied values in previously empty 8³ cells at
  y=200 and z>=1024. This exhausts 16,640 headroom slots and grows capacity from
  83,200 to 124,800. `growth` includes publication, convergence and frame completion;
  `growth_rebuild` and `growth_upload` are the existing nested owner observations.

## Memory accounting

Every byte field is an integer. CPU observations count live requested Rust heap
allocations through the same allocator counter as the publication example.
They exclude allocator bookkeeping, native driver allocations, stacks, and
resident-process overhead. The categories overlap and must not be added together.

- Publication peak includes input, retained frontend storage and publication
  scratch. This is a conservative bound on scratch, not a claim that all those
  bytes are scratch. The baseline precedes terrain generation; peak collection
  starts just before publication.
- Enumeration peak includes output and working allocations during the replay.
  Output bytes measure the largest returned batch's owned heap allocations.
  Working-state bytes sample allocations retained after dropping each output
  batch. Transient allocations inside `next()` are included in the combined peak,
  not necessarily in the retained working-state sample. `enumeration_working_cells`
  is the owner's deduplication counter; zero is valid for the split-brick iterator.
- Preparation peak is the incremental requested heap high-water mark above the
  pre-preparation baseline, including the new CPU representation and scratch.
  The growth preparation bound additionally includes frontend editing, concurrent
  preparation, and staging through completed installation.
- Initial GPU allocation bytes come from querying Vulkan requirements for the
  same size, usage and sharing mode as the owner's scene buffer. The owner allocates
  exactly that requirement. The query allocates no device memory and destroys its
  temporary buffer. Upload staging is the owner's serialized upload byte count.
- Ordinary edits share the visible scene allocation. Their record charges its
  allocation plus the largest patch payload. Growth uses the owner's observed
  old-plus-new Vulkan allocation requirements plus complete new upload staging.
  Its `actual_peak_bytes` includes Vulkan alignment. Visible/candidate bytes in
  the growth record are that peak minus staging.
- The budget includes CPU upload staging as required by the workload plan. It
  excludes CPU frontend and preparation storage, render targets, camera/probe
  buffers, pipelines and swapchain resources. This is scene-budget accounting,
  not a hardware VRAM residency measurement. Zero CPU fields in non-initial memory
  records mean that category was not collected again, not zero process usage.

## Recorded result

Measured on 2026-09-29, Windows x64, AMD Ryzen 5 7600, NVIDIA GeForce RTX 4070,
raw Vulkan driver version 2497200128, Rust `1.99.0-nightly (14cae6813 2026-07-08)`.
The terrain fingerprint is `58ce86bc1227cf55`. See the
[raw runs and report](../evidence/large-sparse/development-machine).

The live device query reports `maxStorageBufferRange = 4,294,967,295` bytes.
The demonstrated dense 32-bit payload needs **4,294,967,296 bytes**, one byte over
that limit before headers and materials.

| Scene-budget component | Bytes |
| --- | ---: |
| Initial scene allocation | 93,585,504 |
| Initial upload staging | 93,585,492 |
| Initial combined peak | 187,170,996 |
| Ordinary edit combined peak | 93,586,532 |
| Growth visible and candidate allocations together | 229,769,408 |
| Growth upload staging | 136,183,892 |
| Growth predicted peak | 365,953,276 |
| Growth measured peak, including staging | 365,953,300 |
| Budget | 1,073,741,824 |

The measured peak is about **349.00 MiB, 34.08% of budget**. All three runs agree
on those byte counts. Empty-to-mixed and uniform-to-mixed edits upload 1,028 bytes;
mixed-to-empty and mixed-to-uniform upload 4 bytes.

The initial CPU publication peak is 408,910,924 requested bytes; preparation peaks
at 409,566,752 incremental bytes. Enumeration's combined peak is 6,360 bytes, with
4,096 output bytes and 216 retained working-state bytes. Growth's incremental
preparation-and-upload bound ranges from 584,946,294 to 585,060,982 bytes.

Timing distributions are recorded below after verification. They include all
samples, without trimming outliers. Three runs are enough to establish repetition,
but their p95 is simply their maximum and is not a tail-latency estimate.

| Timing | Samples | Median ms | p95 ms | Maximum ms |
| --- | ---: | ---: | ---: | ---: |
| `empty_to_mixed` | 3 | 7.433 | 7.488 | 7.488 |
| `enumeration_replay` | 3 | 357.497 | 367.600 | 367.600 |
| `first_correct_frame` | 3 | 2112.118 | 2125.581 | 2125.581 |
| `growth` | 3 | 3204.266 | 4460.659 | 4460.659 |
| `growth_rebuild` | 3 | 2404.978 | 3684.470 | 3684.470 |
| `growth_upload` | 3 | 31.065 | 31.393 | 31.393 |
| `mixed_to_empty` | 3 | 7.582 | 9.669 | 9.669 |
| `mixed_to_uniform` | 3 | 7.204 | 11.299 | 11.299 |
| `preparation` | 3 | 1584.573 | 1588.082 | 1588.082 |
| `publication` | 3 | 272.042 | 272.681 | 272.681 |
| `steady_cpu` | 180 | 1.692 | 2.444 | 3.749 |
| `steady_gpu` | 180 | 1.118 | 1.703 | 2.227 |
| `uniform_to_mixed` | 3 | 7.464 | 7.664 | 7.664 |
| `upload` | 3 | 19.305 | 19.821 | 19.821 |
| `validation` | 3 | 113.531 | 119.985 | 119.985 |

## Semantic limitations

The large-scale checks use `observe_along_ray` plus independent analytical probes.
They cover empty travel, occupied surface and interior, cavity walls, roof/floor
edits, and a newly occupied growth-pool cell. They do not brute-force the billion
logical values, compare every screen pixel, or prove correctness for every ray.
The existing large-terrain GPU test and runner share the same analytical fixtures.
The measurement code consumes existing owner observations and adds no owner
instrumentation.

## Implementation checks

`cargo test --workspace --all-targets --all-features` passed, with the explicitly
ignored GPU and timing tests left opt-in. The script above runs the large sparse
GPU qualification explicitly. The latest six `measurement-evidence` large sparse
contract tests pass, including budget boundaries, contradictory records, exact
device limits, incomplete runs, JSON Lines shapes, and clean shutdown requirements.
`cargo fmt --all` and `cargo clippy --workspace --all-targets --all-features` are clean.
Independent Standards and Spec reviews found no blocking issues. Standards noted
the pre-existing numeric probe-phase convention as a possible future cleanup.
