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
//! - `Box` fragments (a block box's background/border) render their
//!   `background_color` as cell `bg` (see "Color" below) but still render
//!   **no border** in text mode — v0 scope call, documented for the
//!   DECISIONS ledger: even simple ASCII-art borders would need their own
//!   cell-accurate box-drawing pass distinct from the text-placement rule
//!   above; deferred rather than half-done.
//! - **Paint order wins ties**: fragments are drawn in slice order (already
//!   paint order per `layout::layout`'s contract), each write overwriting
//!   whatever a prior fragment left in the same cell.
//! - Grid height is derived from the max fragment bottom edge (`origin.y +
//!   size.h`) across ALL fragments (not just `Text`), so the full document
//!   dumps rather than clipping to one screen — `render`'s `cols` only
//!   bounds width; height is always content-driven.
//!
//! ## Color (packet/tty-color)
//!
//! Each cell now carries a foreground and background [`Color`] (reused from
//! `surface`, not a new type) alongside its `char`:
//!
//! - `Box { style }` fills the cells its rect covers with
//!   `style.background_color` as the cell `bg` — but only when that color
//!   isn't fully transparent (`a == 0`); a transparent box leaves whatever
//!   `bg` was already there untouched, same as painting nothing. Borders
//!   still render **nothing** in text mode (unchanged v0 scope call — ASCII-
//!   art box-drawing is a distinct, still-deferred pass).
//! - `Text { text, style, .. }` sets each written cell's `ch` *and* `fg`
//!   (from `style.color`) but never touches `bg`. Because paint order already
//!   draws `Box` fragments before the `Text` fragments they contain (brief's
//!   paint-order contract), a text cell's `bg` is simply whatever the
//!   enclosing box already painted (or the grid's default transparent, if no
//!   box covered that cell) — text never needs to know or re-derive its own
//!   background.
//! - `Image` placeholders (`[img]`) set `ch` only, `fg` stays the grid
//!   default (see below) — `FragmentKind::Image` carries no `ComputedStyle`
//!   to color it with.
//! - The grid's default cell is `ch: ' '`, `fg: Color::BLACK` (the UA's own
//!   initial `color`), `bg: Color::TRANSPARENT` (the UA's own initial
//!   `background-color`) — matching `ComputedStyle::default()` exactly, so
//!   an uncolored document renders identically to the pre-color grid.
//! - [`TextGrid::to_text`] is UNCHANGED: it reads only `ch` per cell, so
//!   color is invisible to it and every existing golden stays byte-identical.
//! - [`TextGrid::to_ansi`] is new: a 24-bit-color ANSI rendering of the same
//!   grid, for the interactive shell packet to draw with. See its own doc.
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
use crate::surface::Color;

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

/// One grid cell: a `char` plus the foreground/background it paints with.
/// `pub(crate)`, not part of `TextGrid`'s public surface — callers reach
/// color only through [`TextGrid::to_ansi`]; `Cell` itself is an internal
/// storage detail, same posture as [`CELL_W`]/[`CELL_H`].
///
/// Default matches `ComputedStyle::default()` (`color: Color::BLACK`,
/// `background_color: Color::TRANSPARENT`) exactly, so a grid nobody paints
/// color into renders identically (via `to_ansi`) to plain black-on-default
/// text — color is strictly additive over the pre-color grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
}

impl Default for Cell {
    fn default() -> Self {
        Cell { ch: ' ', fg: Color::BLACK, bg: Color::TRANSPARENT }
    }
}

