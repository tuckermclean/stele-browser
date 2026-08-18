//! Table layout geometry + totality tests (table-layout packet, M3): the
//! solved grid wired into `layout::layout()` end to end, using hand-built
//! `LayoutNode` trees (real font metrics via `layout::layout`'s own
//! `BitmapFont::vga_8x16`, 8px/char monospace advance at the default 16px
//! font-size — see `text::bitmap`). Assertions are robust
//! (alignment/summed-span geometry) rather than brittle sub-pixel, per the
//! packet brief, except where an exact monospace pixel count is the whole
//! point of the assertion.

use stele::layout::{layout, BoxContent, Fragment, FragmentKind, LayoutNode, Size};
use stele::style::computed::{BorderCollapse, BorderSide, BorderStyle, Dimension, Display, Edges, LengthPercentage};
use stele::style::ComputedStyle;
use stele::surface::Color;

fn styled(display: Display) -> ComputedStyle {
    ComputedStyle { display, ..ComputedStyle::default() }
}

fn text_node(s: &str) -> LayoutNode {
    LayoutNode {
        style: ComputedStyle::default(),
        content: BoxContent::Text(s.to_string()),
        children: Vec::new(),
        interactive: None,
    }
}

fn cell(colspan: u16, rowspan: u16, text: &str) -> LayoutNode {
    LayoutNode {
        style: styled(Display::TableCell),
        content: BoxContent::TableCell { colspan, rowspan },
        children: vec![text_node(text)],
        interactive: None,
    }
}

fn row(cells: Vec<LayoutNode>) -> LayoutNode {
    LayoutNode { style: styled(Display::TableRow), content: BoxContent::Container, children: cells, interactive: None }
}

fn tbody(rows: Vec<LayoutNode>) -> LayoutNode {
    LayoutNode { style: styled(Display::TableRowGroup), content: BoxContent::Container, children: rows, interactive: None }
}

