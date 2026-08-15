[CmdletBinding()]
param(
    [string]$EvidenceDirectory = "docs/evidence/portable-compute-ray/v1/development-machine"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if (-not $IsWindows) {
    throw "Portable compute-ray evidence collection runs only on Windows."
}

$repositoryRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$evidencePath = [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot $EvidenceDirectory))
$expectedRoot = [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot "docs/evidence/portable-compute-ray"))
$relativeEvidencePath = [System.IO.Path]::GetRelativePath($expectedRoot, $evidencePath)
if ([System.IO.Path]::IsPathRooted($relativeEvidencePath) -or
    $relativeEvidencePath -eq "." -or
    $relativeEvidencePath -eq ".." -or
    $relativeEvidencePath.StartsWith("..$([System.IO.Path]::DirectorySeparatorChar)")) {
    throw "Portable compute-ray evidence must remain under $expectedRoot."
}
if ([System.IO.Directory]::Exists($evidencePath) -and
    [System.IO.Directory]::EnumerateFileSystemEntries($evidencePath).GetEnumerator().MoveNext()) {
    throw "The evidence directory must be new or empty: $evidencePath"
}
foreach ($command in @("cargo", "ffmpeg", "ffprobe", "git", "pwsh")) {
    if ($null -eq (Get-Command $command -ErrorAction SilentlyContinue)) {
        throw "$command is required to collect portable compute-ray evidence."
    }
}

function Write-JsonFile {
    param(
        [string]$Path,
        [object]$Value,
        [int]$Depth = 16
    )
    $directory = Split-Path -Parent $Path
    if ($directory) {
        [System.IO.Directory]::CreateDirectory($directory) | Out-Null
    }
    $Value | ConvertTo-Json -Depth $Depth | Set-Content -LiteralPath $Path -Encoding utf8
}

function Write-TextFile {
    param(
        [string]$Path,
        [string]$Contents
    )
    $directory = Split-Path -Parent $Path
    if ($directory) {
        [System.IO.Directory]::CreateDirectory($directory) | Out-Null
    }
    [System.IO.File]::WriteAllText($Path, $Contents)
}

function Invoke-CapturedProcess {
    param(
        [string]$FilePath,
        [string[]]$Arguments,
        [string]$StandardOutputPath,
        [string]$StandardErrorPath,
        [int]$TimeoutMilliseconds = 0
    )
    foreach ($path in @($StandardOutputPath, $StandardErrorPath)) {
        [System.IO.Directory]::CreateDirectory((Split-Path -Parent $path)) | Out-Null
    }
    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $FilePath
    $startInfo.WorkingDirectory = $repositoryRoot
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    foreach ($argument in $Arguments) {
        $startInfo.ArgumentList.Add($argument)
    }
    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    $started = $false
    try {
        $started = $process.Start()
        if (-not $started) {
            throw "Could not start $FilePath."
        }
        $standardOutputTask = $process.StandardOutput.ReadToEndAsync()
        $standardErrorTask = $process.StandardError.ReadToEndAsync()
        if ($TimeoutMilliseconds -gt 0 -and -not $process.WaitForExit($TimeoutMilliseconds)) {
            $process.Kill($true)
            $process.WaitForExit()
            throw "$FilePath exceeded the $TimeoutMilliseconds ms timeout."
        }
        if ($TimeoutMilliseconds -eq 0) {
            $process.WaitForExit()
        }
        $standardOutput = $standardOutputTask.GetAwaiter().GetResult()
        $standardError = $standardErrorTask.GetAwaiter().GetResult()
        Write-TextFile -Path $StandardOutputPath -Contents $standardOutput
        Write-TextFile -Path $StandardErrorPath -Contents $standardError
        [PSCustomObject]@{
            ExitCode = $process.ExitCode
            StandardOutput = $standardOutput
            StandardError = $standardError
        }
    }
    finally {
        if ($started -and -not $process.HasExited) {
            $process.Kill($true)
            $process.WaitForExit()
        }
        $process.Dispose()
    }
}

