#!/usr/bin/env python3
"""Score a SHOTORI_REGIONDUMP dump against hand-drawn ground truth.

Usage:
  regiondetect_eval.py DUMP_DIR [--gt-dir DIR] [--iou T ...] [--render DIR]
                        [--show-rejected N]

DUMP_DIR holds `<output>-trace.json` (+ `-input.png`, `-edges.png`) as
written by the detector, and ground truth `<output>.json` files
({"rects": [[l,t,r,b], ...]}, as graded in
regiondetect_workbench.py) in --gt-dir (default DUMP_DIR/gt). All
rectangles are half-open image-pixel rects.

Prints, per screen and overall, precision/recall at each IoU threshold,
the rejection-reason histogram, and — the payoff — for every
ground-truth region the detector MISSED, the closest rejected proposal
with the gate that killed it and its measurements. That turns "this
region is missing" into "second_side gate killed it at 0.31", which is
where to turn the next screw.

Caveat on precision: candidates form a containment hierarchy the wheel
walks, so "one candidate per GT" is not the goal and raw precision
undercounts usefulness. Treat recall + the miss diagnoses as the signal;
precision is printed for regression watching (a flooding detector shows
up there first).

--render DIR writes `<output>-eval.png` overlays: green = matched GT,
thick red = missed GT, cyan = matched candidate, orange = unmatched
candidate, blue dashed-ish = closest rejected proposal for a missed GT.
"""
import argparse
import json
import pathlib
from collections import Counter

from PIL import Image, ImageDraw

GT_FILENAME = "gt.json"


def iou(a, b):
    left = max(a[0], b[0])
    top = max(a[1], b[1])
    right = min(a[2], b[2])
    bottom = min(a[3], b[3])
    if right <= left or bottom <= top:
        return 0.0
    inter = (right - left) * (bottom - top)
    area_a = (a[2] - a[0]) * (a[3] - a[1])
    area_b = (b[2] - b[0]) * (b[3] - b[1])
    return inter / (area_a + area_b - inter)


def load_gt(path):
    if not path.exists():
        return None
    data = json.loads(path.read_text())
    rects = data["rects"] if isinstance(data, dict) else data
    return [list(map(int, rect)) for rect in rects]


def greedy_match(gts, candidates, threshold):
    """Candidates (quality desc) each take their best unmatched GT.

    Returns (matches, matched_gt_indices); matches are (gt_idx, cand_idx).
    """
    matches = []
    used_gt = set()
    order = sorted(range(len(candidates)), key=lambda i: -candidates[i]["quality"])
    for ci in order:
        best, best_iou = None, threshold
        for gi, gt in enumerate(gts):
            if gi in used_gt:
                continue
            value = iou(gt, candidates[ci]["rect"])
            if value >= best_iou:
                best, best_iou = gi, value
        if best is not None:
            used_gt.add(best)
            matches.append((best, ci))
    return matches, used_gt


def closest_entry(rect, entries, key):
    """Best-IoU entry in `entries` (list of dicts) against `rect`."""
    best, best_iou = None, 0.0
    for entry in entries:
        value = iou(rect, entry[key])
        if value > best_iou:
            best, best_iou = entry, value
    return best, best_iou


def diagnose_miss(gt_rect, trace, matched_candidates):
    """The un-matched candidate or rejected proposal that overlaps a
    missed GT most — with the gate name for rejected proposals."""
    cands = [c for i, c in enumerate(trace["candidates"]) if i not in matched_candidates]
    best_cand, cand_iou = closest_entry(gt_rect, cands, "rect")
    rejected = [p for p in trace["proposals"] if p["rejected"] is not None]
    best_rej, rej_iou = closest_entry(gt_rect, rejected, "rect_image")
    if rej_iou >= cand_iou and best_rej is not None:
        return ("rejected", best_rej, rej_iou)
    if best_cand is not None:
        return ("candidate", best_cand, cand_iou)
    return (None, None, 0.0)


