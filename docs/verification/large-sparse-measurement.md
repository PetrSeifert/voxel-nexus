# Large sparse workload evidence

Implements [issue #111](https://github.com/PetrSeifert/voxel-nexus/issues/111), with
classification, allocation prediction, validation memory, and evidence fixes from
[#112](https://github.com/PetrSeifert/voxel-nexus/issues/112),
[#113](https://github.com/PetrSeifert/voxel-nexus/issues/113),
[#114](https://github.com/PetrSeifert/voxel-nexus/issues/114), and
[#115](https://github.com/PetrSeifert/voxel-nexus/issues/115).
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

The deterministic rejection fixture supplies rounded allocation requirements to
the convergence owner. Packed data fits its 8,444-byte budget, but the
allocation-based prediction is 8,468 bytes, so the owner rejects it.
The fixture checks retry and unchanged visible storage, then emits the values
from that typed rejection. The script attaches that observed record to each run.
This is a repeatable CPU owner check, separate from measured GPU growth peaks.

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
  Generation, the heap measurement replay, and semantic checking are outside this timing.
  Backend setup, shader/pipeline setup and observation overhead are included.
- `publication` includes validation and sparse storage construction. `validation`
  comes from the existing publication counter and is a nested subphase.
- `preparation` measures the actual first compute preparation. Its `enumeration`
  subphase times creation and advancement of the Voxel Cell Grid iterator with
  edge 8 and batch capacity 64. `construction` measures brickmap construction
  excluding those iterator calls. `serialization` measures conversion to the
  complete compute storage buffer, including reserved pool capacity. Each event
  identifies the initial scene revision. The enclosing preparation also includes
  representation validation and measurement overhead, so the subphases need not
  sum exactly to it. The later enumeration replay measures heap categories only.
- `validation_allocation` reports reserved validation vector bytes and their
  simultaneous peak separately from publication construction. It excludes input
  and storage memory, allocator bookkeeping, and error metadata clones.
- `verified` requires 13 probes at each initial/edit revision and 14 after growth,
  with `installed_revision` equal to the revision checked against the oracle.
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

Re-recorded on 2026-09-29, Windows x64, AMD Ryzen 5 7600, NVIDIA GeForce RTX 4070,
raw Vulkan driver version 2497200128, Rust `1.99.0-nightly (14cae6813 2026-07-08)`.
The terrain fingerprint is `58ce86bc1227cf55`. See the
[raw runs and report](../evidence/large-sparse/development-machine).

The live device query reports `maxStorageBufferRange = 4,294,967,295` bytes.
The dense 32-bit payload needs 4,294,967,296 bytes, one byte over
that limit before headers and materials.

| Scene-budget component | Bytes |
| --- | ---: |
| Initial scene allocation | 93,585,504 |
| Initial upload staging | 93,585,492 |
| Initial combined peak | 187,170,996 |
| Ordinary edit combined peak | 93,586,532 |
| Growth visible and candidate allocations together | 229,769,408 |
| Growth upload staging | 136,183,892 |
| Growth predicted peak | 365,953,300 |
| Growth measured peak, including staging | 365,953,300 |
| Budget | 1,073,741,824 |

The measured peak is 349.00 MiB,
34.08% of budget.
All three runs agree on those byte counts. Growth's allocation-based prediction
matches its measured peak. Empty-to-mixed and uniform-to-mixed edits upload
1,028 bytes; mixed-to-empty and mixed-to-uniform upload 4 bytes.

Validation allocates 26,156,544 requested buffer bytes in total,
with 26,024,448 bytes simultaneously reserved at its peak.
These figures are separate from storage construction and identical in all three runs.
Initial publication peaks at 408,910,924
requested heap bytes; preparation peaks at
409,566,848 incremental bytes.
Enumeration's replay heap peak is 6,360 bytes, with
4,096 output bytes and 216 retained working-state bytes.
The replay supplies memory observations only. The `enumeration` timing below
comes from the actual first preparation.
Growth's incremental preparation-and-upload bound is
584,946,294 to 585,060,982 bytes.

The observed owner rejection reports 8,468 predicted bytes against a
8,444-byte budget. Its typed failure supplies the JSON record attached
to each run; the script does not invent these values. Retry leaves the installed
revision intact. Every run verifies 13 probes at each initial/edit revision and
14 after growth, including the five far-corner probes, and records the installed
revision. Vulkan validation reports zero warnings and errors; shutdown releases
all compute resources.

Timing distributions include all samples without trimming. Three runs establish
repetition, but their p95 equals their maximum and is not a tail-latency estimate.

| Timing | Samples | Median ms | p95 ms | Maximum ms |
| --- | ---: | ---: | ---: | ---: |
| `construction` | 3 | 1555.903 | 1987.438 | 1987.438 |
| `empty_to_mixed` | 3 | 12.417 | 13.633 | 13.633 |
| `enumeration` | 3 | 507.602 | 598.966 | 598.966 |
| `first_correct_frame` | 3 | 2836.940 | 3631.269 | 3631.269 |
| `growth` | 3 | 4613.151 | 5464.233 | 5464.233 |
| `growth_rebuild` | 3 | 3560.896 | 4307.293 | 4307.293 |
| `growth_upload` | 3 | 74.135 | 85.564 | 85.564 |
| `mixed_to_empty` | 3 | 10.279 | 14.821 | 14.821 |
| `mixed_to_uniform` | 3 | 12.039 | 13.808 | 13.808 |
| `preparation` | 3 | 2133.569 | 2670.540 | 2670.540 |
| `publication` | 3 | 346.219 | 535.074 | 535.074 |
| `serialization` | 3 | 76.515 | 92.704 | 92.704 |
| `steady_cpu` | 180 | 2.132 | 4.260 | 7.573 |
| `steady_gpu` | 180 | 1.119 | 2.714 | 4.409 |
| `uniform_to_mixed` | 3 | 8.249 | 14.610 | 14.610 |
| `upload` | 3 | 24.658 | 25.291 | 25.291 |
| `validation` | 3 | 147.885 | 232.071 | 232.071 |

## Semantic limitations

The large-scale checks use `observe_along_ray` plus independent analytical probes.
Five probes exercise the far corner at coordinates near x=z=2048: a face contact,
an x/z edge contact, a three-axis corner contact, and nearly horizontal and
vertical directions. Their known contacts identify stone at [2047,79,2047] or
grass at [2047,80,2047], with distances 1, sqrt(2), sqrt(3), or sqrt(1+2e-12).
Each GPU observation identifies its installed revision. Requests use batches of
at most eight probes.
They cover empty travel, occupied surface and interior, cavity walls, roof/floor
edits, and a newly occupied growth-pool cell. They do not brute-force the billion
logical values, compare every screen pixel, or prove correctness for every ray.
The existing large-terrain GPU test and runner share the same analytical fixtures.
The compute owner now reports the actual enumeration, construction, and
serialization phases of first preparation.

## Implementation checks

`cargo test --workspace --all-targets --all-features` passed, with the explicitly
ignored GPU and timing tests left opt-in. The script above runs the large sparse
GPU qualification explicitly. The ten `measurement-evidence` large sparse
contract tests pass, including budget boundaries, contradictory records, exact
device limits, incomplete runs, JSON Lines shapes, and clean shutdown requirements.
`cargo fmt --all` and `cargo clippy --workspace --all-targets --all-features` are clean.
The implementation and re-recorded evidence are reviewed against the four
follow-up tickets and repository coding standards.
