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
//!   `background_color` as cell `bg` (see "Color" below). Borders are still
//!   mostly out of scope in text mode — a full cell-accurate box-drawing
//!   pass (four sides, corners) for ARBITRARY boxes is deferred rather than
//!   half-done — with TWO narrow exceptions:
//!     - (packet/hr-rule) a box whose top border is `Solid`, `>= 1px`, AND
//!       whose OWN right/bottom/left are not ALSO a `Solid`/`>= 1px` border
//!       draws a `'─'` (U+2500) rule across the box's own width, in the
//!       border color, via `draw_top_border_rule`. This exists specifically
//!       so `<hr>` (a zero-content-height box with only `border-top` set in
//!       the UA sheet, `style::ua::UA_CSS`) shows as a visible horizontal
//!       line instead of blank space. The "sole solid side" gate matters: a
//!       box bordered on all four sides (e.g. a bordered flex child) draws
//!       NOTHING here — a lone top tick with no matching sides/bottom would
//!       look like a glitch, not a border, and that box's real border still
//!       paints correctly in the pixel/fb backend either way.
//!     - (packet/border-collapse follow-up) a `Display::Table`/
//!       `Display::TableCell` box with ANY solid border side draws real
//!       box-drawing grid lines (`'─'`/`'│'`, plus simple `┌┐└┘` corners) via
//!       `draw_table_grid_lines` — see that function's own doc comment. This
//!       is what turns a bordered/collapsed table into a readable ruled grid
//!       in tty mode instead of unseparated cell text (`"Widget4"`). Scoped
//!       tightly to table/cell boxes only — an ordinary bordered `<div>` or a
//!       bordered flex child still draws nothing here, same as before.
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
//!   `bg` was already there untouched, same as painting nothing. Borders are
//!   still otherwise out of scope in text mode (full ASCII-art box-drawing —
//!   all four sides, corners — is a distinct, still-deferred pass), except
//!   for the one solid top-border rule line `draw_top_border_rule` draws
//!   (packet/hr-rule, `fg` = the border's own color) — see "What paints,
//!   what doesn't" above.
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
//! ## Readability contract (packet/tty-color)
//!
//! `to_ansi` never emits a cell's raw `fg`/`bg` literally — it always routes
//! through `resolve_cell_colors` first, which can defer to the terminal's
//! own SGR defaults (`39`/`49`) instead of a concrete RGB triplet. This
//! fixes a real usability bug: the grid's default cell is `fg: Color::BLACK,
//! bg: Color::TRANSPARENT`, and emitting that literally is black text with
//! no background override — on any dark terminal (the common case) that's
//! black-on-black, invisible. See `resolve_cell_colors`'s own doc for the
//! full "B+C" rule set (terminal-native defaults for unset/extreme colors
//! on the terminal's own canvas; WCAG-contrast-forced fg wherever an author
//! sets a concrete background).
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
use crate::style::computed::{BorderCollapse, BorderSide, BorderStyle, ComputedStyle, Display};
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
    /// foreground, `\x1b[48;2;R;G;Bm` for background, or the terminal's own
    /// SGR defaults (`39` fg / `49` bg) wherever [`resolve_cell_colors`]
    /// says the concrete color isn't safe to claim — see that function's
    /// doc for the readability contract. Both halves are packed into ONE
    /// SGR escape per color change, `\x1b[0m` reset at the end of each line,
    /// rows joined with `\n`. Run-length optimized: the escape is only
    /// re-emitted when the RESOLVED `(fg, bg)` pair differs from the
    /// previous cell (not the raw, unresolved cell), so a uniformly-colored
    /// (or uniformly-defaulted) line emits exactly one escape, not one per
    /// cell. Total: bounded by the grid's own already-capped size
    /// (`MAX_GRID_COLS` × `MAX_GRID_ROWS`); every color component is a
    /// `u8` and every comparison in `resolve_cell_colors` is plain `f64`
    /// arithmetic over `u8`-derived values, so there's no non-finite or
    /// out-of-range value to guard against.
    pub fn to_ansi(&self) -> String {
        let mut out = String::new();
        for (i, row) in self.rows.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            let mut last: Option<(Option<Color>, Option<Color>)> = None;
            for cell in row {
                let resolved = resolve_cell_colors(cell.fg, cell.bg);
                if last != Some(resolved) {
                    let fg_sgr = match resolved.0 {
                        Some(c) => format!("38;2;{};{};{}", c.r, c.g, c.b),
                        None => "39".to_string(),
                    };
                    let bg_sgr = match resolved.1 {
                        Some(c) => format!("48;2;{};{};{}", c.r, c.g, c.b),
                        None => "49".to_string(),
                    };
                    out.push_str(&format!("\x1b[{fg_sgr};{bg_sgr}m"));
                    last = Some(resolved);
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
            FragmentKind::Box { style } => {
                fill_box(&mut rows, f, style.background_color, cols);
                draw_top_border_rule(&mut rows, f, style, cols);
                draw_table_grid_lines(&mut rows, f, style, cols);
            }
        }
    }

    TextGrid { rows }
}

