#!/usr/bin/env python3
"""Draw ground-truth rectangles on a region-detect dump screenshot.

Half of the smart-selection evaluation loop (scoring: see
scripts/regiondetect_eval.py). Open the `<output>-input.png` from a
SHOTORI_REGIONDUMP dump, curate the region set, press `s`.

Usage:
  regiondetect_annotate.py INPUT_PNG [GT_JSON] [--trace TRACE_JSON] [--no-seed]

With a sibling `<output>-trace.json` (or --trace) the detector's own
candidates are pre-filled as the starting set — blue = program claim,
green = your addition. Curating beats drawing from scratch: click
inside a blue rect you disagree with to remove it, drag the misses in
yourself. If the GT file already exists it is loaded as-is and the
trace is NOT re-seeded (your curation wins over re-running).

Keys: left-drag add · click-inside delete · z undo · s save · q quit.
Rectangles are stored half-open [l, t, r, b] in original image pixels —
the same space the detector's trace reports.

Mind the bias: a seeded answer key measures "is the detector's output
acceptable", not "how good is it from scratch". For an honest recall
number, `--no-seed` and draw from empty on at least a couple of
screens; seeded runs are for fast iteration.
"""
import json
import pathlib
import sys
import tkinter as tk

from PIL import Image, ImageTk

# Fit-to-window zoom cap; dump screenshots are full screens.
MAX_WINDOW = 1400

CLICK_SLOP = 4  # px in image space: below this a press+release is a click


