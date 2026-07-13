# Localized editable raster evidence, schema v1

This retained bundle assembles the completed uninterrupted edit-burst demonstration, representative frames, semantic and localization qualifications, orchestration timelines, failure and shutdown logs, raw Raster Region measurements, extent selection, scale comparison, and Vulkan validation output for one recorded Windows development machine.

The runtime and measurements are descriptive for the recorded machine only. They are not portable performance targets.

From a clean checkout with the Vulkan SDK and `VK_LAYER_KHRONOS_validation` available, reproduce the three source evidence sets in order:

```powershell
pwsh -NoProfile -File scripts/verify-edit-burst-demo.ps1 -EvidenceDirectory artifacts/edit-burst-issue-46
pwsh -NoProfile -File scripts/qualify-raster-region-extents.ps1 -EvidenceDirectory artifacts/raster-region-extent-selection-issue-47
pwsh -NoProfile -File scripts/characterize-raster-region-scales.ps1 -EvidenceDirectory artifacts/raster-region-scale-characterization-issue-48 -SelectionManifest artifacts/raster-region-extent-selection-issue-47/manifest.json
pwsh -NoProfile -File scripts/assemble-localized-raster-evidence.ps1
```

Verify every retained artifact, cross-check the nested source manifests, and enforce all correctness gates:

```powershell
cargo run --locked --package localized-raster-evidence --bin verify-localized-raster-evidence -- docs/evidence/localized-editable-raster/v1/development-machine
```

The top-level `manifest.json` records the source repository revisions independently because the completed evidence was intentionally reused rather than remeasured. `comparison.svg` is derived deterministically from `scale-characterization/raw-distributions.json` during assembly.
