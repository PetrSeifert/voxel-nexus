# Voxel Nexus

Voxel Nexus is a voxel engine whose logical scene remains independent of how voxel data is stored and how an image is produced.

## Language

**Voxel Scene**:
The renderer-independent collection of voxel volumes and the meaning of their contents.
_Avoid_: World, render scene

**Voxel Value**:
The logical contents of one voxel coordinate: either empty or the identity of a Voxel Material.
_Avoid_: Storage word, leaf payload

**Voxel Material**:
A stable renderer-independent identity and linear base color for an opaque occupied voxel value.
_Avoid_: GPU material, shader material

**Voxel Volume**:
An independently identified, finite three-dimensional field of voxel values with integer local coordinates, a scene-space origin, and a uniform voxel size. Coordinates outside its finite bounds are logically empty.
_Avoid_: Chunk, octree

**Voxel Region**:
An axis-aligned integer-coordinate subset of a Voxel Volume used to request or report logical voxel contents; internal partitioning is not part of its meaning.
_Avoid_: Chunk, brick, SVO node

**Voxel Cell Grid**:
A power-of-two cubic partition of a Voxel Volume, chosen by the caller and aligned to the volume-local origin, whose edge cells are clipped to the volume bounds and identified by their grid coordinate. Each cell is a Voxel Region classified by the same uniform-or-mixed content as any other region, and enumeration reports only the non-empty cells.
_Avoid_: Chunk grid, brick grid

**Voxel Frontend**:
The owner of voxel-scene state, storage-independent access, and edit semantics presented to render paths.
_Avoid_: Data backend

**Voxel Edit Command**:
An atomic request to set zero or more coordinates across identified Voxel Volumes to Voxel Values, with the last entry winning for repeated coordinates and any invalid entry rejecting the whole request. A command whose final values differ from the current scene publishes exactly one new Voxel Scene Revision; otherwise it publishes nothing.
_Avoid_: Edit batch, editor action

**Voxel Scene View**:
A stable, read-only view of the logical Voxel Scene at one revision; later edits do not change what it presents.
_Avoid_: Live scene, canonical copy

**Voxel Scene Revision**:
The monotonically ordered identity, within one Voxel Scene, of the logical contents presented by a Voxel Scene View. Each value-changing Voxel Edit Command receives the checked immediate successor of the current revision.
_Avoid_: Frame, storage version

**Required Voxel Scene Revision**:
The newest Voxel Scene Revision that a Render Path is obligated to make visible. Only a strictly newer changed submission can advance it, and no older not-yet-visible candidate can become visible.
_Avoid_: Target revision, requested revision

**Visible Voxel Scene Revision**:
The one Voxel Scene Revision represented by a Render Path's complete installed state used to produce frames. It remains unchanged until a required revision is installed atomically.
_Avoid_: Current revision, rendered version

**Voxel Change Set**:
A storage-independent description of semantic invalidations for one Voxel Scene between two adjacent Voxel Scene Revisions; changed regions may be conservative but never incomplete. It applies only to a consumer representing both the Voxel Scene identity and predecessor revision it names.
_Avoid_: Edit history, storage diff

**Raster Region**:
A fixed volume-local spatial identity for one independently derived raster result. Its identity and extent do not depend on a Voxel Scene Revision or Storage Tier partitioning.
_Avoid_: Chunk, mesh chunk, storage region

**Raster Convergence**:
The Render Path process of replacing its Visible Voxel Scene Revision with its newest Required Voxel Scene Revision while preserving complete revision-correct frames.
_Avoid_: Raster refresh, artifact update

**Storage Tier**:
The representation used to hold a voxel volume without changing that volume's logical contents.
_Avoid_: Storage backend, voxel format

**Render Path**:
A complete strategy for turning a voxel scene into an image, such as rasterization or ray traversal.
_Avoid_: Pipeline

**Camera State**:
The renderer-independent eye, target, up direction, vertical field of view, near plane, and far plane used by Render Paths to produce an image.
_Avoid_: Raster camera, camera settings

**Camera State Revision**:
The monotonically ordered identity of an accepted Camera State. A Render Path handoff requires both paths to acknowledge the same latest revision.
_Avoid_: Camera version, camera frame

**Semantic Ray**:
A finite scene-space ray segment evaluated against one Voxel Scene View independently of any Render Path representation.
_Avoid_: Screen ray, DDA ray

**Semantic Ray Observation**:
A Voxel Scene Revision-attributed miss or nearest occupied-voxel contact for one Semantic Ray, identifying the Voxel Volume, local coordinate, Voxel Material, and whether the ray entered through an outward face or started inside the occupied value.
_Avoid_: Pixel sample, shader hit

**Presenting Render Path**:
The Render Path whose complete installed state is designated to produce presented frames. A Render Path switch preserves it until a replacement is ready to take over.
_Avoid_: Active path, current path

**Replacement Render Path**:
A Render Path being prepared at a fixed Voxel Scene Revision to take over from the Presenting Render Path. It does not produce presented frames before the handoff.
_Avoid_: Candidate path, inactive path

**Retiring Render Path**:
A former Presenting Render Path retained after handoff until cleanup proves that it owns no resources or workers. It never produces another presented frame.
_Avoid_: Inactive path, old path

**Render Backend**:
The graphics execution layer shared by render paths.
_Avoid_: Renderer frontend
