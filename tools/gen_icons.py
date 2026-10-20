"""Generate CAMEO icon PNGs (32, 128, 256, 512).
C-emblem: gradient ring with a gap forming the letter C, round caps, core dot.
Standard library only."""
import math
import struct
import sys
import zlib
from pathlib import Path

CYAN = (34, 211, 238)
VIOLET = (139, 92, 246)
BLACK = (0, 0, 0)

def clamp01(v: float) -> float:
    return 0.0 if v < 0 else (1.0 if v > 1 else v)

def lerp(a: float, b: float, t: float) -> float:
    return a + (b - a) * t

def gradient(t: float) -> tuple[int, int, int]:
    t = clamp01(t)
    return (
        lerp(CYAN[0], VIOLET[0], t),
        lerp(CYAN[1], VIOLET[1], t),
        lerp(CYAN[2], VIOLET[2], t),
    )

OUTER = 0.44
INNER = 0.26
GAP_DEG = 32.0          # C opening spans ±GAP_DEG around the right direction
MID = (OUTER + INNER) / 2
STROKE = (OUTER - INNER) / 2
CORE = (0.435, 0.5, STROKE * 1.12)   # x, y, radius of the AI core dot
FLAG = (0.685, 0.5, STROKE * 0.52)   # small accent dot

def cap_centers():
    caps = []
    for sgn in (1, -1):
        a = math.radians(GAP_DEG * sgn)
        caps.append((0.5 + MID * math.cos(a), 0.5 + MID * math.sin(a)))
    return caps

CAPS = cap_centers()

def sample(x: float, y: float) -> tuple[float, float, float, float]:
    dx, dy = x - 0.5, y - 0.5
    r = math.hypot(dx, dy)
    ang = math.degrees(math.atan2(dy, dx))  # 0 = right, positive downward

    alpha = 0.0
    # ring body, minus the wedge gap
    in_gap = abs(ang) < GAP_DEG
    if not in_gap and INNER <= r <= OUTER:
        edge = min(OUTER - r, r - INNER) / 0.012
        alpha = clamp01(edge)
    else:
        # round stroke caps at both gap ends
        for cx, cy in CAPS:
            d = math.hypot(dx - (cx - 0.5), dy - (cy - 0.5))
            a = (STROKE - d) / 0.012
            alpha = max(alpha, clamp01(a))
    # core + accent dots
    for (cx, cy, cr) in ((CORE[0], CORE[1], CORE[2]), (FLAG[0], FLAG[1], FLAG[2])):
        d = math.hypot(x - cx, y - cy)
        a = (cr - d) / 0.012
        alpha = max(alpha, clamp01(a))
    if alpha <= 0:
        return (0, 0, 0, 0.0)
    t = clamp01(0.5 - dy)          # top → cyan, bottom → violet
    col = gradient(t)
    return (col[0], col[1], col[2], alpha)

def render(size: int) -> bytes:
    ss = 4
    step = 1.0 / (size * ss)
    out_rows = []
    for py in range(size):
        row = bytearray()
        row.append(0)
        for px in range(size):
            ar = ag = ab = aa = 0.0
            n = 0.0
            for sy in range(ss):
                for sx in range(ss):
                    x = (px + (sx + 0.5) * step) / size
                    y = (py + (sy + 0.5) * step) / size
                    r, g, b, a = sample(x, y)
                    ar += r * a
                    ag += g * a
                    ab += b * a
                    aa += a
                    n += 1
            if aa > 0:
                weight = aa / n
                row += bytes((
                    int(ar / aa),
                    int(ag / aa),
                    int(ab / aa),
                    int(clamp01(weight) * 255),
                ))
            else:
                row += bytes((0, 0, 0, 0))
        out_rows.append(bytes(row))
    return encode_png(size, size, b"".join(out_rows))

def encode_png(w: int, h: int, raw: bytes) -> bytes:
    def chunk(tag: bytes, data: bytes) -> bytes:
        return (struct.pack(">I", len(data)) + tag + data
                + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF))
    ihdr = struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", ihdr)
            + chunk(b"IDAT", zlib.compress(raw, 9))
            + chunk(b"IEND", b""))

def main(outdir: str) -> None:
    out = Path(outdir)
    out.mkdir(parents=True, exist_ok=True)
    for size in (32, 128, 256, 512):
        data = render(size)
        (out / f"cameo_{size}.png").write_bytes(data)
        print(f"wrote cameo_{size}.png ({len(data)} bytes)")

if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "src-tauri/icons")
