[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$EvidenceDirectory,
    [switch]$SkipBuild
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if (-not $IsWindows -and $PSVersionTable.PSEdition -eq "Core") {
    throw "The compute shutdown proof runs only on Windows."
}

$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$evidencePath = if ([System.IO.Path]::IsPathRooted($EvidenceDirectory)) {
    [System.IO.Path]::GetFullPath($EvidenceDirectory)
} else {
    [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot $EvidenceDirectory))
}
if ([System.IO.Directory]::Exists($evidencePath) -and [System.IO.Directory]::EnumerateFileSystemEntries($evidencePath).GetEnumerator().MoveNext()) {
    throw "The evidence directory must be new or empty: $evidencePath"
}
[System.IO.Directory]::CreateDirectory($evidencePath) | Out-Null

Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class ComputeShutdownWindow {
    public const uint CloseMessage = 0x0010;
    public const uint KeyDownMessage = 0x0100;
    public const uint KeyUpMessage = 0x0101;
    public const uint TabKey = 0x09;
    public const uint SpaceKey = 0x20;

    public delegate bool EnumWindowsCallback(IntPtr window, IntPtr parameter);

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
    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    while ([DateTime]::UtcNow -lt $deadline) {
        if ($Process.HasExited) {
            throw "The desktop demo exited before creating its window."
        }
        $window = [ComputeShutdownWindow]::Find([uint32]$Process.Id)
        if ($window -ne [IntPtr]::Zero) {
            return $window
        }
        Start-Sleep -Milliseconds 25
    }
    throw "The desktop demo did not create its window within 15 seconds."
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
        if ($Process.HasExited) {
            throw "The desktop demo exited while waiting for title pattern '$Pattern'."
        }
        $title = [ComputeShutdownWindow]::Title($Window)
        if ($title -match $Pattern) {
            return $title
        }
        Start-Sleep -Milliseconds 25
    }
    throw "The desktop demo did not report '$Pattern' within $TimeoutSeconds seconds. Last title: $([ComputeShutdownWindow]::Title($Window))"
}

function Send-Key {
    param(
        [IntPtr]$Window,
        [uint32]$Key,
        [string]$Name
    )
    if (-not [ComputeShutdownWindow]::PostMessage($Window, [ComputeShutdownWindow]::KeyDownMessage, [IntPtr]$Key, [IntPtr]::Zero)) {
        throw "Could not send the $Name key-down message."
    }
    if (-not [ComputeShutdownWindow]::PostMessage($Window, [ComputeShutdownWindow]::KeyUpMessage, [IntPtr]$Key, [IntPtr]::Zero)) {
        throw "Could not send the $Name key-up message."
    }
}

