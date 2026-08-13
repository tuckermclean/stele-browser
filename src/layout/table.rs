//! Table column solver (P8): the bespoke two-pass min/max-content automatic
//! table layout algorithm — CSS 2.1 §17.5.2 / HTML 4.01 "auto" table layout.
//! Per charter §158: "tables = a bespoke HTML 4.01 auto-layout pre-pass
//! (two-pass min/max-content column solver + row heights + span placement —
//! the genuinely hard part, written in-house) whose output feeds flex as
//! fixed bases."
//!
//! This module is a STANDALONE pure function over its own abstract input
//! types (`TableSpec`/`CellSpec`). It does not measure text or read the
//! styled DOM — a cell's `min_content`/`max_content`/`intrinsic_height` are
//! supplied already-measured by the caller (the inline engine, eventually).
//! It is **not** wired into `layout()` yet: the frozen `ComputedStyle::Display`
//! has no `Table`/`TableRow`/`TableCell` variants, so box-tree integration is
//! deferred to M3 + a freeze amendment. The only caller of this module today
//! is its own contract test suite below.
//!
//! ## Totality (`panic = "abort"`)
//!
//! `solve_table` never panics, on any input however malformed: zero columns
//! or rows, a cell whose `col`/`row` is out of range, `colspan`/`rowspan` of
//! `0` or absurdly large, NaN/negative/infinite `min_content`/`max_content`/
//! `intrinsic_height`/`available_width`/border-spacing, or cells that overlap
//! each other. See the per-step notes below for exactly how each case is
//! handled; the guiding rule throughout is clamp-or-skip, never unwrap/index.
//!
//! ## Documented scope calls (for the DECISIONS ledger)
//!
//! - **Grid sizing cap:** `spec.columns`/`spec.rows` are clamped to
//!   [`MAX_GRID_DIM`] *before* any `Vec` is sized by them, so a hostile spec
//!   (e.g. `columns: usize::MAX`) can't force a huge/aborting allocation.
//!   Real HTML tables never approach this bound.
//! - **Placement / overlap:** cells carry explicit `col`/`row` (the caller —
//!   eventually the box-tree walk — is responsible for auto-flow placement
//!   honoring previous rows' `rowspan`s). This solver does not build a full
//!   `rows × columns` occupancy grid (that risks an O(columns × rows)
//!   allocation from a hostile spec even after the dimension cap); instead it
//!   tracks the rectangles of already-*accepted* cells and skips (does not
//!   place) any later cell whose span rectangle intersects one already
//!   accepted. A skipped cell contributes nothing to column/row sizing and
//!   gets a zero [`Rect`] (`Rect::default()`) in the output — "clamp/skip
//!   rather than panic" per the packet brief. Cells are otherwise processed
//!   in input order, so the earliest cell claiming a slot wins ties.
//! - **Out-of-range cells:** a cell whose `col >= columns` or `row >= rows`
//!   (after the dimension cap) is skipped the same way (not placed; zero
//!   `Rect` in the output). `colspan`/`rowspan` of `0` is treated as `1` (a
//!   cell always occupies at least its own slot); an oversized span is
//!   clamped to the remaining columns/rows from its origin.
//! - **Column min/max-content spanning distribution:** for a colspan>1 cell
//!   whose `min_content` (resp. `max_content`) exceeds the sum of the
//!   currently-known `min_i` (resp. `max_i`) of its spanned columns, the
//!   excess is distributed across those columns proportionally to their
//!   current `max_i`, or evenly if that weight sum is zero — exactly the
//!   rule the packet brief specifies. Multi-span cells are processed in
//!   ascending `colspan` order (narrower spans settle first, standard CSS
//!   table algorithm ordering), min-distribution fully before max-distribution
//!   (so max-distribution weights aren't polluted by min excess). A final
//!   pass clamps `max_i >= min_i` per column (spanning-cell min excess can
//!   push a column's min above its own singly-spanning max).
//! - **Row height spanning distribution:** for a rowspan>1 cell whose
//!   `intrinsic_height` exceeds the sum of its spanned rows' heights, the
//!   excess is split **evenly** across the spanned rows (the brief only
//!   specifies proportional-to-max weighting for the *column* case; rows have
//!   no analogous "max" concept in this model, so even split is the simplest
//!   deterministic rule — documented here rather than left ambiguous).
//! - **Table width resolution when `available_width >= sum_max`:** columns
//!   are set to their `max_i` and leftover space (`available_width -
//!   sum_max`) is **not** distributed back into the columns (no "fill" /
//!   stretch behavior in v1) — the brief explicitly allows either choice for
//!   v1 and asks it be documented.
//! - **Border spacing:** a fixed gap is inserted *between* adjacent columns
//!   and rows only — `(columns - 1)` gaps of `border_spacing_x`, `(rows - 1)`
//!   gaps of `border_spacing_y`. No spacing is added at the table's outer
//!   edge (real CSS separated-borders model does add outer edge spacing; this
//!   is the "keep it simple" v1 the brief allows).
//! - **Non-finite/negative numeric inputs** (`min_content`, `max_content`,
//!   `intrinsic_height`, `available_width`, `border_spacing_x`,
//!   `border_spacing_y`) are clamped: NaN or infinite becomes `0.0`, negative
//!   becomes `0.0`. If a cell's sanitized `max_content < min_content` (a
//!   malformed cell spec), `max_content` is raised to equal `min_content`.

