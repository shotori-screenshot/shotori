#!/usr/bin/env python3
"""Draw ground-truth rectangles on a region-detect dump screenshot.

Half of the smart-selection evaluation loop (scoring: see
scripts/regiondetect_eval.py). Open the `<output>-input.png` from a
SHOTORI_REGIONDUMP dump, drag a rectangle around every region a human
would expect the detector to offer, then press `s`.

Usage:
  regiondetect_annotate.py INPUT_PNG [GT_JSON]

Without GT_JSON the file lands next to the dump as
`gt/<output>.json` (derived from the `-input.png` suffix). Rectangles
are stored half-open [l, t, r, b] in original image pixels — the same
space the detector's trace reports.

Keys: left-drag add · z undo · s save · q quit without saving.
Annotate BEFORE looking at eval renders, so the ground truth stays
honest (you should not be copying the detector's opinions).
"""
import json
import pathlib
import sys
import tkinter as tk

from PIL import Image, ImageTk

# Fit-to-window zoom cap; dump screenshots are full screens.
MAX_WINDOW = 1400


class Annotator:
    def __init__(self, root, image_path, gt_path):
        self.image_path = image_path
        self.gt_path = gt_path
        self.image = Image.open(image_path).convert("RGB")
        self.rects = self.load_existing()
        self.zoom = min(
            1.0, MAX_WINDOW / self.image.width, MAX_WINDOW / self.image.height
        )
        size = (
            round(self.image.width * self.zoom),
            round(self.image.height * self.zoom),
        )
        self.photo = ImageTk.PhotoImage(self.image.resize(size, Image.BILINEAR))

        self.root = root
        self.base_title = f"{image_path.name}  —  drag=rect  z=undo  s=save  q=quit"
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

    def load_existing(self):
        if self.gt_path.exists():
            data = json.loads(self.gt_path.read_text())
            rects = data["rects"] if isinstance(data, dict) else data
            return [list(map(int, rect)) for rect in rects]
        return []

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
        if right - left < 4 or bottom - top < 4:  # a click, not a drag
            return
        self.rects.append([left, top, right, bottom])
        self.redraw()

    def undo(self):
        if self.rects:
            self.rects.pop()
            self.redraw()

    def redraw(self):
        self.canvas.delete("rect")
        for left, top, right, bottom in self.rects:
            self.canvas.create_rectangle(
                left * self.zoom,
                top * self.zoom,
                right * self.zoom,
                bottom * self.zoom,
                outline="#22cc44",
                width=2,
                tags="rect",
            )
        self.root.title(f"{self.base_title}  —  {len(self.rects)} rects")

    def save(self):
        self.gt_path.parent.mkdir(parents=True, exist_ok=True)
        self.gt_path.write_text(json.dumps({"rects": self.rects}, indent=1))
        print(f"saved {len(self.rects)} rects -> {self.gt_path}")


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(1)
    image_path = pathlib.Path(sys.argv[1])
    if len(sys.argv) > 2:
        gt_path = pathlib.Path(sys.argv[2])
    else:
        stem = image_path.name
        for suffix in ("-input.png", ".png"):
            if stem.endswith(suffix):
                stem = stem[: -len(suffix)]
                break
        gt_path = image_path.parent / "gt" / f"{stem}.json"
    root = tk.Tk()
    Annotator(root, image_path, gt_path)
    root.mainloop()


if __name__ == "__main__":
    main()
