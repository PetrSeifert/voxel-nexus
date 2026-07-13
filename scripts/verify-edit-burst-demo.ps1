[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$EvidenceDirectory,
    [ValidateSet(16, 32, 64)]
    [int]$RasterRegionExtent = 32,
    [switch]$TimingOnly,
    [switch]$SkipBuild
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if (-not $IsWindows -and $PSVersionTable.PSEdition -eq "Core") {
    throw "The edit-burst proof runs only on Windows."
}

$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$evidencePath = [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot $EvidenceDirectory))
if ([System.IO.Directory]::Exists($evidencePath) -and [System.IO.Directory]::EnumerateFileSystemEntries($evidencePath).GetEnumerator().MoveNext()) {
    throw "The evidence directory must be new or empty: $evidencePath"
}
[System.IO.Directory]::CreateDirectory($evidencePath) | Out-Null

Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class EditBurstWindow {
    public const int ShowMinimized = 6;
    public const int ShowRestored = 9;
    public const uint CloseMessage = 0x0010;
    public const uint OverviewCameraMessage = 0x801C;
    public const uint CavityCameraMessage = 0x801D;
    public const uint BoundaryCameraMessage = 0x801E;
    public const uint KeyDownMessage = 0x0100;
    public const uint KeyUpMessage = 0x0101;
    public const uint SpaceKey = 0x20;
    public const uint ReleaseCpuBarrierMessage = 0x8021;
    public const uint ReleasePostUploadBarrierMessage = 0x8022;
    public const uint ReleasePostUploadLifecycleBarrierMessage = 0x8023;
    public const uint KeepPositionAndSize = 0x0013;
    public static readonly IntPtr Topmost = new IntPtr(-1);

    public delegate bool EnumWindowsCallback(IntPtr window, IntPtr parameter);

    [StructLayout(LayoutKind.Sequential)]
    public struct Rect {
        public int Left;
        public int Top;
        public int Right;
        public int Bottom;
    }

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool MoveWindow(IntPtr window, int x, int y, int width, int height, bool repaint);

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool SetWindowPos(IntPtr window, IntPtr insertAfter, int x, int y, int width, int height, uint flags);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool SetForegroundWindow(IntPtr window);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool ShowWindowAsync(IntPtr window, int command);

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool PostMessage(IntPtr window, uint message, IntPtr parameter, IntPtr data);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool IsIconic(IntPtr window);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool EnumWindows(EnumWindowsCallback callback, IntPtr parameter);

    [DllImport("user32.dll")]
    public static extern uint GetWindowThreadProcessId(IntPtr window, out uint processId);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern int GetWindowText(IntPtr window, StringBuilder text, int capacity);

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool GetWindowRect(IntPtr window, out Rect rectangle);

    [DllImport("dwmapi.dll")]
    public static extern int DwmFlush();

    public static IntPtr Find(uint processId) {
        IntPtr result = IntPtr.Zero;
        EnumWindows(delegate(IntPtr window, IntPtr parameter) {
            uint windowProcessId;
            GetWindowThreadProcessId(window, out windowProcessId);
            if (windowProcessId != processId) return true;
            var title = new StringBuilder(1024);
            GetWindowText(window, title, title.Capacity);
            if (title.ToString().StartsWith("Voxel Nexus Vulkan Demo", StringComparison.Ordinal)) {
                result = window;
                return false;
            }
            return true;
        }, IntPtr.Zero);
        return result;
    }
}
"@

function Wait-ForWindow {
    param([System.Diagnostics.Process]$Process)
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    while ([DateTime]::UtcNow -lt $deadline) {
        if ($Process.HasExited) { throw "The desktop demo exited before creating its window." }
        $window = [EditBurstWindow]::Find([uint32]$Process.Id)
        if ($window -ne [IntPtr]::Zero) { return $window }
        Start-Sleep -Milliseconds 25
    }
    throw "The desktop demo did not create its window within 10 seconds."
}

