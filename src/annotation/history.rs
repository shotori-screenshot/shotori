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
    /// One gesture that moved or restyled several shapes at once (a
    /// multi-selection drag, a batch style edit): undo restores every
    /// snapshot as ONE step, so the selection the user built does not
    /// come back half-undone.
    EditMany { edits: Vec<ShapeEdit> },
    /// Single or grouped deletion, including each numbering change in gesture order.
    Remove {
        removals: Vec<ShapeRemoval>,
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

/// One shape's pre/post snapshot inside a multi-shape edit.
pub(super) type ShapeEdit = (usize, Shape, Shape);

/// Indices refer to the list immediately before this mark was removed.
#[derive(Clone)]
pub(super) struct ShapeRemoval {
    ix: usize,
    shape: Shape,
    renumbered: Vec<(usize, u32)>,
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
        self.selected = if self.auto_select {
            vec![self.shapes.len() - 1]
        } else {
            Vec::new()
        };
    }

    /// Delete a mark and optionally close a numbering gap in the same history entry.
    pub(crate) fn delete_shape(&mut self, ix: usize) {
        self.delete_shapes(vec![ix]);
    }

    /// Delete valid indices as one reversible action, including sequence renumbering.
    pub(super) fn delete_shapes(&mut self, mut indices: Vec<usize>) -> bool {
        indices.retain(|&ix| ix < self.shapes.len());
        indices.sort_unstable();
        indices.dedup();
        if indices.is_empty() {
            return false;
        }
        self.end_size_drag();
        self.selected.clear();
        self.text_original = None;
        let number_before = self.number_override;
        let mut removals = Vec::with_capacity(indices.len());
        // Descending indices keep not-yet-removed marks at their original indices.
        for ix in indices.into_iter().rev() {
            let shape = self.shapes[ix].clone();
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
            removals.push(ShapeRemoval {
                ix,
                shape,
                renumbered,
            });
        }
        self.history.push(HistoryEntry::Remove {
            removals,
            number_before,
            number_after: self.number_override,
        });
        self.redo.clear();
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
        // A text edit parks the original shape in text_original and
        // undoes would shift indices under it, making finish_text_edit
        // overwrite the wrong shape. The key context normally prevents
        // this dispatch; make the immunity structural.
        if self.text_original.is_some() {
            return effect;
        }
        self.pressed = false;
        self.selected.clear();
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
                HistoryEntry::EditMany { edits } => {
                    for (ix, before, _) in edits.iter() {
                        if let Some(shape) = self.shapes.get_mut(*ix) {
                            *shape = before.clone();
                        }
                    }
                }
                HistoryEntry::Remove {
                    removals,
                    number_before,
                    ..
                } => {
                    for removal in removals.iter().rev() {
                        self.shapes.insert(removal.ix, removal.shape.clone());
                        for &(ix, number) in &removal.renumbered {
                            self.shapes[ix].number = Some(number);
                        }
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
        // index here is a user-reachable OOB (delete → undo →
        // re-select → redo → wheel over canvas).
        self.selected.clear();
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
                HistoryEntry::EditMany { edits } => {
                    for (ix, _, after) in edits.iter() {
                        if let Some(shape) = self.shapes.get_mut(*ix) {
                            *shape = after.clone();
                        }
                    }
                }
                HistoryEntry::Remove {
                    removals,
                    number_after,
                    ..
                } => {
                    for removal in removals {
                        for &(ix, number) in &removal.renumbered {
                            self.shapes[ix].number = Some(number - 1);
                        }
                        self.shapes.remove(removal.ix);
                    }
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