fn table(children: Vec<LayoutNode>) -> LayoutNode {
    LayoutNode { style: styled(Display::Table), content: BoxContent::Container, children, interactive: None }
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
    LayoutNode {
        style: styled(Display::Block),
        content: BoxContent::Container,
        children: vec![table_node],
        interactive: None,
    }
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
            interactive: None,
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
            interactive: None,
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

// ---- packet/table-spacing: `border-spacing` (gap) + cell `padding` -------

/// A table whose own `ComputedStyle.border_spacing_x/y` is overridden away
/// from the default (8.0/0.0) -- the solved gap between two adjacent
/// columns' boxes must reflect the TABLE's own resolved style, not the old
/// hardcoded constant this packet replaces.
fn table_with_spacing(spacing_x: f32, spacing_y: f32, children: Vec<LayoutNode>) -> LayoutNode {
    let mut style = styled(Display::Table);
    style.border_spacing_x = spacing_x;
    style.border_spacing_y = spacing_y;
    LayoutNode { style, content: BoxContent::Container, children, interactive: None }
}

#[test]
fn table_border_spacing_style_controls_the_gap_between_columns() {
    // col0 ("aaaaaaaaaa", 10 chars) = 80px, col1 ("b", 1 char) = 8px -- both
    // unambiguous, distinct from every other box in this tiny tree.
    let t = root_with(table_with_spacing(20.0, 0.0, vec![row(vec![cell(1, 1, "aaaaaaaaaa"), cell(1, 1, "b")])]));
    let fragments = layout(&t, Size { w: 640.0, h: 480.0 });
    assert_all_finite_nonneg(&fragments);
    let boxes = box_fragments(&fragments);
    let col0 = *boxes.iter().find(|bx| (bx.rect.size.w - 80.0).abs() < 0.5).expect("col0 (80px) box present");
    let col1 = *boxes.iter().find(|bx| (bx.rect.size.w - 8.0).abs() < 0.5).expect("col1 (8px) box present");
    let gap = col1.rect.origin.x - (col0.rect.origin.x + col0.rect.size.w);
    assert!((gap - 20.0).abs() < 0.5, "gap {gap} should equal the table's own border_spacing_x (20px), not the old 8px default");
}

/// The flip side of the above: a table with NO explicit `border_spacing_x`
/// override (i.e. `ComputedStyle::default`'s 8.0) must still produce the
/// SAME 8px gap as before this packet -- the "no golden churn" guarantee at
/// the wiring level, not just the cascade-default level already covered by
/// `style::computed`/`style::cascade`'s own tests.
#[test]
fn table_with_default_style_still_uses_8px_border_spacing() {
    let t = root_with(table(vec![row(vec![cell(1, 1, "aaaaaaaaaa"), cell(1, 1, "b")])]));
    let fragments = layout(&t, Size { w: 640.0, h: 480.0 });
    let boxes = box_fragments(&fragments);
    let col0 = *boxes.iter().find(|bx| (bx.rect.size.w - 80.0).abs() < 0.5).expect("col0 (80px) box present");
    let col1 = *boxes.iter().find(|bx| (bx.rect.size.w - 8.0).abs() < 0.5).expect("col1 (8px) box present");
    let gap = col1.rect.origin.x - (col0.rect.origin.x + col0.rect.size.w);
    assert!((gap - 8.0).abs() < 0.5, "gap {gap} should stay the pre-existing 8px default");
}

// ---- packet/border-collapse: collapsed tables feed spacing 0 -------------

fn table_with_spacing_and_collapse(
    spacing_x: f32,
    spacing_y: f32,
    collapse: BorderCollapse,
    children: Vec<LayoutNode>,
) -> LayoutNode {
    let mut style = styled(Display::Table);
    style.border_spacing_x = spacing_x;
    style.border_spacing_y = spacing_y;
    style.border_collapse = collapse;
    LayoutNode { style, content: BoxContent::Container, children, interactive: None }
}

/// A collapsed table ignores its own `border_spacing_x` entirely (CSS
/// `border-collapse: collapse` spec behavior) -- even though this table's
/// style sets a nonzero 20px `border_spacing_x` (same value
/// `table_border_spacing_style_controls_the_gap_between_columns` above
/// proves DOES produce a 20px gap for a `Separate` table), the solved gap
/// between its two adjacent column boxes must be zero: cells sit flush
/// against each other, no inter-cell gap at all.
#[test]
fn collapsed_table_ignores_border_spacing_cells_are_adjacent() {
    let t = root_with(table_with_spacing_and_collapse(
        20.0,
        0.0,
        BorderCollapse::Collapse,
        vec![row(vec![cell(1, 1, "aaaaaaaaaa"), cell(1, 1, "b")])],
    ));
    let fragments = layout(&t, Size { w: 640.0, h: 480.0 });
    assert_all_finite_nonneg(&fragments);
    let boxes = box_fragments(&fragments);
    let col0 = *boxes.iter().find(|bx| (bx.rect.size.w - 80.0).abs() < 0.5).expect("col0 (80px) box present");
    let col1 = *boxes.iter().find(|bx| (bx.rect.size.w - 8.0).abs() < 0.5).expect("col1 (8px) box present");
    let gap = col1.rect.origin.x - (col0.rect.origin.x + col0.rect.size.w);
    assert!((gap - 0.0).abs() < 0.5, "collapsed table should feed 0 border-spacing to the solver, got gap {gap}");
}

/// The flip side: the SAME style (20px `border_spacing_x`) with
/// `border_collapse` left `Separate` still produces the 20px gap -- proves
/// the zeroing above is specifically gated on `Collapse`, not some blanket
/// regression.
#[test]
fn separate_table_with_same_spacing_style_still_shows_the_gap() {
    let t = root_with(table_with_spacing_and_collapse(
        20.0,
        0.0,
        BorderCollapse::Separate,
        vec![row(vec![cell(1, 1, "aaaaaaaaaa"), cell(1, 1, "b")])],
    ));
    let fragments = layout(&t, Size { w: 640.0, h: 480.0 });
    let boxes = box_fragments(&fragments);
    let col0 = *boxes.iter().find(|bx| (bx.rect.size.w - 80.0).abs() < 0.5).expect("col0 (80px) box present");
    let col1 = *boxes.iter().find(|bx| (bx.rect.size.w - 8.0).abs() < 0.5).expect("col1 (8px) box present");
    let gap = col1.rect.origin.x - (col0.rect.origin.x + col0.rect.size.w);
    assert!((gap - 20.0).abs() < 0.5, "separate table should still show its own 20px border_spacing_x, got gap {gap}");
}

/// A single-cell, single-column table (no border-spacing gaps in play at
/// all) built with the cell's own `padding` set directly on its
/// `ComputedStyle` -- proves the measurement pipeline
/// (`cell_min_max_width`/`cell_content_layout`) and `emit`'s content
/// offsetting both honor a cell's `padding`, independent of any box-tree
/// `cellpadding`-attribute stamping (tested separately in `box_tree`).
fn one_cell_table(padding_px: Option<f32>, text: &str) -> LayoutNode {
    let mut cell_style = styled(Display::TableCell);
    if let Some(p) = padding_px {
        cell_style.padding = Edges::all(LengthPercentage::Px(p));
    }
    let cell_node = LayoutNode {
        style: cell_style,
        content: BoxContent::TableCell { colspan: 1, rowspan: 1 },
        children: vec![text_node(text)],
        interactive: None,
    };
    root_with(table(vec![row(vec![cell_node])]))
}

/// Box fragments narrower than the (stretched-to-viewport) root box: in a
/// single-cell single-column table with no border-spacing gaps, the table's
/// own box and the cell's own box are BOTH exactly the solved column width
/// -- this collects just those (excluding the always-viewport-wide root).
fn non_root_boxes<'a>(fragments: &'a [Fragment], viewport_w: f32) -> Vec<&'a Fragment> {
    box_fragments(fragments).into_iter().filter(|bx| bx.rect.size.w < viewport_w - 0.5).collect()
}

