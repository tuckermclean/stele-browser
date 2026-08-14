//! Grid auto-placement tests (table-layout packet, M3): the HTML table
//! auto-placement algorithm implemented by `layout::table_layout::place_grid`
//! over hand-built `LayoutNode` trees — no parsing/cascade involved (that's
//! the golden/integration tests). Exercises row-group flattening, colspan,
//! rowspan, ragged rows, and totality on hostile/degenerate input.

use stele::layout::table_layout::place_grid;
use stele::layout::{BoxContent, LayoutNode};
use stele::style::computed::Display;
use stele::style::ComputedStyle;

fn styled(display: Display) -> ComputedStyle {
    ComputedStyle { display, ..ComputedStyle::default() }
}

fn text_node(s: &str) -> LayoutNode {
    LayoutNode { style: ComputedStyle::default(), content: BoxContent::Text(s.to_string()), children: Vec::new() }
}

fn cell(colspan: u16, rowspan: u16, text: &str) -> LayoutNode {
    LayoutNode {
        style: styled(Display::TableCell),
        content: BoxContent::TableCell { colspan, rowspan },
        children: vec![text_node(text)],
    }
}

fn row(cells: Vec<LayoutNode>) -> LayoutNode {
    LayoutNode { style: styled(Display::TableRow), content: BoxContent::Container, children: cells }
}

fn row_group(rows: Vec<LayoutNode>) -> LayoutNode {
    LayoutNode { style: styled(Display::TableRowGroup), content: BoxContent::Container, children: rows }
}

fn table(children: Vec<LayoutNode>) -> LayoutNode {
    LayoutNode { style: styled(Display::Table), content: BoxContent::Container, children }
}

/// A plain 2x2 table (no groups, no spans): cells land exactly where their
/// row/column order says.
#[test]
fn plain_2x2_places_cells_at_expected_positions() {
    let t = table(vec![
        row(vec![cell(1, 1, "a"), cell(1, 1, "b")]),
        row(vec![cell(1, 1, "c"), cell(1, 1, "d")]),
    ]);
    let grid = place_grid(&t);
    assert_eq!(grid.columns, 2);
    assert_eq!(grid.rows, 2);
    assert_eq!(grid.cells.len(), 4);

    let positions: Vec<(usize, usize)> = grid.cells.iter().map(|c| (c.col, c.row)).collect();
    assert_eq!(positions, vec![(0, 0), (1, 0), (0, 1), (1, 1)]);
    for c in &grid.cells {
        assert_eq!((c.colspan, c.rowspan), (1, 1));
    }
}

/// `<thead>`/`<tbody>` (TableRowGroup) are transparent: their rows flatten
/// into the same grid, continuing the row index across the group boundary.
#[test]
fn header_and_body_row_groups_flatten_into_one_grid() {
    let t = table(vec![
        row_group(vec![row(vec![cell(1, 1, "h1"), cell(1, 1, "h2")])]),
        row_group(vec![
            row(vec![cell(1, 1, "b1"), cell(1, 1, "b2")]),
            row(vec![cell(1, 1, "b3"), cell(1, 1, "b4")]),
        ]),
    ]);
    let grid = place_grid(&t);
    assert_eq!(grid.columns, 2);
    assert_eq!(grid.rows, 3, "header row + two body rows = 3 total rows");
    let positions: Vec<(usize, usize)> = grid.cells.iter().map(|c| (c.col, c.row)).collect();
    assert_eq!(positions, vec![(0, 0), (1, 0), (0, 1), (1, 1), (0, 2), (1, 2)]);
}

/// A colspan=2 cell occupies two columns; the next cell in the row starts
/// after it.
#[test]
fn colspan_cell_occupies_multiple_columns() {
    let t = table(vec![row(vec![cell(2, 1, "wide"), cell(1, 1, "narrow")])]);
    let grid = place_grid(&t);
    assert_eq!(grid.columns, 3);
    assert_eq!(grid.rows, 1);
    assert_eq!(grid.cells[0].col, 0);
    assert_eq!(grid.cells[0].colspan, 2);
    assert_eq!(grid.cells[1].col, 2, "second cell starts after the colspan-2 cell");
}

