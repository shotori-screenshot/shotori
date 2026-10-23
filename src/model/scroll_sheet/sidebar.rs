//! A separated right-hand column has its own scroll position. Its captured
//! pixels remain a coherent column when it pins while the document continues.
//! Headings may cross its gutter above the column content.
//! Padding uses the observed gutter background. Document motion across the
//! former gutter bounds the column and preserves a later full-width section.
//! Existing column pixels retain their identity when scrolling resumes. New
//! cross-gutter content waits for overlap evidence before entering either plane.

use std::ops::Range;

use super::floating::FrameEdge;
use super::{Axis, Fold, Placement, RowProfile, ScrollSheet, SheetLimits, sheet_log};

const MIN_BACKGROUND_ROWS: usize = 8;
const COLOR_TOLERANCE: u8 = 3;
const MIN_DETAIL_ROWS: usize = 3;
const MIN_DETAIL_EDGES: usize = 4;

pub(super) struct Sidebar {
    left: usize,
    sheet: ScrollSheet,
    origin: i64,
    parent_origin: i64,
    independent: bool,
    background: Option<Vec<u8>>,
    gutter_color: [u8; 4],
    previous: Vec<u8>,
    previous_frame_edge: Vec<bool>,
    frame_edge: Vec<bool>,
    document_position: i64,
    flow: Option<FlowTail>,
    status: ColumnStatus,
}

/// A verified change to document flow owns its pixels in document coordinates.
/// Retaining them also prevents an old pinned column from painting over them
/// when revisiting an earlier viewport.
struct FlowTail {
    start: i64,
    pixels: Vec<u8>,
    observed: Vec<bool>,
}

#[derive(Clone, Copy)]
enum ColumnStatus {
    Ready,
    Retained,
    Uncovered,
}

impl Sidebar {
    pub(super) fn new(
        width: u32,
        height: u32,
        frame: &[u8],
        options: SheetLimits,
    ) -> Option<Box<Self>> {
        let left = separator(frame, width as usize, height as usize)?;
        let pixels = crop(frame, width as usize, left);
        let sheet = ScrollSheet::new_internal(
            width - left as u32,
            height,
            &pixels,
            options,
            Axis::Vertical,
            false,
        )
        .ok()?;
        let sample = (height as usize * 3 / 4 * width as usize + left) * 4;
        let gutter_color = frame[sample..sample + 4].try_into().ok()?;
        Some(Box::new(Self {
            left,
            sheet,
            origin: 0,
            parent_origin: 0,
            independent: false,
            background: None,
            gutter_color,
            previous: frame.to_vec(),
            previous_frame_edge: vec![false; frame.len() / 4],
            frame_edge: vec![false; frame.len() / 4],
            document_position: 0,
            flow: None,
            status: ColumnStatus::Ready,
        }))
    }