#[test]
fn cell_padding_grows_min_max_width_and_intrinsic_height() {
    let unpadded = one_cell_table(None, "hello");
    let padded = one_cell_table(Some(10.0), "hello");

    let frags_u = layout(&unpadded, Size { w: 640.0, h: 480.0 });
    let frags_p = layout(&padded, Size { w: 640.0, h: 480.0 });
    assert_all_finite_nonneg(&frags_u);
    assert_all_finite_nonneg(&frags_p);

    let boxes_u = non_root_boxes(&frags_u, 640.0);
    let boxes_p = non_root_boxes(&frags_p, 640.0);
    assert!(!boxes_u.is_empty() && !boxes_p.is_empty());

    let w_u = boxes_u[0].rect.size.w;
    let h_u = boxes_u[0].rect.size.h;
    for b in &boxes_u {
        assert_eq!(b.rect.size.w, w_u, "table box and cell box coincide (single cell, single column)");
        assert_eq!(b.rect.size.h, h_u);
    }
    let w_p = boxes_p[0].rect.size.w;
    let h_p = boxes_p[0].rect.size.h;
    for b in &boxes_p {
        assert_eq!(b.rect.size.w, w_p);
        assert_eq!(b.rect.size.h, h_p);
    }

    assert!(
        (w_p - (w_u + 20.0)).abs() < 0.5,
        "10px left+right padding should grow the cell's max-content width by 20px: unpadded={w_u} padded={w_p}"
    );
    assert!(
        (h_p - (h_u + 20.0)).abs() < 0.5,
        "10px top+bottom padding should grow the cell's intrinsic height by 20px: unpadded={h_u} padded={h_p}"
    );
}

