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
//! It is **not** wired into `layout()` yet: a display-table freeze amendment
//! has since landed `Display::Table`/`TableRow`/`TableCell`/`TableRowGroup`
//! as markers (parsed from CSS + the UA sheet), but every consumer —
//! including `layout::block::map_display` — still falls back to
//! block-equivalent behavior. Real box-tree integration with this solver is
//! deferred to the next (table-layout) packet. The only caller of this
//! module today is its own contract test suite below.
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
//!   The grid *area* is further clamped to [`MAX_GRID_CELLS`] (reducing
//!   `rows` if needed) so the occupancy bitset placement uses (see next
//!   bullet) stays a bounded allocation too. Real HTML tables never approach
//!   either bound.
//! - **Placement / overlap:** cells carry explicit `col`/`row` (the caller —
//!   eventually the box-tree walk — is responsible for auto-flow placement
//!   honoring previous rows' `rowspan`s). Placement uses a `Vec<bool>`
//!   occupied-slot bitset sized to the (capped) grid area rather than an
//!   O(n²) scan against every previously-accepted cell: a hostile spec with
//!   a huge `cells.len()` (nothing caps that field directly) must not be
//!   able to hang the process. Beyond the bitset, a shared `placement_budget`
//!   (initialized to the grid area) is decremented once per occupied-slot
//!   *read* across the whole placement pass; once it hits zero, all
//!   remaining/in-progress cells are treated as unplaced. This bounds total
//!   placement work to O(grid area) — a small constant — regardless of
//!   `cells.len()` or how an adversary chooses cell spans/origins to
//!   maximize wasted scan-before-reject work. A cell that can't be placed
//!   (out of range, overlapping, or the budget ran out) contributes nothing
//!   to column/row sizing and gets a zero [`Rect`] (`Rect::default()`) in
//!   the output — "clamp/skip rather than panic" per the packet brief.
//!   Cells are otherwise processed in input order, so the earliest cell
//!   claiming a slot wins ties.
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
//! - **Output scrub:** inputs are sanitized, but the arithmetic in between
//!   (summing many columns/spans, dividing by a weight sum) can still
//!   overflow to `inf` or combine to `NaN` for sufficiently extreme
//!   (near-`f32::MAX`) inputs — not reachable from real measured content, but
//!   this module's whole premise is totality. As a final step every output
//!   float (`col_widths`, `row_heights`, and each `Rect`'s `x`/`y`/`w`/`h`)
//!   is passed back through the same finite-non-negative clamp as the
//!   inputs, so the "no NaN/inf ever leaks out" claim above is airtight, not
//!   just true for the cases this module happens to reason through.

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

/// Hard cap on each grid dimension this solver will size arrays to,
/// independent of what a (possibly hostile) `TableSpec` declares. Applied
/// *before* any `Vec` is allocated with `columns`/`rows` as its length, so a
/// spec claiming e.g. `columns: usize::MAX` cannot force an aborting
/// allocation. Far beyond any real HTML table.
const MAX_GRID_DIM: usize = 4096;

/// Hard cap on the grid *area* (`columns * rows`) this solver will size the
/// placement occupancy bitset to — applied on top of [`MAX_GRID_DIM`], since
/// two dimensions each near that per-dimension cap could otherwise multiply
/// to a large allocation (4096 * 4096 ≈ 16M bools). 512 × 512 (or any
/// equivalent split), far beyond any real HTML table, keeps the bitset (and
/// the placement work it bounds, see the module doc's "Placement / overlap"
/// scope call) a small, fixed-size cost.
const MAX_GRID_CELLS: usize = 262_144;

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

/// A sanitized, successfully-placed cell (grid-clamped span, finite/non-neg
/// measurements). Cells that were skipped (out of range, or overlapping an
/// already-accepted cell) have no `Placed` entry.
struct Placed {
    col: usize,
    row: usize,
    colspan: usize,
    rowspan: usize,
    min_content: f32,
    max_content: f32,
    intrinsic_height: f32,
}

