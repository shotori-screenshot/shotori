#!/usr/bin/env python3
"""Region-detect grading workbench: curate ground truth, watch the score.

The annotation step reframed as grading: the detector's and the vision
model's proposed regions come to you one at a time (triage), you accept
or reject each, and the live recall of the detector against your
growing answer key sits in the status bar. F5 runs the full eval
(precision/recall, miss diagnosis, overlay renders) without leaving
the window.

Usage:
  regiondetect_workbench.py DUMP_DIR [STEM]

DUMP_DIR holds a SHOTORI_REGIONDUMP dump (`<stem>-trace.json`,
`<stem>-input.png`). STEM picks the screen; default is the first
without a saved answer key.

Keys:
  Tab            triage: zoom to the next pending draft box
  y / Enter      accept the triaged box (draft -> answer key)
  d              delete the triaged draft
  Esc            back to overview
  left-drag      draw a new answer rect (snap-free)
  click inside   delete the smallest answer rect under the cursor,
                 or accept the smallest draft if no answer rect hit
  wheel          zoom at cursor      middle/shift-drag  pan
  v / c          toggle draft / detector-candidate overlays
  z              undo    s save    n next screen (auto-saves)
  F5             run the eval script, show the report
  F1             help    q quit

Files: the answer key lands in <dump>/gt/<stem>.json (what
regiondetect_eval.py reads); the pending draft queue is kept in
<dump>/gt/<stem>.queue.json so a reopened session resumes where you
left off. The draft queue is seeded from vision/<stem>-seed.json when
present, else from the detector's own candidates.
"""
import json
import pathlib
import subprocess
import sys
import tkinter as tk
from tkinter import font as tkfont

from PIL import Image, ImageDraw, ImageTk

EVAL_SCRIPT = pathlib.Path(__file__).with_name("regiondetect_eval.py")
CLICK_SLOP = 6  # screen px: press+release closer than this is a click
FIXED_MARGIN = 3  # triage context margin multiplier


def iou(a, b):
    left = max(a[0], b[0])
    top = max(a[1], b[1])
    right = min(a[2], b[2])
    bottom = min(a[3], b[3])
    if right <= left or bottom <= top:
        return 0.0
    inter = (right - left) * (bottom - top)
    return inter / ((a[2] - a[0]) * (a[3] - a[1]) + (b[2] - b[0]) * (b[3] - b[1]) - inter)


def recall_of(gts, candidates, threshold=0.5):
    """Fraction of GT covered by some candidate at `threshold`."""
    if not gts:
        return None
    hit = 0
    for gt in gts:
        if any(iou(gt, cand["rect"]) >= threshold for cand in candidates):
            hit += 1
    return hit / len(gts)


