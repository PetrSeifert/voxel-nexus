# Windows milestone completion evidence

The completion bundle is runtime proof for one recorded Windows development machine. It does not establish portable runtime behavior or performance thresholds.

From a clean committed checkout with the Vulkan SDK, validation layer, `ffmpeg`, and `ffprobe` available, run:

```powershell
pwsh -NoProfile -File scripts/capture-milestone-completion.ps1 -EvidenceDirectory docs/evidence/milestone-completion/development-machine
```

The collector first records the timing evidence while the checkout is still clean. It then captures generated-artifact build, formatting, strict Clippy, workspace tests, Voxel Frontend read tests, semantic surface diagnostics, deterministic failures, prerequisite regressions, and the validation-enabled Windows lifecycle proof.

The lifecycle proof records one uninterrupted H.264 clip inside a controlled black-backed screen region. Its event timeline covers paused background preparation, lifecycle events, the first matching-revision frame, all fixed poses, the deterministic camera move, and clean close. The collector verifies the clip with `ffprobe`, decodes it fully with `ffmpeg`, and extracts representative frames for inspection.

Verify the retained manifest and every inventoried SHA-256 hash with:

```powershell
cargo run --locked --package completion-evidence --bin verify-completion-evidence -- docs/evidence/milestone-completion/development-machine
```

The verifier rejects portable runtime claims, unsafe or duplicate paths, missing required evidence categories, incomplete timing streams, invalid or interrupted video metadata, nonzero validation counts, changed byte sizes, and changed hashes.
