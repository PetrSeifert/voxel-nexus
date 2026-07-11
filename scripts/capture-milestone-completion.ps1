[CmdletBinding()]
param(
    [string]$EvidenceDirectory = "docs/evidence/milestone-completion/development-machine"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if (-not $IsWindows) {
    throw "Milestone completion evidence collection is supported only on Windows."
}

$repositoryRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$evidencePath = [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot $EvidenceDirectory))
$expectedRoot = [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot "docs/evidence/milestone-completion"))
$relativeEvidencePath = [System.IO.Path]::GetRelativePath($expectedRoot, $evidencePath)
if ([System.IO.Path]::IsPathRooted($relativeEvidencePath) -or
    $relativeEvidencePath -eq "." -or
    $relativeEvidencePath -eq ".." -or
    $relativeEvidencePath.StartsWith("..$([System.IO.Path]::DirectorySeparatorChar)")) {
    throw "Completion evidence must remain under $expectedRoot."
}
if ([System.IO.Directory]::Exists($evidencePath) -and
    [System.IO.Directory]::EnumerateFileSystemEntries($evidencePath).GetEnumerator().MoveNext()) {
    throw "The completion evidence directory must be new or empty: $evidencePath"
}
foreach ($command in @("cargo", "rustc", "ffmpeg", "ffprobe", "pwsh")) {
    if ($null -eq (Get-Command $command -ErrorAction SilentlyContinue)) {
        throw "$command is required to collect completion evidence."
    }
}