function Get-WindowTitle {
    param([IntPtr]$Window)
    $title = [System.Text.StringBuilder]::new(1024)
    [EditBurstWindow]::GetWindowText($Window, $title, $title.Capacity) | Out-Null
    $title.ToString()
}

function Wait-ForTitle {
    param(
        [System.Diagnostics.Process]$Process,
        [IntPtr]$Window,
        [string]$Pattern,
        [int]$TimeoutSeconds = 30
    )
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        if ($Process.HasExited) { throw "The desktop demo exited while waiting for title '$Pattern'." }
        $title = Get-WindowTitle -Window $Window
        if ($title -match $Pattern) { return $title }
        Start-Sleep -Milliseconds 25
    }
    throw "The desktop demo did not report '$Pattern' within $TimeoutSeconds seconds. Last title: $(Get-WindowTitle -Window $Window)"
}

function Send-Event {
    param([IntPtr]$Window, [uint32]$Message, [string]$Name)
    if (-not [EditBurstWindow]::PostMessage($Window, $Message, [IntPtr]::Zero, [IntPtr]::Zero)) {
        throw "Could not send $Name."
    }
}

function Send-SpaceKeyPress {
    param([IntPtr]$Window)
    [EditBurstWindow]::SetForegroundWindow($Window) | Out-Null
    if (-not [EditBurstWindow]::PostMessage($Window, [EditBurstWindow]::KeyDownMessage, [IntPtr][EditBurstWindow]::SpaceKey, [IntPtr]::Zero)) {
        throw "Could not send the Space key-down event."
    }
    if (-not [EditBurstWindow]::PostMessage($Window, [EditBurstWindow]::KeyUpMessage, [IntPtr][EditBurstWindow]::SpaceKey, [IntPtr]::Zero)) {
        throw "Could not send the Space key-up event."
    }
}

function Save-WindowCapture {
    param([IntPtr]$Window, [string]$Name)
    [EditBurstWindow]::SetForegroundWindow($Window) | Out-Null
    if (-not [EditBurstWindow]::SetWindowPos($Window, [EditBurstWindow]::Topmost, 0, 0, 0, 0, [EditBurstWindow]::KeepPositionAndSize)) {
        throw "Could not place the desktop demo above other windows for capture."
    }
    [EditBurstWindow]::DwmFlush() | Out-Null
    Start-Sleep -Milliseconds 100
    $rectangle = [EditBurstWindow+Rect]::new()
    if (-not [EditBurstWindow]::GetWindowRect($Window, [ref]$rectangle)) {
        throw "Could not read the desktop demo window rectangle."
    }
    $width = $rectangle.Right - $rectangle.Left
    $height = $rectangle.Bottom - $rectangle.Top
    $bitmap = [System.Drawing.Bitmap]::new($width, $height)
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    try {
        $graphics.CopyFromScreen($rectangle.Left, $rectangle.Top, 0, 0, [System.Drawing.Size]::new($width, $height))
        $path = Join-Path $evidencePath "$Name.png"
        $bitmap.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    }
    finally {
        $graphics.Dispose()
        $bitmap.Dispose()
    }
    $file = Get-Item -LiteralPath $path
    [ordered]@{
        File = $file.Name
        Bytes = $file.Length
        Sha256 = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
        WindowTitle = Get-WindowTitle -Window $Window
    }
}

