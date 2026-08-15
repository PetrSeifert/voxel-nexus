# Windows timing evidence

The timing baseline is descriptive evidence for one Windows/Vulkan machine. It does not define a performance acceptance threshold.

From a clean, committed checkout with the Vulkan SDK environment configured, run:

```powershell
pwsh -File scripts/collect-timing-evidence.ps1
```

The collector builds `desktop-demo` in release mode, runs the canonical generation, semantic-face oracle, and measurement-contract correctness diagnostics, then performs ten fresh first-correct-frame processes at each canonical scale. It also performs one validation-disabled, `VK_PRESENT_MODE_IMMEDIATE_KHR` 1920×1080 process per scale, with a five-second warm-up followed by thirty seconds of CPU and GPU timestamp collection.

The output directory contains every JSONL sample stream, standard output and error logs, machine-readable diagnostics, SHA-256-attributed manifest, and the compact three-scale SVG comparison. The collector stops instead of silently substituting a throttled present mode or unavailable GPU timestamps.

## Render Path comparison input

The portable compute milestone uses the same machine-local claim boundary. Its focused report input has schema version 1 and retains raw timing streams, explicit comparison pairs, a resource ledger, attributed failure-retry, retirement, and shutdown events, and the traversal-selection record. Generate the checked report with:

```powershell
cargo run --locked --package measurement-evidence --bin render-path-evidence-report -- <input.json> <report.json>
```

The report command rejects comparisons unless both samples have the same machine, executable, repository revision, device, driver, queue, scene revision, camera revision, extent, presentation route, validation mode, timestamp mode, scenario, sample sequence, and clock. Only the Render Path may differ. The required comparisons are cold request to first matching frame, changed submission to final Visible revision, steady CPU frame time, and steady GPU frame time.

Every raw sample carries a stream identity. The report checks conditions, scenario, sequence, and required phase coverage within each stream, so unrelated samples cannot satisfy a stream's missing phase. It requires Raster and compute-ray streams for cold construction, steady presentation, edit convergence, and switching. Together, those streams attribute preparation, upload, installation, dispatch, composite, presentation, switching, and total time. Compute diagnostics print preparation, upload, installation, dispatch, and composite events with scene, revision, and generation attribution. The desktop harness adds presentation and switching events.

Resource input records create, transition, and destroy events by identity. Each event carries Render Path, role, scene revision, generation, ownership state, bytes, object count, allocation count, workers, and retained Voxel Scene Views. A live identity may change role or ownership state, but not its Render Path, scene, revision, generation, type, or quantity. The report rejects an empty ledger, sequence regressions, duplicate creation, destruction of unknown resources, any live identity at shutdown, and a run without simultaneous Raster and compute-ray ownership. It reports both the overall peak and the cross-path overlap peak. Failure-retry, retirement, and shutdown events must match an attributed resource generation in the ledger and retain their diagnostic identity.

Dense DDA is the default traversal selection and needs no occupancy experiment. If the input contains a fixed `16 x 16 x 16` occupancy experiment, every dense/occupancy dispatch pair must carry a unique sequence and the same attributed compute-ray machine conditions for its canonical camera. The report selects occupancy only after all correctness and lifecycle gates pass and the paired GPU-dispatch bootstrap interval is strictly above zero at the overview, cavity, and boundary cameras. Otherwise it selects dense DDA.
