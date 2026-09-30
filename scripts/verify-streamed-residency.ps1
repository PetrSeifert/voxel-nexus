param([string]$EvidenceDirectory='docs/evidence/streamed-fixture/production',[switch]$RunCpu,[switch]$RunGpu,[switch]$CpuOnly,[switch]$DryRun)
$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
$workspaceDirectory=Split-Path -Parent $PSScriptRoot
Push-Location $workspaceDirectory
try {
    $caps=@{cpu_materialization=10300368L;raster_cpu=58242848L;brickmap_cpu=8914536L;raster_gpu=980640L;brickmap_gpu=4760640L}
    $coefficients=@{S=413448L;P=2858304L;R=1046656L;H=91072L;R_peak=1176992L;B=100856L;B_peak=385368L;G_raster=18160L;G_brickmap=88160L}
    $formulas=@{cpu_materialization=19*$coefficients.S+$coefficients.P-$coefficients.S;raster_cpu=54*$coefficients.R+6*$coefficients.H+$coefficients.R_peak;brickmap_cpu=54*$coefficients.B+9*$coefficients.B_peak;raster_gpu=54*$coefficients.G_raster;brickmap_gpu=54*$coefficients.G_brickmap}
    foreach ($category in $caps.Keys) {if ($formulas[$category] -ne $caps[$category]) {throw 'Ratified formula changed'}}
    $cpuModes=@('cpu-calibration','cpu-matched-8','cpu-matched-16','cpu-lifecycle')
    $contract=Get-Content -Raw scripts/streamed-residency-contract.json | ConvertFrom-Json
    $crossingTimes=@(8.0,24.0,43.31370849898476,65.94112549695429,88.5685424949238,111.19595949289332,130.50966799187808,146.50966799187808)
    $sourcePaths=@(rg --files apps crates scripts -g '*.rs' -g '*.toml' -g '*.comp' -g '*.vert' -g '*.frag' -g '*.ps1' -g 'streamed-residency-contract.json' | ForEach-Object {$_.Replace('\','/')} | Sort-Object)+@('Cargo.toml','Cargo.lock')
    function Hash([string]$path) {
        $content=[IO.File]::ReadAllText((Join-Path $workspaceDirectory $path)).Replace("`r`n","`n")
        [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($content)))
    }
    if ((Hash 'scripts/streamed-residency-contract.json') -cne '0B71FE36C681FEE783346E8A0ECF719C0C8EFC638664F53189FF28502FFD5EC7') {throw 'Frozen edit script definitions changed'}
    function Commit-Hash([string]$revision,[string]$path) {
        $process=[Diagnostics.Process]::new()
        $process.StartInfo.FileName='git';$process.StartInfo.UseShellExecute=$false
        $process.StartInfo.RedirectStandardOutput=$true;$process.StartInfo.RedirectStandardError=$true
        foreach ($argument in @('cat-file','blob',"${revision}:$path")) {$process.StartInfo.ArgumentList.Add($argument)}
        if (-not $process.Start()) {throw 'Cannot read committed source'}
        $content=$process.StandardOutput.ReadToEnd().Replace("`r`n","`n");$errorText=$process.StandardError.ReadToEnd();$process.WaitForExit()
        if ($process.ExitCode -ne 0) {throw "Missing committed source $path : $errorText"}
        $process.Dispose()
        [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($content)))
    }
    function Frozen {
        $recipe=Hash 'apps/desktop-demo/src/streamed_fixture_recipe.rs';$route=Hash 'apps/desktop-demo/src/streamed_residency_route.rs'
        $probes=[IO.File]::ReadAllText((Join-Path $workspaceDirectory 'apps/desktop-demo/src/streamed_residency_probes.rs')).Replace("`r`n","`n")
        $start=$probes.IndexOf('        for pixel in');$end=$probes.IndexOf('            let observation',$start)
        if ($start -lt 0 -or $end -le $start) {throw 'Missing frozen probes'}
        $probeHash=[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($probes.Substring($start,$end-$start))))
        @{fixture=$recipe;recipe=$recipe;route=$route;crossing_clock=$route;coverage=$route;probes=$probeHash;edit_script=(Hash 'scripts/streamed-residency-contract.json')}
    }
    function Read([string]$name) { @(Get-Content -LiteralPath (Join-Path $EvidenceDirectory "$name.jsonl") | ForEach-Object {$_ | ConvertFrom-Json}) }
    function Records($rows,[string]$kind) { @($rows | Where-Object kind -eq $kind) }
    function One($rows,[string]$kind,[string]$phase='') {
        $items=@(Records $rows $kind)
        if ($phase) {$items=@($items | Where-Object phase -eq $phase)}
        if ($items.Count -ne 1) {throw "Expected one $kind/$phase record"};$items[0]
    }
    function True($value,[string]$reason) {if ($value -isnot [bool] -or -not $value) {throw $reason}}
    function Number($value,[double]$maximum,[string]$reason) {
        if ($null -eq $value -or $value -is [string] -or $value -is [bool] -or -not [double]::IsFinite([double]$value) -or $value -lt 0 -or $value -gt $maximum) {throw $reason}
    }
    function Array($values,[int]$length) {if (@($values).Count -ne $length) {throw 'Missing allocation array'};foreach ($value in $values) {Number $value ([double]::MaxValue) 'Invalid allocation count'}}
    function Zero($values) {foreach ($value in $values) {Number $value 0 'Validation message or cleanup debt'}}
    function Same($first,$second) {(ConvertTo-Json -InputObject $first -Compress) -ceq (ConvertTo-Json -InputObject $second -Compress)}
    function Sample($record,[bool]$gpu) {
        foreach ($field in @('cpu_live','cpu_peak','cpu_allocations')) {Array $record.$field 7}
        foreach ($values in @($record.cpu_live,$record.cpu_peak)) {
            Number ([long]$values[1]+[long]$values[2]) $caps.cpu_materialization 'CPU materialization/generation cap; reopen shaping'
            Number $values[3] $caps.raster_cpu 'Raster CPU cap; reopen shaping';Number $values[4] $caps.brickmap_cpu 'Brickmap CPU cap; reopen shaping'
            Number $values[0] 16777216 'Control heap bound';Number $values[5] 262144 'Metadata heap bound';Number $values[6] 16384 'Source/edit/history heap bound'
        }
        Number $record.copies 19 'Nineteen-copy admission';Number $record.peak_copies 19 'Nineteen-copy peak admission';Number $record.query_copies 1 'Query copy bound'
        Number $record.metadata_entries 256 'Metadata count bound';Number $record.edited_coordinates 6 'Edit count bound';Number $record.history_entries 24 'History entry bound'
        Number $record.live_historical_views 2 'Historical view bound';Number $record.peak_historical_views 2 'Historical view peak bound'
        Number $record.source_recipe_count 1 'Source recipe bound';Number $record.material_count 2 'Material count bound';Number $record.generating 1 'Generation worker bound'
        if ($record.source_recipe_count -ne 1 -or $record.material_count -ne 2) {throw 'Missing source recipe or material palette'}
        if (-not $gpu) {return}
        foreach ($field in @('gpu_live','gpu_peak','gpu_allocations','gpu_allocations_peak','gpu_objects','gpu_objects_peak')) {Array $record.$field 3}
        foreach ($values in @($record.gpu_live,$record.gpu_peak)) {
            Number $values[0] 26543904 'Fixed GPU cap; reopen shaping';Number $values[1] $caps.raster_gpu 'Raster GPU cap; reopen shaping';Number $values[2] $caps.brickmap_gpu 'Brickmap GPU cap; reopen shaping'
        }
        foreach ($values in @($record.gpu_objects,$record.gpu_objects_peak)) {Number $values[0] 6 'Pipeline object bound';Number $values[1] 3 'Image object bound';Number $values[2] 9 'Framebuffer bound'}
        Number $record.gpu_allocations_peak[0] 9 'Fixed GPU allocation bound';Number $record.gpu_allocations_peak[1] 6480 'Raster buffer bound';Number $record.gpu_allocations_peak[2] 3 'Brickmap scene buffer bound'
        Number $record.audit_entries 65536 'Audit entry bound';Number $record.owners 3 'Renderer owner bound';Number $record.workers 1 'Derivation worker bound';Number $record.raster_workers 1 'Raster worker bound';Number $record.brickmap_workers 1 'Brickmap worker bound';Number $record.representation_copies 18 'Representation copy bound';Number $record.coverage_stalls 0 'Coverage stall'
        if ($record.presentation_images -ne 3) {throw 'Swapchain object bound'}
    }
    function Write-Context([string]$kind) {
        $revision=git rev-parse HEAD;if ($LASTEXITCODE -ne 0) {throw 'Missing revision'}
        $toolchain=@(rustc -Vv);if ($LASTEXITCODE -ne 0) {throw 'Missing toolchain'}
        $cargoVersion=cargo -V;if ($LASTEXITCODE -ne 0) {throw 'Missing Cargo context'}
        $device=$null;if ($kind -eq 'gpu') {$device=One (Read 'residency-raster') 'device'}
        @{source_revision=$revision;source_dirty=(@(git status --porcelain).Count -ne 0);capture_kind=$kind;toolchain=$toolchain;cargo=$cargoVersion;gpu_device=$device;frozen_sha256=(Frozen);executable_sha256=(Get-FileHash target/release/streamed-residency-qualification.exe).Hash;source_sha256=@($sourcePaths | ForEach-Object {@{path=$_;sha256=(Hash $_)}})} | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $EvidenceDirectory residency-context.json)
    }
    if ($RunGpu) {
        if ($DryRun) {throw 'Recorded capture cannot use DryRun'}
        $dirty=@(git status --porcelain);if ($LASTEXITCODE -ne 0 -or $dirty.Count -ne 0) {throw 'GPU capture requires a clean committed working tree'}
    }
    if ($RunCpu -or $RunGpu) {
        New-Item -ItemType Directory -Force -Path $EvidenceDirectory | Out-Null
        cargo build --release --locked -p desktop-demo --features qualification --bin streamed-residency-qualification
        if ($LASTEXITCODE -ne 0) {throw 'Qualification build failed'}
        foreach ($mode in $cpuModes) {
            & ./target/release/streamed-residency-qualification.exe $mode (Join-Path $EvidenceDirectory "$mode.jsonl")
            if ($LASTEXITCODE -ne 0) {throw "$mode failed"}
        }
        if ($RunGpu) {
            foreach ($mode in @('matched-8','matched-16','raster','brickmap')) {
                & ./target/release/streamed-residency-qualification.exe $mode (Join-Path $EvidenceDirectory "residency-$mode.jsonl") --allow-gpu 2> (Join-Path $EvidenceDirectory "residency-$mode.stderr.log")
                if ($LASTEXITCODE -ne 0) {throw "GPU $mode failed"}
            }
            Write-Context 'gpu'
        } else {Write-Context 'cpu'}
    }
    $context=Get-Content -Raw -LiteralPath (Join-Path $EvidenceDirectory residency-context.json) | ConvertFrom-Json
    if ($context.source_revision -notmatch '^[a-f0-9]{40}$' -or @($context.toolchain).Count -lt 3 -or -not $context.cargo -or $context.executable_sha256 -notmatch '^[A-F0-9]{64}$') {throw 'Missing source/toolchain/executable provenance'}
    $resolvedRevision=git rev-parse --verify "$($context.source_revision)^{commit}" 2>$null
    if ($LASTEXITCODE -ne 0 -or $resolvedRevision -cne $context.source_revision) {throw 'Invalid source revision'}
    if ($context.source_dirty -isnot [bool]) {throw 'Missing source cleanliness provenance'}
    $hashes=@($context.source_sha256)
    if (-not (Same @($hashes.path | Sort-Object) @($sourcePaths | Sort-Object))) {throw 'Missing or duplicated source hashes'}
    foreach ($hash in $hashes) {
        if ($hash.sha256 -cne (Hash $hash.path)) {throw "Changed source hash $($hash.path)"}
        if (-not $context.source_dirty -and $hash.sha256 -cne (Commit-Hash $context.source_revision $hash.path)) {throw "Source hash does not belong to reported commit: $($hash.path)"}
    }
    if ($context.source_dirty -and $context.source_revision -cne (git rev-parse HEAD)) {throw 'Dirty capture does not identify its base revision'}
    $frozen=Frozen
    foreach ($name in $frozen.Keys) {
        if ($context.frozen_sha256.$name -cne $frozen[$name]) {throw "Changed frozen $name hash"}
        if ($name -ne 'edit_script' -and $frozen[$name] -cne $contract.$name) {throw "Frozen $name definitions changed"}
    }
    $cpu=@{}
    foreach ($mode in $cpuModes) {
        $rows=Read $mode;$cpu[$mode]=$rows;$modeContext=One $rows 'context'
        True $modeContext.cpu_only 'CPU mode created graphics work';True $modeContext.production 'CPU mode used prototype APIs'
        if ($modeContext.gpu_dispatched -isnot [bool] -or $modeContext.gpu_dispatched) {throw 'CPU mode dispatched graphics'}
        $samples=@(Records $rows 'cpu-residency');if ($samples.Count -lt 1) {throw 'Missing CPU samples'}
        foreach ($sample in $samples) {Sample $sample $false}
        $releases=@(Records $rows 'cpu-released');$expected=if ($mode -eq 'cpu-calibration') {3} else {1}
        if ($releases.Count -ne $expected) {throw 'Missing CPU cleanup'}
        foreach ($release in $releases) {Array $release.live 6;Zero $release.live}
    }
    foreach ($phase in @('generated','edited','restored')) {
        $entry=One $cpu['cpu-calibration'] 'fingerprint' $phase;$expected=if ($phase -eq 'edited') {'478ee4a9737e1ad7'} else {'9660d9308d69ede5'}
        if ($entry.fingerprint -cne $expected) {throw 'Frozen fingerprint changed'}
    }
    $small=One $cpu['cpu-matched-8'] 'cpu-residency' 'cpu-matched-8';$large=One $cpu['cpu-matched-16'] 'cpu-residency' 'cpu-matched-16'
    if (-not (Same $small.cpu_live[1..4] $large.cpu_live[1..4]) -or $small.cpu_live[5] -ge $large.cpu_live[5] -or $small.cpu_live[6] -ne $large.cpu_live[6]) {throw 'Matched residency/metadata/history separation failed'}
    $lifecycle=$cpu['cpu-lifecycle'];$result=One $lifecycle 'cpu-lifecycle-result'
    foreach ($field in @('evicted_edit','historical_reads','restored','unrelated_edit_reuse','compacted','nineteen_copy_admission')) {True $result.$field "Missing lifecycle $field"}
    if ($result.repeat_laps -ne 2) {throw 'Missing CPU lap'}
    $compact=One $lifecycle 'compaction';if (-not (Same $compact.edit_script $contract.edit_script)) {throw 'Changed fixed edit script'};Zero @($compact.live_historical_views,$compact.edited_coordinates,$compact.history_entries);True $compact.unchanged_volume_reused 'Unrelated edit reuse missing'
    $historical=One $lifecycle 'cpu-residency' 'historical-queries';if ($historical.live_historical_views -ne 2 -or $historical.peak_historical_views -ne 2) {throw 'Missing measured historical views'}
    $overlap=One $lifecycle 'cpu-residency' 'disjoint-query-overlap';if ($overlap.copies -ne 19 -or $overlap.query_copies -ne 1) {throw 'Missing nineteen-copy overlap'}
    $first=One $lifecycle 'cpu-residency' 'lap-0-settled';$second=One $lifecycle 'cpu-residency' 'lap-1-settled'
    if ($first.copies -ne 9 -or $second.copies -ne 9 -or -not (Same $first.cpu_live[1..6] $second.cpu_live[1..6]) -or -not (Same $first.cpu_allocations[1..6] $second.cpu_allocations[1..6])) {throw 'CPU allocation plateau differs'}
    if ($CpuOnly -or ($RunCpu -and -not $RunGpu)) {
        @{verdict='CPU PASS';caps=$caps;source_revision=$context.source_revision} | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $EvidenceDirectory cpu-summary.json)
        Write-Output 'CPU production residency, lifecycle and fixed caps verified.';return
    }
    if ($context.capture_kind -ne 'gpu' -and -not $DryRun) {throw 'Missing recorded GPU provenance'}
    if ($context.source_dirty -and -not $DryRun) {throw 'Dirty GPU capture source'}
    if (-not $context.gpu_device.name) {throw 'Missing device provenance'}
    $summaries=@{}
    foreach ($mode in @('matched-8','matched-16','raster','brickmap')) {
        $rows=Read "residency-$mode";$device=One $rows 'device';True $device.validation_enabled 'Vulkan validation disabled'
        if ($device.name -cne $context.gpu_device.name -or $device.driver_version -ne $context.gpu_device.driver_version -or $device.api_version -ne $context.gpu_device.api_version) {throw 'Device provenance differs'}
        $route=One $rows 'context';True $route.production 'GPU mode used prototype APIs';True $route.typed_dense_rejection 'Missing typed Dense rejection'
        if ($route.mode -cne $mode) {throw 'Wrong route start/mode'}
        if (-not (Same $route.crossings $crossingTimes) -or -not (Same $route.projection @(1920,1080,60.0,0.1,34.0))) {throw 'Frozen analytic crossing clock or projection changed'}
        $samples=@(Records $rows 'residency');if ($samples.Count -lt 1) {throw 'Missing GPU samples'}
        foreach ($sample in $samples) {Sample $sample $true}
        $validation=One $rows 'validation';Zero @($validation.errors,$validation.warnings)
        $release=One $rows 'released';foreach ($field in @('gpu_live','gpu_allocations','gpu_objects')) {Array $release.$field 3;Zero $release.$field};Zero @($release.workers)
        $cpuRelease=One $rows 'cpu-released';Array $cpuRelease.live 6;Zero $cpuRelease.live
        $probes=@(Records $rows 'probes');if ($probes.Count -lt 1) {throw 'Missing rendered/oracle probes'}
        foreach ($probe in $probes) {
            True $probe.matching 'Rendered/oracle mismatch';True $probe.coverage_contains_view 'Probe view outside installed coverage';True $probe.coverage_contains_ray_domain 'Probe ray domain outside installed coverage'
            if ($probe.count -ne 4) {throw 'Incomplete frozen probe batch'}
        }
        if ($mode -like 'matched-*') {continue}
        if ($route.route_start -cne $mode) {throw 'Missing route start'}
        $installed=@(Records $rows 'installed');$crossings=@(Records $rows 'crossing');$laps=@(Records $rows 'route-result')
        if ($installed.Count -ne 16 -or $crossings.Count -ne 16 -or $laps.Count -ne 2) {throw 'Missing route lap or fence-safe installation'}
        foreach ($lapIndex in 0..1) {
            $lap=@($laps | Where-Object lap -eq $lapIndex)
            if ($lap.Count -ne 1 -or $lap[0].crossings -ne 8 -or $lap[0].installed_crossings -ne 8 -or $lap[0].rendered_probes -ne 32) {throw 'Missing or incomplete lap'}
            Number $lap[0].coverage_stalls 0 'Route coverage stall';$directions=@()
            $strategy=if ($mode -eq 'raster') {'voxel-nexus.raster'} else {'voxel-nexus.compute-ray'}
            foreach ($index in 0..7) {
                $crossing=@($crossings | Where-Object {$_.lap -eq $lapIndex -and $_.index -eq $index});$install=@($installed | Where-Object {$_.lap -eq $lapIndex -and $_.index -eq $index})
                if ($crossing.Count -ne 1 -or $install.Count -ne 1) {throw 'Uninstalled or duplicated crossing'}
                $crossing=$crossing[0];$install=$install[0];$expected=$crossingTimes[$index]
                if ([math]::Abs($crossing.origin_seconds-$expected) -gt 0.000000001 -or $crossing.unresolved_origin_seconds -ne $crossing.origin_seconds) {throw 'Analytic crossing clock restarted or changed'}
                True $install.fence_safe 'Installation was not fence safe';True $install.selection_matches 'Selection mismatch';Number $install.crossing_seconds 2.5 'Crossing exceeded 2.5 seconds; reopen shaping'
                $from=$strategy
                if ($index -eq 2 -or $index -eq 5) {$strategy=if ($strategy -eq 'voxel-nexus.raster') {'voxel-nexus.compute-ray'} else {'voxel-nexus.raster'}}
                if ($install.from -cne $from -or $install.to -cne $strategy) {throw 'Wrong switch direction or route start'}
                if ([math]::Abs($install.boundary_seconds-$crossing.origin_seconds-$install.crossing_seconds) -gt 0.000001) {throw 'Crossing latency did not end at actual boundary'}
                if ($index -eq 2 -or $index -eq 5) {
                    True $crossing.switch_requested 'Missing switch request';Number $install.switch_seconds 2.5 'Switch exceeded 2.5 seconds; reopen shaping';Number $crossing.switch_requested_seconds $install.boundary_seconds 'Invalid switch request clock'
                    $origin=[math]::Max($crossing.origin_seconds,$crossing.switch_requested_seconds)
                    if ([math]::Abs($install.boundary_seconds-$origin-$install.switch_seconds) -gt 0.000001) {throw 'Handoff latency did not end at actual boundary'}
                    $directions+="$($install.from)>$($install.to)"
                } elseif ($null -ne $install.switch_seconds -or $crossing.switch_requested) {throw 'Unexpected switch crossing'}
                if (@($probes | Where-Object {$_.lap -eq $lapIndex -and $_.index -eq $index}).Count -ne 1) {throw 'Missing crossing probe comparison'}
            }
            if (-not (Same @($directions | Sort-Object) @('voxel-nexus.compute-ray>voxel-nexus.raster','voxel-nexus.raster>voxel-nexus.compute-ray'))) {throw 'Missing switch direction in lap'}
        }
        foreach ($phase in @('revision-replacement','disjoint-query-overlap','failed-candidate-cleaned','stress-settled')) {One $rows 'residency' $phase | Out-Null}
        $revision=One $rows 'revision-replacement';$replacement=One $rows 'residency' 'revision-replacement'
        if ($revision.predecessor -cne '1' -or $revision.successor -cne '2' -or $replacement.required_revision -cne $revision.successor -or $replacement.visible_revision -cne $revision.successor) {throw 'Stale revision replacement'}
        True $replacement.fully_converged 'Revision replacement did not converge'
        if ($replacement.live_historical_views -ne 2 -or $replacement.peak_historical_views -ne 2) {throw 'Missing GPU measured historical views'}
        if (@($probes | Where-Object {$null -eq $_.lap -and $null -eq $_.index -and $_.revision -eq 2}).Count -lt 1) {throw 'Missing revision replacement probe'}
        $admission=One $rows 'admission';if ($admission.copies -ne 19) {throw 'Missing GPU nineteen-copy admission'};True $admission.second_query_rejected 'Admitted second query copy'
        $recovery=One $rows 'failure-recovery' 'raster-upload';True $recovery.observed_upload_failure 'Missing upload failure';True $recovery.presenting_preserved 'Upload failure changed presentation'
        $churn=One $rows 'boundary-churn'
        if ($churn.crossings -ne 12 -or $churn.installations -ne 12 -or $churn.hysteresis -isnot [bool] -or $churn.hysteresis) {throw 'Missing boundary churn or changed policy'};Number $churn.coverage_stalls 0 'Boundary churn coverage stall'
        $compact=One $rows 'compaction';if (-not (Same $compact.edit_script $contract.edit_script)) {throw 'Changed route edit script'};Zero @($compact.live_historical_views,$compact.edited_coordinates,$compact.history_entries);True $compact.unchanged_volume_reused 'Missing route unrelated-edit reuse'
        $settled=@(Records $rows 'residency' | Where-Object phase -eq 'lap-settled')
        if ($settled.Count -ne 2 -or $settled[0].copies -ne 9 -or $settled[1].copies -ne 9) {throw 'Missing settled lap plateaus'}
        foreach ($field in @('cpu_live','cpu_allocations','gpu_live','gpu_allocations','gpu_objects')) {
            $first=$settled[0].$field;$second=$settled[1].$field;if ($field -like 'cpu_*') {$first=$first[1..6];$second=$second[1..6]}
            if (-not (Same $first $second)) {throw "Lap allocation plateau differs: $field"}
        }
        $summaries[$mode]=@{laps=2;installations=16;switches=4;probes=64;maximum_crossing_seconds=($installed.crossing_seconds | Measure-Object -Maximum).Maximum}
    }
    @{verdict=$(if ($DryRun) {'DRY RUN PASS'} else {'PASS'});caps=$caps;source_revision=$context.source_revision;routes=$summaries} | ConvertTo-Json -Depth 7 | Set-Content (Join-Path $EvidenceDirectory residency-summary.json)
    Write-Output 'Production streamed qualification verified within the fixed caps.'
} finally {Pop-Location}