#[test]
fn cell_padding_offsets_content_from_the_cell_box_origin() {
    let unpadded = one_cell_table(None, "hello");
    let padded = one_cell_table(Some(10.0), "hello");

    let frags_u = layout(&unpadded, Size { w: 640.0, h: 480.0 });
    let frags_p = layout(&padded, Size { w: 640.0, h: 480.0 });

    let box_u = non_root_boxes(&frags_u, 640.0)[0].rect.origin;
    let box_p = non_root_boxes(&frags_p, 640.0)[0].rect.origin;

    let hello_u = *text_fragments(&frags_u).iter().find(|f| text_of(f) == "hello").expect("hello present");
    let hello_p = *text_fragments(&frags_p).iter().find(|f| text_of(f) == "hello").expect("hello present");

    assert!((hello_u.rect.origin.x - box_u.x).abs() < 0.5, "unpadded content sits flush with the cell box");
    assert!((hello_u.rect.origin.y - box_u.y).abs() < 0.5);

    assert!(
        (hello_p.rect.origin.x - (box_p.x + 10.0)).abs() < 0.5,
        "padded content should be inset 10px from the cell box's left edge"
    );
    assert!(
        (hello_p.rect.origin.y - (box_p.y + 10.0)).abs() < 0.5,
        "padded content should be inset 10px from the cell box's top edge"
    );
}

// ---------------------------------------------------------------------------
// packet/collapse-geometry: `border-collapse: collapse` shared grid lines.
//
// Every cell keeps its FULL 4-side border (no dedup) -- the single-line
// effect comes from POSITIONING adjacent cells so their borders coincide on
// the same pixels, per `layout::block`'s own "packet/collapse-geometry" doc
// comments. For a uniform 1px border, the closed-form geometry this proves:
//   - every SINGLE-span column/row keeps its exact separate-mode width/
//     height (only its POSITION shifts) -- adjacent cells intentionally
//     OVERLAP by exactly one border-width (1px here) at each interior grid
//     line, so their independently-painted borders land on the same pixel.
//   - a cell spanning multiple columns/rows is narrower/shorter than the
//     raw summed span by `(span - 1) * border_width` (the interior lines it
//     internally swallows are genuinely gone, not shared with a neighbor).
//   - the table's total collapsed width/height is the raw sum minus
//     `(count - 1) * border_width` (one shared line removed per interior
//     boundary; the true outer edges are untouched at the frameless-table
//     boundary).
// ---------------------------------------------------------------------------

fn bordered_cell(colspan: u16, rowspan: u16, text: &str, border_px: f32) -> LayoutNode {
    let mut style = styled(Display::TableCell);
    style.border = Edges::all(BorderSide { width: border_px, style: BorderStyle::Solid, color: Color::BLACK });
    LayoutNode {
        style,
        content: BoxContent::TableCell { colspan, rowspan },
        children: vec![text_node(text)],
        interactive: None,
    }
}

/// `frame_border_px`: `Some(n)` gives the TABLE's own box a solid `n`px
/// border on all four sides too (the `<table border>` shape -- bug #1 in
/// the packet brief: the frame and the first row/col cells' own borders
/// must coincide, not stack into a doubled edge); `None` leaves the table's
/// own border at the CSS default (the CSS-only-collapsed shape with no
/// table-level border at all -- e.g. kitchen-sink's `table {
/// border-collapse: collapse } td { border: 1px solid }`).
fn collapsed_table(frame_border_px: Option<f32>, children: Vec<LayoutNode>) -> LayoutNode {
    let mut style = styled(Display::Table);
    style.border_collapse = BorderCollapse::Collapse;
    if let Some(bw) = frame_border_px {
        style.border = Edges::all(BorderSide { width: bw, style: BorderStyle::Solid, color: Color::BLACK });
    }
    LayoutNode { style, content: BoxContent::Container, children, interactive: None }
}

