param(
    [string]$EvidenceDirectory = "docs/evidence/streamed-fixture/development-machine",
    [switch]$RunCpu
)

$ErrorActionPreference = "Stop"
$workspaceDirectory = Split-Path -Parent $PSScriptRoot
Push-Location $workspaceDirectory
try {
    if ($RunCpu) {
        cargo build --release -p desktop-demo --features qualification --bin streamed-residency-prototype
        if ($LASTEXITCODE -ne 0) { throw "CPU qualification build failed" }
        foreach ($mode in @("cpu-calibration", "cpu-baseline", "cpu-matched-8", "cpu-matched-16", "cpu-lifecycle")) {
            & ./target/release/streamed-residency-prototype.exe $mode (Join-Path $EvidenceDirectory "$mode.jsonl")
            if ($LASTEXITCODE -ne 0) { throw "$mode failed" }
        }
    }
    function Read-Records($name) {
        return @(Get-Content -LiteralPath (Join-Path $EvidenceDirectory "$name.jsonl") | ForEach-Object { $_ | ConvertFrom-Json })
    }
    function One-Record($records, $kind, $phase) {
        $matching = @($records | Where-Object { $_.kind -eq $kind -and $_.phase -eq $phase })
        if ($matching.Count -ne 1) { throw "Expected one $kind/$phase record" }
        return $matching[0]
    }
    function Maximum($values) { return [long](($values | Measure-Object -Maximum).Maximum) }
    function Same-Array($first, $second) { return ($first | ConvertTo-Json -Compress) -eq ($second | ConvertTo-Json -Compress) }

    $calibration = Read-Records "cpu-calibration"
    $gpuCalibration = Read-Records "residency-calibration"
    $lifecycle = Read-Records "cpu-lifecycle"
    $baseline = One-Record (Read-Records "cpu-baseline") "cpu-residency" "cpu-baseline"
    $small = One-Record (Read-Records "cpu-matched-8") "cpu-residency" "cpu-matched-8"
    $large = One-Record (Read-Records "cpu-matched-16") "cpu-residency" "cpu-matched-16"
    $gpuBaseline = Read-Records "residency-baseline"
    $gpuMatched = Read-Records "residency-matched-16"
    $route = Read-Records "residency-raster"
    $incident = Get-Content -Raw -LiteralPath (Join-Path $EvidenceDirectory "host-failure.json") | ConvertFrom-Json

    foreach ($phase in @("generated", "edited", "restored")) {
        $fingerprint = One-Record $calibration "fingerprint" $phase
        $expected = if ($phase -eq "edited") { "478ee4a9737e1ad7" } else { "9660d9308d69ede5" }
        if ($fingerprint.fingerprint -ne $expected) { throw "Frozen fingerprint changed" }
    }
    foreach ($category in 1..4) {
        if ($small.cpu_live[$category] -ne $large.cpu_live[$category]) { throw "Matched CPU category $category differs" }
    }
    if ($small.cpu_live[6] -ne $large.cpu_live[6] -or $small.cpu_live[5] -ge $large.cpu_live[5]) {
        throw "Metadata/history separation failed"
    }
    $perVolume = @($calibration | Where-Object kind -eq "cpu-residency")
    $materialized = Maximum @($perVolume | ForEach-Object { [long]$_.cpu_live[1] + [long]$_.cpu_live[2] })
    $publicationPeak = Maximum @($perVolume | ForEach-Object { [long]$_.cpu_peak[1] + [long]$_.cpu_peak[2] })
    $raster = One-Record $gpuCalibration "residency" "edited-raster"
    $generatedRaster = One-Record $gpuCalibration "residency" "generated-raster"
    $brickmap = One-Record $gpuCalibration "residency" "edited-brickmap"
    foreach ($phase in @("generated", "edited", "restored")) {
        $stateRaster = One-Record $gpuCalibration "residency" "$phase-raster"
        $stateBrickmap = One-Record $gpuCalibration "residency" "$phase-brickmap"
        if ($stateRaster.cpu_live[3] -gt $raster.cpu_live[3] -or $stateRaster.cpu_peak[3] -gt $raster.cpu_peak[3] `
                -or $stateRaster.gpu_live[1] -gt $raster.gpu_live[1] -or $stateBrickmap.cpu_live[4] -gt $brickmap.cpu_live[4] `
                -or $stateBrickmap.cpu_peak[4] -gt $brickmap.cpu_peak[4] -or $stateBrickmap.gpu_live[2] -gt $brickmap.gpu_live[2]) {
            throw "Per-volume coefficients do not bound every frozen state"
        }
    }
    $selectedRaster = One-Record $gpuMatched "residency" "matched-edited-raster"
    $rasterOverhead = [long]$selectedRaster.cpu_live[3] - 8 * [long]$generatedRaster.cpu_live[3] - [long]$raster.cpu_live[3]
    if ($rasterOverhead -lt 0) { throw "Negative raster selection overhead" }
    $renderCopies = 3 * 18
    $caps = [ordered]@{
        cpu_materialization = [ordered]@{ formula = "19*S + (P-S)"; S = $materialized; P = $publicationPeak; bytes = 19 * $materialized + $publicationPeak - $materialized }
        raster_cpu = [ordered]@{ formula = "54*R + 6*H + R_peak"; R = $raster.cpu_live[3]; H = $rasterOverhead; R_peak = $raster.cpu_peak[3]; bytes = $renderCopies * [long]$raster.cpu_live[3] + 6 * $rasterOverhead + [long]$raster.cpu_peak[3] }
        brickmap_cpu = [ordered]@{ formula = "54*B + 9*B_peak"; B = $brickmap.cpu_live[4]; B_peak = $brickmap.cpu_peak[4]; bytes = $renderCopies * [long]$brickmap.cpu_live[4] + 9 * [long]$brickmap.cpu_peak[4] }
        raster_gpu = [ordered]@{ formula = "54*G_raster"; G_raster = $raster.gpu_live[1]; bytes = $renderCopies * [long]$raster.gpu_live[1] }
        brickmap_gpu = [ordered]@{ formula = "54*G_brickmap"; G_brickmap = $brickmap.gpu_live[2]; bytes = $renderCopies * [long]$brickmap.gpu_live[2] }
    }
    $fullyResidentRaster = One-Record $gpuBaseline "residency" "initial"
    $fullyResidentBrickmap = One-Record $gpuBaseline "residency" "baseline-brickmap"
    $baselineBytes = [ordered]@{
        cpu_materialization = [long]$baseline.cpu_live[1] + [long]$baseline.cpu_live[2]
        raster_cpu = [long]$baseline.cpu_live[3]
        brickmap_cpu = [long]$baseline.cpu_live[4]
        raster_gpu = [long]$fullyResidentRaster.gpu_live[1]
        brickmap_gpu = [long]$fullyResidentBrickmap.gpu_live[2]
    }
    foreach ($category in $caps.Keys) {
        if ($baselineBytes[$category] -le $caps[$category].bytes) { throw "Baseline does not exceed $category cap" }
    }
    $cpuSamples = @($lifecycle | Where-Object kind -eq "cpu-residency")
    foreach ($record in $cpuSamples) {
        if ($record.copies -gt 19 -or $record.peak_copies -gt 19 -or $record.query_copies -gt 1 -or $record.generation_workers_peak -gt 1 -or $record.derivation_workers_peak -gt 1) {
            throw "CPU admission/worker envelope failed"
        }
        if ([long]$record.cpu_peak[1] + [long]$record.cpu_peak[2] -gt $caps.cpu_materialization.bytes `
                -or $record.cpu_peak[3] -gt $caps.raster_cpu.bytes -or $record.cpu_peak[4] -gt $caps.brickmap_cpu.bytes) {
            throw "CPU category envelope failed"
        }
        if ($record.cpu_peak[0] -gt 16MB -or $record.cpu_peak[5] -gt 256KB -or $record.cpu_peak[6] -gt 16KB) { throw "Plain CPU bound failed" }
    }
    $firstLap = One-Record $lifecycle "cpu-residency" "lap-0-settled"
    $secondLap = One-Record $lifecycle "cpu-residency" "lap-1-settled"
    if ($firstLap.copies -ne 9 -or $secondLap.copies -ne 9 `
            -or -not (Same-Array $firstLap.cpu_live[1..6] $secondLap.cpu_live[1..6]) `
            -or -not (Same-Array $firstLap.cpu_allocations[1..6] $secondLap.cpu_allocations[1..6])) { throw "CPU travel plateau failed" }
    foreach ($name in @("cpu-calibration", "cpu-baseline", "cpu-matched-8", "cpu-matched-16", "cpu-lifecycle")) {
        foreach ($release in @((Read-Records $name) | Where-Object kind -eq "cpu-released")) {
            if (@($release.live | Where-Object { $_ -ne 0 }).Count -ne 0) { throw "CPU cleanup debt" }
        }
    }
    $gpuSamples = @($route | Where-Object kind -eq "residency")
    foreach ($record in $gpuSamples) {
        if ($record.peak_copies -gt 19 -or $record.peak_owners -gt 3 `
                -or $record.gpu_peak[1] -gt $caps.raster_gpu.bytes -or $record.gpu_peak[2] -gt $caps.brickmap_gpu.bytes `
                -or $record.gpu_peak[0] -gt 3 * [long]$brickmap.gpu_live[0] -or $record.cpu_peak[0] -gt 16MB) {
            throw "Captured GPU prefix exceeds an envelope"
        }
    }
    $installed = @($route | Where-Object kind -eq "installed")
    foreach ($record in $installed) {
        if ($record.crossing_seconds -gt 2.5 -or ($null -ne $record.switch_seconds -and $record.switch_seconds -gt 2.5) `
                -or -not $record.fence_safe -or -not $record.selection_matches) { throw "Captured installation deadline failed" }
    }
    if ($incident.status -ne "host-gpu-stall" -or @($route | Where-Object kind -eq "route-result").Count -ne 0) {
        throw "Expected the recorded aborted GPU qualification, not a fabricated complete result"
    }
    $summary = [ordered]@{
        verdict = "FAIL"
        failed_phase = "GPU frame progress after Raster-to-Brickmap route handoff; host required restart"
        caps_status = "provisional, not ratified"
        caps = $caps
        baseline_bytes = $baselineBytes
        fixed_bounds = [ordered]@{ control_heap_bytes = 16MB; metadata_heap_bytes = 256KB; history_heap_bytes = 16KB; fixed_application_gpu_bytes = 3 * [long]$brickmap.gpu_live[0]; metadata_entries = 256; historical_views = 2; edited_coordinates = 6; source_recipes = 1; swapchain_images = 3; renderer_owners = 3; audit_entries = 65536 }
        cpu_lifecycle = [ordered]@{ peak_copies = Maximum $cpuSamples.peak_copies; settled_copies = 9; repeated_plateau = $true; cleanup = "zero"; metadata_live_bytes = $secondLap.cpu_live[5]; history_peak_bytes = Maximum @($cpuSamples | ForEach-Object { $_.cpu_peak[6] }); residency_peak_bytes = @((Maximum @($cpuSamples | ForEach-Object { [long]$_.cpu_peak[1] + [long]$_.cpu_peak[2] })), (Maximum @($cpuSamples | ForEach-Object { $_.cpu_peak[3] })), (Maximum @($cpuSamples | ForEach-Object { $_.cpu_peak[4] }))) }
        captured_gpu_prefix = [ordered]@{ crossings_completed = $installed.Count; maximum_crossing_seconds = ($installed.crossing_seconds | Measure-Object -Maximum).Maximum; maximum_switch_seconds = ($installed.switch_seconds | Where-Object { $null -ne $_ } | Measure-Object -Maximum).Maximum; residency_gpu_peak_bytes = @((Maximum @($gpuSamples | ForEach-Object { $_.gpu_peak[1] })), (Maximum @($gpuSamples | ForEach-Object { $_.gpu_peak[2] }))); complete_route = $false; full_coverage_stall_count = "unmeasured"; rendered_plateau = "unmeasured"; final_retirement_cleanup = "unmeasured" }
    }
    $summary | ConvertTo-Json -Depth 9 | Set-Content -LiteralPath (Join-Path $EvidenceDirectory "residency-summary.json")
    Write-Output "Evidence verified. Qualification verdict: FAIL. GPU feasibility shaping remains open."
} finally { Pop-Location }
