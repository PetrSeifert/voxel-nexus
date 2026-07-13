[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$EvidenceDirectory,
    [ValidateRange(2, 100)]
    [int]$SampleCount = 5
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if (-not $IsWindows -and $PSVersionTable.PSEdition -eq "Core") {
    throw "Raster Region extent qualification runs only on Windows."
}

$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$evidencePath = [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot $EvidenceDirectory))
if ([System.IO.Directory]::Exists($evidencePath) -and [System.IO.Directory]::EnumerateFileSystemEntries($evidencePath).GetEnumerator().MoveNext()) {
    throw "The evidence directory must be new or empty: $evidencePath"
}
[System.IO.Directory]::CreateDirectory($evidencePath) | Out-Null

function Write-JsonFile {
    param([string]$Path, [object]$Value)
    $Value | ConvertTo-Json -Depth 12 | Set-Content -Encoding utf8 $Path
}

function Invoke-LoggedCargo {
    param([string[]]$Arguments, [string]$LogName)
    $logPath = Join-Path $evidencePath $LogName
    & cargo @Arguments *> $logPath
    if ($LASTEXITCODE -ne 0) {
        throw "cargo $($Arguments -join ' ') failed with exit code $LASTEXITCODE; see $logPath"
    }
}

function Invoke-CheckedNativeText {
    param([string]$Executable, [string[]]$Arguments)
    $output = & $Executable @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$Executable $($Arguments -join ' ') failed with exit code $LASTEXITCODE"
    }
    $output -join "`n"
}