use crate::layout::{Point, Rect, Size};

/// One already-measured cell: its grid position/span, and its min/max
/// content width (from the inline engine, eventually) and intrinsic height
/// (from its own content's layout). This solver only places and sizes —
/// it never measures text itself.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellSpec {
    pub col: usize,
    pub row: usize,
    pub colspan: usize,
    pub rowspan: usize,
    pub min_content: f32,
    pub max_content: f32,
    pub intrinsic_height: f32,
}

/// A table's abstract shape: declared grid dimensions (a hint the solver
/// clamps and defends against, see [`MAX_GRID_DIM`]), its cells, and layout
/// inputs (available width + separated-borders spacing, simplified per the
/// module doc's "Border spacing" scope call).
#[derive(Debug, Clone)]
pub struct TableSpec {
    pub columns: usize,
    pub rows: usize,
    pub cells: Vec<CellSpec>,
    pub available_width: f32,
    pub border_spacing_x: f32,
    pub border_spacing_y: f32,
}

/// The solved layout: final column widths, final row heights, and each
/// input cell's positioned rect — `cell_rects[i]` corresponds to
/// `spec.cells[i]`; a cell that could not be placed (out of range,
/// degenerate, or overlapping an earlier cell) gets `Rect::default()`
/// (a zero rect at the origin).
#[derive(Debug, Clone, PartialEq)]
pub struct TableLayout {
    pub col_widths: Vec<f32>,
    pub row_heights: Vec<f32>,
    pub cell_rects: Vec<Rect>,
}

/// Hard cap on the grid dimensions this solver will size arrays to,
/// independent of what a (possibly hostile) `TableSpec` declares. Applied
/// *before* any `Vec` is allocated with `columns`/`rows` as its length, so a
/// spec claiming e.g. `columns: usize::MAX` cannot force an aborting
/// allocation. Far beyond any real HTML table.
const MAX_GRID_DIM: usize = 4096;

/// Clamp a layout-space scalar to finite and non-negative: NaN/±infinity and
/// negative values all become `0.0`. Used for every numeric input this
/// solver accepts, so no NaN/negative ever propagates into the arithmetic.
fn sanitize_nonneg(x: f32) -> f32 {
    if !x.is_finite() || x < 0.0 {
        0.0
    } else {
        x
    }
}