class Annotator:
    def __init__(self, root, image_path, gt_path, trace_path=None, seed=True):
        self.image_path = image_path
        self.gt_path = gt_path
        self.image = Image.open(image_path).convert("RGB")
        self.rects = []  # [l, t, r, b] in image px
        self.seeded = []  # parallel: True = came from the detector
        self.undo_stack = []  # ("add", idx) | ("delete", idx, rect, was_seeded)
        self.load_existing(trace_path, seed)
        self.zoom = min(
            1.0, MAX_WINDOW / self.image.width, MAX_WINDOW / self.image.height
        )
        size = (
            round(self.image.width * self.zoom),
            round(self.image.height * self.zoom),
        )
        self.photo = ImageTk.PhotoImage(self.image.resize(size, Image.BILINEAR))

        self.root = root
        self.base_title = (
            f"{image_path.name}  —  drag=add  click-in=delete  z=undo  s=save  q=quit"
        )
        self.canvas = tk.Canvas(root, width=size[0], height=size[1], cursor="crosshair")
        self.canvas.pack()
        self.canvas.create_image(0, 0, image=self.photo, anchor=tk.NW)
        self.drag_start = None
        self.preview = None
        self.canvas.bind("<ButtonPress-1>", self.press)
        self.canvas.bind("<B1-Motion>", self.drag)
        self.canvas.bind("<ButtonRelease-1>", self.release)
        self.root.bind("z", lambda event: self.undo())
        self.root.bind("s", lambda event: self.save())
        self.root.bind("q", lambda event: root.destroy())
        self.redraw()

    def load_existing(self, trace_path, seed):
        if self.gt_path.exists():
            data = json.loads(self.gt_path.read_text())
            rects = data["rects"] if isinstance(data, dict) else data
            for rect in rects:
                self.rects.append([int(v) for v in rect])
                self.seeded.append(False)
            return
        if trace_path and trace_path.exists() and seed:
            data = json.loads(trace_path.read_text())
            if isinstance(data, dict) and "candidates" in data:
                # detector trace.json shape
                rects = [cand["rect"] for cand in data["candidates"]]
            elif isinstance(data, dict):
                rects = data.get("rects", [])
            else:
                rects = data
            for rect in rects:
                self.rects.append([int(v) for v in rect])
                self.seeded.append(True)
            print(
                f"seeded {len(self.rects)} rects from {trace_path.name}; "
                "click inside the wrong ones, drag in the misses"
            )

    def to_image(self, event):
        return (round(event.x / self.zoom), round(event.y / self.zoom))

    def press(self, event):
        self.drag_start = self.to_image(event)

    def drag(self, event):
        if not self.drag_start:
            return
        if self.preview:
            self.canvas.delete(self.preview)
        self.preview = self.canvas.create_rectangle(
            self.drag_start[0] * self.zoom,
            self.drag_start[1] * self.zoom,
            event.x,
            event.y,
            outline="#ff3333",
            width=2,
        )

    def release(self, event):
        if not self.drag_start:
            return
        start = self.drag_start
        self.drag_start = None
        if self.preview:
            self.canvas.delete(self.preview)
            self.preview = None
        end = self.to_image(event)
        left, right = sorted((start[0], end[0]))
        top, bottom = sorted((start[1], end[1]))
        if right - left < CLICK_SLOP or bottom - top < CLICK_SLOP:
            # a click: delete the topmost rect containing the point
            for index in range(len(self.rects) - 1, -1, -1):
                rect = self.rects[index]
                if rect[0] <= left < rect[2] and rect[1] <= top < rect[3]:
                    self.undo_stack.append(
                        ("delete", index, self.rects.pop(index), self.seeded.pop(index))
                    )
                    self.redraw()
                    return
            return
        self.rects.append([left, top, right, bottom])
        self.seeded.append(False)
        self.undo_stack.append(("add", len(self.rects) - 1))
        self.redraw()

    def undo(self):
        if not self.undo_stack:
            return
        op = self.undo_stack.pop()
        if op[0] == "add":
            index = op[1]
            if index < len(self.rects):
                self.rects.pop(index)
                self.seeded.pop(index)
        else:
            _, index, rect, was_seeded = op
            self.rects.insert(min(index, len(self.rects)), rect)
            self.seeded.insert(min(index, len(self.seeded)), was_seeded)
        self.redraw()

    def redraw(self):
        self.canvas.delete("rect")
        for index, (left, top, right, bottom) in enumerate(self.rects):
            color = "#3aa0ff" if self.seeded[index] else "#22cc44"
            self.canvas.create_rectangle(
                left * self.zoom,
                top * self.zoom,
                right * self.zoom,
                bottom * self.zoom,
                outline=color,
                width=2,
                tags="rect",
            )
        blue = sum(self.seeded)
        self.root.title(
            f"{self.base_title}  —  {len(self.rects)} rects ({blue} from detector)"
        )

    def save(self):
        self.gt_path.parent.mkdir(parents=True, exist_ok=True)
        payload = {"rects": [rect[:] for rect in self.rects]}
        self.gt_path.write_text(json.dumps(payload, indent=1))
        print(f"saved {len(self.rects)} rects -> {self.gt_path}")


def main():
    args = sys.argv[1:]
    seed = "--no-seed" not in args
    args = [arg for arg in args if arg != "--no-seed"]
    trace_arg = None
    if "--trace" in args:
        index = args.index("--trace")
        trace_arg = pathlib.Path(args[index + 1])
        del args[index : index + 2]
    if not args:
        print(__doc__)
        sys.exit(1)
    image_path = pathlib.Path(args[0])
    if len(args) > 1:
        gt_path = pathlib.Path(args[1])
    else:
        stem = image_path.name
        for suffix in ("-input.png", ".png"):
            if stem.endswith(suffix):
                stem = stem[: -len(suffix)]
                break
        gt_path = image_path.parent / "gt" / f"{stem}.json"
    if trace_arg is None:
        for suffix in ("-input.png", ".png"):
            if image_path.name.endswith(suffix):
                candidate = image_path.with_name(
                    image_path.name[: -len(suffix)] + "-trace.json"
                )
                if candidate.exists():
                    trace_arg = candidate
                break

    root = tk.Tk()
    Annotator(root, image_path, gt_path, trace_arg, seed)
    root.mainloop()


if __name__ == "__main__":
    main()