/// Fill the cells `fragment.rect` covers with `bg`, clipped to the grid's
/// bounds in both axes. A fully transparent `bg` (`a == 0` — the UA default,
/// and any author `background-color` that resolves to `transparent`) is a
/// no-op: it leaves whatever a prior fragment already painted in those
/// cells, same as "paints nothing". No border is drawn here — see
/// `draw_top_border_rule` right below for the one border edge the tty
/// backend does draw, and module docs for the rest of the (still deferred)
/// border scope.
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

/// Draw a horizontal rule (`'─'`, U+2500) across `fragment`'s TOP row when
/// its top border is the box's SOLE solid border — the tty backend's first
/// (and, per the packet brief, ONLY) border edge: `<hr>`'s UA rule (packet/
/// hr-rule) is a zero-content-height box with just a solid top border, and
/// this is what turns that into a visible `────────` line in text mode.
///
/// Deliberately narrower than "any solid top border": a box with a full
/// 4-side border (e.g. a bordered `<table>` cell, or a bordered flex child —
/// both real cases in `fixtures/kitchen-sink.html`) does NOT get a rule here
/// — with no matching side/bottom edges to accompany it (tty draws none —
/// see module docs), a lone top tick over an otherwise-unbordered-looking
/// cell reads as a rendering glitch, not a border, and the pixel/fb backend
/// already draws all four of THAT box's edges correctly on its own. So the
/// gate is: `top` is `Solid`/`>= 1px` AND none of right/bottom/left is ALSO
/// a `Solid`/`>= 1px` border — i.e. this box's border is genuinely top-only
/// (an `<hr>`, or an intentional author `border-top`-only separator), never
/// one edge of a box that's bordered all the way around.
///
/// Uses the SAME column span `fill_box` computes for this fragment (clipped
/// to the grid's width) — the rule spans exactly the box's own width, one
/// row, so this is bounded by the grid's already-capped column count, never
/// an unbounded loop.
fn draw_top_border_rule(rows: &mut [Vec<Cell>], fragment: &Fragment, style: &ComputedStyle, cols: usize) {
    let top = style.border.top;
    if top.style != BorderStyle::Solid || !(top.width >= 1.0) {
        return;
    }
    let is_solid = |side: BorderSide| side.style == BorderStyle::Solid && side.width >= 1.0;
    if is_solid(style.border.right) || is_solid(style.border.bottom) || is_solid(style.border.left) {
        // A full (or partial-but-not-top-only) border belongs to the still-
        // deferred ASCII-art box-drawing pass, not this narrow rule-line
        // special case — see this function's own doc comment.
        return;
    }
    let row = cell_index(fragment.rect.origin.y, CELL_H);
    let Some(row_cells) = rows.get_mut(row) else { return };
    let col_start = cell_index(fragment.rect.origin.x, CELL_W).min(cols);
    let col_end = cell_index(fragment.rect.origin.x + nonneg(fragment.rect.size.w), CELL_W).min(cols).max(col_start);
    for cell in row_cells.iter_mut().skip(col_start).take(col_end - col_start) {
        cell.ch = '─';
        cell.fg = top.color;
    }
}

