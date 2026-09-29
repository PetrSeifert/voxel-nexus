param(
    [string]$OutputDirectory = "docs/evidence/large-sparse/development-machine",
    [ValidateRange(3, 100)][int]$Runs = 3
)
$ErrorActionPreference = "Stop"
$repository = Split-Path $PSScriptRoot -Parent
Push-Location $repository
try {
    New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
    cargo build --release -p desktop-demo --bin large-sparse-measurement --features qualification -p measurement-evidence --bin large-sparse-report
    if ($LASTEXITCODE -ne 0) { throw "Measurement build failed" }

    # The owner reports the rejection after receiving rounded allocation requirements.
    $rejection = cargo test --release -p compute-ray-render-path --lib compute_convergence::tests::rounded_growth_allocation_rejection_preserves_visible_revision -- --exact --nocapture 2>&1
    Set-Content -Path (Join-Path $OutputDirectory "predicted-rejection.log") -Value (($rejection -join [Environment]::NewLine).TrimEnd())
    if ($LASTEXITCODE -ne 0 -or -not ($rejection -match 'test result: ok. 1 passed')) {
        throw "Predicted over-budget growth rejection was not verified"
    }
    $rejectionRecords = @($rejection | ForEach-Object { $_.ToString() } | Where-Object { $_ -match '^\{"event":"predicted_rejection",' })
    if ($rejectionRecords.Count -ne 1) { throw "Expected one observed predicted-rejection record" }
    $observedRejection = $rejectionRecords[0] | ConvertFrom-Json
    if (-not $observedRejection.rejected -or $observedRejection.predicted_bytes -le $observedRejection.budget_bytes) {
        throw "Observed growth rejection did not exceed its budget"
    }
    $qualification = cargo test --release -p desktop-demo --test large_sparse_gpu -- --ignored --nocapture 2>&1
    Set-Content -Path (Join-Path $OutputDirectory "semantic-qualification.log") -Value (($qualification -join [Environment]::NewLine).TrimEnd())
    if ($LASTEXITCODE -ne 0 -or -not ($qualification -match 'test result: ok. 1 passed')) {
        throw "Large sparse Vulkan qualification failed"
    }
    $paths = @()
    for ($run = 1; $run -le $Runs; $run++) {
        $path = Join-Path $OutputDirectory "run-$run.jsonl"
        $runOutput = & target/release/large-sparse-measurement.exe complete $path 2>&1
        $runExitCode = $LASTEXITCODE
        [System.IO.File]::WriteAllText((Join-Path (Resolve-Path $OutputDirectory) "run-$run.log"), ($runOutput -join [Environment]::NewLine))
        if ($runExitCode -ne 0) { throw "Measurement run $run failed: $runOutput" }
        $rejectionRecords[0] | Add-Content -Encoding utf8 $path
        $paths += $path
    }
    $report = & target/release/large-sparse-report.exe @paths
    if ($LASTEXITCODE -ne 0) { throw "Large sparse evidence was rejected" }
    $report | Set-Content (Join-Path $OutputDirectory "report.json")
    rustc --version | Set-Content (Join-Path $OutputDirectory "toolchain.txt")
    $report
} finally {
    Pop-Location
}
