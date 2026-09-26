# Dense region reads, issue #86

Measured on Windows with an AMD Ryzen 5 7600 and rustc 1.99.0-nightly (14cae6813 2026-07-08), using the release profile. The baseline is commit `d7d307fae0ae2b6dd35cb9bb700c43ad0c9b44bc` plus the two timing tests added by this change.

Run from the repository root:

```powershell
cargo test --locked --release -p raster-render-path -p compute-ray-render-path --test region_read_timing -- --ignored --nocapture
```

Each operation has three warmups followed by 15 samples. The reported value is the median. Scene generation and publication happen before timing. Timing includes result construction and excludes result destruction. Raster measures `derive_raster_regions` over the whole scene using 16 x 16 x 16 Raster Regions. Compute measures `ComputeSceneBundle::from_view`, including material tables, voxel words, and packed storage words. These are CPU preparation measurements, not frame times or isolated frontend reads.

The canonical Small scene has dimensions 64 x 32 x 64. Large has dimensions 256 x 128 x 256. Both runs use the same scene generator, published view, and timing code. These local measurements do not establish performance on other machines.

| Operation | Scene | Before median, ms | After median, ms | Reduction |
| --- | --- | ---: | ---: | ---: |
| Raster region derivation | Small | 21.8900 | 3.9830 | 81.8% |
| Raster region derivation | Large | 858.3264 | 199.3531 | 76.8% |
| Compute preparation | Small | 3.9629 | 1.6256 | 59.0% |
| Compute preparation | Large | 199.7321 | 104.1194 | 47.9% |

The final after run had no concurrent build or test workload. An earlier after run overlapped workspace tests and is excluded from this table.

Validation passed:

- `cargo test --locked --workspace`, including doctests, Semantic Ray observations, raster semantic/localization/retry qualifications, and compute agreement with the oracle.
- `cargo clippy --locked --workspace --all-targets -- -D warnings`.
- `cargo fmt --all -- --check`.
- Both ignored timing tests with the command above.

New regression tests cover non-cubic dense ordering, partial overlap on both sides of the volume, extreme out-of-volume coordinates, reuse of populated buffers, typed size errors, unchanged buffers on invalid requests, and compute row placement across all three block boundaries and multiple volumes. Existing qualification assertions were preserved. GPU presentation was not re-recorded for this change.
