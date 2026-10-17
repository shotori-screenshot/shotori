#!/usr/bin/env python3
"""Vision-agent drafting for region-detect ground truth.

A full dump screenshot is too big for a vision model to label tightly,
so this splits it into overlapping tiles; a vision model labels each
tile; this then merges the tile-local rectangles back into one seed
file for regiondetect_annotate.py. The human still signs off — vision
output is a draft with loose edges, not truth.

Workflow (the vision passes run inside the coding agent, which is the
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
  4. review: regiondetect_visiontiles.py review DUMP/eDP-1-input.png
       -> review/<tile>.png, the draft boxes drawn on the tile with id
       numbers. Give the vision agent the review image with this prompt:

       "Review ground-truth boxes for a screenshot-region selector. The
       image is one tile of a desktop screenshot; numbered orange boxes
       are a draft, each box's id at its top-left. For EACH box decide:
       keep (fix coordinates that are loose or clipped — tight to the
       visible element bounds) or delete (not a real selectable UI
       region, duplicate, meaningless). Then add real selectable regions
       the draft missed. Reply ONLY with JSON, no fences, no prose:
       {"keep": [[box_id, left, top, right, bottom], ...],
        "del": [box_id, ...], "new": [[left, top, right, bottom], ...]}
       TILE-LOCAL half-open pixels; every box id appears in keep or del."

     save each reply as vision/<stem>/review/<tile>.json (the file,
     not the image)
  5. merge:  regiondetect_visiontiles.py merge DUMP/eDP-1-input.png
       -> vision/<stem>-seed.json  {"rects": [...]} in FULL-image pixels
       (review files win over raw drafts; the per-tile audit prints
       how many boxes were kept/fixed/deleted/added)
  6. curate: regiondetect_workbench.py DUMP_DIR eDP-1
       (Tab walks the draft queue zoomed to each box, y accepts, d
       deletes; click to delete/accept in the overview; F5 runs the
       eval report in-window)

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


def extract_json(text):
    """First JSON blob in a reply, tolerating prose and fences."""
    text = re.sub(r"```[a-z]*", "", text)
    for open_char, close_char in (("{", "}"), ("[", "]")):
        start = text.find(open_char)
        end = text.rfind(close_char)
        if start >= 0 and end > start:
            try:
                return json.loads(text[start : end + 1])
            except json.JSONDecodeError:
                continue
    return None


def extract_rects(text):
    """Rect list from a draft reply: an array of 4-number arrays (or
    {"rects": ...})."""
    data = extract_json(text)
    if isinstance(data, dict):
        data = data.get("rects", [])
    if not isinstance(data, list):
        return []
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


def load_tile_rects(work, name):
    """Rects for one tile: the review verdict wins over the raw draft.

    Returns (rects, audit) — audit is None when unreviewed, else a
    dict of counts. Box ids the review never mentions count as kept
    unchanged (sloppy verdicts degrade gracefully, no punishment)."""
    draft_path = work / "result" / f"{name}.json"
    if not draft_path.exists():
        return None, None
    draft = extract_rects(draft_path.read_text())
    review_path = work / "review" / f"{name}.json"
    if not review_path.exists():
        return draft, None

    data = extract_json(review_path.read_text())
    if not (isinstance(data, dict) and "keep" in data):
        # model ignored the format and replied with a plain list
        return extract_rects(review_path.read_text()), None

    keep = {}
    fixed = 0
    for entry in data.get("keep", []):
        if not (
            isinstance(entry, (list, tuple))
            and len(entry) == 5
            and all(isinstance(v, (int, float)) for v in entry)
        ):
            continue
        index, left, top, right, bottom = (int(round(v)) for v in entry)
        if not 0 <= index < len(draft):
            continue
        rect = [left, top, right, bottom]
        if rect[2] > rect[0] and rect[3] > rect[1]:
            # a nudge is not a fix: only edges moved by ≥4 px count
            old, new = draft[index], rect
            if max(abs(new[i] - old[i]) for i in range(4)) >= 4:
                fixed += 1
            keep[index] = rect
    deleted = {
        int(v)
        for v in data.get("del", [])
        if isinstance(v, (int, float)) and 0 <= int(v) < len(draft)
    }
    rects = [
        keep.get(index, draft[index])
        for index in range(len(draft))
        if index not in deleted
    ]
    new_rects = extract_rects(json.dumps(data.get("new", [])))
    audit = {
        "draft": len(draft),
        "fixed": fixed,
        "deleted": len(deleted),
        "new": len(new_rects),
    }
    return rects + new_rects, audit


def iou(a, b):
    left = max(a[0], b[0])
    top = max(a[1], b[1])
    right = min(a[2], b[2])
    bottom = min(a[3], b[3])
    if right <= left or bottom <= top:
        return 0.0
    inter = (right - left) * (bottom - top)
    return inter / ((a[2] - a[0]) * (a[3] - a[1]) + (b[2] - b[0]) * (b[3] - b[1]) - inter)


def cmd_review(image_path, out_root):
    """Draw each tile's draft boxes (numbered) on the tile for the
    vision agent's self-review pass."""
    work = workdir(image_path, out_root)
    manifest = json.loads((work / "manifest.json").read_text())
    review_dir = work / "review"
    review_dir.mkdir(parents=True, exist_ok=True)
    from PIL import ImageDraw, ImageFont

    font = ImageFont.load_default(size=26)
    for name, _offset in sorted(manifest["tiles"].items()):
        draft_path = work / "result" / f"{name}.json"
        if not draft_path.exists():
            continue
        rects = extract_rects(draft_path.read_text())
        img = Image.open(work / "tiles" / f"{name}.png").convert("RGB")
        draw = ImageDraw.Draw(img)
        for index, (left, top, right, bottom) in enumerate(rects):
            draw.rectangle([left, top, right - 1, bottom - 1], outline=(255, 120, 0), width=3)
            label = str(index)
            bbox = draw.textbbox((0, 0), label, font=font)
            label_w, label_h = bbox[2] - bbox[0], bbox[3] - bbox[1]
            label_x, label_y = left, max(0, top - label_h - 8)
            draw.rectangle(
                [label_x, label_y, label_x + label_w + 10, label_y + label_h + 8],
                fill=(255, 120, 0),
            )
            draw.text((label_x + 5, label_y + 3), label, fill=(0, 0, 0), font=font)
        img.save(review_dir / f"{name}.png")
    print(f"rendered numbered drafts -> {review_dir}")


