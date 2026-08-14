//! Table layout geometry + totality tests (table-layout packet, M3): the
//! solved grid wired into `layout::layout()` end to end, using hand-built
//! `LayoutNode` trees (real font metrics via `layout::layout`'s own
//! `BitmapFont::vga_8x16`, 8px/char monospace advance at the default 16px
//! font-size — see `text::bitmap`). Assertions are robust
//! (alignment/summed-span geometry) rather than brittle sub-pixel, per the
//! packet brief, except where an exact monospace pixel count is the whole
//! point of the assertion.

use stele::layout::{layout, BoxContent, Fragment, FragmentKind, LayoutNode, Size};
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

fn tbody(rows: Vec<LayoutNode>) -> LayoutNode {
    LayoutNode { style: styled(Display::TableRowGroup), content: BoxContent::Container, children: rows }
}

fn table(children: Vec<LayoutNode>) -> LayoutNode {
    LayoutNode { style: styled(Display::Table), content: BoxContent::Container, children }
}

/// Wrap a table in a plain block container, matching a real document's
/// shape (`<body><table>...</table></body>` — a table is a normal
/// block-level child, not itself the document root). Geometry assertions
/// use this rather than passing the bare table as `layout()`'s root: the
/// root box is unconditionally viewport-stretched (see `block::layout_tree`'s
/// "root itself is stretched to the viewport width" step, a UA-stylesheet-
/// less-`<html>` documented behavior) — stretching a *table's own* box that
/// way would fix its outer size at the full viewport width regardless of
/// its solved content width, which is correct only when a table really is
/// the root (an edge case `layout_table`'s totality tests exercise on
/// purpose) and would otherwise make geometry assertions about the table's
/// OWN box ambiguous with the (wider) stretched viewport box.
fn root_with(table_node: LayoutNode) -> LayoutNode {
    LayoutNode { style: styled(Display::Block), content: BoxContent::Container, children: vec![table_node] }
}

fn box_fragments(fragments: &[Fragment]) -> Vec<&Fragment> {
    fragments.iter().filter(|f| matches!(f.kind, FragmentKind::Box { .. })).collect()
}

fn text_fragments(fragments: &[Fragment]) -> Vec<&Fragment> {
    fragments.iter().filter(|f| matches!(f.kind, FragmentKind::Text { .. })).collect()
}

fn text_of(f: &Fragment) -> &str {
    match &f.kind {
        FragmentKind::Text { text, .. } => text.as_str(),
        _ => panic!("not a text fragment"),
    }
}

fn assert_all_finite_nonneg(fragments: &[Fragment]) {
    for f in fragments {
        assert!(f.rect.origin.x.is_finite() && f.rect.origin.y.is_finite(), "{:?}", f.rect);
        assert!(f.rect.size.w.is_finite() && f.rect.size.h.is_finite(), "{:?}", f.rect);
        assert!(f.rect.size.w >= 0.0 && f.rect.size.h >= 0.0, "{:?}", f.rect);
    }
}

/// A plain 2x2 table with fixed-length text cells: columns align across
/// rows (same x for col0 across both rows; same x for col1 across both
/// rows), and rows stack (row1 sits below row0).
#[test]
fn plain_2x2_table_columns_align_across_rows() {
    let t = root_with(table(vec![
        row(vec![cell(1, 1, "aa"), cell(1, 1, "bbbb")]),
        row(vec![cell(1, 1, "c"), cell(1, 1, "d")]),
    ]));
    let fragments = layout(&t, Size { w: 640.0, h: 480.0 });
    assert_all_finite_nonneg(&fragments);

    let texts = text_fragments(&fragments);
    let aa = *texts.iter().find(|f| text_of(f) == "aa").expect("aa present");
    let bbbb = *texts.iter().find(|f| text_of(f) == "bbbb").expect("bbbb present");
    let c = *texts.iter().find(|f| text_of(f) == "c").expect("c present");
    let d = *texts.iter().find(|f| text_of(f) == "d").expect("d present");

    // Column alignment: col0's cells share an x origin, col1's cells share
    // an x origin, and col1 sits to the right of col0.
    assert_eq!(aa.rect.origin.x, c.rect.origin.x, "col0 aligns across rows");
    assert_eq!(bbbb.rect.origin.x, d.rect.origin.x, "col1 aligns across rows");
    assert!(bbbb.rect.origin.x > aa.rect.origin.x, "col1 right of col0");

    // Row stacking: row1 sits below row0.
    assert!(c.rect.origin.y > aa.rect.origin.y, "row1 below row0");
    assert!(d.rect.origin.y > bbbb.rect.origin.y, "row1 below row0");
    assert_eq!(c.rect.origin.y, d.rect.origin.y, "row1's two cells share a y origin");
}

