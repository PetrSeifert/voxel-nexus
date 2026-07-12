# The First Real Voxel Image

- Milestone: [#21](https://github.com/PetrSeifert/voxel-nexus/issues/21)
- Status: Draft
- Episode role: Continuation
- Target duration: 8–10 minutes
- Estimated narration: ~1,010 words

## Beat 1 — Hook (~00:00–00:45)

**[Existing — Open on `milestone-proof.mkv` at the overview, then cut through the cavity, finite-boundary pose, and camera move. Hold each result long enough to read the three material regions.]**

**VO:**
Last episode, Voxel Nexus could render exactly one thing: a pink triangle that was much more robust than it had any right to be.

Now that triangle has become a dense scene with more than three million occupied voxels, visible cavities, three materials, and a camera that can actually inspect it.

This is the engine's first real voxel image. It also made it all the way into a completion bundle before one small graphics convention invalidated every glamour shot. The scene looked solid at first glance, but the renderer was showing the back of it through the front.

So this episode isn't just about getting voxels on screen. It's about whether the boundaries I built around that triangle still work when the image becomes real—and whether the proof is strict enough to catch an image that's convincingly wrong.

**[Capture — Title card: "Voxel Nexus 02 — The First Real Voxel Image"]**

## Beat 2 — Why this matters (~00:45–02:00)

**VO:**
The architectural bet behind Voxel Nexus is that a logical voxel scene should remain independent of two things: how its data is stored, and how that data becomes an image.

For this milestone the storage is dense and the image is conventional rasterization, but neither choice is supposed to leak across the engine. The Voxel Frontend publishes an immutable scene view. A Render Path turns that view into an image. The existing Render Backend handles Vulkan execution and the Windows lifecycle without knowing what kind of image it's drawing.

**[Capture — Compact diagram: `dense Storage Tier → Voxel Frontend → immutable Voxel Scene View → raster Render Path → Render Backend`. Highlight the two seams.]**

**VO (continued):**
The tradeoff is deliberately unglamorous. This isn't greedy meshing, streaming, or an editable world. The raster artifact creates one independent quad for every exposed voxel face. That uses more geometry than a production mesher should, but it makes the result exact, easy to verify, and independent of however the dense storage chooses to batch its reads.

For the first real image, proving those seams matters more than pretending the first mesher is the final one.

## Beat 3 — Decision and messy middle (~02:00–06:05)

**VO:**
The first step was to evict the triangle from the Render Backend.

Its shaders, pipeline, framebuffer, and draw state moved behind a small Render Path protocol. The backend still controlled synchronization, presentation, resize, minimize, restore, and failure propagation. The same pink triangle survived the move, which was useful precisely because nothing visible changed: it proved the boundary before voxel code depended on it.

**[Existing — Brief montage of the triangle lifecycle captures. Then Capture — animate the protocol order: `release → recreate presentation targets → configure → record → submit/present`.]**

**VO (continued):**
Next came the scene side. The Voxel Frontend publishes one validated revision with stable scene, volume, and material identities. The renderer keeps a read-only view of that revision and asks for logical Voxel Regions instead of reaching into dense memory.

The public tests read the same contents as one batch and as four differently shaped batches. The surface result has to be identical either way. An occupied voxel next to empty space produces a face; two occupied neighbors do not, even if their materials differ. A hollow three-by-three-by-three diagnostic produces exactly sixty exposed faces, including the internal cavity.

**[Capture — Show the one-batch/four-batch equivalence test, then the hollow diagnostic's 60-face semantic result. Keep storage layout off screen.]**

**VO (continued):**
That exact face set becomes an immutable raster artifact tagged with its source revision. A worker prepares it in the background, and the Render Path installs it only after the complete matching artifact reaches the GPU.

The lifecycle proof can pause that worker at a deterministic barrier. While the geometry is frozen, the real desktop window is resized to landscape and portrait, minimized, and restored. Release the worker, and revision one is installed once. Inject a derivation or upload failure instead, and no partial artifact becomes visible; the error reaches the application boundary with the phase and revision attached.

**[Existing — Use `worker_paused.png`, the lifecycle clip through restore, then `first_matching_revision_frame.png`. Overlay only the recorded event names and revision identity.]**

**VO (continued):**
With that working, a fixed-seed generator produced three scene scales and three repeatable inspection poses: an overview, a carved cavity, and a cut through the finite boundary. The largest scene contains 3,366,912 occupied voxels and derives 217,856 exposed quads.

Then, after the first completion bundle was recorded, the image was found to be inside-out.

Vulkan's projection correction flips the vertical axis. The raster state still declared clockwise framebuffer winding as front-facing, so back-face culling removed the nearest outward surfaces and left farther back faces visible through them. The fix was one convention change—counter-clockwise is front-facing—plus a regression that runs a real derived artifact through the real camera transform.

**[Capture — Extract the invalid overview from commit `adaed0d`, then match-cut to the corrected overview. Existing — follow with the warm-near/blue-far winding diagnostic pair.]**

**VO (continued):**
The final completion evidence was regenerated after that fix. A screenshot can be stable, repeatable, validation-clean, and still be a stable picture of the wrong thing. Apparently even the pixels need a second source.

## Beat 4 — Honest payoff (~06:05–08:20)

**[Existing — Play the uninterrupted 38.4-second completion clip: paused worker, landscape, portrait, minimize, restore, first matching-revision frame, overview, cavity, boundary, camera move, clean close.]**

**VO:**
This is the complete result in one validation-enabled Windows process.

The window stays responsive while preparation is paused. The finished artifact appears only after the worker is released. The three fixed poses show the silhouette, material regions, carved opening, and finite edge. The camera completes its deterministic move, the application closes cleanly, and the run reports zero Vulkan validation warnings and zero errors.

On this recorded development machine, ten fresh runs of the largest scene took a median of about 675 milliseconds from publication to the first correct frame. That's a descriptive baseline, not a performance target. The current artifact is deliberately simple, whole-volume, and non-incremental.

**[Existing — Hold the corrected overview, then show `comparison.svg` with the 256-scale total median highlighted. Do not generalize beyond the recorded machine.]**

**VO (continued):**
The retained bundle starts from a detached clean checkout, records the build and checks, verifies 212 inventoried artifacts by hash, and independently decodes the completion clip. It also keeps the exact face diagnostics and the raw timing streams, so the visible result and the claims around it can be audited separately.

What this does not prove is editing, production performance, sparse storage, another Render Path, or portability beyond this Windows machine.

What it does prove is the first vertical slice of the engine's main idea: one storage-neutral scene revision can cross background preparation, become a raster artifact, and survive the same desktop lifecycle that the triangle established—without the Voxel Frontend, Render Path, and Render Backend collapsing into one system.

## Beat 5 — Next promise (~08:20–09:10)

**VO:**
The next roadmap candidate is where this static proof starts moving: edit voxels interactively, publish later scene revisions, remesh only the affected raster results, and make sure stale background work never replaces newer geometry.

That's where the revision tag stops being a label on one artifact and becomes an actual concurrency contract.

The triangle proved the graphics foundation. This scene proved the first complete path through the engine. Next, the voxels have to change without breaking either one.

**[Capture — Return to the roadmap. Lock “dense raster Voxel Scene proof” and light up the provisional “revision-correct editable raster proof.” Close on the corrected cavity view with one voxel edit indicated as an explicitly aspirational transition.]**

## Editing notes

- **Best messy-middle material:** The invalid completion capture from `adaed0d` versus the corrected capture after `0bdc68f` is the strongest story receipt. Pair it with the warm-near/blue-far diagnostic so the fix is visually legible instead of merely stated.
- **Capture gap worth investigating:** Extract matching invalid and corrected overview/cavity frames from Git history at identical crop and pose. The invalid source is retained in commit history but is not present in the current evidence directory.
- **Reserve for another episode:** Full timing tables, the dependency catalogue, shader details, generalized meshing discussion, and implementation specifics for edits, Voxel Change Sets, greedy remeshing, or sparse storage.
- **Proof pacing:** Let the cavity and boundary shots breathe. The forms are intentionally diagnostic rather than scenic, so quick cuts make the result harder—not more exciting—to understand.
- **Terminology:** Keep “Voxel Scene,” “Voxel Scene View,” “Storage Tier,” “Render Path,” and “Render Backend” aligned with `CONTEXT.md`; avoid calling the demonstrated Voxel Volume a world or chunk system.

## Production source map

| Script claim or beat | Source | Evidence used |
| --- | --- | --- |
| Series continuity and triangle starting point | [Devlog milestone #8](8-robust-vulkan-triangle.md), [issue #8](https://github.com/PetrSeifert/voxel-nexus/issues/8) | Prior episode premise, validated desktop lifecycle, and triangle-only starting state. |
| Project bet and canonical terminology | [`CONTEXT.md`](../../CONTEXT.md) | Storage-independent Voxel Scene and distinct Voxel Frontend, Render Path, and Render Backend definitions. |
| Milestone goal, frozen scope, and Windows-only limits | [Milestone #21](https://github.com/PetrSeifert/voxel-nexus/issues/21) | Closed milestone inventory, completion demo, non-goals, and verified planned work. |
| Render Path protocol behind an unchanged triangle | [Issue #22](https://github.com/PetrSeifert/voxel-nexus/issues/22) | Render-path-owned state and backend-driven lifecycle ordering with validation-clean triangle captures. |
| Immutable revision and storage-neutral region reads | [Issue #23](https://github.com/PetrSeifert/voxel-nexus/issues/23) | Stable identities, retained view, one-batch/four-batch equivalence, and contextual invalid-read behavior. |
| Exact exposed-face artifact and 60-face hollow diagnostic | [Issue #24](https://github.com/PetrSeifert/voxel-nexus/issues/24), [`semantic-face-report.json`](../evidence/milestone-completion/development-machine/semantic-face-report.json) | Occupied-to-empty face rule, material behavior, finite-boundary behavior, batch permutation, and exact semantic oracle. |
| Canonical scene, inspection poses, and geometry counts | [Issue #26](https://github.com/PetrSeifert/voxel-nexus/issues/26), [`geometry-resource-counts.json`](../evidence/milestone-completion/development-machine/geometry-resource-counts.json) | Fixed-seed scales and 256-scale counts: 3,366,912 occupied voxels and 217,856 exposed quads. |
| Background preparation and matching-revision installation | [Issue #27](https://github.com/PetrSeifert/voxel-nexus/issues/27), [`completion-video-events.json`](../evidence/milestone-completion/development-machine/lifecycle/completion-video-events.json) | Worker barrier, responsive lifecycle, one complete revision install, three poses, camera move, and clean close. |
| Descriptive 256-scale timing | [Issue #28](https://github.com/PetrSeifert/voxel-nexus/issues/28), [`timing-summary.json`](../evidence/milestone-completion/development-machine/timing-summary.json) | Ten fresh runs and 674.7408 ms median publication-to-first-correct-frame total on the recorded Windows machine. |
| Clean-checkout bundle and uninterrupted proof | [Issue #29](https://github.com/PetrSeifert/voxel-nexus/issues/29), [`README.md`](../evidence/milestone-completion/development-machine/README.md), [`milestone-proof.mkv`](../evidence/milestone-completion/development-machine/lifecycle/milestone-proof.mkv) | Reproduction and verification commands, retained clip, lifecycle sequence, and evidence inventory. |
| Inside-out image, winding correction, and refreshed 212-artifact bundle | [Issue #30](https://github.com/PetrSeifert/voxel-nexus/issues/30), [fix commit `0bdc68f`](https://github.com/PetrSeifert/voxel-nexus/commit/0bdc68f), [`winding-diagnostic-a.png`](../evidence/milestone-completion/development-machine/lifecycle/winding-diagnostic-a.png), [`winding-diagnostic-b.png`](../evidence/milestone-completion/development-machine/lifecycle/winding-diagnostic-b.png) | Counter-clockwise framebuffer winding, real-artifact regression, corrected occlusion, zero Vulkan findings, and verified refreshed inventory. |
| Provisional next editable-raster outcome | [Roadmap resolution #20](https://github.com/PetrSeifert/voxel-nexus/issues/20#issuecomment-4946954995) | Revision-correct interactive edits, affected-result replacement, stale-work rejection, and the explicit provisional status of later outcomes. |

## Unresolved inputs

- None.
