# Windows lifecycle verification

These instructions reproduce the Vulkan desktop lifecycle proof on a Windows machine. Runtime execution is claimed only for the Windows development machine recorded in the checked-in evidence; the code remains architecturally portable, but this proof makes no Linux or macOS runtime claim.

## Clean-checkout prerequisites

- Windows 10 or 11 with a graphics driver exposing Vulkan 1.3 and window presentation support.
- Git and a Rust toolchain with Cargo.
- The Vulkan SDK with `VK_LAYER_KHRONOS_validation` installed and `VULKAN_SDK` set. The demo deliberately refuses to start without this validation artifact.
- A native C/C++ build toolchain and CMake for the `shaderc` build dependency.
- PowerShell 7 (`pwsh`) and an interactive desktop session. The lifecycle runner captures the demo client area and cannot run in a headless session.

The repository does not depend on precompiled shader files. `crates/render-backend/build.rs` compiles `triangle.vert` and `triangle.frag` into Vulkan 1.3 SPIR-V inside Cargo's build output. A clean checkout therefore needs no untracked shader artifact.

From a fresh checkout, run:

```powershell
git clone https://github.com/PetrSeifert/voxel-nexus.git
Set-Location voxel-nexus
cargo build --locked --package desktop-demo
pwsh -NoProfile -File scripts/verify-windows-lifecycle.ps1 -EvidenceDirectory artifacts/windows-lifecycle
```

The explicit build command proves the clean checkout can produce the executable and generated shaders. The runner repeats that locked build, runs the unsupported-prerequisite integration tests, and then performs the runtime proof.

## What the runner proves

One validation-enabled `desktop-demo.exe` process is kept alive while the runner:

1. captures two materially identical launch frames containing both the triangle and clear background;
2. resizes to landscape and portrait extents and repeats the stable paired-frame check at each extent;
3. minimizes the window and verifies its minimized state;
4. restores the same process and repeats the stable paired-frame check;
5. sends a normal window-close request and requires exit code 0 within ten seconds; and
6. requires zero Vulkan validation warnings and zero Vulkan validation errors in the complete stderr log.

The evidence directory contains a JSON manifest with the Git revision, build profile, Windows version, toolchain, validation context, lifecycle sequence, client extents, pixel counts, capture hashes, process result, and unsupported-case results. Raw stdout records the device, driver version, and Vulkan API version. Raw stderr is retained as the complete validation log, even when empty. PNG pairs make triangle presentation at each visible state auditable.

## Unsupported prerequisites

The lifecycle runner separately invokes these deterministic diagnostics:

```powershell
target\debug\desktop-demo.exe --verify-unsupported-prerequisite vulkan-1.2
target\debug\desktop-demo.exe --verify-unsupported-prerequisite presentation
```

Both commands must exit with code 1 promptly, without a panic. Their stderr logs retain the actionable Vulkan-version or presentation-capability error. These injected cases prove the application boundary independently of the development machine's supported Vulkan device.

## Checked-in development-machine evidence

The repository evidence under `docs/evidence/windows-lifecycle/` was produced by this runner from a clean checkout of the revision named in its manifest. Treat it as a record of that machine and run, not as a claim about other machines or operating systems.
