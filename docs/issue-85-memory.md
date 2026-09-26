# Material palette memory measurements

Measured on Windows x64 with the release build of the canonical scene generator.
Run `cargo run -p canonical-scene --example publication_memory --release`.
The baseline is commit `aa5ba2964e08acac1e40a1aac5acd0027ccf10fd` with the same measurement example added.

The example counts live requested heap bytes through the Rust global allocator.
It resets the peak after generating each input scene, then measures publication.
Peak includes the input scene, temporary allocations, and published storage.
Retained bytes include all published scene allocations, including page tables and
the material palette. These are allocation measurements, not process working set
or allocator bookkeeping measurements. No Render Path is constructed.

| Scale | Voxels | Payload before | Payload after | Retained before | Retained after | Publication peak before | Publication peak after |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 64 | 131,072 | 2,097,152 | 524,288 | 2,122,412 | 549,832 | 7,365,428 | 2,646,932 |
| 128 | 1,048,576 | 16,777,216 | 4,194,304 | 16,974,508 | 4,391,880 | 58,917,684 | 21,169,044 |
| 256 | 8,388,608 | 134,217,728 | 33,554,432 | 135,791,276 | 35,128,264 | 471,335,732 | 169,345,940 |

All byte columns use bytes, not MB. Payload is the voxel count multiplied by the
storage element size, 16 bytes before and 4 bytes after. Retained and peak columns
are measured by the example. Input allocations are unchanged at 2,097,536,
16,777,600, and 134,218,112 bytes respectively.

Publication now fills the final shared pages directly. Zero represents empty;
the largest index marks coordinates not supplied yet, preserving duplicate and
incomplete batch validation without a second volume-sized value array. Material
identities and values remain public API types. Raster material resolution uses
the scene's identity lookup map; compute-ray already builds its own lookup map.
Reading public voxel samples still clones material identities at that boundary.

Pages retain their existing 256-voxel boundaries. A changed edit copies at most
1,024 bytes of voxel payload and shares the untouched pages and scene palette.
