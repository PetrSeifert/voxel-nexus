# The Voxels Can Finally Change

- Milestone: [#38](https://github.com/PetrSeifert/voxel-nexus/issues/38)
- Status: Draft
- Episode role: Continuation
- Target duration: 8–10 minutes
- Estimated narration: ~1,150 words

## Beat 1 — Hook (~00:00–00:45)

**[Existing — Open on `final-visible.png`, then cut backward to `post-upload-barrier-held.png` and `cpu-barrier-held.png`. Crop tightly enough that the Required and Visible revision numbers remain legible.]**

**VO:**
Last episode, Voxel Nexus produced its first complete voxel image. It was immutable, which is a wonderfully convenient property for anything that doesn't need to change.

Now one press of the Space bar publishes three voxel edits. But the important result isn't just that three tiny pieces of geometry appear. It's what the engine refuses to show while they are being built.

Revision two is cancelled during CPU work. Revision three reaches the GPU and is still rejected. Through all of that, revision one remains complete, visible, and usable. Only when revision four is ready does the whole visible scene advance at once.

Apparently editing one voxel is also a concurrency problem.

**[Capture — Title card: “Voxel Nexus 03 — The Voxels Can Finally Change”]**

## Beat 2 — Why this matters (~00:45–02:05)

**VO:**
Voxel Nexus is built around one separation: the logical Voxel Scene shouldn't depend on how its data is stored or how an image is produced. The dense scene and raster renderer are the current implementations, not the definition of the engine.

That separation was manageable when the Voxel Frontend published one read-only revision. Editing makes it a real contract. A command validates one coordinate, preserves older views, and publishes one successor only when the value changes. The renderer receives that complete view plus a storage-independent Voxel Change Set. It never gets permission to reach into dense memory and improvise.

**[Capture — Compact diagram: `Voxel Edit Command → immutable successor Voxel Scene View + Voxel Change Set → raster Render Path`. Keep the dense Storage Tier below the Frontend seam, not in the Render Path.]**

**VO (continued):**
The central tradeoff is that I want to rebuild less geometry without ever presenting part of a new scene.

The raster path now divides each finite Voxel Volume into stable Raster Regions. These aren't storage chunks. They're independently derived pieces of the image, each with its own identity and GPU resources. A region reads its core plus a one-voxel face-neighbor halo, because changing a voxel can reveal or hide a face on either side of a boundary.

That lets one edit conservatively select the few regions whose output may have changed while every unrelated region keeps the exact installation and resources it already had.

## Beat 3 — Decision and messy middle (~02:05–06:15)

**[Capture — Animate a 2×2×2 Raster Region grid. Put an edit beside three region boundaries; highlight its core and the three face-adjacent regions, but leave the diagonal regions untouched.]**

**VO:**
The first part was making that ownership exact. Empty regions still need entries, identities must stay stable across revisions, and occupied voxels across a region boundary must not create an internal seam. Tests compare every affected result with an independent face oracle and verify that diagonal and unrelated regions do no derivation, upload, or replacement work.

Localization creates a new danger, though. If affected region A is replaced before affected region B, the frame represents neither the old scene nor the new one. It is an accidental revision assembled from both.

So the renderer distinguishes the newest Required Voxel Scene Revision from the one complete Visible Voxel Scene Revision. Raster Convergence can retain one active CPU generation, one replaceable newest pending target, and at most one hidden uploaded GPU candidate. New edits accumulate their invalidation into the newest requirement. Cancellation saves obsolete CPU work, but cancellation isn't trusted for correctness; generation and revision checks still guard upload and the frame-boundary commit.

**[Capture — Revision timeline: Visible stays at 1. Required advances 1 → 2 → 3 → 4. Show revision 2 stopping at the CPU gate, revision 3 stopping after upload, and only revision 4 crossing the frame-boundary commit.]**

**VO (continued):**
The uninterrupted demonstration turns those rules into a deliberately hostile three-command sequence.

The first command publishes revision two. After exactly one region is scheduled, a deterministic barrier pauses its CPU preparation. The second command advances the requirement to revision three, so the obsolete generation is cancelled before upload. While it is held, the application changes camera, resizes between landscape and portrait, minimizes, and restores. The old complete scene keeps working.

Revision three then finishes preparation and uploads its replacement resources, but they remain hidden behind a second barrier. The window goes through more presentation lifecycle changes, and the candidate is restored without becoming visible. Then the third command publishes revision four.

At the next commit gate, revision three is stale. The backend rejects it and retires its two GPU resources only when it is safe to do so. Revision four derives the accumulated affected set and replaces it in one frame-boundary installation. The Visible revision moves directly from one to four; two and three never appear.

**[Existing — Interleave `cpu-barrier-held.png` and `post-upload-barrier-held.png` with brief excerpts from `desktop-demo.stdout.log`: “Obsolete CPU generation cancelled” and “Superseded candidate rejected at commit.”]**

**VO (continued):**
Failures follow the same rule. A current derivation, upload, or commit failure preserves the visible installation and pauses automatic retry with useful context. A newer requirement can supersede an older failure. Shutdown joins active work and retires every owned resource.

The remaining design choice was region size. Smaller regions localize work more precisely, but create more GPU resources. Larger regions reduce resource count, but make each small edit rebuild more geometry.

All three candidates—16, 32, and 64 voxels cubed—first had to pass the same correctness, lifecycle, failure, shutdown, retirement, and Vulkan validation gates. On this machine, 16 cubed won with a median final-visible latency of about 732 milliseconds. It also used 1,414 live GPU buffers, compared with 302 and 64 for the larger candidates. That is a local choice, not a universal ideal.

**[Existing — Animate `comparison.svg`. Highlight the 16-cubed latency result, then hold the buffer-count tradeoff. Label every value “recorded Windows development machine.”]**

## Beat 4 — Honest payoff (~06:15–08:25)

**[Existing — Play the retained demonstration states in order: CPU-held cavity, post-upload-held boundary, then final overview. Hold each overlay long enough to read Required, Visible, Affected, and Unaffected.]**

**VO:**
The visible edit is small because the proof is about everything around it.

During the CPU hold, Required is three while Visible remains one. Two of the provisional 32-cubed regions are affected and 254 are untouched. After the final command, Required reaches four while Visible is still one. Only after the newest installation is complete does Visible become four, with three affected regions and 253 unaffected regions retained.

There is no flash of revision two, no frame made from revision three plus revision one, and no full-scene raster rebuild disguised as localization. Camera and desktop lifecycle actions remain operable at both barriers. The validation-enabled process closes normally with zero Vulkan warnings, zero Vulkan errors, and zero Render Path-owned raster resources left after shutdown.

**[Capture — Build three overlay states from the retained log: `Required/Visible=3/1`, `4/1`, and `4/4`. Draw one horizontal line through `Visible=1, 1, 4`; avoid zooming into the actual edited voxels as if they were a dramatic visual transformation.]**

**VO (continued):**
The later selected 16-cubed configuration was also characterized across three scene scales. Five recorded samples per scale produced machine-local median keypress-to-final-visible times of about 122, 187, and 729 milliseconds. Those are descriptive distributions, not a production budget and not a cross-machine claim.

The final evidence bundle inventories 146 artifacts across the demonstration, correctness checks, lifecycle logs, raw measurements, selection, and validation. Its verifier checks their hashes, nested manifests, required exits, and selected extent.

This still isn't an editor. There is no picking, free-form tool, history, streaming, greedy meshing, or performance target. Space triggers a fixed sequence designed to expose stale work.

What it proves is narrower and more useful: the scene can publish real edits, the raster path can isolate their spatial consequences, and asynchronous work can converge on the newest complete revision without sacrificing the last good image.

## Beat 5 — Next promise (~08:25–09:15)

**VO:**
The next roadmap hypothesis changes a different axis.

The editable Voxel Scene now has a raster Render Path with a real revision and convergence contract. The next candidate is to render that same logical scene through a portable compute-ray Render Path, then compare the two image strategies without changing storage at the same time.

If that works, rasterization stops being the way Voxel Nexus renders and becomes one way it can render.

The first triangle established the graphics foundation. The static scene established the first voxel image. Now the voxels can change without tearing that image apart. Next, the same scene needs a genuinely different path to the screen.

**[Capture — Roadmap transition: lock “localized editable raster convergence,” light up “portable compute-ray Render Path,” and leave “editable-SVO storage independence” dimmed as a later hypothesis.]**

## Editing notes

- **Best messy-middle material:** The two deterministic barriers form the clearest sequence: revision 2 cancelled during CPU work, revision 3 restored after lifecycle changes but rejected after upload, and revision 4 installed atomically. Use the held screenshots, reconstruct the `Required=4 / Visible=1` state from the retained log, and prefer two short log excerpts over a commit montage.
- **Capture gap worth investigating:** `[NEEDS INPUT: Confirm or recapture demo/before-burst.png. Visual inspection shows unrelated artwork rather than the Voxel Nexus window described by its manifest entry.]` Use a fresh initial revision-one frame or the prior episode's final overview for the “before” state.
- **Reserve for another episode:** Detailed failure injection cases, all 15 scale samples, the 128-edit bounded-state regression, evidence-schema implementation, and a tutorial-level explanation of halo reads or GPU fence retirement.
- **Proof pacing:** The geometry change is intentionally difficult to see at overview scale. Let the revision overlay and regional counts carry the proof; do not manufacture a dramatic before/after zoom that the retained footage does not support.
- **Terminology:** Keep Voxel Scene, Voxel Scene View, Voxel Change Set, Raster Region, Raster Convergence, Required Voxel Scene Revision, Visible Voxel Scene Revision, Render Path, and Storage Tier aligned with `CONTEXT.md`. Do not call Raster Regions chunks.

## Production source map

| Script claim or beat | Source | Evidence used |
| --- | --- | --- |
| Series continuity and prior immutable raster scene | [Devlog milestone #21](21-dense-raster-voxel-scene.md), [issue #21](https://github.com/PetrSeifert/voxel-nexus/issues/21) | Previous episode premise, one immutable Voxel Scene Revision, and whole-volume raster starting point. |
| Project bet and canonical terminology | [`CONTEXT.md`](../../CONTEXT.md) | Storage- and renderer-independent Voxel Scene, edit, revision, regional, and convergence definitions. |
| Frozen milestone scope and completion inventory | [Milestone #38](https://github.com/PetrSeifert/voxel-nexus/issues/38) | Closed milestone goal, completion demo, planned-work notes, non-goals, and capture plan. |
| Serialized edit publication and immutable retained views | [Issue #39](https://github.com/PetrSeifert/voxel-nexus/issues/39) | Contextual error/no-op/success outcomes, adjacent successor revision, storage-independent Voxel Change Set, and public editing tests. |
| Stable Raster Regions and localized replacement | [Issue #40](https://github.com/PetrSeifert/voxel-nexus/issues/40), [issue #41](https://github.com/PetrSeifert/voxel-nexus/issues/41) | Core-plus-face-neighbor derivation, explicit empty results, stable identities, semantic-face oracle, no diagonal invalidation, and retained unaffected resources. |
| Bounded newest-required CPU convergence | [Issue #42](https://github.com/PetrSeifert/voxel-nexus/issues/42) | One active preparation, replaceable newest target, accumulated invalidation, cancellation, stale-before-upload check, and bounded 128-edit regression. |
| Hidden candidate and atomic frame-boundary commit | [Issue #43](https://github.com/PetrSeifert/voxel-nexus/issues/43) | At most one hidden GPU candidate, stale commit rejection, complete affected-set swap, unaffected resource retention, and fence-safe retirement. |
| Contextual retry and safe lifecycle/shutdown | [Issue #44](https://github.com/PetrSeifert/voxel-nexus/issues/44), [issue #45](https://github.com/PetrSeifert/voxel-nexus/issues/45) | Typed phase/revision/region failures, explicit retry, newer-requirement supersession, joined workers, held-candidate lifecycle proof, clean close, and zero owned resources. |
| Three-command demonstration and exact overlay states | [Issue #46](https://github.com/PetrSeifert/voxel-nexus/issues/46), [`demo/manifest.json`](../evidence/localized-editable-raster/v1/development-machine/demo/manifest.json), [`desktop-demo.stdout.log`](../evidence/localized-editable-raster/v1/development-machine/demo/desktop-demo.stdout.log) | One Space keypress, revisions 2–4, CPU cancellation, stale uploaded candidate, lifecycle actions, atomic 1→4 visibility, regional counts, validation 0/0, and shutdown resources 0. |
| Region extent tradeoff and selected 16-cubed result | [Issue #47](https://github.com/PetrSeifert/voxel-nexus/issues/47), [`selection.json`](../evidence/localized-editable-raster/v1/development-machine/extent-selection/selection.json) | Qualification gates, deterministic selection rule, five samples per candidate, latency medians, GPU bytes, buffer counts, and selected extent. |
| Machine-local scale characterization | [Issue #48](https://github.com/PetrSeifert/voxel-nexus/issues/48), [`raw-distributions.json`](../evidence/localized-editable-raster/v1/development-machine/scale-characterization/raw-distributions.json), [`comparison.svg`](../evidence/localized-editable-raster/v1/development-machine/comparison.svg) | Five samples at each scale; phase, work-disposition, resource, cancellation, retirement, and keypress-to-final-visible distributions. |
| Verified 146-artifact evidence bundle | [Issue #49](https://github.com/PetrSeifert/voxel-nexus/issues/49), [`README.md`](../evidence/localized-editable-raster/v1/development-machine/README.md), [`manifest.json`](../evidence/localized-editable-raster/v1/development-machine/manifest.json) | Versioned inventory, source-manifest and process-outcome cross-checks, checksums, selected extent, verifier, and reproduction instructions. |
| Provisional next compute-ray outcome | [Roadmap resolution #37](https://github.com/PetrSeifert/voxel-nexus/issues/37) | Portable compute-ray Render Path as the next one-sentence hypothesis, followed later by editable-SVO storage independence. |

## Unresolved inputs

- `[NEEDS INPUT: Confirm whether the retained demo/before-burst.png should be replaced before production; its pixels do not match the Voxel Nexus demo state recorded in its manifest metadata.]`
