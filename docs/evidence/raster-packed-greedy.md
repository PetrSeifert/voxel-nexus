# Packed raster vertices and greedy meshing

Issue: [#94](https://github.com/PetrSeifert/voxel-nexus/issues/94).

Raster vertices now occupy 8 bytes instead of 40. Three unsigned 16-bit components hold region-local integer positions. The remaining 16 bits hold a three-bit outward normal index and a 13-bit region material index. Each nonempty Raster Region owns a GPU storage buffer of linear material colors. Its vertex shader resolves colors from that table and transforms local coordinates using volume metadata and the integer region origin.

Greedy meshing merges rectangular coplanar faces with the same Voxel Material identity within each Raster Region. Equal colors do not make different materials merge. Halo reads still suppress occupied neighbor faces across region boundaries. Semantic voxel faces remain individually available, with a mapping to their merged quads. Explicit inspection decodes packed vertices to scene-space positions and colors; normal rendering retains only packed geometry.

Packed coordinates accept endpoints through 65,535, and a region can reference up to 8,192 materials. Larger values return a contextual geometry error instead of truncating. The region material table is separate from vertex data. This change does not add a frontend material-edit API.

## Before and after

Measured on Windows, AMD Ryzen 5 7600, with `rustc 1.99.0-nightly (14cae6813 2026-07-08)`. Baseline code was commit `05821842d3e70d1975c6c1b71d0916f061ff989c`, with only the timing test extended to print byte counts and include Medium. After measurements used this issue's uncommitted implementation on the same machine.

```text
cargo test --locked -p raster-render-path --release --test region_read_timing -- --ignored --nocapture
```

Each scale uses the canonical scene and 16x16x16 Raster Regions. Scene generation and publication occur before timing. Each run performs three warmups and fifteen measured complete derivations. The timed interval includes region reads, face extraction, geometry construction and artifact assembly, but excludes destruction of the returned artifact. The table reports medians and observed ranges. No other build, test or demo was running during these measurements.

| Scene dimensions | Vertex bytes before | Vertex bytes after | Index bytes before | Index bytes after | Median before | Median after |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 64x32x64 | 2,178,560 | 5,376 | 326,784 | 4,032 | 7.0605 ms | 6.5938 ms |
| 128x64x128 | 8,714,240 | 10,432 | 1,307,136 | 7,824 | 47.2430 ms | 41.1679 ms |
| 256x128x256 | 34,856,960 | 30,080 | 5,228,544 | 22,560 | 452.4465 ms | 370.8045 ms |

| Scene scale | Before min-max | After min-max |
| --- | ---: | ---: |
| Small | 5.3915-8.0989 ms | 5.9934-9.8945 ms |
| Medium | 41.9071-53.4055 ms | 39.9799-42.7497 ms |
| Large | 432.7231-490.1091 ms | 356.0272-394.3793 ms |

Byte counts describe vertex and index payloads only. They exclude material tables, retained semantic faces, face-to-quad mappings, Vulkan allocations and allocator overhead. Runtime GPU resource accounting includes material-table buffers. The canonical scene has large uniform surfaces, so these reductions do not predict results for highly fragmented or mixed-material scenes. These are CPU derivation observations, not frame-time or GPU performance measurements.

## Verification

The raster test suite passes semantic face qualification, framebuffer front-face winding, localized replacement, failure/retry gates and unaffected geometry sharing. New tests check a solid box reduces to six quads, equal-color material boundaries remain separate, region boundaries stop merging, nonzero region origins decode correctly, and packed coordinate/material limits reject overflow. All 256 occupancy patterns in a 2x2x2 volume reproduce exactly the semantic surface without overlapping quads and with outward winding. Canonical-scale tests keep their semantic-face expectations while allowing fewer rendered quads.

A native validation-enabled run on NVIDIA GeForce RTX 4070, Vulkan 1.4.329, driver 2497200128, completed three raster/compute switches and the revision 1 to 4 edit sequence. It recorded 15 passing semantic observations, zero obsolete presented frames, zero validation warnings/errors, and no owned raster resources after shutdown. The run used `desktop-demo --scene-scale 64 --portable-compute-ray-milestone-demo`, driven by a temporary copy of `scripts/verify-portable-compute-ray-milestone.ps1`. That copy matched the current `voxel-nexus.raster` and `voxel-nexus.compute-ray` window-title names, omitted video/screenshots and the transient initial raster-title wait, and retained the scenario's semantic, revision, resource and validation assertions. No visual comparison is claimed.