/// A rendered character grid: rows of columns, ready to print.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextGrid {
    rows: Vec<Vec<Cell>>,
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
        TextGrid { rows: vec![vec![Cell::default(); cols]; rows] }
    }

    /// This grid's row count (its rendered/content-driven height in cells).
    pub fn rows_len(&self) -> usize {
        self.rows.len()
    }

    /// This grid's column count (`0` for an empty grid). `pub(crate)` — the
    /// interactive shell (`crate::browser`) needs it to size its viewport
    /// window at exactly the page grid's own width; not part of the public
    /// API surface (mirrors [`Cell`]'s own visibility posture).
    pub(crate) fn cols(&self) -> usize {
        self.rows.first().map(|r| r.len()).unwrap_or(0)
    }

    /// The cell at `(row, col)`, or [`Cell::default`] when out of bounds —
    /// total, never panics, matching every other accessor in this module.
    /// `pub(crate)`: the interactive shell reads cells to build its own
    /// focus-highlight overlay (see [`Self::window`]/[`Self::set`]).
    pub(crate) fn get(&self, row: usize, col: usize) -> Cell {
        self.rows.get(row).and_then(|r| r.get(col)).copied().unwrap_or_default()
    }

    /// Overwrite the cell at `(row, col)`, a silent no-op when out of bounds
    /// (same totality posture as [`Self::blit`]). `pub(crate)`: the
    /// interactive shell's focus-highlight overlay writes through this
    /// rather than reaching into `rows` directly, keeping `Cell`/`rows`
    /// themselves private storage details of this module.
    pub(crate) fn set(&mut self, row: usize, col: usize, cell: Cell) {
        if let Some(c) = self.rows.get_mut(row).and_then(|r| r.get_mut(col)) {
            *c = cell;
        }
    }

    /// Crop a `row_count`-tall window starting at `row_start`, same width as
    /// `self`, padding with blank (`Cell::default`) rows when `self` is
    /// shorter than the requested window (e.g. a short document scrolled to
    /// its very bottom, or a totally empty grid) — never panics, unlike
    /// naive slicing. This is the interactive shell's scroll-viewport
    /// primitive (`crate::browser::frame::render_frame`): a `Page`'s grid
    /// holds the WHOLE rendered document, and each drawn frame needs just
    /// the `rows`-tall slice starting at the current `scroll_row`.
    pub(crate) fn window(&self, row_start: usize, row_count: usize) -> TextGrid {
        let cols = self.cols();
        let mut out = TextGrid::blank(cols, row_count);
        for r in 0..row_count {
            let Some(src_row) = self.rows.get(row_start + r) else { break };
            if let Some(dst_row) = out.rows.get_mut(r) {
                for (c, cell) in src_row.iter().enumerate() {
                    if let Some(dst) = dst_row.get_mut(c) {
                        *dst = *cell;
                    }
                }
            }
        }
        out
    }

    /// Blit `other` into `self` at cell offset `(col_off, row_off)`, clipped
    /// to `self`'s own bounds in both axes — cells of `other` that would
    /// land outside `self` are silently dropped rather than panicking (an
    /// out-of-range offset, or `other` wider/taller than the remaining
    /// space, are both ordinary inputs from the frames packet's grid math,
    /// not something exceptional). Later writes at the same cell (e.g. two
    /// overlapping `blit` calls) win over earlier ones, mirroring `render`'s
    /// own "paint order wins ties" rule. Copies whole cells — `char` plus
    /// `fg`/`bg` travel together, so a composited frameset keeps each
    /// child frame's own colors.
    pub fn blit(&mut self, other: &TextGrid, col_off: usize, row_off: usize) {
        for (r, src_row) in other.rows.iter().enumerate() {
            let dst_r = row_off + r;
            let Some(dst_row) = self.rows.get_mut(dst_r) else { break };
            for (c, &src_cell) in src_row.iter().enumerate() {
                let dst_c = col_off + c;
                let Some(cell) = dst_row.get_mut(dst_c) else { break };
                *cell = src_cell;
            }
        }
    }

    /// Join rows with `\n`, trimming trailing spaces from each row and
    /// dropping any trailing (bottom) blank rows — deterministic output with
    /// no dangling whitespace, suitable for an exact-match golden. Interior
    /// blank rows (part of the document's vertical rhythm) are preserved.
    ///
    /// Reads `ch` only — color is invisible here by construction, so this
    /// is byte-identical to the pre-color implementation for any grid,
    /// colored or not (see `to_text_is_blind_to_cell_color` below).
    pub fn to_text(&self) -> String {
        let mut lines: Vec<String> = self
            .rows
            .iter()
            .map(|row| {
                let s: String = row.iter().map(|cell| cell.ch).collect();
                s.trim_end_matches(' ').to_string()
            })
            .collect();
        while matches!(lines.last(), Some(l) if l.is_empty()) {
            lines.pop();
        }
        lines.join("\n")
    }

    /// Render the grid as 24-bit-color ANSI: `\x1b[38;2;R;G;Bm` for
    /// foreground, `\x1b[48;2;R;G;Bm` for background (or `49` — "default
    /// background" — when a cell's `bg` is fully transparent), both packed
    /// into ONE SGR escape per color change, `\x1b[0m` reset at the end of
    /// each line, rows joined with `\n`. Run-length optimized: the escape is
    /// only re-emitted when `(fg, bg)` differs from the previous cell, so a
    /// uniformly-colored line emits exactly one escape (at its first cell),
    /// not one per cell. Total: bounded by the grid's own already-capped
    /// size (`MAX_GRID_COLS` × `MAX_GRID_ROWS`); every color component is a
    /// `u8`, so there's no non-finite/out-of-range value to guard against.
    pub fn to_ansi(&self) -> String {
        let mut out = String::new();
        for (i, row) in self.rows.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            let mut last: Option<(Color, Color)> = None;
            for cell in row {
                if last != Some((cell.fg, cell.bg)) {
                    if cell.bg.a == 0 {
                        out.push_str(&format!("\x1b[38;2;{};{};{};49m", cell.fg.r, cell.fg.g, cell.fg.b));
                    } else {
                        out.push_str(&format!(
                            "\x1b[38;2;{};{};{};48;2;{};{};{}m",
                            cell.fg.r, cell.fg.g, cell.fg.b, cell.bg.r, cell.bg.g, cell.bg.b
                        ));
                    }
                    last = Some((cell.fg, cell.bg));
                }
                out.push(cell.ch);
            }
            out.push_str("\x1b[0m");
        }
        out
    }

    #[cfg(test)]
    fn row_text(&self, row: usize) -> String {
        self.rows.get(row).map(|r| r.iter().map(|cell| cell.ch).collect()).unwrap_or_default()
    }

    #[cfg(test)]
    fn cell_at(&self, row: usize, col: usize) -> Cell {
        self.rows[row][col]
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

    let mut rows: Vec<Vec<Cell>> = vec![vec![Cell::default(); cols]; rows_needed];

    for f in fragments {
        match &f.kind {
            FragmentKind::Text { text, style, .. } => write_marker(&mut rows, f, text, Some(style.color), cols),
            FragmentKind::Image { .. } => write_marker(&mut rows, f, "[img]", None, cols),
            FragmentKind::Box { style } => fill_box(&mut rows, f, style.background_color, cols),
        }
    }

    TextGrid { rows }
}

