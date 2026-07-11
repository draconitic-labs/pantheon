# Draws the Pantheon icon (gold orbit engravings + luminous star on black)
# and writes assets/icon.png (window icon) + assets/icon.ico (multi-size,
# PNG-compressed entries) for the desktop shortcut.
Add-Type -AssemblyName System.Drawing

function Draw-Pantheon([int]$size) {
    $bmp = New-Object System.Drawing.Bitmap($size, $size)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = "AntiAlias"
    $g.Clear([System.Drawing.Color]::FromArgb(255, 2, 3, 6))
    $cx = $size / 2.0; $cy = $size / 2.0
    $s = $size / 256.0

    # faint star dust
    $rand = New-Object System.Random(7)
    for ($i = 0; $i -lt 40; $i++) {
        $x = $rand.NextDouble() * $size; $y = $rand.NextDouble() * $size
        $a = 40 + $rand.Next(60)
        $b = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb($a, 200, 210, 235))
        $g.FillEllipse($b, $x, $y, 1.6 * $s, 1.6 * $s); $b.Dispose()
    }

    # engraved elliptical orbits, slightly rotated like the atlas
    $gold = [System.Drawing.Color]::FromArgb(230, 212, 175, 105)
    $goldDim = [System.Drawing.Color]::FromArgb(150, 160, 130, 80)
    foreach ($ring in @(@(86, 0.94, -14, $gold), @(62, 0.90, 12, $goldDim), @(108, 0.97, 5, $goldDim))) {
        $r = $ring[0] * $s; $sq = $ring[1]; $ang = $ring[2]
        $pen = New-Object System.Drawing.Pen($ring[3], [Math]::Max(1.4 * $s, 1.2))
        $st = $g.Save()
        $g.TranslateTransform($cx, $cy); $g.RotateTransform($ang)
        $g.DrawEllipse($pen, -$r, -$r * $sq, 2 * $r, 2 * $r * $sq)
        $g.Restore($st); $pen.Dispose()
    }

    # planet bead on the middle orbit
    $bead = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(255, 125, 163, 255))
    $bx = $cx + 86 * $s * [Math]::Cos(0.6) * [Math]::Cos(-14 * [Math]::PI / 180)
    $by = $cy + 86 * $s * 0.94 * [Math]::Sin(0.6)
    $g.FillEllipse($bead, $bx - 4 * $s, $by - 4 * $s, 8 * $s, 8 * $s); $bead.Dispose()

    # luminous star core with radial glow
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $glowR = 60 * $s
    $path.AddEllipse($cx - $glowR, $cy - $glowR, 2 * $glowR, 2 * $glowR)
    $pgb = New-Object System.Drawing.Drawing2D.PathGradientBrush($path)
    $pgb.CenterColor = [System.Drawing.Color]::FromArgb(255, 255, 235, 190)
    $pgb.SurroundColors = @([System.Drawing.Color]::FromArgb(0, 255, 220, 150))
    $g.FillEllipse($pgb, $cx - $glowR, $cy - $glowR, 2 * $glowR, 2 * $glowR)
    $pgb.Dispose(); $path.Dispose()
    $core = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(255, 255, 244, 214))
    $g.FillEllipse($core, $cx - 9 * $s, $cy - 9 * $s, 18 * $s, 18 * $s); $core.Dispose()
    # diffraction spikes
    $spike = New-Object System.Drawing.Pen([System.Drawing.Color]::FromArgb(170, 255, 235, 190), [Math]::Max(1.2 * $s, 1.0))
    $g.DrawLine($spike, $cx - 34 * $s, $cy, $cx + 34 * $s, $cy)
    $g.DrawLine($spike, $cx, $cy - 34 * $s, $cx, $cy + 34 * $s)
    $spike.Dispose()

    $g.Dispose()
    return $bmp
}

$root = Split-Path $PSScriptRoot -Parent
$assets = Join-Path $root "assets"

$png256 = Draw-Pantheon 256
$png256.Save((Join-Path $assets "icon.png"), [System.Drawing.Imaging.ImageFormat]::Png)

# Build ICO with PNG-compressed entries
$sizes = @(256, 64, 48, 32, 16)
$blobs = @()
foreach ($sz in $sizes) {
    $bmp = Draw-Pantheon $sz
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $blobs += , $ms.ToArray()
    $bmp.Dispose(); $ms.Dispose()
}
$count = $sizes.Count
$offset = 6 + 16 * $count
$out = New-Object System.IO.MemoryStream
$w = New-Object System.IO.BinaryWriter($out)
$w.Write([UInt16]0); $w.Write([UInt16]1); $w.Write([UInt16]$count)
for ($i = 0; $i -lt $count; $i++) {
    $sz = $sizes[$i]
    $b = if ($sz -ge 256) { [byte]0 } else { [byte]$sz }
    $w.Write($b); $w.Write($b)            # width, height (0 = 256)
    $w.Write([byte]0); $w.Write([byte]0)  # palette, reserved
    $w.Write([UInt16]1); $w.Write([UInt16]32)
    $w.Write([UInt32]$blobs[$i].Length); $w.Write([UInt32]$offset)
    $offset += $blobs[$i].Length
}
foreach ($blob in $blobs) { $w.Write($blob) }
[System.IO.File]::WriteAllBytes((Join-Path $assets "icon.ico"), $out.ToArray())
$w.Dispose(); $out.Dispose(); $png256.Dispose()
Write-Host "wrote $assets\icon.png and icon.ico ($([math]::Round((Get-Item (Join-Path $assets 'icon.ico')).Length/1KB,1)) KB)"