/// A rowspan=2 cell in row 0 reserves its column in row 1: row 1's cell
/// lands in the next free column, not column 0.
#[test]
fn rowspan_cell_reserves_column_in_next_row() {
    let t = table(vec![
        row(vec![cell(1, 2, "tall"), cell(1, 1, "b")]),
        row(vec![cell(1, 1, "c")]),
    ]);
    let grid = place_grid(&t);
    assert_eq!(grid.columns, 2);
    assert_eq!(grid.rows, 2);
    // cells: [tall(0,0) rowspan2, b(1,0), c(?,1)]
    assert_eq!((grid.cells[0].col, grid.cells[0].row, grid.cells[0].rowspan), (0, 0, 2));
    assert_eq!((grid.cells[1].col, grid.cells[1].row), (1, 0));
    assert_eq!((grid.cells[2].col, grid.cells[2].row), (1, 1), "row1's cell must skip col0 (occupied by the rowspan)");
}

/// Ragged rows (different cell counts per row) place without panicking; grid
/// width is the max column extent across all rows.
#[test]
fn ragged_rows_place_without_panicking() {
    let t = table(vec![
        row(vec![cell(1, 1, "a"), cell(1, 1, "b"), cell(1, 1, "c")]),
        row(vec![cell(1, 1, "d")]),
    ]);
    let grid = place_grid(&t);
    assert_eq!(grid.columns, 3);
    assert_eq!(grid.rows, 2);
    assert_eq!(grid.cells.len(), 4);
    assert_eq!((grid.cells[3].col, grid.cells[3].row), (0, 1));
}

/// An empty table (no rows) never panics and yields an empty grid.
#[test]
fn empty_table_never_panics() {
    let t = table(vec![]);
    let grid = place_grid(&t);
    assert_eq!(grid.columns, 0);
    assert_eq!(grid.rows, 0);
    assert!(grid.cells.is_empty());
}

/// A row with no cells (e.g. `<tr></tr>`) never panics.
#[test]
fn empty_row_never_panics() {
    let t = table(vec![row(vec![]), row(vec![cell(1, 1, "a")])]);
    let grid = place_grid(&t);
    assert_eq!(grid.rows, 2);
    assert_eq!(grid.cells.len(), 1);
    assert_eq!((grid.cells[0].col, grid.cells[0].row), (0, 1));
}

/// Absurd colspan/rowspan (near u16::MAX) clamp to the grid rather than
/// blowing up allocation or hanging.
#[test]
fn huge_spans_clamp_and_never_panic() {
    let t = table(vec![
        row(vec![cell(u16::MAX, u16::MAX, "huge")]),
        row(vec![cell(1, 1, "b")]),
    ]);
    let grid = place_grid(&t);
    // Must terminate and produce a bounded grid, not attempt a
    // u16::MAX-wide/tall allocation.
    assert!(grid.columns > 0 && grid.columns < 100_000);
    assert!(grid.rows >= 2 && grid.rows < 100_000);
    assert_eq!(grid.cells.len(), 2);
}

/// A large number of rows (well past any real HTML document) completes
/// promptly and caps at a bounded grid dimension rather than hanging.
#[test]
fn many_rows_completes_promptly_and_stays_bounded() {
    let rows: Vec<LayoutNode> = (0..20_000).map(|_| row(vec![cell(1, 1, "x")])).collect();
    let t = table(rows);
    let grid = place_grid(&t);
    assert!(grid.rows <= 4096, "row count must be capped, got {}", grid.rows);
}

/// Two cells that would overlap due to a bogus explicit structure (not
/// reachable via normal colspan/rowspan math, but the placement algorithm
/// must still not panic on a table containing a non-cell child interleaved
/// with real cells, e.g. stray text between `<td>`s).
#[test]
fn stray_non_cell_children_in_a_row_are_skipped() {
    let mut r = row(vec![cell(1, 1, "a")]);
    r.children.push(text_node("stray"));
    r.children.push(cell(1, 1, "b"));
    let t = table(vec![r]);
    let grid = place_grid(&t);
    assert_eq!(grid.cells.len(), 2);
    assert_eq!((grid.cells[0].col, grid.cells[1].col), (0, 1));
}
