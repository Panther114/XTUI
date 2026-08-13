param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string] $InputSvg,

    [Parameter(Mandatory = $true, Position = 1)]
    [string] $OutputPng
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

$culture = [Globalization.CultureInfo]::InvariantCulture
$svgPath = (Resolve-Path -LiteralPath $InputSvg).Path
[xml] $document = Get-Content -LiteralPath $svgPath -Raw -Encoding utf8
$root = $document.DocumentElement
$width = [int]::Parse($root.GetAttribute('width'), $culture)
$height = [int]::Parse($root.GetAttribute('height'), $culture)

$bitmap = [Drawing.Bitmap]::new($width, $height, [Drawing.Imaging.PixelFormat]::Format32bppArgb)
$graphics = [Drawing.Graphics]::FromImage($bitmap)
$graphics.Clear([Drawing.Color]::Black)
$graphics.SmoothingMode = [Drawing.Drawing2D.SmoothingMode]::HighQuality
$graphics.TextRenderingHint = [Drawing.Text.TextRenderingHint]::AntiAliasGridFit

function Read-Number([System.Xml.XmlElement] $node, [string] $name, [double] $fallback = 0) {
    $value = $node.GetAttribute($name)
    if ([string]::IsNullOrWhiteSpace($value)) { return $fallback }
    return [double]::Parse($value, $culture)
}

function Read-Color([System.Xml.XmlElement] $node) {
    $color = [Drawing.ColorTranslator]::FromHtml($node.GetAttribute('fill'))
    $opacityValue = $node.GetAttribute('fill-opacity')
    if (-not [string]::IsNullOrWhiteSpace($opacityValue)) {
        $opacity = [double]::Parse($opacityValue, $culture)
        $color = [Drawing.Color]::FromArgb([int](255 * $opacity), $color)
    }
    return $color
}

try {
    foreach ($node in $document.SelectNodes("//*[local-name()='rect']")) {
        $x = Read-Number $node 'x'
        $y = Read-Number $node 'y'
        $nodeWidth = if ($node.GetAttribute('width') -eq '100%') { $width } else { Read-Number $node 'width' }
        $nodeHeight = if ($node.GetAttribute('height') -eq '100%') { $height } else { Read-Number $node 'height' }
        $brush = [Drawing.SolidBrush]::new((Read-Color $node))
        try {
            $graphics.FillRectangle($brush, [single]$x, [single]$y, [single]$nodeWidth, [single]$nodeHeight)
        } finally {
            $brush.Dispose()
        }
    }

    $fontSize = Read-Number $root.SelectSingleNode("//*[local-name()='g']") 'font-size' 15
    $regularFont = [Drawing.Font]::new('Cascadia Mono', [single]$fontSize, [Drawing.FontStyle]::Regular, [Drawing.GraphicsUnit]::Pixel)
    $boldFont = [Drawing.Font]::new('Cascadia Mono', [single]$fontSize, [Drawing.FontStyle]::Bold, [Drawing.GraphicsUnit]::Pixel)
    $format = [Drawing.StringFormat]::new([Drawing.StringFormat]::GenericTypographic)
    $format.Alignment = [Drawing.StringAlignment]::Center
    $format.LineAlignment = [Drawing.StringAlignment]::Near
    $format.FormatFlags = $format.FormatFlags -bor [Drawing.StringFormatFlags]::NoWrap -bor [Drawing.StringFormatFlags]::NoClip

    try {
        foreach ($node in $document.SelectNodes("//*[local-name()='text']")) {
            $x = Read-Number $node 'x'
            $baseline = Read-Number $node 'y'
            $font = if ($node.GetAttribute('font-weight') -eq '700') { $boldFont } else { $regularFont }
            $brush = [Drawing.SolidBrush]::new((Read-Color $node))
            try {
                $graphics.DrawString(
                    $node.InnerText,
                    $font,
                    $brush,
                    [single]$x,
                    [single]($baseline - ($fontSize * 0.88)),
                    $format
                )
            } finally {
                $brush.Dispose()
            }
        }
    } finally {
        $format.Dispose()
        $boldFont.Dispose()
        $regularFont.Dispose()
    }

    $outputPath = [IO.Path]::GetFullPath((Join-Path (Get-Location) $OutputPng))
    $outputDirectory = [IO.Path]::GetDirectoryName($outputPath)
    [IO.Directory]::CreateDirectory($outputDirectory) | Out-Null
    $bitmap.Save($outputPath, [Drawing.Imaging.ImageFormat]::Png)
    Write-Output $outputPath
} finally {
    $graphics.Dispose()
    $bitmap.Dispose()
}