/// A collapsed 2x2 table (no table-level border -- the CSS-only-collapsed
/// shape), 1px cell borders, columns pinned to exact known pixel widths by
/// single-word (unsplittable) cell text: col0 = "aaaaaaaaaa" (80px content,
/// 8px/char monospace) + 1px border each side = 82px; col1 = "b" (8px) +
/// 2px border = 10px. Both rows use the SAME text so column widths (the
/// solver's per-column max across every cell in that column) are identical
/// top to bottom, keeping the geometry this test asserts fully closed-form.
#[test]
fn collapsed_single_span_cells_share_one_grid_line_no_table_frame() {
    let t = root_with(collapsed_table(
        None,
        vec![
            row(vec![bordered_cell(1, 1, "aaaaaaaaaa", 1.0), bordered_cell(1, 1, "b", 1.0)]),
            row(vec![bordered_cell(1, 1, "aaaaaaaaaa", 1.0), bordered_cell(1, 1, "b", 1.0)]),
        ],
    ));
    let fragments = layout(&t, Size { w: 640.0, h: 480.0 });
    assert_all_finite_nonneg(&fragments);
    let boxes = box_fragments(&fragments);
    // [0]=root, [1]=table's own box, [2..6]=the four cells in row-major
    // (place_grid) order: r0c0, r0c1, r1c0, r1c1.
    assert_eq!(boxes.len(), 6, "root + table + 4 cell boxes");
    let table_box = boxes[1];
    let r0c0 = boxes[2];
    let r0c1 = boxes[3];
    let r1c0 = boxes[4];
    let r1c1 = boxes[5];

    // No table-level border: the outer frame is formed ENTIRELY by the edge
    // cells' own borders -- col0/row0's own box starts exactly where the
    // table's own (borderless) content box starts, no gap and no gratuitous
    // extra shift.
    assert!((r0c0.rect.origin.x - table_box.rect.origin.x).abs() < 0.01, "left frame: r0c0 flush with table's own box");
    assert!((r0c0.rect.origin.y - table_box.rect.origin.y).abs() < 0.01, "top frame: r0c0 flush with table's own box");

    // Single-span, non-last AND last columns both keep their exact raw
    // (separate-mode-equal) width -- only positions shift to overlap.
    assert!((r0c0.rect.size.w - 82.0).abs() < 0.5, "col0 width unchanged by collapse: {}", r0c0.rect.size.w);
    assert!((r0c1.rect.size.w - 10.0).abs() < 0.5, "col1 width unchanged by collapse: {}", r0c1.rect.size.w);

    // Shared vertical grid line: col0's right edge is exactly 1 border-width
    // AFTER col1's left edge (a deliberate 1px overlap so their
    // independently-painted borders land on the SAME pixel column) --
    // verified for both rows (columns align top to bottom).
    let overlap_row0 = (r0c0.rect.origin.x + r0c0.rect.size.w) - r0c1.rect.origin.x;
    assert!((overlap_row0 - 1.0).abs() < 0.5, "col0/col1 should overlap by exactly the 1px border width, got {overlap_row0}");
    assert!((r0c0.rect.origin.x - r1c0.rect.origin.x).abs() < 0.01, "col0 aligns across rows");
    assert!((r0c1.rect.origin.x - r1c1.rect.origin.x).abs() < 0.01, "col1 aligns across rows");
    let overlap_row1 = (r1c0.rect.origin.x + r1c0.rect.size.w) - r1c1.rect.origin.x;
    assert!((overlap_row1 - overlap_row0).abs() < 0.01, "both rows share the identical grid line");

    // Shared horizontal grid line: row0's bottom edge overlaps row1's top
    // edge by exactly the (1px) border height, same coincidence trick along
    // the other axis.
    let overlap_y_col0 = (r0c0.rect.origin.y + r0c0.rect.size.h) - r1c0.rect.origin.y;
    assert!((overlap_y_col0 - 1.0).abs() < 0.5, "row0/row1 should overlap by exactly the 1px border height, got {overlap_y_col0}");
    let overlap_y_col1 = (r0c1.rect.origin.y + r0c1.rect.size.h) - r1c1.rect.origin.y;
    assert!((overlap_y_col1 - overlap_y_col0).abs() < 0.01, "both columns share the identical horizontal grid line");

    // Total collapsed table width/height: raw sum minus ONE interior
    // border-width (2 columns/rows -> 1 shared interior boundary each).
    let expected_total_w = 82.0 + 10.0 - 1.0;
    let expected_total_h = r0c0.rect.size.h + r1c0.rect.size.h - 1.0;
    assert!((table_box.rect.size.w - expected_total_w).abs() < 0.5, "table width {} vs expected {expected_total_w}", table_box.rect.size.w);
    assert!((table_box.rect.size.h - expected_total_h).abs() < 0.5, "table height {} vs expected {expected_total_h}", table_box.rect.size.h);

    // No gaps: the rightmost/bottommost cells' own far edges reach exactly
    // to the table's own far edges (a complete outer frame, no background-
    // colored gap between the last cell and the table's own box).
    assert!(
        ((r0c1.rect.origin.x + r0c1.rect.size.w) - (table_box.rect.origin.x + table_box.rect.size.w)).abs() < 0.5,
        "right frame: rightmost cell reaches the table's own right edge, no gap"
    );
    assert!(
        ((r1c0.rect.origin.y + r1c0.rect.size.h) - (table_box.rect.origin.y + table_box.rect.size.h)).abs() < 0.5,
        "bottom frame: bottommost cell reaches the table's own bottom edge, no gap"
    );
}