/// A colspan=2 cell's Box fragment spans the summed width of its two
/// columns. Cell text is chosen so every column width is fixed by an
/// unsplittable single word (min == max, no proportional-distribution
/// arithmetic in play — that math is `layout::table::solve_table`'s own
/// unit-tested territory; this integration test only needs to prove the
/// WIRING): col0's width is pinned by "aaaaaaaaaa" (80px, 8px/char
/// monospace), col1's by "b" (8px) -- both distinct from every other box's
/// width in this tree, so they're found unambiguously. The header's own
/// text ("TOTAL", 40px) is well under col0+col1 (88px), so it needs no
/// excess-width distribution and the columns stay exactly at their
/// row-1-derived widths.
#[test]
fn colspan_cell_box_spans_summed_column_width() {
    let t = root_with(table(vec![
        row(vec![cell(2, 1, "TOTAL")]),
        row(vec![cell(1, 1, "aaaaaaaaaa"), cell(1, 1, "b")]),
    ]));
    let fragments = layout(&t, Size { w: 640.0, h: 480.0 });
    assert_all_finite_nonneg(&fragments);
    let boxes = box_fragments(&fragments);

    let col0_box = *boxes.iter().find(|bx| (bx.rect.size.w - 80.0).abs() < 0.5).expect("col0 (80px) box present");
    let col1_box = *boxes.iter().find(|bx| (bx.rect.size.w - 8.0).abs() < 0.5).expect("col1 (8px) box present");
    // A colspan-2 cell's rect also includes the one border-spacing gap
    // *between* its two spanned columns (`layout::table::solve_table`'s own
    // `cell_rects` formula: span width = summed column widths + (colspan-1)
    // gaps) -- `block::BORDER_SPACING_X` (private to that module) is `8.0`
    // px, mirrored here rather than imported.
    const BORDER_SPACING_X: f32 = 8.0;
    let summed = col0_box.rect.size.w + col1_box.rect.size.w + BORDER_SPACING_X;

    // The header cell's box and the table's own outer box are BOTH exactly
    // `summed` wide (the table shrink-wraps to its content) -- disambiguate
    // by height: the header only covers row 0 (one line tall), while the
    // table's own box covers both rows (two lines tall, since col0/col1's
    // cells are unambiguously one line each here).
    let row_h = col0_box.rect.size.h;
    assert_eq!(col1_box.rect.size.h, row_h, "col0/col1 share a single-line row height");
    let header_box = boxes
        .iter()
        .find(|bx| (bx.rect.size.w - summed).abs() < 0.5 && (bx.rect.size.h - row_h).abs() < 0.5)
        .expect("spanning header box (one row tall, summed width) present");
    let table_box = boxes
        .iter()
        .find(|bx| (bx.rect.size.w - summed).abs() < 0.5 && bx.rect.size.h > row_h + 0.5)
        .expect("table's own box (two rows tall, summed width) present");

    assert!((header_box.rect.size.w - summed).abs() < 0.01, "header {} vs summed {}", header_box.rect.size.w, summed);
    assert!((table_box.rect.size.h - 2.0 * row_h).abs() < 0.5, "table height should be two row heights");
}

