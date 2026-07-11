# Windows timing evidence

The timing baseline is descriptive evidence for one Windows/Vulkan machine. It does not define a performance acceptance threshold.

From a clean, committed checkout with the Vulkan SDK environment configured, run:

```powershell
pwsh -File scripts/collect-timing-evidence.ps1
```

The collector builds `desktop-demo` in release mode, runs the canonical generation, semantic-face oracle, and measurement-contract correctness diagnostics, then performs ten fresh first-correct-frame processes at each canonical scale. It also performs one validation-disabled, `VK_PRESENT_MODE_IMMEDIATE_KHR` 1920×1080 process per scale, with a five-second warm-up followed by thirty seconds of CPU and GPU timestamp collection.

The output directory contains every JSONL sample stream, standard output and error logs, machine-readable diagnostics, SHA-256-attributed manifest, and the compact three-scale SVG comparison. The collector stops instead of silently substituting a throttled present mode or unavailable GPU timestamps.
