# Portable compute-ray evidence

The versioned bundle under `docs/evidence/portable-compute-ray/v1` records the final milestone proof on one attributed Windows development machine.

Collect a new bundle from a clean committed checkout:

```powershell
pwsh -NoProfile -File scripts/capture-portable-compute-ray-evidence.ps1 -EvidenceDirectory docs/evidence/portable-compute-ray/v1/reproduction
```

Verify a retained bundle and every artifact hash:

```powershell
cargo run --locked --package portable-compute-ray-evidence --bin verify-portable-compute-ray-evidence -- docs/evidence/portable-compute-ray/v1/development-machine
```

The verifier requires machine and executable attribution, the fixed revision sequence, exact semantic outcomes, balanced final ownership, zero workers, zero validation findings, dense DDA selection, five named stills, the uninterrupted clip, raw timing and resource files, and the supporting qualification logs. It rejects changed hashes and any superiority or cross-machine claim.
