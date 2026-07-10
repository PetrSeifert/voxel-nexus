[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$EvidenceDirectory
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if (-not $IsWindows -and $PSVersionTable.PSEdition -eq "Core") {
    throw "The lifecycle proof runs only on Windows."
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

public static class LifecycleWindow {
    public const int ShowMinimized = 6;
    public const int ShowRestored = 9;
    public const uint CloseMessage = 0x0010;
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
        $trianglePixels = 0L
        $backgroundPixels = 0L
        $sampledPixels = 0L
        for ($y = 0; $y -lt $bitmap.Height; $y += 2) {
            for ($x = 0; $x -lt $bitmap.Width; $x += 2) {
                $sampledPixels++
                $pixel = $bitmap.GetPixel($x, $y)
                if ($pixel.R -gt 200 -and ($pixel.R - $pixel.G) -gt 80 -and ($pixel.R - $pixel.B) -gt 60) {
                    $trianglePixels++
                }
                if ($pixel.R -lt 100 -and $pixel.G -lt 100 -and $pixel.B -lt 130) {
                    $backgroundPixels++
                }
            }
        }
        $bitmap.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
        $triangleFraction = $trianglePixels / $sampledPixels
        $backgroundFraction = $backgroundPixels / $sampledPixels
        if ($triangleFraction -lt 0.20 -or $triangleFraction -gt 0.25 -or $backgroundFraction -lt 0.74 -or $backgroundFraction -gt 0.80) {
            throw "Capture $Name does not contain the expected complete triangle and background coverage (triangle: $triangleFraction; background: $backgroundFraction)."
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
        TriangleSamplePixels = $trianglePixels
        BackgroundSamplePixels = $backgroundPixels
        SampledPixels = $sampledPixels
        TriangleFraction = [Math]::Round($triangleFraction, 4)
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
                        TriangleSamplePixels = $first.TriangleSamplePixels
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

Push-Location $repositoryRoot
try {
    Invoke-Cargo -Arguments @("build", "--locked", "--package", "desktop-demo")
    Invoke-Cargo -Arguments @("test", "--locked", "--package", "desktop-demo", "--test", "unsupported_prerequisites")

    $binaryPath = Join-Path $repositoryRoot "target\debug\desktop-demo.exe"
    $unsupportedCases = @()
    foreach ($case in @("vulkan-1.2", "presentation")) {
        $caseResult = Invoke-CapturedProcess `
            -Executable $binaryPath `
            -Arguments @("--verify-unsupported-prerequisite", $case) `
            -StandardOutputPath (Join-Path $evidencePath "unsupported-$case.stdout.log") `
            -StandardErrorPath (Join-Path $evidencePath "unsupported-$case.stderr.log")
        if ($caseResult.ExitCode -eq 0 -or $caseResult.StandardError -notmatch "Voxel Nexus could not start") {
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

    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $binaryPath
    $startInfo.WorkingDirectory = $repositoryRoot
    $startInfo.UseShellExecute = $false
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $demoProcess = [System.Diagnostics.Process]::new()
    $demoProcess.StartInfo = $startInfo
    if (-not $demoProcess.Start()) {
        throw "Could not start the desktop demo."
    }

    $window = Wait-ForWindow -Process $demoProcess
    if (-not [LifecycleWindow]::MoveWindow($window, 100, 100, 900, 650, $true)) {
        throw "Could not set the desktop demo launch extent."
    }
    $captures = @()
    $captures += Capture-StablePresentation -Window $window -Name "launch"

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

    if (-not [LifecycleWindow]::PostMessage($window, [LifecycleWindow]::CloseMessage, [IntPtr]::Zero, [IntPtr]::Zero)) {
        throw "Could not request a normal desktop-demo window close."
    }
    if (-not $demoProcess.WaitForExit(10000)) {
        Stop-LifecycleProcess -Process $demoProcess
        throw "The desktop demo did not exit within 10 seconds of a normal close."
    }
    $demoStandardOutput = $demoProcess.StandardOutput.ReadToEnd()
    $demoStandardError = $demoProcess.StandardError.ReadToEnd()
    [System.IO.File]::WriteAllText((Join-Path $evidencePath "desktop-demo.stdout.log"), $demoStandardOutput)
    [System.IO.File]::WriteAllText((Join-Path $evidencePath "desktop-demo.stderr.log"), $demoStandardError)
    if ($demoProcess.ExitCode -ne 0) {
        throw "The desktop demo exited with code $($demoProcess.ExitCode)."
    }
    foreach ($requiredLine in @("Vulkan device:", "Driver version:", "Vulkan API version:", "Vulkan validation: enabled")) {
        if ($demoStandardOutput -notmatch [Regex]::Escape($requiredLine)) {
            throw "The desktop-demo runtime context is missing '$requiredLine'."
        }
    }
    $validationWarnings = ([Regex]::Matches($demoStandardError, "(?m)^Vulkan validation WARNING")).Count
    $validationErrors = ([Regex]::Matches($demoStandardError, "(?m)^Vulkan validation ERROR")).Count
    if ($validationWarnings -ne 0 -or $validationErrors -ne 0) {
        throw "The lifecycle run produced $validationWarnings validation warning(s) and $validationErrors validation error(s)."
    }
    $deviceMatch = [Regex]::Match($demoStandardOutput, "(?m)^Vulkan device: (.+)$")
    $driverMatch = [Regex]::Match($demoStandardOutput, "(?m)^Driver version: (.+)$")
    $apiMatch = [Regex]::Match($demoStandardOutput, "(?m)^Vulkan API version: (.+)$")

    $revision = (& git rev-parse HEAD).Trim()
    if ($LASTEXITCODE -ne 0) {
        throw "Could not determine the build revision."
    }
    $rustVersion = (& rustc --version).Trim()
    $cargoVersion = (& cargo --version).Trim()
    $operatingSystem = Get-CimInstance Win32_OperatingSystem
    $manifest = [ordered]@{
        SchemaVersion = 1
        Scope = "Runtime execution proven only on this Windows development machine."
        RecordedAtUtc = [DateTime]::UtcNow.ToString("o")
        RepositoryRevision = $revision
        BuildProfile = "dev (unoptimized + debuginfo)"
        BuildCommand = "cargo build --locked --package desktop-demo"
        ShaderArtifacts = "Generated from triangle.vert and triangle.frag by render-backend/build.rs during the Cargo build."
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
        Lifecycle = @("launch", "landscape resize", "portrait resize", "minimize", "restore", "normal close", "clean process exit")
        Presentations = $captures
        UnsupportedPrerequisites = $unsupportedCases
    }
    $manifest | ConvertTo-Json -Depth 8 | Set-Content -Encoding utf8 (Join-Path $evidencePath "manifest.json")
    Write-Host "Windows lifecycle proof passed. Evidence: $evidencePath"
}
finally {
    Pop-Location
    if ($null -ne (Get-Variable demoProcess -ErrorAction SilentlyContinue) -and -not $demoProcess.HasExited) {
        Stop-LifecycleProcess -Process $demoProcess
    }
}
