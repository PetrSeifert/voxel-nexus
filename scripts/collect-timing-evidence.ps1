[CmdletBinding()]
param(
    [string]$OutputDirectory = "docs/evidence/timing-baseline/development-machine"
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

if (-not $IsWindows) {
    throw "Timing evidence collection is supported only on Windows."
}

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$outputPath = [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot $OutputDirectory))
$expectedRoot = [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot "docs/evidence/timing-baseline"))
$relativeOutputPath = [System.IO.Path]::GetRelativePath($expectedRoot, $outputPath)
if ([System.IO.Path]::IsPathRooted($relativeOutputPath) -or
    $relativeOutputPath -eq "." -or
    $relativeOutputPath -eq ".." -or
    $relativeOutputPath.StartsWith("..$([System.IO.Path]::DirectorySeparatorChar)")) {
    throw "The evidence output must remain under $expectedRoot."
}

function Invoke-CapturedProcess {
    param(
        [Parameter(Mandatory)] [string]$FilePath,
        [Parameter(Mandatory)] [string[]]$Arguments,
        [Parameter(Mandatory)] [string]$StandardOutputPath,
        [Parameter(Mandatory)] [string]$StandardErrorPath
    )

    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $FilePath
    $startInfo.WorkingDirectory = $repositoryRoot
    $startInfo.UseShellExecute = $false
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    foreach ($argument in $Arguments) {
        $startInfo.ArgumentList.Add($argument)
    }
    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    if (-not $process.Start()) {
        throw "Could not start $FilePath."
    }
    $standardOutputTask = $process.StandardOutput.ReadToEndAsync()
    $standardErrorTask = $process.StandardError.ReadToEndAsync()
    $process.WaitForExit()
    $standardOutput = $standardOutputTask.GetAwaiter().GetResult()
    $standardError = $standardErrorTask.GetAwaiter().GetResult()
    [System.IO.File]::WriteAllText($StandardOutputPath, $standardOutput)
    [System.IO.File]::WriteAllText($StandardErrorPath, $standardError)
    [pscustomobject]@{
        ExitCode = $process.ExitCode
        StandardOutput = $standardOutput
        StandardError = $standardError
    }
}

function Read-Events {
    param([Parameter(Mandatory)] [string]$Path)

    $events = @(Get-Content -LiteralPath $Path | ForEach-Object { $_ | ConvertFrom-Json })
    if ($events.Count -eq 0) {
        throw "Measurement file $Path is empty."
    }
    $events
}

function Read-FirstCorrectFrame {
    param([Parameter(Mandatory)] [string]$Path)

    $events = Read-Events -Path $Path
    $publication = @($events | Where-Object event -eq "scene_revision_published")
    $derivation = @($events | Where-Object event -eq "artifact_derived")
    $installation = @($events | Where-Object event -eq "artifact_installed")
    $presentation = @($events | Where-Object event -eq "matching_artifact_presented")
    if ($publication.Count -ne 1 -or $derivation.Count -ne 1 -or
        $installation.Count -ne 1 -or $presentation.Count -ne 1) {
        throw "First-correct-frame file $Path does not contain exactly one event per phase."
    }
    if ($publication[0].source_revision -ne $derivation[0].source_revision -or
        $publication[0].source_revision -ne $installation[0].source_revision -or
        $publication[0].source_revision -ne $presentation[0].source_revision) {
        throw "First-correct-frame file $Path crosses Voxel Scene Revisions."
    }
    if ($publication[0].elapsed_ms -ne 0 -or
        $derivation[0].elapsed_ms -gt $installation[0].elapsed_ms -or
        $installation[0].elapsed_ms -gt $presentation[0].elapsed_ms) {
        throw "First-correct-frame file $Path contains invalid phase ordering."
    }
    [ordered]@{
        SourceRevision = $publication[0].source_revision
        DerivationMilliseconds = [double]$derivation[0].elapsed_ms
        UploadInstallMilliseconds = [double]$installation[0].elapsed_ms - [double]$derivation[0].elapsed_ms
        PresentationMilliseconds = [double]$presentation[0].elapsed_ms - [double]$installation[0].elapsed_ms
        TotalMilliseconds = [double]$presentation[0].elapsed_ms
        Resources = $derivation[0].resources
        RawFile = [System.IO.Path]::GetFileName($Path)
    }
}

