[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$EvidenceDirectory,
    [ValidateSet("overview", "cavity", "boundary")]
    [string]$CameraPose = "overview",
    [switch]$CaptureCanonicalInspectionSet,
    [string]$VideoFile = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if (-not $IsWindows -and $PSVersionTable.PSEdition -eq "Core") {
    throw "The lifecycle proof runs only on Windows."
}

$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$evidencePath = [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot $EvidenceDirectory))
$currentCameraSelection = $CameraPose
if ([System.IO.Directory]::Exists($evidencePath) -and [System.IO.Directory]::EnumerateFileSystemEntries($evidencePath).GetEnumerator().MoveNext()) {
    throw "The evidence directory must be new or empty: $evidencePath"
}
[System.IO.Directory]::CreateDirectory($evidencePath) | Out-Null

Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
Add-Type @"
using System;
using System.Runtime.InteropServices;

public static class LifecycleWindow {
    public const int ShowMinimized = 6;
    public const int ShowRestored = 9;
    public const uint CloseMessage = 0x0010;
    public const uint KeepPositionAndSize = 0x0013;
    public const uint ReleasePreparationMessage = 0x801B;
    public const uint OverviewCameraMessage = 0x801C;
    public const uint CavityCameraMessage = 0x801D;
    public const uint BoundaryCameraMessage = 0x801E;
    public const uint StartCameraMoveMessage = 0x801F;
    public static readonly IntPtr Topmost = new IntPtr(-1);

    public delegate bool EnumWindowsCallback(IntPtr window, IntPtr parameter);

    [StructLayout(LayoutKind.Sequential)]
    public struct Rect {
        public int Left;
        public int Top;
        public int Right;
        public int Bottom;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct Point {
        public int X;
        public int Y;
    }

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool MoveWindow(IntPtr window, int x, int y, int width, int height, bool repaint);

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool SetWindowPos(IntPtr window, IntPtr insertAfter, int x, int y, int width, int height, uint flags);

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
    public static extern int GetWindowText(IntPtr window, System.Text.StringBuilder text, int capacity);

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool GetClientRect(IntPtr window, out Rect rectangle);

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool ClientToScreen(IntPtr window, ref Point point);

    [DllImport("dwmapi.dll")]
    public static extern int DwmFlush();

