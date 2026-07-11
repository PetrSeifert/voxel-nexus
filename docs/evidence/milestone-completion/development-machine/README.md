# Dense raster Voxel Scene completion evidence

Runtime and timing claims in this bundle apply only to the recorded Windows development machine in the nested manifests. They are descriptive evidence, not portable correctness or performance claims.

From a clean committed checkout with the Vulkan SDK and VK_LAYER_KHRONOS_validation available, reproduce the complete proof into a new directory:

`powershell
pwsh -NoProfile -File scripts/capture-milestone-completion.ps1 -EvidenceDirectory docs/evidence/milestone-completion/reproduction
`

Verify this retained bundle and every inventoried SHA-256 hash:

`powershell
cargo run --locked --package completion-evidence --bin verify-completion-evidence -- docs/evidence/milestone-completion/development-machine
`

Decode the uninterrupted clip independently:

`powershell
ffmpeg -v error -i docs/evidence/milestone-completion/development-machine/lifecycle/milestone-proof.mkv -map 0:v:0 -f null NUL
`

checks/clean-checkout-summary.json records generated-artifact build, formatting, strict lint, workspace unit/integration, Voxel Frontend read, diagnostic surface, lifecycle, deterministic-failure, prerequisite, and timing commands with their raw output logs. The top-level summaries cross-link the nested lifecycle and timing manifests, 30 first-correct-frame streams, three CPU/GPU streams, geometry/resource counts, and comparison chart.
