param(
    [string]$OutputDirectory = (Join-Path $PSScriptRoot "..\assets")
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

Add-Type -AssemblyName System.Drawing

function New-RoundedRectanglePath {
    param(
        [System.Drawing.RectangleF]$Rectangle,
        [float]$Radius
    )

    $diameter = $Radius * 2.0
    $path = [System.Drawing.Drawing2D.GraphicsPath]::new()
    $path.AddArc($Rectangle.X, $Rectangle.Y, $diameter, $diameter, 180, 90)
    $path.AddArc($Rectangle.Right - $diameter, $Rectangle.Y, $diameter, $diameter, 270, 90)
    $path.AddArc(
        $Rectangle.Right - $diameter,
        $Rectangle.Bottom - $diameter,
        $diameter,
        $diameter,
        0,
        90
    )
    $path.AddArc($Rectangle.X, $Rectangle.Bottom - $diameter, $diameter, $diameter, 90, 90)
    $path.CloseFigure()
    return $path
}

function New-AppIconBitmap {
    param([int]$Size)

    $scale = $Size / 256.0
    $bitmap = [System.Drawing.Bitmap]::new(
        $Size,
        $Size,
        [System.Drawing.Imaging.PixelFormat]::Format32bppArgb
    )
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $graphics.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $graphics.Clear([System.Drawing.Color]::Transparent)

    $background = [System.Drawing.RectangleF]::new(
        [float](8 * $scale),
        [float](8 * $scale),
        [float](240 * $scale),
        [float](240 * $scale)
    )
    $backgroundPath = New-RoundedRectanglePath $background ([float](45 * $scale))
    $backgroundBrush = [System.Drawing.SolidBrush]::new(
        [System.Drawing.ColorTranslator]::FromHtml("#0F766E")
    )
    $graphics.FillPath($backgroundBrush, $backgroundPath)

    $backgroundPen = [System.Drawing.Pen]::new(
        [System.Drawing.ColorTranslator]::FromHtml("#12363B"),
        [float]([Math]::Max(1.0, 5.0 * $scale))
    )
    $graphics.DrawPath($backgroundPen, $backgroundPath)

    $sheet = [System.Drawing.RectangleF]::new(
        [float](43 * $scale),
        [float](33 * $scale),
        [float](148 * $scale),
        [float](187 * $scale)
    )
    $sheetPath = New-RoundedRectanglePath $sheet ([float](10 * $scale))
    $sheetBrush = [System.Drawing.SolidBrush]::new(
        [System.Drawing.ColorTranslator]::FromHtml("#F8FBFA")
    )
    $graphics.FillPath($sheetBrush, $sheetPath)

    $sheetPen = [System.Drawing.Pen]::new(
        [System.Drawing.ColorTranslator]::FromHtml("#12363B"),
        [float]([Math]::Max(1.0, 5.0 * $scale))
    )
    $graphics.DrawPath($sheetPen, $sheetPath)

    $headerBrush = [System.Drawing.SolidBrush]::new(
        [System.Drawing.ColorTranslator]::FromHtml("#CFEAE6")
    )
    $graphics.FillRectangle(
        $headerBrush,
        [float](57 * $scale),
        [float](50 * $scale),
        [float](120 * $scale),
        [float](29 * $scale)
    )

    $gridPen = [System.Drawing.Pen]::new(
        [System.Drawing.ColorTranslator]::FromHtml("#5CA9A1"),
        [float]([Math]::Max(1.0, 4.0 * $scale))
    )
    foreach ($x in @(57, 97, 137, 177)) {
        $graphics.DrawLine(
            $gridPen,
            [float]($x * $scale),
            [float](50 * $scale),
            [float]($x * $scale),
            [float](199 * $scale)
        )
    }
    foreach ($y in @(50, 79, 119, 159, 199)) {
        $graphics.DrawLine(
            $gridPen,
            [float](57 * $scale),
            [float]($y * $scale),
            [float](177 * $scale),
            [float]($y * $scale)
        )
    }

    $badgeBrush = [System.Drawing.SolidBrush]::new(
        [System.Drawing.ColorTranslator]::FromHtml("#D99A2B")
    )
    $badgePen = [System.Drawing.Pen]::new(
        [System.Drawing.Color]::White,
        [float]([Math]::Max(1.0, 7.0 * $scale))
    )
    $graphics.FillEllipse(
        $badgeBrush,
        [float](116 * $scale),
        [float](123 * $scale),
        [float](112 * $scale),
        [float](112 * $scale)
    )
    $graphics.DrawEllipse(
        $badgePen,
        [float](116 * $scale),
        [float](123 * $scale),
        [float](112 * $scale),
        [float](112 * $scale)
    )

    $checkPen = [System.Drawing.Pen]::new(
        [System.Drawing.Color]::White,
        [float]([Math]::Max(1.0, 13.0 * $scale))
    )
    $checkPen.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
    $checkPen.EndCap = [System.Drawing.Drawing2D.LineCap]::Round
    $checkPen.LineJoin = [System.Drawing.Drawing2D.LineJoin]::Round
    $graphics.DrawLines(
        $checkPen,
        [System.Drawing.PointF[]]@(
            [System.Drawing.PointF]::new([float](141 * $scale), [float](178 * $scale)),
            [System.Drawing.PointF]::new([float](163 * $scale), [float](200 * $scale)),
            [System.Drawing.PointF]::new([float](204 * $scale), [float](153 * $scale))
        )
    )

    $checkPen.Dispose()
    $badgePen.Dispose()
    $badgeBrush.Dispose()
    $gridPen.Dispose()
    $headerBrush.Dispose()
    $sheetPen.Dispose()
    $sheetBrush.Dispose()
    $sheetPath.Dispose()
    $backgroundPen.Dispose()
    $backgroundBrush.Dispose()
    $backgroundPath.Dispose()
    $graphics.Dispose()

    return $bitmap
}

$outputPath = [System.IO.Path]::GetFullPath($OutputDirectory)
[System.IO.Directory]::CreateDirectory($outputPath) | Out-Null

$pngPath = Join-Path $outputPath "app-icon.png"
$pngBitmap = New-AppIconBitmap 256
$pngBitmap.Save($pngPath, [System.Drawing.Imaging.ImageFormat]::Png)
$pngBitmap.Dispose()

$iconSizes = @(16, 20, 24, 32, 40, 48, 64, 128, 256)
$iconEntries = foreach ($size in $iconSizes) {
    $bitmap = New-AppIconBitmap $size
    $stream = [System.IO.MemoryStream]::new()
    $bitmap.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png)
    $bytes = $stream.ToArray()
    $stream.Dispose()
    $bitmap.Dispose()
    [PSCustomObject]@{
        Size = $size
        Bytes = $bytes
    }
}