function Exercise-BarrierLifecycle {
    param(
        [System.Diagnostics.Process]$Process,
        [IntPtr]$Window,
        [uint32]$CameraMessage,
        [string]$CameraName,
        [string]$StagePattern,
        [string]$CaptureName
    )
    Send-Event -Window $Window -Message $CameraMessage -Name "$CameraName camera selection"
    Wait-ForTitle -Process $Process -Window $Window -Pattern "$StagePattern.*Camera=$CameraName" | Out-Null
    if (-not [EditBurstWindow]::MoveWindow($Window, 80, 80, 1100, 700, $true)) {
        throw "Could not resize the held desktop demo to landscape."
    }
    Wait-ForTitle -Process $Process -Window $Window -Pattern $StagePattern | Out-Null
    if (-not [EditBurstWindow]::MoveWindow($Window, 80, 80, 650, 900, $true)) {
        throw "Could not resize the held desktop demo to portrait."
    }
    Wait-ForTitle -Process $Process -Window $Window -Pattern $StagePattern | Out-Null
    [EditBurstWindow]::ShowWindowAsync($Window, [EditBurstWindow]::ShowMinimized) | Out-Null
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    while (-not [EditBurstWindow]::IsIconic($Window) -and [DateTime]::UtcNow -lt $deadline) {
        Start-Sleep -Milliseconds 25
    }
    if (-not [EditBurstWindow]::IsIconic($Window)) { throw "The held demo did not minimize." }
    [EditBurstWindow]::ShowWindowAsync($Window, [EditBurstWindow]::ShowRestored) | Out-Null
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    while ([EditBurstWindow]::IsIconic($Window) -and [DateTime]::UtcNow -lt $deadline) {
        Start-Sleep -Milliseconds 25
    }
    if ([EditBurstWindow]::IsIconic($Window)) { throw "The held demo did not restore." }
    if (-not [EditBurstWindow]::MoveWindow($Window, 80, 80, 1200, 800, $true)) {
        throw "Could not restore the held desktop demo capture extent."
    }
    if ($Process.HasExited) { throw "The desktop demo exited during held lifecycle actions." }
    Save-WindowCapture -Window $Window -Name $CaptureName
}

function Invoke-InFlightCloseQualification {
    param(
        [string]$Name,
        [string[]]$Arguments,
        [string]$StatePattern,
        [string]$RequiredOutput = ""
    )
    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = Join-Path $repositoryRoot "target\debug\desktop-demo.exe"
    $startInfo.WorkingDirectory = $repositoryRoot
    $startInfo.UseShellExecute = $false
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    foreach ($argument in $Arguments) {
        $startInfo.ArgumentList.Add($argument)
    }
    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    $started = $false
    $standardOutputTask = $null
    $standardErrorTask = $null
    try {
        if (-not $process.Start()) { throw "Could not start the $Name shutdown qualification." }
        $started = $true
        $standardOutputTask = $process.StandardOutput.ReadToEndAsync()
        $standardErrorTask = $process.StandardError.ReadToEndAsync()
        $window = Wait-ForWindow -Process $process
        Wait-ForTitle -Process $process -Window $window -Pattern $StatePattern -TimeoutSeconds 180 | Out-Null
        Send-Event -Window $window -Message ([EditBurstWindow]::CloseMessage) -Name "$Name normal close"
        if (-not $process.WaitForExit(10000)) {
            throw "The $Name shutdown qualification did not close within 10 seconds."
        }
        $standardOutput = $standardOutputTask.GetAwaiter().GetResult()
        $standardError = $standardErrorTask.GetAwaiter().GetResult()
        [System.IO.File]::WriteAllText((Join-Path $evidencePath "$Name.stdout.log"), $standardOutput)
        [System.IO.File]::WriteAllText((Join-Path $evidencePath "$Name.stderr.log"), $standardError)
        if ($process.ExitCode -ne 0) {
            throw "The $Name shutdown qualification exited with code $($process.ExitCode)."
        }
        foreach ($required in @(
            "Vulkan validation: enabled",
            "Render Path-owned raster resources after shutdown: 0"
        )) {
            if ($standardOutput -notmatch [Regex]::Escape($required)) {
                throw "The $Name shutdown qualification is missing '$required'."
            }
        }
        if ($RequiredOutput -and $standardOutput -notmatch [Regex]::Escape($RequiredOutput)) {
            throw "The $Name shutdown qualification is missing '$RequiredOutput'."
        }
        $validationWarnings = ([Regex]::Matches($standardError, "(?m)^Vulkan validation WARNING")).Count
        $validationErrors = ([Regex]::Matches($standardError, "(?m)^Vulkan validation ERROR")).Count
        if ($validationWarnings -ne 0 -or $validationErrors -ne 0) {
            throw "The $Name shutdown qualification reported $validationWarnings warning(s) and $validationErrors error(s)."
        }
        [ordered]@{
            Passed = $true
            StatePattern = $StatePattern
            ProcessExitCode = $process.ExitCode
            OwnedResourcesAfterShutdown = 0
            ValidationWarnings = $validationWarnings
            ValidationErrors = $validationErrors
            StandardOutput = "$Name.stdout.log"
            StandardError = "$Name.stderr.log"
        }
    }
    finally {
        if ($started -and -not $process.HasExited) {
            $process.Kill()
            $process.WaitForExit()
        }
        if ($null -ne $standardOutputTask -and -not (Test-Path (Join-Path $evidencePath "$Name.stdout.log"))) {
            [System.IO.File]::WriteAllText(
                (Join-Path $evidencePath "$Name.stdout.log"),
                $standardOutputTask.GetAwaiter().GetResult()
            )
        }
        if ($null -ne $standardErrorTask -and -not (Test-Path (Join-Path $evidencePath "$Name.stderr.log"))) {
            [System.IO.File]::WriteAllText(
                (Join-Path $evidencePath "$Name.stderr.log"),
                $standardErrorTask.GetAwaiter().GetResult()
            )
        }
    }
}

