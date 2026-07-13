[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$EvidenceDirectory,
    [string]$SelectionManifest = "artifacts/raster-region-extent-selection-issue-47/manifest.json",
    [ValidateRange(2, 100)]
    [int]$SampleCount = 5
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if (-not $IsWindows -and $PSVersionTable.PSEdition -eq "Core") {
    throw "Raster Region scale characterization runs only on Windows."
}

$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$evidencePath = [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot $EvidenceDirectory))
if ([System.IO.Directory]::Exists($evidencePath) -and [System.IO.Directory]::EnumerateFileSystemEntries($evidencePath).GetEnumerator().MoveNext()) {
    throw "The evidence directory must be new or empty: $evidencePath"
}
[System.IO.Directory]::CreateDirectory($evidencePath) | Out-Null

function Invoke-CheckedNativeText {
    param([string]$Executable, [string[]]$Arguments)
    $output = & $Executable @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$Executable $($Arguments -join ' ') failed with exit code $LASTEXITCODE"
    }
    $output -join "`n"
}

function Write-JsonFile {
    param([string]$Path, [object]$Value)
    $Value | ConvertTo-Json -Depth 16 | Set-Content -Encoding utf8 $Path
}

function Get-Values {
    param([object[]]$Samples, [scriptblock]$Selector)
    @($Samples | ForEach-Object $Selector)
}