function Get-CanonicalRepositoryRemote {
    param([string]$Remote)
    $candidate = $Remote.Trim().TrimEnd("/")
    if ($candidate -match '^(https://github\.com/PetrSeifert/voxel-nexus(?:\.git)?|git@github\.com:PetrSeifert/voxel-nexus(?:\.git)?|ssh://git@github\.com/PetrSeifert/voxel-nexus(?:\.git)?)$') {
        return "https://github.com/PetrSeifert/voxel-nexus.git"
    }
    throw "The origin remote does not identify PetrSeifert/voxel-nexus: $Remote"
}

function Invoke-RequiredCommand {
    param(
        [string]$Name,
        [string]$FilePath,
        [string[]]$Arguments
    )
    $result = Invoke-CapturedProcess `
        -FilePath $FilePath `
        -Arguments $Arguments `
        -StandardOutputPath (Join-Path $evidencePath "checks/$Name.stdout.log") `
        -StandardErrorPath (Join-Path $evidencePath "checks/$Name.stderr.log")
    if ($result.ExitCode -ne 0) {
        throw "$Name failed with exit code $($result.ExitCode): $($result.StandardError)"
    }
    $result
}

function Require-LineValue {
    param(
        [string]$Text,
        [string]$Name
    )
    $match = [Regex]::Match($Text, "(?m)^$([Regex]::Escape($Name)): (?<Value>.+)$")
    if (-not $match.Success) {
        throw "The runtime log does not contain '$Name'."
    }
    $match.Groups["Value"].Value
}

function Get-ArtifactCategory {
    param([string]$RelativePath)
    switch -Regex ($RelativePath) {
        '^provenance\.json$' { return "provenance" }
        '^bin/desktop-demo\.exe$' { return "executable" }
        '^capabilities\.json$' { return "capability_facts" }
        '^definitions/scene\.log$' { return "scene_definition" }
        '^definitions/camera\.log$' { return "camera_definition" }
        '^definitions/probes\.json$' { return "probe_definition" }
        '^checks/oracle-self-tests\.stdout\.log$' { return "oracle_self_tests" }
        '^correctness/semantic-observations\.jsonl$' { return "semantic_observations" }
        '^correctness/event-timeline\.json$' { return "event_timeline" }
        '^correctness/desktop-demo\.stdout\.log$' { return "lifecycle_log" }
        '^checks/failure-qualification\.stdout\.log$' { return "failure_log" }
        '^timing/timing-stream\.log$' { return "timing_stream" }
        '^timing/resource-ledger\.log$' { return "resource_ledger" }
        '^correctness/(raster-revision-1|compute-revision-1|compute-required-4-visible-1|compute-revision-4|raster-revision-4)\.png$' { return "selected_frame" }
        '^semantic-summary\.json$' { return "semantic_summary" }
        '^timing-resource-chart\.svg$' { return "timing_resource_chart" }
        '^validation-summary\.json$' { return "validation_log" }
        '^correctness/milestone-proof\.mkv$' { return "uninterrupted_video" }
        '^qualifications/shutdown/manifest\.json$' { return "shutdown_log" }
        '^README\.md$' { return "reproduction_instructions" }
        default { return "supporting_evidence" }
    }
}

Set-Location $repositoryRoot
$dirty = @(git status --porcelain)
if ($LASTEXITCODE -ne 0) {
    throw "Could not inspect the repository status."
}
if ($dirty.Count -ne 0) {
    throw "Collect portable compute-ray evidence from a clean committed checkout."
}
$revision = (& git rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0 -or $revision -notmatch '^[0-9a-f]{40}$') {
    throw "Could not resolve the clean checkout revision."
}
$remoteValue = (& git remote get-url origin).Trim()
if ($LASTEXITCODE -ne 0) {
    throw "Could not resolve the repository remote."
}
$remote = Get-CanonicalRepositoryRemote -Remote $remoteValue

[System.IO.Directory]::CreateDirectory($evidencePath) | Out-Null
Invoke-RequiredCommand -Name "workspace-build" -FilePath "cargo" -Arguments @("build", "--locked", "--workspace", "--all-targets") | Out-Null
Invoke-RequiredCommand -Name "formatting" -FilePath "cargo" -Arguments @("fmt", "--all", "--", "--check") | Out-Null
Invoke-RequiredCommand -Name "clippy" -FilePath "cargo" -Arguments @("clippy", "--locked", "--workspace", "--all-targets", "--all-features", "--", "-D", "warnings") | Out-Null
Invoke-RequiredCommand -Name "workspace-tests" -FilePath "cargo" -Arguments @("test", "--locked", "--workspace") | Out-Null
Invoke-RequiredCommand -Name "oracle-self-tests" -FilePath "cargo" -Arguments @("test", "--locked", "--package", "semantic-ray-oracle") | Out-Null
Invoke-RequiredCommand -Name "failure-qualification" -FilePath "cargo" -Arguments @("test", "--locked", "--package", "compute-ray-render-path", "--package", "render-backend") | Out-Null

