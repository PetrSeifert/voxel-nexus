# Localized raster CPU geometry sharing

Issue: [#82](https://github.com/PetrSeifert/voxel-nexus/issues/82).

`RasterRegionResult` owns its immutable vertices, indices, and semantic faces through one shared `Arc<RasterGeometry>`. Candidate assembly clones region metadata and shared references. It sums buffer lengths without walking geometry. The normal rendering path retains no flattened arrays. Semantic qualification iterates region faces, and evidence counts use stored byte totals. Inspection code can request a separate, fallible `flatten_geometry()` result with rebased indices.

## Measurement

Measured on Windows, AMD Ryzen 5 7600, with `rustc 1.99.0-nightly (14cae6813 2026-07-08)`, using:

```text
cargo test --locked -p raster-render-path --release --lib localized_installation_shares_geometry -- --nocapture
```

The test holds eight 16×16×16 Raster Regions constant. Seven contain checkerboard geometry in cubes of side 4, 8, or 16. One voxel at coordinate 2,2,2 in the remaining region alternates between empty and occupied. Every edit affects exactly one region. Each scale has one warmup and twenty measured installations.

The measured interval starts after background preparation reports ready and includes candidate assembly and frame-boundary commit. A thread-local allocator counter records requested allocation sizes, including replacement allocations from reallocations. It excludes worker allocation, setup, and assertions. There is no Vulkan device or GPU upload in this measurement. The reported time is CPU installation cost, not total frame time.

| Faces after the final edit | Geometry payload bytes | Installation allocation bytes | Geometry bytes copied during installation | Median time | Min–max time |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1,350 | 313,200 | 2,876 | 0 | 3.2 µs | 2.3–5.7 µs |
| 10,758 | 2,495,856 | 2,876 | 0 | 2.8 µs | 2.0–6.3 µs |
| 86,022 | 19,957,104 | 2,876 | 0 | 2.7 µs | 1.7–6.6 µs |

Geometry payload bytes count vertex and index buffers plus `size_of::<SemanticFace>()` per face. They exclude allocations owned by identifiers inside semantic faces and allocator overhead. Zero geometry copies are established by `Arc::ptr_eq` checks against the prepared replacement and all seven prior unchanged regions after every commit. Allocation totals are measured, not estimated. The test asserts that they remain equal across measured edits and scales; timings are reported without a pass/fail threshold.

This isolates growth in geometry while holding region bookkeeping constant. Installation still allocates region metadata, and its searches and bookkeeping can grow with the number of regions. These results do not establish constant cost as region count grows or quantify a before/after speedup.

## Regression coverage

The allocation test verifies shared geometry across adjacent installed revisions, including occupied-to-empty replacements. The explicit flattening test spans two nonempty regions separated by an empty region and checks vertex order, global index rebasing, face order, and face-to-quad lookup. Existing lifecycle tests cover atomic Visible revision changes, newest-only candidate gates, upload failures, and unaffected GPU resource identity. Existing semantic qualification and evidence consumers continue to compile against region iteration and allocation-free counts.