/// Draw box-drawing GRID lines (`'─'`/`'│'`, with simple L-corner glyphs
/// `┌┐└┘` where two adjacent solid sides meet) for a TABLE or TABLE-CELL
/// box's own solid border sides (coordinator-directed follow-up to the
/// `border-collapse` packet: the sole-top-border rule (`draw_top_border_rule`)
/// deliberately declines a fully-bordered box, which used to leave collapsed/
/// separate bordered tables showing NO visual separation at all in tty mode —
/// e.g. adjacent cell text reading as `"Widget4"` once `border-collapse`
/// dropped border-spacing to 0 between cells with no border lines to fill the
/// gap).
///
/// SCOPE: gated on `style.display` being `Display::Table` or
/// `Display::TableCell` ONLY — every other box (an ordinary bordered `<div>`,
/// a bordered flex child, `<hr>`'s sole-top rule) is completely unaffected;
/// `draw_top_border_rule` right above still owns that territory exactly as
/// before. An unbordered table/cell (no solid side at all) also draws
/// nothing here, so a plain `<table>` with no `border`/CSS borders renders
/// exactly as it did before this function existed.
///
/// Each side is drawn independently along its own edge (top/bottom → `─`
/// across the full width, left/right → `│` down the full height) in that
/// side's OWN border color; a corner cell where two adjacent sides are BOTH
/// solid is then overwritten with the matching L-corner glyph. This is a
/// deliberately simple, PER-BOX approximation, not full multi-box junction
/// resolution (real box-drawing would want `┬`/`┴`/`├`/`┤`/`┼` wherever a
/// junction is shared by three or four cells, not just two) — readability is
/// the bar here, not pixel-perfect box art, and paint order (later fragments
/// overwrite earlier ones, the same rule this whole module already follows)
/// means a specific cell's own corner glyph naturally wins over whatever a
/// neighboring box's line left in that same tty cell. For the uniform-border
/// tables this packet targets (a `<table border>` stamp, or `td { border:
/// 1px solid } table { border-collapse: collapse }`), the result reads as a
/// clean grid; a table with genuinely non-uniform per-cell borders may show
/// a visually rougher (but still legible) join — out of scope, same follow-up
/// note as the layout-side dedup step's own doc comment
/// (`layout::box_tree::apply_border_collapse`).
///
/// The START index uses the SAME rounding `cell_index` (and therefore
/// `fill_box`/`draw_top_border_rule`) already use, so a box's own top-left
/// grid line still lines up with where its own content/other fragments land.
/// The END index is a CEILING, not `cell_index`'s round-to-nearest: a
/// padded table cell's real pixel height (e.g. `6px padding + 16px line +
/// 6px padding` = 28px, not a clean multiple of `CELL_H`) would otherwise
/// round its END down to the SAME row as its START (`round(28/16) = round
/// (1.75) = 2`, only one row past start, when the cell visually needs a
/// full two-row span to show both a top rule row's worth of content AND its
/// own bottom rule without the two colliding) -- `fill_box`'s own round-
/// based end is fine for a background fill (a slightly short fill is
/// invisible), but a border's END edge is a whole separate LINE of
/// characters that must land on its OWN row, not silently merge into the
/// start row. Ceiling (always rounding UP) guarantees the span is never
/// narrower than the real content, at the cost of occasionally being one
/// row/col more generous than the tightest possible fit -- a harmless
/// over-approximation for grid lines specifically (unlike a background
/// fill, where over-filling would visibly bleed color into a neighbor).
fn span_end(start_px: f32, size_px: f32, cell: f32, start_idx: usize) -> usize {
    let end_px = start_px + nonneg(size_px);
    if !end_px.is_finite() || cell <= 0.0 {
        return start_idx + 1;
    }
    let end = (end_px / cell).ceil();
    if end < 0.0 || !end.is_finite() {
        return start_idx + 1;
    }
    (end as usize).max(start_idx + 1)
}

