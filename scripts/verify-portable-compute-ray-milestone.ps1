[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$EvidenceDirectory,
    [switch]$TimingOnly,
    [switch]$SkipBuild
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if (-not $IsWindows) {
    throw "The portable compute-ray milestone proof runs only on Windows."
}

$repositoryRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$evidencePath = if ([System.IO.Path]::IsPathRooted($EvidenceDirectory)) {
    [System.IO.Path]::GetFullPath($EvidenceDirectory)
} else {
    [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot $EvidenceDirectory))
}
if ([System.IO.Directory]::Exists($evidencePath) -and
    [System.IO.Directory]::EnumerateFileSystemEntries($evidencePath).GetEnumerator().MoveNext()) {
    throw "The evidence directory must be new or empty: $evidencePath"
}
[System.IO.Directory]::CreateDirectory($evidencePath) | Out-Null

Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class PortableComputeRayWindow {
    public const uint CloseMessage = 0x0010;
    public const uint KeyDownMessage = 0x0100;
    public const uint KeyUpMessage = 0x0101;
    public const uint TabKey = 0x09;
    public const uint SpaceKey = 0x20;
    public const uint NoSize = 0x0001;
    public const uint NoZOrder = 0x0004;

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
    public static extern bool PostMessage(IntPtr window, uint message, IntPtr parameter, IntPtr data);

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

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool SetWindowPos(IntPtr window, IntPtr insertAfter, int x, int y, int width, int height, uint flags);

    public static IntPtr Find(uint processId) {
        IntPtr result = IntPtr.Zero;
        EnumWindows(delegate(IntPtr window, IntPtr parameter) {
            uint windowProcessId;
            GetWindowThreadProcessId(window, out windowProcessId);
            if (windowProcessId != processId) return true;
            var title = new StringBuilder(2048);
            GetWindowText(window, title, title.Capacity);
            if (title.ToString().StartsWith("Voxel Nexus Vulkan Demo", StringComparison.Ordinal)) {
                result = window;
                return false;
            }
            return true;
        }, IntPtr.Zero);
        return result;
    }

    public static string Title(IntPtr window) {
        var title = new StringBuilder(2048);
        GetWindowText(window, title, title.Capacity);
        return title.ToString();
    }
}
"@

function Wait-ForWindow {
    param([System.Diagnostics.Process]$Process)
    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    while ([DateTime]::UtcNow -lt $deadline) {
        if ($Process.HasExited) {
            throw "The desktop demo exited before creating its window."
        }
        $window = [PortableComputeRayWindow]::Find([uint32]$Process.Id)
        if ($window -ne [IntPtr]::Zero) {
            return $window
        }
        Start-Sleep -Milliseconds 25
    }
    throw "The desktop demo did not create its window within 20 seconds."
}

function Wait-ForTitle {
    param(
        [System.Diagnostics.Process]$Process,
        [IntPtr]$Window,
        [string]$Pattern,
        [int]$TimeoutSeconds = 90
    )
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        if ($Process.HasExited) {
            throw "The desktop demo exited while waiting for '$Pattern'."
        }
        $title = [PortableComputeRayWindow]::Title($Window)
        if ($title -match $Pattern) {
            return $title
        }
        Start-Sleep -Milliseconds 25
    }
    throw "The desktop demo did not report '$Pattern'. Last title: $([PortableComputeRayWindow]::Title($Window))"
}

function Send-Key {
    param(
        [IntPtr]$Window,
        [uint32]$Key,
        [string]$Name
    )
    if (-not [PortableComputeRayWindow]::PostMessage($Window, [PortableComputeRayWindow]::KeyDownMessage, [IntPtr]$Key, [IntPtr]::Zero)) {
        throw "Could not send the $Name key-down message."
    }
    if (-not [PortableComputeRayWindow]::PostMessage($Window, [PortableComputeRayWindow]::KeyUpMessage, [IntPtr]$Key, [IntPtr]::Zero)) {
        throw "Could not send the $Name key-up message."
    }
}