function Read-RuntimeContext {
    param([Parameter(Mandatory)] [string]$StandardOutput)

    $patterns = [ordered]@{
        Device = "(?m)^Vulkan device: (.+)$"
        DriverVersion = "(?m)^Driver version: (.+)$"
        ApiVersion = "(?m)^Vulkan API version: (.+)$"
        DrawableExtent = "(?m)^Vulkan drawable extent: (.+)$"
        Validation = "(?m)^Vulkan validation: (.+)$"
        PresentMode = "(?m)^Vulkan present mode: (.+)$"
        TimestampValidBits = "(?m)^GPU timestamp valid bits: (.+)$"
        TimestampPeriodNanoseconds = "(?m)^GPU timestamp period nanoseconds: (.+)$"
    }
    $context = [ordered]@{}
    foreach ($entry in $patterns.GetEnumerator()) {
        $match = [regex]::Match($StandardOutput, $entry.Value)
        if (-not $match.Success) {
            throw "Runtime output is missing $($entry.Key)."
        }
        $context[$entry.Key] = $match.Groups[1].Value.Trim()
    }
    $context
}

function Get-CanonicalMetadata {
    param([Parameter(Mandatory)] [string]$StandardOutput)

    $scene = [regex]::Match(
        $StandardOutput,
        "Canonical scene: generator=(\S+) version=(\d+) seed=(\d+) dimensions=(\d+)x(\d+)x(\d+) origin=([^ ]+) voxel_size=([^ ]+) materials=([^ ]+) material_colors=([^ ]+) occupied=(\d+) exposed_faces=(\d+) exposed_face_limit=(\d+)"
    )
    $camera = [regex]::Match(
        $StandardOutput,
        "Canonical camera: camera=(\S+) eye=([^ ]+) target=([^ ]+) up=([^ ]+) fov_degrees=([^ ]+) near=([^ ]+) far=([^\r\n]+)"
    )
    if (-not $scene.Success -or -not $camera.Success) {
        throw "Runtime output is missing canonical scene or camera metadata."
    }
    [ordered]@{
        Scene = [ordered]@{
            GeneratorIdentity = $scene.Groups[1].Value
            GeneratorVersion = [int]$scene.Groups[2].Value
            Seed = [uint64]$scene.Groups[3].Value
            Dimensions = @([int]$scene.Groups[4].Value, [int]$scene.Groups[5].Value, [int]$scene.Groups[6].Value)
            Origin = @($scene.Groups[7].Value.Split(",") | ForEach-Object { [double]$_ })
            VoxelSize = [double]$scene.Groups[8].Value
            MaterialCatalogue = @($scene.Groups[9].Value.Split(","))
            MaterialLinearBaseColors = @($scene.Groups[10].Value.Split(";") | ForEach-Object {
                , @($_.Split(",") | ForEach-Object { [double]$_ })
            })
            OccupiedVoxels = [uint64]$scene.Groups[11].Value
            ExposedQuads = [uint64]$scene.Groups[12].Value
            ExposedQuadLimit = [uint64]$scene.Groups[13].Value
        }
        Camera = [ordered]@{
            Selection = $camera.Groups[1].Value
            Eye = @($camera.Groups[2].Value.Split(",") | ForEach-Object { [double]$_ })
            Target = @($camera.Groups[3].Value.Split(",") | ForEach-Object { [double]$_ })
            Up = @($camera.Groups[4].Value.Split(",") | ForEach-Object { [double]$_ })
            FieldOfViewDegrees = [double]$camera.Groups[5].Value
            NearPlane = [double]$camera.Groups[6].Value
            FarPlane = [double]$camera.Groups[7].Value
        }
    }
}

