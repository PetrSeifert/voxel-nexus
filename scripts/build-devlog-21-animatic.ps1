param(
    [string]$OutputDirectory,
    [string]$OutputFileName = 'voxel-nexus-devlog-02-animatic.mp4',
    [int]$VoiceRate = -2,
    [switch]$BurnSubtitles,
    [ValidateRange(0, 2147483647)]
    [int]$MaximumNarrationSegments = 0
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repositoryRoot = Split-Path $PSScriptRoot -Parent
if ([string]::IsNullOrWhiteSpace($OutputDirectory)) {
    $OutputDirectory = Join-Path $repositoryRoot 'artifacts/devlog-21-animatic'
}
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
$narrationDirectory = Join-Path $OutputDirectory 'narration'
$visualDirectory = Join-Path $OutputDirectory 'visuals'
$scriptPath = Join-Path $repositoryRoot 'docs/devlogs/21-dense-raster-voxel-scene.md'
$milestoneEvidenceDirectory = Join-Path $repositoryRoot 'docs/evidence/milestone-completion/development-machine'
$representativeFrameDirectory = Join-Path $milestoneEvidenceDirectory 'representative-frames'
$lifecycleDirectory = Join-Path $milestoneEvidenceDirectory 'lifecycle'
$outputVideoPath = Join-Path $OutputDirectory $OutputFileName

foreach ($command in 'ffmpeg', 'ffprobe') {
    if (-not (Get-Command $command -ErrorAction SilentlyContinue)) {
        throw "$command is required but was not found on PATH."
    }
}

Add-Type -AssemblyName System.Speech
Add-Type -AssemblyName System.Drawing

New-Item -ItemType Directory -Force $OutputDirectory, $narrationDirectory, $visualDirectory | Out-Null

function Invoke-MediaCommand {
    param(
        [Parameter(Mandatory)][string]$Command,
        [Parameter(Mandatory)][string[]]$Arguments
    )

    & $Command @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$Command failed with exit code $LASTEXITCODE."
    }
}

function Get-MediaDuration {
    param([Parameter(Mandatory)][string]$Path)

    $durationText = & ffprobe -v error -show_entries format=duration -of default=noprint_wrappers=1:nokey=1 $Path
    if ($LASTEXITCODE -ne 0) {
        throw "ffprobe could not read $Path."
    }
    return [double]::Parse($durationText.Trim(), [Globalization.CultureInfo]::InvariantCulture)
}

function ConvertTo-SrtTimestamp {
    param([Parameter(Mandatory)][double]$Seconds)

    $totalMilliseconds = [long][Math]::Round($Seconds * 1000.0)
    $hours = [Math]::Floor($totalMilliseconds / 3600000)
    $remainder = $totalMilliseconds % 3600000
    $minutes = [Math]::Floor($remainder / 60000)
    $remainder %= 60000
    $secondsPart = [Math]::Floor($remainder / 1000)
    $milliseconds = $remainder % 1000
    return '{0:00}:{1:00}:{2:00},{3:000}' -f $hours, $minutes, $secondsPart, $milliseconds
}

function Split-CaptionText {
    param(
        [Parameter(Mandatory)][string]$Text,
        [int]$MaximumCharacters = 68
    )

    $words = @($Text -split '\s+')
    $chunkCount = [Math]::Max(1, [Math]::Ceiling($Text.Length / $MaximumCharacters))
    $wordsPerChunk = [Math]::Ceiling($words.Count / $chunkCount)
    $chunks = [Collections.Generic.List[string]]::new()
    for ($start = 0; $start -lt $words.Count; $start += $wordsPerChunk) {
        $end = [Math]::Min($start + $wordsPerChunk - 1, $words.Count - 1)
        $chunks.Add(($words[$start..$end] -join ' '))
    }
    return $chunks
}

function ConvertTo-ConcatPath {
    param([Parameter(Mandatory)][string]$Path)

    return ([IO.Path]::GetFullPath($Path) -replace '\\', '/') -replace "'", "'\\''"
}

