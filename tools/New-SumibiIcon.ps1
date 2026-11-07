#Requires -Version 5.1
# Convert the upstream artwork to a Windows ICO containing PNG frames.
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$assets = Join-Path $PSScriptRoot '..\crates\sumibi-host\assets'
$image = [Drawing.Image]::FromFile((Join-Path $assets 'AppIcon.png'))
$sizes = @(16, 20, 24, 32, 40, 48, 64, 96, 128, 256)
try {
    if ($image.Width -ne $image.Height) { throw 'Expected square app artwork.' }
    $frames = foreach ($size in $sizes) {
        $bitmap = [Drawing.Bitmap]::new($size, $size, [Drawing.Imaging.PixelFormat]::Format32bppArgb)
        $stream = [IO.MemoryStream]::new()
        try {
            $graphics = [Drawing.Graphics]::FromImage($bitmap)
            try {
                $graphics.CompositingMode = [Drawing.Drawing2D.CompositingMode]::SourceCopy
                $graphics.InterpolationMode = [Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
                $graphics.DrawImage($image, 0, 0, $size, $size)
            } finally { $graphics.Dispose() }
            $bitmap.Save($stream, [Drawing.Imaging.ImageFormat]::Png)
            ,$stream.ToArray()
        } finally { $stream.Dispose(); $bitmap.Dispose() }
    }
} finally { $image.Dispose() }

$file = [IO.File]::Create((Join-Path $assets 'Sumibi.ico'))
$writer = [IO.BinaryWriter]::new($file)
try {
    $writer.Write([uint16]0) # Reserved
    $writer.Write([uint16]1) # Icon
    $writer.Write([uint16]$sizes.Count)
    $offset = 6 + 16 * $sizes.Count
    for ($i = 0; $i -lt $sizes.Count; $i++) {
        $dimension = [byte]($sizes[$i] % 256) # 0 encodes 256 pixels
        $writer.Write($dimension)
        $writer.Write($dimension)
        $writer.Write([uint16]0) # Palette count and reserved
        $writer.Write([uint16]1) # Color planes
        $writer.Write([uint16]32)
        $writer.Write([uint32]$frames[$i].Length)
        $writer.Write([uint32]$offset)
        $offset += $frames[$i].Length
    }
    foreach ($frame in $frames) { $writer.Write([byte[]]$frame) }
} finally { $writer.Dispose() }
