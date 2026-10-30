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
    Add {
        shape: Shape,
        number_before: Option<u32>,
        number_after: Option<u32>,
    },
    NumberCursor {
        before: Option<u32>,
        after: Option<u32>,
    },
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
        renumbered: Vec<(usize, u32)>,
        number_before: Option<u32>,
        number_after: Option<u32>,
    },
    /// Clear-all (issue #15): every placed shape leaves as ONE entry,
    /// so a single undo restores the whole sequence in order. A
    /// whole-list replacement rather than N per-shape `Remove`s: the
    /// saved sequence is self-describing, so undo puts the exact list
    /// back no matter what interleaves after the clear — no index
    /// arithmetic over a list that empties and refills.
    RemoveAll { shapes: Vec<Shape> },
}

impl Annotations {
    /// Append a placed mark, invalidate redo, and apply the selection preference.
    pub(super) fn record_add(&mut self, shape: Shape) {
        self.record_placement(shape, false);
    }

    pub(super) fn record_placement(&mut self, shape: Shape, repeat_number: bool) {
        let number_before = self.number_override;
        if !repeat_number
            && number_before.is_some()
            && let Some(value) = shape.number
        {
            self.number_override = Some(value.saturating_add(1));
        }
        self.shapes.push(shape.clone());
        self.history.push(HistoryEntry::Add {
            shape,
            number_before,
            number_after: self.number_override,
        });
        self.redo.clear();
        self.selected = self.auto_select.then(|| self.shapes.len() - 1);
    }

    /// Delete a mark and optionally close a numbering gap in the same history entry.
    pub(crate) fn delete_shape(&mut self, ix: usize) {
        let Some(shape) = self.shapes.get(ix).cloned() else {
            return;
        };
        self.end_size_drag();
        let number_before = self.number_override;
        let mut renumbered = Vec::new();
        if self.renumber_on_delete
            && let Some(removed) = shape.number
        {
            for (other_ix, other) in self.shapes.iter_mut().enumerate() {
                if let Some(number) = other.number.filter(|number| *number > removed) {
                    renumbered.push((other_ix, number));
                    other.number = Some(number - 1);
                }
            }
            if let Some(next) = self.number_override.filter(|next| *next > removed) {
                self.number_override = Some(next - 1);
            }
        }
        self.shapes.remove(ix);
        self.history.push(HistoryEntry::Remove {
            ix,
            shape,
            renumbered,
            number_before,
            number_after: self.number_override,
        });
        self.redo.clear();
        self.selected = None;
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
        // A text edit parks the original shape in text_original and
        // undoes would shift indices under it, making finish_text_edit
        // overwrite the wrong shape. The key context normally prevents
        // this dispatch; make the immunity structural.
        if self.text_original.is_some() {
            return effect;
        }
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
                HistoryEntry::Add { number_before, .. } => {
                    self.shapes.pop();
                    self.number_override = *number_before;
                }
                HistoryEntry::NumberCursor { before, .. } => {
                    self.number_override = *before;
                }
                HistoryEntry::Edit { ix, before, .. } => {
                    if let Some(shape) = self.shapes.get_mut(*ix) {
                        *shape = before.clone();
                    }
                }
                HistoryEntry::Remove {
                    ix,
                    shape,
                    renumbered,
                    number_before,
                    ..
                } => {
                    self.shapes.insert(*ix, shape.clone());
                    for &(ix, number) in renumbered {
                        self.shapes[ix].number = Some(number);
                    }
                    self.number_override = *number_before;
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
        // Same index-shift hazard as undo(): see the text_original gate
        // there.
        if self.text_original.is_some() {
            return effect;
        }
        // Mirror undo(): any shape mutation invalidates the selected
        // index. Remove/RemoveAll shift or clear the list, and a stale
        // Some(ix) here is a user-reachable OOB (delete → undo →
        // re-select → redo → wheel over canvas).
        self.selected = None;
        // Unlike undo(), a live draft blocks redo entirely: there is
        // nothing meaningful to redo mid-stroke, and cancelling the
        // user's in-flight gesture as a side effect of Ctrl+Y would be
        // surprising.
        if self.draft.is_none()
            && let Some(entry) = self.redo.pop()
        {
            match &entry {
                HistoryEntry::Selection { after, .. } => {
                    effect = HistoryEffect::Selection(*after);
                }
                HistoryEntry::Add {
                    shape,
                    number_after,
                    ..
                } => {
                    self.shapes.push(shape.clone());
                    self.number_override = *number_after;
                }
                HistoryEntry::NumberCursor { after, .. } => {
                    self.number_override = *after;
                }
                HistoryEntry::Edit { ix, after, .. } => {
                    if let Some(shape) = self.shapes.get_mut(*ix) {
                        *shape = after.clone();
                    }
                }
                HistoryEntry::Remove {
                    ix,
                    renumbered,
                    number_after,
                    ..
                } => {
                    for &(ix, number) in renumbered {
                        self.shapes[ix].number = Some(number - 1);
                    }
                    self.shapes.remove(*ix);
                    self.number_override = *number_after;
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