class Workbench:
    def __init__(self, root, dump_dir, stem=None):
        self.root = root
        self.dump = dump_dir
        self.gt_dir = dump_dir / "gt"
        self.stems = sorted(p.name[: -len("-trace.json")] for p in dump_dir.glob("*-trace.json"))
        if not self.stems:
            raise SystemExit(f"no *-trace.json under {dump_dir}")
        if stem and stem in self.stems:
            self.stem = stem
        else:
            self.stem = next(
                (s for s in self.stems if not (self.gt_dir / f"{s}.json").exists()),
                self.stems[0],
            )
        self.history = []  # undo stack
        self.dirty = False
        self.triaged = None  # index into self.drafts while in triage mode
        self.show_drafts = True
        self.show_detector = True
        self.mode = "overview"

        self.canvas = tk.Canvas(root, bg="#202228", cursor="crosshair")
        self.canvas.pack(fill=tk.BOTH, expand=True)
        self.status = tk.Label(root, anchor="w", font=("monospace", 10), bg="#16181c", fg="#ddd")
        self.status.pack(fill=tk.X)
        self.help_line = tk.Label(
            root,
            anchor="w",
            font=("monospace", 9),
            bg="#16181c",
            fg="#888",
            text="Tab 过堂(y收/d毙) | 拖画框/点框删 | 滚轮缩放 | v草稿 c检测器 | z撤销 s保存 n换屏 F5出分 F1帮助 q退出",
        )
        self.help_line.pack(fill=tk.X)

        self.canvas.bind("<Configure>", lambda event: self.render())
        self.canvas.bind("<ButtonPress-1>", self.on_press)
        self.canvas.bind("<B1-Motion>", self.on_motion)
        self.canvas.bind("<ButtonRelease-1>", self.on_release)
        self.canvas.bind("<ButtonPress-2>", self.on_pan_start)
        self.canvas.bind("<B2-Motion>", self.on_pan_move)
        self.canvas.bind("<Button-4>", lambda e: self.on_wheel(1.25))
        self.canvas.bind("<Button-5>", lambda e: self.on_wheel(0.8))
        self.canvas.bind("<MouseWheel>", lambda e: self.on_wheel(1.25 if e.delta > 0 else 0.8))
        self.canvas.bind("<Shift-ButtonPress-1>", self.on_pan_start)
        self.canvas.bind("<Shift-B1-Motion>", self.on_pan_move)
        for key in ("Tab", "y", "d", "Escape", "v", "c", "z", "s", "n", "q"):
            root.bind(key, lambda event, k=key: self.on_key(k))
        root.bind("<Return>", lambda event: self.on_key("y"))
        root.bind("<F5>", lambda event: self.run_eval())
        root.bind("<F1>", lambda event: self.show_help())

        self.load_screen(self.stem)

    # ── data ─────────────────────────────────────────────────────────

    def load_screen(self, stem):
        self.stem = stem
        self.trace = json.loads((self.dump / f"{stem}-trace.json").read_text())
        self.detector = self.trace["candidates"]
        self.image = Image.open(self.dump / f"{stem}-input.png").convert("RGB")

        self.gt = []  # accepted answer rects
        gt_path = self.gt_dir / f"{stem}.json"
        if gt_path.exists():
            data = json.loads(gt_path.read_text())
            self.gt = [list(map(int, r)) for r in (data["rects"] if isinstance(data, dict) else data)]

        self.drafts = []  # pending review rects
        queue_path = self.gt_dir / f"{stem}.queue.json"
        if queue_path.exists():
            self.drafts = [list(map(int, r)) for r in json.loads(queue_path.read_text())]
        else:
            seed = self.dump / "vision" / f"{stem}-seed.json"
            if not seed.exists():
                # fall back to the detector's own candidates as the queue
                seed = self.dump / f"{stem}-trace.json"
            if seed.exists():
                data = json.loads(seed.read_text())
                if isinstance(data, dict) and "candidates" in data:
                    rects = [c["rect"] for c in data["candidates"]]
                elif isinstance(data, dict):
                    rects = data.get("rects", [])
                else:
                    rects = data
                self.drafts = [list(map(int, r)) for r in rects]
        self.history = []
        self.dirty = False
        self.triaged = None
        self.mode = "overview"

        # fit view
        canvas_w = max(self.canvas.winfo_width(), 800)
        canvas_h = max(self.canvas.winfo_height(), 600)
        self.zoom = min(canvas_w / self.image.width, canvas_h / self.image.height, 1.0)
        self.origin = [0.0, 0.0]
        self.root.title(f"regiondetect workbench — {stem}")
        self.render()

    def save(self):
        self.gt_dir.mkdir(parents=True, exist_ok=True)
        (self.gt_dir / f"{self.stem}.json").write_text(
            json.dumps({"rects": self.gt}, indent=1)
        )
        (self.gt_dir / f"{self.stem}.queue.json").write_text(json.dumps(self.drafts))
        self.dirty = False
        self.set_status("saved")

    # ── view transform ───────────────────────────────────────────────

    def clamp_view(self):
        canvas_w = max(self.canvas.winfo_width(), 1)
        canvas_h = max(self.canvas.winfo_height(), 1)
        view_w, view_h = canvas_w / self.zoom, canvas_h / self.zoom
        self.origin[0] = min(max(self.origin[0], 0), max(0, self.image.width - view_w))
        self.origin[1] = min(max(self.origin[1], 0), max(0, self.image.height - view_h))

    def to_screen(self, rect):
        ox, oy = self.origin
        z = self.zoom
        return (
            (rect[0] - ox) * z,
            (rect[1] - oy) * z,
            (rect[2] - ox) * z,
            (rect[3] - oy) * z,
        )

    def to_image(self, sx, sy):
        return (sx / self.zoom + self.origin[0], sy / self.zoom + self.origin[1])

    # ── rendering ────────────────────────────────────────────────────

    def render(self):
        if not hasattr(self, "image"):
            return
        self.clamp_view()
        canvas_w = self.canvas.winfo_width()
        canvas_h = self.canvas.winfo_height()
        ox, oy = self.origin
        crop = self.image.crop(
            (
                int(ox),
                int(oy),
                min(self.image.width, int(ox + canvas_w / self.zoom) + 1),
                min(self.image.height, int(oy + canvas_h / self.zoom) + 1),
            )
        )
        scaled = crop.resize(
            (max(1, int(crop.width * self.zoom)), max(1, int(crop.height * self.zoom))),
            Image.BILINEAR,
        )
        self.photo = ImageTk.PhotoImage(scaled)
        canvas = self.canvas
        canvas.delete("all")
        canvas.create_image(0, 0, image=self.photo, anchor=tk.NW)

        if self.show_detector:
            for cand in self.detector:
                self.draw_rect(cand["rect"], outline="#2fb3c9", width=1, dashes=(3, 3))
        if self.show_drafts:
            for index, rect in enumerate(self.drafts):
                highlight = self.mode == "triage" and index == self.triaged
                self.draw_rect(
                    rect,
                    outline="#ffd23f" if highlight else "#3aa0ff",
                    width=4 if highlight else 2,
                    label=str(index) if self.mode == "triage" else None,
                )
        for rect in self.gt:
            self.draw_rect(rect, outline="#22cc44", width=2)
        if self.mode == "triage" and self.triaged is not None and self.triaged < len(self.drafts):
            rect = self.drafts[self.triaged]
            self.draw_rect(rect, outline="#ffffff", width=1)
        self.update_status()

    def draw_rect(self, rect, outline, width, dashes=None, label=None):
        x0, y0, x1, y1 = self.to_screen(rect)
        if x1 < 0 or y1 < 0 or x0 > self.canvas.winfo_width() or y0 > self.canvas.winfo_height():
            return
        self.canvas.create_rectangle(
            x0, y0, x1, y1, outline=outline, width=width, dash=dashes
        )
        if label:
            self.canvas.create_rectangle(
                x0, max(0, y0 - 22), x0 + 26, max(22, y0), fill=outline, outline=""
            )
            self.canvas.create_text(
                x0 + 13, max(0, y0 - 22) + 11, text=label, fill="#000", font=("monospace", 11, "bold")
            )

    # ── status ───────────────────────────────────────────────────────

    def update_status(self):
        recall = recall_of(self.gt, self.detector)
        recall_text = f"{recall:.0%}" if recall is not None else "—"
        mode = (
            f"过堂 {self.triaged + 1}/{len(self.drafts)}"
            if self.mode == "triage" and self.drafts
            else "全景"
        )
        self.status.config(
            fg="#eee",
            text=f"{self.stem} | 模式 {mode} | 答案 {len(self.gt)} | 待审草稿 {len(self.drafts)}"
            f" | 检测器召回(vs你的答案) {recall_text} | {'未保存' if self.dirty else '已保存'}",
        )

    def set_status(self, message):
        self.status.config(fg="#ffd23f", text=message)

    # ── input: keyboard ──────────────────────────────────────────────

    def on_key(self, key):
        if key == "q":
            if self.dirty:
                self.save()
            self.root.destroy()
        elif key == "s":
            self.save()
            self.render()
        elif key == "n":
            if self.dirty:
                self.save()
            index = self.stems.index(self.stem)
            self.load_screen(self.stems[(index + 1) % len(self.stems)])
        elif key == "Tab":
            self.triage_next()
        elif key == "Escape":
            self.mode = "overview"
            self.triaged = None
            self.render()
        elif key in ("y", "d"):
            if self.mode == "triage" and self.drafts and self.triaged is not None:
                if key == "y":
                    self.accept_triaged()
                else:
                    self.delete_triaged()
                if self.drafts:
                    self.triaged %= len(self.drafts)
                    self.focus_triaged()
                else:
                    self.mode = "overview"
                    self.set_status("草稿全部过完！s 保存，或继续手画")
                    self.render()
        elif key == "v":
            self.show_drafts = not self.show_drafts
            self.render()
        elif key == "c":
            self.show_detector = not self.show_detector
            self.render()
        elif key == "z":
            self.undo()

    def triage_next(self):
        if not self.drafts:
            self.set_status("没有待审草稿")
            return
        self.mode = "triage"
        if self.triaged is None or self.triaged >= len(self.drafts):
            self.triaged = 0
        self.focus_triaged()

    def focus_triaged(self):
        rect = self.drafts[self.triaged]
        cx, cy = (rect[0] + rect[2]) / 2, (rect[1] + rect[3]) / 2
        canvas_w = max(self.canvas.winfo_width(), 1)
        canvas_h = max(self.canvas.winfo_height(), 1)
        span_x = max((rect[2] - rect[0]) * FIXED_MARGIN, 300)
        span_y = max((rect[3] - rect[1]) * FIXED_MARGIN, 300)
        self.zoom = min(canvas_w / span_x, canvas_h / span_y, 4.0)
        self.origin = [cx - canvas_w / self.zoom / 2, cy - canvas_h / self.zoom / 2]
        self.render()

    def accept_triaged(self):
        rect = self.drafts.pop(self.triaged)
        self.gt.append(rect)
        self.history.append(("accept", rect))
        self.dirty = True

    def delete_triaged(self):
        rect = self.drafts.pop(self.triaged)
        self.history.append(("drop", rect))
        self.dirty = True
        self.triaged = self.triaged - 1 if self.triaged >= len(self.drafts) else self.triaged

    # ── input: mouse ─────────────────────────────────────────────────

    def on_wheel(self, factor):
        sx, sy = self.canvas.winfo_pointerx() - self.canvas.winfo_rootx(), self.canvas.winfo_pointery() - self.canvas.winfo_rooty()
        img_x, img_y = self.to_image(sx, sy)
        self.zoom = min(max(self.zoom * factor, 0.05), 16.0)
        self.origin = [img_x - sx / self.zoom, img_y - sy / self.zoom]
        self.render()

    def on_pan_start(self, event):
        self.pan_at = (event.x, event.y)

    def on_pan_move(self, event):
        if not getattr(self, "pan_at", None):
            return
        dx = (event.x - self.pan_at[0]) / self.zoom
        dy = (event.y - self.pan_at[1]) / self.zoom
        self.pan_at = (event.x, event.y)
        self.origin[0] -= dx
        self.origin[1] -= dy
        self.render()

    def on_press(self, event):
        if self.mode == "triage":
            # triage is keyboard-driven: a click only exits the mode
            # (swallowed here so it cannot double as delete/accept)
            self.mode = "overview"
            self.triaged = None
            self.render()
            self.press_at = None
            return
        self.press_at = (event.x, event.y)
        self.preview = None

    def on_motion(self, event):
        if not getattr(self, "press_at", None):
            return
        if self.preview:
            self.canvas.delete(self.preview)
        self.preview = self.canvas.create_rectangle(
            self.press_at[0], self.press_at[1], event.x, event.y, outline="#ff3333", width=2
        )

    def on_release(self, event):
        if not getattr(self, "press_at", None):
            return
        start = self.press_at
        self.press_at = None
        if self.preview:
            self.canvas.delete(self.preview)
            self.preview = None
        moved = abs(event.x - start[0]) > CLICK_SLOP or abs(event.y - start[1]) > CLICK_SLOP
        img_start = self.to_image(*start)
        img_end = self.to_image(event.x, event.y)
        if moved:
            left, right = sorted((img_start[0], img_end[0]))
            top, bottom = sorted((img_start[1], img_end[1]))
            if right - left >= 8 and bottom - top >= 8:
                self.gt.append([round(left), round(top), round(right), round(bottom)])
                self.history.append(("add", len(self.gt) - 1))
                self.dirty = True
                self.render()
            return
        # a click: smallest answer rect containing the point -> delete,
        # else smallest draft -> accept
        point = (round(img_start[0]), round(img_start[1]))

        def containing(rects):
            hits = [
                (index, rect)
                for index, rect in enumerate(rects)
                if rect[0] <= point[0] < rect[2] and rect[1] <= point[1] < rect[3]
            ]
            hits.sort(key=lambda item: (item[1][2] - item[1][0]) * (item[1][3] - item[1][1]))
            return hits[0][0] if hits else None

        index = containing(self.gt)
        if index is not None:
            self.history.append(("del_gt", index, self.gt.pop(index)))
            self.dirty = True
            self.render()
            return
        index = containing(self.drafts)
        if index is not None:
            rect = self.drafts.pop(index)
            self.gt.append(rect)
            self.history.append(("accept", rect))
            self.dirty = True
            self.render()

    def undo(self):
        if not self.history:
            self.set_status("没有可撤销的操作")
            return
        op = self.history.pop()
        if op[0] == "add":
            self.gt.pop(op[1])
        elif op[0] == "del_gt":
            self.gt.insert(min(op[1], len(self.gt)), op[2])
        elif op[0] == "accept":
            if op[2] < len(self.gt) and self.gt[op[2]] == op[1]:
                self.gt.pop(op[2])
            self.drafts.append(op[1])
        elif op[0] == "drop":
            self.drafts.append(op[1])
        self.dirty = True
        self.render()

    # ── eval integration ─────────────────────────────────────────────

    def run_eval(self):
        if self.dirty:
            self.save()
        self.set_status("eval 运行中…")
        self.root.update_idletasks()
        result = subprocess.run(
            [sys.executable, str(EVAL_SCRIPT), str(self.dump),
             "--render", str(self.dump / "render")],
            capture_output=True, text=True,
        )
        report = result.stdout + result.stderr
        window = tk.Toplevel(self.root)
        window.title("regiondetect_eval report")
        text = tk.Text(window, width=130, height=40, font=("monospace", 10))
        text.pack(fill=tk.BOTH, expand=True)
        text.insert("1.0", report or "(no output)")
        self.set_status(f"eval 完成（exit {result.returncode}）；对比图在 {self.dump / 'render'}")
        self.render()

    def show_help(self):
        window = tk.Toplevel(self.root)
        window.title("help")
        text = tk.Text(window, width=90, height=26, font=("monospace", 11))
        text.pack(fill=tk.BOTH, expand=True)
        text.insert("1.0", __doc__)


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(1)
    dump_dir = pathlib.Path(sys.argv[1])
    stem = sys.argv[2] if len(sys.argv) > 2 else None
    root = tk.Tk()
    root.geometry("1400x950")
    Workbench(root, dump_dir, stem)
    root.mainloop()


if __name__ == "__main__":
    main()