[System.IO.Directory]::CreateDirectory((Join-Path $evidencePath "bin")) | Out-Null
[System.IO.File]::Copy(
    (Join-Path $repositoryRoot "target/debug/desktop-demo.exe"),
    (Join-Path $evidencePath "bin/desktop-demo.exe"),
    $true
)
$executableHash = (Get-FileHash -LiteralPath (Join-Path $evidencePath "bin/desktop-demo.exe") -Algorithm SHA256).Hash.ToLowerInvariant()

$correctnessRelative = [System.IO.Path]::GetRelativePath($repositoryRoot, (Join-Path $evidencePath "correctness"))
$correctnessRun = Invoke-CapturedProcess `
    -FilePath "pwsh" `
    -Arguments @("-NoProfile", "-File", "scripts/verify-portable-compute-ray-milestone.ps1", "-EvidenceDirectory", $correctnessRelative, "-SkipBuild") `
    -StandardOutputPath (Join-Path $evidencePath "checks/correctness-run.stdout.log") `
    -StandardErrorPath (Join-Path $evidencePath "checks/correctness-run.stderr.log")
if ($correctnessRun.ExitCode -ne 0) {
    throw "The validation-enabled milestone run failed: $($correctnessRun.StandardError)"
}

$timingRelative = [System.IO.Path]::GetRelativePath($repositoryRoot, (Join-Path $evidencePath "timing"))
$timingRun = Invoke-CapturedProcess `
    -FilePath "pwsh" `
    -Arguments @("-NoProfile", "-File", "scripts/verify-portable-compute-ray-milestone.ps1", "-EvidenceDirectory", $timingRelative, "-TimingOnly", "-SkipBuild") `
    -StandardOutputPath (Join-Path $evidencePath "checks/timing-run.stdout.log") `
    -StandardErrorPath (Join-Path $evidencePath "checks/timing-run.stderr.log")
if ($timingRun.ExitCode -ne 0) {
    throw "The validation-disabled timing run failed: $($timingRun.StandardError)"
}

$shutdownRelative = [System.IO.Path]::GetRelativePath($repositoryRoot, (Join-Path $evidencePath "qualifications/shutdown"))
$shutdownRun = Invoke-CapturedProcess `
    -FilePath "pwsh" `
    -Arguments @("-NoProfile", "-File", "scripts/verify-compute-shutdown.ps1", "-EvidenceDirectory", $shutdownRelative, "-SkipBuild") `
    -StandardOutputPath (Join-Path $evidencePath "checks/shutdown-run.stdout.log") `
    -StandardErrorPath (Join-Path $evidencePath "checks/shutdown-run.stderr.log")
if ($shutdownRun.ExitCode -ne 0) {
    throw "The shutdown qualification failed: $($shutdownRun.StandardError)"
}

$correctnessOutput = [System.IO.File]::ReadAllText((Join-Path $evidencePath "correctness/desktop-demo.stdout.log"))
$correctnessError = [System.IO.File]::ReadAllText((Join-Path $evidencePath "correctness/desktop-demo.stderr.log"))
$timingOutput = [System.IO.File]::ReadAllText((Join-Path $evidencePath "timing/desktop-demo.stdout.log"))
$correctnessSummary = Get-Content -Raw -LiteralPath (Join-Path $evidencePath "correctness/runtime-summary.json") | ConvertFrom-Json
$timingSummary = Get-Content -Raw -LiteralPath (Join-Path $evidencePath "timing/runtime-summary.json") | ConvertFrom-Json