function Get-NarrationSegments {
    param([Parameter(Mandatory)][string]$Markdown)

    $segments = [Collections.Generic.List[object]]::new()
    $beatMatches = [regex]::Matches($Markdown, '(?ms)^## Beat (?<beat>\d+).*?\r?\n(?<body>.*?)(?=^## Beat |^## Editing notes)')
    foreach ($beatMatch in $beatMatches) {
        $beat = [int]$beatMatch.Groups['beat'].Value
        $voiceMatches = [regex]::Matches(
            $beatMatch.Groups['body'].Value,
            '(?ms)^\*\*VO(?: \(continued\))?:\*\*\r?\n(?<text>.*?)(?=^\*\*\[|\z)'
        )
        $beatSentences = [Collections.Generic.List[object]]::new()
        foreach ($voiceMatch in $voiceMatches) {
            $paragraphs = [regex]::Split($voiceMatch.Groups['text'].Value.Trim(), '\r?\n\s*\r?\n')
            foreach ($paragraph in $paragraphs) {
                if ([string]::IsNullOrWhiteSpace($paragraph)) {
                    continue
                }
                $sentences = [regex]::Split($paragraph.Trim(), '(?<=[.!?])\s+(?=[A-Z"''])')
                for ($sentenceIndex = 0; $sentenceIndex -lt $sentences.Count; $sentenceIndex++) {
                    $text = ($sentences[$sentenceIndex] -replace '\s+', ' ').Trim()
                    if ([string]::IsNullOrWhiteSpace($text)) {
                        continue
                    }
                    $beatSentences.Add([pscustomobject]@{
                        Beat = $beat
                        Text = $text
                        ParagraphEnd = $sentenceIndex -eq ($sentences.Count - 1)
                    })
                }
            }
        }
        for ($index = 0; $index -lt $beatSentences.Count; $index++) {
            $sentence = $beatSentences[$index]
            $segments.Add([pscustomobject]@{
                Beat = $sentence.Beat
                Text = $sentence.Text
                ParagraphEnd = $sentence.ParagraphEnd
                BeatEnd = $index -eq ($beatSentences.Count - 1)
            })
        }
    }
    return $segments
}

$backgroundColor = [Drawing.ColorTranslator]::FromHtml('#20283B')
$panelColor = [Drawing.ColorTranslator]::FromHtml('#303A54')
$panelMutedColor = [Drawing.ColorTranslator]::FromHtml('#283047')
$pinkColor = [Drawing.ColorTranslator]::FromHtml('#F96C89')
$whiteColor = [Drawing.ColorTranslator]::FromHtml('#F5F7FF')
$mutedColor = [Drawing.ColorTranslator]::FromHtml('#AEB7CF')
$greenColor = [Drawing.ColorTranslator]::FromHtml('#76D39B')
$orangeColor = [Drawing.ColorTranslator]::FromHtml('#F18458')
$blueColor = [Drawing.ColorTranslator]::FromHtml('#4D8BDE')