fn draw_table_grid_lines(rows: &mut [Vec<Cell>], fragment: &Fragment, style: &ComputedStyle, cols: usize) {
    if !matches!(style.display, Display::Table | Display::TableCell) {
        return;
    }
    let is_solid = |side: BorderSide| side.style == BorderStyle::Solid && side.width >= 1.0;
    let top = is_solid(style.border.top);
    let right = is_solid(style.border.right);
    let bottom = is_solid(style.border.bottom);
    let left = is_solid(style.border.left);
    if !(top || right || bottom || left) {
        // Unbordered table/cell: draw nothing at all -- a plain `<table>`
        // with no visible border renders exactly as plain text, same as
        // before this function existed.
        return;
    }

    // Rows use the generous `span_end` (see its own doc comment: a padded
    // cell's real height easily rounds down to a too-short row span, which
    // would silently drop its own bottom border row). Columns stay on the
    // ORIGINAL round-based `cell_index` `fill_box`/`draw_top_border_rule`
    // already use: unlike rows, a table's columns are typically many cells
    // wide relative to `CELL_W`, so this squashing risk is far smaller, and
    // (empirically, see the packet report) a generous ceiling on columns
    // instead makes ADJACENT cells' rects overlap by a column, which then
    // lets one cell's later paint silently erase its neighbor's corner/
    // vertical-bar characters — worse than the squashing it would fix.
    let row_start = cell_index(fragment.rect.origin.y, CELL_H);
    let row_end = span_end(fragment.rect.origin.y, fragment.rect.size.h, CELL_H, row_start);
    let col_start = cell_index(fragment.rect.origin.x, CELL_W).min(cols);
    let col_end = cell_index(fragment.rect.origin.x + nonneg(fragment.rect.size.w), CELL_W).min(cols).max(col_start);
    if row_end <= row_start || col_end <= col_start {
        return; // degenerate (zero-cell) span: nothing to draw
    }
    let last_row = row_end - 1;
    // packet/collapse-geometry: for a COLLAPSED table only, the RIGHT/bottom
    // rule's own tty column is the far grid-line index itself (`col_end`),
    // NOT one column inside it (`col_end - 1`, every other case's -- and
    // this function's own pre-packet -- convention) -- matching how `left`'s
    // rule already lands directly AT `col_start` with no offset. A collapsed
    // cell now keeps its full 4-side border and is positioned (see
    // `layout::block`'s own "packet/collapse-geometry" doc section) to
    // OVERLAP its neighbor by one border-width, a deliberate sub-pixel nudge
    // that makes `cell_index` round BOTH a cell's own far edge and its
    // neighbor's near edge to the exact SAME tty column/row -- but only if
    // this side's own rule lands on that shared column too, not the column
    // just before it. `style.border_collapse` is reliable here even for a
    // CELL fragment (normally not inherited, so a bare cell would read
    // `Separate` regardless of its table) because `box_tree::build_node`
    // stamps every cell's own field to match its table's resolved value
    // specifically so this gate can see it -- see that stamp's own doc
    // comment. A `Separate` table (the CSS default -- cells kept visually
    // apart by `border-spacing`/`cellspacing`, e.g. `table-spacing.html`)
    // MUST NOT get this treatment: two cells separated by a real gap would
    // otherwise appear to visually merge at a shared column that isn't
    // actually shared. Trade-off for the collapsed case: an ISOLATED
    // collapsed box with no neighbor to coincide with (e.g. a lone table
    // with a border and a single cell) renders its right/bottom rule one
    // tty column/row past its exact pixel width (see
    // `fully_bordered_table_box_draws_all_four_grid_sides_with_corners`'s
    // own doc comment) -- a harmless sub-cell-rounding artifact, not a
    // misplacement of any real content.
    let last_col = if style.border_collapse == BorderCollapse::Collapse { col_end } else { col_end - 1 };

    if top {
        if let Some(r) = rows.get_mut(row_start) {
            for c in r.iter_mut().take(col_end).skip(col_start) {
                c.ch = '─';
                c.fg = style.border.top.color;
            }
        }
    }
    if bottom && last_row != row_start {
        if let Some(r) = rows.get_mut(last_row) {
            for c in r.iter_mut().take(col_end).skip(col_start) {
                c.ch = '─';
                c.fg = style.border.bottom.color;
            }
        }
    }
    if left {
        for r in rows.iter_mut().take(row_end).skip(row_start) {
            if let Some(c) = r.get_mut(col_start) {
                c.ch = '│';
                c.fg = style.border.left.color;
            }
        }
    }
    if right && last_col != col_start {
        for r in rows.iter_mut().take(row_end).skip(row_start) {
            if let Some(c) = r.get_mut(last_col) {
                c.ch = '│';
                c.fg = style.border.right.color;
            }
        }
    }

    // Corners: only where BOTH sides meeting there are solid -- see this
    // function's own doc comment on why this is a per-box approximation,
    // not full junction resolution.
    set_grid_ch(rows, row_start, col_start, '┌', style.border.top.color, top && left);
    set_grid_ch(rows, row_start, last_col, '┐', style.border.top.color, top && right && last_col != col_start);
    set_grid_ch(rows, last_row, col_start, '└', style.border.bottom.color, bottom && left && last_row != row_start);
    set_grid_ch(
        rows,
        last_row,
        last_col,
        '┘',
        style.border.bottom.color,
        bottom && right && last_row != row_start && last_col != col_start,
    );
}