/// A colspan=2 cell spanning BOTH columns of the same 2-column grid as the
/// test above: its collapsed width is the interior-shrunk TOTAL table width
/// (not the raw, unshrunk 92px sum) -- it swallows the one interior grid
/// line entirely (no neighbor to share it with), and spans the full grid
/// exactly like the table's own box.
#[test]
fn collapsed_colspan_cell_spans_the_full_grid_width() {
    let t = root_with(collapsed_table(
        None,
        vec![
            row(vec![bordered_cell(2, 1, "TOTAL", 1.0)]),
            row(vec![bordered_cell(1, 1, "aaaaaaaaaa", 1.0), bordered_cell(1, 1, "b", 1.0)]),
        ],
    ));
    let fragments = layout(&t, Size { w: 640.0, h: 480.0 });
    assert_all_finite_nonneg(&fragments);
    let boxes = box_fragments(&fragments);
    // [0]=root, [1]=table, [2]=the colspan-2 TOTAL cell, [3..5]=row1's cells.
    assert_eq!(boxes.len(), 5, "root + table + colspan cell + 2 baseline cells");
    let table_box = boxes[1];
    let total_cell = boxes[2];
    let r1c0 = boxes[3];

    assert!((total_cell.rect.origin.x - r1c0.rect.origin.x).abs() < 0.01, "colspan cell aligns with col0's left edge");
    // Exact number, not just "matches the table box": raw sum (82 + 10 = 92)
    // minus the ONE interior border-width the colspan cell swallows = 91 --
    // pins the fix precisely rather than merely checking self-consistency
    // (which the pre-fix code would also trivially satisfy, since both
    // values were equally un-shrunk).
    assert!((total_cell.rect.size.w - 91.0).abs() < 0.5, "colspan cell width should be 91 (92 raw - 1 swallowed interior border), got {}", total_cell.rect.size.w);
    assert!(
        (total_cell.rect.size.w - table_box.rect.size.w).abs() < 0.5,
        "colspan cell spans the FULL collapsed table width ({}), not the raw unshrunk sum: got {}",
        table_box.rect.size.w,
        total_cell.rect.size.w
    );
}

