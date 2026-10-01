# Voxel Nexus

Voxel Nexus is a voxel engine that keeps the Voxel Scene independent of storage and rendering. The desktop demo renders a generated scene using rasterization and a compute-ray Render Path, with fixed edit sequences for checking convergence and switching.

## Requirements

The desktop window currently runs on Windows. You need:

- A graphics driver with Vulkan 1.3 and window presentation support.
- Rust and Cargo, plus a native C/C++ build toolchain and CMake for the `shaderc` dependency.
- The Vulkan SDK with `VK_LAYER_KHRONOS_validation` installed and `VULKAN_SDK` set. Normal demo runs require the validation layer.
- PowerShell 7 for the automated verification scripts.

Cargo compiles the shaders during the build. Run the commands below from the repository root in an interactive desktop session.

## Start here

Fly around the small scene and edit it:

```powershell
cargo run --locked --package desktop-demo -- --interactive --scene-scale 64
```

`--interactive` needs no qualification build. It also accepts `--camera-pose` for the starting view and `--raster-region-extent`. It can't be combined with the other demo modes.

| Input | Action |
| --- | --- |
| Click the window | Capture the mouse for looking around. This click doesn't edit. |
| Mouse | Look around while the mouse is captured |
| **Escape** | Release the mouse |
| **W** / **A** / **S** / **D** | Move forward, left, back, and right |
| **E** / **Q** | Move up and down |
| **Shift** | Move faster |
| Left click | Break the targeted voxel |
| Right click | Place the selected material on the targeted face |
| **1**–**9** | Select a material, in the scene's material order |
| **Tab** | Switch between raster and compute-ray rendering |

The target is the voxel under the screen center. The overlay and window title show the Presenting Render Path, switch state, Required and Visible revisions, the target volume, coordinate, material, and entry face (or `none`), the selected material, and the result of the last action. Each click makes at most one edit, and holding a button doesn't repeat it. A placement outside the volume shows `Place-rejected-out-of-bounds`.

Edits are rejected while a Render Path switch is preparing its replacement (`Break-rejected-switching`). They're accepted again once the replacement starts presenting. Tab is rejected while a switch is in progress or while the Presenting Render Path is still converging. Rejected requests aren't queued. Wait for Required and Visible to match, then press Tab again.

### Large sparse terrain

```powershell
cargo run --release --locked --package desktop-demo -- --large-sparse-scene
```

This selects a deterministic 2048�256�2048 terrain, publishes sparse input into
SparsePages, and starts directly in compute-ray with the brickmap representation.
No raster preparation runs. The starting camera is just above the terrain near
`32,88,32`. The enclosed cavity spans `768..1024` in X/Z and `32..64` in Y.

Use the free-fly controls above: click to capture the mouse, WASD to move, Q/E to
move vertically, Shift for faster flight, Escape to release capture, left click
to break, and right click to place. Keys 1 and 2 select stone and grass. Picking
uses `observe_along_ray`. The overlay shows Required and Visible revisions;
after an edit, Visible catches up when the complete revision is installed.

Tab reports `Tab-rejected-LargeSparseSceneRequiresCompute` and queues nothing.
The flag cannot combine with `--interactive`, other demo modes, `--scene-scale`,
canonical camera selections, raster options, or dense compute. It accepts
`--compute-representation brickmap` and `--brickmap-budget-bytes`; the default
large-scene budget is 1 GiB. The restriction belongs to the demo configuration.

A Windows Vulkan 1.3 device with the compute-path capabilities is required.
It must support the brickmap storage buffers and have enough GPU memory for
visible and candidate scene data together, plus presentation resources. Plan
for the 1 GiB scene budget and additional process RAM for terrain generation.
Startup reports device capability or allocation failures. Debug qualification
also requires the Vulkan validation layer.

Run the large-scene GPU qualification explicitly on a capable device:

```powershell
cargo test --release --locked -p desktop-demo --test large_sparse_gpu large_sparse_terrain_gpu_qualification -- --ignored --nocapture
```

Ordinary test runs skip this qualification. It checks long empty rays, fill
interiors, mixed surfaces, cavity hits and misses, and all four structural
transitions using the interactive break/place command code. Every GPU result
reports its installed revision and is compared with `observe_along_ray` and
analytical expectations. The run requires zero validation warnings or errors.