/// A rowspan=2 cell's Box fragment spans the summed height of the two rows
/// it covers.
#[test]
fn rowspan_cell_box_spans_summed_row_height() {
    // col0 ("TALL", 4 chars) is pinned to 32px by the rowspan cell itself
    // (the only col0 cell). col1 is pinned to 48px by "BOTTOM" (6 chars) --
    // "TOP" (3 chars, 24px) is narrower so contributes no baseline excess.
    // 32 is unique across every box in this tree (col1's two cells share
    // 48px, the table/root are wider still), so it unambiguously identifies
    // the rowspan cell's own box.
    let t = root_with(table(vec![
        row(vec![cell(1, 2, "TALL"), cell(1, 1, "TOP")]),
        row(vec![cell(1, 1, "BOTTOM")]),
    ]));
    let fragments = layout(&t, Size { w: 640.0, h: 480.0 });
    assert_all_finite_nonneg(&fragments);
    let boxes = box_fragments(&fragments);

    let tall_box = *boxes.iter().find(|bx| (bx.rect.size.w - 32.0).abs() < 0.5).expect("tall (32px) box present");
    let col1_boxes: Vec<&Fragment> = boxes.iter().filter(|bx| (bx.rect.size.w - 48.0).abs() < 0.5).copied().collect();
    assert_eq!(col1_boxes.len(), 2, "expected exactly the top and bottom col1 cell boxes at 48px");
    let (top_box, bottom_box) = if col1_boxes[0].rect.origin.y < col1_boxes[1].rect.origin.y {
        (col1_boxes[0], col1_boxes[1])
    } else {
        (col1_boxes[1], col1_boxes[0])
    };
    assert!(top_box.rect.origin.y < bottom_box.rect.origin.y, "top above bottom");

    let summed = top_box.rect.size.h + bottom_box.rect.size.h;
    assert!((tall_box.rect.size.h - summed).abs() < 0.01, "tall {} vs summed {}", tall_box.rect.size.h, summed);
}

// ---- Totality: never panic on any table, however malformed ----

#[test]
fn empty_table_never_panics() {
    let t = table(vec![]);
    let fragments = layout(&t, Size { w: 640.0, h: 480.0 });
    assert_all_finite_nonneg(&fragments);
}

#[test]
fn table_with_empty_row_never_panics() {
    let t = table(vec![row(vec![])]);
    let fragments = layout(&t, Size { w: 640.0, h: 480.0 });
    assert_all_finite_nonneg(&fragments);
}

#[test]
fn ragged_rows_never_panic() {
    let t = table(vec![
        row(vec![cell(1, 1, "a"), cell(1, 1, "b"), cell(1, 1, "c")]),
        row(vec![cell(1, 1, "d")]),
    ]);
    let fragments = layout(&t, Size { w: 640.0, h: 480.0 });
    assert_all_finite_nonneg(&fragments);
}

#[test]
fn huge_spans_never_panic() {
    let t = table(vec![
        row(vec![cell(u16::MAX, u16::MAX, "huge")]),
        row(vec![cell(1, 1, "b")]),
    ]);
    let fragments = layout(&t, Size { w: 640.0, h: 480.0 });
    assert_all_finite_nonneg(&fragments);
}

/// A table nested inside a cell, nested inside another table, several
/// levels deep: must degrade gracefully (bounded recursion) rather than
/// stack-overflowing the process.
#[test]
fn deeply_nested_tables_do_not_abort() {
    // Build a table nested 20 levels deep inside cells -- comfortably past
    // any sane table-in-table nesting bound.
    let mut inner = table(vec![row(vec![cell(1, 1, "leaf")])]);
    for _ in 0..20 {
        let wrapping_cell = LayoutNode {
            style: styled(Display::TableCell),
            content: BoxContent::TableCell { colspan: 1, rowspan: 1 },
            children: vec![inner],
        };
        inner = table(vec![row(vec![wrapping_cell])]);
    }
    let fragments = layout(&inner, Size { w: 640.0, h: 480.0 });
    assert_all_finite_nonneg(&fragments);
}

/// Same nested-table bomb, but wide/deep enough to catch a regression that
/// only degrades gracefully at shallow depths.
#[test]
fn very_deeply_nested_tables_do_not_abort() {
    let mut inner = table(vec![row(vec![cell(1, 1, "leaf")])]);
    for _ in 0..200 {
        let wrapping_cell = LayoutNode {
            style: styled(Display::TableCell),
            content: BoxContent::TableCell { colspan: 1, rowspan: 1 },
            children: vec![inner],
        };
        inner = table(vec![row(vec![wrapping_cell])]);
    }
    let fragments = layout(&inner, Size { w: 640.0, h: 480.0 });
    assert_all_finite_nonneg(&fragments);
}