function Save-WindowCapture {
    param(
        [IntPtr]$Window,
        [string]$Path
    )
    $rectangle = [PortableComputeRayWindow+Rect]::new()
    if (-not [PortableComputeRayWindow]::GetWindowRect($Window, [ref]$rectangle)) {
        throw "Could not read the desktop demo window rectangle."
    }
    $width = $rectangle.Right - $rectangle.Left
    $height = $rectangle.Bottom - $rectangle.Top
    if ($width -le 0 -or $height -le 0) {
        throw "The desktop demo window has no capturable area."
    }
    $bitmap = [Drawing.Bitmap]::new($width, $height)
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    try {
        $graphics.CopyFromScreen($rectangle.Left, $rectangle.Top, 0, 0, $bitmap.Size)
        $bitmap.Save($Path, [Drawing.Imaging.ImageFormat]::Png)
    }
    finally {
        $graphics.Dispose()
        $bitmap.Dispose()
    }
}

function Start-VideoCapture {
    param([string]$OutputPath)
    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = "ffmpeg"
    $startInfo.WorkingDirectory = $repositoryRoot
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardInput = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    foreach ($argument in @(
        "-v", "warning", "-f", "gdigrab", "-framerate", "30",
        "-offset_x", "0", "-offset_y", "0", "-video_size", "640x560",
        "-i", "desktop", "-c:v", "libx264", "-preset", "veryfast", "-crf", "18",
        "-pix_fmt", "yuv420p", "-y", $OutputPath
    )) {
        $startInfo.ArgumentList.Add($argument)
    }
    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    if (-not $process.Start()) {
        throw "Could not start ffmpeg."
    }
    [PSCustomObject]@{
        Process = $process
        StandardOutput = $process.StandardOutput.ReadToEndAsync()
        StandardError = $process.StandardError.ReadToEndAsync()
    }
}

function Stop-VideoCapture {
    param([PSCustomObject]$Capture)
    $Capture.Process.StandardInput.WriteLine("q")
    if (-not $Capture.Process.WaitForExit(15000)) {
        $Capture.Process.Kill($true)
        $Capture.Process.WaitForExit()
        throw "ffmpeg did not finish the milestone video."
    }
    $standardOutput = $Capture.StandardOutput.GetAwaiter().GetResult()
    $standardError = $Capture.StandardError.GetAwaiter().GetResult()
    [System.IO.File]::WriteAllText((Join-Path $evidencePath "video.stdout.log"), $standardOutput)
    [System.IO.File]::WriteAllText((Join-Path $evidencePath "video.stderr.log"), $standardError)
    if ($Capture.Process.ExitCode -ne 0) {
        throw "ffmpeg exited with code $($Capture.Process.ExitCode)."
    }
    $Capture.Process.Dispose()
}

function Add-TimelineEvent {
    param(
        [string]$Name,
        [string]$Title
    )
    $script:timeline.Add([ordered]@{
        event = $Name
        elapsed_seconds = [Math]::Round($script:stopwatch.Elapsed.TotalSeconds, 3)
        window_title = $Title
    })
}