function Write-ComparisonChart {
    param(
        [Parameter(Mandatory)] [object[]]$Scales,
        [Parameter(Mandatory)] [string]$Path
    )

    $colors = @("#4c78a8", "#f58518", "#54a24b")
    $panels = @(
        @{ Name = "First correct frame median (ms)"; Value = { param($scale) $scale.FirstCorrectFrame.Total.median } },
        @{ Name = "Steady CPU frame median (ms)"; Value = { param($scale) $scale.SteadyState.CpuFrame.median } },
        @{ Name = "Steady GPU frame median (ms)"; Value = { param($scale) $scale.SteadyState.GpuFrame.median } }
    )
    $svg = [System.Text.StringBuilder]::new()
    [void]$svg.AppendLine('<svg xmlns="http://www.w3.org/2000/svg" width="900" height="520" viewBox="0 0 900 520">')
    [void]$svg.AppendLine('<rect width="900" height="520" fill="#ffffff"/><text x="30" y="35" font-family="sans-serif" font-size="22">Voxel Nexus descriptive timing baseline</text>')
    for ($panelIndex = 0; $panelIndex -lt $panels.Count; $panelIndex++) {
        $panel = $panels[$panelIndex]
        $top = 65 + 145 * $panelIndex
        [void]$svg.AppendLine("<text x=`"30`" y=`"$top`" font-family=`"sans-serif`" font-size=`"15`">$($panel.Name)</text>")
        $values = @($Scales | ForEach-Object { [double](& $panel.Value $_) })
        $maximum = ($values | Measure-Object -Maximum).Maximum
        for ($scaleIndex = 0; $scaleIndex -lt $Scales.Count; $scaleIndex++) {
            $value = $values[$scaleIndex]
            $width = if ($maximum -eq 0) { 0 } else { 650 * $value / $maximum }
            $y = $top + 14 + 32 * $scaleIndex
            [void]$svg.AppendLine("<text x=`"30`" y=`"$($y + 17)`" font-family=`"monospace`" font-size=`"13`">$($Scales[$scaleIndex].Scale)</text>")
            [void]$svg.AppendLine("<rect x=`"85`" y=`"$y`" width=`"$width`" height=`"21`" fill=`"$($colors[$scaleIndex])`"/>")
            [void]$svg.AppendLine("<text x=`"$([Math]::Min(750, 95 + $width))`" y=`"$($y + 16)`" font-family=`"monospace`" font-size=`"12`">$($value.ToString('0.###'))</text>")
        }
    }
    [void]$svg.AppendLine('<text x="30" y="505" font-family="sans-serif" font-size="12" fill="#555">Medians describe this recorded Windows/Vulkan run; no performance pass/fail threshold is applied.</text></svg>')
    [System.IO.File]::WriteAllText($Path, $svg.ToString())
}

Set-Location $repositoryRoot
$dirty = @(git status --porcelain)
if ($LASTEXITCODE -ne 0) {
    throw "Could not inspect repository status."
}
if ($dirty.Count -ne 0) {
    throw "Commit code before collecting revision-attributed evidence."
}
$revision = (git rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0 -or $revision -notmatch '^[0-9a-f]{40}$') {
    throw "Could not resolve the repository revision."
}

if (Test-Path -LiteralPath $outputPath) {
    Remove-Item -LiteralPath $outputPath -Recurse -Force
}
[System.IO.Directory]::CreateDirectory($outputPath) | Out-Null

$build = Invoke-CapturedProcess -FilePath "cargo" `
    -Arguments @("build", "--locked", "--release", "--package", "desktop-demo", "--package", "measurement-evidence") `
    -StandardOutputPath (Join-Path $outputPath "build.stdout.log") `
    -StandardErrorPath (Join-Path $outputPath "build.stderr.log")
if ($build.ExitCode -ne 0) {
    throw "The release build failed with exit code $($build.ExitCode)."
}

$diagnosticCommands = @(
    @{ Name = "canonical-generation"; Arguments = @("test", "--locked", "--release", "--package", "canonical-scene", "--test", "generation") },
    @{ Name = "semantic-face-oracles"; Arguments = @("test", "--locked", "--release", "--package", "raster-render-path", "--test", "derivation") },
    @{ Name = "measurement-contract"; Arguments = @("test", "--locked", "--release", "--package", "measurement-evidence", "--test", "public_contract") }
)
$diagnostics = @()
foreach ($diagnostic in $diagnosticCommands) {
    $result = Invoke-CapturedProcess -FilePath "cargo" -Arguments $diagnostic.Arguments `
        -StandardOutputPath (Join-Path $outputPath "$($diagnostic.Name).stdout.log") `
        -StandardErrorPath (Join-Path $outputPath "$($diagnostic.Name).stderr.log")
    $diagnostics += [ordered]@{
        Name = $diagnostic.Name
        Command = "cargo $($diagnostic.Arguments -join ' ')"
        ExitCode = $result.ExitCode
        Passed = $result.ExitCode -eq 0
        StandardOutput = "$($diagnostic.Name).stdout.log"
        StandardError = "$($diagnostic.Name).stderr.log"
    }
    if ($result.ExitCode -ne 0) {
        throw "Correctness diagnostic $($diagnostic.Name) failed."
    }
}
$diagnostics | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $outputPath "diagnostics.json")

$binaryPath = Join-Path $repositoryRoot "target/release/desktop-demo.exe"
$scaleRecords = @()
$aggregationInputs = @()
$runtimeContext = $null
foreach ($scale in @(64, 128, 256)) {
    $firstCorrectFrameSamples = @()
    $canonicalMetadata = $null
    for ($run = 1; $run -le 10; $run++) {
        $stem = "first-correct-frame-$scale-$($run.ToString('00'))"
        $rawPath = Join-Path $outputPath "$stem.jsonl"
        $result = Invoke-CapturedProcess -FilePath $binaryPath `
            -Arguments @("--scene-scale", "$scale", "--measurement-mode", "first-correct-frame", "--measurement-output", $rawPath) `
            -StandardOutputPath (Join-Path $outputPath "$stem.stdout.log") `
            -StandardErrorPath (Join-Path $outputPath "$stem.stderr.log")
        if ($result.ExitCode -ne 0) {
            throw "Fresh first-correct-frame run $run at scale $scale failed."
        }
        $sample = Read-FirstCorrectFrame -Path $rawPath
        $firstCorrectFrameSamples += $sample
        if ($null -eq $canonicalMetadata) {
            $canonicalMetadata = Get-CanonicalMetadata -StandardOutput $result.StandardOutput
        }
    }
    if ($firstCorrectFrameSamples.Count -ne 10) {
        throw "Scale $scale retained $($firstCorrectFrameSamples.Count) fresh samples instead of ten."
    }

    $steadyStem = "steady-state-$scale"
    $steadyRawPath = Join-Path $outputPath "$steadyStem.jsonl"
    $steady = Invoke-CapturedProcess -FilePath $binaryPath `
        -Arguments @("--scene-scale", "$scale", "--measurement-mode", "steady-state", "--measurement-output", $steadyRawPath) `
        -StandardOutputPath (Join-Path $outputPath "$steadyStem.stdout.log") `
        -StandardErrorPath (Join-Path $outputPath "$steadyStem.stderr.log")
    if ($steady.ExitCode -ne 0) {
        throw "Steady-state run at scale $scale failed."
    }
    $runtime = Read-RuntimeContext -StandardOutput $steady.StandardOutput
    if ($runtime.Validation -ne "disabled" -or $runtime.PresentMode -ne "IMMEDIATE" -or
        $runtime.DrawableExtent -ne "1920x1080" -or
        [int]$runtime.TimestampValidBits -le 0 -or [double]$runtime.TimestampPeriodNanoseconds -le 0) {
        throw "Scale $scale did not run with the required valid, unthrottled measurement context."
    }
    if ($null -eq $runtimeContext) {
        $runtimeContext = $runtime
    } elseif (($runtimeContext | ConvertTo-Json -Compress) -ne ($runtime | ConvertTo-Json -Compress)) {
        throw "Vulkan runtime context changed between scales."
    }
    $steadyEvents = Read-Events -Path $steadyRawPath
    $steadyFrames = @($steadyEvents | Where-Object event -eq "steady_frame")
    if ($steadyFrames.Count -eq 0) {
        throw "Scale $scale retained no steady-state frames."
    }
    $previousSequence = 0
    foreach ($frame in $steadyFrames) {
        if ([uint64]$frame.sequence -le $previousSequence) {
            throw "Scale $scale steady-state sequence is not strictly increasing."
        }
        $previousSequence = [uint64]$frame.sequence
    }

    $aggregationInputs += [ordered]@{
        scale = $scale
        first_correct_frame_samples = @($firstCorrectFrameSamples | ForEach-Object {
            [ordered]@{
                derivation_milliseconds = $_.DerivationMilliseconds
                upload_install_milliseconds = $_.UploadInstallMilliseconds
                presentation_milliseconds = $_.PresentationMilliseconds
                total_milliseconds = $_.TotalMilliseconds
            }
        })
        cpu_frame_milliseconds = @($steadyFrames.cpu_frame_ms)
        gpu_frame_milliseconds = @($steadyFrames.gpu_frame_ms)
    }
    $scaleRecords += [ordered]@{
        Scale = $scale
        Scene = $canonicalMetadata.Scene
        Camera = $canonicalMetadata.Camera
        Resources = $firstCorrectFrameSamples[0].Resources
        FirstCorrectFrame = [ordered]@{
            FreshReleaseRuns = 10
            Samples = $firstCorrectFrameSamples
        }
        SteadyState = [ordered]@{
            DrawableExtent = @($runtime.DrawableExtent.Split("x") | ForEach-Object { [int]$_ })
            WarmupSeconds = 5
            CollectionSeconds = 30
            ValidationEnabled = $false
            PresentationThrottlingEnabled = $false
            RawFile = "$steadyStem.jsonl"
        }
    }
}

$aggregationInputPath = Join-Path $outputPath "aggregation-input.json"
$aggregationOutputPath = Join-Path $outputPath "aggregation-output.json"
$aggregationInputs | ConvertTo-Json -Depth 10 -Compress | Set-Content -LiteralPath $aggregationInputPath
$aggregation = Invoke-CapturedProcess `
    -FilePath (Join-Path $repositoryRoot "target/release/measurement-evidence-report.exe") `
    -Arguments @($aggregationInputPath, $aggregationOutputPath) `
    -StandardOutputPath (Join-Path $outputPath "aggregation.stdout.log") `
    -StandardErrorPath (Join-Path $outputPath "aggregation.stderr.log")
if ($aggregation.ExitCode -ne 0) {
    throw "The tested evidence aggregator rejected the collected samples."
}
$aggregationReport = @(Get-Content -Raw -LiteralPath $aggregationOutputPath | ConvertFrom-Json)
[System.IO.File]::Delete($aggregationInputPath)
foreach ($scaleRecord in $scaleRecords) {
    $summary = @($aggregationReport | Where-Object scale -eq $scaleRecord.Scale)
    if ($summary.Count -ne 1) {
        throw "Aggregation output does not contain exactly one summary for scale $($scaleRecord.Scale)."
    }
    $scaleRecord.FirstCorrectFrame.Derivation = $summary[0].derivation
    $scaleRecord.FirstCorrectFrame.UploadInstall = $summary[0].upload_install
    $scaleRecord.FirstCorrectFrame.Presentation = $summary[0].presentation
    $scaleRecord.FirstCorrectFrame.Total = $summary[0].total
    $scaleRecord.SteadyState.CpuFrame = $summary[0].cpu_frame
    $scaleRecord.SteadyState.GpuFrame = $summary[0].gpu_frame
}

$operatingSystem = Get-CimInstance Win32_OperatingSystem
$machine = [ordered]@{
    OperatingSystem = $operatingSystem.Caption
    OperatingSystemVersion = $operatingSystem.Version
    Rust = (& rustc --version)
    Cargo = (& cargo --version)
    VulkanSdk = $env:VULKAN_SDK
}
Write-ComparisonChart -Scales $scaleRecords -Path (Join-Path $outputPath "comparison.svg")
$rawFiles = @(Get-ChildItem -LiteralPath $outputPath -File | Sort-Object Name | ForEach-Object {
    [ordered]@{
        File = $_.Name
        Sha256 = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        Bytes = $_.Length
    }
})
$manifest = [ordered]@{
    SchemaVersion = 1
    Scope = "Descriptive runtime evidence for this recorded Windows development machine only."
    RecordedAtUtc = [DateTime]::UtcNow.ToString("o")
    RepositoryRevision = $revision
    BuildProfile = "release"
    BuildCommand = "cargo build --locked --release --package desktop-demo --package measurement-evidence"
    Machine = $machine
    VulkanRuntime = $runtimeContext
    Correctness = [ordered]@{
        PerformanceThresholdsApplied = $false
        DiagnosticsFile = "diagnostics.json"
        Gates = $diagnostics
    }
    Scales = $scaleRecords
    RawFiles = $rawFiles
    ComparisonChart = "comparison.svg"
}
$manifest | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath (Join-Path $outputPath "manifest.json")

Write-Host "Recorded timing evidence for revision $revision at $outputPath"
