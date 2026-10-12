#!/usr/bin/env python3
"""Regenerate app icons from the canonical SVG (requires rsvg-convert)."""
import pathlib
import struct
import subprocess

ROOT = pathlib.Path(__file__).resolve().parents[1]
ASSETS = ROOT / "assets" / "app"
SIZES = (16, 20, 24, 32, 48, 64, 96, 128, 192, 256, 512, 1024)
ICO_SIZES = (16, 24, 32, 48, 64, 128, 256)


def main():
    for size in SIZES:
        subprocess.run(
            ["rsvg-convert", "-w", str(size), "-h", str(size),
             str(ASSETS / "shotori.svg"), "-o", str(ASSETS / f"shotori-{size}.png")],
            check=True,
        )
    # ICO permits PNG frames; keep every frame at its native vector-rendered size.
    frames = [(size, (ASSETS / f"shotori-{size}.png").read_bytes()) for size in ICO_SIZES]
    offset = 6 + 16 * len(frames)
    directory = bytearray(struct.pack("<HHH", 0, 1, len(frames)))
    for size, data in frames:
        directory.extend(struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(data), offset))
        offset += len(data)
    (ASSETS / "shotori.ico").write_bytes(bytes(directory) + b"".join(data for _, data in frames))


if __name__ == "__main__":
    main()
