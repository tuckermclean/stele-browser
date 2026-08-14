//! The tty backend (P7): a deterministic character-grid renderer over the
//! frozen `layout::Fragment` vector. No display/terminal is touched here —
//! this module only builds a [`TextGrid`] and turns it into a `String`; the
//! interactive raw-mode tty (scrolling, following links) is a later, separate
//! packet. This is the backbone `--headless --dump-text` renders through.
//!
//! ## Cell mapping (pins the golden — read before touching the math)
//!
//! Fragments live in continuous layout-pixel space; the grid is discrete
//! 8x16 cells, matching `text::BitmapFont::vga_8x16` (the same font P6's
//! inline engine measures text with, so a monospace document's fragment
//! coordinates already fall near cell boundaries).
//!
//! - `col = round(rect.origin.x / CELL_W)`, `row = round(rect.origin.y / CELL_H)`.
//! - **Top, not baseline**: a `Text` fragment's `rect.origin` is the
//!   top-left of its *line box* (see `layout::block::emit`), not the glyph
//!   baseline — `FragmentKind::Text::baseline` is a pixel offset *within*
//!   that rect, used by pixel backends to sit glyphs on the baseline. The
//!   tty backend has no glyph rasterizer, and every run in a line box shares
//!   one `rect.origin.y` but could in principle carry different baselines
//!   (different font sizes on one line); anchoring text-mode rows to the
//!   shared line-box top keeps every run in a line landing on the exact same
//!   grid row, which anchoring to (possibly-differing) baselines would not
//!   guarantee. So: top, always.
//! - Rounding (not floor/ceil) picks the *nearest* cell, so a fragment placed
//!   a fraction of a pixel off a cell boundary (accumulated float error) still
//!   lands where a human eyeballing the layout would expect.
//!
//! ## What paints, what doesn't
//!
//! - `Text` fragments write their string's chars left-to-right starting at
//!   the mapped cell, one column per `char` (not byte — multi-byte UTF-8
//!   is placed by Unicode scalar, matching the monospace font's own
//!   per-`char` advance model), clipped at the grid's right edge.
//! - `Image` fragments (a decoded image placed by the layout engine) render a
//!   compact `[img]` placeholder marker at their mapped cell, same clipping
//!   rule. (Scope note: today's block-flow pipeline (`layout::block::emit`)
//!   paints a `Replaced` box as a plain `FragmentKind::Box`, not `Image` — no
//!   pixel data exists on the frozen `Replaced` node yet, that's P9's fb
//!   backend wiring. `Image` handling here is still real and exercised by
//!   this module's own synthetic-fragment tests, ready for whenever a
//!   pipeline actually emits one.)
//! - `Box` fragments (a block box's background/border) render **nothing** in
//!   text mode — v0 scope call, documented for the DECISIONS ledger: a text
//!   grid has no color/background concept, and even simple ASCII-art borders
//!   would need their own cell-accurate box-drawing pass distinct from the
//!   text-placement rule above; deferred rather than half-done.
//! - **Paint order wins ties**: fragments are drawn in slice order (already
//!   paint order per `layout::layout`'s contract), each write overwriting
//!   whatever a prior fragment left in the same cell.
//! - Grid height is derived from the max fragment bottom edge (`origin.y +
//!   size.h`) across ALL fragments (not just `Text`), so the full document
//!   dumps rather than clipping to one screen — `render`'s `cols` only
//!   bounds width; height is always content-driven.
//!
//! ## Totality
//!
//! `render` never panics: non-finite/negative coordinates are clamped to
//! cell `0`; `cols == 0` yields an empty grid; a pathologically huge
//! fragment bottom (e.g. from a hostile huge-margin declaration reaching all
//! the way through layout) is capped at [`MAX_GRID_ROWS`]. `cols` itself —
//! directly caller-controlled (`--cols` on the CLI, reachable on ANY
//! document, not just a hostile one) — is independently clamped to
//! [`MAX_GRID_COLS`] before anything allocates; a content-free layout
//! (`rows_needed == 0`) also short-circuits before ever sizing a row. Every
//! axis of the grid allocation is bounded — the 486 target has little memory
//! to spare for an attacker- or user-inflated grid.

