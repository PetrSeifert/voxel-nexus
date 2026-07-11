# The Triangle Is Not the Point

- Milestone: [#8](https://github.com/PetrSeifert/voxel-nexus/issues/8)
- Status: Draft
- Episode role: Genesis
- Target duration: 8–10 minutes
- Estimated narration: ~1,000 words

## Beat 1 — Hook (~00:00–00:45)

**[Capture — Open on one uninterrupted run: pink triangle at launch, drag rapidly into landscape and portrait, minimize, restore, then close. Hold briefly on each state; do not reveal the validation result yet.]**

**VO:**
Of all the images this engine is ever going to render, this triangle is the least interesting one.

But before I can get anywhere near editable voxel scenes, or swapping storage structures around, or comparing rasterization against ray traversal — I need one image to stay on screen without falling apart the second the window does something a window normally does. Gets resized. Gets minimized. Gets dragged around.

So milestone one is a triangle. One that survives all of that, that can actually tell me why it refused to launch if it refuses to launch, and that I can back up with more than "yeah, looked fine when I ran it."

**[Existing — Smash cut through `launch-a.png`, `landscape-b.png`, `portrait-a.png`, and `restored-b.png`, ending on the manifest fields `ValidationErrors: 0` and `ProcessExitCode: 0`.]**

**[Capture — Title card: "Voxel Nexus 01 — The Triangle Is Not the Point"]**

## Beat 2 — Why this matters (~00:45–02:05)

**VO:**
Here's the architectural bet I'm making with Voxel Nexus: the logical scene shouldn't care how its voxel data is stored, and it shouldn't care how that data turns into an image either. Those are supposed to be three separate problems, not one tangled one.

The roadmap I've got written down works out to this: get a portable graphics backend running. Render a dense voxel volume with plain rasterization. Add editing and remesh it incrementally. Get the same scene rendering through a compute-ray path instead. Then — the actual payoff — swap between dense and sparse storage and have both render paths keep working without caring that anything changed underneath them.

Two storage tiers, two render paths, all four combinations working off the same scene. That's the real destination. The triangle's just the first thing that has to hold weight.

**[Capture — Compact roadmap graphic: `Render Backend → dense raster → editable raster → compute ray → 2 Render Paths × 2 Storage Tiers`. Keep future milestones visually muted.]**

**VO (continued):**
The boundary that actually matters right now is between the portable render backend and the Windows-specific window handling. The backend's job is Vulkan — picking a device, running commands, synchronizing, presenting. The Windows side just hands it a window and surface, because that part can't be made portable anyway.

And I'm keeping the triangle dumb on purpose. If I start designing a big renderer abstraction around it now, I'm designing an interface for voxels that don't exist yet based on a triangle that isn't representative of anything. That's a great way to build the wrong abstraction with total confidence.

**[Capture — Add the creator's personal series premise over a repo-from-empty visual]**

**VO:**
I've always been drawn to voxel games, especially the logic behind making all those tiny pieces manageable. 

Voxel Nexus is my attempt to build a general-purpose engine where different storage and rendering techniques can share one foundation, so trying a new approach doesn't mean rebuilding everything around it.

## Beat 3 — Decision and messy middle (~02:05–06:15)

**VO:**
The real decision here was to stop treating "the window did something" as a crash condition and start treating it as Tuesday.

Vulkan presents through a swapchain — a set of images sized and configured for whatever the window currently looks like. Resize the window and that config is stale. Minimize it and the drawable area drops to zero. Restore it and presentation just has to pick back up like nothing happened. None of that is exotic. It's just what windows do.

My first version basically ignored all of it. It checked for a Vulkan 1.3-capable GPU, opened the window through the adapter, printed which device it picked, and shut down cleanly. That got the boundary in place. It still didn't draw a single pixel.

**[Capture — Architecture diagram: `desktop demo → Windows presentation adapter → portable Render Backend → Vulkan`. Highlight that backend resources must be released before their window/surface dependencies.]**

**VO (continued):**
Even that empty shell bit me once, during review. Rust drops struct fields in the order they're declared, so the render backend field has to come before the window field — otherwise the window gets torn down while the Vulkan surface still depends on it existing. One-line fix. Took longer to notice than to fix, honestly.

After that came the actual triangle — pink, on a dark background, with generated shaders, a real swapchain, command recording, frame sync, all running with Vulkan validation turned on the whole time.

Then review caught something sneakier: I was reusing one "render finished" semaphore across frames like waiting on a frame's CPU fence also meant presentation was done with that frame. It doesn't. So the fix was to give presentation its own semaphore per swapchain image, so what I'm waiting on actually matches what's using it.

**[Capture — Show commit `5c6f002`; zoom only on `render_finished: Vec<vk::Semaphore>` and the semaphore selected by acquired image index. Animate one semaphore per swapchain image.]**

**VO (continued):**
That ended up being the rule for the rest of this: figure out what the system is actually waiting on, not what's convenient to wait on.

Swapchain goes invalid — rebuild it at whatever size the window is now. Window's got no usable area — stop trying to draw, don't spin the CPU. Minimized — sleep instead of burning a core doing nothing. Surface temporarily unavailable mid-transition — retry it, don't treat an ordinary desktop event like a fatal error.

None of this is exciting to build. It's also exactly the work that decides whether everything I build after this sits on an actual foundation, or just happened to work on my machine that one time.

**[Capture — Run the demo with an on-screen state overlay: `Presenting → Rebuild requested → Suspended at zero size → Restored → Presenting`. Pair minimize with a CPU graph or process sample showing no busy-loop.]**

**VO (continued):**
There was one more piece I didn't want to skip: what happens when it fails.

Originally every rejected GPU collapsed into the same message — no suitable device found. Which is an error message in the same way "something went wrong" is documentation.

Now each rejection keeps its actual reason. Vulkan 1.2 device gets told it needs 1.3 and pointed at a driver update. Missing swapchain support, missing image formats, no presentation mode — each of those gets reported separately with what to actually do about it.

My dev machine already supports everything it needs, so I can't just naturally hit these failure paths — I inject them deliberately instead. The tests launch the real process, check for actual actionable text in stderr, require a nonzero exit, make sure nothing panics, and time out if it hangs past five seconds.

**[Existing — Side-by-side crop of `unsupported-vulkan-1.2.stderr.log` and `unsupported-presentation.stderr.log`; highlight the unmet requirement and corrective guidance. Overlay `exit 1`, `no panic`, `under 5s` from the verified subprocess behavior.]**

## Beat 4 — Honest payoff (~06:15–08:20)

**[Capture — Replay the complete lifecycle proof at readable speed: launch, landscape resize, portrait resize, minimize, restore, normal close. Let the restored triangle sit untouched for three seconds.]**

**VO:**
So here's where that all lands.

The triangle launches on my Windows machine, stays correct through landscape and portrait, stops presenting while minimized, picks back up after restore, and exits cleanly.

That's it, though. It's not a voxel scene. It doesn't tell me anything about performance. And one machine running Windows doesn't prove this actually works on Linux or macOS — I haven't tested that yet, so I'm not going to claim it.

What it does prove is smaller than that, but it's real: the portable boundary is actually there, the first thing I've asked it to draw survives normal desktop use, and the run I've got recorded finished with zero validation warnings and zero validation errors.

**[Existing — Hold `restored-b.png`, then reveal `manifest.json`: Windows 11 Pro, NVIDIA GeForce RTX 4070, Vulkan 1.4.329, validation enabled, warnings 0, errors 0, lifecycle exit 0.]**

**VO (continued):**
The way I checked this was to start from a clean clone at a specific commit. The runner builds the shaders as part of the normal Cargo build, drives one validation-enabled process through the whole lifecycle, grabs matching screenshots at every visible state, and writes down the runtime context and exit codes as it goes. Both of the failure cases I injected exited with code one and kept their diagnostic output.

Thirteen tests passed. Formatting and lint both passed clean. And all of that evidence lives in the repo now, instead of living in my memory of which window I dragged where.

That's really the difference I care about. Anyone can get three vertices onto a screen. What I actually built here is the graphics layer that every render path I add later is going to have to trust.

**[Existing — Lay out the four visible lifecycle captures beside `manifest.json` and the two unsupported stderr logs. End on a simple stamp: `Supported path: clean exit` / `Unsupported path: actionable exit`.]**

## Beat 5 — Next promise (~08:20–09:10)

**VO:**
So the triangle's done its job.

Next milestone, I actually get to bring in real voxel data — a dense voxel volume, read through the storage-independent frontend I designed for it, rendered through a minimal raster path.

That's the first real test of the bet I'm making with this whole project: the backend shouldn't care how the scene is stored, and the scene shouldn't care how the image gets made. Right now that's a nice sentence in a docs file. Next episode it either holds up or it doesn't.

For now, I've got one very simple image on screen, and a genuinely overbuilt way of making sure it doesn't fall over. Next time, it gets some actual voxels.

**[Capture — Roadmap returns; milestone one locks into place and "dense raster proof" lights up. Close on the triangle breaking into a simple voxel silhouette only if that visual is clearly presented as an episode transition, not current engine footage.]**

## Editing notes

- **Best messy-middle material:** the field-order shutdown fix and the per-swapchain-image semaphore fix are the two tightest "here's the receipt" moments for the lifetime theme running through this episode. The later lifecycle commits stretch that same idea from Rust/Vulkan object lifetimes out to window state.
- **Capture gap worth investigating:** I've got the paired PNGs, logs, and manifest checked in, but no actual uninterrupted screen recording of a full run. Need to re-run the documented proof with screen recording on. Also still owe the one-line personal-motivation sentence flagged in Beat 2.
- **Reserve for another episode:** dependency setup, a full Vulkan walkthrough, the actual shader code, the full test list, and any real design talk about the voxel frontend, raster path, compute-ray path, or the editable SVO.
- **Capture selection:** the paired PNGs have matching triangle/background pixel counts per lifecycle state — use the named stills for the montage, keep the paired captures around as backup/verification texture.
- **Tone:** let the "graphics coincidence" line and the generic-error joke do the humor. Keep the actual proof sequence straight and don't rush it.

## Production source map

| Script claim or beat | Source | Evidence used |
| --- | --- | --- |
| Project bet and canonical terminology | [`CONTEXT.md`](../../CONTEXT.md) | The Voxel Scene remains independent of Storage Tier and Render Path; canonical definitions of Render Backend, Render Path, Voxel Frontend, and Voxel Volume. |
| Five-outcome roadmap and next dense-raster promise | [Roadmap resolution](https://github.com/PetrSeifert/voxel-nexus/issues/7#issuecomment-4939328251) | Dependency-ordered outcomes from portable Render Backend through two Render Paths and two Storage Tiers. |
| Triangle milestone scope and honest portability limit | [Milestone #8](https://github.com/PetrSeifert/voxel-nexus/issues/8) | Goal, completion demo, non-goals, Windows-only runtime-validation claim, and absence of a performance target. |
| Portable boundary, runtime qualification, and shutdown lifetime review fix | [Work issue #9](https://github.com/PetrSeifert/voxel-nexus/issues/9#issuecomment-4939622307), [commit `680972f`](https://github.com/PetrSeifert/voxel-nexus/commit/680972f5312c9868beb6d7b2290fdcf273551ba0) | Windows adapter boundary, qualified device diagnostics, clean close, and reviewed backend-before-window destruction order. |
| Validation-clean triangle and per-image semaphore fix | [Work issue #10](https://github.com/PetrSeifert/voxel-nexus/issues/10#issuecomment-4939746181), [commit `5c6f002`](https://github.com/PetrSeifert/voxel-nexus/commit/5c6f00261b18c2de79db83aad6aea6b92f3cb866) | Stable triangle, zero validation errors, generated shaders, and presentation semaphores tied to swapchain images. |
| Resize, zero-size suspension, minimize sleep, retry, and restore | [Work issue #11](https://github.com/PetrSeifert/voxel-nexus/issues/11#issuecomment-4939866268) | Verified extents, zero validation errors after restore, 0.0 CPU seconds during a five-second minimized sample, and clean exit. |
| Actionable deterministic prerequisite failures | [Work issue #12](https://github.com/PetrSeifert/voxel-nexus/issues/12#issuecomment-4939923396), [`unsupported-vulkan-1.2.stderr.log`](../evidence/windows-lifecycle/development-machine/unsupported-vulkan-1.2.stderr.log), [`unsupported-presentation.stderr.log`](../evidence/windows-lifecycle/development-machine/unsupported-presentation.stderr.log) | Exact Vulkan-version and presentation-capability guidance, exit code 1, no panic, and five-second subprocess hang guard. |
| Clean-checkout lifecycle proof and final verification | [Work issue #13](https://github.com/PetrSeifert/voxel-nexus/issues/13#issuecomment-4940043797), [`manifest.json`](../evidence/windows-lifecycle/development-machine/manifest.json) | Recorded revision and machine, generated shaders, four visible states, validation warnings/errors 0, lifecycle exit 0, unsupported exits 1, 13 passing tests, formatting, and strict lint checks. |
| Existing visible payoff | [`launch-a.png`](../evidence/windows-lifecycle/development-machine/launch-a.png), [`landscape-b.png`](../evidence/windows-lifecycle/development-machine/landscape-b.png), [`portrait-a.png`](../evidence/windows-lifecycle/development-machine/portrait-a.png), [`restored-b.png`](../evidence/windows-lifecycle/development-machine/restored-b.png) | Retained triangle captures at launch, landscape, portrait, and restored states. |

## Unresolved inputs

None