Push-Location $repositoryRoot
try {
    $selectionManifestPath = [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot $SelectionManifest))
    if (-not [System.IO.File]::Exists($selectionManifestPath)) {
        throw "The extent selection manifest does not exist: $selectionManifestPath"
    }
    $selectionManifestValue = Get-Content -Raw $selectionManifestPath | ConvertFrom-Json
    if ([int]$selectionManifestValue.schema_version -ne 1) {
        throw "Unsupported extent selection manifest schema version $($selectionManifestValue.schema_version)."
    }
    $selectionReportPath = Join-Path (Split-Path $selectionManifestPath) $selectionManifestValue.selection_report
    $selectionReport = Get-Content -Raw $selectionReportPath | ConvertFrom-Json
    if ([int]$selectionReport.schema_version -ne 1) {
        throw "Unsupported extent selection report schema version $($selectionReport.schema_version)."
    }
    $selectedExtent = [int]$selectionReport.selected_extent
    $manifestExtents = @($selectionManifestValue.selected_extent | ForEach-Object { [int]$_ })
    if ($manifestExtents.Count -ne 3 -or @($manifestExtents | Where-Object { $_ -ne $selectedExtent }).Count -ne 0) {
        throw "The selection report and manifest disagree about the selected Raster Region extent."
    }
    if ($selectedExtent -notin @(16, 32, 64)) {
        throw "The selected Raster Region extent $selectedExtent is not a qualified fixed candidate."
    }

    $buildLog = Join-Path $evidencePath "build.log"
    & cargo build --locked --package desktop-demo *> $buildLog
    if ($LASTEXITCODE -ne 0) {
        throw "The shared desktop-demo build failed with exit code $LASTEXITCODE; see $buildLog"
    }
    $repositoryRevision = (Invoke-CheckedNativeText -Executable "git" -Arguments @("rev-parse", "HEAD")).Trim()
    $binaryPath = Join-Path $repositoryRoot "target/debug/desktop-demo.exe"
    $binarySha256 = (Get-FileHash -LiteralPath $binaryPath -Algorithm SHA256).Hash.ToLowerInvariant()

    $scaleDefinitions = @(
        [ordered]@{ scale = 64; dimensions = @(64, 32, 64) },
        [ordered]@{ scale = 128; dimensions = @(128, 64, 128) },
        [ordered]@{ scale = 256; dimensions = @(256, 128, 256) }
    )
    $scaleReports = @()
    foreach ($scaleDefinition in $scaleDefinitions) {
        $scale = [int]$scaleDefinition.scale
        $scaleDirectory = Join-Path $evidencePath "scale-$scale"
        [System.IO.Directory]::CreateDirectory($scaleDirectory) | Out-Null
        $samples = @()
        for ($sample = 1; $sample -le $SampleCount; $sample++) {
            $sampleName = "sample-{0:D2}" -f $sample
            $samplePath = Join-Path $scaleDirectory $sampleName
            $sampleRelative = [System.IO.Path]::GetRelativePath($repositoryRoot, $samplePath)
            & (Join-Path $PSScriptRoot "verify-edit-burst-demo.ps1") `
                -EvidenceDirectory $sampleRelative `
                -RasterRegionExtent $selectedExtent `
                -SceneScale $scale `
                -TimingOnly `
                -SkipBuild
            if ($LASTEXITCODE -ne 0) {
                throw "Scale $scale sample $sample failed with exit code $LASTEXITCODE."
            }
            $sampleManifestPath = Join-Path $samplePath "manifest.json"
            $sampleManifest = Get-Content -Raw $sampleManifestPath | ConvertFrom-Json
            if ([int]$sampleManifest.SchemaVersion -ne 1 -or [int]$sampleManifest.CanonicalInput.Scale -ne $scale) {
                throw "Scale $scale sample $sample has an inconsistent manifest schema or scale."
            }
            if ([int]$sampleManifest.CanonicalInput.RasterRegionExtent[0] -ne $selectedExtent) {
                throw "Scale $scale sample $sample did not use the selected Raster Region extent."
            }
            if ($sampleManifest.RepositoryRevision -ne $repositoryRevision) {
                throw "Scale $scale sample $sample used repository revision $($sampleManifest.RepositoryRevision), expected $repositoryRevision."
            }
            if ($sampleManifest.Validation.Warnings -ne 0 -or $sampleManifest.Validation.Errors -ne 0 -or $sampleManifest.ProcessExitCode -ne 0) {
                throw "Scale $scale sample $sample did not complete cleanly with zero validation diagnostics."
            }
            $currentBinarySha256 = (Get-FileHash -LiteralPath $binaryPath -Algorithm SHA256).Hash.ToLowerInvariant()
            if ($currentBinarySha256 -ne $binarySha256) {
                throw "The shared desktop-demo binary changed during scale characterization."
            }
            $samples += [ordered]@{
                sample = $sample
                manifest = "scale-$scale/$sampleName/manifest.json"
                phases_milliseconds = [ordered]@{
                    submission_bookkeeping = [double]$sampleManifest.Measurement.Phases.SubmissionBookkeepingMilliseconds
                    queued_wait = [double]$sampleManifest.Measurement.Phases.QueuedWaitMilliseconds
                    cpu_derivation = [double]$sampleManifest.Measurement.Phases.CpuDerivationMilliseconds
                    upload = [double]$sampleManifest.Measurement.Phases.UploadMilliseconds
                    frame_boundary_commit = [double]$sampleManifest.Measurement.Phases.FrameBoundaryCommitMilliseconds
                    keypress_to_final_visible = [double]$sampleManifest.Measurement.KeypressToFinalVisibleMilliseconds
                }
                work_disposition = [ordered]@{
                    scheduled_regions = [uint64]$sampleManifest.Measurement.WorkDisposition.ScheduledRegions
                    completed_regions = [uint64]$sampleManifest.Measurement.WorkDisposition.CompletedRegions
                    cancelled_regions = [uint64]$sampleManifest.Measurement.WorkDisposition.CancelledRegions
                    stale_regions = [uint64]$sampleManifest.Measurement.WorkDisposition.StaleRegions
                }
                resources = [ordered]@{
                    installed = $sampleManifest.Measurement.Resources.Installed
                    hidden = $sampleManifest.Measurement.Resources.Hidden
                    retired = $sampleManifest.Measurement.Resources.Retired
                    peak = $sampleManifest.Measurement.Resources.Peak
                }
                cancellation_observations = @($sampleManifest.Measurement.CancellationObservations)
                safe_retirement_events = @($sampleManifest.Measurement.SafeRetirementEvents)
            }
        }
        $scaleReports += [ordered]@{
            scale = $scale
            dimensions = $scaleDefinition.dimensions
            samples = $samples
            raw_distributions = [ordered]@{
                phases_milliseconds = [ordered]@{
                    submission_bookkeeping = Get-Values $samples { $_.phases_milliseconds.submission_bookkeeping }
                    queued_wait = Get-Values $samples { $_.phases_milliseconds.queued_wait }
                    cpu_derivation = Get-Values $samples { $_.phases_milliseconds.cpu_derivation }
                    upload = Get-Values $samples { $_.phases_milliseconds.upload }
                    frame_boundary_commit = Get-Values $samples { $_.phases_milliseconds.frame_boundary_commit }
                    keypress_to_final_visible = Get-Values $samples { $_.phases_milliseconds.keypress_to_final_visible }
                }
                work_disposition = [ordered]@{
                    scheduled_regions = Get-Values $samples { $_.work_disposition.scheduled_regions }
                    completed_regions = Get-Values $samples { $_.work_disposition.completed_regions }
                    cancelled_regions = Get-Values $samples { $_.work_disposition.cancelled_regions }
                    stale_regions = Get-Values $samples { $_.work_disposition.stale_regions }
                }
                resource_bytes = [ordered]@{
                    installed = Get-Values $samples { $_.resources.installed.Bytes }
                    hidden = Get-Values $samples { $_.resources.hidden.Bytes }
                    retired = Get-Values $samples { $_.resources.retired.Bytes }
                    peak = Get-Values $samples { $_.resources.peak.Bytes }
                }
                resource_counts = [ordered]@{
                    installed = Get-Values $samples { $_.resources.installed.Resources }
                    hidden = Get-Values $samples { $_.resources.hidden.Resources }
                    retired = Get-Values $samples { $_.resources.retired.Resources }
                    peak = Get-Values $samples { $_.resources.peak.Resources }
                }
                cancellation_observations = Get-Values $samples { @($_.cancellation_observations) }
                safe_retirement_events = Get-Values $samples { @($_.safe_retirement_events) }
            }
        }
    }

    $rawReport = [ordered]@{
        schema_version = 1
        scope = "Raw descriptive Raster Region scale distributions for this recorded Windows development machine only; not a production budget, performance gate, or cross-machine claim."
        selected_extent = @($selectedExtent, $selectedExtent, $selectedExtent)
        sample_count_per_scale = $SampleCount
        scales = $scaleReports
    }
    Write-JsonFile -Path (Join-Path $evidencePath "raw-distributions.json") -Value $rawReport

    $manifest = [ordered]@{
        schema_version = 1
        scope = $rawReport.scope
        recorded_at_utc = [DateTime]::UtcNow.ToString("o")
        repository_revision = $repositoryRevision
        selected_extent_source = [ordered]@{
            manifest = [System.IO.Path]::GetRelativePath($repositoryRoot, $selectionManifestPath).Replace("\", "/")
            manifest_sha256 = (Get-FileHash -LiteralPath $selectionManifestPath -Algorithm SHA256).Hash.ToLowerInvariant()
            report = [System.IO.Path]::GetRelativePath($repositoryRoot, $selectionReportPath).Replace("\", "/")
            report_sha256 = (Get-FileHash -LiteralPath $selectionReportPath -Algorithm SHA256).Hash.ToLowerInvariant()
            selected_extent = @($selectedExtent, $selectedExtent, $selectedExtent)
        }
        shared_build = [ordered]@{
            command = "cargo build --locked --package desktop-demo"
            log = "build.log"
            binary = "target/debug/desktop-demo.exe"
            binary_sha256 = $binarySha256
        }
        sample_count_per_scale = $SampleCount
        scales = $scaleDefinitions
        raw_distributions = "raw-distributions.json"
        machine = [ordered]@{
            operating_system = (Get-CimInstance Win32_OperatingSystem | Select-Object Caption, Version, BuildNumber)
            processors = @(Get-CimInstance Win32_Processor | Select-Object Name, Manufacturer, NumberOfCores, NumberOfLogicalProcessors)
            video_controllers = @(Get-CimInstance Win32_VideoController | Select-Object Name, DriverVersion)
            powershell = $PSVersionTable.PSVersion.ToString()
            rustc = Invoke-CheckedNativeText -Executable "rustc" -Arguments @("-Vv")
            cargo = Invoke-CheckedNativeText -Executable "cargo" -Arguments @("-V")
        }
    }
    Write-JsonFile -Path (Join-Path $evidencePath "manifest.json") -Value $manifest
    Write-Host "Raster Region scale characterization passed for selected extent $selectedExtent. Evidence: $evidencePath"
}
finally {
    Pop-Location
}