def render(out_path, input_path, gts, trace, matches, missed_gts):
    img = Image.open(input_path).convert("RGB")
    draw = ImageDraw.Draw(img)
    matched_gt = {gi for gi, _ in matches}
    matched_cand = {ci for _, ci in matches}
    # closest rejected proposal per missed GT, for the overlay
    for gi in missed_gts:
        kind, entry, _ = diagnose_miss(gts[gi], trace, matched_cand)
        if kind == "rejected" and entry:
            r = entry["rect_image"]
            draw.rectangle(
                [r[0], r[1], r[2] - 1, r[3] - 1], outline=(60, 90, 240), width=2
            )
    for ci, cand in enumerate(trace["candidates"]):
        if ci not in matched_cand:
            r = cand["rect"]
            draw.rectangle([r[0], r[1], r[2] - 1, r[3] - 1], outline=(240, 160, 40), width=1)
    for ci, cand in enumerate(trace["candidates"]):
        if ci in matched_cand:
            r = cand["rect"]
            draw.rectangle([r[0], r[1], r[2] - 1, r[3] - 1], outline=(40, 200, 220), width=2)
    for gi, gt in enumerate(gts):
        missed = gi in missed_gts
        draw.rectangle(
            [gt[0], gt[1], gt[2] - 1, gt[3] - 1],
            outline=(230, 40, 40) if missed else (60, 200, 60),
            width=5 if missed else 2,
        )
    img.save(out_path)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("dump_dir", type=pathlib.Path)
    parser.add_argument("--gt-dir", type=pathlib.Path, default=None)
    parser.add_argument("--iou", type=float, nargs="+", default=[0.5, 0.7])
    parser.add_argument("--render", type=pathlib.Path, default=None)
    args = parser.parse_args()

    gt_dir = args.gt_dir or (args.dump_dir / "gt")
    traces = sorted(args.dump_dir.glob("*-trace.json"))
    if not traces:
        print(f"no *-trace.json in {args.dump_dir} — run shotori with SHOTORI_REGIONDUMP set")
        return 1

    reasons = Counter()
    totals = {t: [0, 0] for t in args.iou}  # threshold -> [matched_gt, total_gt]
    total_candidates = 0

    for trace_path in traces:
        stem = trace_path.name[: -len("-trace.json")]
        trace = json.loads(trace_path.read_text())
        gts = load_gt(gt_dir / f"{stem}.json")
        candidates = trace["candidates"]
        total_candidates += len(candidates)
        reasons.update(p["rejected"] for p in trace["proposals"] if p["rejected"])

        print(f"\n== {stem}: {len(gts) if gts is not None else '-'} gt, "
              f"{len(candidates)} candidates, {len(trace['proposals'])} proposals")

        if gts is None:
            print("   no ground truth (regiondetect_workbench.py) — stats only")
            if args.render:
                # candidates-only overlay: eyeball detection quality
                # before annotating
                args.render.mkdir(parents=True, exist_ok=True)
                img = Image.open(args.dump_dir / f"{stem}-input.png").convert("RGB")
                draw = ImageDraw.Draw(img)
                for cand in candidates:
                    r = cand["rect"]
                    draw.rectangle([r[0], r[1], r[2] - 1, r[3] - 1],
                                   outline=(40, 200, 220), width=2)
                img.save(args.render / f"{stem}-eval.png")
            continue

        missed = set(range(len(gts)))
        lines = []
        for threshold in args.iou:
            matches, matched_gt = greedy_match(gts, candidates, threshold)
            missed -= matched_gt
            recall = len(matched_gt) / len(gts) if gts else 1.0
            precision = len(matches) / len(candidates) if candidates else 0.0
            f1 = (2 * precision * recall / (precision + recall)) if precision + recall else 0.0
            totals[threshold][0] += len(matched_gt)
            totals[threshold][1] += len(gts)
            lines.append(f"   @IoU {threshold:.2f}: P {precision:5.1%}  R {recall:5.1%}"
                         f"  ({len(matched_gt)}/{len(gts)})  F1 {f1:.3f}")
        print("\n".join(lines))

        if missed and args.render is None:
            for gi in sorted(missed):
                kind, entry, overlap = diagnose_miss(gts[gi], trace, set())
                if entry is None:
                    print(f"   MISS {gts[gi]}: no overlapping proposal at all")
                    continue
                if kind == "rejected":
                    print(
                        f"   MISS {gts[gi]}: rejected '{entry['rejected']}' IoU {overlap:.2f}"
                        f"  [{entry['source']}] sides L/T/R/B"
                        f" {entry['sides'][0]:.2f}/{entry['sides'][1]:.2f}"
                        f"/{entry['sides'][2]:.2f}/{entry['sides'][3]:.2f}"
                        f" weakest {entry['weakest_side']:.2f} second {entry['second_side']:.2f}"
                        f" support {entry['support']:.2f} density {entry['density']:.2f}"
                        f" inset {entry['inset_support']:.2f}"
                    )
                else:
                    print(
                        f"   MISS {gts[gi]}: candidate exists but IoU {overlap:.2f} < threshold"
                        f"  (quality {entry['quality']:.2f}, {entry['sources']})"
                    )

        if args.render:
            args.render.mkdir(parents=True, exist_ok=True)
            render(
                args.render / f"{stem}-eval.png",
                args.dump_dir / f"{stem}-input.png",
                gts,
                trace,
                greedy_match(gts, candidates, args.iou[0])[0],
                missed,
            )

    print(f"\n== overall candidates: {total_candidates}")
    for threshold, (matched, total) in sorted(totals.items()):
        if total:
            print(f"   recall @IoU {threshold:.2f}: {matched}/{total} = {matched / total:5.1%}")
    print("== rejection reasons:")
    for reason, count in reasons.most_common():
        print(f"   {reason:14s} {count}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
