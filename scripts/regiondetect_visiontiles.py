#!/usr/bin/env python3
"""Vision-agent drafting for region-detect ground truth.

A full dump screenshot is too big for a vision model to label tightly,
so this splits it into overlapping tiles; a vision model labels each
tile; this then merges the tile-local rectangles back into one seed
file for regiondetect_annotate.py. The human still signs off — vision
output is a draft with loose edges, not truth.

Workflow (the vision pass runs inside the coding agent, which is the
only thing that can talk to the vision model):

  1. split:  regiondetect_visiontiles.py split DUMP/eDP-1-input.png
       -> vision/eDP-1/tiles/tile-rXX-cYY.png + manifest.json
  2. label:  for each tile, give the vision agent the tile path with
     this prompt (verbatim; adjust the platform only):

       "You are building ground truth for a screenshot-region selector
       (the kind of tool that highlights the UI region under the mouse).
       The image is one tile of a larger desktop screenshot. List EVERY
       rectangular region a human would want to snap-select: windows,
       panels, cards, sidebars, toolbars, buttons, input fields, list
       rows, text blocks, code blocks, images/thumbnails, tables.
       Rules: reply with ONLY a JSON array of half-open rects
       [[left, top, right, bottom], ...] in TILE-LOCAL pixels; rectangles
       tight to the visible element bounds; nesting is fine (a panel and
       the button inside it both appear); no rect may be the whole tile;
       skip anything smaller than 20x20 px."

  3. save each reply verbatim as vision/<stem>/result/<tile>.json
     (the merge tolerates markdown fences and prose around the array)
  4. merge:  regiondetect_visiontiles.py merge DUMP/eDP-1-input.png
       -> vision/<stem>-seed.json  {"rects": [...]} in FULL-image pixels
  5. curate: regiondetect_annotate.py DUMP/<stem>-input.png --trace vision/<stem>-seed.json
       (blue = drafted rects; click to delete, drag to add, s to save)

Rectangles are half-open [l, t, r, b] throughout.
"""
import argparse
import json
import pathlib
import re
import sys

from PIL import Image

MERGE_IOU = 0.6  # cross-tile duplicates collapse into the larger rect
MIN_SIZE = 16  # px per side: drop slivers after clipping


def stem_of(image_path):
    stem = image_path.name
    for suffix in ("-input.png", ".png"):
        if stem.endswith(suffix):
            return stem[: -len(suffix)]
    return stem


def workdir(image_path, out_root):
    return out_root / stem_of(image_path)


def cmd_split(image_path, out_root, tile, overlap):
    image = Image.open(image_path)
    width, height = image.size
    work = workdir(image_path, out_root)
    tiles_dir = work / "tiles"
    tiles_dir.mkdir(parents=True, exist_ok=True)

    def axis_positions(size, tile, overlap):
        if size <= tile:
            return [0]
        step = tile - overlap
        positions = list(range(0, size - tile + 1, step))
        if positions[-1] != size - tile:
            positions.append(size - tile)
        return positions

    manifest = {
        "image": [width, height],
        "tile": tile,
        "overlap": overlap,
        "tiles": {},
    }
    xs = axis_positions(width, tile, overlap)
    ys = axis_positions(height, tile, overlap)
    for row, y in enumerate(ys):
        for col, x in enumerate(xs):
            name = f"tile-r{row:02d}-c{col:02d}"
            image.crop((x, y, min(x + tile, width), min(y + tile, height))).save(
                tiles_dir / f"{name}.png"
            )
            manifest["tiles"][name] = [x, y]
    (work / "manifest.json").write_text(json.dumps(manifest, indent=1))
    print(f"split {width}x{height} into {len(manifest['tiles'])} tiles -> {tiles_dir}")


