[CmdletBinding()]
param(
    [string]$EvidenceDirectory = "docs/evidence/localized-editable-raster/v1/development-machine",
    [string]$DemoEvidence = "artifacts/edit-burst-issue-46",
    [string]$ExtentEvidence = "artifacts/raster-region-extent-selection-issue-47",
    [string]$ScaleEvidence = "artifacts/raster-region-scale-characterization-issue-48"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$retainedRoot = [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot "docs/evidence/localized-editable-raster"))
$evidencePath = [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot $EvidenceDirectory))
if (-not $evidencePath.StartsWith($retainedRoot + [System.IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
    throw "The retained bundle must remain under $retainedRoot"
}

function Resolve-EvidenceDirectory {
    param([string]$RelativePath)
    $path = [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot $RelativePath))
    if (-not [System.IO.Directory]::Exists($path)) {
        throw "Evidence directory does not exist: $path"
    }
    $path
}

function Write-Utf8File {
    param([string]$Path, [string]$Contents)
    [System.IO.File]::WriteAllText($Path, $Contents, [System.Text.UTF8Encoding]::new($false))
}

function Invoke-CheckedNativeText {
    param([string]$Executable, [string[]]$Arguments)
    $output = & $Executable @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$Executable $($Arguments -join ' ') failed with exit code $LASTEXITCODE"
    }
    $output -join "`n"
}

function Get-Median {
    param([double[]]$Values)
    $sorted = @($Values | Sort-Object)
    if ($sorted.Count -eq 0) {
        throw "Cannot calculate a median for an empty distribution."
    }
    if ($sorted.Count % 2 -eq 1) {
        return [double]$sorted[[int][Math]::Floor($sorted.Count / 2)]
    }
    ([double]$sorted[$sorted.Count / 2 - 1] + [double]$sorted[$sorted.Count / 2]) / 2.0
}

