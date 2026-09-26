# Compute DDA GPU regression

Run the installed Render Path regression on Windows with a Vulkan 1.3 GPU and the Vulkan validation layer:

```powershell
cargo test --locked --features qualification -p desktop-demo --test compute_dda_gpu -- --ignored --nocapture
```

The test creates a hidden presentation window, installs the compute Render Path, dispatches its production shader, and reads observations through `ComputeSemanticRayController`. It compares scene identity, revision, hit or miss, voxel coordinate, material, contact classification, normal, and distance with the independent Semantic Ray oracle. Vulkan validation errors fail the test. Ordinary workspace tests skip this hardware-dependent test.

The fixtures cover issue #76's 2 x 2 x 1 volume, positive and negative near-diagonal crossings in XY, YZ, and ZX, and voxel sizes 0.125, 1, and 16. True simultaneous crossings must skip cells touched for zero distance and preserve normal priority. Additional probes check half-open bounds and clipping.

On the NVIDIA GeForce RTX 4070, driver 0x94d84000, the original tolerance-based shader returned a miss for the unit-scale positive XY fixture while the oracle returned coordinate (1, 0, 0) at distance 0.7071067811865476. Requiring equal computed crossing distances fixes that regression. Distance tolerance in the comparison permits floating-point distance error only; identities and contact classifications must match exactly.
