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
    /// Batch placement from duplicate or Alt-drag cloning: one undo removes
    /// every cloned mark and restores the sequence cursor.
    AddMany {
        shapes: Vec<Shape>,
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
    /// Z-order change across the committed stack.
    Reorder {
        before: Vec<Shape>,
        after: Vec<Shape>,
    },
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
    pub(super) fn delete_shapes(&mut self, indices: Vec<usize>) -> bool {
        self.remove_shapes(indices, false)
    }

    /// Delete valid indices in descending order, folding consecutive
    /// removals into the open brush-sweep entry when `merge_eraser` and
    /// `self.eraser_entry_open` both hold so one Undo restores the
    /// entire sweep.
    pub(super) fn remove_shapes(&mut self, mut indices: Vec<usize>, merge_eraser: bool) -> bool {
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
        let number_after = self.number_override;
        match self.history.last_mut() {
            Some(HistoryEntry::Remove {
                removals: existing,
                number_after: existing_after,
                ..
            }) if merge_eraser && self.eraser_entry_open => {
                existing.extend(removals);
                *existing_after = number_after;
            }
            _ => {
                self.history.push(HistoryEntry::Remove {
                    removals,
                    number_before,
                    number_after,
                });
                self.redo.clear();
                self.eraser_entry_open = merge_eraser;
            }
        }
        true
    }

    /// Apply a discrete style/parameter edit across every selected shape,
    /// recording a single `Edit` or `EditMany` step when any mark changed.
    pub(super) fn edit_selected(&mut self, mut apply: impl FnMut(&mut Shape)) {
        self.end_size_drag();
        if self.selected.is_empty() {
            return;
        }
        let mut before = Vec::with_capacity(self.selected.len());
        for &ix in &self.selected {
            if let Some(shape) = self.shapes.get_mut(ix) {
                let snapshot = shape.clone();
                apply(shape);
                before.push((ix, snapshot));
            }
        }
        self.commit_moves(&before);
    }

    /// Apply a continuous slider edit across every selected shape, merging
    /// consecutive ticks of the same drag into one `Edit` or `EditMany` entry
    /// while always re-deriving from the pre-drag snapshot to avoid float drift.
    pub(super) fn edit_selected_drag(&mut self, mut apply: impl FnMut(&Shape) -> Shape) {
        if self.selected.is_empty() {
            return;
        }
        if self.size_drag_active {
            match self.history.last_mut() {
                Some(HistoryEntry::Edit { ix, before, after })
                    if self.selected.contains(ix)
                        && self.selected.iter().all(|&other| {
                            other == *ix || self.shapes.get(other).is_none_or(|s| apply(s) == *s)
                        }) =>
                {
                    let next = apply(before);
                    if let Some(slot) = self.shapes.get_mut(*ix) {
                        *slot = next.clone();
                    }
                    *after = next;
                    return;
                }
                Some(HistoryEntry::EditMany { edits })
                    if edits.iter().all(|(ix, ..)| self.selected.contains(ix))
                        && self.selected.iter().all(|&other| {
                            edits.iter().any(|(ix, ..)| *ix == other)
                                || self.shapes.get(other).is_none_or(|s| apply(s) == *s)
                        }) =>
                {
                    for (ix, before, after) in edits.iter_mut() {
                        let next = apply(before);
                        if let Some(slot) = self.shapes.get_mut(*ix) {
                            *slot = next.clone();
                        }
                        *after = next;
                    }
                    return;
                }
                _ => {}
            }
        }
        let mut edits = Vec::new();
        for &ix in &self.selected {
            let Some(current) = self.shapes.get(ix) else {
                continue;
            };
            let next = apply(current);
            if next != *current {
                let before = current.clone();
                self.shapes[ix] = next.clone();
                edits.push((ix, before, next));
            }
        }
        match edits.len() {
            0 => {}
            1 => {
                let (ix, before, after) = edits.remove(0);
                self.history.push(HistoryEntry::Edit { ix, before, after });
                self.redo.clear();
                self.size_drag_active = true;
            }
            _ => {
                self.history.push(HistoryEntry::EditMany { edits });
                self.redo.clear();
                self.size_drag_active = true;
            }
        }
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
        let open_sweep = std::mem::replace(&mut self.eraser_entry_open, false);
        if self.draft.take().is_some() {
            return effect;
        }
        // An uncommitted area-eraser rect or an empty brush sweep cancels
        // without unwinding earlier history; a brush sweep that already
        // removed marks falls through to pop that live Remove entry while
        // keeping `self.eraser` open for continued dragging.
        if self.eraser.is_some() && !open_sweep {
            self.eraser = None;
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
                HistoryEntry::AddMany {
                    shapes,
                    number_before,
                    ..
                } => {
                    let keep = self.shapes.len().saturating_sub(shapes.len());
                    self.shapes.truncate(keep);
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
                HistoryEntry::Reorder { before, .. } => {
                    self.shapes = before.clone();
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
        self.eraser_entry_open = false;
        // Unlike undo(), a live draft or eraser sweep blocks redo entirely:
        // there is nothing meaningful to redo mid-stroke, and cancelling the
        // user's in-flight gesture as a side effect of Ctrl+Y would be
        // surprising.
        if self.draft.is_none()
            && self.eraser.is_none()
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
                HistoryEntry::AddMany {
                    shapes,
                    number_after,
                    ..
                } => {
                    self.shapes.extend(shapes.iter().cloned());
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
                HistoryEntry::Reorder { after, .. } => {
                    self.shapes = after.clone();
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