function New-Canvas {
    $bitmap = [Drawing.Bitmap]::new(1920, 1080)
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    $graphics.SmoothingMode = [Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $graphics.InterpolationMode = [Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $graphics.TextRenderingHint = [Drawing.Text.TextRenderingHint]::AntiAliasGridFit
    $graphics.Clear($backgroundColor)
    return [pscustomobject]@{ Bitmap = $bitmap; Graphics = $graphics }
}

function Save-Canvas {
    param(
        [Parameter(Mandatory)]$Canvas,
        [Parameter(Mandatory)][string]$Path
    )

    $Canvas.Bitmap.Save($Path, [Drawing.Imaging.ImageFormat]::Png)
    $Canvas.Graphics.Dispose()
    $Canvas.Bitmap.Dispose()
}

function Draw-TextBlock {
    param(
        [Parameter(Mandatory)][Drawing.Graphics]$Graphics,
        [Parameter(Mandatory)][string]$Text,
        [Parameter(Mandatory)][Drawing.Font]$Font,
        [Parameter(Mandatory)][Drawing.Color]$Color,
        [Parameter(Mandatory)][Drawing.RectangleF]$Rectangle,
        [Drawing.StringAlignment]$Alignment = [Drawing.StringAlignment]::Near,
        [Drawing.StringAlignment]$LineAlignment = [Drawing.StringAlignment]::Near
    )

    $brush = [Drawing.SolidBrush]::new($Color)
    $format = [Drawing.StringFormat]::new()
    $format.Alignment = $Alignment
    $format.LineAlignment = $LineAlignment
    $format.Trimming = [Drawing.StringTrimming]::Word
    $Graphics.DrawString($Text, $Font, $brush, $Rectangle, $format)
    $format.Dispose()
    $brush.Dispose()
}

function Draw-Arrow {
    param(
        [Parameter(Mandatory)][Drawing.Graphics]$Graphics,
        [Parameter(Mandatory)][float]$StartX,
        [Parameter(Mandatory)][float]$StartY,
        [Parameter(Mandatory)][float]$EndX,
        [Parameter(Mandatory)][float]$EndY
    )

    $pen = [Drawing.Pen]::new($mutedColor, 6)
    $pen.EndCap = [Drawing.Drawing2D.LineCap]::ArrowAnchor
    $Graphics.DrawLine($pen, $StartX, $StartY, $EndX, $EndY)
    $pen.Dispose()
}

function New-TitleCard {
    param([Parameter(Mandatory)][string]$Path)

    $canvas = New-Canvas
    $graphics = $canvas.Graphics
    $voxelBrushes = @(
        [Drawing.SolidBrush]::new($orangeColor),
        [Drawing.SolidBrush]::new($greenColor),
        [Drawing.SolidBrush]::new($blueColor)
    )
    for ($column = 0; $column -lt 3; $column++) {
        for ($row = 0; $row -lt (4 - $column); $row++) {
            $graphics.FillRectangle($voxelBrushes[$column], 220 + ($column * 145), 650 - ($row * 115), 125, 95)
        }
    }
    foreach ($brush in $voxelBrushes) { $brush.Dispose() }
    $eyebrowFont = [Drawing.Font]::new('Bahnschrift', 28, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $titleFont = [Drawing.Font]::new('Bahnschrift', 78, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $subtitleFont = [Drawing.Font]::new('Bahnschrift', 38, [Drawing.FontStyle]::Regular, [Drawing.GraphicsUnit]::Pixel)
    $smallFont = [Drawing.Font]::new('Bahnschrift', 24, [Drawing.FontStyle]::Regular, [Drawing.GraphicsUnit]::Pixel)
    Draw-TextBlock $graphics 'VOXEL NEXUS 02' $eyebrowFont $pinkColor ([Drawing.RectangleF]::new(870, 235, 850, 55))
    Draw-TextBlock $graphics "THE FIRST REAL`nVOXEL IMAGE" $titleFont $whiteColor ([Drawing.RectangleF]::new(860, 325, 900, 250))
    Draw-TextBlock $graphics 'Dense scene - raster Render Path - auditable proof' $subtitleFont $mutedColor ([Drawing.RectangleF]::new(865, 640, 850, 120))
    Draw-TextBlock $graphics 'Animatic - Microsoft David Windows voice' $smallFont $mutedColor ([Drawing.RectangleF]::new(868, 820, 800, 50))
    $eyebrowFont.Dispose()
    $titleFont.Dispose()
    $subtitleFont.Dispose()
    $smallFont.Dispose()
    Save-Canvas $canvas $Path
}

function New-EvidenceCard {
    param(
        [Parameter(Mandatory)][string]$SourcePath,
        [Parameter(Mandatory)][string]$Label,
        [Parameter(Mandatory)][string]$Detail,
        [Parameter(Mandatory)][string]$Path
    )

    $canvas = New-Canvas
    $graphics = $canvas.Graphics
    $labelFont = [Drawing.Font]::new('Bahnschrift', 42, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $detailFont = [Drawing.Font]::new('Bahnschrift', 24, [Drawing.FontStyle]::Regular, [Drawing.GraphicsUnit]::Pixel)
    Draw-TextBlock $graphics $Label $labelFont $whiteColor ([Drawing.RectangleF]::new(130, 45, 1660, 65)) ([Drawing.StringAlignment]::Center)
    Draw-TextBlock $graphics $Detail $detailFont $mutedColor ([Drawing.RectangleF]::new(150, 115, 1620, 45)) ([Drawing.StringAlignment]::Center)
    $image = [Drawing.Image]::FromFile($SourcePath)
    $maximumWidth = 1620.0
    $maximumHeight = 810.0
    $scale = [Math]::Min($maximumWidth / $image.Width, $maximumHeight / $image.Height)
    $width = [float]($image.Width * $scale)
    $height = [float]($image.Height * $scale)
    $x = [float]((1920 - $width) / 2)
    $y = [float](190 + (($maximumHeight - $height) / 2))
    $shadowBrush = [Drawing.SolidBrush]::new([Drawing.Color]::FromArgb(90, 0, 0, 0))
    $graphics.FillRectangle($shadowBrush, $x + 16, $y + 16, $width, $height)
    $shadowBrush.Dispose()
    $graphics.DrawImage($image, [Drawing.RectangleF]::new($x, $y, $width, $height))
    $image.Dispose()
    $labelFont.Dispose()
    $detailFont.Dispose()
    Save-Canvas $canvas $Path
}

function New-TriangleCard {
    param([Parameter(Mandatory)][string]$Path)

    $canvas = New-Canvas
    $graphics = $canvas.Graphics
    $labelFont = [Drawing.Font]::new('Bahnschrift', 42, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $detailFont = [Drawing.Font]::new('Bahnschrift', 24, [Drawing.FontStyle]::Regular, [Drawing.GraphicsUnit]::Pixel)
    Draw-TextBlock $graphics 'Where episode one ended' $labelFont $whiteColor ([Drawing.RectangleF]::new(130, 45, 1660, 65)) ([Drawing.StringAlignment]::Center)
    Draw-TextBlock $graphics 'A validation-clean triangle surviving the Windows lifecycle' $detailFont $mutedColor ([Drawing.RectangleF]::new(150, 115, 1620, 45)) ([Drawing.StringAlignment]::Center)
    $panelBrush = [Drawing.SolidBrush]::new($panelColor)
    $graphics.FillRectangle($panelBrush, 660, 190, 600, 810)
    $panelBrush.Dispose()
    $triangleBrush = [Drawing.SolidBrush]::new($pinkColor)
    $points = [Drawing.PointF[]]@(
        [Drawing.PointF]::new(960, 300),
        [Drawing.PointF]::new(750, 860),
        [Drawing.PointF]::new(1170, 860)
    )
    $graphics.FillPolygon($triangleBrush, $points)
    $triangleBrush.Dispose()
    $labelFont.Dispose()
    $detailFont.Dispose()
    Save-Canvas $canvas $Path
}

function New-FlowCard {
    param(
        [Parameter(Mandatory)][string]$Title,
        [Parameter(Mandatory)][string[]]$Steps,
        [Parameter(Mandatory)][string]$Footer,
        [Parameter(Mandatory)][string]$Path,
        [int]$ActiveStep = -1
    )

    $canvas = New-Canvas
    $graphics = $canvas.Graphics
    $titleFont = [Drawing.Font]::new('Bahnschrift', 54, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $boxFont = [Drawing.Font]::new('Bahnschrift', 25, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $smallFont = [Drawing.Font]::new('Bahnschrift', 23, [Drawing.FontStyle]::Regular, [Drawing.GraphicsUnit]::Pixel)
    Draw-TextBlock $graphics $Title $titleFont $whiteColor ([Drawing.RectangleF]::new(140, 85, 1640, 90)) ([Drawing.StringAlignment]::Center)
    $gap = 45
    $boxWidth = [int][Math]::Floor((1640 - (($Steps.Count - 1) * $gap)) / $Steps.Count)
    $startX = 140
    for ($index = 0; $index -lt $Steps.Count; $index++) {
        $x = $startX + ($index * ($boxWidth + $gap))
        $color = if ($index -eq $ActiveStep) { $pinkColor } else { $panelColor }
        $brush = [Drawing.SolidBrush]::new($color)
        $graphics.FillRectangle($brush, $x, 370, $boxWidth, 250)
        $brush.Dispose()
        $stepText = $Steps[$index] -replace '`n', "`n"
        Draw-TextBlock $graphics $stepText $boxFont $whiteColor ([Drawing.RectangleF]::new($x + 18, 410, $boxWidth - 36, 160)) ([Drawing.StringAlignment]::Center) ([Drawing.StringAlignment]::Center)
        if ($index -lt ($Steps.Count - 1)) {
            Draw-Arrow $graphics ($x + $boxWidth + 6) 495 ($x + $boxWidth + $gap - 6) 495
        }
    }
    Draw-TextBlock $graphics $Footer $smallFont $mutedColor ([Drawing.RectangleF]::new(210, 760, 1500, 100)) ([Drawing.StringAlignment]::Center)
    $titleFont.Dispose()
    $boxFont.Dispose()
    $smallFont.Dispose()
    Save-Canvas $canvas $Path
}

function New-MetricCard {
    param(
        [Parameter(Mandatory)][string]$Title,
        [Parameter(Mandatory)][object[]]$Metrics,
        [Parameter(Mandatory)][string]$Footer,
        [Parameter(Mandatory)][string]$Path
    )

    $canvas = New-Canvas
    $graphics = $canvas.Graphics
    $titleFont = [Drawing.Font]::new('Bahnschrift', 52, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $numberFont = [Drawing.Font]::new('Bahnschrift', 86, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $labelFont = [Drawing.Font]::new('Bahnschrift', 25, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $footerFont = [Drawing.Font]::new('Bahnschrift', 23, [Drawing.FontStyle]::Regular, [Drawing.GraphicsUnit]::Pixel)
    Draw-TextBlock $graphics $Title $titleFont $whiteColor ([Drawing.RectangleF]::new(150, 70, 1620, 90)) ([Drawing.StringAlignment]::Center)
    $boxWidth = 500
    $gap = 65
    $startX = 145
    $colors = @($pinkColor, $greenColor, $whiteColor)
    for ($index = 0; $index -lt $Metrics.Count; $index++) {
        $x = $startX + ($index * ($boxWidth + $gap))
        $brush = [Drawing.SolidBrush]::new($panelMutedColor)
        $graphics.FillRectangle($brush, $x, 290, $boxWidth, 360)
        $brush.Dispose()
        Draw-TextBlock $graphics ([string]$Metrics[$index][0]) $numberFont $colors[$index] ([Drawing.RectangleF]::new($x + 20, 345, $boxWidth - 40, 125)) ([Drawing.StringAlignment]::Center)
        Draw-TextBlock $graphics ([string]$Metrics[$index][1]) $labelFont $whiteColor ([Drawing.RectangleF]::new($x + 25, 500, $boxWidth - 50, 90)) ([Drawing.StringAlignment]::Center)
    }
    Draw-TextBlock $graphics $Footer $footerFont $mutedColor ([Drawing.RectangleF]::new(220, 770, 1480, 100)) ([Drawing.StringAlignment]::Center)
    $titleFont.Dispose()
    $numberFont.Dispose()
    $labelFont.Dispose()
    $footerFont.Dispose()
    Save-Canvas $canvas $Path
}

function New-WindingCard {
    param([Parameter(Mandatory)][string]$Path)

    $canvas = New-Canvas
    $graphics = $canvas.Graphics
    $titleFont = [Drawing.Font]::new('Bahnschrift', 51, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $labelFont = [Drawing.Font]::new('Bahnschrift', 27, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $bodyFont = [Drawing.Font]::new('Bahnschrift', 24, [Drawing.FontStyle]::Regular, [Drawing.GraphicsUnit]::Pixel)
    Draw-TextBlock $graphics 'The completion image was convincingly inside-out' $titleFont $whiteColor ([Drawing.RectangleF]::new(120, 65, 1680, 90)) ([Drawing.StringAlignment]::Center)
    $paths = @(
        (Join-Path $lifecycleDirectory 'winding-diagnostic-a.png'),
        (Join-Path $lifecycleDirectory 'winding-diagnostic-b.png')
    )
    $labels = @('WARM NEAR FACE', 'BLUE FAR FACE')
    for ($index = 0; $index -lt 2; $index++) {
        $image = [Drawing.Image]::FromFile($paths[$index])
        $x = 120 + ($index * 900)
        $graphics.DrawImage($image, [Drawing.RectangleF]::new($x, 245, 780, 540))
        $image.Dispose()
        Draw-TextBlock $graphics $labels[$index] $labelFont $(if ($index -eq 0) { $orangeColor } else { $blueColor }) ([Drawing.RectangleF]::new($x, 820, 780, 55)) ([Drawing.StringAlignment]::Center)
    }
    Draw-TextBlock $graphics 'Projection flips Y - counter-clockwise framebuffer winding is front-facing' $bodyFont $mutedColor ([Drawing.RectangleF]::new(200, 920, 1520, 55)) ([Drawing.StringAlignment]::Center)
    $titleFont.Dispose()
    $labelFont.Dispose()
    $bodyFont.Dispose()
    Save-Canvas $canvas $Path
}

function Get-VisualKey {
    param(
        [Parameter(Mandatory)][int]$Index,
        [Parameter(Mandatory)]$Segment
    )

    $text = $Segment.Text
    switch ($Segment.Beat) {
        1 {
            if ($text -match 'pink triangle') { return 'triangle' }
            if ($text -match 'graphics convention|back of it|convincingly wrong') { return 'winding' }
            if ($text -match 'cavities|three materials|camera') { return 'overview' }
            return 'title'
        }
        2 {
            if ($text -match 'independent|Voxel Frontend|Render Path|Render Backend') { return 'architecture' }
            return 'semantics'
        }
        3 {
            if ($text -match 'evict|protocol|synchronization|presentation|resize|minimize|restore') { return 'protocol' }
            if ($text -match 'validated revision|stable scene|logical Voxel Regions|one batch|four differently|sixty exposed') { return 'semantics' }
            if ($text -match 'background|matching artifact|worker|partial artifact|phase and revision') {
                if ($text -match 'released|installed') { return 'matching' }
                return 'worker'
            }
            if ($text -match '3,366,912|217,856|fixed-seed|inspection poses') { return 'geometry' }
            if ($text -match 'inside-out|projection|winding|back-face|pixels need') { return 'winding' }
            return 'overview'
        }
        4 {
            if ($text -match 'paused|released|responsive') { return 'worker' }
            if ($text -match 'three fixed poses|carved opening|finite edge|camera') {
                $payoffVisuals = @('overview', 'cavity', 'boundary', 'camera')
                return $payoffVisuals[$Index % $payoffVisuals.Count]
            }
            if ($text -match '675 milliseconds|descriptive baseline|performance target') { return 'timing' }
            if ($text -match '212|hash|decodes|audited|validation warnings|zero errors') { return 'proof' }
            if ($text -match 'does not prove') { return 'limits' }
            if ($text -match 'vertical slice|storage-neutral|collapsing') { return 'architecture' }
            return 'matching'
        }
        5 { return 'roadmap' }
        default { return 'title' }
    }
}

$visualPaths = @{
    title = Join-Path $visualDirectory 'title.png'
    triangle = Join-Path $visualDirectory 'triangle.png'
    overview = Join-Path $visualDirectory 'overview.png'
    cavity = Join-Path $visualDirectory 'cavity.png'
    boundary = Join-Path $visualDirectory 'boundary.png'
    camera = Join-Path $visualDirectory 'camera.png'
    worker = Join-Path $visualDirectory 'worker.png'
    matching = Join-Path $visualDirectory 'matching.png'
    architecture = Join-Path $visualDirectory 'architecture.png'
    protocol = Join-Path $visualDirectory 'protocol.png'
    semantics = Join-Path $visualDirectory 'semantics.png'
    geometry = Join-Path $visualDirectory 'geometry.png'
    winding = Join-Path $visualDirectory 'winding.png'
    timing = Join-Path $visualDirectory 'timing.png'
    proof = Join-Path $visualDirectory 'proof.png'
    limits = Join-Path $visualDirectory 'limits.png'
    roadmap = Join-Path $visualDirectory 'roadmap.png'
}

New-TitleCard $visualPaths.title
New-TriangleCard $visualPaths.triangle
New-EvidenceCard (Join-Path $representativeFrameDirectory 'fixed_pose_overview.png') 'The first complete Voxel Scene' 'Overview - three material regions - fixed-seed canonical scene' $visualPaths.overview
New-EvidenceCard (Join-Path $representativeFrameDirectory 'fixed_pose_cavity.png') 'Carved cavity inspection pose' 'Internal surfaces remain visible and semantically exact' $visualPaths.cavity
New-EvidenceCard (Join-Path $representativeFrameDirectory 'fixed_pose_boundary.png') 'Finite-boundary inspection pose' 'The volume edge is part of the exposed-face rule' $visualPaths.boundary
New-EvidenceCard (Join-Path $representativeFrameDirectory 'deterministic_camera_move.png') 'Deterministic camera move' 'The completed artifact remains stable while the camera changes' $visualPaths.camera
New-EvidenceCard (Join-Path $representativeFrameDirectory 'worker_paused.png') 'Background preparation paused' 'The real window remains responsive before geometry is installed' $visualPaths.worker
New-EvidenceCard (Join-Path $representativeFrameDirectory 'first_matching_revision_frame.png') 'First matching-revision frame' 'Revision 1 becomes visible once, only after complete GPU installation' $visualPaths.matching
New-FlowCard 'One scene revision crosses two deliberate seams' @('dense`nStorage Tier', 'Voxel`nFrontend', 'immutable`nScene View', 'raster`nRender Path', 'Render`nBackend') 'Storage layout stays behind the Voxel Frontend. Vulkan execution stays behind the Render Backend.' $visualPaths.architecture 2
New-FlowCard 'The backend drives the Render Path lifecycle' @('release', 'recreate`ntargets', 'configure', 'record', 'submit +`npresent') 'The triangle survived this move before voxel code depended on the protocol.' $visualPaths.protocol 2
New-FlowCard 'Logical reads produce an exact surface artifact' @('one batch`nor four', 'occupied to`nempty', 'exact face`nset', 'immutable`nartifact') 'A hollow 3 x 3 x 3 diagnostic produces exactly 60 exposed faces, including its cavity.' $visualPaths.semantics 2
New-MetricCard 'Canonical 256-scale scene' @(@('3,366,912', 'occupied voxels'), @('217,856', 'exposed quads'), @('3', 'repeatable poses')) 'Independent quads are deliberately simple, whole-volume, and non-incremental.' $visualPaths.geometry
New-WindingCard $visualPaths.winding
New-MetricCard 'Recorded preparation baseline' @(@('10', 'fresh runs'), @('674.7 ms', 'median total'), @('256^3', 'scene scale')) 'Publication to first correct frame on this one Windows development machine - not a performance target.' $visualPaths.timing
New-MetricCard 'Auditable milestone bundle' @(@('0', 'Vulkan warnings'), @('0', 'Vulkan errors'), @('212', 'hashed artifacts')) 'Detached clean checkout - independently decoded completion clip - exact diagnostics and raw timing streams' $visualPaths.proof
New-FlowCard 'What this milestone does not prove' @('interactive`nediting', 'production`nperformance', 'sparse`nstorage', 'another`nRender Path', 'other`nplatforms') 'The claim is intentionally limited to this validation-enabled Windows proof.' $visualPaths.limits
New-FlowCard 'The next concurrency contract' @('edit`nvoxels', 'publish newer`nrevision', 'remesh affected`nresults', 'reject stale`nwork') 'Dense raster proof is complete. Revision-correct editable raster is the provisional next outcome.' $visualPaths.roadmap 1

$markdown = Get-Content -Raw $scriptPath
$allSegments = @(Get-NarrationSegments $markdown)
if ($allSegments.Count -eq 0) {
    throw 'No narration segments were found in the devlog script.'
}
if ($MaximumNarrationSegments -gt 0 -and $MaximumNarrationSegments -lt $allSegments.Count) {
    $segments = @($allSegments[0..($MaximumNarrationSegments - 1)])
}
else {
    $segments = $allSegments
}

$synthesizer = [Speech.Synthesis.SpeechSynthesizer]::new()
$synthesizer.SelectVoice('Microsoft David Desktop')
$synthesizer.Rate = $VoiceRate

$renderedSegments = [Collections.Generic.List[object]]::new()
try {
    for ($index = 0; $index -lt $segments.Count; $index++) {
        $segment = $segments[$index]
        $baseName = 'segment-{0:000}' -f ($index + 1)
        $rawPath = Join-Path $narrationDirectory "$baseName-raw.wav"
        $paddedPath = Join-Path $narrationDirectory "$baseName.wav"
        $synthesizer.SetOutputToWaveFile($rawPath)
        $synthesizer.Speak($segment.Text)
        $synthesizer.SetOutputToNull()
        $rawDuration = Get-MediaDuration $rawPath
        $padding = if ($segment.BeatEnd) { 1.0 } elseif ($segment.ParagraphEnd) { 0.55 } else { 0.2 }
        $targetDuration = $rawDuration + $padding
        $targetDurationText = $targetDuration.ToString('0.000000', [Globalization.CultureInfo]::InvariantCulture)
        Invoke-MediaCommand ffmpeg @(
            '-hide_banner', '-loglevel', 'error', '-y', '-i', $rawPath,
            '-af', "apad=pad_dur=$padding", '-t', $targetDurationText,
            '-ar', '48000', '-ac', '1', '-c:a', 'pcm_s16le', $paddedPath
        )
        $renderedSegments.Add([pscustomobject]@{
            Text = $segment.Text
            RawDuration = $rawDuration
            PaddedDuration = Get-MediaDuration $paddedPath
            AudioPath = $paddedPath
            VisualKey = Get-VisualKey $index $segment
        })
    }
}
finally {
    $synthesizer.Dispose()
}

$audioConcatPath = Join-Path $OutputDirectory 'audio.ffconcat'
$audioConcatLines = [Collections.Generic.List[string]]::new()
$audioConcatLines.Add('ffconcat version 1.0')
foreach ($segment in $renderedSegments) {
    $audioConcatLines.Add("file '$(ConvertTo-ConcatPath $segment.AudioPath)'")
}
[IO.File]::WriteAllLines($audioConcatPath, $audioConcatLines, [Text.UTF8Encoding]::new($false))

$narrationPath = Join-Path $OutputDirectory 'narration.wav'
Invoke-MediaCommand ffmpeg @(
    '-hide_banner', '-loglevel', 'error', '-y', '-f', 'concat', '-safe', '0', '-i', $audioConcatPath,
    '-c:a', 'pcm_s16le', '-ar', '48000', '-ac', '1', $narrationPath
)

$captionPath = Join-Path $OutputDirectory 'captions.srt'
$captionLines = [Collections.Generic.List[string]]::new()
$currentTime = 0.0
$captionIndex = 1
foreach ($segment in $renderedSegments) {
    $captionChunks = @(Split-CaptionText $segment.Text)
    $totalWordCount = ($captionChunks | ForEach-Object { ($_ -split '\s+').Count } | Measure-Object -Sum).Sum
    $captionStart = $currentTime
    foreach ($captionChunk in $captionChunks) {
        $chunkWordCount = ($captionChunk -split '\s+').Count
        $chunkDuration = $segment.RawDuration * ($chunkWordCount / $totalWordCount)
        $captionLines.Add($captionIndex.ToString())
        $captionLines.Add("$(ConvertTo-SrtTimestamp $captionStart) --> $(ConvertTo-SrtTimestamp ($captionStart + $chunkDuration))")
        $captionLines.Add($captionChunk)
        $captionLines.Add('')
        $captionStart += $chunkDuration
        $captionIndex++
    }
    $currentTime += $segment.PaddedDuration
}
[IO.File]::WriteAllLines($captionPath, $captionLines, [Text.UTF8Encoding]::new($true))

$visualConcatPath = Join-Path $OutputDirectory 'visuals.ffconcat'
$visualConcatLines = [Collections.Generic.List[string]]::new()
$visualConcatLines.Add('ffconcat version 1.0')
foreach ($segment in $renderedSegments) {
    $visualPath = $visualPaths[$segment.VisualKey]
    $visualConcatLines.Add("file '$(ConvertTo-ConcatPath $visualPath)'")
    $visualConcatLines.Add("duration $($segment.PaddedDuration.ToString('0.000000', [Globalization.CultureInfo]::InvariantCulture))")
}
$lastVisualPath = $visualPaths[$renderedSegments[$renderedSegments.Count - 1].VisualKey]
$visualConcatLines.Add("file '$(ConvertTo-ConcatPath $lastVisualPath)'")
[IO.File]::WriteAllLines($visualConcatPath, $visualConcatLines, [Text.UTF8Encoding]::new($false))

Push-Location $OutputDirectory
try {
    $videoFilter = if ($BurnSubtitles) {
        "fps=30,subtitles='captions.srt':force_style='FontName=Arial,FontSize=14,PrimaryColour=&H00FFFFFF,BorderStyle=3,BackColour=&H90000000,Outline=0,Shadow=0,MarginV=32,Alignment=2',format=yuv420p"
    }
    else {
        'fps=30,format=yuv420p'
    }
    Invoke-MediaCommand ffmpeg @(
        '-hide_banner', '-loglevel', 'error', '-y',
        '-f', 'concat', '-safe', '0', '-i', 'visuals.ffconcat',
        '-i', 'narration.wav',
        '-vf', $videoFilter,
        '-af', 'loudnorm=I=-16:TP=-1.5:LRA=7',
        '-c:v', 'libx264', '-preset', 'medium', '-crf', '20',
        '-c:a', 'aac', '-b:a', '160k', '-ar', '48000',
        '-movflags', '+faststart', '-shortest',
        '-metadata', 'title=Voxel Nexus Devlog 02 - The First Real Voxel Image',
        $outputVideoPath
    )
}
finally {
    Pop-Location
}

$videoDuration = Get-MediaDuration $outputVideoPath
$summary = [ordered]@{
    Output = $outputVideoPath
    VoiceProvider = 'Windows'
    Voice = 'Microsoft David Desktop'
    VoiceRate = $VoiceRate
    NarrationSegments = $renderedSegments.Count
    DurationSeconds = [Math]::Round($videoDuration, 3)
    Resolution = '1920x1080'
    FrameRate = 30
    BurnedSubtitles = [bool]$BurnSubtitles
    Music = 'None'
    SourceScript = $scriptPath
}
$summary | ConvertTo-Json | Set-Content -Encoding utf8 (Join-Path $OutputDirectory 'build-summary.json')
$summary | ConvertTo-Json