    /// Track the column against its own last accepted observation. A stalled
    /// column is not a sequence of occluding buttons and must not be tiled
    /// through the document's coordinate system.
    pub(super) fn observe(
        &mut self,
        frame: &[u8],
        document_dy: i32,
        prepend: u32,
        output_height: u32,
        content: Range<usize>,
        frame_edge: &FrameEdge,
    ) {
        let width = self.left + self.sheet.width as usize;
        self.frame_edge.copy_from_slice(frame_edge.mask());
        let mut content_frame = (!frame_edge.is_empty()).then(|| frame.to_vec());
        if let Some(frame) = &mut content_frame {
            frame_edge.remove_from_frame(frame, self.gutter_color);
        }
        let frame = content_frame.as_deref().unwrap_or(frame);
        self.sheet.floating.remove_frame_edge(
            &mut self.sheet.canvas,
            self.sheet.width as usize,
            self.left,
            frame_edge,
            self.gutter_color,
        );
        let position = self.document_position + i64::from(document_dy);
        let mut pixels = crop(frame, width, self.left);
        // Returning above a registered document tail restores the column's
        // own alignment and occlusion checks, just as before the tail existed.
        let column_only = self
            .flow
            .as_ref()
            .is_none_or(|flow| position + i64::from(self.sheet.viewport_height) <= flow.start);
        let column_position = if self.independent && column_only {
            let profile =
                RowProfile::from_rgba(&pixels, self.sheet.width, self.sheet.viewport_height);
            match self
                .sheet
                .classify(&self.sheet.stable_profile, &profile, None)
            {
                Placement::Match(estimate) => {
                    Some(self.sheet.state.position + i64::from(estimate.dy))
                }
                Placement::Still => Some(self.sheet.state.position),
                Placement::Reject(_) => None,
            }
        } else {
            None
        };
        if self.independent
            && self.flow.is_none()
            && document_dy != 0
            && let Some(column_position) = column_position
            && let Some(first) =
                self.flow_boundary(frame, width, document_dy, content.clone(), column_position)
        {
            self.flow = Some(FlowTail {
                start: position + first as i64,
                pixels: Vec::new(),
                observed: Vec::new(),
            });
            sheet_log(&format!(
                "column at {} joins document at {} (column position {column_position:?})",
                self.left,
                position + first as i64,
            ));
        }
        let flow_matches = self.flow.as_ref().is_some_and(|flow| {
            let start = (flow.start - position).max(content.start as i64) as usize;
            document_dy != 0
                && self.translated_suffix_matches(
                    frame,
                    width,
                    document_dy,
                    start..content.end,
                    &content,
                )
        });
        if flow_matches && let Some(flow) = &mut self.flow {
            // Every overlapping row must support document motion before any
            // unseen suffix can extend it. Never infer a whole panel's motion
            // from one matching rule or a heading above that panel.
            flow.extend(
                &self.previous,
                width,
                self.left,
                self.document_position,
                content.clone(),
                &self.previous_frame_edge,
            );
            flow.extend(
                frame,
                width,
                self.left,
                position,
                content.clone(),
                &self.frame_edge,
            );
        }
        let captured_detail_end = column_position
            .and_then(|position| self.captured_detail_end(frame, width, content.clone(), position));
        if column_only && let Some(column_position) = column_position {
            // A body decoration can cross a moving panel without turning the
            // panel into document flow. Retain its covered rows, while allowing
            // the independently aligned metadata below to extend the column.
            for y in content.clone() {
                if self.crosses_gutter(frame, width, y)
                    && !self.is_captured_column_row(frame, width, y, column_position)
                    && captured_detail_end.is_some_and(|end| y + 1 < end)
                {
                    self.retain_column_rows(&mut pixels, y..y + 1, column_position);
                }
            }
        }
        let provisional = if column_only
            && let Some(column_position) = column_position
            && let Some(first) = content.clone().find(|&y| {
                self.crosses_gutter(frame, width, y)
                    && !self.is_captured_column_row(frame, width, y, column_position)
                    && !captured_detail_end.is_some_and(|end| y + 1 < end)
            }) {
            // A newly exposed document section has no overlapping observation
            // yet. Keep it out of the column's identity until its motion can be
            // verified; otherwise the next frame would treat it as old metadata.
            let first =
                self.section_prefix(frame, width, document_dy, &content, first, column_position);
            self.retain_column_rows(
                &mut pixels,
                first..self.sheet.viewport_height as usize,
                column_position,
            );
            true
        } else {
            false
        };
        if !provisional && let Some(column_position) = column_position {
            // Parent-owned bottom chrome never enters the independent column
            // plane. Even the first few rows of a new section may lie inside
            // this exclusion before there is enough overlap to recognize it.
            self.retain_column_rows(
                &mut pixels,
                content.end..self.sheet.viewport_height as usize,
                column_position,
            );
        }
        self.document_position = position;
        self.previous.copy_from_slice(frame);
        self.previous_frame_edge.copy_from_slice(&self.frame_edge);
        self.parent_origin -= i64::from(prepend);
        if self
            .flow
            .as_ref()
            .is_some_and(|flow| position + i64::from(self.sheet.viewport_height) > flow.start)
        {
            // A mixed viewport no longer describes a single column translation.
            // Its document section is registered separately, not fed into the
            // column matcher where it could displace already captured metadata.
            self.status = if flow_matches {
                ColumnStatus::Ready
            } else {
                ColumnStatus::Retained
            };
            return;
        }
        let hint = (!self.independent).then_some(document_dy);
        let result = self.sheet.push_with_document_motion(&pixels, hint);
        let dy = match result {
            Fold::GrewBottom { dy, .. } => dy as i32,
            Fold::GrewTop { dy, growth } => {
                self.origin -= i64::from(growth);
                -(dy as i32)
            }
            Fold::Shifted { dy } => dy,
            Fold::Still | Fold::Unchanged => 0,
            Fold::Refused(reason) => {
                // The body has its own verified position. A column refusal
                // must neither freeze it nor write unaligned column pixels.
                sheet_log(&format!("column at {} retained: {reason:?}", self.left));
                0
            }
        };
        self.status = match result {
            Fold::Refused(_) => ColumnStatus::Retained,
            Fold::Unchanged if matches!(self.status, ColumnStatus::Retained) => {
                ColumnStatus::Retained
            }
            _ => ColumnStatus::Ready,
        };
        if provisional {
            self.status = ColumnStatus::Retained;
        }
        let retained = matches!(self.status, ColumnStatus::Retained);
        self.independent |= retained || dy != document_dy;
        if !retained {
            self.background = self.background_row();
        }
        let top = self.origin - self.parent_origin;
        let bottom = top + i64::from(self.sheet.height());
        if self.independent
            && !retained
            && (top > 0 || bottom < i64::from(output_height))
            && self.background.is_none()
        {
            self.status = ColumnStatus::Uncovered;
        }
    }