def extract_rects(text):
    """Tolerate prose/markdown fences: grab the first JSON array of
    4-number arrays (or {"rects": ...}) in the reply."""
    text = re.sub(r"```[a-z]*", "", text)
    match = re.search(r"\[[^\[\]]*\[", text)
    start = text.find("[", text.rfind("[", 0, match.end()) if match else 0)
    # find the outermost array: first '[' ... last ']'
    start = text.find("[")
    end = text.rfind("]")
    if start < 0 or end <= start:
        return []
    try:
        data = json.loads(text[start : end + 1])
    except json.JSONDecodeError:
        return []
    if isinstance(data, dict):
        data = data.get("rects", [])
    rects = []
    for item in data:
        if (
            isinstance(item, (list, tuple))
            and len(item) == 4
            and all(isinstance(v, (int, float)) for v in item)
        ):
            left, top, right, bottom = (int(round(v)) for v in item)
            if right > left and bottom > top:
                rects.append([left, top, right, bottom])
    return rects


def iou(a, b):
    left = max(a[0], b[0])
    top = max(a[1], b[1])
    right = min(a[2], b[2])
    bottom = min(a[3], b[3])
    if right <= left or bottom <= top:
        return 0.0
    inter = (right - left) * (bottom - top)
    return inter / ((a[2] - a[0]) * (a[3] - a[1]) + (b[2] - b[0]) * (b[3] - b[1]) - inter)


def cmd_merge(image_path, out_root, min_size, merge_iou):
    work = workdir(image_path, out_root)
    manifest = json.loads((work / "manifest.json").read_text())
    width, height = manifest["image"]

    merged = []  # full-image rects, largest-first dedupe
    per_tile_counts = {}
    for name, (ox, oy) in sorted(manifest["tiles"].items()):
        result_path = work / "result" / f"{name}.json"
        if not result_path.exists():
            print(f"warning: no result for {name} (skipped)")
            continue
        rects = extract_rects(result_path.read_text())
        kept = 0
        for left, top, right, bottom in rects:
            rect = [
                min(max(left + ox, 0), width),
                min(max(top + oy, 0), height),
                min(max(right + ox, 0), width),
                min(max(bottom + oy, 0), height),
            ]
            if rect[2] - rect[0] < min_size or rect[3] - rect[1] < min_size:
                continue
            if any(iou(rect, other) >= merge_iou for other in merged):
                continue
            merged.append(rect)
            kept += 1
        per_tile_counts[name] = f"{len(rects)}->+{kept}"

    merged.sort(key=lambda r: (r[1], r[0]))
    seed_path = out_root / f"{stem_of(image_path)}-seed.json"
    seed_path.write_text(json.dumps({"rects": merged}, indent=1))
    for name, count in per_tile_counts.items():
        print(f"  {name}: {count}")
    print(f"merged {len(merged)} rects -> {seed_path}")


def main():
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    sub = parser.add_subparsers(dest="command", required=True)

    split = sub.add_parser("split", help="cut the screenshot into labeled tiles")
    split.add_argument("image", type=pathlib.Path, help="dump <stem>-input.png")
    split.add_argument("--tile", type=int, default=1280)
    split.add_argument("--overlap", type=int, default=128)
    split.add_argument("--out", type=pathlib.Path, default=None,
                       help="work root (default: <image dir>/vision)")

    merge = sub.add_parser("merge", help="offset+dedupe per-tile results into a seed file")
    merge.add_argument("image", type=pathlib.Path, help="dump <stem>-input.png")
    merge.add_argument("--min-size", type=int, default=MIN_SIZE)
    merge.add_argument("--iou", type=float, default=MERGE_IOU)
    merge.add_argument("--out", type=pathlib.Path, default=None,
                       help="work root (default: <image dir>/vision)")
    args = parser.parse_args()

    out_root = args.out or (args.image.parent / "vision")
    if args.command == "split":
        cmd_split(args.image, out_root, args.tile, args.overlap)
    else:
        cmd_merge(args.image, out_root, args.min_size, args.iou)


if __name__ == "__main__":
    main()