/// Bug #1 from the packet brief: a `<table border>`-shaped table (the table
/// box ITSELF also carries a solid border, not just its cells) must not
/// double its top/left (or bottom/right) edge -- the table's own frame and
/// the edge cells' own borders must coincide on the exact same pixels, so
/// this asserts the table's own box and the first/last cell's own box share
/// the same outer corners exactly (not merely close, not offset by the
/// frame's border width).
#[test]
fn collapsed_table_with_own_frame_border_coincides_with_edge_cells() {
    let t = root_with(collapsed_table(
        Some(1.0),
        vec![row(vec![bordered_cell(1, 1, "aaaaaaaaaa", 1.0), bordered_cell(1, 1, "b", 1.0)])],
    ));
    let fragments = layout(&t, Size { w: 640.0, h: 480.0 });
    assert_all_finite_nonneg(&fragments);
    let boxes = box_fragments(&fragments);
    assert_eq!(boxes.len(), 4, "root + table + 2 cells");
    let table_box = boxes[1];
    let r0c0 = boxes[2];
    let r0c1 = boxes[3];

    // Top-left corner: the table's own frame border and cell (0,0)'s own
    // top/left border must coincide exactly -- no 2px doubled edge.
    assert!(
        (r0c0.rect.origin.x - table_box.rect.origin.x).abs() < 0.01,
        "table frame and r0c0's own left edge must coincide exactly, table.x={} r0c0.x={}",
        table_box.rect.origin.x,
        r0c0.rect.origin.x
    );
    assert!(
        (r0c0.rect.origin.y - table_box.rect.origin.y).abs() < 0.01,
        "table frame and r0c0's own top edge must coincide exactly, table.y={} r0c0.y={}",
        table_box.rect.origin.y,
        r0c0.rect.origin.y
    );

    // Bottom-right corner: the LAST cell's own far edge must coincide with
    // the table's own far edge (both x and y -- a single row, so the last
    // cell is both the rightmost AND the bottommost).
    let table_right = table_box.rect.origin.x + table_box.rect.size.w;
    let table_bottom = table_box.rect.origin.y + table_box.rect.size.h;
    let cell_right = r0c1.rect.origin.x + r0c1.rect.size.w;
    let cell_bottom = r0c1.rect.origin.y + r0c1.rect.size.h;
    assert!((cell_right - table_right).abs() < 0.5, "table frame and last cell's own right edge must coincide, table_right={table_right} cell_right={cell_right}");
    assert!((cell_bottom - table_bottom).abs() < 0.5, "table frame and last cell's own bottom edge must coincide, table_bottom={table_bottom} cell_bottom={cell_bottom}");
}

/// Separate mode (the CSS default) must be completely unaffected by this
/// packet's collapse geometry: adjacent cells still sit at their normal
/// (non-overlapping, spacing-separated) positions even when every cell
/// carries the exact same 1px border that triggers the collapse geometry
/// above.
#[test]
fn separate_mode_bordered_cells_do_not_overlap() {
    let mut style = styled(Display::Table);
    style.border_collapse = BorderCollapse::Separate;
    let t = root_with(LayoutNode {
        style,
        content: BoxContent::Container,
        children: vec![row(vec![bordered_cell(1, 1, "aaaaaaaaaa", 1.0), bordered_cell(1, 1, "b", 1.0)])],
        interactive: None,
    });
    let fragments = layout(&t, Size { w: 640.0, h: 480.0 });
    let boxes = box_fragments(&fragments);
    assert_eq!(boxes.len(), 4);
    let r0c0 = boxes[2];
    let r0c1 = boxes[3];
    let gap = r0c1.rect.origin.x - (r0c0.rect.origin.x + r0c0.rect.size.w);
    assert!(gap >= 0.0, "separate mode must never overlap adjacent cells, got gap {gap}");
}