use crate::layout::{Fragment, FragmentKind};

/// `pub(crate)`, not just private: the frames packet (`crate::frames`)
/// composites multiple independently-rendered `TextGrid`s into one viewport
/// and needs the exact same cell-mapping constants this module's own
/// docs pin — duplicating the magic numbers would risk the two modules
/// silently drifting. Still not part of the public API surface.
pub(crate) const CELL_W: f32 = 8.0;
pub(crate) const CELL_H: f32 = 16.0;

/// Hard cap on grid rows, independent of document content. Far beyond any
/// realistic document (10,000 lines is a ~160,000px-tall page) but bounded,
/// so a hostile/degenerate layout (huge margins, huge coordinates) can't
/// drive an unbounded allocation. See module docs.
pub(crate) const MAX_GRID_ROWS: usize = 10_000;

/// Hard cap on grid columns, independent of the caller-supplied `cols`.
/// `cols` is directly attacker/user-controlled (it's `--cols` on the CLI,
/// reachable on ANY document, not just a hostile one) — unlike
/// [`MAX_GRID_ROWS`], which only bounds a value *derived* from layout, this
/// axis had NO bound at all (C1: a `--cols 999999999` alone drove a
/// multi-gigabyte allocation, capacity-overflow panic or OOM abort under
/// `panic=abort`, regardless of document content). 2,000 columns is already
/// far past any real terminal (even "ultra-wide" virtual ttys top out
/// nowhere near this); combined with `MAX_GRID_ROWS`, the worst-case grid is
/// `2_000 * 10_000 * size_of::<char>()` = 80MB — bounded, not necessarily
/// cheap, but a `--cols` flag alone can no longer drive an unbounded
/// allocation.
pub(crate) const MAX_GRID_COLS: usize = 2_000;

/// A rendered character grid: rows of columns, ready to print.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextGrid {
    rows: Vec<Vec<char>>,
}

impl TextGrid {
    /// A blank (all-space) grid of the given size — the canvas the frames
    /// packet (`crate::frames`) builds before [`blit`](Self::blit)-ing each
    /// frame's own independently-rendered grid into place. `cols`/`rows` are
    /// clamped to [`MAX_GRID_COLS`]/[`MAX_GRID_ROWS`] (same totality
    /// rationale as `render`: a frameset's computed grid dimensions are
    /// ultimately attacker/document-controlled, same as `--cols`); either
    /// dimension being `0` yields an empty grid, matching `render`'s own
    /// `cols == 0` / `rows_needed == 0` short-circuits.
    pub fn blank(cols: usize, rows: usize) -> Self {
        if cols == 0 || rows == 0 {
            return TextGrid { rows: Vec::new() };
        }
        let cols = cols.min(MAX_GRID_COLS);
        let rows = rows.min(MAX_GRID_ROWS);
        TextGrid { rows: vec![vec![' '; cols]; rows] }
    }

    /// This grid's row count (its rendered/content-driven height in cells).
    pub fn rows_len(&self) -> usize {
        self.rows.len()
    }

    /// Blit `other` into `self` at cell offset `(col_off, row_off)`, clipped
    /// to `self`'s own bounds in both axes — cells of `other` that would
    /// land outside `self` are silently dropped rather than panicking (an
    /// out-of-range offset, or `other` wider/taller than the remaining
    /// space, are both ordinary inputs from the frames packet's grid math,
    /// not something exceptional). Later writes at the same cell (e.g. two
    /// overlapping `blit` calls) win over earlier ones, mirroring `render`'s
    /// own "paint order wins ties" rule.
    pub fn blit(&mut self, other: &TextGrid, col_off: usize, row_off: usize) {
        for (r, src_row) in other.rows.iter().enumerate() {
            let dst_r = row_off + r;
            let Some(dst_row) = self.rows.get_mut(dst_r) else { break };
            for (c, &ch) in src_row.iter().enumerate() {
                let dst_c = col_off + c;
                let Some(cell) = dst_row.get_mut(dst_c) else { break };
                *cell = ch;
            }
        }
    }