/// Overwrite one cell's `ch`/`fg` iff `condition` holds and `(row, col)` is
/// in bounds -- the shared totality-safe write `draw_table_grid_lines`'s
/// four corner calls funnel through.
fn set_grid_ch(rows: &mut [Vec<Cell>], row: usize, col: usize, ch: char, fg: Color, condition: bool) {
    if !condition {
        return;
    }
    if let Some(c) = rows.get_mut(row).and_then(|r| r.get_mut(col)) {
        c.ch = ch;
        c.fg = fg;
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

/// Relative luminance, per the brief's specified (deliberately simplified —
/// NOT gamma-corrected sRGB-to-linear) formula: `L = (0.2126*r + 0.7152*g +
/// 0.0722*b) / 255.0`, components promoted to `f64`. Range `0.0` (black) to
/// `1.0` (white). Always finite and in range: every component is a `u8`.
fn relative_luminance(c: Color) -> f64 {
    (0.2126 * c.r as f64 + 0.7152 * c.g as f64 + 0.0722 * c.b as f64) / 255.0
}

/// WCAG-style contrast ratio between two colors, order-independent:
/// `(L1 + 0.05) / (L2 + 0.05)` where `L1` is the larger of the two relative
/// luminances and `L2` the smaller. Range `[1.0, 21.0]`.
fn contrast_ratio(a: Color, b: Color) -> f64 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// The WCAG AA contrast floor this module enforces for any cell where an
/// author has set a concrete (opaque) background: below this ratio, the
/// author's own foreground color is discarded in favor of forced
/// max-contrast black or white (see [`resolve_cell_colors`]).
const MIN_CONTRAST: f64 = 4.5;

/// Luminance below which a color counts as "near-black" for the
/// terminal-default-canvas branch of [`resolve_cell_colors`] (rule 1).
const NEAR_BLACK: f64 = 0.15;

/// Luminance above which a color counts as "near-white" for the same rule.
const NEAR_WHITE: f64 = 0.85;

/// Resolve the `(fg, bg)` a cell should actually be painted with when
/// emitting ANSI — the "B+C" readability contract (packet/tty-color).
/// `None` means "emit the terminal's own SGR default" (`39` for fg, `49`
/// for bg) rather than a concrete color.
///
/// The bug this fixes: the grid's default cell is `fg: Color::BLACK, bg:
/// Color::TRANSPARENT` (`ComputedStyle::default()`'s own initial values).
/// Emitting that literally is black text on whatever the terminal's actual
/// background happens to be — invisible on any dark terminal, i.e. most of
/// them. The fix has two halves:
///
/// 1. **`bg.a == 0`** (no author background — this cell paints onto the
///    terminal's own canvas, whose real color this module can never know):
///    `bg_out` is always `None` (defer to `49`). For `fg`:
///    - unset (`fg.a == 0`) → `None` (defer to `39`).
///    - "extreme" — near-black (`L < 0.15`) or near-white (`L > 0.85`) →
///      also `None`: a black-on-black-themed or white-on-white-themed
///      terminal would swallow it, but the terminal's OWN default fg is
///      guaranteed visible against its own background.
///    - otherwise (a mid-tone/chromatic color, e.g. link blue `#3366cc`) →
///      `Some(fg)`, preserved: a mid color reads acceptably on both dark
///      and light terminal themes, and is meaningful (an author chose it).
/// 2. **`bg.a != 0`** (author set a concrete background — this cell's whole
///    canvas is under this module's control, so it can and must guarantee
///    legibility): `bg_out` is always `Some(bg)`. For `fg`, start from a
///    candidate — the author's own `fg` if set, else black/white chosen by
///    `bg`'s own luminance — then check its contrast ratio against `bg`. If
///    it clears [`MIN_CONTRAST`] (4.5:1, WCAG AA for normal text), keep it;
///    otherwise force max-contrast black or white (by `bg`'s luminance),
///    which is *guaranteed* to clear the floor (black or white against any
///    background reaches at least ~4.5:1 for one of the two).
///
/// NOTE on the luminance formula: the brief specifies the simplified,
/// non-gamma-corrected form above, not full sRGB-linearized WCAG luminance.
/// That choice is followed literally here. One consequence, verified by
/// this module's own tests: `#333` text on a `#eee` background — the
/// brief's own illustrative "readable card" example — computes to a
/// contrast ratio of ~3.93:1 under this formula (just under 4.5:1), so it
/// gets forced to black rather than passing through as `#333`. A
/// gamma-corrected luminance would flip that one case to "adequate" (~10.8:
/// 1) but would ALSO push the brief's other illustrative example — the
/// `#3366cc` link on a transparent bg — from L≈0.386 (comfortably
/// mid-tone) down to L≈0.146 (just under the 0.15 "near-black extreme"
/// cutoff), breaking that case instead. The two examples in the brief
/// aren't simultaneously satisfiable under one luminance model; this
/// implementation follows the brief's own explicit formula rather than
/// silently picking a model to make one example match.
fn resolve_cell_colors(fg: Color, bg: Color) -> (Option<Color>, Option<Color>) {
    if bg.a == 0 {
        if fg.a == 0 {
            return (None, None);
        }
        let l = relative_luminance(fg);
        if l < NEAR_BLACK || l > NEAR_WHITE {
            return (None, None);
        }
        return (Some(fg), None);
    }

    let candidate = if fg.a == 0 {
        if relative_luminance(bg) > 0.5 { Color::BLACK } else { Color::WHITE }
    } else {
        fg
    };
    if contrast_ratio(candidate, bg) >= MIN_CONTRAST {
        (Some(candidate), Some(bg))
    } else {
        let forced = if relative_luminance(bg) > 0.5 { Color::BLACK } else { Color::WHITE };
        (Some(forced), Some(bg))
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

    /// A `Box` fragment whose style carries only a top border (the `<hr>`
    /// shape: zero content height, one solid top edge) — every other side
    /// stays `BorderSide::default()` (`BorderStyle::None`).
    fn box_fragment_top_border(x: f32, y: f32, w: f32, h: f32, border: crate::style::computed::BorderSide) -> Fragment {
        use crate::style::computed::Edges;
        Fragment {
            rect: Rect { origin: Point { x, y }, size: Size { w, h } },
            kind: FragmentKind::Box {
                style: ComputedStyle { border: Edges { top: border, ..Edges::all(Default::default()) }, ..ComputedStyle::default() },
            },
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

    // ------------------------------------------------------- hr rule (packet/hr-rule)

    #[test]
    fn sole_solid_top_border_draws_a_horizontal_rule_across_the_box_width() {
        use crate::style::computed::{BorderSide, BorderStyle};
        let gray = Color::rgb(0x80, 0x80, 0x80);
        let border = BorderSide { width: 1.0, style: BorderStyle::Solid, color: gray };
        // 24px wide box at (0,0) -> 3 cells (0..3) on row 0. `box_fragment_top_border`
        // sets ONLY the top side -- right/bottom/left stay `BorderSide::default()`
        // (BorderStyle::None), the `<hr>` shape this gate is meant to catch.
        let fragments = vec![box_fragment_top_border(0.0, 0.0, 24.0, 0.0, border)];
        let grid = render(&fragments, 10);
        assert_eq!(grid.row_text(0).trim_end(), "\u{2500}\u{2500}\u{2500}", "top-only border should draw '─' across the box's width");
        for col in 0..3 {
            assert_eq!(grid.cell_at(0, col).fg, gray, "col {col} should carry the border color");
        }
        assert_eq!(grid.cell_at(0, 3).ch, ' ', "col 3 is outside the box and must stay blank");
    }

    #[test]
    fn no_border_style_draws_nothing_regression_guard() {
        use crate::style::computed::BorderSide;
        // Default BorderSide (BorderStyle::None) must not draw a rule — the
        // overwhelmingly common case (every box without an explicit border).
        let fragments = vec![box_fragment_top_border(0.0, 0.0, 24.0, 0.0, BorderSide::default())];
        let grid = render(&fragments, 10);
        assert_eq!(grid.row_text(0).trim_end(), "", "no border style set should draw no rule characters");
    }

    #[test]
    fn full_four_side_solid_border_draws_no_tty_rule_regression_guard() {
        // Coordinator-directed narrowing: a NON-TABLE box bordered on ALL
        // FOUR sides (a bordered flex child shape, a real case in
        // `fixtures/kitchen-sink.html` via `border: 1px solid ...`) must
        // draw NOTHING in tty from `draw_top_border_rule` -- a lone top tick
        // with no matching sides/bottom (tty still draws none of those, for
        // non-table boxes) reads as a glitch, not a border. The pixel/fb
        // backend paints all four edges correctly on its own regardless.
        // (A `Display::Table`/`Display::TableCell` box with the SAME border
        // shape DOES now draw a real grid -- see `draw_table_grid_lines`'s
        // own tests below; this fragment stays plain `ComputedStyle::
        // default()` display (`Inline`), so it's unaffected by that.)
        use crate::style::computed::{BorderSide, BorderStyle, Edges};
        let dark = Color::rgb(0x33, 0x33, 0x33);
        let side = BorderSide { width: 1.0, style: BorderStyle::Solid, color: dark };
        let fragments = vec![Fragment {
            rect: Rect { origin: Point { x: 0.0, y: 0.0 }, size: Size { w: 24.0, h: 16.0 } },
            kind: FragmentKind::Box { style: ComputedStyle { border: Edges::all(side), ..ComputedStyle::default() } },
            interactive: None,
        }];
        let grid = render(&fragments, 10);
        assert_eq!(grid.row_text(0).trim_end(), "", "a fully-bordered box must draw no tty rule line");
    }

    // ------------------------------------------- table grid lines (packet/border-collapse follow-up)

    fn table_box_fragment(
        display: Display,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        border: crate::style::computed::Edges<BorderSide>,
    ) -> Fragment {
        Fragment {
            rect: Rect { origin: Point { x, y }, size: Size { w, h } },
            kind: FragmentKind::Box { style: ComputedStyle { display, border, ..ComputedStyle::default() } },
            interactive: None,
        }
    }

    fn solid(color: Color) -> BorderSide {
        BorderSide { width: 1.0, style: BorderStyle::Solid, color }
    }

    #[test]
    fn bordered_table_cell_draws_box_drawing_grid_lines() {
        use crate::style::computed::Edges;
        let gray = Color::rgb(0x80, 0x80, 0x80);
        // A collapsed cell's own shape (packet/border-collapse dedup): top +
        // left solid, right/bottom None. 24px wide (3 cols) x 32px tall (2
        // rows), so there's a second row to prove the left rule continues
        // down the cell, not just at the corner.
        let border = Edges { top: solid(gray), left: solid(gray), right: BorderSide::default(), bottom: BorderSide::default() };
        let fragments = vec![table_box_fragment(Display::TableCell, 0.0, 0.0, 24.0, 32.0, border)];
        let grid = render(&fragments, 10);
        assert_eq!(grid.row_text(0).trim_end(), "┌──", "top row: corner + top rule");
        assert_eq!(grid.cell_at(1, 0).ch, '│', "left column carries the vertical rule on row 1 too");
        for col in 0..3 {
            assert_eq!(grid.cell_at(0, col).fg, gray, "col {col} top border color");
        }
    }

    #[test]
    fn fully_bordered_table_box_draws_all_four_grid_sides_with_corners() {
        use crate::style::computed::Edges;
        let dark = Color::rgb(0x33, 0x33, 0x33);
        let border = Edges::all(solid(dark));
        // 24px wide (3 cells) x 48px tall (3 cells): a small closed
        // rectangle with a real interior row between top and bottom.
        // `border_collapse: Collapse` (NOT `table_box_fragment`'s default
        // `Separate`) -- packet/collapse-geometry gates the "rule lands AT
        // the far grid-line column" convention this test exercises on
        // collapse mode specifically (see `draw_table_grid_lines`'s own doc
        // comment); a `<table border>`'s own frame box IS collapsed by
        // default (the presentational hint), which is the real shape this
        // test represents.
        let fragments = vec![Fragment {
            rect: Rect { origin: Point { x: 0.0, y: 0.0 }, size: Size { w: 24.0, h: 48.0 } },
            kind: FragmentKind::Box {
                style: ComputedStyle { display: Display::Table, border, border_collapse: BorderCollapse::Collapse, ..ComputedStyle::default() },
            },
            interactive: None,
        }];
        let grid = render(&fragments, 10);
        // packet/collapse-geometry: the right/bottom rule now lands AT the
        // box's own far grid-line column (`col_end`/`row_end`), not one
        // column/row INSIDE it (`col_end - 1`) -- so a `border-collapse`
        // cell's shared boundary (which now sits fractionally past its
        // neighbor's own edge, a deliberate sub-pixel overlap -- see
        // `layout::block`'s own "packet/collapse-geometry" doc section)
        // lands on the SAME tty column as its neighbor's opposite edge,
        // instead of the adjacent-but-different column the old `col_end - 1`
        // convention produced (visually a doubled "┐┌"-style seam -- see
        // `tests/kitchen_sink_golden.rs`'s golden for the before/after).
        // The one-time cost: an ISOLATED box with no neighbor to coincide
        // with (like this test's own 24px = exactly-3-tty-cell box) now
        // renders one tty column/row past its exact pixel width -- an
        // acceptable sub-cell-rounding artifact, not a correctness bug (no
        // real content is misplaced, just the rule line's own visual
        // extent).
        assert_eq!(grid.row_text(0).trim_end(), "┌──┐", "top row: both corners + top rule");
        assert_eq!(grid.row_text(1).trim_end(), "│  │", "middle row: left/right rules, blank interior");
        assert_eq!(grid.row_text(2).trim_end(), "└──┘", "bottom row: both corners + bottom rule");
    }

    #[test]
    fn unbordered_table_cell_draws_no_grid_chars() {
        // A plain `<table>` with no `border`/CSS borders: the cell's border
        // stays the CSS default (`BorderStyle::None` on every side), so this
        // must render exactly as before -- text only, no grid chars at all.
        use crate::style::computed::Edges;
        let fragments = vec![
            table_box_fragment(Display::TableCell, 0.0, 0.0, 24.0, 16.0, Edges::all(BorderSide::default())),
            text_fragment(0.0, 0.0, 16.0, 16.0, "hi"),
        ];
        let grid = render(&fragments, 10);
        assert_eq!(grid.row_text(0).trim_end(), "hi", "no border at all should draw no grid characters");
    }

    #[test]
    fn non_table_box_with_a_table_like_border_still_draws_no_grid_chars() {
        // Scope guard: the SAME top+left border shape as the collapsed-cell
        // test above, but on a plain (non-table) box -- must draw nothing,
        // exactly `full_four_side_solid_border_draws_no_tty_rule_regression_
        // guard`'s sibling case for the new grid-lines path specifically.
        use crate::style::computed::Edges;
        let gray = Color::rgb(0x80, 0x80, 0x80);
        let border = Edges { top: solid(gray), left: solid(gray), right: BorderSide::default(), bottom: BorderSide::default() };
        let fragments = vec![table_box_fragment(Display::Block, 0.0, 0.0, 24.0, 16.0, border)];
        let grid = render(&fragments, 10);
        assert_eq!(grid.row_text(0).trim_end(), "", "non-table box must draw no grid characters");
    }

    #[test]
    fn table_grid_lines_do_not_disturb_the_hr_sole_top_rule() {
        // hr's own shape (sole solid top border, non-table display) still
        // goes through `draw_top_border_rule` only -- unaffected by the new
        // table-grid path.
        use crate::style::computed::BorderSide as Side;
        let gray = Color::rgb(0x80, 0x80, 0x80);
        let border = Side { width: 1.0, style: BorderStyle::Solid, color: gray };
        let fragments = vec![box_fragment_top_border(0.0, 0.0, 24.0, 0.0, border)];
        let grid = render(&fragments, 10);
        assert_eq!(grid.row_text(0).trim_end(), "───", "hr's plain rule line must still render, unaffected by table grid lines");
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