def cmd_merge(image_path, out_root, min_size, merge_iou):
    work = workdir(image_path, out_root)
    manifest = json.loads((work / "manifest.json").read_text())
    width, height = manifest["image"]

    merged = []  # full-image rects, largest-first dedupe
    for name, (ox, oy) in sorted(manifest["tiles"].items()):
        rects, audit = load_tile_rects(work, name)
        if rects is None:
            print(f"warning: no result for {name} (skipped)")
            continue
        kept = 0
        # the model only saw this tile: clamp its verdicts to the tile's
        # extent in full-image space (review coords overshoot sometimes)
        tile_w = min(manifest["tile"], width - ox)
        tile_h = min(manifest["tile"], height - oy)
        for left, top, right, bottom in rects:
            rect = [
                min(max(left + ox, ox), ox + tile_w),
                min(max(top + oy, oy), oy + tile_h),
                min(max(right + ox, ox), ox + tile_w),
                min(max(bottom + oy, oy), oy + tile_h),
            ]
            if rect[2] - rect[0] < min_size or rect[3] - rect[1] < min_size:
                continue
            if any(iou(rect, other) >= merge_iou for other in merged):
                continue
            merged.append(rect)
            kept += 1
        note = f"{kept} merged"
        if audit:
            note += (
                f"  [review: draft {audit['draft']}, fixed {audit['fixed']},"
                f" deleted {audit['deleted']}, new {audit['new']}]"
            )
        print(f"  {name}: {note}")

    merged.sort(key=lambda r: (r[1], r[0]))
    seed_path = out_root / f"{stem_of(image_path)}-seed.json"
    seed_path.write_text(json.dumps({"rects": merged}, indent=1))
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

    review = sub.add_parser("review", help="render numbered drafts for the review pass")
    review.add_argument("image", type=pathlib.Path, help="dump <stem>-input.png")
    review.add_argument("--out", type=pathlib.Path, default=None,
                        help="work root (default: <image dir>/vision)")
    args = parser.parse_args()

    out_root = args.out or (args.image.parent / "vision")
    if args.command == "split":
        cmd_split(args.image, out_root, args.tile, args.overlap)
    elif args.command == "review":
        cmd_review(args.image, out_root)
    else:
        cmd_merge(args.image, out_root, args.min_size, args.iou)


if __name__ == "__main__":
    main()