    /// Join rows with `\n`, trimming trailing spaces from each row and
    /// dropping any trailing (bottom) blank rows — deterministic output with
    /// no dangling whitespace, suitable for an exact-match golden. Interior
    /// blank rows (part of the document's vertical rhythm) are preserved.
    pub fn to_text(&self) -> String {
        let mut lines: Vec<String> = self
            .rows
            .iter()
            .map(|row| {
                let s: String = row.iter().collect();
                s.trim_end_matches(' ').to_string()
            })
            .collect();
        while matches!(lines.last(), Some(l) if l.is_empty()) {
            lines.pop();
        }
        lines.join("\n")
    }

    #[cfg(test)]
    fn row_text(&self, row: usize) -> String {
        self.rows.get(row).map(|r| r.iter().collect()).unwrap_or_default()
    }
}

/// Render paint-ordered `fragments` (as `layout::layout` produces them) into
/// a `cols`-wide [`TextGrid`]. See module docs for the cell-mapping rule,
/// what each `FragmentKind` paints, and the totality guarantees.
pub fn render(fragments: &[Fragment], cols: usize) -> TextGrid {
    if cols == 0 {
        return TextGrid { rows: Vec::new() };
    }
    // Clamp FIRST, before anything else touches `cols` — this is the only
    // choke point every caller (CLI, tests, any future backend user) goes
    // through, so it's the one place that has to hold the line. See
    // MAX_GRID_COLS's doc comment (C1).
    let cols = cols.min(MAX_GRID_COLS);

    let mut rows_needed = 0usize;
    for f in fragments {
        let top_row = cell_index(f.rect.origin.y, CELL_H);
        let bottom_row = cell_index(f.rect.origin.y + nonneg(f.rect.size.h), CELL_H);
        rows_needed = rows_needed.max(top_row + 1).max(bottom_row);
    }
    rows_needed = rows_needed.min(MAX_GRID_ROWS);

    if rows_needed == 0 {
        // Nothing to draw: return early rather than materializing even one
        // `vec![' '; cols]` row we'd immediately throw away — `vec![elem;
        // n]` evaluates `elem` once regardless of `n`, so without this guard
        // a content-free document still paid for a `cols`-wide allocation.
        return TextGrid { rows: Vec::new() };
    }

    let mut rows: Vec<Vec<char>> = vec![vec![' '; cols]; rows_needed];

    for f in fragments {
        match &f.kind {
            FragmentKind::Text { text, .. } => write_marker(&mut rows, f, text, cols),
            FragmentKind::Image { .. } => write_marker(&mut rows, f, "[img]", cols),
            FragmentKind::Box { .. } => {
                // v0 scope call: no background/border rendering in text
                // mode. See module docs.
            }
        }
    }

    TextGrid { rows }
}

/// Write `text`'s characters left-to-right into `rows`, starting at the cell
/// `fragment.rect.origin` maps to, clipped to the grid's bounds. Shared by
/// the `Text` and `Image`-placeholder paint paths.
///
/// KNOWN LIMITATION (documented, not fixed — I2 in the P7 review, logged to
/// DECISIONS by the orchestrator): this advances exactly one grid column per
/// `char`, regardless of the fragment's own font size. `text::BitmapFont::
/// advance` scales with `size_px` (e.g. an h1 at 32px is 16 real pixels —
/// two cells — per character; an h2 at 24px is 12px, one and a half cells),
/// but the fixed 8px tty cell can only place a run's *start* at a size-aware
/// `origin.x` cell and then walks it forward one column at a time. Two
/// `Text` fragments of *different* font sizes sharing one line box (e.g.
/// `<h1>Big <small>text</small></h1>`) would therefore drift out of
/// alignment (gap or overlap) after the first run — each run's start cell is
/// correct, but its *own* per-char advance isn't scaled to match its font
/// size. Invisible in `fixtures/basic.html` (every heading is one uniform
/// run alone on its line) and in every current M2 fixture, so left
/// undisturbed: correct tty handling of mixed inline font sizes on one line
/// is a real design question (partial cells? proportional skipping? render
/// only the dominant run's size?) for a later packet, not a quick fix here.
fn write_marker(rows: &mut [Vec<char>], fragment: &Fragment, text: &str, cols: usize) {
    let row = cell_index(fragment.rect.origin.y, CELL_H);
    if row >= rows.len() {
        return;
    }
    let mut col = cell_index(fragment.rect.origin.x, CELL_W);
    for ch in text.chars() {
        if col >= cols {
            break;
        }
        rows[row][col] = ch;
        col += 1;
    }
}

