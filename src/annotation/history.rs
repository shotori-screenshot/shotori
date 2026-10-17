//! The undo/redo ledger. One reversible step per entry; edits record
//! both directions, removals keep their indices stable because the
//! stack unwinds in reverse. The `Add`/`Edit`/`Remove*` variants are
//! constructed across the annotation module (every gesture commits),
//! so the enum is `pub(super)`.

use super::{Annotations, Shape};

/// One reversible step. Placement history was a plain shape stack, but
/// in-place edits need both directions recorded: `before` to undo,
/// `after` to redo. Shape indices are stable because committed shapes
/// only ever leave from the tail via undo.
#[derive(Clone)]
pub(super) enum HistoryEntry {
    Add(Shape),
    Edit {
        ix: usize,
        before: Shape,
        after: Shape,
    },
    /// A mid-sequence removal (the Delete key): undo re-inserts at the
    /// same index. Indices in older entries stay valid because the
    /// stack unwinds in reverse order — each entry's index matches the
    /// moment it was recorded.
    Remove {
        ix: usize,
        shape: Shape,
    },
    /// An eraser gesture's whole-shape deletions (issue #14), one
    /// entry per gesture so a single undo restores everything the
    /// sweep took (and redo re-takes it). Recorded LIVE — removals
    /// hit `shapes` as the brush touches them — so each pair's index
    /// is the moment it left; undo re-inserts in reverse, redo
    /// removes in forward order.
    RemoveMany {
        removals: Vec<(usize, Shape)>,
    },
    /// Clear-all (issue #15): every placed shape leaves as ONE entry,
    /// so a single undo restores the whole sequence in order. A
    /// whole-list replacement rather than N per-shape `Remove`s: the
    /// saved sequence is self-describing, so undo puts the exact list
    /// back no matter what interleaves after the clear — no index
    /// arithmetic over a list that empties and refills.
    RemoveAll {
        shapes: Vec<Shape>,
    },
}

impl Annotations {
    /// Record a committed shape: the Add entry enables undo, and any
    /// diverging action invalidates the redo stack. Also drops the
    /// selection — indices above the tail may have shifted, and since
    /// the 2026-10-06 select-tool flip a placed mark no longer selects
    /// itself: selecting is the select tool's job, never a side effect
    /// of drawing.
    pub(super) fn record_add(&mut self, shape: Shape) {
        self.shapes.push(shape.clone());
        self.history.push(HistoryEntry::Add(shape));
        self.redo.clear();
        self.selected = None;
    }

    /// The stroke eraser's brush radius — half the eraser tool's
    /// entry (or open one). One undo then restores the whole sweep.
    pub(super) fn commit_removals(&mut self, removed: Vec<(usize, Shape)>) -> bool {
        if removed.is_empty() {
            return false;
        }
        self.selected = None; // indices shift below
        match self.history.last_mut() {
            Some(HistoryEntry::RemoveMany { removals }) if self.eraser_entry_open => {
                removals.extend(removed);
            }
            _ => {
                self.history
                    .push(HistoryEntry::RemoveMany { removals: removed });
                self.redo.clear();
                self.eraser_entry_open = true;
            }
        }
        true
    }

    pub(crate) fn undo(&mut self) {
        self.pressed = false;
        self.selected = None;
        if self.draft.take().is_some() {
            return;
        }
        if let Some(entry) = self.history.pop() {
            match &entry {
                HistoryEntry::Add(_) => {
                    self.shapes.pop();
                }
                HistoryEntry::Edit { ix, before, .. } => {
                    if let Some(shape) = self.shapes.get_mut(*ix) {
                        *shape = before.clone();
                    }
                }
                HistoryEntry::Remove { ix, shape } => {
                    self.shapes.insert(*ix, shape.clone());
                }
                HistoryEntry::RemoveMany { removals } => {
                    // reverse gesture order: each index matches the
                    // list state at the moment that shape left
                    for (ix, shape) in removals.iter().rev() {
                        self.shapes.insert(*ix, shape.clone());
                    }
                }
                HistoryEntry::RemoveAll { shapes } => {
                    self.shapes = shapes.clone();
                }
            }
            self.redo.push(entry);
        }
    }
    pub(crate) fn redo(&mut self) {
        if self.draft.is_none()
            && let Some(entry) = self.redo.pop()
        {
            match &entry {
                HistoryEntry::Add(shape) => {
                    self.shapes.push(shape.clone());
                }
                HistoryEntry::Edit { ix, after, .. } => {
                    if let Some(shape) = self.shapes.get_mut(*ix) {
                        *shape = after.clone();
                    }
                }
                HistoryEntry::Remove { ix, .. } => {
                    self.shapes.remove(*ix);
                }
                HistoryEntry::RemoveMany { removals } => {
                    // forward gesture order reproduces the states the
                    // indices were recorded against
                    for (ix, _) in removals {
                        self.shapes.remove(*ix);
                    }
                    self.selected = None;
                }
                HistoryEntry::RemoveAll { .. } => {
                    self.shapes.clear();
                }
            }
            self.history.push(entry);
        }
    }
}