    /// Preserve known column rows and leave uncaptured rows as gutter color.
    /// Raw pixels from another plane must not become part of column identity.
    fn retain_column_rows(&self, pixels: &mut [u8], rows: Range<usize>, position: i64) {
        let row = self.sheet.width as usize * 4;
        for y in rows {
            let source_y = position + y as i64;
            let target = &mut pixels[y * row..(y + 1) * row];
            if (0..i64::from(self.sheet.height())).contains(&source_y) {
                let source = source_y as usize * row;
                target.copy_from_slice(&self.sheet.canvas[source..source + row]);
            } else {
                for pixel in target.as_chunks_mut::<4>().0 {
                    pixel.copy_from_slice(&self.gutter_color);
                }
            }
        }
    }

    pub(super) fn is_incomplete(&self) -> bool {
        !matches!(self.status, ColumnStatus::Ready)
    }

    pub(super) fn left(&self) -> usize {
        self.left
    }

    pub(super) fn is_independent(&self) -> bool {
        self.independent
    }

    /// A column owns its local overlays only until observed document flow
    /// replaces it. The body resumes ownership below that boundary.
    pub(super) fn overlay_bounds(&self) -> Option<(usize, Range<usize>)> {
        let height = self.sheet.viewport_height as usize;
        let end = self.flow.as_ref().map_or(height, |flow| {
            (flow.start - self.document_position).clamp(0, height as i64) as usize
        });
        (self.independent && end > 0).then_some((self.left, 0..end))
    }