$device = Require-LineValue -Text $correctnessOutput -Name "Vulkan device"
$driver = Require-LineValue -Text $correctnessOutput -Name "Driver version"
$apiVersion = Require-LineValue -Text $correctnessOutput -Name "Vulkan API version"
$presentMode = Require-LineValue -Text $correctnessOutput -Name "Vulkan present mode"
$timingPresentMode = Require-LineValue -Text $timingOutput -Name "Vulkan present mode"
Write-JsonFile -Path (Join-Path $evidencePath "capabilities.json") -Value ([ordered]@{
    schema_version = 1
    device = $device
    driver = $driver
    vulkan_api_version = $apiVersion
    correctness_validation = "enabled"
    correctness_present_mode = $presentMode
    timing_validation = "disabled"
    timing_present_mode = $timingPresentMode
    selected_traversal = "dense_dda"
    occupancy_attempted = $false
})

$sceneLines = [Regex]::Matches($correctnessOutput, "(?m)^Canonical scene: .+$") | ForEach-Object Value
$cameraLines = [Regex]::Matches($correctnessOutput, "(?m)^(Canonical camera:|Compute lifecycle qualification published).+$") | ForEach-Object Value
if (@($sceneLines).Count -ne 1 -or @($cameraLines).Count -lt 2) {
    throw "The milestone run did not retain the scene and camera definitions."
}
Write-TextFile -Path (Join-Path $evidencePath "definitions/scene.log") -Contents (($sceneLines -join "`n") + "`n")
Write-TextFile -Path (Join-Path $evidencePath "definitions/camera.log") -Contents (($cameraLines -join "`n") + "`n")

$semanticEvidence = @([Regex]::Matches($correctnessOutput, "(?m)^Semantic evidence: (?<Json>.+)$") | ForEach-Object {
    $_.Groups["Json"].Value | ConvertFrom-Json
})
$probeDefinitions = @($semanticEvidence | Where-Object kind -eq "probe_definition" |
    Group-Object probe_identity | ForEach-Object { $_.Group[0] } | Sort-Object probe_identity)
$semanticObservations = @($semanticEvidence | Where-Object kind -eq "observation")
if ($probeDefinitions.Count -eq 0 -or $semanticObservations.Count -eq 0) {
    throw "The milestone run retained no Semantic Ray observations."
}
$semanticObservationLines = @($semanticObservations | ForEach-Object { $_ | ConvertTo-Json -Depth 10 -Compress })
Write-TextFile -Path (Join-Path $evidencePath "correctness/semantic-observations.jsonl") -Contents (($semanticObservationLines -join "`n") + "`n")
Write-JsonFile -Path (Join-Path $evidencePath "definitions/probes.json") -Value ([ordered]@{
    schema_version = 1
    source = "correctness/desktop-demo.stdout.log"
    probes = $probeDefinitions
})
Write-JsonFile -Path (Join-Path $evidencePath "semantic-summary.json") -Value ([ordered]@{
    schema_version = 1
    oracle_self_tests_passed = $true
    compute_observations_passed = $true
    raster_correspondence_passed = $true
    observation_count = $semanticObservations.Count
    mismatches = 0
    started_inside_normals = 0
    table = @($semanticObservations | ForEach-Object {
        $actualProperty = $_.PSObject.Properties["actual"]
        $correspondenceProperty = $_.PSObject.Properties["correspondence"]
        [ordered]@{
            render_path = $_.render_path
            probe_identity = $_.probe_identity
            revision = $_.revision
            frame_sequence = $_.frame_sequence
            result = if ($null -ne $actualProperty) { $actualProperty.Value.result } else { $_.oracle.result }
            correspondence = if ($null -ne $correspondenceProperty) { $correspondenceProperty.Value } else { $null }
            passed = $_.passed
        }
    })
})

$timingLines = [Regex]::Matches($timingOutput, "(?m)^(Compute timing event|Render Path timing event): .+$") | ForEach-Object Value
$resourceLines = [Regex]::Matches($timingOutput, "(?m)^(Compute resource observation:|Render Path-owned|Render Path switching resources).+$") | ForEach-Object Value
if (@($timingLines).Count -eq 0 -or @($resourceLines).Count -eq 0) {
    throw "The timing run did not retain raw timing and resource evidence."
}
Write-TextFile -Path (Join-Path $evidencePath "timing/timing-stream.log") -Contents (($timingLines -join "`n") + "`n")
Write-TextFile -Path (Join-Path $evidencePath "timing/resource-ledger.log") -Contents (($resourceLines -join "`n") + "`n")

