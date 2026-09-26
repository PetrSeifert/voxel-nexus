# Raster worker pool measurements

Issue [#95](https://github.com/PetrSeifert/voxel-nexus/issues/95), measured on 2026-09-26. The baseline is commit `70d9b09f41bd9f770146ad12cc6f94af653b0fa0`; the after sample uses the implementation committed with this evidence. [measurements.json](measurements.json) preserves the first-frame events and edit-burst measurements extracted from the application and verification script.

Windows development machine, AMD Ryzen 5 7600 with 12 logical processors, NVIDIA GeForce RTX 4070. Both builds used the dev profile, the qualification feature, and Vulkan validation. The new pool used eight region workers and one coordinator. Each convergence instance retains its pool between generations and joins its threads during shutdown or drop. Initial preparation releases its separate pool on completion or cancellation.

| Measurement | Sequential baseline | Worker pool |
| --- | ---: | ---: |
| Publication to first correct frame, ms | 1338.7892 | 588.9259 |
| Publication to derived artifact, ms | 1328.6709 | 578.1705 |
| Edit keypress to final visible revision, ms | 95.2462 | 104.3955 |
| Summed edit region derivation time, ms | 31.9264 | 34.9884 |

These are single samples on one machine, with no statistical speedup claim. First-frame time improved in this sample; the small edit burst was slower. The burst covers seven scheduled regions across three generations and includes scripted barrier release and frame scheduling. Summed derivation time adds individual region durations and is not parallel wall-clock time.

Both edit runs scheduled one region before cancelling revision 2, rejected revision 3 after upload, and first made revision 4 visible. Neither showed an intermediate revision. Both finished with zero Vulkan validation warnings or errors. The script reported successful shutdown and resource retirement. An armed CPU qualification barrier deliberately dispatches one region at a time; later generations use parallel batches after the barrier has fired.

## Reproduction

Build each revision with `cargo build --locked --features qualification --package desktop-demo`. Run measurements without concurrent tests or builds. Create the output directory first. Substitute `before` or `after` for `<phase>`.

```powershell
./target/debug/desktop-demo.exe --scene-scale 128 --measurement-mode first-correct-frame --measurement-output artifacts/issue-95/<phase>/first-frame.jsonl
pwsh -NoProfile -File scripts/verify-edit-burst-demo.ps1 -EvidenceDirectory artifacts/issue-95/<phase>/edit -RasterRegionExtent 16 -SceneScale 128 -TimingOnly -SkipBuild
```

First-frame measurement uses the default region extent of 32; edit measurement explicitly uses 16. Each before/after pair uses the same configuration. Raw application logs and script manifests remain in `artifacts/issue-95/` in the measurement workspace.
