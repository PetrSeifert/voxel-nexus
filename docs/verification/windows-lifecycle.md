# Windows lifecycle verification

These instructions reproduce the Vulkan desktop lifecycle proof on a Windows machine. Runtime execution is claimed only for the Windows development machine recorded in the checked-in evidence; the code remains architecturally portable, but this proof makes no Linux or macOS runtime claim.

## Clean-checkout prerequisites

- Windows 10 or 11 with a graphics driver exposing Vulkan 1.3 and window presentation support.
- Git and a Rust toolchain with Cargo.
- The Vulkan SDK with `VK_LAYER_KHRONOS_validation` installed and `VULKAN_SDK` set. The demo deliberately refuses to start without this validation artifact.
- A native C/C++ build toolchain and CMake for the `shaderc` build dependency.
- PowerShell 7 (`pwsh`) and an interactive desktop session. The lifecycle runner captures the demo client area and cannot run in a headless session.

The repository does not depend on precompiled shader files. `crates/raster-render-path/build.rs` compiles `raster.vert` and `raster.frag` into Vulkan 1.3 SPIR-V inside Cargo's build output. A clean checkout therefore needs no untracked shader artifact.

From a fresh checkout, run:

```powershell
git clone https://github.com/PetrSeifert/voxel-nexus.git
Set-Location voxel-nexus
cargo build --locked --package desktop-demo
pwsh -NoProfile -File scripts/verify-windows-lifecycle.ps1 -EvidenceDirectory artifacts/windows-lifecycle
```

The explicit build command proves the clean checkout can produce the executable and generated shaders. The runner repeats that locked build, runs the deterministic failure integration tests, and then performs the runtime proof.

To retain the canonical overview lifecycle plus stable paired captures for the cavity/material close-up, finite-boundary cutaway, and deterministic move midpoint, add `-CaptureCanonicalInspectionSet`:

```powershell
pwsh -NoProfile -File scripts/verify-windows-lifecycle.ps1 `
    -EvidenceDirectory artifacts/canonical-scene `
    -CaptureCanonicalInspectionSet
```

## What the runner proves

One validation-enabled `desktop-demo.exe` process is kept alive while the runner:

1. waits for the real preparation worker to reach its held condition-variable barrier;
2. observes landscape and portrait resize, zero-size suspension, minimize, restore, and presentation recreation through typed event-loop state while preparation remains paused;
3. releases the worker through a dedicated desktop verification event, requires exactly one complete artifact installation for the published Voxel Scene Revision, and observes the first matching-revision frame;
4. captures stable material-colored frames through the lifecycle, presents the overview, cavity/material, and finite-boundary poses, and completes all 120 steps of the deterministic overview-to-cavity move in that same process;
5. sends a normal window-close request and requires exit code 0 within ten seconds; and
6. requires zero Vulkan validation warnings and zero Vulkan validation errors in the complete stderr log.

The application event loop waits when it has no work. Worker arrival and completion, barrier release, camera selection, and camera-move progression wake it with explicit events; no timing sleep or redraw spin is part of the synchronization protocol.

With `-CaptureCanonicalInspectionSet`, three additional validation-enabled processes render the same generated 256×128×256 Voxel Scene at the cavity/material pose, boundary-cutaway pose, and step 60 of the fixed 120-step overview-to-cavity move. Each process retains a tolerant same-run pair, exits normally, and records zero validation findings. The manifest records generator identity and version, seed, dimensions, origin, voxel size, material catalogue, occupied and exposed-face counts, surface bound, and every exact camera parameter.

The evidence directory contains a JSON manifest with the Git revision, build profile, Windows version, toolchain, validation context, lifecycle sequence, client extents, material pixel counts, capture hashes, process result, and deterministic failure results. Raw stdout records the device, driver version, Vulkan API version, and installed raster artifact revision. Raw stderr is retained as the complete validation log, even when empty. PNG pairs make material-colored, depth-correct exposed voxel faces at each visible state auditable.

## Deterministic failures

The lifecycle runner separately invokes these deterministic diagnostics:

- background derivation failure, retaining derivation phase, source revision, build phase, and injected missing-volume source context;
- a one-shot failure inside the real raster GPU upload/install path, retaining upload phase and source revision while reporting no installed revision;
- Render Path release, configure, and record failures, each retaining phase and injected source context at the desktop application boundary;
- raster artifact upload failure retaining installation phase, Voxel Scene Revision 41, and injected source context at that boundary; and
- Vulkan 1.2 and unavailable-presentation prerequisites, each retaining actionable qualification context.

```powershell
target\debug\desktop-demo.exe --verify-render-path-failure release
target\debug\desktop-demo.exe --verify-render-path-failure configure
target\debug\desktop-demo.exe --verify-render-path-failure record
target\debug\desktop-demo.exe --verify-render-path-failure upload
target\debug\desktop-demo.exe --verify-background-preparation-failure derivation
target\debug\desktop-demo.exe --scene-scale 64 --inject-raster-upload-failure
target\debug\desktop-demo.exe --verify-unsupported-prerequisite vulkan-1.2
target\debug\desktop-demo.exe --verify-unsupported-prerequisite presentation
```

Every command must exit with code 1 promptly, without a panic. Their stderr logs retain either the Render Path phase and injected source or the actionable Vulkan qualification error. These injected cases prove the application boundary independently of the development machine's supported Vulkan device.

## Checked-in development-machine evidence

The repository evidence under `docs/evidence/background-preparation/` records the held-worker lifecycle, matching-revision installation, uninterrupted camera sequence, deterministic derivation and upload failures, and prerequisite regressions from the reviewed issue #27 revision. The earlier evidence under `docs/evidence/windows-lifecycle/` records the pre-voxel lifecycle proof. Each bundle was produced by this runner from the revision named in its manifest. Treat them as records of that machine and run, not as claims about other machines or operating systems.