    /// The gutter supplies the page color. Narrow outer chrome must not make
    /// an otherwise plain row unusable, nor be repeated as part of the fill.
    fn background_row(&self) -> Option<Vec<u8>> {
        let width = self.sheet.width as usize;
        let row = width * 4;
        let first = self.sheet.state.position as usize;
        let height = self.sheet.viewport_height as usize;
        let edge = ((width + self.left) / 64).max(4).min(width / 8);
        let mut run = 0;
        for y in (first..first + height).rev() {
            let source = &self.sheet.canvas[y * row..(y + 1) * row - edge * 4];
            let plain = source
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| color_matches(pixel, &self.gutter_color));
            run = if plain { run + 1 } else { 0 };
            if run >= MIN_BACKGROUND_ROWS {
                return Some(self.gutter_color.repeat(width));
            }
        }
        None
    }

    /// Motion must cross the former gutter and match the body independently of
    /// the column. A fixed header or a column merely resuming its own scroll
    /// does not establish a full-width document section. A verified column
    /// position is required: failure to align a partially repainted column
    /// cannot confer document ownership on its still-visible lower content.
    /// Known text anywhere below a candidate boundary keeps that suffix in
    /// the column, even when a foreign rule above it follows document motion.
    fn flow_boundary(
        &self,
        frame: &[u8],
        width: usize,
        dy: i32,
        content: Range<usize>,
        column_position: i64,
    ) -> Option<usize> {
        let right = width - (width / 64).max(4);
        let gutter = (width / 160).max(4);
        let left = self.left.saturating_sub(gutter);
        let mut first = None;
        let mut last = 0;
        let mut rows = 0;
        let captured_detail_end =
            self.captured_detail_end(frame, width, content.clone(), column_position);
        for y in content.clone() {
            let previous_y = y as i64 + i64::from(dy);
            if !(content.start as i64..content.end as i64).contains(&previous_y) {
                continue;
            }
            let at = (y * width + left) * 4;
            let before = (previous_y as usize * width + left) * 4;
            let bytes = (right - left) * 4;
            let current = &frame[at..at + bytes];
            let translated = &self.previous[before..before + bytes];
            let stationary = &self.previous[at..at + bytes];
            if !self.crosses_gutter(frame, width, y) {
                continue;
            }
            // A pinned panel can resume at the container boundary. Its rules
            // and text then move with the body again, but retain their original
            // column coordinates. Motion alone does not make them new content.
            if self.is_captured_column_row(frame, width, y, column_position) {
                first = None;
                rows = 0;
                continue;
            }
            let mut mismatch = 0;
            let mut changed = 0;
            for ((pixel, moved), still) in current
                .as_chunks::<4>()
                .0
                .iter()
                .zip(translated.as_chunks::<4>().0)
                .zip(stationary.as_chunks::<4>().0)
            {
                mismatch += usize::from(!color_matches(pixel, moved));
                changed += usize::from(!color_matches(pixel, still));
            }
            if mismatch > (right - left) / 100 || changed < gutter {
                continue;
            }
            if first.is_none() || y - last > MIN_BACKGROUND_ROWS {
                first = Some(y);
                rows = 0;
            }
            last = y;
            rows += 1;
            // A broad moving boundary is sufficient by itself; sparse glyphs
            // need evidence on several nearby rows to rule out pixel noise.
            if changed > (right - left) / 2 || rows >= 3 {
                let start =
                    self.section_prefix(frame, width, dy, &content, first?, column_position);
                if !captured_detail_end.is_some_and(|end| start < end)
                    && self.translated_suffix_matches(
                        frame,
                        width,
                        dy,
                        start..content.end,
                        &content,
                    )
                {
                    return Some(start);
                }
                first = None;
                rows = 0;
            }
        }
        None
    }

    fn crosses_gutter(&self, frame: &[u8], width: usize, y: usize) -> bool {
        let gutter = (width / 160).max(4);
        let left = self.left.saturating_sub(gutter);
        let at = (y * width + left) * 4;
        frame[at..at + gutter * 2 * 4]
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|pixel| !color_matches(*pixel, &self.gutter_color))
            .count()
            >= gutter
    }

    /// Include glyph ascenders, but never pull previously
    /// captured column text into a later document section when the panel moves.
    fn section_prefix(
        &self,
        frame: &[u8],
        width: usize,
        dy: i32,
        content: &Range<usize>,
        mut start: usize,
        column_position: i64,
    ) -> usize {
        let right = width - (width / 64).max(4);
        let left = self.left.saturating_sub((width / 160).max(4));
        let mut plain_rows = 0;
        let mut first_detail = start;
        while start > content.start {
            let y = start - 1;
            let foreground = frame[(y * width + self.left) * 4..(y * width + right) * 4]
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| !color_matches(pixel, &self.gutter_color));
            if foreground && self.is_captured_column_row(frame, width, y, column_position) {
                let source = (column_position + y as i64) as usize * self.sheet.width as usize * 4;
                let pixels = &frame[(y * width + self.left) * 4..(y * width + right) * 4];
                let known = &self.sheet.canvas[source..source + pixels.len()];
                // A sparse new ascender may fit the row's noise allowance.
                // Only an ink pixel also present in the column can stop it.
                if pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .zip(known.as_chunks::<4>().0)
                    .any(|(pixel, known)| {
                        !color_matches(pixel, &self.gutter_color) && color_matches(pixel, known)
                    })
                {
                    break;
                }
            }
            let previous_y = y as i64 + i64::from(dy);
            if (content.start as i64..content.end as i64).contains(&previous_y)
                && self.translated_row_matches(frame, width, (y, previous_y as usize), left..right)
                    == Some(false)
            {
                break;
            }
            start = y;
            if foreground {
                first_detail = start;
            }
            plain_rows = if foreground { 0 } else { plain_rows + 1 };
            // A plain margin ends the line. Backtracking through an entire
            // blank gap could steal newly exposed metadata above the section.
            if plain_rows >= MIN_BACKGROUND_ROWS {
                break;
            }
        }
        first_detail
    }

    fn is_captured_column_row(&self, frame: &[u8], width: usize, y: usize, position: i64) -> bool {
        let source_y = position + y as i64;
        if !(0..i64::from(self.sheet.height())).contains(&source_y) {
            return false;
        }
        let right = width - (width / 64).max(4);
        let bytes = (right - self.left) * 4;
        let at = (y * width + self.left) * 4;
        let source = source_y as usize * self.sheet.width as usize * 4;
        frame[at..at + bytes]
            .as_chunks::<4>()
            .0
            .iter()
            .zip(self.sheet.canvas[source..source + bytes].as_chunks::<4>().0)
            .filter(|(a, b)| !color_matches(*a, *b))
            .count()
            <= (right - self.left) / 100
    }

    /// Suffixes beginning below this exclusive bound have insufficient known
    /// column detail to rule out document flow. Background and horizontal
    /// rules are not identity evidence; text/icons have internal row edges.
    /// Compute a suffix bound rather than rescanning for every candidate row.
    fn captured_detail_end(
        &self,
        frame: &[u8],
        width: usize,
        rows: Range<usize>,
        position: i64,
    ) -> Option<usize> {
        let right = width - (width / 64).max(4);
        let mut matching = 0;
        for y in rows.rev() {
            if !self.is_captured_column_row(frame, width, y, position) {
                continue;
            }
            let at = (y * width + self.left) * 4;
            let pixels = frame[at..(y * width + right) * 4].as_chunks::<4>().0;
            let source = (position + y as i64) as usize * self.sheet.width as usize * 4;
            let known = self.sheet.canvas[source..source + pixels.len() * 4]
                .as_chunks::<4>()
                .0;
            // The row-level noise allowance may hide a tiny new glyph on a
            // wide blank row. Identity needs edges present in both images.
            let edges = pixels
                .windows(2)
                .zip(known.windows(2))
                .filter(|(pair, known)| {
                    !color_matches(&pair[0], &pair[1])
                        && color_matches(&pair[0], &known[0])
                        && color_matches(&pair[1], &known[1])
                })
                .take(MIN_DETAIL_EDGES)
                .count();
            if edges >= MIN_DETAIL_EDGES {
                matching += 1;
                if matching >= MIN_DETAIL_ROWS {
                    return Some(y + 1);
                }
            }
        }
        None
    }

    /// A matching prefix is insufficient when content below it belongs to a
    /// fixed layer. Check the whole proposed overlap and exclude viewport
    /// chrome in both observations before allowing the section to grow.
    fn translated_suffix_matches(
        &self,
        frame: &[u8],
        width: usize,
        dy: i32,
        proposed: Range<usize>,
        content: &Range<usize>,
    ) -> bool {
        let right = width - (width / 64).max(4);
        let mut rows = 0;
        for y in proposed {
            let previous_y = y as i64 + i64::from(dy);
            if !(content.start as i64..content.end as i64).contains(&previous_y) {
                continue;
            }
            match self.translated_row_matches(
                frame,
                width,
                (y, previous_y as usize),
                self.left..right,
            ) {
                Some(false) => return false,
                Some(true) => rows += 1,
                None => {}
            }
        }
        rows >= MIN_BACKGROUND_ROWS
    }

    /// Only exposed samples can attest document motion; padding under a rounded
    /// frame is neither a matching background nor a mismatching glyph.
    fn translated_row_matches(
        &self,
        frame: &[u8],
        width: usize,
        rows: (usize, usize),
        columns: Range<usize>,
    ) -> Option<bool> {
        let mut observed = 0;
        let mut mismatch = 0;
        for x in columns.clone() {
            let current = rows.0 * width + x;
            let previous = rows.1 * width + x;
            if self.frame_edge[current] || self.previous_frame_edge[previous] {
                continue;
            }
            observed += 1;
            mismatch += usize::from(!color_matches(
                &frame[current * 4..current * 4 + 4],
                &self.previous[previous * 4..previous * 4 + 4],
            ));
        }
        (observed * 2 >= columns.len()).then_some(mismatch <= observed / 100)
    }

    /// Keep the captured column once, fill its extra extent with page color,
    /// and give verified document sections priority over column padding.
    pub(super) fn composite(&self, canvas: &mut [u8], width: usize, bottom: usize) {
        if !self.independent {
            return;
        }
        let row = width * 4;
        let side_row = (width - self.left) * 4;
        // The parent owns one copy of confirmed bottom chrome at the output's
        // end. Column padding must not replace those last captured rows.
        let rows = (canvas.len() / row).saturating_sub(bottom);
        let top = self.origin - self.parent_origin;
        let height = i64::from(self.sheet.height());
        for (y, target) in canvas.chunks_exact_mut(row).take(rows).enumerate() {
            let source_y = y as i64 - top;
            let target = &mut target[self.left * 4..];
            if (0..height).contains(&source_y) {
                let source_y = source_y as usize;
                target.copy_from_slice(
                    &self.sheet.canvas[source_y * side_row..(source_y + 1) * side_row],
                );
            } else if let Some(background) = &self.background {
                target.copy_from_slice(background);
            } else {
                for pixel in target.as_chunks_mut::<4>().0 {
                    pixel.copy_from_slice(&self.gutter_color);
                }
            }
        }
        if let Some(flow) = &self.flow {
            let start = (flow.start - self.parent_origin).max(0) as usize;
            for (source, target) in flow
                .pixels
                .chunks_exact(side_row)
                .zip(canvas.chunks_exact_mut(row).take(rows).skip(start))
            {
                target[self.left * 4..].copy_from_slice(source);
            }
        }
    }
}

