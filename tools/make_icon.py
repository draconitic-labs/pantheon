#!/usr/bin/env python3
"""Draw the Pantheon icons with the standard library only.

Gold orbit engravings and a luminous star on black. Writes:
  assets/icon.png      256 px, window icon and menu entry for Pantheon
  assets/icon.ico      multi-size, PNG-compressed entries
  assets/icon-3d.png   256 px, menu entry for Pantheon 3D (orbits tilted in depth)
"""

from __future__ import annotations

import math
import random
import struct
import zlib
from pathlib import Path

MASTER = 768  # drawn large, then box-filtered down for clean edges
Pixel = tuple[float, float, float]


def over(dst: Pixel, src: Pixel, a: float) -> Pixel:
    return (dst[0] + (src[0] - dst[0]) * a, dst[1] + (src[1] - dst[1]) * a, dst[2] + (src[2] - dst[2]) * a)


def smooth(lo: float, hi: float, x: float) -> float:
    t = max(0.0, min(1.0, (x - lo) / (hi - lo)))
    return t * t * (3.0 - 2.0 * t)


def draw(size: int, depth: bool) -> list[Pixel]:
    s = size / 256.0
    cx = cy = size / 2.0
    rng = random.Random(7)
    dust = [(rng.random() * size, rng.random() * size, 40 + rng.random() * 60) for _ in range(40)]
    gold = (212.0, 175.0, 105.0)
    gold_dim = (160.0, 130.0, 80.0)
    blue = (125.0, 163.0, 255.0)
    # radius, squash, rotation degrees, colour, alpha
    squash = 0.42 if depth else 1.0
    rings = [
        (86.0, 0.94 * squash, -14.0, gold, 0.9),
        (62.0, 0.90 * squash, 12.0, gold_dim, 0.6),
        (108.0, 0.97 * squash, 5.0, gold_dim, 0.6),
    ]
    rings = [(r * s, q, math.radians(a), c, al) for r, q, a, c, al in rings]
    width = max(1.4 * s, 1.2)
    bead_a = 0.6
    r0, q0, a0, _, _ = rings[0]
    ex, ey = r0 * math.cos(bead_a), r0 * q0 * math.sin(bead_a)
    bead = (cx + ex * math.cos(a0) - ey * math.sin(a0), cy + ex * math.sin(a0) + ey * math.cos(a0))

    out: list[Pixel] = []
    for y in range(size):
        for x in range(size):
            px = x + 0.5
            py = y + 0.5
            col: Pixel = (2.0, 3.0, 6.0)
            for dx, dy, a in dust:
                d2 = (px - dx) ** 2 + (py - dy) ** 2
                if d2 < (1.3 * s) ** 2:
                    col = over(col, (200.0, 210.0, 235.0), a / 255.0)
            rx, ry = px - cx, py - cy
            for r, q, ang, c, al in rings:
                u = rx * math.cos(ang) + ry * math.sin(ang)
                v = (-rx * math.sin(ang) + ry * math.cos(ang)) / q
                # distance to the ellipse, by the implicit function's gradient
                rho = math.hypot(u, v)
                if rho < 1e-6:
                    continue
                grad = math.hypot(u / rho, (v / rho) / q)
                dist = abs(rho - r) / max(grad, 1e-6)
                a = 1.0 - smooth(width * 0.5, width * 0.5 + 1.0, dist)
                if a > 0.0:
                    col = over(col, c, a * al)
            d = math.hypot(px - bead[0], py - bead[1])
            a = 1.0 - smooth(4.0 * s - 0.8, 4.0 * s + 0.8, d)
            if a > 0.0:
                col = over(col, blue, a)
            # the star: a radial glow, a core, a soft diffraction cross
            d = math.hypot(rx, ry)
            glow = max(0.0, 1.0 - d / (60.0 * s)) ** 2.4
            col = (col[0] + 255.0 * glow, col[1] + 228.0 * glow, col[2] + 170.0 * glow)
            core = 1.0 - smooth(8.0 * s, 10.0 * s, d)
            if core > 0.0:
                col = over(col, (255.0, 244.0, 214.0), core)
            for t, along in ((abs(ry), abs(rx)), (abs(rx), abs(ry))):
                if along < 34.0 * s:
                    a = (1.0 - smooth(0.4 * s, 1.3 * s, t)) * (1.0 - along / (34.0 * s)) * 0.75
                    if a > 0.0:
                        col = over(col, (255.0, 235.0, 190.0), a)
            out.append((min(col[0], 255.0), min(col[1], 255.0), min(col[2], 255.0)))
    return out


def downsample(px: list[Pixel], size: int, to: int) -> bytes:
    f = size // to
    out = bytearray()
    area = f * f
    for y in range(to):
        for x in range(to):
            r = g = b = 0.0
            for j in range(y * f, (y + 1) * f):
                row = j * size
                for i in range(x * f, (x + 1) * f):
                    p = px[row + i]
                    r += p[0]
                    g += p[1]
                    b += p[2]
            out += bytes((int(r / area + 0.5), int(g / area + 0.5), int(b / area + 0.5), 255))
    return bytes(out)


def png(rgba: bytes, size: int) -> bytes:
    def chunk(tag: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    stride = size * 4
    raw = b"".join(b"\x00" + rgba[y * stride : (y + 1) * stride] for y in range(size))
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def ico(entries: list[tuple[int, bytes]]) -> bytes:
    head = struct.pack("<HHH", 0, 1, len(entries))
    offset = 6 + 16 * len(entries)
    dir_ = b""
    for size, blob in entries:
        b = 0 if size >= 256 else size
        dir_ += struct.pack("<BBBBHHII", b, b, 0, 0, 1, 32, len(blob), offset)
        offset += len(blob)
    return head + dir_ + b"".join(blob for _, blob in entries)


def main() -> None:
    assets = Path(__file__).resolve().parent.parent / "assets"
    master = draw(MASTER, False)
    sizes = [256, 64, 48, 32, 16]
    blobs = [(sz, png(downsample(master, MASTER, sz), sz)) for sz in sizes]
    (assets / "icon.png").write_bytes(blobs[0][1])
    (assets / "icon.ico").write_bytes(ico(blobs))
    (assets / "icon-3d.png").write_bytes(png(downsample(draw(MASTER, True), MASTER, 256), 256))
    print(f"wrote {assets / 'icon.png'}, icon.ico, icon-3d.png")


if __name__ == "__main__":
    main()
