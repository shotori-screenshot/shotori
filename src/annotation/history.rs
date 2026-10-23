//! A shared undo/redo ledger for crop geometry and annotations.
//! Each completed gesture records one reversible entry. Removal indices
//! remain valid because entries are unwound in reverse order.

use super::{Annotations, Shape};
use gpui_kit::{Bounds, Pixels};

/// Session geometry restored by a history step.
pub(crate) enum HistoryEffect {
    None,
    Selection(Bounds<Pixels>),
}

/// Both directions of a completed edit. Indices match the shape list at
/// the moment of recording and are restored by reverse-order undo.
#[derive(Clone)]
pub(super) enum HistoryEntry {
    Add(Shape),
    Selection {
        before: Bounds<Pixels>,
        after: Bounds<Pixels>,
    },
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
    /// Append a placed mark, invalidate redo and leave it unselected.
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

    /// Record a finalized crop change in the same ledger as annotation edits.
    pub(crate) fn record_selection(&mut self, before: Bounds<Pixels>, after: Bounds<Pixels>) {
        if before != after {
            self.history.push(HistoryEntry::Selection { before, after });
            self.redo.clear();
        }
    }

    pub(crate) fn undo(&mut self) -> HistoryEffect {
        let mut effect = HistoryEffect::None;
        self.pressed = false;
        self.selected = None;
        if self.draft.take().is_some() {
            return effect;
        }
        if let Some(entry) = self.history.pop() {
            match &entry {
                HistoryEntry::Selection { before, .. } => {
                    effect = HistoryEffect::Selection(*before);
                }
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
        effect
    }

    pub(crate) fn redo(&mut self) -> HistoryEffect {
        let mut effect = HistoryEffect::None;
        if self.draft.is_none()
            && let Some(entry) = self.redo.pop()
        {
            match &entry {
                HistoryEntry::Selection { after, .. } => {
                    effect = HistoryEffect::Selection(*after);
                }
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
        effect
    }
}
