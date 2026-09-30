param([string]$EvidenceDirectory='artifacts/verifier-negative-tests')
$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
$workspaceDirectory=Split-Path -Parent $PSScriptRoot
Push-Location $workspaceDirectory
try {
    New-Item -ItemType Directory -Force -Path $EvidenceDirectory | Out-Null
    function Hash([string]$path) {[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes([IO.File]::ReadAllText((Join-Path $workspaceDirectory $path)).Replace("`r`n","`n"))))}
    function Write-Rows([string]$name,$rows) { @($rows | ForEach-Object {ConvertTo-Json -InputObject $_ -Compress -Depth 10}) | Set-Content (Join-Path $EvidenceDirectory "$name.jsonl") }
    function Cpu([string]$phase,[int]$metadata=256) { @{kind='cpu-residency';phase=$phase;copies=9;peak_copies=19;query_copies=0;generating=0;metadata_entries=$metadata;edited_coordinates=0;history_entries=0;cpu_live=@(1,1,0,1,1,$metadata,1);cpu_peak=@(1,1,0,1,1,$metadata,1);cpu_allocations=@(1,1,0,1,1,1,1)} }
    function Sample([string]$phase) { @{kind='residency';phase=$phase;copies=9;peak_copies=19;query_copies=0;metadata_entries=256;edited_coordinates=0;history_entries=0;cpu_live=@(1,1,0,1,1,256,1);cpu_peak=@(1,1,0,1,1,256,1);cpu_allocations=@(1,1,0,1,1,1,1);gpu_live=@(1,1,1);gpu_peak=@(1,1,1);gpu_allocations=@(1,1,1);gpu_allocations_peak=@(1,1,1);gpu_objects=@(1,1,1);gpu_objects_peak=@(1,1,1);audit_entries=1;owners=1;workers=0;raster_workers=0;brickmap_workers=0;representation_copies=9;coverage_stalls=0;presentation_images=3} }
    $contract=Get-Content -Raw scripts/streamed-residency-contract.json | ConvertFrom-Json
    $compact=@{kind='compaction';edit_script=$contract.edit_script;live_historical_views=0;edited_coordinates=0;history_entries=0;unchanged_volume_reused=$true}
    $calibration=@()
    foreach ($phase in @('generated','edited','restored')) {$calibration+=@((Cpu $phase),@{kind='fingerprint';phase=$phase;fingerprint=$(if ($phase -eq 'edited') {'478ee4a9737e1ad7'} else {'9660d9308d69ede5'})},@{kind='cpu-released';live=@(0,0,0,0,0,0)})}
    $calibration+=@{kind='context';cpu_only=$true;production=$true;gpu_dispatched=$false};Write-Rows 'cpu-calibration' $calibration
    foreach ($side in @(8,16)) {Write-Rows "cpu-matched-$side" @((Cpu "cpu-matched-$side" ($side*$side)),@{kind='cpu-released';live=@(0,0,0,0,0,0)},@{kind='context';cpu_only=$true;production=$true;gpu_dispatched=$false})}
    $overlap=Cpu 'disjoint-query-overlap';$overlap.copies=19;$overlap.query_copies=1
    Write-Rows 'cpu-lifecycle' @($overlap,(Cpu 'lap-0-settled'),(Cpu 'lap-1-settled'),$compact,@{kind='cpu-lifecycle-result';evicted_edit=$true;historical_reads=$true;restored=$true;unrelated_edit_reuse=$true;compacted=$true;nineteen_copy_admission=$true;repeat_laps=2},@{kind='cpu-released';live=@(0,0,0,0,0,0)},@{kind='context';cpu_only=$true;production=$true;gpu_dispatched=$false})
    $device=@{kind='device';name='Verifier fixture, no graphics dispatched';driver_version=1;api_version=1;validation_enabled=$true}
    $crossingTimes=@(8.0,24.0,43.31370849898476,65.94112549695429,88.5685424949238,111.19595949289332,130.50966799187808,146.50966799187808)
    foreach ($mode in @('matched-8','matched-16','raster','brickmap')) {
        $rows=@($device,@{kind='context';mode=$mode;route_start=$mode;production=$true;typed_dense_rejection=$true;crossings=$crossingTimes},(Sample 'initial'))
        if ($mode -like 'matched-*') {$rows+=@{kind='probes';lap=$null;index=$null;count=4;matching=$true;coverage_contains_view=$true;coverage_contains_ray_domain=$true}}
        else {
            foreach ($phase in @('revision-replacement','disjoint-query-overlap','failed-candidate-cleaned','stress-settled')) {$rows+=Sample $phase}
            $rows+=@(@{kind='admission';copies=19;second_query_rejected=$true},@{kind='failure-recovery';phase='raster-upload';observed_upload_failure=$true;presenting_preserved=$true},@{kind='boundary-churn';crossings=12;installations=12;hysteresis=$false;coverage_stalls=0},$compact)
            foreach ($lap in 0..1) {
                $strategy=if ($mode -eq 'raster') {'voxel-nexus.raster'} else {'voxel-nexus.compute-ray'}
                foreach ($index in 0..7) {
                    $origin=$crossingTimes[$index];$switch=$index -eq 2 -or $index -eq 5;$from=$strategy
                    if ($switch) {$strategy=if ($strategy -eq 'voxel-nexus.raster') {'voxel-nexus.compute-ray'} else {'voxel-nexus.raster'}}
                    $rows+=@(@{kind='crossing';lap=$lap;index=$index;origin_seconds=$origin;unresolved_origin_seconds=$origin;switch_requested=$switch;switch_requested_seconds=$(if ($switch) {$origin+0.1} else {$null})},@{kind='installed';lap=$lap;index=$index;crossing_seconds=0.25;switch_seconds=$(if ($switch) {0.15} else {$null});boundary_seconds=$origin+0.25;fence_safe=$true;selection_matches=$true;from=$from;to=$strategy},@{kind='probes';lap=$lap;index=$index;count=4;matching=$true;coverage_contains_view=$true;coverage_contains_ray_domain=$true})
                }
                $rows+=@((Sample 'lap-settled'),@{kind='route-result';lap=$lap;crossings=8;installed_crossings=8;coverage_stalls=0;rendered_probes=32})
            }
        }
        $rows+=@(@{kind='validation';warnings=0;errors=0},@{kind='cpu-released';live=@(0,0,0,0,0,0)},@{kind='released';gpu_live=@(0,0,0);gpu_allocations=@(0,0,0);gpu_objects=@(0,0,0);workers=0})
        Write-Rows "residency-$mode" $rows
    }
    $probes=[IO.File]::ReadAllText((Join-Path $workspaceDirectory 'apps/desktop-demo/src/streamed_residency_probes.rs')).Replace("`r`n","`n")
    $start=$probes.IndexOf('        for pixel in');$end=$probes.IndexOf('            let observation',$start)
    $frozen=@{fixture=$contract.fixture;recipe=$contract.recipe;route=$contract.route;crossing_clock=$contract.crossing_clock;coverage=$contract.coverage;probes=[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($probes.Substring($start,$end-$start))));edit_script=(Hash 'scripts/streamed-residency-contract.json')}
    $sourcePaths=@(rg --files apps crates scripts -g '*.rs' -g '*.toml' -g '*.comp' -g '*.vert' -g '*.frag' -g '*.ps1' -g 'streamed-residency-contract.json' | ForEach-Object {$_.Replace('\','/')} | Sort-Object)+@('Cargo.toml','Cargo.lock')
    @{source_revision=(git rev-parse HEAD);source_dirty=$true;capture_kind='test-fixture';toolchain=@('fixture compiler','fixture host','fixture release');cargo='fixture';gpu_device=$device;executable_sha256=('0'*64);frozen_sha256=$frozen;source_sha256=@($sourcePaths | ForEach-Object {@{path=$_;sha256=(Hash $_)}})} | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $EvidenceDirectory residency-context.json)
    $saved=@{};Get-ChildItem -LiteralPath $EvidenceDirectory -File | Where-Object Extension -in '.json','.jsonl' | ForEach-Object {$saved[$_.Name]=[IO.File]::ReadAllText($_.FullName)}
    function Verify { $output=& pwsh -NoProfile -File scripts/verify-streamed-residency.ps1 -EvidenceDirectory $EvidenceDirectory -DryRun 2>&1;@{code=$LASTEXITCODE;output=($output -join "`n")} }
    $positive=Verify;if ($positive.code -ne 0) {throw "Positive verifier fixture failed: $($positive.output)"}
    function Reject([string]$name,[scriptblock]$change,[string]$file='residency-raster.jsonl') {
        foreach ($entry in $saved.GetEnumerator()) {[IO.File]::WriteAllText((Join-Path $EvidenceDirectory $entry.Key),$entry.Value)}
        $path=Join-Path $EvidenceDirectory $file
        if ($file.EndsWith('.jsonl')) {$rows=@(Get-Content $path | ForEach-Object {$_ | ConvertFrom-Json -AsHashtable});$rows=& $change $rows;Write-Rows ($file.Replace('.jsonl','')) $rows}
        else {$context=Get-Content -Raw $path | ConvertFrom-Json -AsHashtable;& $change $context;$context | ConvertTo-Json -Depth 10 | Set-Content $path}
        $result=Verify;if ($result.code -eq 0) {throw "Verifier accepted negative fixture: $name"};Write-Output "Rejected $name"
    }
    Reject 'missing route start' {param($rows) @($rows | Where-Object kind -eq 'context')[0].mode='brickmap';$rows}
    Reject 'missing lap' {param($rows) $rows | Where-Object {-not ($_.kind -eq 'route-result' -and $_.lap -eq 1)}}
    Reject 'fewer than sixteen installs' {param($rows) $rows | Where-Object {-not ($_.kind -eq 'installed' -and $_.index -eq 7 -and $_.lap -eq 1)}}
    Reject 'missing switch direction' {param($rows) @($rows | Where-Object {$_.kind -eq 'installed' -and $_.index -eq 5 -and $_.lap -eq 0})[0].to='voxel-nexus.compute-ray';$rows}
    Reject 'uninstalled crossing' {param($rows) @($rows | Where-Object kind -eq 'installed')[0].index=99;$rows}
    Reject 'coverage stall' {param($rows) @($rows | Where-Object kind -eq 'route-result')[0].coverage_stalls=1;$rows}
    Reject 'slow crossing' {param($rows) @($rows | Where-Object kind -eq 'installed')[0].crossing_seconds=2.501;$rows}
    Reject 'slow switch' {param($rows) @($rows | Where-Object {$_.kind -eq 'installed' -and $_.index -eq 2})[0].switch_seconds=2.501;$rows}
    Reject 'unsafe installation' {param($rows) @($rows | Where-Object kind -eq 'installed')[0].fence_safe=$false;$rows}
    Reject 'supersession restarted clock' {param($rows) @($rows | Where-Object kind -eq 'crossing')[0].unresolved_origin_seconds=9.0;$rows}
    Reject 'wrong handoff boundary clock' {param($rows) @($rows | Where-Object {$_.kind -eq 'installed' -and $_.index -eq 2})[0].switch_seconds=0.01;$rows}
    Reject 'validation message' {param($rows) @($rows | Where-Object kind -eq 'validation')[0].warnings=1;$rows}
    Reject 'CPU cleanup debt' {param($rows) @($rows | Where-Object kind -eq 'cpu-released')[0].live[0]=1;$rows}
    Reject 'GPU cleanup debt' {param($rows) @($rows | Where-Object kind -eq 'released')[0].gpu_live[0]=1;$rows}
    Reject 'worker cleanup debt' {param($rows) @($rows | Where-Object kind -eq 'released')[0].workers=1;$rows}
    Reject 'twentieth copy' {param($rows) @($rows | Where-Object kind -eq 'residency')[0].peak_copies=20;$rows}
    Reject 'CPU materialization cap' {param($rows) @($rows | Where-Object kind -eq 'residency')[0].cpu_peak[1]=10300369;$rows}
    Reject 'Raster CPU cap' {param($rows) @($rows | Where-Object kind -eq 'residency')[0].cpu_peak[3]=58242849;$rows}
    Reject 'Brickmap CPU cap' {param($rows) @($rows | Where-Object kind -eq 'residency')[0].cpu_peak[4]=8914537;$rows}
    Reject 'Raster GPU cap' {param($rows) @($rows | Where-Object kind -eq 'residency')[0].gpu_peak[1]=980641;$rows}
    Reject 'Brickmap GPU cap' {param($rows) @($rows | Where-Object kind -eq 'residency')[0].gpu_peak[2]=4760641;$rows}
    Reject 'Control heap bound' {param($rows) @($rows | Where-Object kind -eq 'residency')[0].cpu_peak[0]=16777217;$rows}
    Reject 'metadata heap bound' {param($rows) @($rows | Where-Object kind -eq 'residency')[0].cpu_peak[5]=262145;$rows}
    Reject 'history heap bound' {param($rows) @($rows | Where-Object kind -eq 'residency')[0].cpu_peak[6]=16385;$rows}
    Reject 'fixed GPU memory bound' {param($rows) @($rows | Where-Object kind -eq 'residency')[0].gpu_peak[0]=26543905;$rows}
    Reject 'fixed GPU allocation count' {param($rows) @($rows | Where-Object kind -eq 'residency')[0].gpu_allocations_peak[0]=10;$rows}
    Reject 'renderer owner count' {param($rows) @($rows | Where-Object kind -eq 'residency')[0].owners=4;$rows}
    Reject 'image count' {param($rows) @($rows | Where-Object kind -eq 'residency')[0].gpu_objects_peak[1]=4;$rows}
    Reject 'framebuffer count' {param($rows) @($rows | Where-Object kind -eq 'residency')[0].gpu_objects_peak[2]=10;$rows}
    Reject 'missing probes' {param($rows) $rows | Where-Object {-not ($_.kind -eq 'probes' -and $_.lap -eq 0 -and $_.index -eq 0)}}
    Reject 'uncovered probe view' {param($rows) @($rows | Where-Object kind -eq 'probes')[0].coverage_contains_view=$false;$rows}
    Reject 'uncovered probe ray domain' {param($rows) @($rows | Where-Object kind -eq 'probes')[0].coverage_contains_ray_domain=$false;$rows}
    Reject 'probe mismatch' {param($rows) @($rows | Where-Object kind -eq 'probes')[0].matching=$false;$rows}
    Reject 'missing revision replacement' {param($rows) $rows | Where-Object {-not ($_.kind -eq 'residency' -and $_.phase -eq 'revision-replacement')}}
    Reject 'missing upload recovery' {param($rows) $rows | Where-Object kind -ne 'failure-recovery'}
    Reject 'upload changed presentation' {param($rows) @($rows | Where-Object kind -eq 'failure-recovery')[0].presenting_preserved=$false;$rows}
    Reject 'missing boundary churn' {param($rows) $rows | Where-Object kind -ne 'boundary-churn'}
    Reject 'different lap plateau' {param($rows) @($rows | Where-Object {$_.kind -eq 'residency' -and $_.phase -eq 'lap-settled'})[1].gpu_live[1]=2;$rows}
    Reject 'missing provenance' {param($context) $context.Remove('source_revision') | Out-Null} 'residency-context.json'
    Reject 'changed source hash' {param($context) $context.source_sha256[0].sha256='0'*64} 'residency-context.json'
    Reject 'changed frozen recipe hash' {param($context) $context.frozen_sha256.recipe='0'*64} 'residency-context.json'
    Reject 'wrong device provenance' {param($rows) @($rows | Where-Object kind -eq 'device')[0].name='another device';$rows}
    Reject 'missing lifecycle compaction' {param($rows) @($rows | Where-Object kind -eq 'cpu-lifecycle-result')[0].compacted=$false;$rows} 'cpu-lifecycle.jsonl'
    foreach ($entry in $saved.GetEnumerator()) {[IO.File]::WriteAllText((Join-Path $EvidenceDirectory $entry.Key),$entry.Value)}
    Write-Output 'Verifier positive fixture and every negative fixture passed without graphics work.'
} finally {Pop-Location}