/// Map a layout-pixel coordinate to a clamped, non-negative cell index.
/// Non-finite or negative inputs clamp to `0`; absurdly large inputs clamp
/// to a large-but-safe index (never overflows the `f32 -> usize` cast, never
/// panics) — see module docs on totality.
pub(crate) fn cell_index(v: f32, cell: f32) -> usize {
    if !v.is_finite() || cell <= 0.0 {
        return 0;
    }
    let c = (v / cell).round();
    if c <= 0.0 {
        0
    } else if c >= MAX_GRID_ROWS as f32 * 4.0 {
        // Comfortably above MAX_GRID_ROWS (itself the real backstop below)
        // so this branch only exists to keep the `as usize` cast in range
        // for non-finite-adjacent extreme values (e.g. f32::MAX).
        MAX_GRID_ROWS * 4
    } else {
        c as usize
    }
}

fn nonneg(v: f32) -> f32 {
    if v.is_finite() && v > 0.0 {
        v
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::img::RgbaImage;
    use crate::layout::{Point, Rect, Size};
    use crate::style::ComputedStyle;

    fn text_fragment(x: f32, y: f32, w: f32, h: f32, text: &str) -> Fragment {
        Fragment {
            rect: Rect { origin: Point { x, y }, size: Size { w, h } },
            kind: FragmentKind::Text { text: text.to_string(), baseline: h * 0.75, style: ComputedStyle::default() },
            interactive: None,
        }
    }

    fn box_fragment(x: f32, y: f32, w: f32, h: f32) -> Fragment {
        Fragment {
            rect: Rect { origin: Point { x, y }, size: Size { w, h } },
            kind: FragmentKind::Box { style: ComputedStyle::default() },
            interactive: None,
        }
    }

    fn image_fragment(x: f32, y: f32) -> Fragment {
        Fragment {
            rect: Rect { origin: Point { x, y }, size: Size { w: 32.0, h: 32.0 } },
            kind: FragmentKind::Image { image: RgbaImage::new(1, 1) },
            interactive: None,
        }
    }

    #[test]
    fn single_text_run_lands_at_the_expected_cell() {
        let fragments = vec![text_fragment(8.0, 16.0, 16.0, 16.0, "hi")];
        let grid = render(&fragments, 20);
        assert_eq!(grid.row_text(1).chars().nth(1), Some('h'));
        assert_eq!(grid.row_text(1).chars().nth(2), Some('i'));
    }

    #[test]
    fn two_runs_on_different_lines_land_on_different_rows() {
        let fragments = vec![
            text_fragment(0.0, 0.0, 40.0, 16.0, "top"),
            text_fragment(0.0, 16.0, 40.0, 16.0, "bottom"),
        ];
        let grid = render(&fragments, 40);
        assert!(grid.row_text(0).starts_with("top"));
        assert!(grid.row_text(1).starts_with("bottom"));
    }

    #[test]
    fn overlapping_runs_the_later_fragment_wins() {
        let fragments = vec![text_fragment(0.0, 0.0, 40.0, 16.0, "first"), text_fragment(0.0, 0.0, 40.0, 16.0, "second")];
        let grid = render(&fragments, 40);
        assert!(grid.row_text(0).starts_with("second"));
    }

    #[test]
    fn text_clips_at_the_right_edge_instead_of_wrapping_or_panicking() {
        let fragments = vec![text_fragment(0.0, 0.0, 400.0, 16.0, "abcdefghij")];
        let grid = render(&fragments, 5);
        assert_eq!(grid.row_text(0), "abcde");
    }

    #[test]
    fn to_text_trims_trailing_spaces_per_row_and_trailing_blank_rows() {
        let fragments = vec![text_fragment(0.0, 0.0, 16.0, 16.0, "hi"), text_fragment(0.0, 32.0, 16.0, 16.0, "yo")];
        // Row 1 (y=16) is untouched (blank interior row); rows 0 and 2 have
        // text. The grid should be exactly 3 rows tall (row 2's bottom is
        // 48px -> row index 2), and the printed text must have no trailing
        // spaces on any row and no trailing blank line after the last row
        // with content.
        let grid = render(&fragments, 10);
        let text = grid.to_text();
        let lines: Vec<&str> = text.split('\n').collect();
        assert_eq!(lines.len(), 3, "expected 3 lines, got: {lines:?}");
        assert_eq!(lines[0], "hi");
        assert_eq!(lines[1], "");
        assert_eq!(lines[2], "yo");
        assert!(!text.ends_with('\n'));
        for line in &lines {
            assert_eq!(line, &line.trim_end());
        }
    }

    #[test]
    fn image_fragment_renders_a_compact_placeholder_marker() {
        let fragments = vec![image_fragment(0.0, 0.0)];
        let grid = render(&fragments, 20);
        assert_eq!(grid.row_text(0).trim_end(), "[img]");
    }

    #[test]
    fn image_placeholder_clips_at_the_right_edge() {
        let fragments = vec![image_fragment(16.0, 0.0)];
        let grid = render(&fragments, 4); // starts at col 2, room for only "[i"
        assert_eq!(grid.row_text(0), "  [i");
    }

    #[test]
    fn box_fragments_paint_nothing_in_text_mode() {
        let fragments = vec![box_fragment(0.0, 0.0, 80.0, 16.0), text_fragment(0.0, 0.0, 16.0, 16.0, "x")];
        let grid = render(&fragments, 10);
        assert_eq!(grid.row_text(0).trim_end(), "x");
    }

    #[test]
    fn zero_cols_yields_an_empty_grid_and_empty_text() {
        let fragments = vec![text_fragment(0.0, 0.0, 16.0, 16.0, "x")];
        let grid = render(&fragments, 0);
        assert_eq!(grid.to_text(), "");
    }

    #[test]
    fn empty_fragment_list_yields_empty_text() {
        let grid = render(&[], 80);
        assert_eq!(grid.to_text(), "");
    }

    /// C1 regression (reviewer-caught Critical): `cols` is caller-controlled
    /// (directly from `--cols` on the CLI, reachable on ANY document) and
    /// was never bounded — only rows had a `MAX_GRID_ROWS` clamp. Because
    /// `vec![elem; n]`'s `elem` is evaluated once regardless of `n`, even a
    /// content-free document (`rows_needed == 0`) still drove the huge
    /// `vec![' '; cols]` allocation. Must clamp `cols` (and skip the alloc
    /// entirely when there's nothing to draw) so this never panics/aborts.
    #[test]
    fn absurdly_large_cols_is_clamped_not_a_panic() {
        let fragments = vec![text_fragment(0.0, 0.0, 16.0, 16.0, "hi")];
        let grid = render(&fragments, 999_999_999);
        // Must not panic (capacity overflow / OOM abort) and must actually
        // clamp to a bounded width, not merely "not crash while still being
        // huge" — assert the printed line stays within a sane bound.
        let text = grid.to_text();
        for line in text.lines() {
            assert!(line.chars().count() <= MAX_GRID_COLS, "line width exceeds MAX_GRID_COLS: {}", line.chars().count());
        }
        assert!(text.starts_with("hi"));
    }

    #[test]
    fn absurdly_large_cols_with_no_content_is_also_clamped_not_a_panic() {
        // Same hostile `cols`, but with NO fragments at all (`rows_needed ==
        // 0`): the pre-fix bug fired here too, since `vec![elem; n]`
        // evaluates `elem` before `n` is even consulted.
        let grid = render(&[], 999_999_999);
        assert_eq!(grid.to_text(), "");
    }

    #[test]
    fn degenerate_rects_never_panic() {
        let degenerate = [
            (f32::NAN, f32::NAN),
            (f32::INFINITY, f32::INFINITY),
            (f32::NEG_INFINITY, f32::NEG_INFINITY),
            (-1.0, -1.0),
            (f32::MAX, f32::MAX),
        ];
        for (x, y) in degenerate {
            let fragments = vec![
                text_fragment(x, y, f32::NAN, f32::NAN, "z"),
                box_fragment(x, y, f32::INFINITY, -1.0),
                image_fragment(x, y),
            ];
            let grid = render(&fragments, 40);
            let _ = grid.to_text(); // must not panic
        }
    }

    #[test]
    fn multi_byte_utf8_is_placed_by_char_not_byte() {
        let fragments = vec![text_fragment(0.0, 0.0, 80.0, 16.0, "é日x")];
        let grid = render(&fragments, 10);
        let row = grid.row_text(0);
        assert_eq!(row.chars().nth(0), Some('é'));
        assert_eq!(row.chars().nth(1), Some('日'));
        assert_eq!(row.chars().nth(2), Some('x'));
    }

    #[test]
    fn grid_height_covers_the_full_document_not_just_one_screen() {
        let fragments = vec![text_fragment(0.0, 0.0, 16.0, 16.0, "a"), text_fragment(0.0, 320.0, 16.0, 16.0, "b")];
        let grid = render(&fragments, 10);
        assert_eq!(grid.rows.len(), 21); // row 20 (320/16) + 1
        assert!(grid.row_text(20).starts_with('b'));
    }

    // ------------------------------- blank / blit (frames compositing) -----

    #[test]
    fn blank_grid_is_all_spaces_of_the_requested_size() {
        let grid = TextGrid::blank(5, 3);
        assert_eq!(grid.rows_len(), 3);
        for r in 0..3 {
            assert_eq!(grid.row_text(r), "     ");
        }
    }

    #[test]
    fn blank_grid_with_a_zero_dimension_is_empty() {
        assert_eq!(TextGrid::blank(0, 3).rows_len(), 0);
        assert_eq!(TextGrid::blank(5, 0).rows_len(), 0);
    }

    #[test]
    fn blank_grid_dimensions_are_clamped_not_a_panic() {
        let grid = TextGrid::blank(999_999_999, 999_999_999);
        assert!(grid.rows_len() <= MAX_GRID_ROWS);
        for r in 0..grid.rows_len().min(2) {
            assert!(grid.row_text(r).chars().count() <= MAX_GRID_COLS);
        }
    }

    #[test]
    fn blit_places_a_grid_at_the_given_cell_offset() {
        let fragments = vec![text_fragment(0.0, 0.0, 16.0, 16.0, "hi")];
        let small = render(&fragments, 4);
        let mut canvas = TextGrid::blank(10, 5);
        canvas.blit(&small, 3, 2);
        assert_eq!(canvas.row_text(2).chars().nth(3), Some('h'));
        assert_eq!(canvas.row_text(2).chars().nth(4), Some('i'));
        // Untouched cells stay blank.
        assert_eq!(canvas.row_text(0), "          ");
        assert_eq!(canvas.row_text(2).chars().nth(0), Some(' '));
    }

    #[test]
    fn blit_clips_at_the_canvas_bounds_instead_of_panicking() {
        let fragments = vec![text_fragment(0.0, 0.0, 80.0, 16.0, "abcdef")];
        let wide = render(&fragments, 6);
        let mut canvas = TextGrid::blank(4, 2);
        // Offset already past the canvas width/height, and content wider
        // than what remains either way: must clip silently, not panic.
        canvas.blit(&wide, 2, 1);
        assert_eq!(canvas.row_text(1), "  ab");
    }

    #[test]
    fn blit_with_offset_entirely_outside_the_canvas_is_a_silent_no_op() {
        let fragments = vec![text_fragment(0.0, 0.0, 16.0, 16.0, "x")];
        let small = render(&fragments, 4);
        let mut canvas = TextGrid::blank(3, 3);
        canvas.blit(&small, 100, 100);
        assert_eq!(canvas.to_text(), "");
    }

    #[test]
    fn later_blit_wins_over_an_earlier_overlapping_one() {
        let first = render(&[text_fragment(0.0, 0.0, 16.0, 16.0, "AA")], 4);
        let second = render(&[text_fragment(0.0, 0.0, 16.0, 16.0, "BB")], 4);
        let mut canvas = TextGrid::blank(4, 1);
        canvas.blit(&first, 0, 0);
        canvas.blit(&second, 0, 0);
        assert_eq!(canvas.row_text(0), "BB  ");
    }
}
