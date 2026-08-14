//! HTML table auto-placement (table-layout packet, M3): walk a `<table>`'s
//! `LayoutNode` subtree (rows, optionally wrapped in transparent row-groups)
//! and assign each cell its `(col, row)` grid origin + clamped `colspan`/
//! `rowspan`, per the HTML "table auto-placement" algorithm (CSS 2.1 §17
//! informative appendix / HTML5 §4.9.11). This is a pure placement pass: it
//! does not measure content or call [`crate::layout::table::solve_table`] —
//! `layout::block` builds the [`table::CellSpec`](crate::layout::table::CellSpec)s
//! from this grid's output and drives the solver.
//!
//! ## Totality (`panic = "abort"`)
//!
//! Never panics: an empty table, a row with no cells, a row-group with no
//! rows, stray non-cell children mixed into a row (skipped), and absurd
//! `colspan`/`rowspan` (up to `u16::MAX`) are all handled by clamping or
//! skipping, mirroring `solve_table`'s own "clamp-or-skip, never
//! unwrap/index" rule. See [`MAX_GRID_DIM`]/[`MAX_GRID_CELLS`] for the
//! bounds that keep a hostile document (thousands of rows, or one cell with
//! a huge span) from doing unbounded work before `solve_table` even gets a
//! chance to apply its own caps.

use crate::layout::{BoxContent, LayoutNode};
use crate::style::computed::Display;

/// Mirrors `layout::table::solve_table`'s own per-dimension cap: a hostile
/// document (e.g. tens of thousands of sibling `<tr>`s, or one cell with
/// `colspan="65535"`) must not make this placement pass do unbounded work.
/// Far beyond any real HTML table.
const MAX_GRID_DIM: usize = 4096;

/// Mirrors `layout::table::solve_table`'s own grid-area cap: bounds the
/// occupied-slot bookkeeping (and the work of marking a single huge-span
/// cell's slots) to a small fixed cost regardless of how a hostile document
/// chooses its spans.
const MAX_GRID_CELLS: usize = 262_144;

/// One placed cell: its resolved grid origin/span (already clamped to the
/// grid), and a reference back to the source `TableCell` [`LayoutNode`] so
/// the caller can measure its content.
pub struct GridCell<'a> {
    pub col: usize,
    pub row: usize,
    pub colspan: usize,
    pub rowspan: usize,
    pub node: &'a LayoutNode,
}

/// The placed grid: overall dimensions plus every successfully-placed cell,
/// in document order.
pub struct Grid<'a> {
    pub columns: usize,
    pub rows: usize,
    pub cells: Vec<GridCell<'a>>,
}

/// Place every cell of `table`'s row (optionally row-group-wrapped)
/// descendants into a grid. `table` itself need not actually be
/// `display: table` — this function only looks at its children's shape
/// (`TableRow`/`TableRowGroup`/`TableCell`), so it degrades harmlessly (an
/// empty grid) on any node that isn't really a table.
///
/// Total: never panics, on any input however malformed (see module docs).
pub fn place_grid(table: &LayoutNode) -> Grid<'_> {
    let mut row_nodes: Vec<&LayoutNode> = Vec::new();
    collect_rows(table, &mut row_nodes);

    let mut cells: Vec<GridCell> = Vec::new();
    let mut occupied: std::collections::HashSet<(usize, usize)> = std::collections::HashSet::new();
    let mut columns = 0usize;
    let mut row_idx = 0usize;
    let mut cell_budget = MAX_GRID_CELLS;

    for row_node in row_nodes {
        if row_idx >= MAX_GRID_DIM {
            break;
        }
        let mut next_col = 0usize;
        for cell_node in row_children_cells(row_node) {
            if cell_budget == 0 {
                break;
            }
            let BoxContent::TableCell { colspan, rowspan } = &cell_node.content else {
                continue; // unreachable given row_children_cells' filter, but never trust it twice.
            };

            // Advance past any column occupied by an earlier row's rowspan.
            let mut scan_guard = 0usize;
            while next_col < MAX_GRID_DIM && occupied.contains(&(next_col, row_idx)) {
                next_col += 1;
                scan_guard += 1;
                if scan_guard > MAX_GRID_DIM {
                    break;
                }
            }
            if next_col >= MAX_GRID_DIM {
                break; // no room left in this row; drop the remaining cells.
            }

            let colspan = (*colspan as usize).max(1).min(MAX_GRID_DIM - next_col);
            let rowspan = (*rowspan as usize).max(1).min(MAX_GRID_DIM - row_idx);

            'mark: for r in row_idx..row_idx + rowspan {
                for c in next_col..next_col + colspan {
                    if cell_budget == 0 {
                        break 'mark;
                    }
                    cell_budget -= 1;
                    occupied.insert((c, r));
                }
            }

            cells.push(GridCell { col: next_col, row: row_idx, colspan, rowspan, node: cell_node });
            columns = columns.max(next_col + colspan);
            next_col += colspan;
        }
        row_idx += 1;
    }

    Grid { columns, rows: row_idx, cells }
}

/// Collect every `TableRow` descendant of `table`, one level deep, flattening
/// a `TableRowGroup` (`<thead>`/`<tbody>`/`<tfoot>`) child transparently (its
/// own `TableRow` children are collected, in order; anything else under a
/// row-group is ignored). A bare `TableRow` child of `table` itself (no
/// group wrapper — also valid HTML) is collected directly. Any other child
/// (stray text, a `<caption>`, ...) is skipped.
fn collect_rows<'a>(table: &'a LayoutNode, out: &mut Vec<&'a LayoutNode>) {
    for child in &table.children {
        match child.style.display {
            Display::TableRow => out.push(child),
            Display::TableRowGroup => {
                for grandchild in &child.children {
                    if grandchild.style.display == Display::TableRow {
                        out.push(grandchild);
                    }
                }
            }
            _ => {}
        }
    }
}

/// The `TableCell` children of a row, in document order; anything else
/// (stray text, a misplaced element) is skipped.
fn row_children_cells(row: &LayoutNode) -> impl Iterator<Item = &LayoutNode> {
    row.children.iter().filter(|c| matches!(c.content, BoxContent::TableCell { .. }))
}