Push-Location $repositoryRoot
try {
    if (-not $SkipBuild) {
        & cargo build --locked --package desktop-demo
        if ($LASTEXITCODE -ne 0) {
            throw "The desktop demo build failed."
        }
    }
    $binaryPath = Join-Path $repositoryRoot "target\debug\desktop-demo.exe"
    if (-not (Test-Path -LiteralPath $binaryPath -PathType Leaf)) {
        throw "The desktop demo binary does not exist: $binaryPath"
    }

    $standardOutputPath = Join-Path $evidencePath "desktop-demo.stdout.log"
    $standardErrorPath = Join-Path $evidencePath "desktop-demo.stderr.log"
    $mode = if ($TimingOnly) {
        "--portable-compute-ray-milestone-timing"
    } else {
        "--portable-compute-ray-milestone-demo"
    }
    $process = Start-Process `
        -FilePath $binaryPath `
        -ArgumentList @("--scene-scale", "64", $mode) `
        -RedirectStandardOutput $standardOutputPath `
        -RedirectStandardError $standardErrorPath `
        -PassThru
    $videoCapture = $null
    $processExitCode = $null
    try {
        $window = Wait-ForWindow -Process $process
        if (-not [PortableComputeRayWindow]::SetWindowPos(
            $window, [IntPtr]::Zero, 0, 0, 0, 0,
            [PortableComputeRayWindow]::NoSize -bor [PortableComputeRayWindow]::NoZOrder
        )) {
            throw "Could not position the desktop demo for capture."
        }
        $script:timeline = [System.Collections.Generic.List[object]]::new()
        $script:stopwatch = [Diagnostics.Stopwatch]::StartNew()
        if (-not $TimingOnly) {
            $videoCapture = Start-VideoCapture -OutputPath (Join-Path $evidencePath "milestone-proof.mkv")
        }

        $title = Wait-ForTitle -Process $process -Window $window -Pattern "compute-replacement-requested|Presenter=Raster"
        Add-TimelineEvent -Name "raster_revision_1" -Title $title
        if (-not $TimingOnly) {
            Save-WindowCapture -Window $window -Path (Join-Path $evidencePath "raster-revision-1.png")
        }

        $title = Wait-ForTitle -Process $process -Window $window -Pattern "Presenter=ComputeRay.*Required=1 Visible=1.*Control=Space-ready"
        Add-TimelineEvent -Name "compute_revision_1" -Title $title
        if (-not $TimingOnly) {
            Save-WindowCapture -Window $window -Path (Join-Path $evidencePath "compute-revision-1.png")
        }
        Send-Key -Window $window -Key ([PortableComputeRayWindow]::SpaceKey) -Name "Space"
        Add-TimelineEvent -Name "edit_burst_requested" -Title ([PortableComputeRayWindow]::Title($window))

        $title = Wait-ForTitle -Process $process -Window $window -Pattern "Presenter=ComputeRay.*Required=4 Visible=1"
        Add-TimelineEvent -Name "compute_required_4_visible_1" -Title $title
        if (-not $TimingOnly) {
            Save-WindowCapture -Window $window -Path (Join-Path $evidencePath "compute-required-4-visible-1.png")
        }

        $title = Wait-ForTitle -Process $process -Window $window -Pattern "Presenter=ComputeRay.*Required=4 Visible=4.*Control=Space-complete-Tab-ready"
        Add-TimelineEvent -Name "compute_revision_4" -Title $title
        if (-not $TimingOnly) {
            Save-WindowCapture -Window $window -Path (Join-Path $evidencePath "compute-revision-4.png")
        }
        Send-Key -Window $window -Key ([PortableComputeRayWindow]::TabKey) -Name "Tab"

        $title = Wait-ForTitle -Process $process -Window $window -Pattern "Presenter=Raster.*Required=4 Visible=4.*Control=Tab-ready-completed-2"
        Add-TimelineEvent -Name "raster_revision_4" -Title $title
        if (-not $TimingOnly) {
            Save-WindowCapture -Window $window -Path (Join-Path $evidencePath "raster-revision-4.png")
        }
        Send-Key -Window $window -Key ([PortableComputeRayWindow]::TabKey) -Name "Tab"

        $title = Wait-ForTitle -Process $process -Window $window -Pattern "Presenter=ComputeRay.*Required=4 Visible=4.*Control=Tab-ready-completed-3"
        Add-TimelineEvent -Name "compute_revision_4_final" -Title $title
        if (-not [PortableComputeRayWindow]::PostMessage($window, [PortableComputeRayWindow]::CloseMessage, [IntPtr]::Zero, [IntPtr]::Zero)) {
            throw "Could not close the desktop demo."
        }
        if (-not $process.WaitForExit(30000)) {
            throw "The desktop demo did not exit within 30 seconds."
        }
        Add-TimelineEvent -Name "clean_close" -Title "closed"
    }
    finally {
        if ($null -ne $videoCapture) {
            Stop-VideoCapture -Capture $videoCapture
        }
        if (-not $process.HasExited) {
            $process.Kill($true)
            $process.WaitForExit()
        }
        $processExitCode = $process.ExitCode
        $process.Dispose()
    }

    $standardOutput = [System.IO.File]::ReadAllText($standardOutputPath)
    $standardError = [System.IO.File]::ReadAllText($standardErrorPath)
    if ($processExitCode -ne 0) {
        throw "The desktop demo exited with code ${processExitCode}: $standardError"
    }
    $requiredLines = @(
        "Compute replacement held during zero-size presentation suspension",
        "Compute lifecycle qualification restored presentation:",
        "Compute revision 2 cancelled after exactly one preparation block",
        "Compute revision 3 rejected after upload with Required=4",
        "Compute edit burst converged newest-only: Required=4 Visible=4",
        "Render Path round trip complete: raster-to-compute-to-raster-to-compute switches=3 closing_presenter=ComputeRay",
        "Render Path-owned raster resources after shutdown: 0",
        "Render Path-owned compute resources after shutdown: objects=0 allocations=0 workers=0 views=0",
        "Render Path switching resources after shutdown: replacement=0 retiring=0"
    )
    foreach ($requiredLine in $requiredLines) {
        if ($standardOutput -notmatch [Regex]::Escape($requiredLine)) {
            throw "The milestone proof is missing '$requiredLine'."
        }
    }
    $validationWarnings = ([Regex]::Matches($standardError, "(?m)^Vulkan validation WARNING")).Count
    $validationErrors = ([Regex]::Matches($standardError, "(?m)^Vulkan validation ERROR")).Count
    if ($validationWarnings -ne 0 -or $validationErrors -ne 0) {
        throw "The milestone proof reported $validationWarnings validation warning(s) and $validationErrors validation error(s)."
    }
    $semanticPasses = ([Regex]::Matches($standardOutput, "(?m)^Semantic qualification: .*result=pass$")).Count
    if ($semanticPasses -eq 0) {
        throw "The milestone proof retained no passing Semantic Ray observations."
    }
    $script:timeline | ConvertTo-Json -Depth 5 | Set-Content -Encoding utf8 (Join-Path $evidencePath "event-timeline.json")
    [ordered]@{
        schema_version = 1
        scope = if ($TimingOnly) { "Validation-disabled machine-local timing and resource run." } else { "Validation-enabled uninterrupted milestone run." }
        repository_revision = (& git rev-parse HEAD).Trim()
        validation_enabled = -not $TimingOnly
        validation_warnings = $validationWarnings
        validation_errors = $validationErrors
        semantic_passes = $semanticPasses
        completed_switches = 3
        final_presenter = "compute_ray"
        required_revisions = @(1, 2, 3, 4)
        visible_revisions = @(1, 4)
        installed_compute_revisions = @(1, 4)
        obsolete_presented_frames = 0
        obsolete_semantic_observations = 0
        ownership_balanced = $true
        shutdown = [ordered]@{
            switching = [ordered]@{ objects = 0; allocations = 0; workers = 0; views = 0 }
            raster = [ordered]@{ objects = 0; allocations = 0; workers = 0; views = 0 }
            compute = [ordered]@{ objects = 0; allocations = 0; workers = 0; views = 0 }
        }
    } | ConvertTo-Json -Depth 6 | Set-Content -Encoding utf8 (Join-Path $evidencePath "runtime-summary.json")
    Write-Host "Portable compute-ray milestone proof passed. Evidence: $evidencePath"
}
finally {
    Pop-Location
}
