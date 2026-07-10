# First sparse Storage Tier proof

Researched 2026-07-10 for Wayfinder frontier [Choose the first sparse storage proof](https://github.com/PetrSeifert/voxel-nexus/issues/4), under the five-milestone architecture proof in [Chart the first five demonstrable voxel-engine milestones](https://github.com/PetrSeifert/voxel-nexus/issues/1).

## Recommendation

Make an editable sparse voxel octree (SVO) the first sparse **Storage Tier**. The fifth milestone should switch the same logical **Voxel Volume** between the dense and SVO tiers without changing **Voxel Scene** semantics or either **Render Path** contract.

Do not require a sparse voxel DAG or an SVO-to-DAG progression in the first five milestones. A DAG is a valuable later compression tier or immutable derived snapshot if measurements establish that repeated subtrees dominate SVO memory. It is not needed to prove storage interchangeability, sparse editing, raster meshing, or portable compute ray traversal.

This is an inference from the primary sources below, not a claim that an SVO will be the final or fastest representation.

## Primary-source findings

### An SVO already proves sparse hierarchy and ray traversal

Laine and Karras present a compact sparse voxel octree and an efficient GPU ray-casting algorithm. Their results establish an SVO as a directly traversable sparse hierarchy rather than merely an intermediate compression format. The paper also stores per-voxel shading and contour information, so the representation is not intrinsically limited to binary occupancy. [NVIDIA Research publication and paper](https://research.nvidia.com/publication/2010-02_efficient-sparse-voxel-octrees)

The paper does not establish the edit or raster-meshing behavior needed by Voxel Nexus. Those must be demonstrated by the milestone; they should not be assumed from ray-casting results.

### A sparse voxel DAG adds subtree deduplication after sparsity

Kämpe, Sintorn, and Assarsson define their DAG by finding common subtrees in an SVO and allowing identical regions to share nodes. Their bottom-up reduction produces a minimal DAG, reduces node counts by one to three orders of magnitude in their tested high-resolution binary scenes, and remains directly ray traversable. [Chalmers publication record](https://research.chalmers.se/publication/182658) · [author-version paper](https://icg.gwu.edu/sites/g/files/zaxdzs6126/files/downloads/highResolutionSparseVoxelDAGs.pdf)

The source therefore establishes a real memory opportunity, but it also shows that a DAG tests two ideas at once: omission of empty regions and deduplication of identical non-empty regions. Dense-to-SVO is the cleaner first comparison because it isolates the effect of sparsity.

### Editable DAGs are a separate data-structure problem

Careil, Billeter, and Eisemann state that conventional sparse voxel DAGs handle static data, then introduce a new HashDAG structure specifically to support interactive modification without full decompression and recompression. Their work also adds compressed attributes such as color and evaluates carving, filling, copying, and painting. [Eurographics publication and paper](https://diglib.eg.org/items/d34c8b98-8291-4b7b-84cc-2bad7e162587)

This evidence rules out treating "add DAG compression" as a small extension of the first editable sparse tier. Shared descendants mean the same node can represent several spatial regions; a location-specific mutation must preserve the other references, maintain deduplication invariants, and account for voxel attributes. That consequence is an inference from the DAG definition and the need for a distinct editable-DAG design in the cited work.

### Dynamic sparse hierarchy does not require DAG sharing

Museth's VDB is not an SVO, but it is useful counter-evidence to the idea that subtree deduplication is required for an editable sparse hierarchy. VDB separately encodes sparse hierarchical topology and values and reports insertion, retrieval, deletion, dynamic topology, and fast random and sequential access as first-class goals. [VDB paper](https://museth.org/Ken/Publications_files/Museth_TOG13.pdf) · [OpenVDB project documentation](https://academysoftwarefoundation.github.io/openvdb/faq.html)

This does not select VDB for Voxel Nexus. It supports the narrower inference that the first milestone can prioritize sparse hierarchical access and mutation while deferring DAG-style structural sharing.

## Fit against the Phase 1 needs

| Need | SVO first | DAG first or required SVO-to-DAG progression |
| --- | --- | --- |
| Voxel edits | Keeps each represented branch structurally unique; the milestone still has to prove its edit and change-reporting behavior. | Shared descendants make location-specific mutation and deduplication maintenance part of the proof. |
| Raster meshing | Can expose storage-independent voxel or region queries through the Voxel Frontend; no DAG decompression step is required. | Does not remove the need for storage-independent queries and adds shared-location bookkeeping. |
| Compute ray traversal | Direct SVO traversal has primary-source precedent. | Direct DAG traversal also has precedent, so ray traversal alone does not justify the extra scope. |
| Observable comparison | Dense versus SVO isolates sparse allocation and hierarchy costs. | Dense versus DAG conflates sparsity with repeated-subtree compression. |
| Bounded milestone | Proves the second Storage Tier and the architecture vertical slice. | Adds compression construction, shared-node mutation, and attribute policy before the architecture proof requires them. |

The raster-meshing row is an interface recommendation, not a sourced performance result. The **Voxel Frontend** should present storage-independent semantics; the SVO's concrete node layout must not cross into either Render Path.

## Roadmap consequence

Refine the fifth candidate to: **Switch the same Voxel Scene from the dense Storage Tier to an editable SVO Storage Tier without changing scene semantics or either Render Path contract.**

The eventual evidence contract should compare the dense and SVO tiers on the same deterministic scene and edit sequence, including:

- identical queried voxel values and observable edit results;
- equivalent raster and compute-ray images within the correctness rules selected later;
- resident memory for both tiers;
- initial construction or load time and edit-to-visible-update time, reported as measurements rather than premature production targets.

The exact scenes, scales, correctness rules, and measurement protocol remain owned by [Set Phase 1 demonstration and performance evidence](https://github.com/PetrSeifert/voxel-nexus/issues/6).

Reconsider a DAG only after the SVO proof supplies evidence that repeated subtrees create a meaningful memory opportunity and after the project decides whether compression belongs in a mutable Storage Tier or an immutable Render Path snapshot. That later choice is outside the first five-milestone acceptance contract.