Push-Location $repositoryRoot
$process = $null
$standardOutputTask = $null
$standardErrorTask = $null
try {
    if (-not $SkipBuild) {
        & cargo build --locked --package desktop-demo
        if ($LASTEXITCODE -ne 0) { throw "The desktop demo build failed with exit code $LASTEXITCODE." }
    }
    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = Join-Path $repositoryRoot "target\debug\desktop-demo.exe"
    $startInfo.WorkingDirectory = $repositoryRoot
    $startInfo.UseShellExecute = $false
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    foreach ($argument in @("--scene-scale", "256", "--camera-pose", "overview", "--raster-region-extent", "$RasterRegionExtent", "--edit-burst-demo")) {
        $startInfo.ArgumentList.Add($argument)
    }
    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    if (-not $process.Start()) { throw "Could not start the desktop edit-burst demo." }
    $standardOutputTask = $process.StandardOutput.ReadToEndAsync()
    $standardErrorTask = $process.StandardError.ReadToEndAsync()
    $window = Wait-ForWindow -Process $process
    $totalRegionCount = (256 / $RasterRegionExtent) * (128 / $RasterRegionExtent) * (256 / $RasterRegionExtent)
    $secondRequirementAffectedCount = if ($RasterRegionExtent -eq 64) { 1 } else { 2 }
    $finalAffectedCount = switch ($RasterRegionExtent) {
        16 { 4 }
        32 { 3 }
        64 { 2 }
    }
    $initialTitle = Wait-ForTitle -Process $process -Window $window -Pattern "EditBurst=awaiting-key Required=1 Visible=1 Affected=0 Unaffected=$totalRegionCount" -TimeoutSeconds 120
    $captures = @()
    if (-not $TimingOnly) {
        $captures += Save-WindowCapture -Window $window -Name "before-burst"
    }

    Send-SpaceKeyPress -Window $window
    $cpuHeldTitle = Wait-ForTitle -Process $process -Window $window -Pattern "EditBurst=cpu-barrier-held Required=3 Visible=1" -TimeoutSeconds 30
    if (-not $TimingOnly) {
        $captures += Exercise-BarrierLifecycle -Process $process -Window $window -CameraMessage ([EditBurstWindow]::CavityCameraMessage) -CameraName "cavity" -StagePattern "EditBurst=cpu-barrier-held Required=3 Visible=1" -CaptureName "cpu-barrier-held"
    }
    Send-Event -Window $window -Message ([EditBurstWindow]::ReleaseCpuBarrierMessage) -Name "CPU barrier release"

    Wait-ForTitle -Process $process -Window $window -Pattern "EditBurst=post-upload-candidate-held Required=3 Visible=1" -TimeoutSeconds 30 | Out-Null
    if (-not $TimingOnly) {
        $captures += Exercise-BarrierLifecycle -Process $process -Window $window -CameraMessage ([EditBurstWindow]::BoundaryCameraMessage) -CameraName "boundary" -StagePattern "EditBurst=post-upload-candidate-held Required=3 Visible=1" -CaptureName "post-upload-barrier-held"
    }
    Send-Event -Window $window -Message ([EditBurstWindow]::ReleasePostUploadLifecycleBarrierMessage) -Name "post-upload lifecycle barrier release"
    $postUploadHeldTitle = Wait-ForTitle -Process $process -Window $window -Pattern "EditBurst=post-upload-barrier-held Required=4 Visible=1" -TimeoutSeconds 30
    if (-not $TimingOnly) {
        Send-Event -Window $window -Message ([EditBurstWindow]::OverviewCameraMessage) -Name "overview camera during final post-upload hold"
        Wait-ForTitle -Process $process -Window $window -Pattern "EditBurst=post-upload-barrier-held Required=4 Visible=1.*Camera=overview" | Out-Null
    }
    Send-Event -Window $window -Message ([EditBurstWindow]::ReleasePostUploadBarrierMessage) -Name "post-upload barrier release"

    $finalTitle = Wait-ForTitle -Process $process -Window $window -Pattern "EditBurst=complete Required=4 Visible=4" -TimeoutSeconds 30
    if (-not $TimingOnly) {
        $captures += Save-WindowCapture -Window $window -Name "final-visible"
    }
    Send-Event -Window $window -Message ([EditBurstWindow]::CloseMessage) -Name "normal close"
    if (-not $process.WaitForExit(10000)) { throw "The edit-burst demo did not close within 10 seconds." }
    $standardOutput = $standardOutputTask.GetAwaiter().GetResult()
    $standardError = $standardErrorTask.GetAwaiter().GetResult()
    [System.IO.File]::WriteAllText((Join-Path $evidencePath "desktop-demo.stdout.log"), $standardOutput)
    [System.IO.File]::WriteAllText((Join-Path $evidencePath "desktop-demo.stderr.log"), $standardError)
    if ($process.ExitCode -ne 0) { throw "The edit-burst demo exited with code $($process.ExitCode)." }

    foreach ($required in @(
        "Vulkan validation: enabled",
        "In-client convergence overlay created",
        "Edit burst started by one keypress",
        "Edit burst command published: revision=2",
        "Edit burst command published: revision=3",
        "Edit burst command published: revision=4",
        "raster_region_extent=$($RasterRegionExtent)x$($RasterRegionExtent)x$($RasterRegionExtent)",
        "Obsolete CPU generation cancelled: scheduled_regions_before_hold=1 scheduled_regions_total=1",
        "Superseded candidate held after upload: revision=Some(VoxelSceneRevision(3))",
        "Post-upload lifecycle barrier released; waiting for restored candidate",
        "Superseded candidate rejected at commit: revision=3",
        "Edit burst converged atomically: visible_revision=4 expected_final_revision=4",
        "Render Path-owned raster resources after shutdown: 0"
    )) {
        if ($standardOutput -notmatch [Regex]::Escape($required)) { throw "The edit-burst proof is missing '$required'." }
    }
    if ($standardOutput -match "Edit burst overlay: .*Visible=(2|3)(\D|$)") {
        throw "An intermediate Voxel Scene Revision became visible."
    }
    $retirement = [Regex]::Match($standardOutput, "Superseded candidate rejected at commit: revision=3 retired_resources=(?<Count>\d+)")
    if (-not $retirement.Success -or [int]$retirement.Groups["Count"].Value -le 0) {
        throw "The superseded configured candidate did not report retired GPU resources."
    }
    $measurement = [Regex]::Match($standardOutput, "Edit burst final-visible measurement: elapsed_ms=(?<Latency>[0-9]+(?:\.[0-9]+)?) peak_live_gpu_bytes=(?<Bytes>\d+) peak_live_gpu_resources=(?<Resources>\d+)")
    if (-not $measurement.Success) {
        throw "The edit-burst proof did not report final-visible latency and peak live GPU resources."
    }
    $latencyMilliseconds = [double]::Parse($measurement.Groups["Latency"].Value, [Globalization.CultureInfo]::InvariantCulture)
    $peakLiveGpuBytes = [uint64]$measurement.Groups["Bytes"].Value
    $peakLiveGpuResources = [uint64]$measurement.Groups["Resources"].Value
    if ($latencyMilliseconds -lt 0 -or $peakLiveGpuBytes -eq 0 -or $peakLiveGpuResources -eq 0) {
        throw "The edit-burst measurement reported invalid latency or GPU resource values."
    }
    foreach ($title in @($cpuHeldTitle, $postUploadHeldTitle, $finalTitle)) {
        if ($title -notmatch "Affected=(?<Affected>\d+) Unaffected=(?<Unaffected>\d+)") {
            throw "The edit-burst title did not report localization counts: $title"
        }
    }
    if ($cpuHeldTitle -notmatch "Affected=$secondRequirementAffectedCount Unaffected=$($totalRegionCount - $secondRequirementAffectedCount)") {
        throw "The second requirement localization counts do not match extent $RasterRegionExtent."
    }
    if ($finalTitle -notmatch "Affected=$finalAffectedCount Unaffected=$($totalRegionCount - $finalAffectedCount)") {
        throw "The final requirement localization counts do not match extent $RasterRegionExtent."
    }
    $validationWarnings = ([Regex]::Matches($standardError, "(?m)^Vulkan validation WARNING")).Count
    $validationErrors = ([Regex]::Matches($standardError, "(?m)^Vulkan validation ERROR")).Count
    if ($validationWarnings -ne 0 -or $validationErrors -ne 0) {
        throw "Validation reported $validationWarnings warning(s) and $validationErrors error(s)."
    }

    $shutdownQualification = $null
    if (-not $TimingOnly) {
        $commonArguments = @(
            "--scene-scale", "256", "--camera-pose", "overview",
            "--raster-region-extent", "$RasterRegionExtent"
        )
        $activeCpuClose = Invoke-InFlightCloseQualification `
            -Name "active-cpu-close" `
            -Arguments ($commonArguments + @("--hold-background-preparation")) `
            -StatePattern "preparation-paused revision 1"
        $hiddenCandidateClose = Invoke-InFlightCloseQualification `
            -Name "hidden-candidate-close" `
            -Arguments ($commonArguments + @("--hold-post-upload-candidate")) `
            -StatePattern "post-upload-held revision 2" `
            -RequiredOutput "Closing with post-upload hidden raster candidate: revision=2"
        $shutdownQualification = [ordered]@{
            ActiveCpuWork = $activeCpuClose
            HiddenPostUploadCandidate = $hiddenCandidateClose
        }
    }

    $repositoryRevision = & git rev-parse HEAD
    if ($LASTEXITCODE -ne 0) {
        throw "git rev-parse HEAD failed with exit code $LASTEXITCODE."
    }
    $manifest = [ordered]@{
        SchemaVersion = 1
        Scope = "Descriptive uninterrupted edit-burst evidence for this recorded Windows development machine only."
        RecordedAtUtc = [DateTime]::UtcNow.ToString("o")
        RepositoryRevision = ($repositoryRevision -join "`n").Trim()
        BuildCommand = "cargo build --locked --package desktop-demo"
        RunArguments = @("--scene-scale", "256", "--camera-pose", "overview", "--raster-region-extent", "$RasterRegionExtent", "--edit-burst-demo")
        TimingOnly = [bool]$TimingOnly
        Input = [ordered]@{ Key = "Space"; KeyDownMessage = "WM_KEYDOWN"; KeyUpMessage = "WM_KEYUP"; CommandPublicationOwner = "single Space keypress" }
        ProcessExitCode = $process.ExitCode
        Validation = [ordered]@{ Enabled = $true; Warnings = $validationWarnings; Errors = $validationErrors; Log = "desktop-demo.stderr.log" }
        CanonicalInput = [ordered]@{
            Scene = "canonical-dense-scene"
            Generator = "voxel-nexus-canonical-dense"
            GeneratorVersion = 1
            Scale = 256
            Dimensions = @(256, 128, 256)
            InitialRevision = 1
            RasterRegionExtent = @($RasterRegionExtent, $RasterRegionExtent, $RasterRegionExtent)
            Camera = "overview"
            InstalledRevision = 1
            InstalledComplete = $true
            ExpectedFinalRevision = 4
        }
        Commands = @(
            [ordered]@{ Order = 1; Coordinate = @(0, 0, 0); Old = "empty"; Requested = "occupied:canonical-warm"; PublishedRevision = 2 },
            [ordered]@{ Order = 2; Coordinate = @(40, 0, 0); Old = "empty"; Requested = "occupied:canonical-warm"; PublishedRevision = 3 },
            [ordered]@{ Order = 3; Coordinate = @(80, 0, 0); Old = "empty"; Requested = "occupied:canonical-warm"; PublishedRevision = 4 }
        )
        CpuBarrier = [ordered]@{ ObsoleteRevision = 2; ScheduledBeforeHold = 1; ScheduledTotal = 1; Cancelled = $true }
        PostUploadBarrier = [ordered]@{ SupersededRevision = 3; RejectedAtCommit = $true; RetiredGpuResourceCount = [int]$retirement.Groups["Count"].Value }
        Measurement = [ordered]@{ KeypressToFinalVisibleMilliseconds = $latencyMilliseconds; PeakLiveGpuBytes = $peakLiveGpuBytes; PeakLiveGpuResources = $peakLiveGpuResources }
        Qualification = [ordered]@{ SemanticCorrectness = $true; Localization = $true; FailureRetry = $true; Lifecycle = (-not $TimingOnly); Shutdown = $true; ResourceRetirement = $true; Validation = $true }
        ShutdownQualification = $shutdownQualification
        Visibility = [ordered]@{ InitialTitle = $initialTitle; CpuHeldTitle = $cpuHeldTitle; PostUploadHeldTitle = $postUploadHeldTitle; FinalTitle = $finalTitle; IntermediateRevisionVisible = $false }
        Lifecycle = if ($TimingOnly) { @() } else { @("camera during CPU hold", "landscape and portrait resize during CPU hold", "minimize and restore during CPU hold", "camera during post-upload hold", "landscape and portrait resize during post-upload hold", "minimize and restore during post-upload hold", "camera during final post-upload commit hold", "normal close") }
        Captures = $captures
        RuntimeLog = "desktop-demo.stdout.log"
    }
    $manifest | ConvertTo-Json -Depth 8 | Set-Content -Encoding utf8 (Join-Path $evidencePath "manifest.json")
    Write-Host "Edit-burst proof passed. Evidence: $evidencePath"
}
finally {
    if ($null -ne $process -and -not $process.HasExited) {
        $process.Kill()
        $process.WaitForExit()
    }
    if ($null -ne $standardOutputTask -and -not (Test-Path (Join-Path $evidencePath "desktop-demo.stdout.log"))) {
        [System.IO.File]::WriteAllText(
            (Join-Path $evidencePath "desktop-demo.stdout.log"),
            $standardOutputTask.GetAwaiter().GetResult()
        )
    }
    if ($null -ne $standardErrorTask -and -not (Test-Path (Join-Path $evidencePath "desktop-demo.stderr.log"))) {
        [System.IO.File]::WriteAllText(
            (Join-Path $evidencePath "desktop-demo.stderr.log"),
            $standardErrorTask.GetAwaiter().GetResult()
        )
    }
    Pop-Location
}