// ---------------------------------------------------------------------------
// packet/acid1-content-box: a table cell keeps `BorderBox` sizing (the
// table solver's own long-standing implicit contract -- `layout::block::
// box_sizing_for`'s own doc comment has the full "why") even though this
// packet switches every OTHER element to CSS-correct `ContentBox` by
// default. A cell with an explicit `width` + padding + border must render
// at EXACTLY that declared width (padding/border eat INTO it, matching
// pre-packet behavior and everything `layout::table`'s column solver
// assumes) -- while a plain (non-table) block box with the identical
// width+padding+border grows PAST its declared width (content-box: padding/
// border add on top), proving the split is real and per-display-type, not
// an accidental global regression toward one model or the other.
// ---------------------------------------------------------------------------

fn sized_bordered_cell(width_px: f32, padding_px: f32, border_px: f32) -> LayoutNode {
    let mut style = styled(Display::TableCell);
    style.width = Dimension::Px(width_px);
    style.padding = Edges::all(LengthPercentage::Px(padding_px));
    style.border = Edges::all(BorderSide { width: border_px, style: BorderStyle::Solid, color: Color::BLACK });
    LayoutNode {
        style,
        content: BoxContent::TableCell { colspan: 1, rowspan: 1 },
        children: vec![text_node("x")],
        interactive: None,
    }
}

fn sized_bordered_block(width_px: f32, padding_px: f32, border_px: f32) -> LayoutNode {
    let mut style = styled(Display::Block);
    style.width = Dimension::Px(width_px);
    style.padding = Edges::all(LengthPercentage::Px(padding_px));
    style.border = Edges::all(BorderSide { width: border_px, style: BorderStyle::Solid, color: Color::BLACK });
    LayoutNode { style, content: BoxContent::Container, children: vec![text_node("x")], interactive: None }
}

#[test]
fn table_cell_with_width_padding_border_keeps_border_box_sizing() {
    // 100px declared width, 10px padding + 5px border on every side --
    // BorderBox: padding+border eat INTO the declared 100px, so the
    // rendered box stays exactly 100px wide (not 100+20+10=130).
    let t = root_with(table(vec![row(vec![sized_bordered_cell(100.0, 10.0, 5.0)])]));
    let fragments = layout(&t, Size { w: 640.0, h: 480.0 });
    let boxes = non_root_boxes(&fragments, 640.0);
    assert!(!boxes.is_empty(), "expected at least one non-root box (the cell/table)");
    for b in &boxes {
        assert_eq!(
            b.rect.size.w, 100.0,
            "a table cell's declared width must stay border-box (padding/border absorbed, not added) -- got {}",
            b.rect.size.w
        );
    }
}

#[test]
fn plain_block_box_with_width_padding_border_uses_content_box_sizing() {
    // Same 100px width, 10px padding, 5px border -- but NOT a table cell.
    // ContentBox (this packet's new default): padding/border add ON TOP of
    // the declared 100px content width, so the rendered box grows to
    // 100 + 2*10 (padding) + 2*5 (border) = 130px.
    let root = LayoutNode {
        style: styled(Display::Block),
        content: BoxContent::Container,
        children: vec![sized_bordered_block(100.0, 10.0, 5.0)],
        interactive: None,
    };
    let fragments = layout(&root, Size { w: 640.0, h: 480.0 });
    // Filter out the (viewport-stretched, 640px-wide) root box the same way
    // `non_root_boxes` does for table geometry above -- this avoids any
    // assumption about paint-order index.
    let boxes = non_root_boxes(&fragments, 640.0);
    assert_eq!(boxes.len(), 1, "expected exactly the one sized child box");
    assert_eq!(
        boxes[0].rect.size.w,
        130.0,
        "a plain block box's declared width must be its CONTENT width under content-box (padding+border add on top) -- got {}",
        boxes[0].rect.size.w
    );
}