$icoPath = Join-Path $outputPath "app-icon.ico"
$fileStream = [System.IO.File]::Open(
    $icoPath,
    [System.IO.FileMode]::Create,
    [System.IO.FileAccess]::Write,
    [System.IO.FileShare]::None
)
$writer = [System.IO.BinaryWriter]::new($fileStream)
$writer.Write([UInt16]0)
$writer.Write([UInt16]1)
$writer.Write([UInt16]$iconEntries.Count)

$imageOffset = 6 + (16 * $iconEntries.Count)
foreach ($entry in $iconEntries) {
    $dimension = if ($entry.Size -eq 256) { 0 } else { $entry.Size }
    $writer.Write([Byte]$dimension)
    $writer.Write([Byte]$dimension)
    $writer.Write([Byte]0)
    $writer.Write([Byte]0)
    $writer.Write([UInt16]1)
    $writer.Write([UInt16]32)
    $writer.Write([UInt32]$entry.Bytes.Length)
    $writer.Write([UInt32]$imageOffset)
    $imageOffset += $entry.Bytes.Length
}
foreach ($entry in $iconEntries) {
    $writer.Write([Byte[]]$entry.Bytes)
}
$writer.Dispose()
$fileStream.Dispose()

Write-Output "Generated $pngPath"
Write-Output "Generated $icoPath"
