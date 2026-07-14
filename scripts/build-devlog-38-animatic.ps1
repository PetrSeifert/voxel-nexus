param(
    [string]$OutputDirectory,
    [string]$OutputFileName = 'voxel-nexus-devlog-03-animatic.mp4',
    [int]$VoiceRate = 0,
    [switch]$BurnSubtitles,
    [ValidateRange(0, 2147483647)]
    [int]$MaximumNarrationSegments = 0
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repositoryRoot = Split-Path $PSScriptRoot -Parent
if ([string]::IsNullOrWhiteSpace($OutputDirectory)) {
    $OutputDirectory = Join-Path $repositoryRoot 'artifacts/devlog-38-animatic'
}
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
$narrationDirectory = Join-Path $OutputDirectory 'narration'
$visualDirectory = Join-Path $OutputDirectory 'visuals'
$scriptPath = Join-Path $repositoryRoot 'docs/devlogs/38-localized-editable-raster-convergence.md'
$evidenceDirectory = Join-Path $repositoryRoot 'docs/evidence/localized-editable-raster/v1/development-machine'
$demoDirectory = Join-Path $evidenceDirectory 'demo'
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

function Draw-Panel {
    param(
        [Parameter(Mandatory)][Drawing.Graphics]$Graphics,
        [Parameter(Mandatory)][Drawing.Rectangle]$Rectangle,
        [Drawing.Color]$Color = $panelColor
    )

    $brush = [Drawing.SolidBrush]::new($Color)
    $Graphics.FillRectangle($brush, $Rectangle)
    $brush.Dispose()
}

function New-TitleCard {
    param([Parameter(Mandatory)][string]$Path)

    $canvas = New-Canvas
    $graphics = $canvas.Graphics
    $accentBrush = [Drawing.SolidBrush]::new($pinkColor)
    $graphics.FillRectangle($accentBrush, 0, 0, 44, 1080)
    $accentBrush.Dispose()
    $eyebrowFont = [Drawing.Font]::new('Bahnschrift', 34, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $titleFont = [Drawing.Font]::new('Bahnschrift', 92, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $subtitleFont = [Drawing.Font]::new('Bahnschrift', 34, [Drawing.FontStyle]::Regular, [Drawing.GraphicsUnit]::Pixel)
    Draw-TextBlock $graphics 'VOXEL NEXUS 03' $eyebrowFont $pinkColor ([Drawing.RectangleF]::new(170, 220, 1500, 60))
    Draw-TextBlock $graphics "THE VOXELS CAN`nFINALLY CHANGE" $titleFont $whiteColor ([Drawing.RectangleF]::new(160, 335, 1600, 250))
    Draw-TextBlock $graphics 'Localized edits • atomic convergence • last-good image retained' $subtitleFont $mutedColor ([Drawing.RectangleF]::new(168, 690, 1500, 65))
    Draw-TextBlock $graphics 'Animatic • Microsoft David Desktop voice' $subtitleFont $mutedColor ([Drawing.RectangleF]::new(168, 850, 1500, 55))
    $eyebrowFont.Dispose()
    $titleFont.Dispose()
    $subtitleFont.Dispose()
    Save-Canvas $canvas $Path
}

function New-EvidenceCard {
    param(
        [Parameter(Mandatory)][string]$SourcePath,
        [Parameter(Mandatory)][string]$Heading,
        [Parameter(Mandatory)][string]$Detail,
        [Parameter(Mandatory)][string]$Path
    )

    $canvas = New-Canvas
    $graphics = $canvas.Graphics
    $headingFont = [Drawing.Font]::new('Bahnschrift', 42, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $detailFont = [Drawing.Font]::new('Bahnschrift', 25, [Drawing.FontStyle]::Regular, [Drawing.GraphicsUnit]::Pixel)
    Draw-TextBlock $graphics $Heading $headingFont $whiteColor ([Drawing.RectangleF]::new(130, 45, 1660, 65)) ([Drawing.StringAlignment]::Center)
    $image = [Drawing.Image]::FromFile($SourcePath)
    try {
        $maximumWidth = 1660.0
        $maximumHeight = 820.0
        $scale = [Math]::Min($maximumWidth / $image.Width, $maximumHeight / $image.Height)
        $width = [int]($image.Width * $scale)
        $height = [int]($image.Height * $scale)
        $x = [int]((1920 - $width) / 2)
        $y = [int](130 + (($maximumHeight - $height) / 2))
        Draw-Panel $graphics ([Drawing.Rectangle]::new($x - 8, $y - 8, $width + 16, $height + 16)) $panelMutedColor
        $graphics.DrawImage($image, [Drawing.Rectangle]::new($x, $y, $width, $height))
    }
    finally {
        $image.Dispose()
    }
    Draw-TextBlock $graphics $Detail $detailFont $mutedColor ([Drawing.RectangleF]::new(160, 975, 1600, 45)) ([Drawing.StringAlignment]::Center)
    $headingFont.Dispose()
    $detailFont.Dispose()
    Save-Canvas $canvas $Path
}

function New-FlowCard {
    param([Parameter(Mandatory)][string]$Path)

    $canvas = New-Canvas
    $graphics = $canvas.Graphics
    $titleFont = [Drawing.Font]::new('Bahnschrift', 54, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $boxFont = [Drawing.Font]::new('Bahnschrift', 28, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $smallFont = [Drawing.Font]::new('Bahnschrift', 23, [Drawing.FontStyle]::Regular, [Drawing.GraphicsUnit]::Pixel)
    Draw-TextBlock $graphics 'One edit publishes a complete successor' $titleFont $whiteColor ([Drawing.RectangleF]::new(120, 80, 1680, 80)) ([Drawing.StringAlignment]::Center)
    $boxes = @(
        @("Voxel Edit`nCommand", 125, $orangeColor),
        @("Immutable successor`nVoxel Scene View", 655, $greenColor),
        @("Raster`nRender Path", 1360, $blueColor)
    )
    foreach ($box in $boxes) {
        Draw-Panel $graphics ([Drawing.Rectangle]::new([int]$box[1], 330, 435, 215)) ([Drawing.Color]$box[2])
        Draw-TextBlock $graphics ([string]$box[0]) $boxFont $whiteColor ([Drawing.RectangleF]::new([int]$box[1] + 20, 365, 395, 145)) ([Drawing.StringAlignment]::Center) ([Drawing.StringAlignment]::Center)
    }
    Draw-TextBlock $graphics '→' $titleFont $mutedColor ([Drawing.RectangleF]::new(565, 390, 80, 80)) ([Drawing.StringAlignment]::Center)
    Draw-TextBlock $graphics '+ Voxel Change Set  →' $smallFont $mutedColor ([Drawing.RectangleF]::new(1090, 405, 265, 55)) ([Drawing.StringAlignment]::Center)
    Draw-Panel $graphics ([Drawing.Rectangle]::new(650, 720, 620, 125)) $panelMutedColor
    Draw-TextBlock $graphics 'Dense Storage Tier stays below the Frontend seam' $smallFont $mutedColor ([Drawing.RectangleF]::new(680, 755, 560, 55)) ([Drawing.StringAlignment]::Center)
    $titleFont.Dispose()
    $boxFont.Dispose()
    $smallFont.Dispose()
    Save-Canvas $canvas $Path
}

function New-RegionCard {
    param([Parameter(Mandatory)][string]$Path)

    $canvas = New-Canvas
    $graphics = $canvas.Graphics
    $titleFont = [Drawing.Font]::new('Bahnschrift', 52, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $labelFont = [Drawing.Font]::new('Bahnschrift', 27, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $smallFont = [Drawing.Font]::new('Bahnschrift', 23, [Drawing.FontStyle]::Regular, [Drawing.GraphicsUnit]::Pixel)
    Draw-TextBlock $graphics 'Localized invalidation follows faces, not diagonals' $titleFont $whiteColor ([Drawing.RectangleF]::new(120, 70, 1680, 85)) ([Drawing.StringAlignment]::Center)
    for ($row = 0; $row -lt 3; $row++) {
        for ($column = 0; $column -lt 3; $column++) {
            $isCore = $row -eq 1 -and $column -eq 1
            $isFaceNeighbor = [Math]::Abs($row - 1) + [Math]::Abs($column - 1) -eq 1
            $color = if ($isCore) { $pinkColor } elseif ($isFaceNeighbor) { $greenColor } else { $panelColor }
            Draw-Panel $graphics ([Drawing.Rectangle]::new(515 + ($column * 300), 220 + ($row * 230), 270, 200)) $color
            $label = if ($isCore) { 'EDIT CORE' } elseif ($isFaceNeighbor) { 'AFFECTED' } else { 'RETAINED' }
            Draw-TextBlock $graphics $label $labelFont $whiteColor ([Drawing.RectangleF]::new(535 + ($column * 300), 285 + ($row * 230), 230, 70)) ([Drawing.StringAlignment]::Center) ([Drawing.StringAlignment]::Center)
        }
    }
    Draw-TextBlock $graphics 'Each Raster Region reads its core plus a one-voxel face-neighbor halo.' $smallFont $mutedColor ([Drawing.RectangleF]::new(200, 950, 1520, 55)) ([Drawing.StringAlignment]::Center)
    $titleFont.Dispose()
    $labelFont.Dispose()
    $smallFont.Dispose()
    Save-Canvas $canvas $Path
}

function New-TimelineCard {
    param([Parameter(Mandatory)][string]$Path)

    $canvas = New-Canvas
    $graphics = $canvas.Graphics
    $titleFont = [Drawing.Font]::new('Bahnschrift', 54, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $numberFont = [Drawing.Font]::new('Bahnschrift', 50, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $smallFont = [Drawing.Font]::new('Bahnschrift', 25, [Drawing.FontStyle]::Regular, [Drawing.GraphicsUnit]::Pixel)
    Draw-TextBlock $graphics 'Required advances. Visible stays complete.' $titleFont $whiteColor ([Drawing.RectangleF]::new(120, 70, 1680, 85)) ([Drawing.StringAlignment]::Center)
    Draw-TextBlock $graphics 'REQUIRED' $smallFont $mutedColor ([Drawing.RectangleF]::new(150, 315, 250, 45))
    Draw-TextBlock $graphics 'VISIBLE' $smallFont $mutedColor ([Drawing.RectangleF]::new(150, 650, 250, 45))
    $positions = @(430, 760, 1090, 1420)
    for ($index = 0; $index -lt 4; $index++) {
        $revision = $index + 1
        $requiredColor = if ($revision -eq 4) { $greenColor } elseif ($revision -eq 1) { $blueColor } else { $pinkColor }
        Draw-Panel $graphics ([Drawing.Rectangle]::new($positions[$index], 270, 190, 140)) $requiredColor
        Draw-TextBlock $graphics "$revision" $numberFont $whiteColor ([Drawing.RectangleF]::new($positions[$index], 292, 190, 90)) ([Drawing.StringAlignment]::Center)
    }
    Draw-Panel $graphics ([Drawing.Rectangle]::new(430, 605, 190, 140)) $blueColor
    Draw-TextBlock $graphics '1' $numberFont $whiteColor ([Drawing.RectangleF]::new(430, 627, 190, 90)) ([Drawing.StringAlignment]::Center)
    Draw-TextBlock $graphics '───────────────' $numberFont $mutedColor ([Drawing.RectangleF]::new(620, 625, 780, 85)) ([Drawing.StringAlignment]::Center)
    Draw-Panel $graphics ([Drawing.Rectangle]::new(1420, 605, 190, 140)) $greenColor
    Draw-TextBlock $graphics '4' $numberFont $whiteColor ([Drawing.RectangleF]::new(1420, 627, 190, 90)) ([Drawing.StringAlignment]::Center)
    Draw-TextBlock $graphics 'Revision 2: cancelled on CPU     Revision 3: rejected after upload     Revision 4: committed atomically' $smallFont $mutedColor ([Drawing.RectangleF]::new(190, 870, 1540, 70)) ([Drawing.StringAlignment]::Center)
    $titleFont.Dispose()
    $numberFont.Dispose()
    $smallFont.Dispose()
    Save-Canvas $canvas $Path
}

function New-LogCard {
    param([Parameter(Mandatory)][string]$Path)

    $canvas = New-Canvas
    $graphics = $canvas.Graphics
    $titleFont = [Drawing.Font]::new('Bahnschrift', 50, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $codeFont = [Drawing.Font]::new('Consolas', 30, [Drawing.FontStyle]::Regular, [Drawing.GraphicsUnit]::Pixel)
    $labelFont = [Drawing.Font]::new('Bahnschrift', 25, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    Draw-TextBlock $graphics 'Stale work is discarded at both gates' $titleFont $whiteColor ([Drawing.RectangleF]::new(120, 75, 1680, 80)) ([Drawing.StringAlignment]::Center)
    Draw-Panel $graphics ([Drawing.Rectangle]::new(160, 245, 1600, 235)) $panelMutedColor
    Draw-TextBlock $graphics 'CPU GATE' $labelFont $pinkColor ([Drawing.RectangleF]::new(215, 285, 300, 45))
    Draw-TextBlock $graphics 'Obsolete CPU generation cancelled: revision 2' $codeFont $whiteColor ([Drawing.RectangleF]::new(215, 355, 1480, 65))
    Draw-Panel $graphics ([Drawing.Rectangle]::new(160, 575, 1600, 235)) $panelMutedColor
    Draw-TextBlock $graphics 'COMMIT GATE' $labelFont $pinkColor ([Drawing.RectangleF]::new(215, 615, 300, 45))
    Draw-TextBlock $graphics 'Superseded candidate rejected at commit: revision 3' $codeFont $whiteColor ([Drawing.RectangleF]::new(215, 685, 1480, 65))
    Draw-TextBlock $graphics 'Correctness still comes from generation and revision checks—not cancellation timing.' $labelFont $mutedColor ([Drawing.RectangleF]::new(220, 900, 1480, 55)) ([Drawing.StringAlignment]::Center)
    $titleFont.Dispose()
    $codeFont.Dispose()
    $labelFont.Dispose()
    Save-Canvas $canvas $Path
}

function New-MetricsCard {
    param([Parameter(Mandatory)][string]$Path)

    $canvas = New-Canvas
    $graphics = $canvas.Graphics
    $titleFont = [Drawing.Font]::new('Bahnschrift', 52, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $numberFont = [Drawing.Font]::new('Bahnschrift', 62, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $labelFont = [Drawing.Font]::new('Bahnschrift', 24, [Drawing.FontStyle]::Regular, [Drawing.GraphicsUnit]::Pixel)
    Draw-TextBlock $graphics 'Raster Region extent tradeoff' $titleFont $whiteColor ([Drawing.RectangleF]::new(120, 70, 1680, 80)) ([Drawing.StringAlignment]::Center)
    $cards = @(
        @('16³', '732 ms median', '1,414 buffers', 150, $greenColor),
        @('32³', '871 ms median', '302 buffers', 690, $panelColor),
        @('64³', '2,550 ms median', '64 buffers', 1230, $panelColor)
    )
    foreach ($card in $cards) {
        Draw-Panel $graphics ([Drawing.Rectangle]::new([int]$card[3], 260, 460, 500)) ([Drawing.Color]$card[4])
        Draw-TextBlock $graphics ([string]$card[0]) $numberFont $whiteColor ([Drawing.RectangleF]::new([int]$card[3] + 20, 320, 420, 100)) ([Drawing.StringAlignment]::Center)
        Draw-TextBlock $graphics ([string]$card[1]) $labelFont $whiteColor ([Drawing.RectangleF]::new([int]$card[3] + 25, 500, 410, 55)) ([Drawing.StringAlignment]::Center)
        Draw-TextBlock $graphics ([string]$card[2]) $labelFont $whiteColor ([Drawing.RectangleF]::new([int]$card[3] + 25, 610, 410, 55)) ([Drawing.StringAlignment]::Center)
    }
    Draw-TextBlock $graphics 'Recorded Windows development machine • five samples per candidate • selected extent: 16³' $labelFont $mutedColor ([Drawing.RectangleF]::new(180, 880, 1560, 65)) ([Drawing.StringAlignment]::Center)
    $titleFont.Dispose()
    $numberFont.Dispose()
    $labelFont.Dispose()
    Save-Canvas $canvas $Path
}

function New-StatesCard {
    param([Parameter(Mandatory)][string]$Path)

    $canvas = New-Canvas
    $graphics = $canvas.Graphics
    $titleFont = [Drawing.Font]::new('Bahnschrift', 48, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $stateFont = [Drawing.Font]::new('Consolas', 33, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $smallFont = [Drawing.Font]::new('Bahnschrift', 23, [Drawing.FontStyle]::Regular, [Drawing.GraphicsUnit]::Pixel)
    Draw-TextBlock $graphics 'One visible jump: 1 → 4' $titleFont $whiteColor ([Drawing.RectangleF]::new(120, 70, 1680, 75)) ([Drawing.StringAlignment]::Center)
    $states = @(
        @('CPU HELD', 'Required 3 / Visible 1', 'Affected 2 / Unaffected 254', 120, $pinkColor),
        @('UPLOAD HELD', 'Required 4 / Visible 1', 'Affected 3 / Unaffected 253', 690, $pinkColor),
        @('COMMITTED', 'Required 4 / Visible 4', 'Affected 3 / Unaffected 253', 1260, $greenColor)
    )
    foreach ($state in $states) {
        Draw-Panel $graphics ([Drawing.Rectangle]::new([int]$state[3], 260, 500, 470)) ([Drawing.Color]$state[4])
        Draw-TextBlock $graphics ([string]$state[0]) $smallFont $whiteColor ([Drawing.RectangleF]::new([int]$state[3] + 25, 305, 450, 50)) ([Drawing.StringAlignment]::Center)
        Draw-TextBlock $graphics ([string]$state[1]) $stateFont $whiteColor ([Drawing.RectangleF]::new([int]$state[3] + 30, 410, 440, 120)) ([Drawing.StringAlignment]::Center)
        Draw-TextBlock $graphics ([string]$state[2]) $smallFont $whiteColor ([Drawing.RectangleF]::new([int]$state[3] + 30, 610, 440, 65)) ([Drawing.StringAlignment]::Center)
    }
    Draw-TextBlock $graphics 'Visible revision:       1  ─────────────────────────  1  ─────────────────────────  4' $smallFont $mutedColor ([Drawing.RectangleF]::new(170, 870, 1580, 60)) ([Drawing.StringAlignment]::Center)
    $titleFont.Dispose()
    $stateFont.Dispose()
    $smallFont.Dispose()
    Save-Canvas $canvas $Path
}

function New-ScaleCard {
    param([Parameter(Mandatory)][string]$Path)

    $canvas = New-Canvas
    $graphics = $canvas.Graphics
    $titleFont = [Drawing.Font]::new('Bahnschrift', 50, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $numberFont = [Drawing.Font]::new('Bahnschrift', 76, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $labelFont = [Drawing.Font]::new('Bahnschrift', 24, [Drawing.FontStyle]::Regular, [Drawing.GraphicsUnit]::Pixel)
    Draw-TextBlock $graphics 'Selected 16³ regions across three scene scales' $titleFont $whiteColor ([Drawing.RectangleF]::new(120, 75, 1680, 75)) ([Drawing.StringAlignment]::Center)
    $scales = @(
        @('64 scale', '122 ms', 180, 260),
        @('128 scale', '187 ms', 760, 420),
        @('256 scale', '729 ms', 1340, 650)
    )
    foreach ($scale in $scales) {
        Draw-Panel $graphics ([Drawing.Rectangle]::new([int]$scale[2], 780 - [int]$scale[3], 400, [int]$scale[3])) $blueColor
        Draw-TextBlock $graphics ([string]$scale[1]) $numberFont $whiteColor ([Drawing.RectangleF]::new([int]$scale[2], 845 - [int]$scale[3], 400, 105)) ([Drawing.StringAlignment]::Center)
        Draw-TextBlock $graphics ([string]$scale[0]) $labelFont $whiteColor ([Drawing.RectangleF]::new([int]$scale[2], 720, 400, 55)) ([Drawing.StringAlignment]::Center)
    }
    Draw-TextBlock $graphics 'Machine-local medians • five samples per scale • descriptive, not a production budget' $labelFont $mutedColor ([Drawing.RectangleF]::new(180, 905, 1560, 60)) ([Drawing.StringAlignment]::Center)
    $titleFont.Dispose()
    $numberFont.Dispose()
    $labelFont.Dispose()
    Save-Canvas $canvas $Path
}

function New-ProofCard {
    param([Parameter(Mandatory)][string]$Path)

    $canvas = New-Canvas
    $graphics = $canvas.Graphics
    $titleFont = [Drawing.Font]::new('Bahnschrift', 50, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $numberFont = [Drawing.Font]::new('Bahnschrift', 120, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $labelFont = [Drawing.Font]::new('Bahnschrift', 25, [Drawing.FontStyle]::Regular, [Drawing.GraphicsUnit]::Pixel)
    Draw-TextBlock $graphics 'Verified milestone evidence' $titleFont $whiteColor ([Drawing.RectangleF]::new(120, 75, 1680, 75)) ([Drawing.StringAlignment]::Center)
    $proofs = @(
        @('146', 'inventoried artifacts', 130, $whiteColor),
        @('0', 'Vulkan warnings', 690, $greenColor),
        @('0', 'Vulkan errors', 1250, $greenColor)
    )
    foreach ($proof in $proofs) {
        Draw-Panel $graphics ([Drawing.Rectangle]::new([int]$proof[2], 285, 500, 390)) $panelMutedColor
        Draw-TextBlock $graphics ([string]$proof[0]) $numberFont ([Drawing.Color]$proof[3]) ([Drawing.RectangleF]::new([int]$proof[2] + 20, 330, 460, 170)) ([Drawing.StringAlignment]::Center)
        Draw-TextBlock $graphics ([string]$proof[1]) $labelFont $whiteColor ([Drawing.RectangleF]::new([int]$proof[2] + 30, 550, 440, 60)) ([Drawing.StringAlignment]::Center)
    }
    Draw-TextBlock $graphics 'Normal close • zero Render Path-owned raster resources after shutdown • hashes and nested manifests verified' $labelFont $mutedColor ([Drawing.RectangleF]::new(180, 835, 1560, 85)) ([Drawing.StringAlignment]::Center)
    $titleFont.Dispose()
    $numberFont.Dispose()
    $labelFont.Dispose()
    Save-Canvas $canvas $Path
}

function New-LimitationsCard {
    param([Parameter(Mandatory)][string]$Path)

    $canvas = New-Canvas
    $graphics = $canvas.Graphics
    $titleFont = [Drawing.Font]::new('Bahnschrift', 54, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $itemFont = [Drawing.Font]::new('Bahnschrift', 31, [Drawing.FontStyle]::Regular, [Drawing.GraphicsUnit]::Pixel)
    Draw-TextBlock $graphics "THIS STILL ISN'T AN EDITOR" $titleFont $whiteColor ([Drawing.RectangleF]::new(120, 80, 1680, 85)) ([Drawing.StringAlignment]::Center)
    $items = @('No picking', 'No free-form tools', 'No history', 'No streaming', 'No greedy meshing', 'No performance target')
    for ($index = 0; $index -lt $items.Count; $index++) {
        $column = $index % 2
        $row = [Math]::Floor($index / 2)
        Draw-Panel $graphics ([Drawing.Rectangle]::new(210 + ($column * 790), 250 + ($row * 205), 700, 145)) $panelMutedColor
        Draw-TextBlock $graphics $items[$index] $itemFont $mutedColor ([Drawing.RectangleF]::new(240 + ($column * 790), 285 + ($row * 205), 640, 75)) ([Drawing.StringAlignment]::Center) ([Drawing.StringAlignment]::Center)
    }
    Draw-TextBlock $graphics 'Space triggers one fixed sequence designed to expose stale work.' $itemFont $pinkColor ([Drawing.RectangleF]::new(240, 895, 1440, 60)) ([Drawing.StringAlignment]::Center)
    $titleFont.Dispose()
    $itemFont.Dispose()
    Save-Canvas $canvas $Path
}

function New-RoadmapCard {
    param([Parameter(Mandatory)][string]$Path)

    $canvas = New-Canvas
    $graphics = $canvas.Graphics
    $titleFont = [Drawing.Font]::new('Bahnschrift', 52, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $boxFont = [Drawing.Font]::new('Bahnschrift', 27, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $smallFont = [Drawing.Font]::new('Bahnschrift', 22, [Drawing.FontStyle]::Regular, [Drawing.GraphicsUnit]::Pixel)
    Draw-TextBlock $graphics 'Next: a genuinely different path to the screen' $titleFont $whiteColor ([Drawing.RectangleF]::new(120, 75, 1680, 85)) ([Drawing.StringAlignment]::Center)
    $roadmap = @(
        @('COMPLETE', "Localized editable`nraster convergence", 130, $greenColor),
        @('NEXT', "Portable compute-ray`nRender Path", 710, $pinkColor),
        @('LATER', "Editable-SVO`nstorage independence", 1290, $panelColor)
    )
    foreach ($item in $roadmap) {
        Draw-Panel $graphics ([Drawing.Rectangle]::new([int]$item[2], 330, 500, 330)) ([Drawing.Color]$item[3])
        Draw-TextBlock $graphics ([string]$item[0]) $smallFont $whiteColor ([Drawing.RectangleF]::new([int]$item[2] + 20, 370, 460, 45)) ([Drawing.StringAlignment]::Center)
        Draw-TextBlock $graphics ([string]$item[1]) $boxFont $whiteColor ([Drawing.RectangleF]::new([int]$item[2] + 30, 465, 440, 140)) ([Drawing.StringAlignment]::Center) ([Drawing.StringAlignment]::Center)
    }
    Draw-TextBlock $graphics 'One logical Voxel Scene • two image strategies • storage held constant' $smallFont $mutedColor ([Drawing.RectangleF]::new(200, 820, 1520, 65)) ([Drawing.StringAlignment]::Center)
    $titleFont.Dispose()
    $boxFont.Dispose()
    $smallFont.Dispose()
    Save-Canvas $canvas $Path
}

$visualPaths = @{
    title = Join-Path $visualDirectory 'title.png'
    final = Join-Path $visualDirectory 'final-visible.png'
    cpu = Join-Path $visualDirectory 'cpu-held.png'
    upload = Join-Path $visualDirectory 'upload-held.png'
    flow = Join-Path $visualDirectory 'flow.png'
    regions = Join-Path $visualDirectory 'regions.png'
    timeline = Join-Path $visualDirectory 'timeline.png'
    logs = Join-Path $visualDirectory 'logs.png'
    metrics = Join-Path $visualDirectory 'metrics.png'
    states = Join-Path $visualDirectory 'states.png'
    scale = Join-Path $visualDirectory 'scale.png'
    proof = Join-Path $visualDirectory 'proof.png'
    limitations = Join-Path $visualDirectory 'limitations.png'
    roadmap = Join-Path $visualDirectory 'roadmap.png'
}

New-TitleCard $visualPaths.title
New-EvidenceCard (Join-Path $demoDirectory 'final-visible.png') 'Revision 4 is complete and visible' 'Required 4 • Visible 4 • Affected 3 • Unaffected 253' $visualPaths.final
New-EvidenceCard (Join-Path $demoDirectory 'cpu-barrier-held.png') 'CPU barrier: the last complete image stays visible' 'Required 3 • Visible 1 • Affected 2 • Unaffected 254' $visualPaths.cpu
New-EvidenceCard (Join-Path $demoDirectory 'post-upload-barrier-held.png') 'Post-upload barrier: the candidate remains hidden' 'Required 4 • Visible 1 • Affected 3 • Unaffected 253 (reconstructed from the retained log)' $visualPaths.upload
New-FlowCard $visualPaths.flow
New-RegionCard $visualPaths.regions
New-TimelineCard $visualPaths.timeline
New-LogCard $visualPaths.logs
New-MetricsCard $visualPaths.metrics
New-StatesCard $visualPaths.states
New-ScaleCard $visualPaths.scale
New-ProofCard $visualPaths.proof
New-LimitationsCard $visualPaths.limitations
New-RoadmapCard $visualPaths.roadmap

function Get-VisualKey {
    param(
        [Parameter(Mandatory)][int]$Index,
        [Parameter(Mandatory)]$Segment
    )

    $text = $Segment.Text
    switch ($Segment.Beat) {
        1 {
            if ($Index -eq 0) { return 'final' }
            if ($text -match 'Revision two|CPU work') { return 'cpu' }
            if ($text -match 'Revision three|reaches the GPU') { return 'upload' }
            if ($text -match 'revision four|whole visible scene') { return 'states' }
            if ($text -match 'concurrency problem') { return 'title' }
            return 'final'
        }
        2 {
            if ($text -match 'Raster Regions|region|halo|boundary|rebuild less geometry') { return 'regions' }
            return 'flow'
        }
        3 {
            if ($text -match 'region size|16|32|64|buffers|local choice') { return 'metrics' }
            if ($text -match 'Failure|Failures|retry|Shutdown') { return 'proof' }
            if ($text -match 'obsolete|rejected|retires|stale') { return 'logs' }
            if ($text -match 'CPU preparation|CPU hold|camera, resizes|minimizes') { return 'cpu' }
            if ($text -match 'uploads|uploaded|second barrier|restored') { return 'upload' }
            if ($text -match 'ownership|Empty regions|face oracle|diagonal') { return 'regions' }
            return 'timeline'
        }
        4 {
            if ($text -match '122|187|729|three scene scales|descriptive distributions') { return 'scale' }
            if ($text -match '146 artifacts|verifier|hashes|evidence bundle|validation-enabled|zero Vulkan') { return 'proof' }
            if ($text -match "isn't an editor|picking|free-form|history|streaming|greedy meshing|fixed sequence") { return 'limitations' }
            if ($text -match 'CPU hold|Required is three|254') { return 'cpu' }
            if ($text -match 'final command|Required reaches four|still one') { return 'upload' }
            if ($text -match 'complete does Visible|flash|full-scene|Visible become four') { return 'states' }
            return 'final'
        }
        5 { return 'roadmap' }
        default { return 'title' }
    }
}

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
        $paddedDuration = Get-MediaDuration $paddedPath
        $renderedSegments.Add([pscustomobject]@{
            Beat = $segment.Beat
            Text = $segment.Text
            RawDuration = $rawDuration
            PaddedDuration = $paddedDuration
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
        '-metadata', 'title=Voxel Nexus Devlog 03 - The Voxels Can Finally Change',
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
    ExcludedEvidence = 'demo/before-burst.png (known unrelated artwork)'
}
$summary | ConvertTo-Json | Set-Content -Encoding utf8 (Join-Path $OutputDirectory 'build-summary.json')
$summary | ConvertTo-Json