### Streamed Voxel Scene

Launch the frozen `streamed-qualification-v1` fixture, a 16x16 grid of 64x64x64 Voxel Volumes:

```powershell
cargo run --release --locked --package desktop-demo --bin desktop-demo -- --streamed-world
```

The scene starts on Raster. Use the interactive camera and editing controls above, and
Tab to switch between Raster and Brickmap compute while moving. A clipped 7x7 Voxel
Residency Selection follows the camera without hysteresis; `--streamed-neighbourhood <size>`
chooses another odd size from 3 to 15. Camera moves wait for installed coverage when needed.
Visibility is capped by distance from the camera, so rotating in place keeps the same terrain
visible. The limit is the neighbourhood's guaranteed reach divided by the frustum's corner
length, about 124 voxels for 7x7 in a 16:9 window, and shortens for wide windows so the view
fits the neighbourhood. Distance fog fades terrain into the sky colour before that limit, and
voxel face edges are darkened so same-material voxels stay distinct. Both are presentation
settings that Raster and Brickmap apply identically; qualification and evidence runs keep them
off. The overlay reports Required and Installed selection identities next to Required and
Visible revisions, plus the neighbourhood size, last and worst crossing latency, and live and
peak materialized residency memory. Each completed crossing is also logged.

Edits survive eviction and return. Press R to restore every coordinate edited in this
session to its generated value, including coordinates in evicted volumes. Restoration
uses the same admission rule as other edits during replacement preparation.

`--streamed-world` enables interactive mode and defaults to Brickmap compute. It accepts
`--streamed-neighbourhood`, `--raster-region-extent`, `--brickmap-budget-bytes`, and explicit
Brickmap selection.
Dense compute and canonical-only demo modes are rejected during argument parsing.

### Compute switch qualification demo

Launch the fixed Render Path switching demo with the small scene:

```powershell
cargo run --locked --features qualification --package desktop-demo -- --scene-scale 64 --compute-switch-demo
```

Click the demo window to give it keyboard focus, then follow this sequence:

1. Wait for the raster scene and `Control=Tab-ready` in the overlay or window title.
2. Press **Tab** once to switch to compute-ray rendering. Wait for `Presenter=ComputeRay` and `Control=Space-ready`.
3. Press **Space** once to run the three-command voxel edit burst. Wait for `Required=4 Visible=4` and `Control=Space-complete-Tab-ready`.
4. Press **Tab** to switch back to raster. Wait for `Presenter=Raster` and `Control=Tab-ready-completed-2`.
5. Press **Tab** again to return to compute-ray. Wait for `Control=Tab-ready-completed-3`, then close the window normally.

This mode checks the completed round trip when you close it. Closing early reports an incomplete qualification error. The edit burst runs once per launch; restart to repeat it.

The overlay shows the presenting Render Path, switch state, Required and Visible Voxel Scene Revisions, edit-burst stage, and control readiness. During edits, Required can advance while Visible still shows the previous complete scene.

In this demo, Tab and Space only drive the fixed sequence. Use `--interactive` for free camera movement and editing.

## View the scene without a qualification sequence

For a simple raster viewer that you can close at any time:

```powershell
cargo run --locked --package desktop-demo -- --scene-scale 64 --camera-pose overview
```

Try the material close-up or boundary cutaway:

```powershell
cargo run --locked --package desktop-demo -- --scene-scale 64 --camera-pose cavity
cargo run --locked --package desktop-demo -- --scene-scale 64 --camera-pose boundary
```

| Option | Values | Default |
| --- | --- | --- |
| `--scene-scale` | `64`, `128`, `256` | `256` |
| `--camera-pose` | `overview`, `cavity`, `boundary` | `overview` |
| `--camera-move-step` | `0` through `120`, a fixed position along the overview-to-cavity move | Unset |
| `--raster-region-extent` | `16`, `32`, `64` | `32` |

Choose either a camera pose or a camera move step. For example, `--camera-move-step 60` opens at the midpoint; it does not start an animation. Scene scales correspond to volumes of 64×32×64, 128×64×128, and 256×128×256 voxels.