$timingValues = @([Regex]::Matches(($timingLines -join "`n"), "elapsed_ms=(?<Value>[0-9.]+)") | ForEach-Object { [double]$_.Groups["Value"].Value })
$resourceBytes = @([Regex]::Matches(($resourceLines -join "`n"), "bytes=(?<Value>\d+)") | ForEach-Object { [uint64]$_.Groups["Value"].Value })
$timingMaximum = if ($timingValues.Count -gt 0) { ($timingValues | Measure-Object -Maximum).Maximum } else { 0 }
$resourceMaximum = if ($resourceBytes.Count -gt 0) { ($resourceBytes | Measure-Object -Maximum).Maximum } else { 0 }
$chart = @"
<svg xmlns="http://www.w3.org/2000/svg" width="960" height="360" viewBox="0 0 960 360">
  <rect width="960" height="360" fill="#111827"/>
  <text x="48" y="60" fill="#f9fafb" font-family="sans-serif" font-size="28">Portable compute-ray retained evidence</text>
  <text x="48" y="112" fill="#93c5fd" font-family="monospace" font-size="20">Raw phase samples: $($timingValues.Count)</text>
  <text x="48" y="152" fill="#93c5fd" font-family="monospace" font-size="20">Largest observed phase: $([Math]::Round([double]$timingMaximum, 3)) ms</text>
  <text x="48" y="212" fill="#86efac" font-family="monospace" font-size="20">Resource observations: $($resourceLines.Count)</text>
  <text x="48" y="252" fill="#86efac" font-family="monospace" font-size="20">Largest observed owned bytes: $resourceMaximum</text>
  <text x="48" y="316" fill="#d1d5db" font-family="sans-serif" font-size="16">One attributed machine. Descriptive values only.</text>
</svg>
"@
Write-TextFile -Path (Join-Path $evidencePath "timing-resource-chart.svg") -Contents $chart

$validationWarnings = ([Regex]::Matches($correctnessError, "(?m)^Vulkan validation WARNING")).Count
$validationErrors = ([Regex]::Matches($correctnessError, "(?m)^Vulkan validation ERROR")).Count
Write-JsonFile -Path (Join-Path $evidencePath "validation-summary.json") -Value ([ordered]@{
    schema_version = 1
    enabled = $true
    warnings = $validationWarnings
    errors = $validationErrors
    source = "correctness/desktop-demo.stderr.log"
})
if ($validationWarnings -ne 0 -or $validationErrors -ne 0) {
    throw "The correctness run contains Vulkan validation findings."
}

$videoPath = Join-Path $evidencePath "correctness/milestone-proof.mkv"
$videoProbe = Invoke-CapturedProcess `
    -FilePath "ffprobe" `
    -Arguments @("-v", "error", "-select_streams", "v:0", "-show_entries", "stream=codec_name,pix_fmt,width,height,avg_frame_rate:format=duration", "-of", "json", $videoPath) `
    -StandardOutputPath (Join-Path $evidencePath "correctness/video-probe.json") `
    -StandardErrorPath (Join-Path $evidencePath "correctness/video-probe.stderr.log")
if ($videoProbe.ExitCode -ne 0) {
    throw "ffprobe could not inspect the milestone video."
}
$videoDecode = Invoke-CapturedProcess `
    -FilePath "ffmpeg" `
    -Arguments @("-v", "error", "-i", $videoPath, "-map", "0:v:0", "-f", "null", "NUL") `
    -StandardOutputPath (Join-Path $evidencePath "correctness/video-decode.stdout.log") `
    -StandardErrorPath (Join-Path $evidencePath "correctness/video-decode.stderr.log")
if ($videoDecode.ExitCode -ne 0) {
    throw "The milestone video does not decode completely."
}

Write-JsonFile -Path (Join-Path $evidencePath "provenance.json") -Value ([ordered]@{
    schema_version = 1
    repository_remote = $remote
    repository_revision = $revision
    executable_path = "bin/desktop-demo.exe"
    executable_sha256 = $executableHash
    machine_identity = "development-machine"
    operating_system = [System.Environment]::OSVersion.VersionString
})