/// A table with a row directly under the table (no row-group wrapper) mixed
/// with a row-group sibling -- both are valid HTML, both must place.
#[test]
fn bare_row_and_row_group_sibling_both_place() {
    let t = table(vec![row(vec![cell(1, 1, "bare")]), tbody(vec![row(vec![cell(1, 1, "grouped")])])]);
    let fragments = layout(&t, Size { w: 640.0, h: 480.0 });
    assert_all_finite_nonneg(&fragments);
    let texts = text_fragments(&fragments);
    assert!(texts.iter().any(|f| text_of(f) == "bare"));
    assert!(texts.iter().any(|f| text_of(f) == "grouped"));
}

// ---- Stress: wide tables (thousands of cells) must not hang (Critical C1) ----
//
// Mirrors `layout::table::solve_table`'s own
// `huge_cell_count_places_promptly_and_stays_1to1` test, but through the
// REAL pipeline (`layout::layout`, not just `place_grid`): each cell's
// min/max-content width and content-at-solved-width are measured via a
// fresh taffy sub-layout (`cell_min_max_width`/`cell_content_layout` in
// `layout::block`), which is orders of magnitude more expensive per cell
// than `place_grid`'s own pure-arithmetic placement. A table with
// thousands of plain `<td>`s (a large spreadsheet export -- not exotic,
// not adversarial) must still return promptly: either because it's small
// enough to measure for real, or because it's large enough to trip
// `block::MAX_TABLE_MEASURED_CELLS` and degrade to plain stacked blocks
// (still total, still bounded, just not "real" table geometry).
fn wide_table(cols: usize, rows_n: usize) -> LayoutNode {
    let rows: Vec<LayoutNode> =
        (0..rows_n).map(|_| row((0..cols).map(|_| cell(1, 1, "x")).collect())).collect();
    root_with(table(rows))
}

/// A table just past `block::MAX_TABLE_MEASURED_CELLS` (2_000): comfortably
/// larger than any real 1996-era data table (hundreds of cells), but still
/// modest by "hostile input" standards -- exactly the shape the coordinator
/// flagged (tens of thousands of `<td>`s is even worse; this is the
/// smallest case that must already degrade). Must return in well under the
/// minutes-long hang this used to take, with every fragment still finite.
#[test]
fn wide_table_past_measurement_cap_degrades_promptly() {
    let t = wide_table(50, 60); // 3_000 cells
    let start = std::time::Instant::now();
    let fragments = layout(&t, Size { w: 640.0, h: 480.0 });
    let elapsed = start.elapsed();
    assert_all_finite_nonneg(&fragments);
    assert!(elapsed.as_secs() < 10, "3_000-cell table took {elapsed:?} -- expected a prompt block-fallback degrade");
    // Degraded to block fallback: every cell's text still renders (nothing
    // silently dropped), just not through real table-grid geometry -- a
    // block-fallback cell's own box collapses to near-zero width (an
    // over-cap table is never given `item_is_table`/measured sizing), so
    // cross-checking even a handful of x/y positions against real solved
    // column math isn't meaningful here; presence + promptness is the bar.
    let texts = text_fragments(&fragments);
    assert_eq!(texts.len(), 3_000, "every cell's text fragment still renders under the block fallback");
}

/// A realistic large-but-real data table (hundreds of cells, comfortably
/// under the cap): still measured for real (real column solve, real
/// per-cell taffy sub-layouts), and must still complete promptly -- this is
/// the case the per-cell measurement CACHING fix (reusing one solve between
/// `measure_node` and `emit` rather than recomputing it) is aimed at.
#[test]
fn large_real_table_under_cap_still_completes_promptly() {
    let t = wide_table(20, 40); // 800 cells, under the 2_000 cap
    let start = std::time::Instant::now();
    let fragments = layout(&t, Size { w: 2000.0, h: 4000.0 });
    let elapsed = start.elapsed();
    assert_all_finite_nonneg(&fragments);
    assert!(elapsed.as_secs() < 10, "800-cell table took {elapsed:?} -- expected the caching fix to keep this fast");
    let texts = text_fragments(&fragments);
    assert_eq!(texts.len(), 800);
}
