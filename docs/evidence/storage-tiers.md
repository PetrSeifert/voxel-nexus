# Storage Tier measurements for issue #87

Measured on the development Windows machine on 2026-09-26 using:

```text
cargo test --release -p compute-ray-render-path --test storage_tiers storage_costs -- --ignored --nocapture
```

| Canonical scale | Tier | Estimated storage bytes | Full dense read median | Edit median |
| --- | --- | ---: | ---: | ---: |
| Small, 64 × 32 × 64 | Dense | 548,928 | 400.6 µs | 2.8 µs |
| Small | SparsePages | 369,248 | 1.1528 ms | 4.4 µs |
| Medium, 128 × 64 × 128 | Dense | 4,390,976 | 3.449 ms | 18.3 µs |
| Medium | SparsePages | 2,757,824 | 12.1339 ms | 26.5 µs |
| Large, 256 × 128 × 256 | Dense | 35,127,360 | 27.7734 ms | 148 µs |
| Large | SparsePages | 22,062,144 | 123.3687 ms | 246.3 µs |

Each read fills a reused, full-volume logical-value buffer. The median is over nine reads. Each edit alternates the origin voxel between empty and the warm material, with 101 edits and the initial view retained. Timings include frontend locking, revision publication and change-set creation. They exclude scene generation and publication. These are local observations, not performance thresholds.

Memory is an estimate of one view's storage representation. It includes page payloads, page containers and reference-count headers. Sparse map nodes use a documented per-entry estimate because Rust's BTreeMap does not expose allocated node capacity. Allocator overhead, scene metadata, logical-value read buffers, publication staging and additional retained revisions are excluded. Shared allocations are counted once within that view, not apportioned between retained views.

Sparse pages save about 33–37% of estimated storage in these scenes, at a cost to reads and edits. Missing pages mean empty values, uniform pages hold one material index, and mixed pages hold at most 256 indices. An edit shares unchanged mixed-page payloads and copies the page directory and edited payload. Dense remains the default.

Select a tier for one volume with `DenseVoxelVolume::with_storage_tier`, or apply a default to every volume in an input scene with `DenseVoxelScene::with_storage_tier`. The dense names describe the publication batches; both tiers consume the same validated input. Sparse publication currently stages a dense representation, so it does not reduce peak publication memory or permit incomplete sparse input.

The internal Storage interface owns point lookup, dense region reads, region classification and structurally shared successors. The frontend validates requests and translates scene-local material indices. `VoxelSceneView::region_content` returns `Uniform`, including empty regions, or `Mixed`, with out-of-volume coordinates treated as empty. Sparse classification checks uniform or absent page spans without reading each voxel. Dense region reads consume this classification and fill uniform buffers without point lookups. Render Paths still use logical frontend access; no native storage fast path is exposed.

The shared frontend contract tests exercise both tiers. The canonical comparison test covers all three scales before and after an edit, comparing complete compute voxel/material arrays, all raster semantic faces, and declared Semantic Ray probes against the oracle. This is CPU representation and semantic evidence, not a GPU screenshot comparison.
