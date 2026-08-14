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
    let t = table(vec![
        row(vec![cell(1, 1, "aa"), cell(1, 1, "bbbb")]),
        row(vec![cell(1, 1, "c"), cell(1, 1, "d")]),
    ]);
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

/// A colspan=2 cell's Box fragment spans the summed width of its two columns
/// (matching the two single-column cells below it in the next row).
#[test]
fn colspan_cell_box_spans_summed_column_width() {
    let t = table(vec![
        row(vec![cell(2, 1, "spanning header")]),
        row(vec![cell(1, 1, "aaaaaaaaaa"), cell(1, 1, "b")]),
    ]);
    let fragments = layout(&t, Size { w: 640.0, h: 480.0 });
    assert_all_finite_nonneg(&fragments);
    let boxes = box_fragments(&fragments);

    // Find the two narrow row-1 cell boxes by width ordering: the wider one
    // (10 chars) and the narrow one (1 char) -- table box + header box +
    // two cell boxes. We instead locate via text fragments' rects and match
    // widths against box fragments containing them.
    let texts = text_fragments(&fragments);
    let a = *texts.iter().find(|f| text_of(f) == "aaaaaaaaaa").expect("present");
    let b = *texts.iter().find(|f| text_of(f) == "b").expect("present");
    let header = *texts.iter().find(|f| text_of(f) == "spanning" || text_of(f).contains("spanning")).expect("present");

    // The spanning header cell's box should be exactly as wide as
    // col0_width + col1_width (i.e. reach at least to where col1 ends).
    // Find the two single-column cell boxes (smallest boxes containing the
    // narrow text) by rect matches.
    let col0_box = boxes
        .iter()
        .find(|bx| bx.rect.origin.x <= a.rect.origin.x && bx.rect.origin.x + bx.rect.size.w >= a.rect.origin.x + a.rect.size.w && bx.rect.origin.y > header.rect.origin.y)
        .expect("col0 cell box present");
    let col1_box = boxes
        .iter()
        .find(|bx| bx.rect.origin.x <= b.rect.origin.x && bx.rect.origin.x + bx.rect.size.w >= b.rect.origin.x + b.rect.size.w && bx.rect.origin.y > header.rect.origin.y && bx.rect.origin.x > col0_box.rect.origin.x)
        .expect("col1 cell box present");

    let header_box = boxes
        .iter()
        .find(|bx| bx.rect.origin.x <= header.rect.origin.x && bx.rect.origin.y <= header.rect.origin.y && bx.rect.size.w > col0_box.rect.size.w && bx.rect.size.w > col1_box.rect.size.w)
        .expect("spanning header box present");

    let summed = col0_box.rect.size.w + col1_box.rect.size.w;
    assert!((header_box.rect.size.w - summed).abs() < 0.01, "header {} vs summed {}", header_box.rect.size.w, summed);
}

/// A rowspan=2 cell's Box fragment spans the summed height of the two rows
/// it covers.
#[test]
fn rowspan_cell_box_spans_summed_row_height() {
    let t = table(vec![
        row(vec![cell(1, 2, "tall"), cell(1, 1, "top")]),
        row(vec![cell(1, 1, "bottom")]),
    ]);
    let fragments = layout(&t, Size { w: 640.0, h: 480.0 });
    assert_all_finite_nonneg(&fragments);
    let boxes = box_fragments(&fragments);
    let texts = text_fragments(&fragments);

    let tall = *texts.iter().find(|f| text_of(f) == "tall").expect("present");
    let top = *texts.iter().find(|f| text_of(f) == "top").expect("present");
    let bottom = *texts.iter().find(|f| text_of(f) == "bottom").expect("present");

    let top_box = boxes
        .iter()
        .find(|bx| bx.rect.origin.y <= top.rect.origin.y && bx.rect.origin.x <= top.rect.origin.x && bx.rect.size.w < 200.0 && bx.rect.origin.x > tall.rect.origin.x - 1.0)
        .expect("top cell box present");
    let bottom_box = boxes
        .iter()
        .find(|bx| bx.rect.origin.y > top_box.rect.origin.y && bx.rect.origin.x <= bottom.rect.origin.x)
        .expect("bottom cell box present");
    let tall_box = boxes
        .iter()
        .find(|bx| bx.rect.origin.x <= tall.rect.origin.x && bx.rect.size.h > top_box.rect.size.h)
        .expect("tall (rowspan) cell box present");

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