impl FlowTail {
    /// Retain observed pixels and repair only samples hidden by the frame.
    /// Padding under a window corner must never become an immutable observation
    /// that prevents a later clear frame from restoring the document text.
    fn extend(
        &mut self,
        frame: &[u8],
        width: usize,
        left: usize,
        position: i64,
        content: Range<usize>,
        frame_edge: &[bool],
    ) {
        let side_row = (width - left) * 4;
        let end = self.start + (self.pixels.len() / side_row) as i64;
        if end < position + content.start as i64 || position + content.end as i64 <= self.start {
            return;
        }
        let new_end = end.max(position + content.end as i64);
        self.pixels
            .resize((new_end - self.start) as usize * side_row, 0);
        self.observed.resize(self.pixels.len() / 4, false);
        for y in content.start.max((self.start - position).max(0) as usize)..content.end {
            let target_row = (position + y as i64 - self.start) as usize * (width - left);
            for x in left..width {
                let target = target_row + x - left;
                let source = y * width + x;
                if !self.observed[target] {
                    self.pixels[target * 4..target * 4 + 4]
                        .copy_from_slice(&frame[source * 4..source * 4 + 4]);
                    self.observed[target] = !frame_edge[source];
                }
            }
        }
    }
}

fn color_matches(a: &[u8], b: &[u8]) -> bool {
    a.iter()
        .zip(b)
        .all(|(a, b)| a.abs_diff(*b) <= COLOR_TOLERANCE)
}