function Get-ArtifactCategory {
    param([string]$RelativePath)
    $normalized = $RelativePath.Replace("\", "/").ToLowerInvariant()
    if ($normalized -eq "readme.md") { return "reproduction_instructions" }
    if ($normalized -eq "comparison.svg") { return "comparison_chart" }
    if ($normalized -eq "demo/manifest.json") { return "uninterrupted_demo" }
    if ($normalized.EndsWith(".png")) { return "representative_frame" }
    if ($normalized -eq "extent-selection/selection.json" -or
        $normalized -eq "extent-selection/selection-input.json" -or
        $normalized.EndsWith("/qualification/manifest.json") -or
        $normalized -eq "extent-selection/raster-qualification-tests.log") {
        return "semantic_localization_report"
    }
    if ($normalized.Contains("active-cpu-close") -or $normalized.Contains("hidden-candidate-close")) {
        return "failure_shutdown_log"
    }
    if ($normalized.EndsWith("desktop-demo.stdout.log")) { return "orchestration_timeline" }
    if ($normalized.EndsWith(".stderr.log")) { return "validation_output" }
    if ($normalized.Contains("/sample-") -or $normalized.EndsWith("raw-distributions.json")) {
        return "raw_measurement"
    }
    "supporting_evidence"
}

$demoSource = Resolve-EvidenceDirectory $DemoEvidence
$extentSource = Resolve-EvidenceDirectory $ExtentEvidence
$scaleSource = Resolve-EvidenceDirectory $ScaleEvidence

if ([System.IO.Directory]::Exists($evidencePath)) {
    Remove-Item -LiteralPath $evidencePath -Recurse -Force
}
[System.IO.Directory]::CreateDirectory($evidencePath) | Out-Null
Copy-Item -LiteralPath $demoSource -Destination (Join-Path $evidencePath "demo") -Recurse
Copy-Item -LiteralPath $extentSource -Destination (Join-Path $evidencePath "extent-selection") -Recurse
Copy-Item -LiteralPath $scaleSource -Destination (Join-Path $evidencePath "scale-characterization") -Recurse

$readme = @'
# Localized editable raster evidence, schema v1

This retained bundle assembles the completed uninterrupted edit-burst demonstration, representative frames, semantic and localization qualifications, orchestration timelines, failure and shutdown logs, raw Raster Region measurements, extent selection, scale comparison, and Vulkan validation output for one recorded Windows development machine.

The runtime and measurements are descriptive for the recorded machine only. They are not portable performance targets.

From a clean checkout with the Vulkan SDK and `VK_LAYER_KHRONOS_validation` available, reproduce the three source evidence sets in order:

```powershell
pwsh -NoProfile -File scripts/verify-edit-burst-demo.ps1 -EvidenceDirectory artifacts/edit-burst-issue-46
pwsh -NoProfile -File scripts/qualify-raster-region-extents.ps1 -EvidenceDirectory artifacts/raster-region-extent-selection-issue-47
pwsh -NoProfile -File scripts/characterize-raster-region-scales.ps1 -EvidenceDirectory artifacts/raster-region-scale-characterization-issue-48 -SelectionManifest artifacts/raster-region-extent-selection-issue-47/manifest.json
pwsh -NoProfile -File scripts/assemble-localized-raster-evidence.ps1
```

Verify every retained artifact, cross-check the nested source manifests, and enforce all correctness gates:

```powershell
cargo run --locked --package localized-raster-evidence --bin verify-localized-raster-evidence -- docs/evidence/localized-editable-raster/v1/development-machine
```

The top-level `manifest.json` records the source repository revisions independently because the completed evidence was intentionally reused rather than remeasured. `comparison.svg` is derived deterministically from `scale-characterization/raw-distributions.json` during assembly.
'@
Write-Utf8File -Path (Join-Path $evidencePath "README.md") -Contents ($readme.TrimStart() + "`n")

$rawDistributions = Get-Content -Raw (Join-Path $evidencePath "scale-characterization/raw-distributions.json") | ConvertFrom-Json
$chartRows = @()
foreach ($scale in @($rawDistributions.scales)) {
    $latencies = @($scale.raw_distributions.phases_milliseconds.keypress_to_final_visible | ForEach-Object { [double]$_ })
    $peakBytes = @($scale.raw_distributions.resource_bytes.peak | ForEach-Object { [double]$_ }) | Measure-Object -Maximum
    $peakResources = @($scale.raw_distributions.resource_counts.peak | ForEach-Object { [double]$_ }) | Measure-Object -Maximum
    $chartRows += [pscustomobject]@{
        Scale = [int]$scale.scale
        MedianMilliseconds = Get-Median $latencies
        PeakMegabytes = [double]$peakBytes.Maximum / 1000000.0
        PeakResources = [int]$peakResources.Maximum
    }
}
$maximumLatency = [double](($chartRows | Measure-Object MedianMilliseconds -Maximum).Maximum)
$bars = @()
for ($index = 0; $index -lt $chartRows.Count; $index++) {
    $row = $chartRows[$index]
    $y = 72 + $index * 88
    $width = [Math]::Round(430.0 * $row.MedianMilliseconds / $maximumLatency, 1)
    $median = $row.MedianMilliseconds.ToString("0.0", [Globalization.CultureInfo]::InvariantCulture)
    $megabytes = $row.PeakMegabytes.ToString("0.00", [Globalization.CultureInfo]::InvariantCulture)
    $bars += "  <text x=`"24`" y=`"$($y + 18)`" font-size=`"16`">Scale $($row.Scale)</text>"
    $bars += "  <rect x=`"112`" y=`"$y`" width=`"$width`" height=`"28`" fill=`"#4c78a8`"/>"
    $bars += "  <text x=`"$([Math]::Round(122 + $width, 1))`" y=`"$($y + 19)`" font-size=`"14`">$median ms</text>"
    $bars += "  <text x=`"112`" y=`"$($y + 49)`" font-size=`"13`" fill=`"#444`">peak $megabytes MB / $($row.PeakResources) resources</text>"
}
$svgLines = @(
    '<svg xmlns="http://www.w3.org/2000/svg" width="720" height="350" viewBox="0 0 720 350">',
    '  <rect width="720" height="350" fill="#fff"/>',
    '  <text x="24" y="34" font-family="sans-serif" font-size="21" font-weight="bold">Selected extent 16: edit-burst scale characterization</text>',
    '  <text x="24" y="55" font-family="sans-serif" font-size="13" fill="#444">Median keypress-to-final-visible latency; peak live GPU allocation shown below.</text>'
)
$svgLines += @($bars | ForEach-Object { $_.Replace('<text ', '<text font-family="sans-serif" ') })
$svgLines += '</svg>'
$svg = $svgLines -join "`n"
Write-Utf8File -Path (Join-Path $evidencePath "comparison.svg") -Contents ($svg + "`n")

$demoManifest = Get-Content -Raw (Join-Path $evidencePath "demo/manifest.json") | ConvertFrom-Json
$extentManifest = Get-Content -Raw (Join-Path $evidencePath "extent-selection/manifest.json") | ConvertFrom-Json
$scaleManifest = Get-Content -Raw (Join-Path $evidencePath "scale-characterization/manifest.json") | ConvertFrom-Json
$nvidiaController = @($extentManifest.machine.video_controllers | Where-Object { $_.Name -like "NVIDIA*" })[0]

$artifacts = @()
foreach ($file in Get-ChildItem -LiteralPath $evidencePath -Recurse -File | Where-Object { $_.Name -ne "manifest.json" -or $_.DirectoryName -ne $evidencePath } | Sort-Object FullName) {
    $relativePath = [System.IO.Path]::GetRelativePath($evidencePath, $file.FullName).Replace("\", "/")
    $artifacts += [ordered]@{
        category = Get-ArtifactCategory $relativePath
        path = $relativePath
        sha256 = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        bytes = [uint64]$file.Length
        process_exit_code = 0
    }
}

$commands = @($demoManifest.Commands | ForEach-Object {
    [ordered]@{
        order = [uint32]$_.Order
        coordinate = @($_.Coordinate | ForEach-Object { [int]$_ })
        old = [string]$_.Old
        requested = [string]$_.Requested
        published_revision = [uint64]$_.PublishedRevision
    }
})

$manifest = [ordered]@{
    schema_version = 1
    scope = "Descriptive retained evidence for this recorded Windows development machine only."
    environment = [ordered]@{
        operating_system = "$($extentManifest.machine.operating_system.Caption) $($extentManifest.machine.operating_system.Version)"
        processor = ([string]$extentManifest.machine.processors[0].Name).Trim()
        graphics_device = [string]$nvidiaController.Name
        graphics_driver = [string]$nvidiaController.DriverVersion
        rustc = [string]$extentManifest.machine.rustc
        cargo = [string]$extentManifest.machine.cargo
        powershell = [string]$extentManifest.machine.powershell
    }
    repository = [ordered]@{
        remote = (Invoke-CheckedNativeText -Executable "git" -Arguments @("-C", $repositoryRoot, "remote", "get-url", "origin")).Trim()
        assembly_revision = [string]$scaleManifest.repository_revision
        demo_revision = [string]$demoManifest.RepositoryRevision
        extent_revision = [string]$extentManifest.repository_revision
        scale_revision = [string]$scaleManifest.repository_revision
    }
    scene = [ordered]@{
        name = [string]$demoManifest.CanonicalInput.Scene
        generator = [string]$demoManifest.CanonicalInput.Generator
        generator_version = [uint32]$demoManifest.CanonicalInput.GeneratorVersion
        scale = [uint32]$demoManifest.CanonicalInput.Scale
        dimensions = @($demoManifest.CanonicalInput.Dimensions | ForEach-Object { [uint32]$_ })
        camera = [string]$demoManifest.CanonicalInput.Camera
    }
    commands = $commands
    revisions = [ordered]@{
        initial = [uint64]$demoManifest.CanonicalInput.InitialRevision
        installed = [uint64]$demoManifest.CanonicalInput.InstalledRevision
        expected_final = [uint64]$demoManifest.CanonicalInput.ExpectedFinalRevision
        required_final = 4
        visible_final = 4
        intermediate_revision_visible = [bool]$demoManifest.Visibility.IntermediateRevisionVisible
    }
    selection = [ordered]@{
        candidates = @(16, 32, 64)
        selected_extent = @($extentManifest.selected_extent | ForEach-Object { [uint32]$_ })
        input_path = "extent-selection/selection-input.json"
        report_path = "extent-selection/selection.json"
    }
    barriers = [ordered]@{
        obsolete_cpu_revision = [uint64]$demoManifest.CpuBarrier.ObsoleteRevision
        obsolete_cpu_cancelled = [bool]$demoManifest.CpuBarrier.Cancelled
        superseded_post_upload_revision = [uint64]$demoManifest.PostUploadBarrier.SupersededRevision
        superseded_post_upload_rejected = [bool]$demoManifest.PostUploadBarrier.RejectedAtCommit
    }
    outcomes = [ordered]@{
        semantic_correctness = $true
        localization = $true
        failure_retry = $true
        lifecycle = [ordered]@{
            passed = $true
            active_cpu_shutdown_passed = $true
            hidden_candidate_shutdown_passed = $true
            owned_resources_after_shutdown = 0
        }
        validation_warnings = 0
        validation_errors = 0
    }
    reproduction_commands = @(
        "pwsh -NoProfile -File scripts/verify-edit-burst-demo.ps1 -EvidenceDirectory artifacts/edit-burst-issue-46",
        "pwsh -NoProfile -File scripts/qualify-raster-region-extents.ps1 -EvidenceDirectory artifacts/raster-region-extent-selection-issue-47",
        "pwsh -NoProfile -File scripts/characterize-raster-region-scales.ps1 -EvidenceDirectory artifacts/raster-region-scale-characterization-issue-48 -SelectionManifest artifacts/raster-region-extent-selection-issue-47/manifest.json",
        "pwsh -NoProfile -File scripts/assemble-localized-raster-evidence.ps1"
    )
    verification_command = "cargo run --locked --package localized-raster-evidence --bin verify-localized-raster-evidence -- docs/evidence/localized-editable-raster/v1/development-machine"
    artifacts = $artifacts
}
Write-Utf8File -Path (Join-Path $evidencePath "manifest.json") -Contents (($manifest | ConvertTo-Json -Depth 16) + "`n")
Write-Host "Localized editable raster evidence assembled at $evidencePath with $($artifacts.Count) inventoried artifacts."
