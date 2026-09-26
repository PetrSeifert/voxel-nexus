# Atomic Voxel Edit Commands

Issue [#88](https://github.com/PetrSeifert/voxel-nexus/issues/88).

`VoxelEditCommand::new` retains the single-coordinate constructor. Use
`VoxelEditCommand::from_edits(Vec<VoxelEdit>)` for an atomic command across coordinates
and volumes. Every entry is validated before storage changes. The last entry for a
repeated coordinate wins, including when it restores the original value. Empty
commands and commands with no final value changes preserve the current publication.

Both storage tiers use a persistent page tree with at most 64 entries per node.
An edit copies only nodes on the affected paths and changed value pages. The tree
depth grows logarithmically with the page count and is bounded by the address width.
The 32,768-page test copies 136 table entries for one changed page. Existing retained
views keep their original values and share untouched pages.

## Brush measurement

Measured on 2026-09-26, Windows, AMD Ryzen 5 7600, using
`rustc 1.99.0-nightly (14cae6813 2026-07-08)` and the release profile.
The fixture is an initially empty 256 × 128 × 256 volume. A 10 × 10 × 10 brush
fills 1,000 distinct coordinates with one material. Raster regions are 16³.
The initial view and raster artifact remain retained throughout the edits.

Each row is one observed run, not a statistical performance guarantee. Worker
scheduling affects the preparation counts and elapsed times. Sparse baseline
storage is initially empty, so it does not exercise a large populated sparse map.

| Implementation | Tier | Revisions | Preparation starts | Restarts | Edit time, ms | Submission time, ms |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Before, 1,000 single commands | Dense | 1,000 | 783 | 782 | 82.688 | 179.089 |
| Before, 1,000 single commands | Sparse | 1,000 | 81 | 80 | 0.755 | 4.012 |
| After, 1,000 single commands | Dense | 1,000 | 107 | 106 | 1.995 | 8.081 |
| After, 1,000 single commands | Sparse | 1,000 | 63 | 62 | 0.844 | 3.731 |
| After, one atomic command | Dense | 1 | 1 | 0 | 0.301 | 0.790 |
| After, one atomic command | Sparse | 1 | 1 | 0 | 0.357 | 0.875 |

Edit time sums time inside `VoxelFrontend::edit`, excluding command construction.
Submission time includes editing, submitting outcomes to `RasterConvergence`, and
draining events. Preparation starts count actual `PreparationStarted` events through
readiness of the final revision. Restarts are starts after the first. The worker
coalesces pending submissions, so 999 superseding requirements need not cause 999
worker restarts. This measures CPU preparation, not GPU upload or presentation.

The before run used the frontend at `629a1325438cc3c1e9baf88ef3a2e628c923fa20`
and the same fixture, with 1,000 calls to the existing single-coordinate constructor.
The retained example reproduces the after cases:

```text
cargo run --release -p raster-render-path --example brush_edit
cargo run --release -p raster-render-path --example brush_edit -- --single
```

## Verification

- Frontend tests cover mixed-tier multi-volume edits, complete change sets, retained
  views, invalid entries including overwritten entries, empty commands, duplicate
  coordinates, net no-ops, and checked revision overflow.
- Page-tree tests count copied entries at 64, 4,096, and 32,768 pages and check that
  mutation and removal leave the predecessor unchanged.
- Raster tests require one preparation and compare localized multi-coordinate
  replacement across region boundaries with complete derivation.
- Compute convergence installs one successor bundle and compares all voxel words
  with a complete rebuild of the resulting view.