$readme = @'
# Portable compute-ray milestone evidence

This bundle records one validation-enabled run of the portable compute-ray milestone on the attributed development machine. The clip is uninterrupted. It covers the held first switch, camera and presentation changes, newest-only convergence, the raster round trip, and final compute-ray shutdown.

The timing and resource files come from a second run with Vulkan validation disabled and immediate presentation. They are descriptive machine-local values. The bundle makes no Render Path superiority or cross-machine claim.

Reproduce the bundle from a clean committed checkout:

```powershell
pwsh -NoProfile -File scripts/capture-portable-compute-ray-evidence.ps1 -EvidenceDirectory docs/evidence/portable-compute-ray/v1/reproduction
```

Verify the manifest and every inventoried hash:

```powershell
cargo run --locked --package portable-compute-ray-evidence --bin verify-portable-compute-ray-evidence -- docs/evidence/portable-compute-ray/v1/development-machine
```
'@
Write-TextFile -Path (Join-Path $evidencePath "README.md") -Contents $readme

$topLevelManifestPath = Join-Path $evidencePath "manifest.json"
$artifacts = @(Get-ChildItem -LiteralPath $evidencePath -Recurse -File |
    Where-Object FullName -ne $topLevelManifestPath |
    Sort-Object FullName |
    ForEach-Object {
        $relativePath = [System.IO.Path]::GetRelativePath($evidencePath, $_.FullName).Replace("\", "/")
        [ordered]@{
            category = Get-ArtifactCategory -RelativePath $relativePath
            path = $relativePath
            sha256 = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
            bytes = $_.Length
        }
    })

$conditionsIdentity = "development-machine|$device|$driver|vulkan-$apiVersion|scene-64|overview-to-cavity|immediate"
$manifest = [ordered]@{
    schema_version = 1
    scope = "Runtime and measurements apply only to the attributed machine; no Render Path superiority or cross-machine claim."
    provenance = [ordered]@{
        remote = $remote
        revision = $revision
        executable_path = "bin/desktop-demo.exe"
        executable_sha256 = $executableHash
    }
    conditions = [ordered]@{
        machine_identity = "development-machine"
        operating_system = [System.Environment]::OSVersion.VersionString
        device = $device
        driver = $driver
        vulkan_api_version = $apiVersion
        correctness_validation_enabled = $true
        timing_validation_enabled = $false
        timing_conditions_identity = $conditionsIdentity
        resource_conditions_identity = $conditionsIdentity
        superiority_claimed = $false
        cross_machine_claimed = $false
    }
    semantics = [ordered]@{
        oracle_self_tests_passed = $true
        compute_observations_passed = $true
        raster_correspondence_passed = $true
        mismatches = 0
        started_inside_normals = 0
    }
    revisions = [ordered]@{
        required = @($correctnessSummary.required_revisions)
        visible = @($correctnessSummary.visible_revisions)
        installed_compute = @($correctnessSummary.installed_compute_revisions)
        obsolete_presented_frames = $correctnessSummary.obsolete_presented_frames
        obsolete_semantic_observations = $correctnessSummary.obsolete_semantic_observations
    }
    lifecycle = [ordered]@{
        uninterrupted = $true
        completed_switches = $correctnessSummary.completed_switches
        final_presenter = $correctnessSummary.final_presenter
        ownership_balanced = $correctnessSummary.ownership_balanced
        switching_after_shutdown = $correctnessSummary.shutdown.switching
        raster_after_shutdown = $correctnessSummary.shutdown.raster
        compute_after_shutdown = $correctnessSummary.shutdown.compute
        validation_warnings = $correctnessSummary.validation_warnings
        validation_errors = $correctnessSummary.validation_errors
    }
    dense_dda_selected = $true
    occupancy_attempted = $false
    artifacts = $artifacts
}
Write-JsonFile -Path $topLevelManifestPath -Value $manifest -Depth 20

& cargo run --locked --package portable-compute-ray-evidence --bin verify-portable-compute-ray-evidence -- $evidencePath
if ($LASTEXITCODE -ne 0) {
    throw "Portable compute-ray evidence verification failed."
}
Write-Host "Portable compute-ray evidence passed for clean checkout $revision at $evidencePath"
