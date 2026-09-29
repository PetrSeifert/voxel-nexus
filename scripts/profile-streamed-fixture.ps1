param(
    [string]$EvidenceDirectory = "docs/evidence/streamed-fixture/development-machine",
    [switch]$VerifyOnly
)

$ErrorActionPreference = "Stop"
$workspaceDirectory = Split-Path -Parent $PSScriptRoot
Push-Location $workspaceDirectory
try {
    if (-not $VerifyOnly) {
        New-Item -ItemType Directory -Force $EvidenceDirectory | Out-Null
        cargo build --release -p desktop-demo --features qualification --bin streamed-fixture-profile
        if ($LASTEXITCODE -ne 0) { throw "Profile build failed" }
        foreach ($mode in @("calibration", "baseline")) {
            & ./target/release/streamed-fixture-profile.exe $mode `
                1> (Join-Path $EvidenceDirectory "$mode.jsonl") `
                2> (Join-Path $EvidenceDirectory "$mode.stderr.log")
            if ($LASTEXITCODE -ne 0) { throw "$mode profile failed" }
        }
        $compiler = @(rustc -Vv)
        if ($LASTEXITCODE -ne 0) { throw "Could not record compiler context" }
        $cargoVersion = cargo -V
        if ($LASTEXITCODE -ne 0) { throw "Could not record Cargo context" }
        $baseRevision = git rev-parse HEAD
        if ($LASTEXITCODE -ne 0) { throw "Could not record base revision" }
        $sourcePaths = @(
            "apps/desktop-demo/src/streamed_fixture_profile.rs",
            "apps/desktop-demo/src/streamed_fixture_recipe.rs",
            "apps/desktop-demo/src/streamed_fixture_vulkan.rs",
            "crates/canonical-scene/examples/support/allocation.rs",
            "scripts/profile-streamed-fixture.ps1"
        )
        $context = [ordered]@{
            captured_utc = (Get-Date).ToUniversalTime().ToString("o")
            base_revision = $baseRevision
            compiler = $compiler
            cargo = $cargoVersion
            operating_system = (Get-CimInstance Win32_OperatingSystem | Select-Object Caption, Version, BuildNumber)
            processor = (Get-CimInstance Win32_Processor | Select-Object -ExpandProperty Name)
            source_sha256 = @($sourcePaths | ForEach-Object {
                [ordered]@{ path = $_; sha256 = (Get-FileHash -LiteralPath $_ -Algorithm SHA256).Hash }
            })
            evidence_sha256 = @("calibration.jsonl", "baseline.jsonl") | ForEach-Object {
                [ordered]@{ path = $_; sha256 = (Get-FileHash -LiteralPath (Join-Path $EvidenceDirectory $_) -Algorithm SHA256).Hash }
            }
        }
        $context | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $EvidenceDirectory "context.json")
    }

    $calibration = @(Get-Content (Join-Path $EvidenceDirectory "calibration.jsonl") | ForEach-Object { $_ | ConvertFrom-Json })
    $baseline = @(Get-Content (Join-Path $EvidenceDirectory "baseline.jsonl") | ForEach-Object { $_ | ConvertFrom-Json })
    function Find-Record($records, $kind, $state) {
        $matching = @($records | Where-Object { $_.kind -eq $kind -and $_.state -eq $state })
        if ($matching.Count -ne 1) { throw "Expected one $kind/$state record" }
        return $matching[0]
    }
    $fixture = @($calibration | Where-Object kind -eq "fixture")
    if ($fixture.Count -ne 1 -or $fixture[0].generated_fingerprint -ne "9660d9308d69ede5" `
            -or $fixture[0].edited_fingerprint -ne "478ee4a9737e1ad7") {
        throw "Frozen fixture changed"
    }
    $baselineFixture = @($baseline | Where-Object kind -eq "fixture")
    if ($baselineFixture.Count -ne 1 -or $baselineFixture[0].recipe -ne $fixture[0].recipe `
            -or $baselineFixture[0].generated_fingerprint -ne $fixture[0].generated_fingerprint `
            -or $baselineFixture[0].edited_fingerprint -ne $fixture[0].edited_fingerprint) {
        throw "Baseline fixture differs"
    }
    $firstDevice = @($calibration | Where-Object kind -eq "device")
    $secondDevice = @($baseline | Where-Object kind -eq "device")
    if ($firstDevice.Count -ne 1 -or $secondDevice.Count -ne 1 `
            -or ($firstDevice[0] | ConvertTo-Json -Compress) -ne ($secondDevice[0] | ConvertTo-Json -Compress)) {
        throw "Device contexts differ"
    }
    foreach ($kind in @("installed-cpu", "raster", "brickmap")) {
        $small = Find-Record $calibration $kind "matched-8x8-selection"
        $large = Find-Record $calibration $kind "matched-16x16-selection"
        $fields = if ($kind -eq "installed-cpu") { @("live_bytes") } else { @("buffer_count", "isolated_vulkan_allocation_bytes_sum") }
        foreach ($field in $fields) {
            if ($small.$field -ne $large.$field) { throw "Matched $kind $field differs" }
        }
        if ($kind -ne "installed-cpu" -and $small.heap.live_bytes -ne $large.heap.live_bytes) {
            throw "Matched $kind heap differs"
        }
    }

    $cpu = Find-Record $calibration "installed-cpu" "edited-current-only"
    $raster = Find-Record $calibration "raster" "edited"
    $compute = Find-Record $calibration "brickmap" "edited"
    $publication = Find-Record $calibration "publication" "generated"
    $restoredCpu = Find-Record $calibration "installed-cpu" "restored-current-only"
    if ($restoredCpu.live_bytes -gt $cpu.live_bytes -or $publication.heap.live_bytes -gt $cpu.live_bytes) {
        throw "Edited materialization no longer bounds every scripted state; recalibrate constants"
    }
    foreach ($record in @($calibration | Where-Object { $_.kind -eq "raster" -and $_.volumes -eq 1 })) {
        if ($record.heap.live_bytes -gt $raster.heap.live_bytes -or $record.heap.peak_bytes -gt $raster.heap.peak_bytes `
                -or $record.isolated_vulkan_allocation_bytes_sum -gt $raster.isolated_vulkan_allocation_bytes_sum) {
            throw "Edited raster no longer bounds every scripted state; recalibrate constants"
        }
    }
    foreach ($record in @($calibration | Where-Object { $_.kind -eq "brickmap" -and $_.volumes -eq 1 })) {
        if ($record.heap.live_bytes -gt $compute.heap.live_bytes -or $record.heap.peak_bytes -gt $compute.heap.peak_bytes `
                -or $record.isolated_vulkan_allocation_bytes_sum -gt $compute.isolated_vulkan_allocation_bytes_sum) {
            throw "Edited Brickmap no longer bounds every scripted state; recalibrate constants"
        }
    }
    $generatedRaster = Find-Record $calibration "raster" "generated"
    $selectedRaster = Find-Record $calibration "raster" "matched-16x16-selection"
    $rasterSelectionOverhead = [long]$selectedRaster.heap.live_bytes `
        - 8 * [long]$generatedRaster.heap.live_bytes - [long]$raster.heap.live_bytes
    if ($rasterSelectionOverhead -lt 0) { throw "Unexpected raster selection overhead" }
    # These counts follow the ownership envelope, independently of the observed peaks.
    $limits = [ordered]@{
        materialized_copies = 19
        settled_copies = 9
        query_only_copies = 1
        render_owners = 3
        copies_per_render_owner = 18
        generation_workers = 1
        derivation_workers = 1
        maximum_derivation_volumes = 9
        scene_metadata_entries = 256
        source_recipes = 1
        materials = 2
        edited_coordinates = 6
        historical_views = 2
    }
    $renderCopies = $limits.render_owners * $limits.copies_per_render_owner
    $caps = [ordered]@{
        cpu_materialization = [ordered]@{
            formula = "19 * S + P"
            per_volume_bytes = $cpu.live_bytes
            scratch_bytes = $publication.heap.peak_bytes
            bytes = 19 * [long]$cpu.live_bytes + [long]$publication.heap.peak_bytes
        }
        raster_cpu = [ordered]@{
            formula = "54 * R + 6 * H_raster + R_peak"
            per_volume_bytes = $raster.heap.live_bytes
            selection_overhead_bytes = $rasterSelectionOverhead
            scratch_bytes = $raster.heap.peak_bytes
            bytes = $renderCopies * [long]$raster.heap.live_bytes `
                + 6 * $rasterSelectionOverhead + [long]$raster.heap.peak_bytes
        }
        brickmap_cpu = [ordered]@{
            formula = "54 * B + 9 * B_peak"
            per_volume_bytes = $compute.heap.live_bytes
            scratch_bytes = 9 * [long]$compute.heap.peak_bytes
            bytes = $renderCopies * [long]$compute.heap.live_bytes + 9 * [long]$compute.heap.peak_bytes
        }
        raster_gpu = [ordered]@{
            formula = "54 * G_raster"
            per_volume_bytes = $raster.isolated_vulkan_allocation_bytes_sum
            bytes = $renderCopies * [long]$raster.isolated_vulkan_allocation_bytes_sum
        }
        brickmap_gpu = [ordered]@{
            formula = "54 * G_brickmap"
            per_volume_bytes = $compute.isolated_vulkan_allocation_bytes_sum
            bytes = $renderCopies * [long]$compute.isolated_vulkan_allocation_bytes_sum
        }
    }
    $fullCpu = Find-Record $baseline "installed-cpu" "fully-resident-256"
    $fullRaster = Find-Record $baseline "raster" "fully-resident-256"
    $fullCompute = Find-Record $baseline "brickmap" "fully-resident-256"
    $baselineBytes = [ordered]@{
        cpu_materialization = $fullCpu.live_bytes
        raster_cpu = $fullRaster.heap.live_bytes
        brickmap_cpu = $fullCompute.heap.live_bytes
        raster_gpu = $fullRaster.isolated_vulkan_allocation_bytes_sum
        brickmap_gpu = $fullCompute.isolated_vulkan_allocation_bytes_sum
    }
    foreach ($category in $caps.Keys) {
        if ($baselineBytes[$category] -le $caps[$category].bytes) {
            throw "Fully resident baseline does not exceed proposed $category cap"
        }
    }
    $retention = Find-Record $baseline "view-retention" "fully-resident-256"
    if ($retention.volumes_pinned_after_frontend_drop -ne 256 -or $retention.live_bytes -le $caps.cpu_materialization.bytes) {
        throw "Expected existing-view retention counterexample is absent"
    }
    $selectedPublication = Find-Record $calibration "publication" "matched-16x16-selection"
    $summary = [ordered]@{
        allocation_calibration = "pass"
        qualification = "incomplete"
        existing_view_residency = "fail"
        cap_status = "proposed, not qualified"
        limits = $limits
        caps = $caps
        fully_resident_bytes = $baselineBytes
        retained_view_bytes = $retention.live_bytes
        nine_volume_publication_peak_bytes = $selectedPublication.heap.peak_bytes
        whole_selection_source_staging = if ($selectedPublication.heap.peak_bytes -gt $caps.cpu_materialization.bytes) { "fail" } else { "not a qualification verdict" }
        pending = @("streamed source/history reconstruction", "global materialization admission and draining", "presenting/replacement/retiring ownership", "actual simultaneous GPU peaks and fixed graphics resources", "fixed-route crossing installation and zero coverage stalls", "fence-safe switch latency", "query overlap and repeated-travel plateaus", "covered rendered/oracle probes", "typed streamed/dense rejection")
    }
    $summary | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $EvidenceDirectory "summary.json")
    $summary | ConvertTo-Json -Depth 8
} finally {
    Pop-Location
}
