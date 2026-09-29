# Large sparse terrain publication

Implements [issue #103](https://github.com/PetrSeifert/voxel-nexus/issues/103).
The reusable generator is `canonical_scene::generate_large_terrain`. Consume its
result with `into_scene()` and publish with `VoxelFrontend::publish_sparse`.
It selects `SparsePages` explicitly. The desktop demo does not use it yet.

## Content and determinism

One Voxel Volume has extent 2048×256×2048, origin [0, 0, 0], and voxel size 1.
At local horizontal coordinate x,z, its inclusive surface height is
`80 + (min(x, 2047-x) + min(z, 2047-z)) / 64`, using integer division.
The surface is grass, its interior is stone, and everything above is empty.
An enclosed cavity occupies half-open bounds [768,1024) × [32,64) × [768,1024).

Generation visits 32×32 horizontal tiles in z-major, then x-major order. Each
tile supplies a stone fill below its minimum surface height and one dense detail
batch from minimum through maximum surface height inclusive. Cavity tiles split
their fill at y=32 and y=64, leaving the cavity absent. There are no overrides or
overlapping batch boxes. No full-volume dense array is constructed.

The content fingerprint is FNV-1a over the ordered sparse batch stream, starting
at `cbf29ce484222325` with prime `100000001b3`. Each batch contributes one kind
byte, 0 for fill or 1 for detail, followed by origin x,y,z and extent x,y,z as six
little-endian i32 values. A fill contributes its material code once; detail
contributes one code per value in x-fastest, then y, then z order. Codes are
0 for empty, 1 for stone, and 2 for grass. This fingerprint includes partitioning
and content, and is not a cryptographic hash or a partition-independent hash.
All three independent runs produced `58ce86bc1227cf55`.

`cargo test -p canonical-scene --test large_terrain` checks repeated generation,
successful overlap-validated publication, horizontal sweep selection, bounded
staging and storage, uniform base, empty upper half, mixed surface, cavity floor,
roof and walls, and analytically known surface values at both far corners and
the central ridge. Existing frontend overlap tests cover rejection semantics.

## Measurements

Run `cargo run -p canonical-scene --example large_terrain_publication --release`.
Measured 2026-09-29 on Windows x64, AMD Ryzen 5 7600, Rust
`1.99.0-nightly (14cae6813 2026-07-08)`. Three fresh processes used the same release
binary. Times are descriptive and include allocation counter overhead.

| Measurement | Value in every run |
| --- | ---: |
| Fill batches | 4,160 |
| Dense detail batches | 4,096 |
| Total batches | 8,256 |
| Dense detail values | 6,291,456 |
| Selected sweep axis | X |
| Candidate pairs examined | 528,608 |
| Publication staged values, cumulative | 69,206,016 |
| Sparse-tier `storage_bytes` | 278,152,864 |
| Input requested heap bytes | 101,059,920 |
| Retained requested heap bytes | 277,887,896 |
| Publication peak requested heap bytes | 408,910,908 |

| Run | Generation ms | Publication ms | Validation ms | Peak resident working set bytes |
| --- | ---: | ---: | ---: | ---: |
| 1 | 39.740 | 264.087 | 112.417 | 430,460,928 |
| 2 | 35.859 | 262.624 | 114.554 | 429,953,024 |
| 3 | 35.461 | 260.791 | 114.678 | 429,953,024 |

Publication time includes validation, sparse storage construction, and input
destruction. Validation time covers sparse batch bounds, material resolution,
detail validation, sweep selection and exact intersection tests. It excludes
scene catalogue validation. Generation time includes fingerprinting.

The example counts live requested allocations through the Rust global allocator.
Input, retained and peak bytes subtract a baseline taken after the CSV header.
The publication peak starts with the generated input resident and includes both
input and publication scratch space. Retained bytes include the frontend and
published view. `storage_bytes` is the frontend's storage estimate, so it differs
from requested heap bytes. Neither allocation measurement includes allocator
bookkeeping or other process memory.

Resident measurements use Windows `Process.PeakWorkingSet64`, sampled every
10 ms while the standalone executable runs. They cover generation and
publication, including executable pages and allocator overhead. Sampling can
miss a peak in the final interval before process exit. Reproduce with PowerShell:

```powershell
cargo build -p canonical-scene --example large_terrain_publication --release
$startInfo = [System.Diagnostics.ProcessStartInfo]::new(
    (Resolve-Path target/release/examples/large_terrain_publication.exe).Path)
$startInfo.UseShellExecute = $false
$startInfo.CreateNoWindow = $true
$startInfo.RedirectStandardOutput = $true
$process = [System.Diagnostics.Process]::Start($startInfo)
$peakResident = 0L
while (-not $process.HasExited) {
    $process.Refresh()
    $peakResident = [Math]::Max($peakResident, $process.PeakWorkingSet64)
    Start-Sleep -Milliseconds 10
}
$process.StandardOutput.ReadToEnd()
"peak_working_set_bytes=$peakResident exit_code=$($process.ExitCode)"
$process.Dispose()
```

## Cost assessment

The chosen 32×32 tiles keep the candidate count below 600,000. X wins the
validator's minimum-projection-overlap selection; Y is never selected for this
terrain. There was no candidate-count problem requiring tile merging or a
validator change. The resident peak of about 410 MiB is practical on this
development machine, so no terrain-density adjustment was needed.

Most retained storage is the 16³ mixed surface bricks. Publication allocates
69,206,016 staging values across those partial bricks, about 6.45% of the logical
volume's 1,073,741,824 values. This is bounded sparse staging, not zero staging.
The regression test allows fewer than 80 Mi staging values and 320 MiB of
reported storage. Those are fixture regression bounds, not general frontend
limits. The full logical dense payload alone would require 4,294,967,296 bytes.

## Predicted GPU footprint

Implements [issue #105](https://github.com/PetrSeifert/voxel-nexus/issues/105).
Reproduce without a GPU:

```powershell
cargo run -p compute-ray-render-path --example large_terrain_footprint --release
```

Measured 2026-09-29 on the same machine and Rust toolchain as above. The example
publishes the unchanged terrain into `SparsePages`, then builds
`BrickmapSceneBundle` through its public `from_view` interface. Its fingerprint
is `58ce86bc1227cf55`. CPU brickmap construction took 1,468.365 ms in this run,
excluding generation and publication. Timing is descriptive.

| Component | Count or bytes |
| --- | ---: |
| Coarse entries, 256 × 32 × 256 | 2,097,152 |
| Coarse grid, 4 bytes per entry | 8,388,608 |
| Mixed bricks | 66,560 |
| Occupied pool, 1,024 bytes per 8³ brick | 68,157,440 |
| Additional slots, ceil(mixed bricks / 4) | 16,640 |
| Pool capacity slots with 25% headroom | 83,200 |
| Pool capacity bytes with 25% headroom | 85,196,800 |
| Metadata and alignment allowance per representation | 65,536 |
| One representation, coarse grid + capacity + allowance | 93,650,944 |
| Visible + complete candidate representations | 187,301,888 |
| Full candidate staging allowance | 93,650,944 |
| Predicted peak including staging | 280,952,832 |
| GPU scene budget, 1 GiB | 1,073,741,824 |
| Remaining budget | 792,788,992 |

The predicted peak is 267.9375 MiB, or 26.17% of the budget. This charges two
independent coarse grids and pools, each with capacity for 25% more slots than
initially occupied, plus a
third complete padded copy for upload staging. The estimate does not rely on
sharing slots between visible and candidate representations. No content-density,
representation, scale, or budget adjustment was needed.

The GPU brickmap ABI and allocator are not implemented yet. The 64 KiB allowance
per copy is a planning assumption for scene/volume headers, the two material
colors, and buffer allocation alignment, not an observed allocation size. The
estimate covers a candidate of the same capacity as the initial terrain. It does
not bound arbitrary edits or successive 1.5× pool growth. Later GPU work must
check actual old/new capacities, staging, metadata and allocation requirements,
and measure the peak. CPU frontend storage and CPU construction scratch are not
GPU scene memory; render targets and other non-scene resources are also outside
this scene budget. This prediction is not GPU qualification evidence.

The chosen extent remains 2048×256×2048. A dense 32-bit voxel payload needs
4,294,967,296 bytes, already one byte above the development RTX 4070's recorded
`maxStorageBufferRange` of 4,294,967,295 bytes, before headers or materials. The
device limit comes from [the parent specification, issue #98](https://github.com/PetrSeifert/voxel-nexus/issues/98),
not a live device query in this CPU example. The example exits with an error if
the predicted peak exceeds the budget or the dense payload stops exceeding that
recorded limit.
