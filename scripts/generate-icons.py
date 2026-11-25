"""Generates crisp anti-aliased Bypath PNG and multi-size ICO icons from vector geometry."""
from __future__ import annotations

import math
import struct
import zlib
from pathlib import Path


def cubic_bezier_points(
    p0: tuple[float, float],
    p1: tuple[float, float],
    p2: tuple[float, float],
    p3: tuple[float, float],
    steps: int = 48,
) -> list[tuple[float, float]]:
    pts: list[tuple[float, float]] = []
    for i in range(steps + 1):
        t = i / steps
        u = 1.0 - t
        x = u**3 * p0[0] + 3 * u**2 * t * p1[0] + 3 * u * t**2 * p2[0] + t**3 * p3[0]
        y = u**3 * p0[1] + 3 * u**2 * t * p1[1] + 3 * u * t**2 * p2[1] + t**3 * p3[1]
        pts.append((x, y))
    return pts


def dist_point_to_segment(
    px: float, py: float, ax: float, ay: float, bx: float, by: float
) -> float:
    abx = bx - ax
    aby = by - ay
    apx = px - ax
    apy = py - ay
    denom = abx * abx + aby * aby
    if denom <= 1e-12:
        return math.hypot(px - ax, py - ay)
    t = max(0.0, min(1.0, (apx * abx + apy * aby) / denom))
    cx = ax + t * abx
    cy = ay + t * aby
    return math.hypot(px - cx, py - cy)


def sdf_rounded_rect(
    px: float, py: float, cx: float, cy: float, hx: float, hy: float, r: float
) -> float:
    qx = abs(px - cx) - (hx - r)
    qy = abs(py - cy) - (hy - r)
    outside = math.hypot(max(qx, 0.0), max(qy, 0.0))
    inside = min(max(qx, qy), 0.0)
    return outside + inside - r


def clamp01(v: float) -> float:
    return 0.0 if v < 0.0 else (1.0 if v > 1.0 else v)


def smooth_coverage(sdf: float, aa: float) -> float:
    return clamp01(0.5 - sdf / aa)


def build_polylines() -> tuple[list[tuple[float, float, float, float]], list[tuple[float, float, float, float]]]:
    gray_segs = [(7.7, 17.6, 7.7, 6.6)]

    white_segs: list[tuple[float, float, float, float]] = [
        (7.7, 17.6, 7.7, 14.9),
        (15.3, 9.3, 15.3, 6.9),
        (12.5, 9.4, 15.3, 6.6),
        (15.3, 6.6, 18.1, 9.4),
    ]
    curve = cubic_bezier_points((7.7, 14.9), (7.7, 11.7), (15.3, 12.5), (15.3, 9.3), steps=48)
    for a, b in zip(curve, curve[1:]):
        white_segs.append((a[0], a[1], b[0], b[1]))
    return gray_segs, white_segs


def render_icon_rgba(size: int) -> bytes:
    gray_segs, white_segs = build_polylines()
    # Slight stroke thickening at tiny icon sizes (16/24px) keeps tray icons razor sharp.
    scale_boost = 1.15 if size <= 24 else (1.06 if size <= 32 else 1.0)
    gray_half = (1.75 / 2.0) * scale_boost
    white_half = (1.8 / 2.0) * scale_boost

    # Analytical SDF AA width in 24x24 viewBox units
    aa = 24.0 / size
    bg = (0x1A, 0x1A, 0x1D)
    gray = (0x6C, 0x6C, 0x77)
    white = (0xFF, 0xFF, 0xFF)

    out = bytearray(size * size * 4)
    idx = 0
    for py_i in range(size):
        vy = (py_i + 0.5) * 24.0 / size
        for px_i in range(size):
            vx = (px_i + 0.5) * 24.0 / size

            rect_d = sdf_rounded_rect(vx, vy, 12.0, 12.0, 10.5, 10.5, 5.4)
            tile_a = smooth_coverage(rect_d, aa)
            if tile_a <= 0.0:
                idx += 4
                continue

            # Gray main trunk
            gd = min(dist_point_to_segment(vx, vy, *s) for s in gray_segs) - gray_half
            ga = smooth_coverage(gd, aa)

            # White active bypass lane
            wd = min(dist_point_to_segment(vx, vy, *s) for s in white_segs) - white_half
            wa = smooth_coverage(wd, aa)

            # Composite over charcoal tile: bg -> gray -> white
            r = bg[0] * (1.0 - ga) + gray[0] * ga
            g = bg[1] * (1.0 - ga) + gray[1] * ga
            b = bg[2] * (1.0 - ga) + gray[2] * ga

            r = r * (1.0 - wa) + white[0] * wa
            g = g * (1.0 - wa) + white[1] * wa
            b = b * (1.0 - wa) + white[2] * wa

            out[idx] = int(round(r))
            out[idx + 1] = int(round(g))
            out[idx + 2] = int(round(b))
            out[idx + 3] = int(round(tile_a * 255.0))
            idx += 4

    return bytes(out)


def encode_png(width: int, height: int, rgba: bytes) -> bytes:
    def chunk(tag: bytes, data: bytes) -> bytes:
        crc = zlib.crc32(tag + data) & 0xFFFFFFFF
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", crc)

    raw_rows = bytearray()
    stride = width * 4
    for y in range(height):
        raw_rows.append(0)  # Filter type 0 (None)
        start = y * stride
        raw_rows.extend(rgba[start : start + stride])

    ihdr = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    idat = zlib.compress(bytes(raw_rows), level=9)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", idat)
        + chunk(b"IEND", b"")
    )


def encode_ico(png_entries: list[tuple[int, bytes]]) -> bytes:
    header = struct.pack("<HHH", 0, 1, len(png_entries))
    offset = 6 + 16 * len(png_entries)
    dirs = bytearray()
    body = bytearray()
    for size, png_bytes in png_entries:
        dim = 0 if size >= 256 else size
        dirs.extend(
            struct.pack(
                "<BBBBHHII",
                dim,
                dim,
                0,
                0,
                1,
                32,
                len(png_bytes),
                offset,
            )
        )
        body.extend(png_bytes)
        offset += len(png_bytes)
    return header + bytes(dirs) + bytes(body)


def main() -> None:
    repo = Path(__file__).resolve().parents[1]
    icons_dir = repo / "apps" / "desktop" / "src-tauri" / "icons"
    assets_dir = repo / "assets"
    icons_dir.mkdir(parents=True, exist_ok=True)
    assets_dir.mkdir(parents=True, exist_ok=True)

    rendered: dict[int, bytes] = {}
    for sz in (16, 24, 32, 48, 64, 128, 256, 512):
        rgba = render_icon_rgba(sz)
        rendered[sz] = encode_png(sz, sz, rgba)

    (icons_dir / "32x32.png").write_bytes(rendered[32])
    (icons_dir / "128x128.png").write_bytes(rendered[128])
    (icons_dir / "icon.png").write_bytes(rendered[512])
    (assets_dir / "bypath-logo-512.png").write_bytes(rendered[512])

    ico_bytes = encode_ico([(sz, rendered[sz]) for sz in (16, 24, 32, 48, 64, 128, 256)])
    (icons_dir / "icon.ico").write_bytes(ico_bytes)
    print(f"Generated Bypath icons in {icons_dir} and {assets_dir}")


if __name__ == "__main__":
    main()