/// Solve a table's column widths, row heights, and cell rects per the
/// two-pass min/max-content automatic table layout algorithm (module docs).
/// Total: never panics on any `TableSpec`, however malformed.
pub fn solve_table(spec: &TableSpec) -> TableLayout {
    let columns = spec.columns.min(MAX_GRID_DIM);
    // Clamp the grid *area* too (not just each dimension) so the occupancy
    // bitset below is a bounded allocation even when both dimensions are
    // independently near MAX_GRID_DIM.
    let rows = if columns > 0 { spec.rows.min(MAX_GRID_DIM).min(MAX_GRID_CELLS / columns) } else { spec.rows.min(MAX_GRID_DIM) };
    let spacing_x = sanitize_nonneg(spec.border_spacing_x);
    let spacing_y = sanitize_nonneg(spec.border_spacing_y);
    let available_width = sanitize_nonneg(spec.available_width);

    // --- Grid placement: sanitize, clamp spans, skip out-of-range/overlap ---
    let mut placed: Vec<Option<Placed>> = Vec::with_capacity(spec.cells.len());
    // Occupied-slot bitset (bounded to `columns * rows <= MAX_GRID_CELLS`)
    // replaces an O(n²) scan against every previously-accepted cell — see
    // the module doc's "Placement / overlap" scope call. `placement_budget`
    // caps the total number of slot *reads* performed across the whole
    // placement pass (not just per cell), so total placement work is
    // O(grid area) regardless of `cells.len()` or adversarial span choices.
    let grid_cells = columns * rows;
    let mut occupied = vec![false; grid_cells];
    let mut placement_budget = grid_cells;

    for c in &spec.cells {
        if columns == 0 || rows == 0 || c.col >= columns || c.row >= rows || placement_budget == 0 {
            placed.push(None);
            continue;
        }
        let colspan = c.colspan.max(1).min(columns - c.col);
        let rowspan = c.rowspan.max(1).min(rows - c.row);

        let mut overlaps = false;
        'scan: for r in c.row..c.row + rowspan {
            for cc in c.col..c.col + colspan {
                if placement_budget == 0 {
                    // Budget exhausted mid-scan: can't confirm this cell is
                    // conflict-free, so treat it conservatively as unplaced
                    // rather than risk placing over an as-yet-unchecked slot.
                    overlaps = true;
                    break 'scan;
                }
                placement_budget -= 1;
                if occupied[r * columns + cc] {
                    overlaps = true;
                    break 'scan;
                }
            }
        }
        if overlaps {
            placed.push(None);
            continue;
        }
        for r in c.row..c.row + rowspan {
            for cc in c.col..c.col + colspan {
                occupied[r * columns + cc] = true;
            }
        }

        let min_content = sanitize_nonneg(c.min_content);
        let mut max_content = sanitize_nonneg(c.max_content);
        if max_content < min_content {
            max_content = min_content;
        }
        let intrinsic_height = sanitize_nonneg(c.intrinsic_height);

        placed.push(Some(Placed { col: c.col, row: c.row, colspan, rowspan, min_content, max_content, intrinsic_height }));
    }

    // --- Column min/max-content widths ---
    let mut min_col = vec![0.0f32; columns];
    let mut max_col = vec![0.0f32; columns];
    for p in placed.iter().flatten() {
        if p.colspan == 1 {
            min_col[p.col] = min_col[p.col].max(p.min_content);
            max_col[p.col] = max_col[p.col].max(p.max_content);
        }
    }

    let mut multi_span: Vec<&Placed> = placed.iter().flatten().filter(|p| p.colspan > 1).collect();
    multi_span.sort_by_key(|p| p.colspan);

    // Min-content excess distribution, proportional to current max_i (even
    // split if that weight sum is zero). Fully before max-distribution so
    // max weights aren't polluted by min excess.
    for p in &multi_span {
        let start = p.col;
        let end = p.col + p.colspan;
        let sum_min: f32 = min_col[start..end].iter().sum();
        let excess = p.min_content - sum_min;
        if excess > 0.0 {
            let weight_sum: f32 = max_col[start..end].iter().sum();
            let n = p.colspan as f32;
            for c in start..end {
                let add = if weight_sum > 0.0 { excess * (max_col[c] / weight_sum) } else { excess / n };
                min_col[c] += add;
            }
        }
    }
    // Max-content excess distribution, proportional to current max_i.
    for p in &multi_span {
        let start = p.col;
        let end = p.col + p.colspan;
        let sum_max: f32 = max_col[start..end].iter().sum();
        let excess = p.max_content - sum_max;
        if excess > 0.0 {
            let weight_sum = sum_max; // weight is the same array being grown
            let n = p.colspan as f32;
            for c in start..end {
                let add = if weight_sum > 0.0 { excess * (max_col[c] / weight_sum) } else { excess / n };
                max_col[c] += add;
            }
        }
    }
    // A spanning cell's min excess can push a column's min above its own
    // (singly-spanning-derived) max; keep the invariant max_i >= min_i.
    for c in 0..columns {
        if max_col[c] < min_col[c] {
            max_col[c] = min_col[c];
        }
    }

    // --- Table width resolution ---
    let col_gaps = columns.saturating_sub(1) as f32;
    let sum_min: f32 = min_col.iter().sum::<f32>() + col_gaps * spacing_x;
    let sum_max: f32 = max_col.iter().sum::<f32>() + col_gaps * spacing_x;

    let col_widths: Vec<f32> = if available_width <= sum_min {
        min_col.clone()
    } else if available_width >= sum_max {
        max_col.clone()
    } else {
        let range = sum_max - sum_min;
        if range <= 0.0 {
            min_col.clone()
        } else {
            let t = (available_width - sum_min) / range;
            min_col.iter().zip(max_col.iter()).map(|(&mn, &mx)| mn + (mx - mn) * t).collect()
        }
    };

    // --- Row heights ---
    let mut row_heights = vec![0.0f32; rows];
    for p in placed.iter().flatten() {
        if p.rowspan == 1 {
            row_heights[p.row] = row_heights[p.row].max(p.intrinsic_height);
        }
    }
    let mut multi_row: Vec<&Placed> = placed.iter().flatten().filter(|p| p.rowspan > 1).collect();
    multi_row.sort_by_key(|p| p.rowspan);
    for p in &multi_row {
        let start = p.row;
        let end = p.row + p.rowspan;
        let sum_h: f32 = row_heights[start..end].iter().sum();
        let excess = p.intrinsic_height - sum_h;
        if excess > 0.0 {
            let add = excess / p.rowspan as f32;
            for r in start..end {
                row_heights[r] += add;
            }
        }
    }

    // --- Cell rects: prefix-sum column x-offsets / row y-offsets ---
    let mut col_x = vec![0.0f32; columns];
    {
        let mut acc = 0.0f32;
        for (i, x) in col_x.iter_mut().enumerate() {
            *x = acc;
            acc += col_widths[i] + spacing_x;
        }
    }
    let mut row_y = vec![0.0f32; rows];
    {
        let mut acc = 0.0f32;
        for (i, y) in row_y.iter_mut().enumerate() {
            *y = acc;
            acc += row_heights[i] + spacing_y;
        }
    }

    let mut cell_rects = Vec::with_capacity(spec.cells.len());
    for p_opt in &placed {
        match p_opt {
            None => cell_rects.push(Rect::default()),
            Some(p) => {
                let x = col_x[p.col];
                let y = row_y[p.row];
                let w = col_widths[p.col..p.col + p.colspan].iter().sum::<f32>()
                    + p.colspan.saturating_sub(1) as f32 * spacing_x;
                let h = row_heights[p.row..p.row + p.rowspan].iter().sum::<f32>()
                    + p.rowspan.saturating_sub(1) as f32 * spacing_y;
                cell_rects.push(Rect { origin: Point { x, y }, size: Size { w, h } });
            }
        }
    }

    // --- Output scrub: force every output float finite & non-negative ---
    // Inputs are sanitized, but summation/division on extreme (near
    // f32::MAX) sanitized values can still overflow to inf or combine to
    // NaN (module doc's "Output scrub" scope call). This is the final
    // totality backstop, independent of whatever arithmetic happened above.
    let col_widths: Vec<f32> = col_widths.into_iter().map(sanitize_nonneg).collect();
    let row_heights: Vec<f32> = row_heights.into_iter().map(sanitize_nonneg).collect();
    let cell_rects: Vec<Rect> = cell_rects
        .into_iter()
        .map(|r| Rect {
            origin: Point { x: sanitize_nonneg(r.origin.x), y: sanitize_nonneg(r.origin.y) },
            size: Size { w: sanitize_nonneg(r.size.w), h: sanitize_nonneg(r.size.h) },
        })
        .collect();

    TableLayout { col_widths, row_heights, cell_rects }
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
            rows: 2,
            cells: vec![
                // Baseline (single-column) cells establish col0/col1/col2's
                // min/max in row 0.
                cell(0, 0, 1, 1, 5.0, 20.0, 0.0),
                cell(1, 0, 1, 1, 5.0, 60.0, 0.0),
                cell(2, 0, 1, 1, 5.0, 5.0, 0.0),
                // A colspan-2 cell in row 1 (so it doesn't overlap the row-0
                // singles) spans col0..col2, min=50 vs baseline sum 10.
                cell(0, 1, 2, 1, 50.0, 50.0, 0.0),
            ],
            available_width: 0.0, // <= sum_min always, forces col_widths == min_i
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
            rows: 2,
            cells: vec![
                // Baseline (single-column) cells in row 0.
                cell(0, 0, 1, 1, 5.0, 5.0, 0.0),
                cell(1, 0, 1, 1, 5.0, 15.0, 0.0),
                cell(2, 0, 1, 1, 1.0, 1.0, 0.0),
                // Colspan-2 cell in row 1 (no overlap with the row-0
                // singles): min doesn't force excess (5 <= 10); max does
                // (100 vs 20).
                cell(0, 1, 2, 1, 5.0, 100.0, 0.0),
            ],
            available_width: f32::MAX, // >= sum_max always, forces col_widths == max_i
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

    /// A hostile spec with a huge `cells.len()` (nothing caps that field
    /// directly — only `columns`/`rows` are dimension-capped) must not hang
    /// the process: placement is bounded to O(grid area) regardless of how
    /// many cells are offered (see `placement_budget` in `solve_table`).
    /// This is a correctness + promptness regression test for the O(n²)
    /// rectangle-scan DoS the coordinator flagged: 100_000 single-slot cells
    /// used to mean 100_000 * accepted.len() rectangle-intersection checks.
    #[test]
    fn huge_cell_count_places_promptly_and_stays_1to1() {
        const N: usize = 100_000;
        // A 10-column table; 10 * (N/10 + 1) rows comfortably exceeds the
        // grid-area cap, so most of these cells land beyond the placement
        // budget once the (capped) grid fills up — exercising the "beyond
        // cap" degrade path, not just the happy path.
        let columns = 10;
        let rows = N / columns + 1;
        let mut cells = Vec::with_capacity(N);
        for i in 0..N {
            cells.push(cell(i % columns, i / columns, 1, 1, 1.0, 1.0, 1.0));
        }
        let spec = TableSpec { columns, rows, cells, available_width: 1000.0, border_spacing_x: 0.0, border_spacing_y: 0.0 };
        let out = solve_table(&spec);
        // 1:1 correspondence with the input, regardless of how many placed.
        assert_eq!(out.cell_rects.len(), N);
        // Every rect is either the placed (nonzero) tile or the skipped
        // (zero) default — never a panic, never a partial/garbage value.
        for r in &out.cell_rects {
            assert!(r.origin.x.is_finite() && r.origin.y.is_finite());
            assert!(r.size.w.is_finite() && r.size.h.is_finite());
        }
        // At least the cells within the grid-area cap were placed.
        assert!(out.cell_rects.iter().any(|r| *r != Rect::default()));
    }

    /// The reviewer's constructed case: a colspan-2 cell's min-excess
    /// distribution can push a column's min above what the (separate)
    /// max-excess distribution alone would give that column — without the
    /// final `max_i >= min_i` clamp, resolving to `max_i` (available_width
    /// >= sum_max) would then give a NARROWER width than the column's own
    /// min_content demands. col0: min-path -> ~14.92, max-path alone ->
    /// ~9.95; the clamp must win, raising col0's resolved max (and thus its
    /// resolved width here) to ~14.92. col1 needs no clamping (its min-path
    /// and max-path don't conflict) — asserted as a control.
    #[test]
    fn colspan_min_excess_clamps_max_when_it_would_undercut_min() {
        let spec = TableSpec {
            columns: 2,
            rows: 2,
            cells: vec![
                cell(0, 0, 1, 1, 5.0, 5.0, 0.0),
                cell(1, 0, 1, 1, 1.0, 1000.0, 0.0),
                cell(0, 1, 2, 1, 2000.0, 2000.0, 0.0),
            ],
            available_width: f32::MAX, // forces col_widths == max_i (post-clamp)
            border_spacing_x: 0.0,
            border_spacing_y: 0.0,
        };
        let out = solve_table(&spec);
        assert!((out.col_widths[0] - 14.920_398).abs() < 0.05, "col0 = {}", out.col_widths[0]);
        assert!((out.col_widths[1] - 1990.049_75).abs() < 0.05, "col1 = {}", out.col_widths[1]);
    }

    /// Extreme (near-f32::MAX) magnitudes can overflow to `inf` inside the
    /// arithmetic (e.g. summing two f32::MAX-sized column widths across a
    /// colspan) even though every input was already sanitized. The final
    /// output-scrub pass must catch it: every output stays finite.
    #[test]
    fn extreme_magnitudes_scrub_to_finite_output() {
        let spec = TableSpec {
            columns: 2,
            rows: 2,
            cells: vec![
                cell(0, 0, 1, 1, f32::MAX, f32::MAX, 0.0),
                cell(1, 0, 1, 1, f32::MAX, f32::MAX, 0.0),
                // Spans both f32::MAX-wide columns: col_widths[0] +
                // col_widths[1] overflows to +inf inside the rect-width sum.
                cell(0, 1, 2, 1, 1.0, 1.0, 0.0),
            ],
            available_width: 100.0, // <= sum_min (inf): forces col_widths == min_col (still finite individually)
            border_spacing_x: 0.0,
            border_spacing_y: 0.0,
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
        // Pin the actual overflow site: the spanning cell's width would be
        // +inf pre-scrub (f32::MAX + f32::MAX); scrubbed, it's exactly 0.0.
        assert_eq!(out.cell_rects[2].size.w, 0.0);
    }

    /// rowspan analog of `degenerate_colspans_clamp_not_panic`: rowspan 0
    /// clamps to 1; rowspan 999 clamps to the remaining rows from its
    /// origin. No panic, and the resulting rect exactly covers both spanned
    /// rows.
    #[test]
    fn degenerate_rowspans_clamp_not_panic() {
        let spec = TableSpec {
            columns: 1,
            rows: 3,
            cells: vec![
                cell(0, 0, 1, 0, 10.0, 10.0, 5.0),    // rowspan 0 -> 1
                cell(0, 1, 1, 999, 10.0, 10.0, 40.0), // rowspan 999 -> clamped to 2 (rows 1..3)
            ],
            available_width: 10.0, // == sum_min == sum_max (single column, min==max==10)
            border_spacing_x: 0.0,
            border_spacing_y: 0.0,
        };
        let out = solve_table(&spec);
        assert_eq!(out.row_heights, vec![5.0, 20.0, 20.0]);
        assert_eq!(out.cell_rects[1], rect(0.0, 5.0, 10.0, 40.0));
    }

    /// The realistic `<td colspan=2 rowspan=2>` case: one cell spans both
    /// two columns and two rows in the corner of a 3x3 grid; the column and
    /// row solves are decoupled, so this should just compose. Values chosen
    /// so no proportional distribution is needed (the spanning cell's
    /// min/max/height exactly match the sum of its spanned columns'/rows'
    /// baselines) — this test is purely about placement/tiling geometry.
    #[test]
    fn combined_colspan_and_rowspan_tiles_correctly() {
        let spec = TableSpec {
            columns: 3,
            rows: 3,
            cells: vec![
                cell(0, 0, 2, 2, 20.0, 20.0, 20.0), // big: spans col0..2, row0..2
                cell(2, 0, 1, 1, 10.0, 10.0, 10.0),
                cell(2, 1, 1, 1, 10.0, 10.0, 10.0),
                cell(0, 2, 1, 1, 10.0, 10.0, 10.0),
                cell(1, 2, 1, 1, 10.0, 10.0, 10.0),
                cell(2, 2, 1, 1, 10.0, 10.0, 10.0),
            ],
            available_width: 30.0, // == sum_min == sum_max (10+10+10)
            border_spacing_x: 0.0,
            border_spacing_y: 0.0,
        };
        let out = solve_table(&spec);
        assert_eq!(out.col_widths, vec![10.0, 10.0, 10.0]);
        assert_eq!(out.row_heights, vec![10.0, 10.0, 10.0]);
        assert_eq!(
            out.cell_rects,
            vec![
                rect(0.0, 0.0, 20.0, 20.0),
                rect(20.0, 0.0, 10.0, 10.0),
                rect(20.0, 10.0, 10.0, 10.0),
                rect(0.0, 20.0, 10.0, 10.0),
                rect(10.0, 20.0, 10.0, 10.0),
                rect(20.0, 20.0, 10.0, 10.0),
            ]
        );
    }
}