fn crop(frame: &[u8], width: usize, left: usize) -> Vec<u8> {
    frame
        .chunks_exact(width * 4)
        .flat_map(|row| row[left * 4..].iter().copied())
        .collect()
}

/// A uniform vertical gutter separates substantial content on its right from
/// the main document. The rightmost qualifying gutter avoids splitting within
/// the wide blank interior of a bordered article. Outer scrollbars cannot vote.
fn separator(frame: &[u8], width: usize, height: usize) -> Option<usize> {
    if width < 160 || height < 160 {
        return None;
    }
    let from = height / 4;
    let to = height - height / 10;
    let edge = width - (width / 64).max(4);
    let mut run_start = 0;
    let mut run_top = from;
    let mut run_color = [0; 4];
    let mut best = None;
    for x in width / 2..width * 7 / 8 {
        let p = ((to - 1) * width + x) * 4;
        let color: [u8; 4] = frame[p..p + 4].try_into().ok()?;
        // A page heading can cross the gutter above the two-column content.
        // Require a sustained gutter below it, not a full-height blank column.
        let mut top = to;
        while top > from {
            let i = ((top - 1) * width + x) * 4;
            if frame[i..i + 4]
                .iter()
                .zip(color)
                .any(|(a, b)| a.abs_diff(b) > COLOR_TOLERANCE)
            {
                break;
            }
            top -= 1;
        }
        let uniform = to - top >= height / 4;
        if !uniform || color != run_color {
            if x.saturating_sub(run_start) >= (width / 160).max(4) && run_start != 0 {
                let left = run_start;
                if structured(frame, width, run_top, to, x, edge) {
                    best = Some(left);
                }
            }
            run_start = if uniform { x } else { 0 };
            run_top = top;
            run_color = color;
        } else {
            run_top = run_top.max(top);
        }
    }
    best
}

