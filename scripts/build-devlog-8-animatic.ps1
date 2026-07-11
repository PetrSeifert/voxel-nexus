param(
    [string]$OutputDirectory,
    [string]$OutputFileName = 'voxel-nexus-devlog-01-animatic.mp4',
    [ValidateSet('Windows', 'ElevenLabs')]
    [string]$VoiceProvider = 'Windows',
    [int]$VoiceRate = -2,
    [string]$ElevenLabsApiKeyEnvironmentVariable = 'ElevenLabsTTS',
    [string]$ElevenLabsVoiceId = 'yl2ZDV1MzN4HbQJbMihG',
    [string]$ElevenLabsModelId = 'eleven_multilingual_v2',
    [switch]$BurnSubtitles,
    [ValidateRange(0, 2147483647)]
    [int]$MaximumNarrationSegments = 0
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repositoryRoot = Split-Path $PSScriptRoot -Parent
if ([string]::IsNullOrWhiteSpace($OutputDirectory)) {
    $OutputDirectory = Join-Path $repositoryRoot 'artifacts/devlog-8-animatic'
}
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
$narrationDirectory = Join-Path $OutputDirectory 'narration'
$visualDirectory = Join-Path $OutputDirectory 'visuals'
$scriptPath = Join-Path $repositoryRoot 'docs/devlogs/8-robust-vulkan-triangle.md'
$evidenceDirectory = Join-Path $repositoryRoot 'docs/evidence/windows-lifecycle/development-machine'
$elevenLabsCacheDirectory = Join-Path $repositoryRoot "artifacts/elevenlabs-cache/$ElevenLabsVoiceId"
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
        [Parameter(Mandatory)]
        [string]$Command,
        [Parameter(Mandatory)]
        [string[]]$Arguments
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

function Get-TextHash {
    param([Parameter(Mandatory)][string]$Text)

    $hashAlgorithm = [Security.Cryptography.SHA256]::Create()
    try {
        $bytes = [Text.Encoding]::UTF8.GetBytes($Text)
        return [Convert]::ToHexString($hashAlgorithm.ComputeHash($bytes)).ToLowerInvariant()
    }
    finally {
        $hashAlgorithm.Dispose()
    }
}

function Get-ElevenLabsSpeech {
    param(
        [Parameter(Mandatory)][string]$Text,
        [AllowEmptyString()][string]$PreviousText,
        [AllowEmptyString()][string]$NextText,
        [Parameter(Mandatory)][string]$CacheDirectory,
        [Parameter(Mandatory)][string]$ApiKeyEnvironmentVariable,
        [Parameter(Mandatory)][string]$VoiceId,
        [Parameter(Mandatory)][string]$ModelId
    )

    $apiKey = [Environment]::GetEnvironmentVariable($ApiKeyEnvironmentVariable, 'Process')
    if ([string]::IsNullOrWhiteSpace($apiKey)) {
        throw "The $ApiKeyEnvironmentVariable environment variable is not visible to this process."
    }

    New-Item -ItemType Directory -Force $CacheDirectory | Out-Null
    $cacheIdentity = $VoiceId, $ModelId, $Text, $PreviousText, $NextText -join "`n---`n"
    $cachePath = Join-Path $CacheDirectory "$(Get-TextHash $cacheIdentity).mp3"
    if (Test-Path $cachePath) {
        return $cachePath
    }

    $temporaryPath = "$cachePath.download"
    $headers = @{
        'xi-api-key' = $apiKey
        'Content-Type' = 'application/json'
    }
    $request = @{
        text = $Text
        model_id = $ModelId
        apply_text_normalization = 'auto'
    }
    if ($ModelId -ne 'eleven_v3') {
        $request.previous_text = $PreviousText
        $request.next_text = $NextText
    }
    $requestBody = $request | ConvertTo-Json
    $uri = "https://api.elevenlabs.io/v1/text-to-speech/$VoiceId`?output_format=mp3_44100_128"
    Invoke-WebRequest -Method Post -Uri $uri -Headers $headers -Body $requestBody -OutFile $temporaryPath
    if (-not (Test-Path $temporaryPath) -or (Get-Item $temporaryPath).Length -eq 0) {
        throw 'ElevenLabs returned an empty audio response.'
    }
    Move-Item -Force $temporaryPath $cachePath
    return $cachePath
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
                $sentences = [regex]::Split($paragraph.Trim(), '(?<=[.!?])\s+(?=[A-Z“"''])')
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

function New-Canvas {
    $bitmap = [Drawing.Bitmap]::new(1920, 1080)
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    $graphics.SmoothingMode = [Drawing.Drawing2D.SmoothingMode]::AntiAlias
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
    $triangleBrush = [Drawing.SolidBrush]::new($pinkColor)
    $points = [Drawing.PointF[]]@(
        [Drawing.PointF]::new(245, 760),
        [Drawing.PointF]::new(520, 280),
        [Drawing.PointF]::new(795, 760)
    )
    $graphics.FillPolygon($triangleBrush, $points)
    $triangleBrush.Dispose()
    $titleFont = [Drawing.Font]::new('Bahnschrift', 83, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $subtitleFont = [Drawing.Font]::new('Bahnschrift', 38, [Drawing.FontStyle]::Regular, [Drawing.GraphicsUnit]::Pixel)
    $smallFont = [Drawing.Font]::new('Bahnschrift', 25, [Drawing.FontStyle]::Regular, [Drawing.GraphicsUnit]::Pixel)
    Draw-TextBlock $graphics 'THE TRIANGLE IS NOT THE POINT' $titleFont $whiteColor ([Drawing.RectangleF]::new(900, 275, 850, 260))
    Draw-TextBlock $graphics 'Voxel Nexus — Devlog 01' $subtitleFont $pinkColor ([Drawing.RectangleF]::new(905, 560, 800, 70))
    Draw-TextBlock $graphics 'First-pass animatic' $smallFont $mutedColor ([Drawing.RectangleF]::new(908, 655, 600, 50))
    $titleFont.Dispose()
    $subtitleFont.Dispose()
    $smallFont.Dispose()
    Save-Canvas $canvas $Path
}

function New-EvidenceCard {
    param(
        [Parameter(Mandatory)][string]$SourcePath,
        [Parameter(Mandatory)][string]$Label,
        [Parameter(Mandatory)][string]$Path
    )

    $canvas = New-Canvas
    $graphics = $canvas.Graphics
    $labelFont = [Drawing.Font]::new('Bahnschrift', 42, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    Draw-TextBlock $graphics $Label $labelFont $whiteColor ([Drawing.RectangleF]::new(130, 55, 1660, 70)) ([Drawing.StringAlignment]::Center)
    $image = [Drawing.Image]::FromFile($SourcePath)
    $maximumWidth = 1600.0
    $maximumHeight = 820.0
    $scale = [Math]::Min($maximumWidth / $image.Width, $maximumHeight / $image.Height)
    $width = [float]($image.Width * $scale)
    $height = [float]($image.Height * $scale)
    $x = [float]((1920 - $width) / 2)
    $y = [float](150 + (($maximumHeight - $height) / 2))
    $shadowBrush = [Drawing.SolidBrush]::new([Drawing.Color]::FromArgb(90, 0, 0, 0))
    $graphics.FillRectangle($shadowBrush, $x + 16, $y + 16, $width, $height)
    $shadowBrush.Dispose()
    $graphics.DrawImage($image, [Drawing.RectangleF]::new($x, $y, $width, $height))
    $image.Dispose()
    $labelFont.Dispose()
    Save-Canvas $canvas $Path
}

function New-RoadmapCard {
    param(
        [Parameter(Mandatory)][int]$ActiveStep,
        [Parameter(Mandatory)][string]$Path
    )

    $canvas = New-Canvas
    $graphics = $canvas.Graphics
    $titleFont = [Drawing.Font]::new('Bahnschrift', 58, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $boxFont = [Drawing.Font]::new('Bahnschrift', 28, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $smallFont = [Drawing.Font]::new('Bahnschrift', 24, [Drawing.FontStyle]::Regular, [Drawing.GraphicsUnit]::Pixel)
    Draw-TextBlock $graphics 'One scene. Multiple ways to store and render it.' $titleFont $whiteColor ([Drawing.RectangleF]::new(150, 90, 1620, 100)) ([Drawing.StringAlignment]::Center)
    $steps = @(
        @('1', 'Render Backend', 'Vulkan lifecycle'),
        @('2', 'Dense raster', 'first voxel scene'),
        @('3', 'Editable raster', 'incremental updates'),
        @('4', 'Compute ray', 'second Render Path'),
        @('5', 'Storage independence', '2 paths × 2 tiers')
    )
    $boxWidth = 292
    $boxHeight = 250
    $gap = 58
    $startX = 92
    for ($index = 0; $index -lt $steps.Count; $index++) {
        $x = $startX + ($index * ($boxWidth + $gap))
        $color = if (($index + 1) -le $ActiveStep) { $pinkColor } else { $panelColor }
        $brush = [Drawing.SolidBrush]::new($color)
        $graphics.FillRectangle($brush, $x, 365, $boxWidth, $boxHeight)
        $brush.Dispose()
        Draw-TextBlock $graphics $steps[$index][0] $titleFont $backgroundColor ([Drawing.RectangleF]::new($x + 15, 385, $boxWidth - 30, 65)) ([Drawing.StringAlignment]::Center)
        Draw-TextBlock $graphics $steps[$index][1] $boxFont $whiteColor ([Drawing.RectangleF]::new($x + 18, 470, $boxWidth - 36, 65)) ([Drawing.StringAlignment]::Center)
        Draw-TextBlock $graphics $steps[$index][2] $smallFont $whiteColor ([Drawing.RectangleF]::new($x + 18, 545, $boxWidth - 36, 55)) ([Drawing.StringAlignment]::Center)
        if ($index -lt ($steps.Count - 1)) {
            Draw-Arrow $graphics ($x + $boxWidth + 8) 490 ($x + $boxWidth + $gap - 8) 490
        }
    }
    Draw-TextBlock $graphics 'Phase destination: storage and rendering choices share stable scene semantics.' $smallFont $mutedColor ([Drawing.RectangleF]::new(240, 760, 1440, 80)) ([Drawing.StringAlignment]::Center)
    $titleFont.Dispose()
    $boxFont.Dispose()
    $smallFont.Dispose()
    Save-Canvas $canvas $Path
}

function New-ArchitectureCard {
    param([Parameter(Mandatory)][string]$Path)

    $canvas = New-Canvas
    $graphics = $canvas.Graphics
    $titleFont = [Drawing.Font]::new('Bahnschrift', 58, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $boxFont = [Drawing.Font]::new('Bahnschrift', 31, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $smallFont = [Drawing.Font]::new('Bahnschrift', 24, [Drawing.FontStyle]::Regular, [Drawing.GraphicsUnit]::Pixel)
    Draw-TextBlock $graphics 'Keep the platform boundary narrow' $titleFont $whiteColor ([Drawing.RectangleF]::new(150, 90, 1620, 100)) ([Drawing.StringAlignment]::Center)
    $boxes = @(
        @('Desktop demo', 'window + event loop', 120),
        @('Windows adapter', 'window and surface handles', 555),
        @('Portable Render Backend', 'device • commands • sync • presentation', 990),
        @('Vulkan', 'graphics execution', 1425)
    )
    foreach ($box in $boxes) {
        $brush = [Drawing.SolidBrush]::new($(if ($box[0] -eq 'Portable Render Backend') { $pinkColor } else { $panelColor }))
        $graphics.FillRectangle($brush, [int]$box[2], 370, 360, 260)
        $brush.Dispose()
        Draw-TextBlock $graphics $box[0] $boxFont $whiteColor ([Drawing.RectangleF]::new([int]$box[2] + 20, 415, 320, 75)) ([Drawing.StringAlignment]::Center)
        Draw-TextBlock $graphics $box[1] $smallFont $whiteColor ([Drawing.RectangleF]::new([int]$box[2] + 22, 525, 316, 60)) ([Drawing.StringAlignment]::Center)
    }
    Draw-Arrow $graphics 490 500 545 500
    Draw-Arrow $graphics 925 500 980 500
    Draw-Arrow $graphics 1360 500 1415 500
    Draw-TextBlock $graphics 'The triangle is a smoke workload, not a premature Render Path API.' $smallFont $mutedColor ([Drawing.RectangleF]::new(300, 760, 1320, 70)) ([Drawing.StringAlignment]::Center)
    $titleFont.Dispose()
    $boxFont.Dispose()
    $smallFont.Dispose()
    Save-Canvas $canvas $Path
}

function New-MotivationCard {
    param([Parameter(Mandatory)][string]$Path)

    $canvas = New-Canvas
    $graphics = $canvas.Graphics
    $eyebrowFont = [Drawing.Font]::new('Bahnschrift', 30, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $titleFont = [Drawing.Font]::new('Bahnschrift', 66, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $bodyFont = [Drawing.Font]::new('Bahnschrift', 37, [Drawing.FontStyle]::Regular, [Drawing.GraphicsUnit]::Pixel)
    Draw-TextBlock $graphics 'WHY VOXEL NEXUS' $eyebrowFont $pinkColor ([Drawing.RectangleF]::new(185, 170, 1550, 55))
    Draw-TextBlock $graphics 'Make different voxel techniques easy to explore.' $titleFont $whiteColor ([Drawing.RectangleF]::new(180, 265, 1500, 180))
    Draw-TextBlock $graphics 'A general-purpose foundation where storage and rendering techniques can change without rebuilding everything around them.' $bodyFont $mutedColor ([Drawing.RectangleF]::new(185, 540, 1460, 210))
    $eyebrowFont.Dispose()
    $titleFont.Dispose()
    $bodyFont.Dispose()
    Save-Canvas $canvas $Path
}

function New-LifetimeCard {
    param([Parameter(Mandatory)][string]$Path)

    $canvas = New-Canvas
    $graphics = $canvas.Graphics
    $titleFont = [Drawing.Font]::new('Bahnschrift', 55, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $codeFont = [Drawing.Font]::new('Consolas', 34, [Drawing.FontStyle]::Regular, [Drawing.GraphicsUnit]::Pixel)
    $smallFont = [Drawing.Font]::new('Bahnschrift', 26, [Drawing.FontStyle]::Regular, [Drawing.GraphicsUnit]::Pixel)
    Draw-TextBlock $graphics 'A one-line lifetime fix' $titleFont $whiteColor ([Drawing.RectangleF]::new(150, 90, 1620, 90)) ([Drawing.StringAlignment]::Center)
    $panelBrush = [Drawing.SolidBrush]::new($panelMutedColor)
    $graphics.FillRectangle($panelBrush, 250, 285, 1420, 475)
    $panelBrush.Dispose()
    Draw-TextBlock $graphics "struct DesktopApplication {`n-    window: Option<Window>,`n     backend: Option<RenderBackend>,`n+    window: Option<Window>,`n}" $codeFont $whiteColor ([Drawing.RectangleF]::new(345, 355, 1240, 330))
    Draw-TextBlock $graphics 'Drop the backend and Vulkan surface before the window they depend on.' $smallFont $mutedColor ([Drawing.RectangleF]::new(280, 840, 1360, 60)) ([Drawing.StringAlignment]::Center)
    $titleFont.Dispose()
    $codeFont.Dispose()
    $smallFont.Dispose()
    Save-Canvas $canvas $Path
}

function New-SynchronizationCard {
    param([Parameter(Mandatory)][string]$Path)

    $canvas = New-Canvas
    $graphics = $canvas.Graphics
    $titleFont = [Drawing.Font]::new('Bahnschrift', 55, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $boxFont = [Drawing.Font]::new('Bahnschrift', 28, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $codeFont = [Drawing.Font]::new('Consolas', 30, [Drawing.FontStyle]::Regular, [Drawing.GraphicsUnit]::Pixel)
    Draw-TextBlock $graphics 'Synchronize with the work that really owns the lifetime' $titleFont $whiteColor ([Drawing.RectangleF]::new(130, 85, 1660, 100)) ([Drawing.StringAlignment]::Center)
    for ($index = 0; $index -lt 3; $index++) {
        $x = 230 + ($index * 510)
        $brush = [Drawing.SolidBrush]::new($(if ($index -eq 1) { $pinkColor } else { $panelColor }))
        $graphics.FillRectangle($brush, $x, 330, 430, 230)
        $brush.Dispose()
        Draw-TextBlock $graphics "Swapchain image $index" $boxFont $whiteColor ([Drawing.RectangleF]::new($x + 20, 365, 390, 55)) ([Drawing.StringAlignment]::Center)
        Draw-TextBlock $graphics "render_finished[$index]" $codeFont $whiteColor ([Drawing.RectangleF]::new($x + 20, 465, 390, 55)) ([Drawing.StringAlignment]::Center)
    }
    Draw-TextBlock $graphics 'The semaphore follows the acquired image instead of being reused after only a CPU fence.' $boxFont $mutedColor ([Drawing.RectangleF]::new(260, 725, 1400, 90)) ([Drawing.StringAlignment]::Center)
    $titleFont.Dispose()
    $boxFont.Dispose()
    $codeFont.Dispose()
    Save-Canvas $canvas $Path
}

function New-LifecycleCard {
    param([Parameter(Mandatory)][string]$Path)

    $canvas = New-Canvas
    $graphics = $canvas.Graphics
    $titleFont = [Drawing.Font]::new('Bahnschrift', 58, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $boxFont = [Drawing.Font]::new('Bahnschrift', 27, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $smallFont = [Drawing.Font]::new('Bahnschrift', 23, [Drawing.FontStyle]::Regular, [Drawing.GraphicsUnit]::Pixel)
    Draw-TextBlock $graphics 'Window lifecycle is state, not failure' $titleFont $whiteColor ([Drawing.RectangleF]::new(160, 90, 1600, 90)) ([Drawing.StringAlignment]::Center)
    $states = @(
        @('Presenting', 'acquire + draw', 120, $greenColor),
        @('Invalidated', 'latest extent wins', 550, $panelColor),
        @('Suspended', 'zero size • sleep', 980, $pinkColor),
        @('Restored', 'rebuild + resume', 1410, $greenColor)
    )
    foreach ($state in $states) {
        $brush = [Drawing.SolidBrush]::new($state[3])
        $graphics.FillRectangle($brush, [int]$state[2], 350, 340, 240)
        $brush.Dispose()
        Draw-TextBlock $graphics $state[0] $boxFont $whiteColor ([Drawing.RectangleF]::new([int]$state[2] + 20, 395, 300, 55)) ([Drawing.StringAlignment]::Center)
        Draw-TextBlock $graphics $state[1] $smallFont $whiteColor ([Drawing.RectangleF]::new([int]$state[2] + 20, 500, 300, 45)) ([Drawing.StringAlignment]::Center)
    }
    Draw-Arrow $graphics 470 470 540 470
    Draw-Arrow $graphics 900 470 970 470
    Draw-Arrow $graphics 1330 470 1400 470
    Draw-TextBlock $graphics 'No acquire at zero size • no busy-loop while minimized • retry transient surface suspension' $smallFont $mutedColor ([Drawing.RectangleF]::new(220, 760, 1480, 80)) ([Drawing.StringAlignment]::Center)
    $titleFont.Dispose()
    $boxFont.Dispose()
    $smallFont.Dispose()
    Save-Canvas $canvas $Path
}

function New-DiagnosticsCard {
    param([Parameter(Mandatory)][string]$Path)

    $canvas = New-Canvas
    $graphics = $canvas.Graphics
    $titleFont = [Drawing.Font]::new('Bahnschrift', 52, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $headingFont = [Drawing.Font]::new('Bahnschrift', 29, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $codeFont = [Drawing.Font]::new('Consolas', 24, [Drawing.FontStyle]::Regular, [Drawing.GraphicsUnit]::Pixel)
    Draw-TextBlock $graphics 'Unsupported should still be actionable' $titleFont $whiteColor ([Drawing.RectangleF]::new(150, 80, 1620, 90)) ([Drawing.StringAlignment]::Center)
    $panelBrush = [Drawing.SolidBrush]::new($panelMutedColor)
    $graphics.FillRectangle($panelBrush, 105, 245, 815, 610)
    $graphics.FillRectangle($panelBrush, 1000, 245, 815, 610)
    $panelBrush.Dispose()
    Draw-TextBlock $graphics 'VULKAN 1.2 CANDIDATE' $headingFont $pinkColor ([Drawing.RectangleF]::new(150, 285, 720, 55))
    Draw-TextBlock $graphics 'Requires Vulkan 1.3 or newer. Update the graphics driver or use a Vulkan 1.3-capable GPU.' $codeFont $whiteColor ([Drawing.RectangleF]::new(150, 375, 700, 260))
    Draw-TextBlock $graphics 'EXIT 1  •  NO PANIC' $headingFont $greenColor ([Drawing.RectangleF]::new(150, 735, 700, 55))
    Draw-TextBlock $graphics 'MISSING PRESENTATION' $headingFont $pinkColor ([Drawing.RectangleF]::new(1045, 285, 720, 55))
    Draw-TextBlock $graphics 'Reports missing swapchain, formats, modes, and presentation queue. Suggests driver, GPU, or desktop-session support.' $codeFont $whiteColor ([Drawing.RectangleF]::new(1045, 375, 700, 300))
    Draw-TextBlock $graphics 'EXIT 1  •  UNDER 5 SECONDS' $headingFont $greenColor ([Drawing.RectangleF]::new(1045, 735, 700, 55))
    $titleFont.Dispose()
    $headingFont.Dispose()
    $codeFont.Dispose()
    Save-Canvas $canvas $Path
}

function New-ProofCard {
    param([Parameter(Mandatory)][string]$Path)

    $canvas = New-Canvas
    $graphics = $canvas.Graphics
    $titleFont = [Drawing.Font]::new('Bahnschrift', 52, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $numberFont = [Drawing.Font]::new('Bahnschrift', 190, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $labelFont = [Drawing.Font]::new('Bahnschrift', 31, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $detailFont = [Drawing.Font]::new('Consolas', 25, [Drawing.FontStyle]::Regular, [Drawing.GraphicsUnit]::Pixel)
    Draw-TextBlock $graphics 'Auditable Windows lifecycle proof' $titleFont $whiteColor ([Drawing.RectangleF]::new(150, 70, 1620, 90)) ([Drawing.StringAlignment]::Center)
    Draw-TextBlock $graphics '0' $numberFont $pinkColor ([Drawing.RectangleF]::new(180, 260, 430, 245)) ([Drawing.StringAlignment]::Center)
    Draw-TextBlock $graphics 'validation errors' $labelFont $whiteColor ([Drawing.RectangleF]::new(170, 520, 450, 60)) ([Drawing.StringAlignment]::Center)
    Draw-TextBlock $graphics '0' $numberFont $greenColor ([Drawing.RectangleF]::new(745, 260, 430, 245)) ([Drawing.StringAlignment]::Center)
    Draw-TextBlock $graphics 'validation warnings' $labelFont $whiteColor ([Drawing.RectangleF]::new(700, 520, 520, 60)) ([Drawing.StringAlignment]::Center)
    Draw-TextBlock $graphics '13' $numberFont $whiteColor ([Drawing.RectangleF]::new(1320, 260, 430, 245)) ([Drawing.StringAlignment]::Center)
    Draw-TextBlock $graphics 'tests passed' $labelFont $whiteColor ([Drawing.RectangleF]::new(1320, 520, 430, 60)) ([Drawing.StringAlignment]::Center)
    $details = "Windows 11 Pro  •  NVIDIA GeForce RTX 4070  •  Vulkan 1.4.329`nLifecycle exit 0  •  Unsupported cases exit 1  •  Revision 88fae9d"
    Draw-TextBlock $graphics $details $detailFont $mutedColor ([Drawing.RectangleF]::new(250, 720, 1420, 120)) ([Drawing.StringAlignment]::Center)
    $titleFont.Dispose()
    $numberFont.Dispose()
    $labelFont.Dispose()
    $detailFont.Dispose()
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
            if ($Index -eq 0) { return 'title' }
            $hookVisuals = @('launch', 'landscape', 'portrait', 'restored', 'proof')
            return $hookVisuals[($Index - 1) % $hookVisuals.Count]
        }
        2 {
            if ($text -match 'always been drawn|general-purpose engine') { return 'motivation' }
            if ($text -match 'boundary|backend owns|adapter supplies|smoke test|renderer abstraction') { return 'architecture' }
            return 'roadmap'
        }
        3 {
            if ($text -match 'lifetime trap|struct fields|one-line reorder') { return 'lifetime' }
            if ($text -match 'semaphore|CPU fence|swapchain images|synchronization problem') { return 'synchronization' }
            if ($text -match 'failure|rejected GPU|qualification path|Vulkan 1\.2|deterministic injected|actionable error') { return 'diagnostics' }
            if ($text -match 'pink triangle|generated shaders') { return 'launch' }
            if ($text -match 'portrait') { return 'portrait' }
            return 'lifecycle'
        }
        4 {
            if ($text -match 'unsupported') { return 'diagnostics' }
            if ($text -match 'zero validation|clean clone|Thirteen|evidence lives|proof establishes|recorded revision|runtime context') { return 'proof' }
            if ($text -match 'pink triangle launches') { return 'launch' }
            if ($text -match 'landscape and portrait') { return 'landscape' }
            $payoffVisuals = @('launch', 'landscape', 'portrait', 'restored', 'proof')
            return $payoffVisuals[$Index % $payoffVisuals.Count]
        }
        5 { return 'roadmap-next' }
        default { return 'title' }
    }
}

$visualPaths = @{
    title = Join-Path $visualDirectory 'title.png'
    launch = Join-Path $visualDirectory 'launch.png'
    landscape = Join-Path $visualDirectory 'landscape.png'
    portrait = Join-Path $visualDirectory 'portrait.png'
    restored = Join-Path $visualDirectory 'restored.png'
    roadmap = Join-Path $visualDirectory 'roadmap.png'
    'roadmap-next' = Join-Path $visualDirectory 'roadmap-next.png'
    architecture = Join-Path $visualDirectory 'architecture.png'
    motivation = Join-Path $visualDirectory 'motivation.png'
    lifetime = Join-Path $visualDirectory 'lifetime.png'
    synchronization = Join-Path $visualDirectory 'synchronization.png'
    lifecycle = Join-Path $visualDirectory 'lifecycle.png'
    diagnostics = Join-Path $visualDirectory 'diagnostics.png'
    proof = Join-Path $visualDirectory 'proof.png'
}

New-TitleCard $visualPaths.title
New-EvidenceCard (Join-Path $evidenceDirectory 'launch-a.png') 'Launch' $visualPaths.launch
New-EvidenceCard (Join-Path $evidenceDirectory 'landscape-b.png') 'Landscape resize' $visualPaths.landscape
New-EvidenceCard (Join-Path $evidenceDirectory 'portrait-a.png') 'Portrait resize' $visualPaths.portrait
New-EvidenceCard (Join-Path $evidenceDirectory 'restored-b.png') 'Restored after minimize' $visualPaths.restored
New-RoadmapCard 1 $visualPaths.roadmap
New-RoadmapCard 2 $visualPaths['roadmap-next']
New-ArchitectureCard $visualPaths.architecture
New-MotivationCard $visualPaths.motivation
New-LifetimeCard $visualPaths.lifetime
New-SynchronizationCard $visualPaths.synchronization
New-LifecycleCard $visualPaths.lifecycle
New-DiagnosticsCard $visualPaths.diagnostics
New-ProofCard $visualPaths.proof

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

$synthesizer = $null
if ($VoiceProvider -eq 'Windows') {
    $synthesizer = [Speech.Synthesis.SpeechSynthesizer]::new()
    $synthesizer.SelectVoice('Microsoft David Desktop')
    $synthesizer.Rate = $VoiceRate
}

$renderedSegments = [Collections.Generic.List[object]]::new()
try {
    for ($index = 0; $index -lt $segments.Count; $index++) {
        $segment = $segments[$index]
        $baseName = 'segment-{0:000}' -f ($index + 1)
        $paddedPath = Join-Path $narrationDirectory "$baseName.wav"
        if ($VoiceProvider -eq 'Windows') {
            $rawPath = Join-Path $narrationDirectory "$baseName-raw.wav"
            $synthesizer.SetOutputToWaveFile($rawPath)
            $synthesizer.Speak($segment.Text)
            $synthesizer.SetOutputToNull()
        }
        else {
            $previousText = if ($index -gt 0) { $allSegments[$index - 1].Text } else { '' }
            $nextText = if (($index + 1) -lt $allSegments.Count) { $allSegments[$index + 1].Text } else { '' }
            $rawPath = Get-ElevenLabsSpeech `
                -Text $segment.Text `
                -PreviousText $previousText `
                -NextText $nextText `
                -CacheDirectory $elevenLabsCacheDirectory `
                -ApiKeyEnvironmentVariable $ElevenLabsApiKeyEnvironmentVariable `
                -VoiceId $ElevenLabsVoiceId `
                -ModelId $ElevenLabsModelId
        }
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
    if ($null -ne $synthesizer) {
        $synthesizer.Dispose()
    }
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
for ($index = 0; $index -lt $renderedSegments.Count; $index++) {
    $segment = $renderedSegments[$index]
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
        '-metadata', 'title=Voxel Nexus Devlog 01 - The Triangle Is Not the Point',
        $outputVideoPath
    )
}
finally {
    Pop-Location
}

$videoDuration = Get-MediaDuration $outputVideoPath
$summary = [ordered]@{
    Output = $outputVideoPath
    VoiceProvider = $VoiceProvider
    Voice = if ($VoiceProvider -eq 'Windows') { 'Microsoft David Desktop' } else { $ElevenLabsVoiceId }
    VoiceRate = if ($VoiceProvider -eq 'Windows') { $VoiceRate } else { $null }
    VoiceModel = if ($VoiceProvider -eq 'ElevenLabs') { $ElevenLabsModelId } else { $null }
    NarrationSegments = $renderedSegments.Count
    DurationSeconds = [Math]::Round($videoDuration, 3)
    Resolution = '1920x1080'
    FrameRate = 30
    BurnedSubtitles = [bool]$BurnSubtitles
    Music = 'None'
}
$summary | ConvertTo-Json | Set-Content -Encoding utf8 (Join-Path $OutputDirectory 'build-summary.json')
$summary | ConvertTo-Json