Print the scene and camera configuration without opening a window:

```powershell
cargo run --locked --package desktop-demo -- --scene-scale 64 --report-canonical-configuration
```

## Other demo modes

| Mode | What it does |
| --- | --- |
| `--portable-compute-ray-milestone-demo` | Automatically exercises compute replacement, camera acknowledgement, resizing, suspension, and restoration. When `Control=Space-ready` appears, continue from step 3 of the interactive sequence above. |
| `--compute-switch-lifecycle-demo` | Runs the automatic replacement lifecycle qualification and exits on completion. |
| `--edit-burst-demo` | Runs the raster edit scenario, with deliberate barriers that the verification script must release. Use the script below for a complete run. |
| `--winding-diagnostic` | Opens a small raster winding diagnostic scene. Use instead of scene-scale and camera options. |

For example:

```powershell
cargo run --locked --features qualification --package desktop-demo -- --scene-scale 64 --portable-compute-ray-milestone-demo
```

Compute demo modes cannot be combined with raster edit-burst, raster hold/failure-injection, or measurement modes. Flags such as `--hold-background-preparation`, `--hold-post-upload-candidate`, and `--compute-shutdown-qualification` are intended for the verification scripts and can deliberately leave work paused.

## Run the scripted demonstrations

These scripts build the demo, drive its controls, and save logs and evidence. Each evidence directory must be new or empty.

```powershell
# Raster edits, lifecycle checks, and shutdown checks.
pwsh -NoProfile -File scripts/verify-edit-burst-demo.ps1 -EvidenceDirectory artifacts/raster-demo -SceneScale 64

# Compute replacement, edits, three Render Path switches, and captures.
pwsh -NoProfile -File scripts/verify-portable-compute-ray-milestone.ps1 -EvidenceDirectory artifacts/compute-demo

# Four compute shutdown scenarios.
pwsh -NoProfile -File scripts/verify-compute-shutdown.ps1 -EvidenceDirectory artifacts/compute-shutdown
```

The portable milestone capture script also uses FFmpeg. See [Windows lifecycle verification](docs/verification/windows-lifecycle.md) for the full lifecycle runner, camera captures, and deliberate failure diagnostics, and [portable compute-ray evidence](docs/verification/portable-compute-ray-evidence.md) for collecting and verifying a complete evidence bundle.

## Collect measurements

Create the output directory first, then run either measurement mode:

```powershell
New-Item -ItemType Directory -Force artifacts | Out-Null

cargo run --locked --features qualification --package desktop-demo -- --scene-scale 64 --measurement-mode first-correct-frame --measurement-output artifacts/first-frame.jsonl

cargo run --locked --features qualification --package desktop-demo -- --scene-scale 64 --measurement-mode steady-state --measurement-output artifacts/steady-state.jsonl
```

`first-correct-frame` exits after presenting the matching raster artifact. `steady-state` uses a 1920×1080 borderless window, warms up for five seconds, then collects CPU/GPU frame measurements for thirty seconds and exits. Keep that window visible at its required size. Output is JSON Lines; an existing output file is overwritten.

For the repeatable timing workflow, see [timing evidence](docs/verification/timing-evidence.md).

## Qualification builds

Default Render Path builds omit failure-injection and barrier-hold controls. The desktop demo rejects verification-only modes with a `--features qualification` diagnostic. Enable the feature for interactive qualification sequences, failure tests, lifecycle checks, and measurements:

```powershell
cargo build --locked --package desktop-demo --features qualification
cargo test --locked --workspace --features qualification
```

The feature forwards to both Render Path crates. Verification scripts enable it when building. With `-SkipBuild`, supply a binary built with this feature. Ordinary scene viewing does not require it.

## Build and check

```powershell
cargo build --locked --package desktop-demo
cargo test --locked --features qualification --workspace
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
```

After building with `--features qualification`, you can launch the interactive qualification directly:

```powershell
.\target\debug\desktop-demo.exe --scene-scale 64 --compute-switch-demo
```

If startup fails, read the terminal error for the missing Vulkan capability or validation layer. If controls appear inactive, check that the window has focus and wait for the overlay's ready state. A raster edit scenario paused at a barrier needs its verification script to continue.
