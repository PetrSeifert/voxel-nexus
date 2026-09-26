# Shared storage across scene revisions

Issue #81 replaces full voxel-array clones with private, reference-counted pages in `DenseStorage`. Each page holds at most 256 values. An edit clones the edited volume's page table and one page. Other pages and other volumes' page tables remain shared. Material and volume metadata catalogues are also shared. The scene's volume lookup map is still cloned.

The frontend still validates and publishes commands under the same write lock. Revision advancement, no-op detection, and change-set construction are unchanged. Storage pages do not appear in the public API or determine the logical changed region.

## Reproduce

```text
cargo run --release -p voxel-frontend --example edit_storage_measurement
cargo test -p voxel-frontend
```

The measurement executable uses only the public frontend API. To compare the old implementation, copy `crates/voxel-frontend` into a temporary standalone crate, replace its library source with `git show f4765bc46c75c1e57ff245a0a6aabdd0b23b4c0e:crates/voxel-frontend/src/voxel_frontend.rs`, add an empty `[workspace]` table to its manifest, and copy the repository lockfile. Run the same example in release mode.

Each case publishes one or two fully occupied cubic volumes with one material. Four untimed edits precede 31 timed edits to distinct coordinates spread across the edited volume. Retention cases keep the four preceding views alive during every sample. Other cases retain no preceding views. Each sample changes an occupied value to empty in the first volume.

A counting system allocator measures successful allocation calls and requested bytes only during `edit`. Time covers that call, including destruction of an unretained predecessor, but excludes scene construction, command construction, outcome destruction, and removal of older retained views. Allocated bytes are cumulative requested bytes, not peak memory, copied bytes, or allocator overhead. The counters themselves add timing overhead. Median and nearest-rank p95 summarize 31 samples; these are local observations, not latency guarantees.

## Copying and remaining costs

The private sharing test compares page identities across revisions and counts the values in replaced pages. It verifies 256 copied values for full pages and one for the partial final page, while all other page identities and the unrelated volume's page table stay unchanged. It also checks every retained revision's values, repeated edits within a page, and unchanged publication identity.

On this 64-bit build, `VoxelValue` occupies 16 bytes. A full-page edit therefore clones 4,096 bytes of voxel payload. The old implementation cloned `volume_count * edge^3 * 16` bytes of voxel payload per command. These are layout calculations supported by the sharing test, not hardware memory-traffic measurements. Material identities within occupied values are reference counted, so cloning a page also updates their reference counts.

The edited volume's flat page table still costs `ceil(edge^3 / 256) * 8` bytes to clone on this build. That is 1,024, 8,192, 65,536, and 524,288 bytes for edges 32, 64, 128, and 256. Table cloning and eventual table destruction visit every page reference. Edit cost thus still grows with the edited volume's page count and the scene's volume count, but does not clone voxel payload from unchanged pages or unrelated volumes. Retained revisions also retain these copied tables. A deeper persistent table could reduce that metadata cost if later measurements justify the complexity.

The 256-value partition limits voxel copying to 4 KiB while keeping the table to one pointer per 256 values. This measurement evaluates that choice against the original flat array; it does not establish an optimal page size. Initial dense publication still validates all values and allocates all pages, and the example does not measure publication or region-read throughput.

## Measurements

Measured on 2026-09-26, Windows x86_64, AMD Ryzen 5 7600, rustc 1.99.0-nightly `14cae6813`, release profile. Baseline source is commit `f4765bc46c75c1e57ff245a0a6aabdd0b23b4c0e`. Both builds used the same example and dependency versions from the repository lockfile. Runs were sequential after workspace tests and Clippy finished.

Allocation results were identical with zero or four retained views. Each row reports per-command totals.

| Edge | Volumes | Old allocations | Shared allocations | Old allocated bytes | Shared allocated bytes |
| --- | --- | --- | --- | --- | --- |
| 32 | 1 | 6 | 7 | 524788 | 5540 |
| 32 | 2 | 7 | 7 | 1049124 | 5540 |
| 64 | 1 | 6 | 7 | 4194804 | 12708 |
| 64 | 2 | 7 | 7 | 8389156 | 12708 |
| 128 | 1 | 6 | 7 | 33554932 | 70052 |
| 128 | 2 | 7 | 7 | 67109412 | 70052 |
| 256 | 1 | 6 | 7 | 268435956 | 528804 |
| 256 | 2 | 7 | 7 | 536871460 | 528804 |

Latency is in microseconds. Retention excludes releasing the oldest view from the timed call, so lower times with retained views do not imply lower total lifecycle cost.

| Edge | Volumes | Retained views | Old median | Shared median | Old p95 | Shared p95 |
| --- | --- | --- | --- | --- | --- | --- |
| 32 | 1 | 0 | 133.400 | 3.200 | 167.600 | 4.100 |
| 32 | 1 | 4 | 92.000 | 1.400 | 146.000 | 2.100 |
| 32 | 2 | 0 | 331.100 | 1.800 | 434.600 | 2.100 |
| 32 | 2 | 4 | 255.600 | 1.100 | 310.900 | 1.400 |
| 64 | 1 | 0 | 1468.800 | 6.200 | 1726.800 | 7.000 |
| 64 | 1 | 4 | 916.200 | 4.100 | 1073.500 | 5.300 |
| 64 | 2 | 0 | 3031.000 | 7.800 | 3452.300 | 8.000 |
| 64 | 2 | 4 | 1861.800 | 3.400 | 2222.100 | 3.600 |
| 128 | 1 | 0 | 12262.600 | 37.300 | 13075.300 | 48.900 |
| 128 | 1 | 4 | 7584.500 | 19.600 | 8297.300 | 21.300 |
| 128 | 2 | 0 | 24840.400 | 37.800 | 26311.500 | 42.300 |
| 128 | 2 | 4 | 15350.000 | 19.600 | 16963.300 | 20.200 |
| 256 | 1 | 0 | 101751.200 | 298.700 | 108247.200 | 355.900 |
| 256 | 1 | 4 | 62895.400 | 154.900 | 65471.200 | 211.700 |
| 256 | 2 | 0 | 203818.200 | 299.000 | 213417.000 | 399.500 |
| 256 | 2 | 4 | 131443.500 | 155.600 | 148462.400 | 206.200 |

With two 256-cubed volumes and four retained views, allocations fell from 536,871,460 to 528,804 bytes per edit; median edit latency fell from 131.444 ms to 0.156 ms in this run. Adding the second volume did not increase allocated bytes in the shared implementation for these map sizes. Allocation count stayed at seven for that case; the improvement is in allocation size and payload cloning, not fewer allocation calls.

Validation passed: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all --check`, and the focused frontend tests after the final sharing assertions were added.