function Invoke-CapturedProcess {
    param(
        [Parameter(Mandatory)] [string]$FilePath,
        [Parameter(Mandatory)] [string[]]$Arguments,
        [Parameter(Mandatory)] [string]$StandardOutputPath,
        [Parameter(Mandatory)] [string]$StandardErrorPath,
        [int]$TimeoutMilliseconds = 0
    )

    $standardOutputDirectory = Split-Path -Parent $StandardOutputPath
    $standardErrorDirectory = Split-Path -Parent $StandardErrorPath
    [System.IO.Directory]::CreateDirectory($standardOutputDirectory) | Out-Null
    [System.IO.Directory]::CreateDirectory($standardErrorDirectory) | Out-Null
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
    $startedAt = [DateTime]::UtcNow
    if (-not $process.Start()) {
        throw "Could not start $FilePath."
    }
    $standardOutputTask = $process.StandardOutput.ReadToEndAsync()
    $standardErrorTask = $process.StandardError.ReadToEndAsync()
    if ($TimeoutMilliseconds -gt 0 -and -not $process.WaitForExit($TimeoutMilliseconds)) {
        $process.Kill()
        $process.WaitForExit()
        throw "$FilePath exceeded the $TimeoutMilliseconds ms timeout."
    }
    if ($TimeoutMilliseconds -eq 0) {
        $process.WaitForExit()
    }
    $standardOutput = $standardOutputTask.GetAwaiter().GetResult()
    $standardError = $standardErrorTask.GetAwaiter().GetResult()
    [System.IO.File]::WriteAllText($StandardOutputPath, $standardOutput)
    [System.IO.File]::WriteAllText($StandardErrorPath, $standardError)
    [PSCustomObject]@{
        ExitCode = $process.ExitCode
        Command = "$FilePath $($Arguments -join ' ')"
        StartedAtUtc = $startedAt.ToString("o")
        FinishedAtUtc = [DateTime]::UtcNow.ToString("o")
        StandardOutput = [System.IO.Path]::GetRelativePath($evidencePath, $StandardOutputPath).Replace("\", "/")
        StandardError = [System.IO.Path]::GetRelativePath($evidencePath, $StandardErrorPath).Replace("\", "/")
        OutputText = $standardOutput
    }
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
    Add-CommandRecord -Name $Name -Result $result
    if ($result.ExitCode -ne 0) {
        throw "$Name failed with exit code $($result.ExitCode)."
    }
}

function Add-CommandRecord {
    param(
        [string]$Name,
        [PSCustomObject]$Result,
        [string]$StandardOutput = $Result.StandardOutput,
        [string]$StandardError = $Result.StandardError
    )

    $script:commandRecords.Add([ordered]@{
        Name = $Name
        Command = $Result.Command
        ExitCode = $Result.ExitCode
        StartedAtUtc = $Result.StartedAtUtc
        FinishedAtUtc = $Result.FinishedAtUtc
        StandardOutput = $StandardOutput
        StandardError = $StandardError
    })
}

function Write-JsonFile {
    param(
        [string]$Path,
        [object]$Value,
        [int]$Depth = 20
    )

    $directory = Split-Path -Parent $Path
    if ($directory) {
        [System.IO.Directory]::CreateDirectory($directory) | Out-Null
    }
    $Value | ConvertTo-Json -Depth $Depth | Set-Content -LiteralPath $Path -Encoding utf8
}

function Get-ArtifactCategory {
    param([string]$RelativePath)

    switch -Regex ($RelativePath) {
        '^lifecycle/milestone-proof\.mkv$' { return "continuous_video" }
        '^lifecycle/completion-video-events\.json$' { return "video_event_timeline" }
        '^lifecycle/(launch|cavity|boundary)-[ab]\.png$' { return "fixed_pose_png" }
        '^comparisons/(overview|cavity|boundary)\.json$' { return "tolerant_comparison" }
        '^semantic-face-report\.json$' { return "semantic_face_report" }
        '^lifecycle/desktop-demo\.stderr\.log$' { return "validation_log" }
        '^lifecycle/background-derivation\.stderr\.log$' { return "derivation_failure_log" }
        '^lifecycle/raster-install-upload\.stderr\.log$' { return "upload_failure_log" }
        '^lifecycle/unsupported-(vulkan-1\.2|presentation)\.stderr\.log$' { return "prerequisite_log" }
        '^timing/manifest\.json$' { return "timing_manifest" }
        '^timing/first-correct-frame-(64|128|256)-\d\d\.jsonl$' { return "first_correct_frame_stream" }
        '^timing/steady-state-(64|128|256)\.jsonl$' { return "steady_cpu_gpu_stream" }
        '^timing-summary\.json$' { return "timing_summary" }
        '^geometry-resource-counts\.json$' { return "geometry_resource_counts" }
        '^timing/comparison\.svg$' { return "comparison_chart" }
        '^checks/clean-checkout-summary\.json$' { return "clean_checkout_log" }
        '^README\.md$' { return "reproduction_instructions" }
        default { return "supporting_evidence" }
    }
}

Set-Location $repositoryRoot
$dirty = @(git status --porcelain)
if ($LASTEXITCODE -ne 0) {
    throw "Could not inspect repository status."
}
if ($dirty.Count -ne 0) {
    throw "Collect completion evidence from a clean committed checkout."
}
$revision = (git rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0 -or $revision -notmatch '^[0-9a-f]{40}$') {
    throw "Could not resolve the clean checkout revision."
}

$script:commandRecords = [System.Collections.Generic.List[object]]::new()
$relativeTimingDirectory = [System.IO.Path]::GetRelativePath(
    $repositoryRoot,
    (Join-Path $evidencePath "timing")
)
$temporaryIdentity = [Guid]::NewGuid().ToString("n")
$temporaryDirectory = [System.IO.Path]::GetTempPath()
$temporaryTimingStandardOutput = Join-Path $temporaryDirectory "voxel-nexus-$temporaryIdentity.stdout.log"
$temporaryTimingStandardError = Join-Path $temporaryDirectory "voxel-nexus-$temporaryIdentity.stderr.log"
$timingCollectionError = $null
try {
    $timingCollector = Invoke-CapturedProcess `
        -FilePath "pwsh" `
        -Arguments @("-NoProfile", "-File", "scripts/collect-timing-evidence.ps1", "-OutputDirectory", $relativeTimingDirectory) `
        -StandardOutputPath $temporaryTimingStandardOutput `
        -StandardErrorPath $temporaryTimingStandardError
    [System.IO.Directory]::CreateDirectory($evidencePath) | Out-Null
    [System.IO.File]::Move($temporaryTimingStandardOutput, (Join-Path $evidencePath "timing-collector.stdout.log"), $true)
    [System.IO.File]::Move($temporaryTimingStandardError, (Join-Path $evidencePath "timing-collector.stderr.log"), $true)
}
catch {
    $timingCollectionError = $_
    throw
}
finally {
    foreach ($temporaryPath in @($temporaryTimingStandardOutput, $temporaryTimingStandardError)) {
        if ([System.IO.File]::Exists($temporaryPath)) {
            try {
                [System.IO.File]::Delete($temporaryPath)
            }
            catch {
                if ($null -eq $timingCollectionError) {
                    throw
                }
                Write-Warning "Could not remove temporary timing log $temporaryPath after collection failure: $($_.Exception.Message)"
            }
        }
    }
}
Add-CommandRecord `
    -Name "timing-evidence" `
    -Result $timingCollector `
    -StandardOutput "timing-collector.stdout.log" `
    -StandardError "timing-collector.stderr.log"
if ($timingCollector.ExitCode -ne 0) {
    throw "Timing evidence collection failed with exit code $($timingCollector.ExitCode)."
}

Invoke-RequiredCommand -Name "generated-artifacts" -FilePath "cargo" -Arguments @("build", "--locked", "--workspace", "--all-targets")
Invoke-RequiredCommand -Name "formatting" -FilePath "cargo" -Arguments @("fmt", "--all", "--", "--check")
Invoke-RequiredCommand -Name "strict-clippy" -FilePath "cargo" -Arguments @("clippy", "--locked", "--workspace", "--all-targets", "--all-features", "--", "-D", "warnings")
Invoke-RequiredCommand -Name "workspace-tests" -FilePath "cargo" -Arguments @("test", "--locked", "--workspace")
Invoke-RequiredCommand -Name "voxel-frontend-read" -FilePath "cargo" -Arguments @("test", "--locked", "--package", "voxel-frontend")
Invoke-RequiredCommand -Name "diagnostic-surface" -FilePath "cargo" -Arguments @("test", "--locked", "--package", "raster-render-path", "--test", "derivation")

$lifecycleResult = Invoke-CapturedProcess `
    -FilePath "pwsh" `
    -Arguments @(
        "-NoProfile", "-File", "scripts/verify-windows-lifecycle.ps1",
        "-EvidenceDirectory", ([System.IO.Path]::GetRelativePath($repositoryRoot, (Join-Path $evidencePath "lifecycle"))),
        "-CaptureCanonicalInspectionSet",
        "-VideoFile", "milestone-proof.mkv"
    ) `
    -StandardOutputPath (Join-Path $evidencePath "lifecycle-run.stdout.log") `
    -StandardErrorPath (Join-Path $evidencePath "lifecycle-run.stderr.log")
Add-CommandRecord -Name "lifecycle-failure-prerequisite-proof" -Result $lifecycleResult
if ($lifecycleResult.ExitCode -ne 0) {
    throw "Windows lifecycle proof failed with exit code $($lifecycleResult.ExitCode)."
}

$lifecycleManifest = Get-Content -Raw -LiteralPath (Join-Path $evidencePath "lifecycle/manifest.json") | ConvertFrom-Json
$timingManifest = Get-Content -Raw -LiteralPath (Join-Path $evidencePath "timing/manifest.json") | ConvertFrom-Json
if ($lifecycleManifest.RepositoryRevision -ne $revision -or $timingManifest.RepositoryRevision -ne $revision) {
    throw "Nested evidence manifests do not identify the clean checkout revision $revision."
}
if ($lifecycleManifest.ValidationWarnings -ne 0 -or $lifecycleManifest.ValidationErrors -ne 0) {
    throw "The lifecycle proof contains Vulkan validation warnings or errors."
}

[System.IO.Directory]::CreateDirectory((Join-Path $evidencePath "comparisons")) | Out-Null
foreach ($pose in @("overview", "cavity", "boundary")) {
    $inspection = @($lifecycleManifest.CanonicalInspections | Where-Object Name -eq $pose)
    if ($inspection.Count -ne 1 -or $inspection[0].Capture.Captures.Count -ne 2) {
        throw "The lifecycle manifest does not retain one paired $pose inspection."
    }
    $poseCapture = $inspection[0].Capture
    Write-JsonFile -Path (Join-Path $evidencePath "comparisons/$pose.json") -Value ([ordered]@{
        Pose = $pose
        Captures = $poseCapture.Captures
        CaptureSha256 = $poseCapture.CaptureSha256
        MaterialDifferenceFraction = $poseCapture.MaterialDifferenceFraction
        MaximumMaterialDifferenceFraction = 0.001
        Passed = $poseCapture.MaterialDifferenceFraction -le 0.001
    })
}

$diagnostics = Get-Content -Raw -LiteralPath (Join-Path $evidencePath "timing/diagnostics.json") | ConvertFrom-Json
$semanticDiagnostic = @($diagnostics | Where-Object Name -eq "semantic-face-oracles")
if ($semanticDiagnostic.Count -ne 1 -or -not $semanticDiagnostic[0].Passed) {
    throw "The semantic-face diagnostic did not pass."
}
$semanticOutput = Get-Content -Raw -LiteralPath (Join-Path $evidencePath "timing/semantic-face-oracles.stdout.log")
$semanticCases = @([Regex]::Matches($semanticOutput, '(?m)^test (?<Name>\S+) \.\.\. ok$') | ForEach-Object {
    [ordered]@{ Name = $_.Groups["Name"].Value; Result = "passed" }
})
if ($semanticCases.Count -lt 7) {
    throw "The semantic-face diagnostic report did not retain all seven cases."
}
Write-JsonFile -Path (Join-Path $evidencePath "semantic-face-report.json") -Value ([ordered]@{
    SchemaVersion = 1
    Scope = "Exact semantic face diagnostics for the recorded clean checkout."
    SemanticIdentity = @("Voxel Volume identity", "occupied coordinate", "outward axis normal", "Voxel Material identity")
    Command = $semanticDiagnostic[0].Command
    Passed = $true
    Cases = $semanticCases
    RawOutput = "timing/semantic-face-oracles.stdout.log"
})

Write-JsonFile -Path (Join-Path $evidencePath "timing-summary.json") -Value ([ordered]@{
    Scope = $timingManifest.Scope
    RepositoryRevision = $timingManifest.RepositoryRevision
    Scales = @($timingManifest.Scales | ForEach-Object {
        [ordered]@{
            Scale = $_.Scale
            FirstCorrectFrame = $_.FirstCorrectFrame
            CpuFrame = $_.SteadyState.CpuFrame
            GpuFrame = $_.SteadyState.GpuFrame
        }
    })
})
Write-JsonFile -Path (Join-Path $evidencePath "geometry-resource-counts.json") -Value ([ordered]@{
    Scope = $timingManifest.Scope
    RepositoryRevision = $timingManifest.RepositoryRevision
    Scales = @($timingManifest.Scales | ForEach-Object {
        [ordered]@{
            Scale = $_.Scale
            Resources = $_.Resources
        }
    })
})

$videoPath = Join-Path $evidencePath "lifecycle/milestone-proof.mkv"
$videoEvents = @(Get-Content -Raw -LiteralPath (Join-Path $evidencePath "lifecycle/completion-video-events.json") | ConvertFrom-Json)
$minimizedEvent = @($videoEvents | Where-Object Event -eq "minimized_while_paused")
$restoredEvent = @($videoEvents | Where-Object Event -eq "restored_while_paused")
$cleanCloseEvent = @($videoEvents | Where-Object Event -eq "clean_close")
if ($minimizedEvent.Count -ne 1 -or $restoredEvent.Count -ne 1 -or $cleanCloseEvent.Count -ne 1) {
    throw "The completion video timeline cannot define the privacy mask intervals."
}
$minimizedMaskStart = [Math]::Max(0, [double]$minimizedEvent[0].ElapsedSeconds - 0.2)
$restoredMaskEnd = [double]$restoredEvent[0].ElapsedSeconds + 0.5
$cleanCloseMaskStart = [double]$cleanCloseEvent[0].ElapsedSeconds
$formatInvariant = [Globalization.CultureInfo]::InvariantCulture
$privacyFilter = "crop=634:642:28:20,drawbox=x=0:y=0:w=iw:h=ih:color=black:t=fill:enable='between(t\,$($minimizedMaskStart.ToString("0.000", $formatInvariant))\,$($restoredMaskEnd.ToString("0.000", $formatInvariant)))+gte(t\,$($cleanCloseMaskStart.ToString("0.000", $formatInvariant)))'"
$privacyFilteredVideoPath = Join-Path $evidencePath "lifecycle/milestone-proof-privacy.mkv"
$privacyFilterResult = Invoke-CapturedProcess `
    -FilePath "ffmpeg" `
    -Arguments @("-v", "error", "-i", $videoPath, "-vf", $privacyFilter, "-c:v", "libx264", "-preset", "veryfast", "-crf", "18", "-pix_fmt", "yuv420p", "-y", $privacyFilteredVideoPath) `
    -StandardOutputPath (Join-Path $evidencePath "video-privacy-filter.stdout.log") `
    -StandardErrorPath (Join-Path $evidencePath "video-privacy-filter.stderr.log")
if ($privacyFilterResult.ExitCode -ne 0) {
    throw "Could not remove unrelated desktop pixels from the completion video."
}
[System.IO.File]::Move($privacyFilteredVideoPath, $videoPath, $true)
Write-JsonFile -Path (Join-Path $evidencePath "video-privacy-filter.json") -Value ([ordered]@{
    Input = "lifecycle/milestone-proof.mkv"
    Output = "lifecycle/milestone-proof.mkv"
    ContinuousTransform = $true
    Crop = [ordered]@{ X = 28; Y = 20; Width = 634; Height = 642 }
    BlackIntervals = @(
        [ordered]@{ Meaning = "window minimized"; StartSeconds = $minimizedMaskStart; EndSeconds = $restoredMaskEnd },
        [ordered]@{ Meaning = "window cleanly closed"; StartSeconds = $cleanCloseMaskStart; EndSeconds = "end of clip" }
    )
    Filter = $privacyFilter
})
$recorderCaptureScope = $lifecycleManifest.CompletionVideo.CaptureScope
$retainedCaptureScope = "Continuous 634x642 privacy crop of the fixed demo-window intersection; minimized and post-close intervals are blacked from the recorded event timeline."
$lifecycleManifest.CompletionVideo.CaptureScope = $retainedCaptureScope
$lifecycleManifest.CompletionVideo | Add-Member -NotePropertyName RecorderCaptureScope -NotePropertyValue $recorderCaptureScope
$lifecycleManifest.CompletionVideo | Add-Member -NotePropertyName RetainedDimensions -NotePropertyValue @(634, 642)
$lifecycleManifest.CompletionVideo | Add-Member -NotePropertyName PrivacyFilter -NotePropertyValue "../video-privacy-filter.json"
Write-JsonFile -Path (Join-Path $evidencePath "lifecycle/manifest.json") -Value $lifecycleManifest
$videoProbe = Invoke-CapturedProcess `
    -FilePath "ffprobe" `
    -Arguments @("-v", "error", "-select_streams", "v:0", "-show_entries", "stream=codec_name,pix_fmt,width,height,avg_frame_rate:format=duration", "-of", "json", $videoPath) `
    -StandardOutputPath (Join-Path $evidencePath "video-probe.json") `
    -StandardErrorPath (Join-Path $evidencePath "video-probe.stderr.log")
if ($videoProbe.ExitCode -ne 0) {
    throw "ffprobe could not inspect the completion video."
}
$probe = $videoProbe.OutputText | ConvertFrom-Json
if ($probe.streams.Count -ne 1) {
    throw "The completion video does not contain exactly one video stream."
}
$videoStream = $probe.streams[0]
$videoDuration = [double]$probe.format.duration
if ($cleanCloseEvent.Count -ne 1 -or $videoDuration -le [double]$cleanCloseEvent[0].ElapsedSeconds) {
    throw "The completion video duration does not cover the clean-close event."
}
$videoDecode = Invoke-CapturedProcess `
    -FilePath "ffmpeg" `
    -Arguments @("-v", "error", "-i", $videoPath, "-map", "0:v:0", "-f", "null", "NUL") `
    -StandardOutputPath (Join-Path $evidencePath "video-decode.stdout.log") `
    -StandardErrorPath (Join-Path $evidencePath "video-decode.stderr.log")
if ($videoDecode.ExitCode -ne 0) {
    throw "The completion video is not fully decodable."
}

$representativeTimes = [ordered]@{}
foreach ($eventName in @("worker_paused", "first_matching_revision_frame", "fixed_pose_overview", "fixed_pose_cavity", "fixed_pose_boundary", "clean_close")) {
    $event = @($videoEvents | Where-Object Event -eq $eventName)
    if ($event.Count -ne 1) {
        throw "Completion video event $eventName is missing or duplicated."
    }
    $representativeTimes[$eventName] = [double]$event[0].ElapsedSeconds + 0.25
}
$moveStart = @($videoEvents | Where-Object Event -eq "deterministic_camera_move_started")
$moveEnd = @($videoEvents | Where-Object Event -eq "deterministic_camera_move_completed")
if ($moveStart.Count -ne 1 -or $moveEnd.Count -ne 1) {
    throw "Deterministic camera move video events are incomplete."
}
$representativeTimes["deterministic_camera_move"] = ([double]$moveStart[0].ElapsedSeconds + [double]$moveEnd[0].ElapsedSeconds) / 2
[System.IO.Directory]::CreateDirectory((Join-Path $evidencePath "representative-frames")) | Out-Null
foreach ($frame in $representativeTimes.GetEnumerator()) {
    $frameResult = Invoke-CapturedProcess `
        -FilePath "ffmpeg" `
        -Arguments @("-v", "error", "-ss", $frame.Value.ToString("0.000", [Globalization.CultureInfo]::InvariantCulture), "-i", $videoPath, "-frames:v", "1", "-y", (Join-Path $evidencePath "representative-frames/$($frame.Key).png")) `
        -StandardOutputPath (Join-Path $evidencePath "representative-frames/$($frame.Key).stdout.log") `
        -StandardErrorPath (Join-Path $evidencePath "representative-frames/$($frame.Key).stderr.log")
    if ($frameResult.ExitCode -ne 0) {
        throw "Could not extract representative video frame $($frame.Key)."
    }
}

Write-JsonFile -Path (Join-Path $evidencePath "checks/clean-checkout-summary.json") -Value ([ordered]@{
    Scope = "Commands executed from clean checkout $revision on the recorded Windows development machine."
    RepositoryRevision = $revision
    WorkingTreeWasCleanBeforeCollection = $true
    Commands = $script:commandRecords
})

$readme = @"
# Dense raster Voxel Scene completion evidence

Runtime and timing claims in this bundle apply only to the recorded Windows development machine in the nested manifests. They are descriptive evidence, not portable correctness or performance claims.

From a clean committed checkout with the Vulkan SDK and `VK_LAYER_KHRONOS_validation` available, reproduce the complete proof into a new directory:

```powershell
pwsh -NoProfile -File scripts/capture-milestone-completion.ps1 -EvidenceDirectory docs/evidence/milestone-completion/reproduction
```

Verify this retained bundle and every inventoried SHA-256 hash:

```powershell
cargo run --locked --package completion-evidence --bin verify-completion-evidence -- docs/evidence/milestone-completion/development-machine
```

Decode the uninterrupted clip independently:

```powershell
ffmpeg -v error -i docs/evidence/milestone-completion/development-machine/lifecycle/milestone-proof.mkv -map 0:v:0 -f null NUL
```

`checks/clean-checkout-summary.json` records generated-artifact build, formatting, strict lint, workspace unit/integration, Voxel Frontend read, diagnostic surface, lifecycle, deterministic-failure, prerequisite, and timing commands with their raw output logs. The top-level summaries cross-link the nested lifecycle and timing manifests, 30 first-correct-frame streams, three CPU/GPU streams, geometry/resource counts, and comparison chart.
"@
Set-Content -LiteralPath (Join-Path $evidencePath "README.md") -Value $readme -Encoding utf8

$topLevelManifestPath = Join-Path $evidencePath "manifest.json"
$files = @(Get-ChildItem -LiteralPath $evidencePath -Recurse -File |
    Where-Object FullName -ne $topLevelManifestPath |
    Sort-Object FullName)
$artifacts = @($files | ForEach-Object {
    $relativePath = [System.IO.Path]::GetRelativePath($evidencePath, $_.FullName).Replace("\", "/")
    [ordered]@{
        category = Get-ArtifactCategory -RelativePath $relativePath
        path = $relativePath
        sha256 = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        bytes = $_.Length
    }
})
$manifest = [ordered]@{
    schema_version = 1
    scope = "Runtime execution proven only on this recorded Windows development machine."
    repository_revision = $revision
    reproduction_commands = @(
        [ordered]@{ category = "generated_artifacts"; command = "cargo build --locked --workspace --all-targets" },
        [ordered]@{ category = "formatting"; command = "cargo fmt --all -- --check" },
        [ordered]@{ category = "lint"; command = "cargo clippy --locked --workspace --all-targets --all-features -- -D warnings" },
        [ordered]@{ category = "unit_and_integration"; command = "cargo test --locked --workspace" },
        [ordered]@{ category = "voxel_frontend_read"; command = "cargo test --locked --package voxel-frontend" },
        [ordered]@{ category = "diagnostic_surface"; command = "cargo test --locked --package raster-render-path --test derivation" },
        [ordered]@{ category = "lifecycle"; command = "pwsh -NoProfile -File scripts/verify-windows-lifecycle.ps1 -EvidenceDirectory docs/evidence/milestone-completion/lifecycle-reproduction -CaptureCanonicalInspectionSet -VideoFile milestone-proof.mkv" },
        [ordered]@{ category = "deterministic_failure"; command = "cargo test --locked --package desktop-demo --test render_path_failures" },
        [ordered]@{ category = "prerequisite_regression"; command = "cargo test --locked --package desktop-demo --test unsupported_prerequisites" },
        [ordered]@{ category = "bundle_verification"; command = "cargo run --locked --package completion-evidence --bin verify-completion-evidence -- <bundle-directory>" }
    )
    video = [ordered]@{
        path = "lifecycle/milestone-proof.mkv"
        capture_scope = $retainedCaptureScope
        duration_seconds = $videoDuration
        codec = $videoStream.codec_name
        pixel_format = $videoStream.pix_fmt
        width = $videoStream.width
        height = $videoStream.height
        average_frame_rate = $videoStream.avg_frame_rate
        validation_warnings = $lifecycleManifest.ValidationWarnings
        validation_errors = $lifecycleManifest.ValidationErrors
        uninterrupted = $lifecycleManifest.CompletionVideo.Uninterrupted
        events = @($videoEvents | ForEach-Object Event)
    }
    artifacts = $artifacts
}
Write-JsonFile -Path (Join-Path $evidencePath "manifest.json") -Value $manifest -Depth 12

& cargo run --locked --package completion-evidence --bin verify-completion-evidence -- $evidencePath
if ($LASTEXITCODE -ne 0) {
    throw "Completion bundle verification failed."
}
Write-Host "Milestone completion evidence passed for clean checkout $revision at $evidencePath"
