# 生成应用图标：蓝色圆角方块 + 白色 "T"
Add-Type -AssemblyName System.Drawing

$ErrorActionPreference = 'Stop'

function New-IconBitmap([int]$size) {
    $bmp = New-Object System.Drawing.Bitmap($size, $size)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAliasGridFit
    $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $g.Clear([System.Drawing.Color]::Transparent)

    # 圆角矩形
    $margin = [int]($size * 3 / 100)
    if ($margin -lt 1) { $margin = 1 }
    $radius = [int]($size * 19 / 100)
    $innerSize = $size - ($margin * 2)
    $rect = New-Object System.Drawing.Rectangle($margin, $margin, $innerSize, $innerSize)
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $path.AddArc($rect.X, $rect.Y, $radius, $radius, 180, 90)
    $path.AddArc($rect.Right - $radius, $rect.Y, $radius, $radius, 270, 90)
    $path.AddArc($rect.Right - $radius, $rect.Bottom - $radius, $radius, $radius, 0, 90)
    $path.AddArc($rect.X, $rect.Bottom - $radius, $radius, $radius, 90, 90)
    $path.CloseFigure()

    $brush = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(0, 174, 236))
    $g.FillPath($brush, $path)

    # "T" 字
    $fontSize = [int]($size * 56 / 100)
    if ($fontSize -lt 8) { $fontSize = 8 }
    $font = New-Object System.Drawing.Font("Microsoft YaHei UI", [float]$fontSize, [System.Drawing.FontStyle]::Bold, [System.Drawing.GraphicsUnit]::Pixel)
    $white = [System.Drawing.Brushes]::White
    $sf = New-Object System.Drawing.StringFormat
    $sf.Alignment = [System.Drawing.StringAlignment]::Center
    $sf.LineAlignment = [System.Drawing.StringAlignment]::Center
    $yOffset = -[int]($size * 4 / 100)
    $textRect = New-Object System.Drawing.RectangleF(0, $yOffset, $size, $size)
    $g.DrawString("T", $font, $white, $textRect, $sf)

    $g.Dispose()
    return $bmp
}

$iconsDir = "src-tauri\icons"
if (-not (Test-Path $iconsDir)) {
    New-Item -ItemType Directory -Path $iconsDir -Force | Out-Null
}

# 生成各尺寸 PNG
$sizes = @{
    "32x32.png"        = 32
    "128x128.png"      = 128
    "128x128@2x.png"   = 256
    "icon.png"         = 64
}

foreach ($name in $sizes.Keys) {
    $size = $sizes[$name]
    $bmp = New-IconBitmap $size
    $path = Join-Path $iconsDir $name
    $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    Write-Host "Generated: $path ($size x $size)"
}

# 生成多分辨率 ICO (32 + 64 + 128 + 256)
function New-MultiIcon($pngPaths, $icoPath) {
    # 简易 ICO 文件构造器：包含多个 PNG 条目
    $ms = New-Object System.IO.MemoryStream
    $bw = New-Object System.IO.BinaryWriter($ms)

    $entries = @()
    foreach ($p in $pngPaths) {
        $bytes = [System.IO.File]::ReadAllBytes($p)
        $img = [System.Drawing.Image]::FromFile($p)
        $w = $img.Width
        $h = $img.Height
        $img.Dispose()
        $entries += ,@{ w = $w; h = $h; bytes = $bytes }
    }

    # ICONDIR header (6 bytes): reserved(2)=0, type(2)=1, count(2)
    $bw.Write([uint16]0)
    $bw.Write([uint16]1)
    $bw.Write([uint16]$entries.Count)

    # 计算图像数据起始偏移
    $dataOffset = 6 + 16 * $entries.Count
    foreach ($e in $entries) {
        $wByte = if ($e.w -ge 256) { [byte]0 } else { [byte]$e.w }
        $hByte = if ($e.h -ge 256) { [byte]0 } else { [byte]$e.h }
        $bw.Write($wByte)               # width
        $bw.Write($hByte)               # height
        $bw.Write([byte]0)              # color count (0 = >=256)
        $bw.Write([byte]0)              # reserved
        $bw.Write([uint16]1)            # color planes
        $bw.Write([uint16]32)           # bits per pixel
        $bw.Write([uint32]$e.bytes.Length)  # image size
        $bw.Write([uint32]$dataOffset)  # offset
        $dataOffset += $e.bytes.Length
    }

    foreach ($e in $entries) {
        $bw.Write($e.bytes)
    }

    [System.IO.File]::WriteAllBytes($icoPath, $ms.ToArray())
    $bw.Dispose()
    $ms.Dispose()
}

$pngList = @(
    (Join-Path $iconsDir "32x32.png"),
    (Join-Path $iconsDir "icon.png"),
    (Join-Path $iconsDir "128x128.png"),
    (Join-Path $iconsDir "128x128@2x.png")
)
New-MultiIcon $pngList (Join-Path $iconsDir "icon.ico")
Write-Host "Generated: icon.ico (multi-size)"

Write-Host "`nAll icons generated successfully."