/// Solve a table's column widths, row heights, and cell rects per the
/// two-pass min/max-content automatic table layout algorithm (module docs).
/// Total: never panics on any `TableSpec`, however malformed.
pub fn solve_table(_spec: &TableSpec) -> TableLayout {
    todo!("P8 RED: implement the two-pass min/max-content column solver")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(col: usize, row: usize, colspan: usize, rowspan: usize, min: f32, max: f32, h: f32) -> CellSpec {
        CellSpec { col, row, colspan, rowspan, min_content: min, max_content: max, intrinsic_height: h }
    }

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { origin: Point { x, y }, size: Size { w, h } }
    }

    /// Simple 2x2, every cell's min == max (fixed width). Columns resolve to
    /// those widths regardless of `available_width`'s relation to the sums
    /// (min==max collapses every resolution branch to the same answer), and
    /// rects tile with no gaps (`border_spacing` 0).
    #[test]
    fn simple_2x2_fixed_widths_tile_correctly() {
        let spec = TableSpec {
            columns: 2,
            rows: 2,
            cells: vec![
                cell(0, 0, 1, 1, 50.0, 50.0, 20.0), // A
                cell(1, 0, 1, 1, 30.0, 30.0, 25.0), // B
                cell(0, 1, 1, 1, 50.0, 50.0, 10.0), // C
                cell(1, 1, 1, 1, 30.0, 30.0, 40.0), // D
            ],
            available_width: 80.0,
            border_spacing_x: 0.0,
            border_spacing_y: 0.0,
        };
        let out = solve_table(&spec);
        assert_eq!(out.col_widths, vec![50.0, 30.0]);
        assert_eq!(out.row_heights, vec![25.0, 40.0]);
        assert_eq!(
            out.cell_rects,
            vec![rect(0.0, 0.0, 50.0, 25.0), rect(50.0, 0.0, 30.0, 25.0), rect(0.0, 25.0, 50.0, 40.0), rect(50.0, 25.0, 30.0, 40.0)]
        );
    }

    /// available_width < sum_min: every column gets its min, and the table
    /// overflows the available width (correct CSS behavior — not clamped
    /// down to fit).
    #[test]
    fn under_constrained_columns_get_min_and_overflow() {
        let spec = TableSpec {
            columns: 2,
            rows: 1,
            cells: vec![cell(0, 0, 1, 1, 40.0, 80.0, 10.0), cell(1, 0, 1, 1, 60.0, 100.0, 10.0)],
            available_width: 50.0, // sum_min = 100
            border_spacing_x: 0.0,
            border_spacing_y: 0.0,
        };
        let out = solve_table(&spec);
        assert_eq!(out.col_widths, vec![40.0, 60.0]);
    }

    /// available_width > sum_max: every column gets its max; documented v1
    /// choice is NOT to stretch into the leftover space.
    #[test]
    fn over_constrained_columns_get_max_no_fill() {
        let spec = TableSpec {
            columns: 2,
            rows: 1,
            cells: vec![cell(0, 0, 1, 1, 40.0, 80.0, 10.0), cell(1, 0, 1, 1, 60.0, 100.0, 10.0)],
            available_width: 300.0, // sum_max = 180; leftover 120 unallocated
            border_spacing_x: 0.0,
            border_spacing_y: 0.0,
        };
        let out = solve_table(&spec);
        assert_eq!(out.col_widths, vec![80.0, 100.0]);
    }

    /// sum_min < available_width < sum_max: slack distributed proportionally
    /// to each column's flexibility (max - min). Numbers chosen to divide
    /// exactly: range 40 each, t = 0.5.
    #[test]
    fn in_between_distributes_slack_proportionally() {
        let spec = TableSpec {
            columns: 2,
            rows: 1,
            cells: vec![cell(0, 0, 1, 1, 40.0, 80.0, 10.0), cell(1, 0, 1, 1, 60.0, 100.0, 10.0)],
            available_width: 140.0, // sum_min=100, sum_max=180, t=0.5
            border_spacing_x: 0.0,
            border_spacing_y: 0.0,
        };
        let out = solve_table(&spec);
        assert_eq!(out.col_widths, vec![60.0, 80.0]);
    }

    /// A colspan-2 cell whose min_content exceeds the two spanned columns'
    /// summed mins: the excess is distributed proportionally to each
    /// column's current max_i (20 and 60 -> 1:3 split of the 40 excess).
    /// available_width is pinned at sum_min so col_widths == min_i directly.
    #[test]
    fn colspan_min_excess_distributes_proportionally_to_max() {
        let spec = TableSpec {
            columns: 3,
            rows: 1,
            cells: vec![
                cell(0, 0, 1, 1, 5.0, 20.0, 0.0),
                cell(1, 0, 1, 1, 5.0, 60.0, 0.0),
                cell(2, 0, 1, 1, 5.0, 5.0, 0.0),
                cell(0, 0, 2, 1, 50.0, 50.0, 0.0), // spans col0..col2, min=50 vs baseline sum 10
            ],
            available_width: 50.0, // == sum_min (15+35+5), forces col_widths == min_i
            border_spacing_x: 0.0,
            border_spacing_y: 0.0,
        };
        let out = solve_table(&spec);
        assert_eq!(out.col_widths, vec![15.0, 35.0, 5.0]);
    }

    /// A colspan-2 cell whose max_content exceeds the two spanned columns'
    /// summed maxes: the excess distributes proportionally to their current
    /// max_i (5 and 15 -> 1:3 split of the 80 excess). available_width is
    /// pinned above sum_max so col_widths == max_i directly.
    #[test]
    fn colspan_max_excess_distributes_proportionally_to_max() {
        let spec = TableSpec {
            columns: 3,
            rows: 1,
            cells: vec![
                cell(0, 0, 1, 1, 5.0, 5.0, 0.0),
                cell(1, 0, 1, 1, 5.0, 15.0, 0.0),
                cell(2, 0, 1, 1, 1.0, 1.0, 0.0),
                cell(0, 0, 2, 1, 5.0, 100.0, 0.0), // min doesn't force excess (5 <= 10); max does (100 vs 20)
            ],
            available_width: 500.0, // >> sum_max, forces col_widths == max_i
            border_spacing_x: 0.0,
            border_spacing_y: 0.0,
        };
        let out = solve_table(&spec);
        assert_eq!(out.col_widths, vec![25.0, 75.0, 1.0]);
    }

    /// A tall rowspan-2 cell in its own column: row heights account for it,
    /// splitting the excess evenly (documented scope call) across the
    /// spanned rows. The cross-check: the rowspan cell's own rect height
    /// exactly equals its intrinsic_height once the excess is folded back in.
    #[test]
    fn rowspan_distributes_excess_into_row_heights() {
        let spec = TableSpec {
            columns: 2,
            rows: 2,
            cells: vec![
                cell(0, 0, 1, 1, 20.0, 20.0, 10.0), // A
                cell(0, 1, 1, 1, 20.0, 20.0, 5.0),  // B
                cell(1, 0, 1, 2, 30.0, 30.0, 40.0), // C: spans row0..row2
            ],
            available_width: 50.0, // == sum_min == sum_max (20+30)
            border_spacing_x: 0.0,
            border_spacing_y: 0.0,
        };
        let out = solve_table(&spec);
        assert_eq!(out.col_widths, vec![20.0, 30.0]);
        assert_eq!(out.row_heights, vec![22.5, 17.5]);
        assert_eq!(
            out.cell_rects,
            vec![
                rect(0.0, 0.0, 20.0, 22.5),
                rect(0.0, 22.5, 20.0, 17.5),
                rect(20.0, 0.0, 30.0, 40.0), // 22.5 + 17.5 == 40, matches intrinsic_height
            ]
        );
    }

    /// border-spacing adds the right offsets between columns and rows (no
    /// outer edge spacing, per the documented v1 scope call).
    #[test]
    fn border_spacing_offsets_rects() {
        let spec = TableSpec {
            columns: 2,
            rows: 2,
            cells: vec![
                cell(0, 0, 1, 1, 10.0, 10.0, 10.0),
                cell(1, 0, 1, 1, 20.0, 20.0, 10.0),
                cell(0, 1, 1, 1, 10.0, 10.0, 8.0),
                cell(1, 1, 1, 1, 20.0, 20.0, 8.0),
            ],
            available_width: 35.0, // sum_min == 10+20 + 1*5 spacing == 35
            border_spacing_x: 5.0,
            border_spacing_y: 3.0,
        };
        let out = solve_table(&spec);
        assert_eq!(out.col_widths, vec![10.0, 20.0]);
        assert_eq!(out.row_heights, vec![10.0, 8.0]);
        assert_eq!(
            out.cell_rects,
            vec![
                rect(0.0, 0.0, 10.0, 10.0),
                rect(15.0, 0.0, 20.0, 10.0), // x = 10 + 5 spacing
                rect(0.0, 13.0, 10.0, 8.0),  // y = 10 + 3 spacing
                rect(15.0, 13.0, 20.0, 8.0),
            ]
        );
    }

    // ---- Totality: never panic on malformed input ----

    #[test]
    fn zero_columns_never_panics() {
        let spec = TableSpec {
            columns: 0,
            rows: 1,
            cells: vec![cell(0, 0, 1, 1, 10.0, 10.0, 5.0)],
            available_width: 100.0,
            border_spacing_x: 0.0,
            border_spacing_y: 0.0,
        };
        let out = solve_table(&spec);
        assert!(out.col_widths.is_empty());
        assert_eq!(out.row_heights, vec![0.0]);
        assert_eq!(out.cell_rects, vec![Rect::default()]);
    }

    #[test]
    fn zero_rows_never_panics() {
        let spec = TableSpec {
            columns: 1,
            rows: 0,
            cells: vec![cell(0, 0, 1, 1, 10.0, 10.0, 5.0)],
            available_width: 100.0,
            border_spacing_x: 0.0,
            border_spacing_y: 0.0,
        };
        let out = solve_table(&spec);
        assert!(out.row_heights.is_empty());
        assert_eq!(out.col_widths, vec![0.0]);
        assert_eq!(out.cell_rects, vec![Rect::default()]);
    }

    #[test]
    fn out_of_range_col_row_never_panics() {
        let spec = TableSpec {
            columns: 2,
            rows: 2,
            cells: vec![cell(5, 0, 1, 1, 10.0, 10.0, 5.0), cell(0, 9, 1, 1, 10.0, 10.0, 5.0)],
            available_width: 100.0,
            border_spacing_x: 0.0,
            border_spacing_y: 0.0,
        };
        let out = solve_table(&spec);
        assert_eq!(out.col_widths.len(), 2);
        assert_eq!(out.row_heights.len(), 2);
        assert_eq!(out.cell_rects, vec![Rect::default(), Rect::default()]);
    }

    /// colspan 0 clamps to 1; colspan 999 clamps to the remaining columns
    /// from its origin (spans col1..col3 in a 3-column table). No panic, and
    /// the resulting rect exactly covers both spanned columns.
    #[test]
    fn degenerate_colspans_clamp_not_panic() {
        let spec = TableSpec {
            columns: 3,
            rows: 1,
            cells: vec![
                cell(0, 0, 0, 1, 10.0, 10.0, 5.0),   // colspan 0 -> 1
                cell(1, 0, 999, 1, 40.0, 40.0, 5.0), // colspan 999 -> clamped to 2
            ],
            available_width: 50.0, // == sum_min == sum_max (10+20+20)
            border_spacing_x: 0.0,
            border_spacing_y: 0.0,
        };
        let out = solve_table(&spec);
        assert_eq!(out.col_widths, vec![10.0, 20.0, 20.0]);
        assert_eq!(out.cell_rects[1], rect(10.0, 0.0, 40.0, 5.0));
    }

    /// NaN/negative/infinite inputs everywhere they can appear: never panics,
    /// and every numeric output is finite and non-negative.
    #[test]
    fn nan_and_negative_inputs_never_panic_and_stay_finite() {
        let spec = TableSpec {
            columns: 2,
            rows: 1,
            cells: vec![
                cell(0, 0, 1, 1, f32::NAN, -10.0, f32::INFINITY),
                cell(1, 0, 1, 1, 5.0, 3.0, 2.0), // max < min: max gets raised to min
            ],
            available_width: f32::NAN,
            border_spacing_x: -5.0,
            border_spacing_y: f32::NEG_INFINITY,
        };
        let out = solve_table(&spec);
        assert!(out.col_widths.iter().all(|w| w.is_finite() && *w >= 0.0));
        assert!(out.row_heights.iter().all(|h| h.is_finite() && *h >= 0.0));
        for r in &out.cell_rects {
            assert!(r.origin.x.is_finite() && r.origin.x >= 0.0);
            assert!(r.origin.y.is_finite() && r.origin.y >= 0.0);
            assert!(r.size.w.is_finite() && r.size.w >= 0.0);
            assert!(r.size.h.is_finite() && r.size.h >= 0.0);
        }
    }

    /// Two cells claim the exact same slot: the second (later, in input
    /// order) is skipped rather than corrupting the grid; it gets a zero rect.
    #[test]
    fn overlapping_cells_skip_the_later_one() {
        let spec = TableSpec {
            columns: 2,
            rows: 1,
            cells: vec![cell(0, 0, 1, 1, 10.0, 10.0, 5.0), cell(0, 0, 1, 1, 99.0, 99.0, 99.0)],
            available_width: 10.0, // == sum_min from the first cell alone
            border_spacing_x: 0.0,
            border_spacing_y: 0.0,
        };
        let out = solve_table(&spec);
        assert_eq!(out.cell_rects[0], rect(0.0, 0.0, 10.0, 5.0));
        assert_eq!(out.cell_rects[1], Rect::default());
    }

    #[test]
    fn empty_cells_never_panics() {
        let spec = TableSpec {
            columns: 2,
            rows: 2,
            cells: vec![],
            available_width: 100.0,
            border_spacing_x: 0.0,
            border_spacing_y: 0.0,
        };
        let out = solve_table(&spec);
        assert_eq!(out.col_widths, vec![0.0, 0.0]);
        assert_eq!(out.row_heights, vec![0.0, 0.0]);
        assert!(out.cell_rects.is_empty());
    }
}