/// Fill the cells `fragment.rect` covers with `bg`, clipped to the grid's
/// bounds in both axes. A fully transparent `bg` (`a == 0` — the UA default,
/// and any author `background-color` that resolves to `transparent`) is a
/// no-op: it leaves whatever a prior fragment already painted in those
/// cells, same as "paints nothing". No border is drawn — see module docs.
///
/// Row/col spans use the same `cell_index` rounding `write_marker` and
/// `render`'s own row-count math use, widened to at least one cell so a
/// fragment with a real (non-zero, finite) size always covers something
/// even if its rounded span would otherwise collapse to empty.
fn fill_box(rows: &mut [Vec<Cell>], fragment: &Fragment, bg: Color, cols: usize) {
    if bg.a == 0 {
        return;
    }
    let row_start = cell_index(fragment.rect.origin.y, CELL_H);
    let row_end = cell_index(fragment.rect.origin.y + nonneg(fragment.rect.size.h), CELL_H).max(row_start + 1);
    let col_start = cell_index(fragment.rect.origin.x, CELL_W).min(cols);
    let col_end = cell_index(fragment.rect.origin.x + nonneg(fragment.rect.size.w), CELL_W).min(cols).max(col_start);
    for row in rows.iter_mut().skip(row_start).take(row_end - row_start) {
        for cell in row.iter_mut().skip(col_start).take(col_end - col_start) {
            cell.bg = bg;
        }
    }
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
/// `fg`, when `Some`, is written onto every cell placed (`Text`'s own
/// `style.color`); `None` (the `Image` placeholder — no `ComputedStyle` to
/// draw from) leaves each cell's `fg` at whatever it already was (the grid
/// default unless something upstream already colored it). Either way `bg`
/// is left untouched — see module docs' "text keeps the box's bg" rule.
fn write_marker(rows: &mut [Vec<Cell>], fragment: &Fragment, text: &str, fg: Option<Color>, cols: usize) {
    let row = cell_index(fragment.rect.origin.y, CELL_H);
    if row >= rows.len() {
        return;
    }
    let mut col = cell_index(fragment.rect.origin.x, CELL_W);
    for ch in text.chars() {
        if col >= cols {
            break;
        }
        rows[row][col].ch = ch;
        if let Some(c) = fg {
            rows[row][col].fg = c;
        }
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

    fn box_fragment_bg(x: f32, y: f32, w: f32, h: f32, bg: Color) -> Fragment {
        Fragment {
            rect: Rect { origin: Point { x, y }, size: Size { w, h } },
            kind: FragmentKind::Box { style: ComputedStyle { background_color: bg, ..ComputedStyle::default() } },
            interactive: None,
        }
    }

    fn text_fragment_fg(x: f32, y: f32, w: f32, h: f32, text: &str, fg: Color) -> Fragment {
        Fragment {
            rect: Rect { origin: Point { x, y }, size: Size { w, h } },
            kind: FragmentKind::Text {
                text: text.to_string(),
                baseline: h * 0.75,
                style: ComputedStyle { color: fg, ..ComputedStyle::default() },
            },
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
    fn box_fragments_with_default_transparent_bg_paint_no_visible_text() {
        // `to_text` is blind to color by construction, so this stays true
        // whether or not `Box` paints a `bg` — a default (transparent)
        // `background_color` paints nothing either way (see
        // `box_fragment_bg_fills_the_covered_cells` for the colored case).
        let fragments = vec![box_fragment(0.0, 0.0, 80.0, 16.0), text_fragment(0.0, 0.0, 16.0, 16.0, "x")];
        let grid = render(&fragments, 10);
        assert_eq!(grid.row_text(0).trim_end(), "x");
    }

    // ------------------------------------------------------- color (P7 tty-color)

    #[test]
    fn box_fragment_bg_fills_the_covered_cells() {
        let navy = Color::rgb(0, 0, 128);
        let fragments = vec![box_fragment_bg(0.0, 0.0, 24.0, 16.0, navy)];
        let grid = render(&fragments, 10);
        // 24px wide / 8px cell = 3 cells covered, one row.
        for col in 0..3 {
            assert_eq!(grid.cell_at(0, col).bg, navy, "col {col}");
        }
        assert_eq!(grid.cell_at(0, 3).bg, Color::TRANSPARENT, "col 3 is outside the box");
    }

    #[test]
    fn box_fragment_with_transparent_bg_leaves_cells_untouched() {
        let fragments = vec![box_fragment(0.0, 0.0, 24.0, 16.0)]; // default style: TRANSPARENT
        let grid = render(&fragments, 10);
        assert_eq!(grid.cell_at(0, 0).bg, Color::TRANSPARENT);
    }

    #[test]
    fn text_fragment_sets_fg_from_its_own_style_color() {
        let red = Color::rgb(255, 0, 0);
        let fragments = vec![text_fragment_fg(0.0, 0.0, 16.0, 16.0, "hi", red)];
        let grid = render(&fragments, 10);
        assert_eq!(grid.cell_at(0, 0).fg, red);
        assert_eq!(grid.cell_at(0, 1).fg, red);
    }

    #[test]
    fn text_over_a_colored_box_keeps_the_boxs_bg() {
        // Paint order (Box before Text, per fragment order) means the text's
        // own write must NOT clobber the bg the enclosing box already left.
        let navy = Color::rgb(0, 0, 128);
        let white = Color::rgb(255, 255, 255);
        let fragments = vec![box_fragment_bg(0.0, 0.0, 16.0, 16.0, navy), text_fragment_fg(0.0, 0.0, 16.0, 16.0, "hi", white)];
        let grid = render(&fragments, 10);
        let cell = grid.cell_at(0, 0);
        assert_eq!(cell.ch, 'h');
        assert_eq!(cell.fg, white);
        assert_eq!(cell.bg, navy, "text write must not clobber the box's bg");
    }

    #[test]
    fn image_placeholder_leaves_fg_at_the_grid_default() {
        // `FragmentKind::Image` carries no `ComputedStyle` to color from.
        let fragments = vec![image_fragment(0.0, 0.0)];
        let grid = render(&fragments, 20);
        assert_eq!(grid.cell_at(0, 0).fg, Color::BLACK);
    }

    #[test]
    fn to_text_is_blind_to_cell_color() {
        // A colored grid and its monochrome twin must print identical text —
        // this is the guard that every existing tty golden relies on.
        let navy = Color::rgb(0, 0, 128);
        let white = Color::rgb(255, 255, 255);
        let colored = vec![box_fragment_bg(0.0, 0.0, 40.0, 16.0, navy), text_fragment_fg(0.0, 0.0, 16.0, 16.0, "hi", white)];
        let mono = vec![box_fragment(0.0, 0.0, 40.0, 16.0), text_fragment(0.0, 0.0, 16.0, 16.0, "hi")];
        assert_eq!(render(&colored, 10).to_text(), render(&mono, 10).to_text());
    }

    #[test]
    fn to_ansi_emits_run_length_optimized_escapes_with_reset_at_line_end() {
        // packet/tty-color: the fg must survive `resolve_cell_colors` unforced
        // for this test to actually exercise a run split (a color that fails
        // the 4.5:1 contrast floor against `navy` would get forced to the
        // SAME fallback the default-fg cell below also forces to, collapsing
        // both cells into one run and defeating the point of this test).
        // Yellow clears contrast against navy comfortably; the default black
        // fg in cell 2 does not, and gets forced to white -- two distinct
        // resolved pairs, i.e. still one escape per change.
        let yellow = Color::rgb(255, 255, 0);
        let navy = Color::rgb(0, 0, 128);
        let fragments = vec![box_fragment_bg(0.0, 0.0, 24.0, 16.0, navy), text_fragment_fg(0.0, 0.0, 16.0, 16.0, "hi", yellow)];
        let grid = render(&fragments, 3);
        // Row is 3 cells: "h" (yellow/navy), "i" (yellow/navy), " " (default
        // fg forced to white/navy, since BLACK-on-navy fails 4.5:1). fg
        // changes at cell 2, bg stays navy throughout, so only ONE escape
        // should fire per fg change.
        let expected = "\x1b[38;2;255;255;0;48;2;0;0;128mhi\x1b[38;2;255;255;255;48;2;0;0;128m \x1b[0m";
        assert_eq!(grid.to_ansi(), expected);
    }

    #[test]
    fn to_ansi_uniformly_colored_line_emits_exactly_one_escape() {
        // packet/tty-color: pure white text on the terminal's own (unset)
        // canvas is an "extreme" color (L > 0.85) per `resolve_cell_colors`,
        // so it resolves to the terminal's own default fg/bg (39/49) rather
        // than a literal white RGB triplet -- still exactly one escape per
        // uniformly-colored run, just not a literal color passthrough.
        let white = Color::rgb(255, 255, 255);
        let fragments = vec![text_fragment_fg(0.0, 0.0, 32.0, 16.0, "abcd", white)];
        let grid = render(&fragments, 4);
        let ansi = grid.to_ansi();
        assert_eq!(ansi.matches('\x1b').count(), 2, "one color escape + one reset, got: {ansi:?}");
        assert_eq!(ansi, "\x1b[39;49mabcd\x1b[0m");
    }

    #[test]
    fn to_ansi_joins_rows_with_newline_and_resets_each_line() {
        let fragments = vec![text_fragment(0.0, 0.0, 8.0, 16.0, "a"), text_fragment(0.0, 16.0, 8.0, 16.0, "b")];
        let grid = render(&fragments, 1);
        let ansi = grid.to_ansi();
        let lines: Vec<&str> = ansi.split('\n').collect();
        assert_eq!(lines.len(), 2);
        for line in &lines {
            assert!(line.ends_with("\x1b[0m"), "line must reset at the end: {line:?}");
        }
    }

    #[test]
    fn to_ansi_on_an_empty_grid_is_an_empty_string() {
        let grid = render(&[], 10);
        assert_eq!(grid.to_ansi(), "");
    }

    // ------------------- resolve_cell_colors: B+C readability (packet/tty-color)

    #[test]
    fn resolve_unstyled_cell_emits_terminal_defaults() {
        // (BLACK, TRANSPARENT) is the grid's own default `Cell` -- an
        // uncolored document must render as the terminal's own default
        // fg/bg (SGR 39/49), NOT literal black-on-whatever-49-means. This is
        // the exact bug this packet fixes: black-on-black on a dark
        // terminal is invisible.
        assert_eq!(resolve_cell_colors(Color::BLACK, Color::TRANSPARENT), (None, None));
    }

    #[test]
    fn resolve_unset_fg_on_transparent_bg_emits_terminal_defaults() {
        assert_eq!(resolve_cell_colors(Color::TRANSPARENT, Color::TRANSPARENT), (None, None));
    }

    #[test]
    fn resolve_near_white_text_on_transparent_bg_falls_back_to_terminal_default_fg() {
        // Near-white (L > 0.85) on the terminal's own (unknown) canvas could
        // vanish on a light-theme terminal; the terminal's own default fg is
        // guaranteed visible against its own background.
        assert_eq!(resolve_cell_colors(Color::WHITE, Color::TRANSPARENT), (None, None));
    }

    #[test]
    fn resolve_chromatic_link_color_on_transparent_bg_passes_through() {
        // A mid-tone/chromatic color (neither near-black nor near-white)
        // contrasts acceptably against either a dark or a light terminal
        // background, so it's preserved rather than defaulted away.
        let link = Color::rgb(0x33, 0x66, 0xcc);
        assert_eq!(resolve_cell_colors(link, Color::TRANSPARENT), (Some(link), None));
    }

    #[test]
    fn resolve_hostile_dark_on_dark_is_forced_to_white_on_the_authors_own_bg() {
        // The author set a concrete (opaque) bg, so we own this cell's whole
        // canvas -- #222-on-#111 fails the 4.5:1 contrast floor, so fg is
        // forced to max-contrast white rather than staying #222.
        let fg = Color::rgb(0x22, 0x22, 0x22);
        let bg = Color::rgb(0x11, 0x11, 0x11);
        assert_eq!(resolve_cell_colors(fg, bg), (Some(Color::WHITE), Some(bg)));
    }

    #[test]
    fn resolve_light_card_text_is_forced_to_black_when_the_authors_own_gray_falls_short_of_4_5_to_1() {
        // #333 on #eee is the packet brief's own illustrative "readable
        // card" example, described there as adequate contrast. Under the
        // SPECIFIED (non-gamma-corrected) luminance formula L = (0.2126r +
        // 0.7152g + 0.0722b)/255, this pair's ratio is:
        //   L(#333) = 51/255 = 0.2, L(#eee) = 238/255 = 0.93333...
        //   ratio = (0.93333 + 0.05) / (0.2 + 0.05) = 0.98333 / 0.25 = 3.9333
        // -- under the 4.5:1 floor, so rule 2's "else" branch fires and
        // forces max-contrast BLACK rather than letting #333 pass through.
        // (A gamma-corrected/true-WCAG luminance would put this same pair
        // around ~10.8:1 instead -- comfortably adequate -- but flipping to
        // gamma-corrected luminance would ALSO push the chromatic-link case
        // above (#3366cc, L ~= 0.146 under gamma vs 0.386 under the linear
        // formula) below the 0.15 "extreme" cutoff, breaking THAT case
        // instead. The two illustrative examples in the brief are mutually
        // exclusive under a single luminance model; this implementation
        // follows the brief's explicit formula literally and reports this
        // discrepancy back rather than silently picking one to special-case.
        // Still fully readable either way: forced BLACK-on-#eee, just not
        // byte-identical to the author's own gray.)
        let fg = Color::rgb(0x33, 0x33, 0x33);
        let bg = Color::rgb(0xee, 0xee, 0xee);
        assert_eq!(resolve_cell_colors(fg, bg), (Some(Color::BLACK), Some(bg)));
    }

    #[test]
    fn resolve_unset_fg_on_concrete_dark_bg_defaults_to_white() {
        let bg = Color::rgb(0x11, 0x11, 0x11);
        assert_eq!(resolve_cell_colors(Color::TRANSPARENT, bg), (Some(Color::WHITE), Some(bg)));
    }

    #[test]
    fn resolve_unset_fg_on_concrete_light_bg_defaults_to_black() {
        let bg = Color::rgb(0xee, 0xee, 0xee);
        assert_eq!(resolve_cell_colors(Color::TRANSPARENT, bg), (Some(Color::BLACK), Some(bg)));
    }

    #[test]
    fn to_ansi_unstyled_cell_uses_terminal_defaults_not_literal_black() {
        let fragments = vec![text_fragment(0.0, 0.0, 8.0, 16.0, "x")];
        let grid = render(&fragments, 1);
        let ansi = grid.to_ansi();
        assert!(ansi.contains("39"), "expected default-fg SGR 39, got: {ansi:?}");
        assert!(ansi.contains("49"), "expected default-bg SGR 49, got: {ansi:?}");
        assert!(!ansi.contains("38;2;0;0;0"), "must not emit literal black fg for unstyled text: {ansi:?}");
    }

    #[test]
    fn box_fill_with_degenerate_rect_never_panics() {
        let navy = Color::rgb(0, 0, 128);
        let degenerate = [
            (f32::NAN, f32::NAN),
            (f32::INFINITY, f32::INFINITY),
            (f32::NEG_INFINITY, f32::NEG_INFINITY),
            (-1.0, -1.0),
            (f32::MAX, f32::MAX),
        ];
        for (x, y) in degenerate {
            let fragments = vec![box_fragment_bg(x, y, f32::NAN, f32::INFINITY, navy)];
            let grid = render(&fragments, 40);
            let _ = grid.to_ansi(); // must not panic
        }
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

    // --------------------------------------- cols/get/set/window (P7 shell)

    #[test]
    fn cols_reports_the_grids_width_zero_when_empty() {
        assert_eq!(TextGrid::blank(5, 3).cols(), 5);
        assert_eq!(TextGrid::blank(0, 0).cols(), 0);
    }

    #[test]
    fn get_out_of_bounds_returns_the_default_cell_not_a_panic() {
        let grid = TextGrid::blank(2, 2);
        assert_eq!(grid.get(0, 0), Cell::default());
        assert_eq!(grid.get(99, 99), Cell::default());
    }

    #[test]
    fn set_writes_a_cell_in_bounds_and_is_a_silent_no_op_out_of_bounds() {
        let mut grid = TextGrid::blank(2, 2);
        let navy = Color::rgb(0, 0, 128);
        grid.set(0, 1, Cell { ch: 'x', fg: Color::BLACK, bg: navy });
        assert_eq!(grid.get(0, 1).ch, 'x');
        grid.set(99, 99, Cell { ch: 'z', fg: Color::BLACK, bg: navy }); // must not panic
    }

    #[test]
    fn window_crops_a_sub_range_of_rows_at_the_same_width() {
        let fragments =
            vec![text_fragment(0.0, 0.0, 8.0, 16.0, "a"), text_fragment(0.0, 16.0, 8.0, 16.0, "b"), text_fragment(0.0, 32.0, 8.0, 16.0, "c")];
        let grid = render(&fragments, 4);
        let win = grid.window(1, 2);
        assert_eq!(win.rows_len(), 2);
        assert_eq!(win.row_text(0), "b   ");
        assert_eq!(win.row_text(1), "c   ");
    }

    #[test]
    fn window_pads_with_blank_rows_past_the_grids_own_height() {
        let fragments = vec![text_fragment(0.0, 0.0, 8.0, 16.0, "a")];
        let grid = render(&fragments, 4); // 1 row tall
        let win = grid.window(0, 3);
        assert_eq!(win.rows_len(), 3);
        assert_eq!(win.row_text(0), "a   ");
        assert_eq!(win.row_text(1), "    ");
        assert_eq!(win.row_text(2), "    ");
    }

    #[test]
    fn window_starting_past_the_grids_end_is_all_blank_not_a_panic() {
        let grid = TextGrid::blank(3, 2);
        let win = grid.window(50, 2);
        assert_eq!(win.rows_len(), 2);
        assert_eq!(win.to_text(), "");
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
