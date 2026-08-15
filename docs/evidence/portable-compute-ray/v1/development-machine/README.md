# Portable compute-ray milestone evidence

This bundle records one validation-enabled run of the portable compute-ray milestone on the attributed development machine. The clip is uninterrupted. It covers the held first switch, camera and presentation changes, newest-only convergence, the raster round trip, and final compute-ray shutdown.

The timing and resource files come from a second run with Vulkan validation disabled and immediate presentation. They are descriptive machine-local values. The bundle makes no Render Path superiority or cross-machine claim.

Reproduce the bundle from a clean committed checkout:

```powershell
pwsh -NoProfile -File scripts/capture-portable-compute-ray-evidence.ps1 -EvidenceDirectory docs/evidence/portable-compute-ray/v1/reproduction
```

Verify the manifest and every inventoried hash:

```powershell
cargo run --locked --package portable-compute-ray-evidence --bin verify-portable-compute-ray-evidence -- docs/evidence/portable-compute-ray/v1/development-machine
```