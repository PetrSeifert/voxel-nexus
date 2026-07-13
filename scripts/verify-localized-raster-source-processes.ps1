[CmdletBinding()]
param(
    [string]$EvidenceDirectory = "artifacts/localized-raster-source-processes-issue-49",
    [string]$ExtentEvidence = "artifacts/raster-region-extent-selection-issue-47"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$artifactsRoot = [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot "artifacts"))
$evidencePath = [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot $EvidenceDirectory))
if (-not $evidencePath.StartsWith($artifactsRoot + [System.IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Source-process verification evidence must remain under $artifactsRoot"
}
if ([System.IO.Directory]::Exists($evidencePath) -and [System.IO.Directory]::EnumerateFileSystemEntries($evidencePath).GetEnumerator().MoveNext()) {
    throw "The evidence directory must be new or empty: $evidencePath"
}
[System.IO.Directory]::CreateDirectory($evidencePath) | Out-Null

$extentPath = [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot $ExtentEvidence))
$selectionInput = Join-Path $extentPath "selection-input.json"
$retainedSelection = Join-Path $extentPath "selection.json"
if (-not [System.IO.File]::Exists($selectionInput) -or -not [System.IO.File]::Exists($retainedSelection)) {
    throw "Extent selection input or retained report is missing under $extentPath"
}

$buildCommand = "cargo build --locked --package desktop-demo"
& cargo build --locked --package desktop-demo *> (Join-Path $evidencePath "desktop-build.log")
$buildExitCode = $LASTEXITCODE

$testCommand = "cargo test --locked --package raster-render-path --all-targets"
& cargo test --locked --package raster-render-path --all-targets *> (Join-Path $evidencePath "raster-qualification-tests.log")
$testExitCode = $LASTEXITCODE

$reproducedSelection = Join-Path $evidencePath "selection-reproduced.json"
$selectionCommand = "cargo run --locked --package measurement-evidence --bin raster-region-extent-report -- $selectionInput $reproducedSelection"
& cargo run --locked --package measurement-evidence --bin raster-region-extent-report -- $selectionInput $reproducedSelection *> (Join-Path $evidencePath "extent-selection.log")
$selectionExitCode = $LASTEXITCODE

$selectionComparisonExitCode = 1
if ($selectionExitCode -eq 0 -and [System.IO.File]::Exists($reproducedSelection)) {
    $retainedHash = (Get-FileHash -LiteralPath $retainedSelection -Algorithm SHA256).Hash
    $reproducedHash = (Get-FileHash -LiteralPath $reproducedSelection -Algorithm SHA256).Hash
    if ($retainedHash -eq $reproducedHash) {
        $selectionComparisonExitCode = 0
    }
}

$repositoryRevision = & git -C $repositoryRoot rev-parse HEAD
if ($LASTEXITCODE -ne 0) {
    throw "git rev-parse HEAD failed with exit code $LASTEXITCODE"
}
$processOutcomes = [ordered]@{
    schema_version = 1
    recorded_at_utc = [DateTime]::UtcNow.ToString("o")
    repository_revision = ($repositoryRevision -join "`n").Trim()
    processes = @(
        [ordered]@{
            name = "desktop_build"
            command = $buildCommand
            exit_code = [int]$buildExitCode
            log = "desktop-build.log"
        },
        [ordered]@{
            name = "raster_qualification_tests"
            command = $testCommand
            exit_code = [int]$testExitCode
            log = "raster-qualification-tests.log"
        },
        [ordered]@{
            name = "extent_selection"
            command = $selectionCommand
            exit_code = [int]$selectionExitCode
            log = "extent-selection.log"
        },
        [ordered]@{
            name = "selection_comparison"
            command = "SHA-256 compare reproduced selection.json with retained selection.json"
            exit_code = [int]$selectionComparisonExitCode
            log = "selection-reproduced.json"
        }
    )
}
$processOutcomes | ConvertTo-Json -Depth 8 | Set-Content -Encoding utf8 (Join-Path $evidencePath "process-outcomes.json")

$failedProcess = @($processOutcomes.processes | Where-Object { $_.exit_code -ne 0 } | Select-Object -First 1)
if ($failedProcess.Count -ne 0) {
    throw "Required process $($failedProcess[0].name) failed with exit code $($failedProcess[0].exit_code)."
}
Write-Host "Localized raster source-process verification passed. Evidence: $evidencePath"