function Invoke-ComputeShutdownCase {
    param(
        [string]$Name,
        [string]$Qualification,
        [string]$SwitchPattern,
        [string]$ReadyPattern,
        [string]$LifecycleLine,
        [string]$ClosingLine,
        [bool]$StartEdit
    )
    $standardOutputPath = Join-Path $evidencePath "$Name.stdout.log"
    $standardErrorPath = Join-Path $evidencePath "$Name.stderr.log"
    $process = Start-Process `
        -FilePath $binaryPath `
        -ArgumentList @("--scene-scale", "64", "--compute-shutdown-qualification", $Qualification) `
        -RedirectStandardOutput $standardOutputPath `
        -RedirectStandardError $standardErrorPath `
        -PassThru
    try {
        $window = Wait-ForWindow -Process $process
        Wait-ForTitle -Process $process -Window $window -Pattern "Presenter=Raster Switch=idle.*Control=Tab-ready" | Out-Null
        Send-Key -Window $window -Key ([ComputeShutdownWindow]::TabKey) -Name "Tab"
        Wait-ForTitle -Process $process -Window $window -Pattern $SwitchPattern -TimeoutSeconds 60 | Out-Null
        if ($StartEdit) {
            Send-Key -Window $window -Key ([ComputeShutdownWindow]::SpaceKey) -Name "Space"
            Wait-ForTitle -Process $process -Window $window -Pattern $ReadyPattern -TimeoutSeconds 60 | Out-Null
        }
        if (-not [ComputeShutdownWindow]::PostMessage($window, [ComputeShutdownWindow]::CloseMessage, [IntPtr]::Zero, [IntPtr]::Zero)) {
            throw "Could not close the $Name desktop demo."
        }
        if (-not $process.WaitForExit(30000)) {
            throw "The $Name desktop demo did not exit within 30 seconds."
        }
        $standardOutput = [System.IO.File]::ReadAllText($standardOutputPath)
        $standardError = [System.IO.File]::ReadAllText($standardErrorPath)
        if ($process.ExitCode -ne 0) {
            throw "The $Name desktop demo exited with code $($process.ExitCode): $standardError"
        }
        foreach ($requiredLine in @(
            "Vulkan validation: enabled",
            $LifecycleLine,
            $ClosingLine,
            "Render Path-owned raster resources after shutdown: 0",
            "Render Path-owned compute resources after shutdown: objects=0 allocations=0 workers=0 views=0",
            "Render Path switching resources after shutdown: replacement=0 retiring=0"
        )) {
            if ($standardOutput -notmatch [Regex]::Escape($requiredLine)) {
                throw "The $Name proof is missing '$requiredLine'."
            }
        }
        $validationWarnings = ([Regex]::Matches($standardError, "(?m)^Vulkan validation WARNING")).Count
        $validationErrors = ([Regex]::Matches($standardError, "(?m)^Vulkan validation ERROR")).Count
        if ($validationWarnings -ne 0 -or $validationErrors -ne 0) {
            throw "The $Name proof reported $validationWarnings validation warning(s) and $validationErrors validation error(s)."
        }
        [PSCustomObject]@{
            Name = $Name
            Qualification = $Qualification
            ExitCode = $process.ExitCode
            ValidationWarnings = $validationWarnings
            ValidationErrors = $validationErrors
            StandardOutput = "$Name.stdout.log"
            StandardError = "$Name.stderr.log"
        }
    }
    finally {
        if (-not $process.HasExited) {
            $process.Kill($true)
            $process.WaitForExit()
        }
        $process.Dispose()
    }
}

Push-Location $repositoryRoot
try {
    if (-not $SkipBuild) {
        & cargo build --locked --features qualification --package desktop-demo
        if ($LASTEXITCODE -ne 0) {
            throw "The desktop demo build failed."
        }
    }
    $binaryPath = Join-Path $repositoryRoot "target\debug\desktop-demo.exe"
    if (-not (Test-Path -LiteralPath $binaryPath -PathType Leaf)) {
        throw "The desktop demo binary does not exist: $binaryPath"
    }
    $cases = @()
    $cases += Invoke-ComputeShutdownCase `
        -Name "compute-presenting-close" `
        -Qualification "presenting" `
        -SwitchPattern "Presenter=ComputeRay Switch=idle.*Control=Space-ready" `
        -ReadyPattern "Control=Space-ready" `
        -LifecycleLine "Render Path retirement complete: Retired=Raster owned_resources=0 workers=0 completed_switches=1" `
        -ClosingLine "Closing while compute presents with switching idle" `
        -StartEdit $false
    $cases += Invoke-ComputeShutdownCase `
        -Name "compute-replacement-close" `
        -Qualification "replacement" `
        -SwitchPattern "Presenter=Raster Switch=handoff-ready" `
        -ReadyPattern "Switch=handoff-ready" `
        -LifecycleLine "Tab switch accepted: Presenting=Raster Replacement=ComputeRay revision=1" `
        -ClosingLine "Closing with compute replacement owned before handoff" `
        -StartEdit $false
    $cases += Invoke-ComputeShutdownCase `
        -Name "compute-active-preparation-close" `
        -Qualification "active-preparation" `
        -SwitchPattern "Presenter=ComputeRay Switch=idle.*Control=Space-ready" `
        -ReadyPattern "Burst=shutdown-active-preparation-held.*Control=shutdown-active-preparation-ready" `
        -LifecycleLine "Render Path retirement complete: Retired=Raster owned_resources=0 workers=0 completed_switches=1" `
        -ClosingLine "Closing with active compute preparation: revision=2 workers=1" `
        -StartEdit $true
    $cases += Invoke-ComputeShutdownCase `
        -Name "compute-hidden-candidate-close" `
        -Qualification "hidden-candidate" `
        -SwitchPattern "Presenter=ComputeRay Switch=idle.*Control=Space-ready" `
        -ReadyPattern "Burst=shutdown-hidden-candidate-held.*Control=shutdown-hidden-candidate-ready" `
        -LifecycleLine "Render Path retirement complete: Retired=Raster owned_resources=0 workers=0 completed_switches=1" `
        -ClosingLine "Closing with hidden uploaded compute candidate: revision=2" `
        -StartEdit $true
    [ordered]@{
        SchemaVersion = 1
        Scope = "Validation-enabled compute shutdown qualification on this Windows machine."
        RecordedAtUtc = [DateTime]::UtcNow.ToString("o")
        RepositoryRevision = (& git rev-parse HEAD).Trim()
        Cases = $cases
    } | ConvertTo-Json -Depth 5 | Set-Content -Encoding utf8 (Join-Path $evidencePath "manifest.json")
    Write-Host "Compute shutdown proof passed. Evidence: $evidencePath"
}
finally {
    Pop-Location
}