    public static IntPtr FindDemoWindow(uint processId) {
        IntPtr result = IntPtr.Zero;
        EnumWindows(delegate(IntPtr window, IntPtr parameter) {
            uint windowProcessId;
            GetWindowThreadProcessId(window, out windowProcessId);
            if (windowProcessId != processId) {
                return true;
            }
            var title = new System.Text.StringBuilder(512);
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

function Invoke-Cargo {
    param([string[]]$Arguments)

    & cargo @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "cargo $($Arguments -join ' ') failed with exit code $LASTEXITCODE"
    }
}

function Stop-LifecycleProcess {
    param([System.Diagnostics.Process]$Process)

    if (-not $Process.HasExited) {
        $Process.Kill()
        $Process.WaitForExit()
    }
}

function Start-CompletionVideoRecording {
    param([string]$FileName)

    $ffmpeg = Get-Command ffmpeg -ErrorAction SilentlyContinue
    if ($null -eq $ffmpeg) {
        throw "ffmpeg is required to record the uninterrupted completion video."
    }
    $videoPath = Join-Path $evidencePath $FileName
    $backdrop = [System.Windows.Forms.Form]::new()
    $backdrop.StartPosition = [System.Windows.Forms.FormStartPosition]::Manual
    $backdrop.Location = [System.Drawing.Point]::new(80, 80)
    $backdrop.ClientSize = [System.Drawing.Size]::new(1140, 940)
    $backdrop.BackColor = [System.Drawing.Color]::Black
    $backdrop.FormBorderStyle = [System.Windows.Forms.FormBorderStyle]::None
    $backdrop.ShowInTaskbar = $false
    $backdrop.TopMost = $true
    $backdrop.Show()
    [System.Windows.Forms.Application]::DoEvents()

    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $ffmpeg.Source
    $startInfo.WorkingDirectory = $repositoryRoot
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardInput = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    foreach ($argument in @(
        "-hide_banner", "-y",
        "-f", "gdigrab",
        "-framerate", "30",
        "-draw_mouse", "0",
        "-offset_x", "80",
        "-offset_y", "80",
        "-video_size", "1140x940",
        "-i", "desktop",
        "-c:v", "libx264",
        "-preset", "veryfast",
        "-crf", "18",
        "-pix_fmt", "yuv420p",
        $videoPath
    )) {
        $startInfo.ArgumentList.Add($argument)
    }
    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    if (-not $process.Start()) {
        $backdrop.Dispose()
        throw "Could not start ffmpeg completion video recording."
    }
    $standardOutputTask = $process.StandardOutput.ReadToEndAsync()
    $standardErrorTask = $process.StandardError.ReadToEndAsync()
    Start-Sleep -Seconds 1
    if ($process.HasExited) {
        $standardError = $standardErrorTask.GetAwaiter().GetResult()
        $backdrop.Dispose()
        throw "ffmpeg completion video recording exited before the proof began: $standardError"
    }
    [PSCustomObject]@{
        Process = $process
        StandardOutputTask = $standardOutputTask
        StandardErrorTask = $standardErrorTask
        Backdrop = $backdrop
        File = $FileName
        Path = $videoPath
        CaptureScope = "1140x940 controlled black-backed desktop region at screen coordinates 80,80"
    }
}

function Stop-CompletionVideoRecording {
    param([PSCustomObject]$Recording)

    try {
        if (-not $Recording.Process.HasExited) {
            $Recording.Process.StandardInput.WriteLine("q")
            $Recording.Process.StandardInput.Close()
            if (-not $Recording.Process.WaitForExit(15000)) {
                $Recording.Process.Kill()
                $Recording.Process.WaitForExit()
                throw "ffmpeg did not finish the completion video within 15 seconds."
            }
        }
        $standardOutput = $Recording.StandardOutputTask.GetAwaiter().GetResult()
        $standardError = $Recording.StandardErrorTask.GetAwaiter().GetResult()
        [System.IO.File]::WriteAllText((Join-Path $evidencePath "completion-video.stdout.log"), $standardOutput)
        [System.IO.File]::WriteAllText((Join-Path $evidencePath "completion-video.stderr.log"), $standardError)
        if ($Recording.Process.ExitCode -ne 0) {
            throw "ffmpeg completion video recording failed with code $($Recording.Process.ExitCode)."
        }
        $video = Get-Item -LiteralPath $Recording.Path
        if ($video.Length -le 0) {
            throw "ffmpeg produced an empty completion video."
        }
    }
    finally {
        $Recording.Backdrop.Dispose()
    }
}

function Add-CompletionVideoEvent {
    param([string]$Event)

    if ($null -ne $script:completionVideoStopwatch) {
        $script:completionVideoEvents.Add([ordered]@{
            Event = $Event
            ElapsedSeconds = [Math]::Round($script:completionVideoStopwatch.Elapsed.TotalSeconds, 3)
            RecordedAtUtc = [DateTime]::UtcNow.ToString("o")
        })
    }
}

function Invoke-CapturedProcess {
    param(
        [string]$Executable,
        [string[]]$Arguments,
        [string]$StandardOutputPath,
        [string]$StandardErrorPath,
        [int]$TimeoutMilliseconds = 10000
    )

    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $Executable
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
        throw "Could not start $Executable"
    }
    if (-not $process.WaitForExit($TimeoutMilliseconds)) {
        Stop-LifecycleProcess -Process $process
        throw "$Executable exceeded the $TimeoutMilliseconds ms timeout"
    }
    $standardOutput = $process.StandardOutput.ReadToEnd()
    $standardError = $process.StandardError.ReadToEnd()
    [System.IO.File]::WriteAllText($StandardOutputPath, $standardOutput)
    [System.IO.File]::WriteAllText($StandardErrorPath, $standardError)
    [PSCustomObject]@{
        ExitCode = $process.ExitCode
        StandardOutput = $standardOutput
        StandardError = $standardError
    }
}

function Wait-ForWindow {
    param([System.Diagnostics.Process]$Process)

    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    while ([DateTime]::UtcNow -lt $deadline) {
        if ($Process.HasExited) {
            throw "The desktop demo exited before creating its window with code $($Process.ExitCode)."
        }
        $window = [LifecycleWindow]::FindDemoWindow($Process.Id)
        if ($window -ne [IntPtr]::Zero) {
            return $window
        }
        Start-Sleep -Milliseconds 100
    }
    throw "The desktop demo did not create a window within 15 seconds."
}

function Get-DemoWindowTitle {
    param([IntPtr]$Window)

    $title = [System.Text.StringBuilder]::new(512)
    [LifecycleWindow]::GetWindowText($Window, $title, $title.Capacity) | Out-Null
    $title.ToString()
}

function Wait-ForWindowTitle {
    param(
        [System.Diagnostics.Process]$Process,
        [IntPtr]$Window,
        [string]$Pattern,
        [string]$PreviousTitle = ""
    )

    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    while ([DateTime]::UtcNow -lt $deadline) {
        if ($Process.HasExited) {
            throw "The desktop demo exited while waiting for window state '$Pattern' with code $($Process.ExitCode)."
        }
        $title = Get-DemoWindowTitle -Window $Window
        if ($title -match $Pattern -and $title -ne $PreviousTitle) {
            return $title
        }
        Start-Sleep -Milliseconds 50
    }
    throw "The desktop demo did not report window state '$Pattern' within 30 seconds. Last title: $(Get-DemoWindowTitle -Window $Window)"
}

function Send-DesktopVerificationEvent {
    param(
        [IntPtr]$Window,
        [uint32]$Message,
        [string]$Name
    )

    if (-not [LifecycleWindow]::PostMessage($Window, $Message, [IntPtr]::Zero, [IntPtr]::Zero)) {
        throw "Could not send the desktop verification event '$Name'."
    }
}

function Get-ClientArea {
    param([IntPtr]$Window)

    $rectangle = [LifecycleWindow+Rect]::new()
    if (-not [LifecycleWindow]::GetClientRect($Window, [ref]$rectangle)) {
        throw "Could not query the desktop demo client rectangle."
    }
    $origin = [LifecycleWindow+Point]::new()
    if (-not [LifecycleWindow]::ClientToScreen($Window, [ref]$origin)) {
        throw "Could not map the desktop demo client rectangle to the screen."
    }
    [PSCustomObject]@{
        X = $origin.X
        Y = $origin.Y
        Width = $rectangle.Right - $rectangle.Left
        Height = $rectangle.Bottom - $rectangle.Top
    }
}

function Save-ClientCapture {
    param(
        [IntPtr]$Window,
        [string]$Name
    )

    if (-not [LifecycleWindow]::SetWindowPos($Window, [LifecycleWindow]::Topmost, 0, 0, 0, 0, [LifecycleWindow]::KeepPositionAndSize)) {
        throw "Could not keep the desktop demo visible for capture."
    }
    Start-Sleep -Milliseconds 100
    if ([LifecycleWindow]::DwmFlush() -ne 0) {
        throw "Could not synchronize the desktop compositor before capture."
    }
    $area = Get-ClientArea -Window $Window
    if ($area.Width -le 0 -or $area.Height -le 0) {
        throw "Cannot capture a non-positive client area for $Name."
    }
    $path = Join-Path $evidencePath "$Name.png"
    $bitmap = [System.Drawing.Bitmap]::new($area.Width, $area.Height)
    try {
        $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
        try {
            $graphics.CopyFromScreen($area.X, $area.Y, 0, 0, $bitmap.Size)
        }
        finally {
            $graphics.Dispose()
        }
        $warmMaterialPixels = 0L
        $greenMaterialPixels = 0L
        $blueMaterialPixels = 0L
        $backgroundPixels = 0L
        $sampledPixels = 0L
        for ($y = 0; $y -lt $bitmap.Height; $y += 2) {
            for ($x = 0; $x -lt $bitmap.Width; $x += 2) {
                $sampledPixels++
                $pixel = $bitmap.GetPixel($x, $y)
                if ($pixel.R -gt 120 -and ($pixel.R - $pixel.G) -gt 40 -and ($pixel.R - $pixel.B) -gt 50) {
                    $warmMaterialPixels++
                }
                if ($pixel.G -gt 80 -and ($pixel.G - $pixel.R) -gt 20 -and ($pixel.G - $pixel.B) -gt 10) {
                    $greenMaterialPixels++
                }
                if ($pixel.B -gt 90 -and ($pixel.B - $pixel.R) -gt 30 -and ($pixel.B - $pixel.G) -gt 10) {
                    $blueMaterialPixels++
                }
                if ($pixel.R -lt 100 -and $pixel.G -lt 100 -and $pixel.B -lt 130) {
                    $backgroundPixels++
                }
            }
        }
        $bitmap.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
        $warmMaterialFraction = $warmMaterialPixels / $sampledPixels
        $greenMaterialFraction = $greenMaterialPixels / $sampledPixels
        $blueMaterialFraction = $blueMaterialPixels / $sampledPixels
        $backgroundFraction = $backgroundPixels / $sampledPixels
        $materialFraction = $warmMaterialFraction + $greenMaterialFraction + $blueMaterialFraction
        $requiresAllMaterials = $script:currentCameraSelection -notin @("boundary", "winding-diagnostic")
        $missingRequiredMaterial = $requiresAllMaterials -and ($warmMaterialFraction -lt 0.005 -or $greenMaterialFraction -lt 0.005 -or $blueMaterialFraction -lt 0.005)
        if ($missingRequiredMaterial -or $materialFraction -lt 0.03 -or $materialFraction -gt 0.98 -or $backgroundFraction -lt 0.02 -or $backgroundFraction -gt 0.97) {
            throw "Capture $Name does not contain the expected warm, green, and blue voxel materials with clear background (warm: $warmMaterialFraction; green: $greenMaterialFraction; blue: $blueMaterialFraction; background: $backgroundFraction)."
        }
    }
    finally {
        $bitmap.Dispose()
    }
    [PSCustomObject]@{
        Name = $Name
        File = [System.IO.Path]::GetFileName($path)
        Width = $area.Width
        Height = $area.Height
        WarmMaterialSamplePixels = $warmMaterialPixels
        GreenMaterialSamplePixels = $greenMaterialPixels
        BlueMaterialSamplePixels = $blueMaterialPixels
        BackgroundSamplePixels = $backgroundPixels
        SampledPixels = $sampledPixels
        WarmMaterialFraction = [Math]::Round($warmMaterialFraction, 4)
        GreenMaterialFraction = [Math]::Round($greenMaterialFraction, 4)
        BlueMaterialFraction = [Math]::Round($blueMaterialFraction, 4)
        BackgroundFraction = [Math]::Round($backgroundFraction, 4)
        Sha256 = (Get-FileHash -Algorithm SHA256 $path).Hash.ToLowerInvariant()
    }
}

function Capture-StablePresentation {
    param(
        [IntPtr]$Window,
        [string]$Name
    )

    Start-Sleep -Milliseconds 800
    $lastFailure = ""
    $first = $null
    for ($attempt = 1; $attempt -le 12; $attempt++) {
        $captureName = if ($null -eq $first) { "$Name-a" } else { "$Name-b" }
        try {
            $capture = Save-ClientCapture -Window $Window -Name $captureName
            if ($null -eq $first) {
                $first = $capture
            }
            else {
                $firstPath = Join-Path $evidencePath $first.File
                $capturePath = Join-Path $evidencePath $capture.File
                $firstBitmap = [System.Drawing.Bitmap]::new($firstPath)
                $captureBitmap = [System.Drawing.Bitmap]::new($capturePath)
                try {
                    if ($firstBitmap.Size -ne $captureBitmap.Size) {
                        $lastFailure = "valid capture dimensions differed"
                        continue
                    }
                    $sampledPixels = 0L
                    $materiallyDifferentPixels = 0L
                    for ($y = 0; $y -lt $firstBitmap.Height; $y += 2) {
                        for ($x = 0; $x -lt $firstBitmap.Width; $x += 2) {
                            $sampledPixels++
                            $firstPixel = $firstBitmap.GetPixel($x, $y)
                            $capturePixel = $captureBitmap.GetPixel($x, $y)
                            $difference = [Math]::Abs($firstPixel.R - $capturePixel.R) +
                                [Math]::Abs($firstPixel.G - $capturePixel.G) +
                                [Math]::Abs($firstPixel.B - $capturePixel.B)
                            if ($difference -gt 30) {
                                $materiallyDifferentPixels++
                            }
                        }
                    }
                    $differenceFraction = $materiallyDifferentPixels / $sampledPixels
                }
                finally {
                    $firstBitmap.Dispose()
                    $captureBitmap.Dispose()
                }
                if ($differenceFraction -le 0.001) {
                    return [PSCustomObject]@{
                        State = $Name
                        Width = $first.Width
                        Height = $first.Height
                        AspectRatio = [Math]::Round($first.Width / $first.Height, 3)
                        WarmMaterialSamplePixels = $first.WarmMaterialSamplePixels
                        GreenMaterialSamplePixels = $first.GreenMaterialSamplePixels
                        BlueMaterialSamplePixels = $first.BlueMaterialSamplePixels
                        BackgroundSamplePixels = $first.BackgroundSamplePixels
                        MaterialDifferenceFraction = [Math]::Round($differenceFraction, 6)
                        CaptureSha256 = @($first.Sha256, $capture.Sha256)
                        Captures = @($first.File, $capture.File)
                    }
                }
                $lastFailure = "valid captures differed materially in $differenceFraction of sampled pixels"
            }
        }
        catch {
            $lastFailure = $_.Exception.Message
        }
        Start-Sleep -Milliseconds 300
    }
    throw "Could not obtain a stable paired $Name capture after twelve attempts: $lastFailure"
}

function Read-CanonicalSceneRecord {
    param([string]$StandardOutput)

    $match = [Regex]::Match(
        $StandardOutput,
        "(?m)^Canonical scene: generator=(?<Generator>\S+) version=(?<Version>\d+) seed=(?<Seed>\d+) dimensions=(?<Width>\d+)x(?<Height>\d+)x(?<Depth>\d+) origin=(?<Origin>\S+) voxel_size=(?<VoxelSize>\S+) materials=(?<Materials>\S+) material_colors=(?<Colors>\S+) occupied=(?<Occupied>\d+) exposed_faces=(?<Exposed>\d+) exposed_face_limit=(?<Limit>\d+)$"
    )
    if (-not $match.Success) {
        throw "The desktop demo did not report complete canonical scene metadata."
    }
    [PSCustomObject]@{
        GeneratorIdentity = $match.Groups["Generator"].Value
        GeneratorVersion = [uint32]$match.Groups["Version"].Value
        Seed = [uint64]$match.Groups["Seed"].Value
        Dimensions = @(
            [uint32]$match.Groups["Width"].Value,
            [uint32]$match.Groups["Height"].Value,
            [uint32]$match.Groups["Depth"].Value
        )
        Origin = @($match.Groups["Origin"].Value.Split(",") | ForEach-Object { [double]$_ })
        VoxelSize = [double]$match.Groups["VoxelSize"].Value
        MaterialCatalogue = @($match.Groups["Materials"].Value.Split(","))
        MaterialLinearBaseColors = @(
            $match.Groups["Colors"].Value.Split(";") |
                ForEach-Object { ,@($_.Split(",") | ForEach-Object { [double]$_ }) }
        )
        OccupiedCount = [uint64]$match.Groups["Occupied"].Value
        ExposedFaceCount = [uint64]$match.Groups["Exposed"].Value
        ExposedFaceLimit = [uint64]$match.Groups["Limit"].Value
    }
}

function Read-CanonicalCameraRecord {
    param([string]$StandardOutput)

    $match = [Regex]::Match(
        $StandardOutput,
        "(?m)^Canonical camera: camera=(?<Camera>\S+) eye=(?<Eye>\S+) target=(?<Target>\S+) up=(?<Up>\S+) fov_degrees=(?<Fov>\S+) near=(?<Near>\S+) far=(?<Far>\S+)$"
    )
    if (-not $match.Success) {
        throw "The desktop demo did not report complete canonical camera metadata."
    }
    [PSCustomObject]@{
        Selection = $match.Groups["Camera"].Value
        Eye = @($match.Groups["Eye"].Value.Split(",") | ForEach-Object { [double]$_ })
        Target = @($match.Groups["Target"].Value.Split(",") | ForEach-Object { [double]$_ })
        Up = @($match.Groups["Up"].Value.Split(",") | ForEach-Object { [double]$_ })
        FieldOfViewDegrees = [double]$match.Groups["Fov"].Value
        NearPlane = [double]$match.Groups["Near"].Value
        FarPlane = [double]$match.Groups["Far"].Value
    }
}

function Start-DesktopDemoProcess {
    param(
        [string]$BinaryPath,
        [string[]]$Arguments
    )

    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $BinaryPath
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
        throw "Could not start the desktop demo."
    }
    $process
}

function Complete-DesktopDemoProcess {
    param(
        [System.Diagnostics.Process]$Process,
        [IntPtr]$Window,
        [string]$Name,
        [string]$StandardOutputFile,
        [string]$StandardErrorFile
    )

    if (-not [LifecycleWindow]::PostMessage($Window, [LifecycleWindow]::CloseMessage, [IntPtr]::Zero, [IntPtr]::Zero)) {
        throw "Could not request a normal $Name close."
    }
    if (-not $Process.WaitForExit(10000)) {
        Stop-LifecycleProcess -Process $Process
        throw "The $Name process did not exit within 10 seconds of a normal close."
    }
    $standardOutput = $Process.StandardOutput.ReadToEnd()
    $standardError = $Process.StandardError.ReadToEnd()
    [System.IO.File]::WriteAllText((Join-Path $evidencePath $StandardOutputFile), $standardOutput)
    [System.IO.File]::WriteAllText((Join-Path $evidencePath $StandardErrorFile), $standardError)
    if ($Process.ExitCode -ne 0) {
        throw "The $Name process exited with code $($Process.ExitCode)."
    }
    $validationWarnings = ([Regex]::Matches($standardError, "(?m)^Vulkan validation WARNING")).Count
    $validationErrors = ([Regex]::Matches($standardError, "(?m)^Vulkan validation ERROR")).Count
    if ($validationWarnings -ne 0 -or $validationErrors -ne 0) {
        throw "The $Name process produced $validationWarnings validation warning(s) and $validationErrors validation error(s)."
    }
    [PSCustomObject]@{
        ExitCode = $Process.ExitCode
        StandardOutput = $standardOutput
        StandardError = $standardError
        ValidationWarnings = $validationWarnings
        ValidationErrors = $validationErrors
    }
}

function Capture-AdditionalPresentation {
    param(
        [string]$BinaryPath,
        [string]$Name,
        [string[]]$Arguments,
        [string]$CameraSelection,
        [string]$Description
    )

    $previousCameraSelection = $script:currentCameraSelection
    $script:currentCameraSelection = $CameraSelection
    $captureProcess = $null
    try {
        $captureProcess = Start-DesktopDemoProcess -BinaryPath $BinaryPath -Arguments $Arguments
        $window = Wait-ForWindow -Process $captureProcess
        if (-not [LifecycleWindow]::MoveWindow($window, 100, 100, 900, 650, $true)) {
            throw "Could not set the $Description extent."
        }
        $capture = Capture-StablePresentation -Window $window -Name $Name
        $completion = Complete-DesktopDemoProcess `
            -Process $captureProcess `
            -Window $window `
            -Name $Description `
            -StandardOutputFile "$Name.stdout.log" `
            -StandardErrorFile "$Name.stderr.log"
        return [PSCustomObject]@{
            Capture = $capture
            Completion = $completion
        }
    }
    finally {
        $script:currentCameraSelection = $previousCameraSelection
        if ($null -ne $captureProcess -and -not $captureProcess.HasExited) {
            Stop-LifecycleProcess -Process $captureProcess
        }
    }
}

function Capture-AdditionalCanonicalPresentation {
    param(
        [string]$BinaryPath,
        [string]$Name,
        [string[]]$Arguments,
        [string]$CameraSelection
    )

    $result = Capture-AdditionalPresentation `
        -BinaryPath $BinaryPath `
        -Name $Name `
        -Arguments $Arguments `
        -CameraSelection $CameraSelection `
        -Description "$Name canonical capture"
    return [PSCustomObject]@{
        Name = $Name
        CameraSelection = $CameraSelection
        Capture = $result.Capture
        StandardOutput = "$Name.stdout.log"
        ValidationLog = "$Name.stderr.log"
        ValidationWarnings = $result.Completion.ValidationWarnings
        ValidationErrors = $result.Completion.ValidationErrors
        Scene = Read-CanonicalSceneRecord -StandardOutput $result.Completion.StandardOutput
        Camera = Read-CanonicalCameraRecord -StandardOutput $result.Completion.StandardOutput
    }
}

function Capture-WindingDiagnosticPresentation {
    param([string]$BinaryPath)

    $result = Capture-AdditionalPresentation `
        -BinaryPath $BinaryPath `
        -Name "winding-diagnostic" `
        -Arguments @("--winding-diagnostic") `
        -CameraSelection "winding-diagnostic" `
        -Description "winding diagnostic capture"
    foreach ($requiredLine in @(
        "Diagnostic scene: identity=raster-front-face-winding dimensions=1x1x2",
        "Diagnostic camera: camera=winding-diagnostic",
        "Vulkan validation: enabled",
        "First matching raster frame presented: revision=1"
    )) {
        if ($result.Completion.StandardOutput -notmatch [Regex]::Escape($requiredLine)) {
            throw "The winding diagnostic capture is missing '$requiredLine'."
        }
    }
    return [PSCustomObject]@{
        Name = "winding-diagnostic"
        Meaning = "A warm near voxel and blue far voxel viewed head-on through the positive-height Vulkan viewport; the warm positive-Z outward face must occlude the blue negative-Z outward face."
        Capture = $result.Capture
        StandardOutput = "winding-diagnostic.stdout.log"
        ValidationLog = "winding-diagnostic.stderr.log"
        ValidationWarnings = $result.Completion.ValidationWarnings
        ValidationErrors = $result.Completion.ValidationErrors
    }
}

$script:completionVideoStopwatch = $null
$script:completionVideoEvents = [System.Collections.Generic.List[object]]::new()
$completionVideoRecording = $null

Push-Location $repositoryRoot
try {
    Invoke-Cargo -Arguments @("build", "--locked", "--package", "desktop-demo")
    Invoke-Cargo -Arguments @("test", "--locked", "--package", "desktop-demo", "--test", "unsupported_prerequisites")
    Invoke-Cargo -Arguments @("test", "--locked", "--package", "desktop-demo", "--test", "render_path_failures")

    $binaryPath = Join-Path $repositoryRoot "target\debug\desktop-demo.exe"
    $unsupportedCases = @()
    foreach ($case in @("vulkan-1.2", "presentation")) {
        $caseResult = Invoke-CapturedProcess `
            -Executable $binaryPath `
            -Arguments @("--verify-unsupported-prerequisite", $case) `
            -StandardOutputPath (Join-Path $evidencePath "unsupported-$case.stdout.log") `
            -StandardErrorPath (Join-Path $evidencePath "unsupported-$case.stderr.log")
        if ($caseResult.ExitCode -ne 1 -or $caseResult.StandardError -notmatch "Voxel Nexus could not start") {
            throw "Unsupported-prerequisite case $case did not produce the expected clean failure."
        }
        if ($caseResult.StandardError -match "panicked") {
            throw "Unsupported-prerequisite case $case panicked."
        }
        $unsupportedCases += [PSCustomObject]@{
            Case = $case
            ExitCode = $caseResult.ExitCode
            StandardError = "unsupported-$case.stderr.log"
        }
    }

    $renderPathFailures = @()
    foreach ($phase in @("release", "configure", "record", "upload")) {
        $phaseResult = Invoke-CapturedProcess `
            -Executable $binaryPath `
            -Arguments @("--verify-render-path-failure", $phase) `
            -StandardOutputPath (Join-Path $evidencePath "render-path-$phase.stdout.log") `
            -StandardErrorPath (Join-Path $evidencePath "render-path-$phase.stderr.log")
        $expectedFailure = if ($phase -eq "upload") {
            "raster artifact upload failed for Voxel Scene Revision 41: injected proof failure"
        }
        else {
            "Render Path $phase failed: injected proof failure"
        }
        if ($phaseResult.ExitCode -ne 1 -or $phaseResult.StandardError -notmatch [Regex]::Escape($expectedFailure)) {
            throw "Render Path phase $phase did not preserve its phase and source failure at the application boundary."
        }
        if ($phaseResult.StandardError -match "panicked") {
            throw "Render Path phase $phase failure panicked."
        }
        $renderPathFailures += [PSCustomObject]@{
            Phase = $phase
            ExitCode = $phaseResult.ExitCode
            StandardError = "render-path-$phase.stderr.log"
        }
    }

    $preparationFailure = Invoke-CapturedProcess `
        -Executable $binaryPath `
        -Arguments @("--verify-background-preparation-failure", "derivation") `
        -StandardOutputPath (Join-Path $evidencePath "background-derivation.stdout.log") `
        -StandardErrorPath (Join-Path $evidencePath "background-derivation.stderr.log")
    foreach ($requiredFailureContext in @(
        "Voxel Nexus could not start",
        "background derivation failed for Voxel Scene Revision VoxelSceneRevision(1)",
        "metadata",
        "injected-missing-volume"
    )) {
        if ($preparationFailure.StandardError -notmatch [Regex]::Escape($requiredFailureContext)) {
            throw "Background derivation failure did not preserve '$requiredFailureContext' at the application boundary."
        }
    }
    if ($preparationFailure.ExitCode -ne 1 -or $preparationFailure.StandardError -match "panicked") {
        throw "Background derivation failure did not terminate cleanly."
    }

    $uploadInstallationFailure = Invoke-CapturedProcess `
        -Executable $binaryPath `
        -Arguments @("--scene-scale", "64", "--inject-raster-upload-failure") `
        -StandardOutputPath (Join-Path $evidencePath "raster-install-upload.stdout.log") `
        -StandardErrorPath (Join-Path $evidencePath "raster-install-upload.stderr.log") `
        -TimeoutMilliseconds 30000
    foreach ($requiredFailureContext in @(
        "raster artifact upload failed for Voxel Scene Revision 1",
        "injected GPU upload failure",
        "installed raster artifact revision after failure: None"
    )) {
        if ($uploadInstallationFailure.StandardError -notmatch [Regex]::Escape($requiredFailureContext)) {
            throw "Raster upload/install failure did not preserve '$requiredFailureContext' at the application boundary."
        }
    }
    if ($uploadInstallationFailure.ExitCode -ne 1 -or $uploadInstallationFailure.StandardError -match "panicked") {
        throw "Raster upload/install failure did not terminate cleanly."
    }

    $demoProcess = Start-DesktopDemoProcess `
        -BinaryPath $binaryPath `
        -Arguments @("--scene-scale", "256", "--camera-pose", $CameraPose, "--hold-background-preparation")

    $window = Wait-ForWindow -Process $demoProcess
    $pausedTitle = Wait-ForWindowTitle `
        -Process $demoProcess `
        -Window $window `
        -Pattern "preparation-paused"
    if ($VideoFile) {
        if (-not [LifecycleWindow]::MoveWindow($window, 100, 100, 900, 650, $true)) {
            throw "Could not set the paused desktop demo recording extent."
        }
        $script:completionVideoStopwatch = [System.Diagnostics.Stopwatch]::StartNew()
        $completionVideoRecording = Start-CompletionVideoRecording -FileName $VideoFile
        if (-not [LifecycleWindow]::SetWindowPos($window, [LifecycleWindow]::Topmost, 0, 0, 0, 0, [LifecycleWindow]::KeepPositionAndSize)) {
            throw "Could not place the desktop demo above the controlled recording backdrop."
        }
        Add-CompletionVideoEvent -Event "worker_paused"
        Start-Sleep -Seconds 1
    }
    if (-not [LifecycleWindow]::MoveWindow($window, 100, 100, 1100, 700, $true)) {
        throw "Could not resize the paused desktop demo to the landscape extent."
    }
    $landscapePausedTitle = Wait-ForWindowTitle `
        -Process $demoProcess `
        -Window $window `
        -Pattern "preparation-paused lifecycle-responsive" `
        -PreviousTitle $pausedTitle
    Add-CompletionVideoEvent -Event "landscape_resize_while_paused"
    if ($VideoFile) { Start-Sleep -Seconds 1 }
    $pausedLandscapeArea = Get-ClientArea -Window $window
    if (-not [LifecycleWindow]::MoveWindow($window, 100, 100, 650, 900, $true)) {
        throw "Could not resize the paused desktop demo to the portrait extent."
    }
    $portraitPausedTitle = Wait-ForWindowTitle `
        -Process $demoProcess `
        -Window $window `
        -Pattern "preparation-paused lifecycle-responsive" `
        -PreviousTitle $landscapePausedTitle
    Add-CompletionVideoEvent -Event "portrait_resize_while_paused"
    if ($VideoFile) { Start-Sleep -Seconds 1 }
    $pausedPortraitArea = Get-ClientArea -Window $window
    if (($pausedLandscapeArea.Width / $pausedLandscapeArea.Height) -le 1 `
        -or ($pausedPortraitArea.Width / $pausedPortraitArea.Height) -ge 1) {
        throw "The paused background proof did not service both landscape and portrait extents."
    }
    [LifecycleWindow]::ShowWindowAsync($window, [LifecycleWindow]::ShowMinimized) | Out-Null
    $minimizeDeadline = [DateTime]::UtcNow.AddSeconds(10)
    while (-not [LifecycleWindow]::IsIconic($window) -and [DateTime]::UtcNow -lt $minimizeDeadline) {
        Start-Sleep -Milliseconds 50
    }
    if (-not [LifecycleWindow]::IsIconic($window)) {
        throw "The paused desktop demo did not enter the minimized state."
    }
    Wait-ForWindowTitle -Process $demoProcess -Window $window -Pattern "preparation-paused suspended" | Out-Null
    Add-CompletionVideoEvent -Event "minimized_while_paused"
    if ($VideoFile) { Start-Sleep -Seconds 1 }
    [LifecycleWindow]::ShowWindowAsync($window, [LifecycleWindow]::ShowRestored) | Out-Null
    $restoreDeadline = [DateTime]::UtcNow.AddSeconds(10)
    while ([LifecycleWindow]::IsIconic($window) -and [DateTime]::UtcNow -lt $restoreDeadline) {
        Start-Sleep -Milliseconds 50
    }
    if ([LifecycleWindow]::IsIconic($window)) {
        throw "The paused desktop demo did not restore from the minimized state."
    }
    Wait-ForWindowTitle -Process $demoProcess -Window $window -Pattern "preparation-paused lifecycle-responsive" | Out-Null
    Add-CompletionVideoEvent -Event "restored_while_paused"
    if ($VideoFile) { Start-Sleep -Seconds 1 }
    Send-DesktopVerificationEvent `
        -Window $window `
        -Message ([LifecycleWindow]::ReleasePreparationMessage) `
        -Name "release background preparation"
    Wait-ForWindowTitle -Process $demoProcess -Window $window -Pattern "artifact-ready revision 1" | Out-Null

    if (-not [LifecycleWindow]::MoveWindow($window, 100, 100, 900, 650, $true)) {
        throw "Could not set the desktop demo launch extent."
    }
    $captures = @()
    $captures += Capture-StablePresentation -Window $window -Name "launch"
    Add-CompletionVideoEvent -Event "first_matching_revision_frame"

    if (-not [LifecycleWindow]::MoveWindow($window, 100, 100, 1100, 700, $true)) {
        throw "Could not resize the desktop demo to the landscape extent."
    }
    $captures += Capture-StablePresentation -Window $window -Name "landscape"

    if (-not [LifecycleWindow]::MoveWindow($window, 100, 100, 650, 900, $true)) {
        throw "Could not resize the desktop demo to the portrait extent."
    }
    $captures += Capture-StablePresentation -Window $window -Name "portrait"
    if ($captures[-2].AspectRatio -le 1 -or $captures[-1].AspectRatio -ge 1) {
        throw "The resize proof did not cover both landscape and portrait aspect ratios."
    }

    [LifecycleWindow]::ShowWindowAsync($window, [LifecycleWindow]::ShowMinimized) | Out-Null
    Start-Sleep -Seconds 1
    if (-not [LifecycleWindow]::IsIconic($window)) {
        throw "The desktop demo did not enter the minimized state."
    }
    [LifecycleWindow]::ShowWindowAsync($window, [LifecycleWindow]::ShowRestored) | Out-Null
    Start-Sleep -Seconds 1
    if ([LifecycleWindow]::IsIconic($window)) {
        throw "The desktop demo did not restore from the minimized state."
    }
    $captures += Capture-StablePresentation -Window $window -Name "restored"

    foreach ($cameraEvent in @(
        [PSCustomObject]@{ Message = [LifecycleWindow]::OverviewCameraMessage; Name = "overview" },
        [PSCustomObject]@{ Message = [LifecycleWindow]::CavityCameraMessage; Name = "cavity" },
        [PSCustomObject]@{ Message = [LifecycleWindow]::BoundaryCameraMessage; Name = "boundary" }
    )) {
        Send-DesktopVerificationEvent -Window $window -Message $cameraEvent.Message -Name $cameraEvent.Name
        Wait-ForWindowTitle `
            -Process $demoProcess `
            -Window $window `
            -Pattern "camera-presented $($cameraEvent.Name)" | Out-Null
        Add-CompletionVideoEvent -Event "fixed_pose_$($cameraEvent.Name)"
        if ($VideoFile) { Start-Sleep -Seconds 1 }
    }
    Add-CompletionVideoEvent -Event "deterministic_camera_move_started"
    Send-DesktopVerificationEvent `
        -Window $window `
        -Message ([LifecycleWindow]::StartCameraMoveMessage) `
        -Name "deterministic camera move"
    Wait-ForWindowTitle -Process $demoProcess -Window $window -Pattern "camera-move-complete" | Out-Null
    Add-CompletionVideoEvent -Event "deterministic_camera_move_completed"
    if ($VideoFile) { Start-Sleep -Seconds 1 }

    $completion = Complete-DesktopDemoProcess `
        -Process $demoProcess `
        -Window $window `
        -Name "desktop demo lifecycle" `
        -StandardOutputFile "desktop-demo.stdout.log" `
        -StandardErrorFile "desktop-demo.stderr.log"
    Add-CompletionVideoEvent -Event "clean_close"
    if ($null -ne $completionVideoRecording) {
        Start-Sleep -Seconds 1
        $script:completionVideoStopwatch.Stop()
        $script:completionVideoEvents | ConvertTo-Json -Depth 4 | Set-Content -Encoding utf8 (Join-Path $evidencePath "completion-video-events.json")
        Stop-CompletionVideoRecording -Recording $completionVideoRecording
        $completionVideoRecording = $null
    }
    $demoStandardOutput = $completion.StandardOutput
    $demoStandardError = $completion.StandardError
    foreach ($requiredLine in @("Vulkan device:", "Driver version:", "Vulkan API version:", "Vulkan validation: enabled")) {
        if ($demoStandardOutput -notmatch [Regex]::Escape($requiredLine)) {
            throw "The desktop-demo runtime context is missing '$requiredLine'."
        }
    }
    foreach ($requiredBackgroundLine in @(
        "Background raster preparation paused: revision=1",
        "Desktop lifecycle serviced while preparation paused: suspended",
        "Background raster preparation released",
        "Raster artifact installed: revision=1 count=1",
        "First matching raster frame presented: revision=1",
        "Canonical camera presented: camera=overview",
        "Canonical camera presented: camera=cavity",
        "Canonical camera presented: camera=boundary",
        "Deterministic camera move started: steps=120",
        "Deterministic camera move completed: steps=120"
    )) {
        if ($demoStandardOutput -notmatch [Regex]::Escape($requiredBackgroundLine)) {
            throw "The uninterrupted background lifecycle proof is missing '$requiredBackgroundLine'."
        }
    }
    $pausedResizeEvents = [Regex]::Matches(
        $demoStandardOutput,
        "(?m)^Desktop lifecycle serviced while preparation paused: resize=\d+x\d+$"
    )
    if ($pausedResizeEvents.Count -lt 3) {
        throw "The uninterrupted background lifecycle proof did not retain all paused resize and restore events."
    }
    $validationWarnings = $completion.ValidationWarnings
    $validationErrors = $completion.ValidationErrors
    $deviceMatch = [Regex]::Match($demoStandardOutput, "(?m)^Vulkan device: (.+)$")
    $driverMatch = [Regex]::Match($demoStandardOutput, "(?m)^Driver version: (.+)$")
    $apiMatch = [Regex]::Match($demoStandardOutput, "(?m)^Vulkan API version: (.+)$")
    $overviewCapture = $captures | Select-Object -First 1
    if ($null -eq $overviewCapture) {
        throw "The lifecycle run did not retain the canonical overview capture."
    }
    $canonicalScene = Read-CanonicalSceneRecord -StandardOutput $demoStandardOutput
    $canonicalInspections = @(
        [PSCustomObject]@{
            Name = "overview"
            CameraSelection = $CameraPose
            Capture = $overviewCapture
            StandardOutput = "desktop-demo.stdout.log"
            ValidationLog = "desktop-demo.stderr.log"
            ValidationWarnings = $validationWarnings
            ValidationErrors = $validationErrors
            Scene = $canonicalScene
            Camera = Read-CanonicalCameraRecord -StandardOutput $demoStandardOutput
        }
    )
    $windingDiagnosticInspection = $null
    if ($CaptureCanonicalInspectionSet) {
        $canonicalInspections += Capture-AdditionalCanonicalPresentation `
            -BinaryPath $binaryPath `
            -Name "cavity" `
            -Arguments @("--scene-scale", "256", "--camera-pose", "cavity") `
            -CameraSelection "cavity"
        $canonicalInspections += Capture-AdditionalCanonicalPresentation `
            -BinaryPath $binaryPath `
            -Name "boundary" `
            -Arguments @("--scene-scale", "256", "--camera-pose", "boundary") `
            -CameraSelection "boundary"
        $canonicalInspections += Capture-AdditionalCanonicalPresentation `
            -BinaryPath $binaryPath `
            -Name "move-midpoint" `
            -Arguments @("--scene-scale", "256", "--camera-move-step", "60") `
            -CameraSelection "move"
        foreach ($inspection in $canonicalInspections) {
            if ($inspection.Scene.GeneratorIdentity -ne $canonicalScene.GeneratorIdentity `
                -or $inspection.Scene.GeneratorVersion -ne $canonicalScene.GeneratorVersion `
                -or $inspection.Scene.Seed -ne $canonicalScene.Seed `
                -or $inspection.Scene.OccupiedCount -ne $canonicalScene.OccupiedCount `
                -or $inspection.Scene.ExposedFaceCount -ne $canonicalScene.ExposedFaceCount) {
                throw "Canonical inspection $($inspection.Name) did not render the same generated Voxel Scene."
            }
        }
        $windingDiagnosticInspection = Capture-WindingDiagnosticPresentation `
            -BinaryPath $binaryPath
    }

    $revision = (& git rev-parse HEAD).Trim()
    if ($LASTEXITCODE -ne 0) {
        throw "Could not determine the build revision."
    }
    $rustVersion = (& rustc --version).Trim()
    $cargoVersion = (& cargo --version).Trim()
    $operatingSystem = Get-CimInstance Win32_OperatingSystem
    $manifest = [ordered]@{
        SchemaVersion = 3
        Scope = "Runtime execution proven only on this Windows development machine."
        RecordedAtUtc = [DateTime]::UtcNow.ToString("o")
        RepositoryRevision = $revision
        BuildProfile = "dev (unoptimized + debuginfo)"
        BuildCommand = "cargo build --locked --package desktop-demo"
        ShaderArtifacts = "Generated from raster.vert and raster.frag by raster-render-path/build.rs during the Cargo build."
        ValidationContext = "VK_LAYER_KHRONOS_validation required and enabled by the application."
        ValidationWarnings = $validationWarnings
        ValidationErrors = $validationErrors
        ProcessExitCode = $demoProcess.ExitCode
        Machine = [ordered]@{
            OperatingSystem = $operatingSystem.Caption
            OperatingSystemVersion = $operatingSystem.Version
            Rust = $rustVersion
            Cargo = $cargoVersion
            VulkanSdk = $env:VULKAN_SDK
        }
        VulkanRuntime = [ordered]@{
            Device = $deviceMatch.Groups[1].Value.Trim()
            DriverVersion = $driverMatch.Groups[1].Value.Trim()
            ApiVersion = $apiMatch.Groups[1].Value.Trim()
        }
        RuntimeContextLog = "desktop-demo.stdout.log"
        ValidationLog = "desktop-demo.stderr.log"
        CompletionVideo = if ($VideoFile) {
            [ordered]@{
                File = $VideoFile
                EventTimeline = "completion-video-events.json"
                CaptureScope = "1140x940 controlled black-backed desktop region at screen coordinates 80,80"
                RecorderStandardOutput = "completion-video.stdout.log"
                RecorderStandardError = "completion-video.stderr.log"
                Uninterrupted = $true
            }
        } else { $null }
        Lifecycle = @(
            "background preparation paused",
            "landscape resize while paused",
            "portrait resize while paused",
            "zero-size suspension while paused",
            "minimize while paused",
            "restore and presentation recreation while paused",
            "background preparation released",
            "one matching-revision artifact installed",
            "first matching-revision frame presented",
            "overview, cavity, and boundary poses presented",
            "120-step deterministic camera move completed",
            "normal close",
            "clean process exit"
        )
        PausedLifecycleExtents = [ordered]@{
            Landscape = @($pausedLandscapeArea.Width, $pausedLandscapeArea.Height)
            Portrait = @($pausedPortraitArea.Width, $pausedPortraitArea.Height)
        }
        Presentations = $captures
        CanonicalScene = $canonicalScene
        CanonicalInspections = $canonicalInspections
        WindingDiagnosticInspection = $windingDiagnosticInspection
        UnsupportedPrerequisites = $unsupportedCases
        BackgroundPreparationFailure = [ordered]@{
            Phase = "derivation"
            SourceRevision = 1
            ExitCode = $preparationFailure.ExitCode
            StandardError = "background-derivation.stderr.log"
        }
        UploadInstallationFailure = [ordered]@{
            Phase = "upload"
            SourceRevision = 1
            InstalledRevisionAfterFailure = $null
            ExitCode = $uploadInstallationFailure.ExitCode
            StandardError = "raster-install-upload.stderr.log"
        }
        RenderPathFailures = $renderPathFailures
    }
    $manifest | ConvertTo-Json -Depth 8 | Set-Content -Encoding utf8 (Join-Path $evidencePath "manifest.json")
    Write-Host "Windows lifecycle proof passed. Evidence: $evidencePath"
}
finally {
    Pop-Location
    if ($null -ne $completionVideoRecording) {
        try {
            Stop-CompletionVideoRecording -Recording $completionVideoRecording
        }
        catch {
            Write-Warning "Could not finish completion video after lifecycle failure: $($_.Exception.Message)"
        }
    }
    if ($null -ne (Get-Variable demoProcess -ErrorAction SilentlyContinue) -and -not $demoProcess.HasExited) {
        Stop-LifecycleProcess -Process $demoProcess
    }
}