fn structured(
    frame: &[u8],
    width: usize,
    from: usize,
    to: usize,
    left: usize,
    right: usize,
) -> bool {
    let mut edges = 0;
    let mut rows = 0;
    let mut first = to;
    let mut last = from;
    for y in from..to - 1 {
        let mut count = 0;
        for x in left..right {
            let i = (y * width + x) * 4;
            let j = i + width * 4;
            if frame[i..i + 3]
                .iter()
                .zip(&frame[j..j + 3])
                .any(|(a, b)| a.abs_diff(*b) > 12)
            {
                count += 1;
            }
        }
        edges += count;
        if count >= 4 {
            rows += 1;
            first = first.min(y);
            last = y;
        }
    }
    // Whitespace above metadata says nothing about its information content.
    // Evaluate density only across its visible structure, with enough vertical
    // extent to exclude a few stray pixels or a single horizontal rule.
    let span = last.saturating_sub(first) + 1;
    span >= 24 && rows >= (span / 8).max(8) && edges >= span * 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_tiny_glyphs_on_plain_rows_do_not_count_as_known_column_detail() {
        let width = 1_280;
        let height = 640;
        let mut seed = vec![255; width * height * 4];
        for y in 180..500 {
            for x in 960..1_100 {
                if y % 31 < 9 && x % 13 < 8 {
                    let at = (y * width + x) * 4;
                    seed[at..at + 3].fill((y * 37 % 151) as u8);
                }
            }
        }
        let sidebar =
            Sidebar::new(width as u32, height as u32, &seed, SheetLimits::default()).unwrap();
        assert!(
            sidebar
                .captured_detail_end(&seed, width, 180..500, 0)
                .is_some()
        );
        let mut next = seed;
        for y in 600..604 {
            for x in [980, 984] {
                let at = (y * width + x) * 4;
                next[at..at + 3].fill(80);
            }
        }
        assert!(sidebar.is_captured_column_row(&next, width, 600, 0));
        assert!(
            sidebar
                .captured_detail_end(&next, width, 600..604, 0)
                .is_none()
        );
    }

    #[test]
    fn a_moving_banner_above_a_fixed_panel_does_not_reclassify_the_whole_column() {
        let width = 480;
        let height = 640;
        let frame = |offset: usize, panel_moves: bool| {
            let mut pixels = vec![255; width * height * 4];
            for y in 300..560 {
                for x in 352..444 {
                    if (y % 31 < 9) && (x % 13 < 8) {
                        let row = if panel_moves { y - offset } else { y };
                        let i = (row * width + x) * 4;
                        pixels[i..i + 3].fill((y * 37 % 151) as u8);
                    }
                }
            }
            for y in 180 - offset..204 - offset {
                for x in 40..width - 8 {
                    let i = (y * width + x) * 4;
                    pixels[i..i + 3].fill(64);
                }
            }
            pixels
        };
        let previous = frame(0, false);
        let mut sidebar = Sidebar::new(
            width as u32,
            height as u32,
            &previous,
            SheetLimits::default(),
        )
        .unwrap();
        sidebar.independent = true;
        assert!(
            sidebar
                .flow_boundary(&frame(8, false), width, 8, 0..height, 0)
                .is_none(),
            "a moving row cannot confer document motion on a fixed panel below it"
        );
        assert!(
            sidebar
                .flow_boundary(&frame(8, true), width, 8, 0..height, 8)
                .is_none(),
            "previously captured metadata stays in its own column when it moves again"
        );
    }
}