Push-Location $repositoryRoot
try {
    Invoke-LoggedCargo -Arguments @("build", "--locked", "--package", "desktop-demo") -LogName "build.log"
    Invoke-LoggedCargo -Arguments @("test", "--locked", "--package", "raster-render-path", "--all-targets") -LogName "raster-qualification-tests.log"

    $candidateInputs = @()
    $candidateRuns = @()
    foreach ($extent in @(16, 32, 64)) {
        $candidateDirectory = Join-Path $evidencePath "extent-$extent"
        [System.IO.Directory]::CreateDirectory($candidateDirectory) | Out-Null
        $qualificationRelative = [System.IO.Path]::GetRelativePath($repositoryRoot, (Join-Path $candidateDirectory "qualification"))
        & (Join-Path $PSScriptRoot "verify-edit-burst-demo.ps1") -EvidenceDirectory $qualificationRelative -RasterRegionExtent $extent -SkipBuild
        if ($LASTEXITCODE -ne 0) {
            throw "extent $extent qualification failed with exit code $LASTEXITCODE"
        }
        $qualification = Get-Content -Raw (Join-Path $candidateDirectory "qualification\manifest.json") | ConvertFrom-Json
        $semanticCorrectnessPassed = $true
        $localizationPassed = [bool]$qualification.Qualification.Localization
        $failureRetryPassed = $true
        $lifecyclePassed = [bool]$qualification.Qualification.Lifecycle
        $shutdownPassed = [bool]$qualification.ShutdownQualification.ActiveCpuWork.Passed -and [bool]$qualification.ShutdownQualification.HiddenPostUploadCandidate.Passed
        $resourceRetirementPassed = [bool]$qualification.Qualification.ResourceRetirement
        $validationPassed = [bool]$qualification.Qualification.Validation `
            -and [int]$qualification.ShutdownQualification.ActiveCpuWork.ValidationWarnings -eq 0 `
            -and [int]$qualification.ShutdownQualification.ActiveCpuWork.ValidationErrors -eq 0 `
            -and [int]$qualification.ShutdownQualification.HiddenPostUploadCandidate.ValidationWarnings -eq 0 `
            -and [int]$qualification.ShutdownQualification.HiddenPostUploadCandidate.ValidationErrors -eq 0
        $latencySamples = @()
        $runMeasurements = @()
        $peakLiveGpuBytes = [uint64]$qualification.Measurement.PeakLiveGpuBytes
        $peakLiveGpuResources = [uint64]$qualification.Measurement.PeakLiveGpuResources
        for ($sample = 1; $sample -le $SampleCount; $sample++) {
            $sampleName = "sample-{0:D2}" -f $sample
            $sampleRelative = [System.IO.Path]::GetRelativePath($repositoryRoot, (Join-Path $candidateDirectory $sampleName))
            & (Join-Path $PSScriptRoot "verify-edit-burst-demo.ps1") -EvidenceDirectory $sampleRelative -RasterRegionExtent $extent -TimingOnly -SkipBuild
            if ($LASTEXITCODE -ne 0) {
                throw "extent $extent timing sample $sample failed with exit code $LASTEXITCODE"
            }
            $sampleManifest = Get-Content -Raw (Join-Path $candidateDirectory "$sampleName\manifest.json") | ConvertFrom-Json
            $latency = [double]$sampleManifest.Measurement.KeypressToFinalVisibleMilliseconds
            $latencySamples += $latency
            $sampleBytes = [uint64]$sampleManifest.Measurement.PeakLiveGpuBytes
            $sampleResources = [uint64]$sampleManifest.Measurement.PeakLiveGpuResources
            $peakLiveGpuBytes = [Math]::Max($peakLiveGpuBytes, $sampleBytes)
            $peakLiveGpuResources = [Math]::Max($peakLiveGpuResources, $sampleResources)
            $runMeasurements += [ordered]@{
                sample = $sample
                manifest = "extent-$extent/$sampleName/manifest.json"
                keypress_to_final_visible_milliseconds = $latency
                peak_live_gpu_bytes = $sampleBytes
                peak_live_gpu_resources = $sampleResources
            }
        }
        $candidateInputs += [ordered]@{
            extent = $extent
            qualification = [ordered]@{
                semantic_correctness = $semanticCorrectnessPassed
                localization = $localizationPassed
                failure_retry = $failureRetryPassed
                lifecycle = $lifecyclePassed
                shutdown = $shutdownPassed
                resource_retirement = $resourceRetirementPassed
                validation = $validationPassed
            }
            latency_samples_milliseconds = $latencySamples
            peak_live_gpu_bytes = $peakLiveGpuBytes
            peak_live_gpu_resources = $peakLiveGpuResources
        }
        $candidateRuns += [ordered]@{
            extent = @($extent, $extent, $extent)
            qualification_manifest = "extent-$extent/qualification/manifest.json"
            qualification_test_log = "raster-qualification-tests.log"
            gate_outcomes = [ordered]@{
                semantic_correctness = [ordered]@{ passed = $semanticCorrectnessPassed; evidence = "raster-qualification-tests.log#fixed_candidate_extents_pass_canonical_semantic_localization_and_failure_retry_gates" }
                localization = [ordered]@{ passed = $localizationPassed; evidence = "extent-$extent/qualification/manifest.json#Qualification.Localization" }
                failure_retry = [ordered]@{ passed = $failureRetryPassed; evidence = "raster-qualification-tests.log#fixed_candidate_extents_pass_canonical_semantic_localization_and_failure_retry_gates" }
                lifecycle = [ordered]@{ passed = $lifecyclePassed; evidence = "extent-$extent/qualification/manifest.json#Lifecycle" }
                shutdown = [ordered]@{ passed = $shutdownPassed; evidence = "extent-$extent/qualification/manifest.json#ShutdownQualification" }
                resource_retirement = [ordered]@{ passed = $resourceRetirementPassed; evidence = "extent-$extent/qualification/manifest.json#PostUploadBarrier" }
                validation = [ordered]@{ passed = $validationPassed; evidence = "extent-$extent/qualification/manifest.json#Validation+ShutdownQualification" }
            }
            timing_runs = $runMeasurements
        }
    }

    $selectionInput = [ordered]@{
        schema_version = 1
        candidates = $candidateInputs
    }
    $selectionInputPath = Join-Path $evidencePath "selection-input.json"
    $selectionPath = Join-Path $evidencePath "selection.json"
    Write-JsonFile -Path $selectionInputPath -Value $selectionInput
    Invoke-LoggedCargo -Arguments @(
        "run", "--locked", "--package", "measurement-evidence", "--bin", "raster-region-extent-report", "--",
        $selectionInputPath, $selectionPath
    ) -LogName "selection-report.log"
    $selection = Get-Content -Raw $selectionPath | ConvertFrom-Json

    $machine = [ordered]@{
        operating_system = (Get-CimInstance Win32_OperatingSystem | Select-Object Caption, Version, BuildNumber)
        processors = @(Get-CimInstance Win32_Processor | Select-Object Name, Manufacturer, NumberOfCores, NumberOfLogicalProcessors)
        video_controllers = @(Get-CimInstance Win32_VideoController | Select-Object Name, DriverVersion)
        powershell = $PSVersionTable.PSVersion.ToString()
        rustc = Invoke-CheckedNativeText -Executable "rustc" -Arguments @("-Vv")
        cargo = Invoke-CheckedNativeText -Executable "cargo" -Arguments @("-V")
    }
    $manifest = [ordered]@{
        schema_version = 1
        scope = "Descriptive Raster Region extent comparison for this recorded Windows development machine only."
        recorded_at_utc = [DateTime]::UtcNow.ToString("o")
        repository_revision = (Invoke-CheckedNativeText -Executable "git" -Arguments @("rev-parse", "HEAD")).Trim()
        canonical_input = [ordered]@{
            scene = "canonical-dense-scene"
            generator = "voxel-nexus-canonical-dense"
            generator_version = 1
            dimensions = @(256, 128, 256)
            camera = "overview"
            initial_revision = 1
            expected_final_revision = 4
            commands = @(
                [ordered]@{ order = 1; coordinate = @(0, 0, 0); old = "empty"; requested = "occupied:canonical-warm" },
                [ordered]@{ order = 2; coordinate = @(40, 0, 0); old = "empty"; requested = "occupied:canonical-warm" },
                [ordered]@{ order = 3; coordinate = @(80, 0, 0); old = "empty"; requested = "occupied:canonical-warm" }
            )
        }
        build = [ordered]@{ command = "cargo build --locked --package desktop-demo"; log = "build.log" }
        common_qualification = [ordered]@{ command = "cargo test --locked --package raster-render-path --all-targets"; log = "raster-qualification-tests.log" }
        sample_count_per_candidate = $SampleCount
        candidate_runs = $candidateRuns
        selection_input = "selection-input.json"
        selection_report = "selection.json"
        selected_extent = @([int]$selection.selected_extent, [int]$selection.selected_extent, [int]$selection.selected_extent)
        machine = $machine
    }
    Write-JsonFile -Path (Join-Path $evidencePath "manifest.json") -Value $manifest
    Write-Host "Raster Region extent qualification passed. Selected $($selection.selected_extent)^3. Evidence: $evidencePath"
}
finally {
    Pop-Location
}
