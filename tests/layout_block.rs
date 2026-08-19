//! Block-flow geometry tests (P6, M2): exact `Rect`s for nested block boxes
//! translated onto taffy's degenerate-column-flex substrate, plus replaced
//! sizing and totality on degenerate input. Hand-built `LayoutNode` trees —
//! no parsing/cascade involved (that's `layout_integration.rs`).

use std::rc::Rc;

use stele::img::RgbaImage;
use stele::layout::{layout, BoxContent, Fragment, FragmentKind, LayoutNode, Size};
use stele::style::computed::{
    BorderSide, BorderStyle, Dimension, Display, Edges, FlexDirection, Float, GridRepetitionCount,
    GridTemplateComponent, GridTrack, GridTrackSize, LengthPercentage, LengthPercentageAuto, Position,
};
use stele::style::ComputedStyle;

/// A `display: block` style with everything else at CSS initial values.
fn block_style() -> ComputedStyle {
    ComputedStyle { display: stele::style::computed::Display::Block, ..ComputedStyle::default() }
}

/// A `display: flex; flex-direction: row` style (the CSS default direction)
/// with the given `gap`, everything else at CSS initial values.
fn flex_row_style(gap: f32) -> ComputedStyle {
    ComputedStyle {
        display: stele::style::computed::Display::Flex,
        flex_direction: FlexDirection::Row,
        gap,
        ..ComputedStyle::default()
    }
}

fn flex_item_style(width: Option<f32>, flex_grow: f32) -> ComputedStyle {
    let mut style = block_style();
    if let Some(w) = width {
        style.width = Dimension::Px(w);
    }
    style.flex_grow = flex_grow;
    style
}

/// packet/css-grid: a `display: grid` style with the given
/// `grid-template-columns` and (single-value) `gap`, everything else at CSS
/// initial values -- mirrors `flex_row_style`'s own shape.
fn grid_style(grid_template_columns: Vec<GridTemplateComponent>, gap: f32) -> ComputedStyle {
    ComputedStyle { display: Display::Grid, grid_template_columns, gap, ..ComputedStyle::default() }
}

/// Wrap `inner` in a plain `display: block` ancestor, exactly like every
/// real document's grid content sits inside `<body>` (block, UA default).
/// This matters for grid specifically (not just cosmetically): taffy's own
/// auto-repeat column-count math (`repeat(auto-fill, ...)`/`repeat(auto-
/// fit, ...)`) requires the grid container to have a DEFINITE known width
/// at layout time -- true automatically for an ordinary in-flow block
/// child (taffy's block algorithm stretch-sizes an auto-width child to the
/// container's own content width before laying it out), but NOT
/// automatically true for a grid container sitting at the very tree root
/// (taffy's `compute_root_layout` only special-cases `Display::Block`/
/// `FlowRoot` roots to derive a definite width from the available viewport
/// space -- a bare `Display::Grid` root gets no such treatment and would
/// see an INDEFINITE width, degrading `repeat(auto-fill/auto-fit, ...)` to
/// a single repetition regardless of viewport width). Every grid test
/// below goes through this wrapper so it measures the real, production
/// code path rather than tripping over that root-only edge case.
fn page(inner: LayoutNode) -> LayoutNode {
    container(block_style(), vec![inner])
}

fn text_node(s: &str) -> LayoutNode {
    LayoutNode {
        style: ComputedStyle::default(),
        content: BoxContent::Text(s.to_string()),
        children: Vec::new(),
        interactive: None,
    }
}

fn px_margin(top: f32, right: f32, bottom: f32, left: f32) -> Edges<LengthPercentageAuto> {
    Edges {
        top: LengthPercentageAuto::Px(top),
        right: LengthPercentageAuto::Px(right),
        bottom: LengthPercentageAuto::Px(bottom),
        left: LengthPercentageAuto::Px(left),
    }
}

fn px_padding_all(v: f32) -> Edges<LengthPercentage> {
    Edges::all(LengthPercentage::Px(v))
}

fn px_border_all(v: f32) -> Edges<BorderSide> {
    Edges::all(BorderSide { width: v, style: BorderStyle::Solid, color: stele::surface::Color::BLACK })
}

fn container(style: ComputedStyle, children: Vec<LayoutNode>) -> LayoutNode {
    LayoutNode { style, content: BoxContent::Container, children, interactive: None }
}

fn leaf_container(style: ComputedStyle) -> LayoutNode {
    container(style, Vec::new())
}

fn replaced(style: ComputedStyle, w: f32, h: f32) -> LayoutNode {
    LayoutNode {
        style,
        content: BoxContent::Replaced { intrinsic: Size { w, h }, image: None },
        children: Vec::new(),
        interactive: None,
    }
}

fn replaced_with_image(style: ComputedStyle, w: f32, h: f32, image: Rc<RgbaImage>) -> LayoutNode {
    LayoutNode {
        style,
        content: BoxContent::Replaced { intrinsic: Size { w, h }, image: Some(image) },
        children: Vec::new(),
        interactive: None,
    }
}

fn box_fragments(fragments: &[Fragment]) -> Vec<&Fragment> {
    fragments.iter().filter(|f| matches!(f.kind, FragmentKind::Box { .. })).collect()
}

#[test]
fn nested_margin_padding_border_produce_expected_rects() {
    // outer: padding 1px all round (blocks parent/child margin collapsing),
    // no margin/border of its own, at the tree root so its border-box width
    // is stretched to the viewport (300).
    let mut outer_style = block_style();
    outer_style.padding = px_padding_all(1.0);

    // inner: margin 10px all round, padding 5px, border 3px, width auto
    // (so it fills outer's content box minus its own margins -- unaffected
    // by box-sizing either way, since box-sizing only ever reinterprets an
    // EXPLICIT declared length, never an auto/stretch-fit width -- see
    // `layout::block::box_sizing_for`'s own doc comment), explicit
    // CONTENT-box height of 40px (packet/acid1-content-box: `ContentBox`
    // is the real CSS default this engine now uses everywhere except
    // table-internal display types and flex/grid, so a declared `height`
    // means content height, padding/border add on top to reach the
    // rendered border-box height -- this test predates that packet, when
    // taffy's own `BorderBox` default meant the declared height WAS the
    // border-box height instead).
    let mut inner_style = block_style();
    inner_style.margin = px_margin(10.0, 10.0, 10.0, 10.0);
    inner_style.padding = px_padding_all(5.0);
    inner_style.border = px_border_all(3.0);
    inner_style.height = Dimension::Px(40.0);

    let tree = container(outer_style, vec![leaf_container(inner_style)]);
    let fragments = layout(&tree, Size { w: 300.0, h: 200.0 });
    let boxes = box_fragments(&fragments);
    assert_eq!(boxes.len(), 2, "expected exactly one outer + one inner Box fragment");

    let outer = boxes[0];
    assert_eq!(outer.rect.origin.x, 0.0);
    assert_eq!(outer.rect.origin.y, 0.0);
    // packet/acid1-content-box: the root's auto-width stretch-fit to the
    // viewport must stay exactly 300 regardless of box-sizing (see
    // `layout::block::layout_tree`'s own doc comment on its root-width
    // override forcing `BoxSizing::BorderBox` for exactly this reason) --
    // this assertion catching a regression back to 302 (300 + this test's
    // own 1px+1px outer padding, wrongly added on top) is the whole point
    // of keeping it in this content-box-heavy test file.
    assert_eq!(outer.rect.size.w, 300.0, "root stretches to viewport width");
    // height = padding-top(1) + inner margin-box height (10 + inner's own
    // content-box height(56, see below) + 10) + padding-bottom(1) = 78.
    assert_eq!(outer.rect.size.h, 78.0);

    let inner = boxes[1];
    // origin = outer padding (1,1) + inner margin (10,10)
    assert_eq!(inner.rect.origin.x, 11.0);
    assert_eq!(inner.rect.origin.y, 11.0);
    // width(auto) = outer content width (300 - 2*1=298) - inner margins (10+10) = 278
    assert_eq!(inner.rect.size.w, 278.0);
    // height: declared height:40 is now a CONTENT height (content-box) --
    // border-box = 40 + padding(2*5=10) + border(2*3=6) = 56.
    assert_eq!(inner.rect.size.h, 56.0);
}

#[test]
fn block_width_auto_fills_container() {
    let root = container(block_style(), vec![leaf_container(block_style())]);
    let fragments = layout(&root, Size { w: 250.0, h: 100.0 });
    let boxes = box_fragments(&fragments);
    assert_eq!(boxes.len(), 2);
    assert_eq!(boxes[0].rect.size.w, 250.0);
    // no margin/padding/border anywhere: child fills the same content width.
    assert_eq!(boxes[1].rect.size.w, 250.0);
    assert_eq!(boxes[1].rect.origin.x, 0.0);
}

#[test]
fn block_height_derives_from_content() {
    let mut child_style = block_style();
    child_style.width = Dimension::Px(50.0);
    let root = container(block_style(), vec![replaced(child_style, 50.0, 30.0)]);
    let fragments = layout(&root, Size { w: 200.0, h: 500.0 });
    let boxes = box_fragments(&fragments);
    assert_eq!(boxes.len(), 2);
    // parent has no explicit height: derives from its one child's height.
    assert_eq!(boxes[0].rect.size.h, 30.0);
    assert_eq!(boxes[1].rect.size, Size { w: 50.0, h: 30.0 });
}

#[test]
fn asymmetric_margins_are_honored() {
    let mut outer_style = block_style();
    outer_style.padding = px_padding_all(2.0); // blocks parent/child collapsing
    let mut inner_style = block_style();
    inner_style.margin = px_margin(5.0, 10.0, 15.0, 20.0); // top right bottom left
    inner_style.width = Dimension::Px(30.0);
    inner_style.height = Dimension::Px(8.0);

    let tree = container(outer_style, vec![leaf_container(inner_style)]);
    let fragments = layout(&tree, Size { w: 300.0, h: 200.0 });
    let boxes = box_fragments(&fragments);
    let inner = boxes[1];
    assert_eq!(inner.rect.origin.x, 2.0 + 20.0, "outer padding + inner margin-left");
    assert_eq!(inner.rect.origin.y, 2.0 + 5.0, "outer padding + inner margin-top");
    assert_eq!(inner.rect.size, Size { w: 30.0, h: 8.0 });

    // parent height = padding-top(2) + margin-top(5) + child height(8) + margin-bottom(15) + padding-bottom(2)
    assert_eq!(boxes[0].rect.size.h, 2.0 + 5.0 + 8.0 + 15.0 + 2.0);
}

// -----------------------------------------------------------------------
// D6: adjacent-sibling margin collapsing (CSS2.1 §8.3.1). Taffy has no
// native concept of collapsing (it sums adjoining margins like a flex
// column would) -- these are the classic contract cases the packet exists
// to fix. Every tree below nests two block siblings directly inside a
// plain block root (no padding/border on the root, so root/first-child
// collapsing -- NOT implemented this packet, sibling-only -- never fires
// and can't be confused with the sibling behavior under test).
// -----------------------------------------------------------------------

/// A `display: block` style with a fixed height and the given margin-top/
/// margin-bottom (right/left left at the CSS initial `0`).
fn block_with_vertical_margin(height: f32, margin_top: f32, margin_bottom: f32) -> ComputedStyle {
    let mut style = block_style();
    style.height = Dimension::Px(height);
    style.margin = px_margin(margin_top, 0.0, margin_bottom, 0.0);
    style
}

#[test]
fn adjacent_block_siblings_collapse_to_the_larger_margin() {
    // A: margin-bottom 20; B: margin-top 30 -- collapsed gap is max(20, 30)
    // = 30, NOT the summed 50 taffy would produce natively.
    let a = leaf_container(block_with_vertical_margin(10.0, 0.0, 20.0));
    let b = leaf_container(block_with_vertical_margin(10.0, 30.0, 0.0));
    let root = container(block_style(), vec![a, b]);
    let fragments = layout(&root, Size { w: 200.0, h: 500.0 });
    let boxes = box_fragments(&fragments);
    assert_eq!(boxes.len(), 3, "root + A + B");
    let (a_box, b_box) = (boxes[1], boxes[2]);
    assert_eq!(a_box.rect.origin.y, 0.0);
    assert_eq!(a_box.rect.size.h, 10.0);
    assert_eq!(b_box.rect.origin.y, 10.0 + 30.0, "gap is max(20, 30), not 20 + 30");
    // root's own height reflects the collapsed (not summed) gap too.
    assert_eq!(boxes[0].rect.size.h, 10.0 + 30.0 + 10.0);
}

#[test]
fn adjacent_block_siblings_with_equal_margins_collapse_to_that_margin() {
    let a = leaf_container(block_with_vertical_margin(10.0, 0.0, 20.0));
    let b = leaf_container(block_with_vertical_margin(10.0, 20.0, 0.0));
    let root = container(block_style(), vec![a, b]);
    let fragments = layout(&root, Size { w: 200.0, h: 500.0 });
    let boxes = box_fragments(&fragments);
    let (a_box, b_box) = (boxes[1], boxes[2]);
    assert_eq!(b_box.rect.origin.y, a_box.rect.origin.y + a_box.rect.size.h + 20.0, "20 + 20 collapses to 20");
}

#[test]
fn a_border_between_block_siblings_prevents_collapse() {
    // B has a (zero-padding) top border: CSS2.1 says a border between two
    // adjoining margins ends the adjoinment -- the gap must stay SUMMED.
    let a = leaf_container(block_with_vertical_margin(10.0, 0.0, 20.0));
    let mut b_style = block_with_vertical_margin(10.0, 30.0, 0.0);
    b_style.border = px_border_all(2.0);
    let b = leaf_container(b_style);
    let root = container(block_style(), vec![a, b]);
    let fragments = layout(&root, Size { w: 200.0, h: 500.0 });
    let boxes = box_fragments(&fragments);
    let (a_box, b_box) = (boxes[1], boxes[2]);
    assert_eq!(
        b_box.rect.origin.y,
        a_box.rect.origin.y + a_box.rect.size.h + 20.0 + 30.0,
        "a border between the boxes must prevent collapsing -- gap stays summed"
    );
}

#[test]
fn padding_between_block_siblings_prevents_collapse() {
    // A has bottom padding: same rule, a padding gap also ends adjoinment.
    let mut a_style = block_with_vertical_margin(10.0, 0.0, 20.0);
    a_style.padding = Edges { top: LengthPercentage::Px(0.0), right: LengthPercentage::Px(0.0), bottom: LengthPercentage::Px(4.0), left: LengthPercentage::Px(0.0) };
    let a = leaf_container(a_style);
    let b = leaf_container(block_with_vertical_margin(10.0, 30.0, 0.0));
    let root = container(block_style(), vec![a, b]);
    let fragments = layout(&root, Size { w: 200.0, h: 500.0 });
    let boxes = box_fragments(&fragments);
    let (a_box, b_box) = (boxes[1], boxes[2]);
    assert_eq!(
        b_box.rect.origin.y,
        a_box.rect.origin.y + a_box.rect.size.h + 20.0 + 30.0,
        "padding between the boxes must prevent collapsing -- gap stays summed"
    );
}

#[test]
fn flex_column_item_margins_do_not_collapse() {
    // Flex items never participate in margin collapsing (CSS Flexbox §4) --
    // a flex column with the same 20/30 margins must keep the SUMMED gap,
    // unlike the identical-looking block case above.
    let flex_style = ComputedStyle {
        display: stele::style::computed::Display::Flex,
        flex_direction: FlexDirection::Column,
        ..ComputedStyle::default()
    };
    let a = leaf_container(block_with_vertical_margin(10.0, 0.0, 20.0));
    let b = leaf_container(block_with_vertical_margin(10.0, 30.0, 0.0));
    let root = container(flex_style, vec![a, b]);
    let fragments = layout(&root, Size { w: 200.0, h: 500.0 });
    let boxes = box_fragments(&fragments);
    let (a_box, b_box) = (boxes[1], boxes[2]);
    assert_eq!(b_box.rect.origin.y, a_box.rect.origin.y + a_box.rect.size.h + 20.0 + 30.0, "flex items sum, never collapse");
}

#[test]
fn floated_sibling_margins_do_not_collapse() {
    // A float is pulled out of normal flow -- CSS2.1 §8.3.1 excludes floats
    // from margin collapsing entirely, so a floated sibling keeps the
    // summed gap even though it's otherwise an ordinary block box.
    let a = leaf_container(block_with_vertical_margin(10.0, 0.0, 20.0));
    let mut b_style = block_with_vertical_margin(10.0, 30.0, 0.0);
    b_style.float = Float::Left;
    let b = leaf_container(b_style);
    let root = container(block_style(), vec![a, b]);
    let fragments = layout(&root, Size { w: 200.0, h: 500.0 });
    let boxes = box_fragments(&fragments);
    let (a_box, b_box) = (boxes[1], boxes[2]);
    assert_eq!(
        b_box.rect.origin.y,
        a_box.rect.origin.y + a_box.rect.size.h + 20.0 + 30.0,
        "a floated sibling must not collapse its margin with its neighbor"
    );
}

#[test]
fn table_row_sibling_margins_do_not_collapse() {
    // Table-internal boxes (table/table-row/table-row-group/table-cell)
    // participate in the TABLE formatting context, never ordinary block
    // sibling collapsing (CSS2.1 -- and this engine's own real <table>
    // pipeline never lays a row/cell out through the sibling collapse
    // pre-pass at all, see translate_any's table-leaf branch) -- this
    // covers the fallback path an orphan table-row/cell takes (outside a
    // real <table>, translated as a plain stacked block -- see translate_
    // any's own doc comment), which must still not collapse even though
    // it otherwise looks like an ordinary block box to `is_inline_ish`.
    let mut a_style = block_with_vertical_margin(10.0, 0.0, 20.0);
    a_style.display = stele::style::computed::Display::TableRow;
    let mut b_style = block_with_vertical_margin(10.0, 30.0, 0.0);
    b_style.display = stele::style::computed::Display::TableRow;
    let a = leaf_container(a_style);
    let b = leaf_container(b_style);
    let root = container(block_style(), vec![a, b]);
    let fragments = layout(&root, Size { w: 200.0, h: 500.0 });
    let boxes = box_fragments(&fragments);
    let (a_box, b_box) = (boxes[1], boxes[2]);
    assert_eq!(
        b_box.rect.origin.y,
        a_box.rect.origin.y + a_box.rect.size.h + 20.0 + 30.0,
        "table-row siblings must not collapse their margins"
    );
}

#[test]
fn whitespace_only_text_between_block_siblings_does_not_break_collapsing() {
    // Ordinary hand-formatted HTML puts a whitespace-only text node (a
    // newline + indentation) between sibling block elements in the DOM;
    // that text generates no box (CSS2.1 §9.2.2.1) and must not be treated
    // as "real content separating the boxes" -- the two blocks must still
    // collapse exactly as if the whitespace weren't there.
    let a = leaf_container(block_with_vertical_margin(10.0, 0.0, 20.0));
    let b = leaf_container(block_with_vertical_margin(10.0, 30.0, 0.0));
    let root = container(block_style(), vec![a, text_node("  \n  "), b]);
    let fragments = layout(&root, Size { w: 200.0, h: 500.0 });
    let boxes = box_fragments(&fragments);
    assert_eq!(boxes.len(), 3, "the whitespace-only text produces no Box fragment of its own");
    let (a_box, b_box) = (boxes[1], boxes[2]);
    assert_eq!(b_box.rect.origin.y, a_box.rect.origin.y + a_box.rect.size.h + 30.0, "collapses through whitespace-only text");
}

#[test]
fn replaced_node_occupies_intrinsic_size_in_flow() {
    let mut style = block_style();
    // width/height left auto: intrinsic size alone should still fix the box.
    style.width = Dimension::Auto;
    style.height = Dimension::Auto;
    let root = container(block_style(), vec![replaced(style, 64.0, 48.0)]);
    let fragments = layout(&root, Size { w: 640.0, h: 480.0 });
    let boxes = box_fragments(&fragments);
    assert_eq!(boxes.len(), 2);
    assert_eq!(boxes[1].rect.size, Size { w: 64.0, h: 48.0 });
}

#[test]
fn replaced_node_with_an_image_emits_an_image_fragment_at_its_rect_not_a_placeholder_box() {
    let mut style = block_style();
    style.width = Dimension::Auto;
    style.height = Dimension::Auto;
    let image = Rc::new(RgbaImage::new(4, 4));
    let root = container(block_style(), vec![replaced_with_image(style, 64.0, 48.0, image)]);
    let fragments = layout(&root, Size { w: 640.0, h: 480.0 });

    let image_fragments: Vec<&Fragment> =
        fragments.iter().filter(|f| matches!(f.kind, FragmentKind::Image { .. })).collect();
    assert_eq!(
        image_fragments.len(),
        1,
        "a Replaced with Some(image) should emit exactly one Image fragment, not a Box placeholder"
    );
    assert_eq!(image_fragments[0].rect.size, Size { w: 64.0, h: 48.0 });
    match &image_fragments[0].kind {
        FragmentKind::Image { image: emitted } => {
            assert_eq!((emitted.width, emitted.height), (4, 4));
        }
        _ => unreachable!(),
    }

    // Only the outer container's own Box fragment -- no placeholder Box for
    // the now-real-image replaced element.
    let boxes = box_fragments(&fragments);
    assert_eq!(boxes.len(), 1);
}

#[test]
fn zero_and_negative_viewport_do_not_panic() {
    let tree = container(block_style(), vec![leaf_container(block_style())]);
    for size in [Size { w: 0.0, h: 0.0 }, Size { w: -10.0, h: -5.0 }, Size { w: f32::NAN, h: f32::NAN }] {
        let fragments = layout(&tree, size);
        // total: never panics, and always yields at least the root box.
        assert!(!fragments.is_empty());
    }
}

#[test]
fn empty_text_child_does_not_break_flow() {
    let root = container(
        block_style(),
        vec![LayoutNode {
            style: ComputedStyle::default(),
            content: BoxContent::Text(String::new()),
            children: Vec::new(),
            interactive: None,
        }],
    );
    let fragments = layout(&root, Size { w: 200.0, h: 100.0 });
    assert!(!fragments.is_empty());
    // Empty text contributes no Text fragment (zero lines, per inline engine's
    // documented scope call).
    assert!(!fragments.iter().any(|f| matches!(f.kind, FragmentKind::Text { .. })));
}

#[test]
fn deeply_nested_tree_does_not_panic() {
    // 100 levels comfortably exceeds any realistic (even gnarly 1996
    // table-in-table) document nesting depth.
    let mut node = leaf_container(block_style());
    for _ in 0..100 {
        node = container(block_style(), vec![node]);
    }
    let fragments = layout(&node, Size { w: 400.0, h: 400.0 });
    assert!(!fragments.is_empty());
}

/// Regression test (code review, Critical 1): before `translate`/`flatten`
/// capped their recursion depth, a chain of nested `Container`s deep enough
/// (empirically ~200+ levels, both here and on the reviewer's machine) blew
/// the thread's stack — a guard-page fault (SIGABRT), not a catchable
/// `panic!`, so `panic = "abort"` gives no mitigation and the *test process
/// itself* aborts rather than failing gracefully. Nesting depth is entirely
/// page-controlled (deeply nested quote threads, WYSIWYG exports, old
/// nested-table markup), so this is reachable from hostile/generated HTML,
/// not just a synthetic stress case.
///
/// After the depth cap: `layout()` must simply *return* — that completion is
/// the proof, no special assertion needed beyond "we got a `Vec` back".
#[test]
fn extremely_deep_nesting_does_not_abort_the_process() {
    for depth in [2000usize, 5000] {
        let mut node = leaf_container(block_style());
        for _ in 0..depth {
            node = container(block_style(), vec![node]);
        }
        let fragments = layout(&node, Size { w: 400.0, h: 400.0 });
        assert!(!fragments.is_empty(), "depth {depth} must still produce a fragment vector");
    }
}

#[test]
fn replaced_with_non_finite_intrinsic_does_not_panic() {
    let root = container(
        block_style(),
        vec![
            replaced(block_style(), f32::NAN, f32::INFINITY),
            replaced(block_style(), -5.0, -5.0),
        ],
    );
    let fragments = layout(&root, Size { w: 200.0, h: 200.0 });
    assert!(!fragments.is_empty());
    for f in &fragments {
        assert!(f.rect.size.w.is_finite());
        assert!(f.rect.size.h.is_finite());
    }
}

// ---------------------------------------------------------------------------
// Flex geometry (M5 flex-polite): `display: flex` was already wired onto
// taffy (P6), but no packet before this one ever styled + rendered + pixel-
// verified a real flex layout. These are hand-built `LayoutNode` trees (no
// parsing/cascade involved, matching this file's own convention) proving the
// taffy translation produces the right RECT geometry for the CSS this
// packet's `fixtures/flex-polite.html` actually uses: row direction (the
// default), `flex-grow`, a fixed-width sibling, and `gap`.
// ---------------------------------------------------------------------------

#[test]
fn flex_row_lays_items_out_left_to_right_with_gap() {
    let root = container(flex_row_style(16.0), vec![leaf_container(flex_item_style(Some(50.0), 0.0)), leaf_container(flex_item_style(Some(50.0), 0.0))]);
    let fragments = layout(&root, Size { w: 400.0, h: 100.0 });
    let boxes = box_fragments(&fragments);
    assert_eq!(boxes.len(), 3, "container + 2 items");
    assert_eq!(boxes[1].rect.origin.x, 0.0);
    assert_eq!(boxes[1].rect.size.w, 50.0);
    // second item starts after the first item's width plus the gap.
    assert_eq!(boxes[2].rect.origin.x, 50.0 + 16.0);
    assert_eq!(boxes[2].rect.size.w, 50.0);
}

/// packet/t3-inline-spacing (the D3 fix): a two-value `gap: <row-gap>
/// <column-gap>` must use the COLUMN-gap -- not the row-gap -- for a
/// row-direction flex container's item-to-item HORIZONTAL advance. Before
/// this packet `ComputedStyle` only carried one scalar `gap`, reused for
/// both taffy axes, so a two-value declaration's column-gap was silently
/// dropped by `value::apply_property`'s `"gap"` arm and the row-gap value
/// did double duty -- see `ComputedStyle::column_gap`'s own doc comment and
/// `fixtures/httpforever.html`'s `.footer__projects { gap: .35rem 1.1rem;
/// }` for the real-world shape this reproduces. Distinct row (4.0) and
/// column (40.0) values so a regression that reads the wrong field (or
/// still reuses one scalar for both) shows up as a wrong `origin.x`, not a
/// coincidentally-matching one.
#[test]
fn flex_row_uses_column_gap_not_row_gap_for_horizontal_item_spacing() {
    let mut style = flex_row_style(4.0);
    style.column_gap = Some(40.0);
    let root = container(style, vec![leaf_container(flex_item_style(Some(50.0), 0.0)), leaf_container(flex_item_style(Some(50.0), 0.0))]);
    let fragments = layout(&root, Size { w: 400.0, h: 100.0 });
    let boxes = box_fragments(&fragments);
    assert_eq!(boxes.len(), 3, "container + 2 items");
    assert_eq!(boxes[1].rect.origin.x, 0.0);
    assert_eq!(boxes[2].rect.origin.x, 50.0 + 40.0, "second item must be offset by the COLUMN gap (40), not the row gap (4)");
}

/// Mirrors `fixtures/flex-polite.html`'s two-column `main`/`aside` layout: a
/// `flex-grow: 1` item must take all the width its fixed-width sibling
/// doesn't use (minus the gap between them), and the two items must sit
/// side by side (same y, different x) -- geometry a block layout could never
/// produce.
#[test]
fn flex_grow_item_takes_remaining_width_beside_a_fixed_width_sibling() {
    let article = flex_item_style(None, 1.0); // width: auto, flex-grow: 1
    let aside = flex_item_style(Some(200.0), 0.0); // fixed width, no grow
    let root = container(flex_row_style(24.0), vec![leaf_container(article), leaf_container(aside)]);
    let fragments = layout(&root, Size { w: 800.0, h: 100.0 });
    let boxes = box_fragments(&fragments);
    assert_eq!(boxes.len(), 3, "container + article + aside");

    let article_box = boxes[1];
    let aside_box = boxes[2];

    // side by side: same y, article strictly to the left of aside.
    assert_eq!(article_box.rect.origin.y, aside_box.rect.origin.y);
    assert!(article_box.rect.origin.x < aside_box.rect.origin.x);

    // aside keeps its fixed width...
    assert_eq!(aside_box.rect.size.w, 200.0);
    // ...and article grows to fill everything else: 800 - 200(aside) - 24(gap).
    assert_eq!(article_box.rect.size.w, 800.0 - 200.0 - 24.0);
    assert!(article_box.rect.size.w > aside_box.rect.size.w, "flex-grow item should end up wider than the fixed sidebar");
}

#[test]
fn justify_content_space_between_pushes_items_to_opposite_edges() {
    let mut style = flex_row_style(0.0);
    style.justify_content = stele::style::computed::JustifyContent::SpaceBetween;
    let title = flex_item_style(Some(100.0), 0.0);
    let nav = flex_item_style(Some(150.0), 0.0);
    let root = container(style, vec![leaf_container(title), leaf_container(nav)]);
    let fragments = layout(&root, Size { w: 800.0, h: 60.0 });
    let boxes = box_fragments(&fragments);
    assert_eq!(boxes.len(), 3);
    let title_box = boxes[1];
    let nav_box = boxes[2];
    assert_eq!(title_box.rect.origin.x, 0.0, "title (left item) stays flush left");
    assert_eq!(nav_box.rect.origin.x, 800.0 - 150.0, "nav (right item) is pushed flush right");
    assert!(title_box.rect.origin.x < nav_box.rect.origin.x, "nav must render to the right of the title");
}

/// Regression test: a whitespace-only `Text` node between two real flex
/// children (exactly what document-formatted HTML like
/// `<nav>\n  <a>Home</a>\n  <a>About</a>\n</nav>` produces -- see
/// `dom::parser`'s doc comment: any non-empty run of raw source text becomes
/// a `Text` node, whitespace-only or not) must NOT become its own flex item.
/// Per the CSS Flexbox spec (§4 "Flex Items"): "a child text node consisting
/// entirely of collapsible white space is not rendered, i.e. it does not
/// generate an anonymous flex item" -- so it must not consume a `gap` on
/// either side of it. Before the fix, `translate_container_children`'s flex
/// branch translated EVERY child (including whitespace-only text) into its
/// own taffy flex item, so a formatted `<nav>` rendered its links twice as
/// far apart as an unformatted one -- a real, silent layout bug any
/// indented/pretty-printed flex markup would trip over.
#[test]
fn whitespace_only_text_between_flex_items_does_not_consume_gap_space() {
    let unformatted = container(
        flex_row_style(16.0),
        vec![leaf_container(flex_item_style(Some(50.0), 0.0)), leaf_container(flex_item_style(Some(50.0), 0.0))],
    );
    let formatted = container(
        flex_row_style(16.0),
        vec![
            leaf_container(flex_item_style(Some(50.0), 0.0)),
            text_node("\n  "),
            leaf_container(flex_item_style(Some(50.0), 0.0)),
        ],
    );

    let unformatted_boxes_owned = layout(&unformatted, Size { w: 400.0, h: 100.0 });
    let formatted_fragments = layout(&formatted, Size { w: 400.0, h: 100.0 });

    let unformatted_boxes = box_fragments(&unformatted_boxes_owned);
    let formatted_boxes = box_fragments(&formatted_fragments);

    assert_eq!(unformatted_boxes.len(), 3);
    assert_eq!(formatted_boxes.len(), 3, "the whitespace-only text node must not emit its own Box fragment as a flex item");

    // The two real items must land at the exact same x in both trees -- the
    // whitespace-only text node in between must be invisible to flex geometry.
    assert_eq!(formatted_boxes[1].rect.origin.x, unformatted_boxes[1].rect.origin.x);
    assert_eq!(formatted_boxes[2].rect.origin.x, unformatted_boxes[2].rect.origin.x);
    assert_eq!(formatted_boxes[2].rect.origin.x, 50.0 + 16.0, "exactly one gap between the two real items, not two");
}

// ---------------------------------------------------------------------------
// Block-in-inline (packet/block-in-inline): a block-level box (`<ol>`/`<li>`)
// nested inside an inline-display `Container` (`<font>`, CSS initial
// `display: inline`) must NOT be folded into the surrounding inline
// formatting context -- CSS's "block-in-inline" resolution means it still
// renders as its own stacked block box. Confirmed real breakage:
// http://68k.news/ wraps every news list in `<font size="4"><ol><li>...`,
// collapsing every list to run-on text before this fix. See
// `fixtures/block-in-inline.html`/`tests/block_in_inline_golden.rs` for the
// real parse->cascade pipeline coverage of the same shape; this is the
// narrower hand-built-tree geometry check, matching this file's convention.
// ---------------------------------------------------------------------------

/// CSS's initial `display` value is `inline` (`ComputedStyle::default()`),
/// matching a real `<font>`/`<em>`/`<b>` element with no UA/author override.
fn inline_style() -> ComputedStyle {
    ComputedStyle::default()
}

#[test]
fn block_level_list_inside_an_inline_wrapper_is_not_folded_into_one_inline_leaf() {
    // <p><font><ol><li>Alpha</li><li>Beta</li></ol></font></p>
    let li1 = container(block_style(), vec![text_node("Alpha")]);
    let li2 = container(block_style(), vec![text_node("Beta")]);
    let ol = container(block_style(), vec![li1, li2]);
    let font = container(inline_style(), vec![ol]);
    let p = container(block_style(), vec![font]);

    let fragments = layout(&p, Size { w: 300.0, h: 500.0 });
    let boxes = box_fragments(&fragments);

    // Before the fix: `<ol>`/`<li>` get flattened into the SAME inline run
    // as the `<font>` wrapper (`is_inline_ish(font)` was true regardless of
    // its block content), so NONE of p/font/ol/li1/li2 besides `p` itself
    // get their own Box fragment. After the fix: each of
    // p, font, ol, li1, li2 is its own stacked block box.
    assert_eq!(boxes.len(), 5, "expected p+font+ol+li1+li2 as five separate Box fragments, got {}", boxes.len());

    let text_ys: Vec<f32> = fragments
        .iter()
        .filter_map(|f| match &f.kind {
            FragmentKind::Text { text, .. } if text.contains("Alpha") || text.contains("Beta") => Some(f.rect.origin.y),
            _ => None,
        })
        .collect();
    assert_eq!(text_ys.len(), 2, "expected one text fragment for each list item, got {text_ys:?}");
    assert_ne!(
        text_ys[0], text_ys[1],
        "Alpha and Beta must render on separate lines (block-level li), not run together in one inline run"
    );
}

// ---------------------------------------------------------------------------
// CSS Grid (packet/css-grid): `display: grid` + `grid-template-columns`/
// `grid-template-rows`, wired straight onto taffy's own `grid` cargo
// feature -- see `layout::block::apply_grid`'s doc comment for the mapping,
// and `Cargo.toml`'s own packet/css-grid comment for the feature-enable
// rationale. Deferred (documented, not covered by any test here):
// `grid-template-areas`, `grid-column`/`grid-row` explicit placement,
// `grid-auto-flow`, `grid-auto-columns`/`rows`, named lines, subgrid.
// ---------------------------------------------------------------------------

fn fr_track(f: f32) -> GridTemplateComponent {
    GridTemplateComponent::Single(GridTrack::Bare(GridTrackSize::Fr(f)))
}

#[test]
fn grid_three_equal_fr_columns_places_items_side_by_side() {
    let cols = vec![fr_track(1.0), fr_track(1.0), fr_track(1.0)];
    let grid = container(
        grid_style(cols, 0.0),
        vec![leaf_container(block_style()), leaf_container(block_style()), leaf_container(block_style())],
    );
    let fragments = layout(&page(grid), Size { w: 900.0, h: 200.0 });
    let boxes = box_fragments(&fragments);
    assert_eq!(boxes.len(), 5, "page + grid container + 3 items");

    let (item1, item2, item3) = (boxes[2], boxes[3], boxes[4]);
    assert_eq!(item1.rect.origin.y, item2.rect.origin.y, "all 3 items must share the same row");
    assert_eq!(item2.rect.origin.y, item3.rect.origin.y);
    assert_eq!(item1.rect.origin.x, 0.0);
    assert_eq!(item1.rect.size.w, 300.0);
    assert_eq!(item2.rect.origin.x, 300.0, "second column starts right after the first (no gap declared)");
    assert_eq!(item2.rect.size.w, 300.0);
    assert_eq!(item3.rect.origin.x, 600.0);
    assert_eq!(item3.rect.size.w, 300.0);
}

#[test]
fn grid_repeat_3_1fr_is_equivalent_to_three_bare_1fr_tracks() {
    // Same fixture as `grid_three_equal_fr_columns_places_items_side_by_side`
    // above, `grid-template-columns: repeat(3, 1fr)` instead of `1fr 1fr
    // 1fr` spelled out -- must produce the EXACT same geometry.
    let cols = vec![GridTemplateComponent::Repeat(GridRepetitionCount::Count(3), vec![GridTrack::Bare(GridTrackSize::Fr(1.0))])];
    let grid = container(
        grid_style(cols, 0.0),
        vec![leaf_container(block_style()), leaf_container(block_style()), leaf_container(block_style())],
    );
    let fragments = layout(&page(grid), Size { w: 900.0, h: 200.0 });
    let boxes = box_fragments(&fragments);
    assert_eq!(boxes.len(), 5, "page + grid container + 3 items");

    let (item1, item2, item3) = (boxes[2], boxes[3], boxes[4]);
    assert_eq!(item1.rect.origin.y, item2.rect.origin.y);
    assert_eq!(item2.rect.origin.y, item3.rect.origin.y);
    assert_eq!((item1.rect.origin.x, item1.rect.size.w), (0.0, 300.0));
    assert_eq!((item2.rect.origin.x, item2.rect.size.w), (300.0, 300.0));
    assert_eq!((item3.rect.origin.x, item3.rect.size.w), (600.0, 300.0));
}

/// `repeat(auto-fill, minmax(200px, 1fr))` at an 800px container width, with
/// NO gap, must produce EXACTLY 4 columns -- hand-verified against taffy's
/// own `compute_explicit_grid_size_in_axis` auto-repeat formula (`taffy-
/// 0.13.0/src/compute/grid/explicit_grid.rs`): a single repetition uses
/// `track_definite_value` = the track's min (200, since its max is `1fr`,
/// not itself definite) = 200px; with zero gap, `floor((800 - 200) / 200)
/// + 1 = floor(3.0) + 1 = 4`. This is the spike's (spike/taffy-grid, PR
/// #69) own reported miscount (2 columns instead of 4) — reproduced here
/// as a passing test now that the grid container has a definite width to
/// auto-repeat against (see `page`'s own doc comment for why that matters).
#[test]
fn grid_auto_fill_minmax_computes_the_correct_column_count_at_800px() {
    let cols = vec![GridTemplateComponent::Repeat(
        GridRepetitionCount::AutoFill,
        vec![GridTrack::MinMax(GridTrackSize::Length(200.0), GridTrackSize::Fr(1.0))],
    )];
    let grid = container(
        grid_style(cols, 0.0),
        vec![
            leaf_container(block_style()),
            leaf_container(block_style()),
            leaf_container(block_style()),
            leaf_container(block_style()),
        ],
    );
    let fragments = layout(&page(grid), Size { w: 800.0, h: 200.0 });
    let boxes = box_fragments(&fragments);
    assert_eq!(boxes.len(), 6, "page + grid container + 4 items");

    let items = &boxes[2..6];
    let row1_y = items[0].rect.origin.y;
    for it in items {
        assert_eq!(it.rect.origin.y, row1_y, "all 4 items must land in row 1, not wrap to a second row");
    }
    let mut xs: Vec<f32> = items.iter().map(|f| f.rect.origin.x).collect();
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(
        xs,
        vec![0.0, 200.0, 400.0, 600.0],
        "repeat(auto-fill, minmax(200px,1fr)) at 800px must produce exactly 4 columns at these x-bands, not 2"
    );
}

#[test]
fn grid_gap_produces_expected_inter_item_spacing() {
    let cols = vec![
        GridTemplateComponent::Single(GridTrack::Bare(GridTrackSize::Length(100.0))),
        GridTemplateComponent::Single(GridTrack::Bare(GridTrackSize::Length(100.0))),
    ];
    let grid = container(grid_style(cols, 20.0), vec![leaf_container(block_style()), leaf_container(block_style())]);
    let fragments = layout(&page(grid), Size { w: 500.0, h: 200.0 });
    let boxes = box_fragments(&fragments);
    assert_eq!(boxes.len(), 4, "page + grid container + 2 items");

    let (item1, item2) = (boxes[2], boxes[3]);
    assert_eq!(item1.rect.origin.x, 0.0);
    assert_eq!(item1.rect.size.w, 100.0);
    assert_eq!(item2.rect.origin.x, 100.0 + 20.0, "second item must be offset by the declared 20px gap");
    assert_eq!(item2.rect.size.w, 100.0);
}

/// Priority-1 regression guard: a document that never declares `display:
/// grid` anywhere must lay out to the EXACT SAME rects after packet/
/// css-grid as before it -- enabling taffy's `grid` cargo feature and
/// adding `Display::Grid`/`apply_grid` must be purely additive. Combines
/// block margin collapsing (D6, packet/t6-margin-collapse) with a nested
/// flex row (two independent non-grid layout paths) in one tree; if either
/// path is undisturbed by this packet, the numbers below are exact.
#[test]
fn non_grid_block_and_flex_layout_is_unchanged_by_grid_support() {
    let mut top = block_style();
    top.margin.bottom = LengthPercentageAuto::Px(10.0);
    let mut bottom = block_style();
    bottom.margin.top = LengthPercentageAuto::Px(20.0);
    let flex_row = container(
        flex_row_style(8.0),
        vec![leaf_container(flex_item_style(Some(50.0), 0.0)), leaf_container(flex_item_style(Some(50.0), 0.0))],
    );
    let root = container(block_style(), vec![leaf_container(top), leaf_container(bottom), flex_row]);
    let fragments = layout(&root, Size { w: 400.0, h: 300.0 });
    let boxes = box_fragments(&fragments);
    assert_eq!(boxes.len(), 6, "root + top + bottom + flex row + 2 flex items");

    // D6 margin collapse: max(10, 20) = 20, not summed to 30.
    let top_box = boxes[1];
    let bottom_box = boxes[2];
    assert_eq!(top_box.rect.origin.y, 0.0);
    assert_eq!(bottom_box.rect.origin.y, 20.0, "adjoining margins must still collapse to max(10,20)=20, not sum to 30");

    // Nested flex row: two 50px-wide items 8px apart, side by side, same row.
    let flex_item1 = boxes[4];
    let flex_item2 = boxes[5];
    assert_eq!(flex_item1.rect.origin.x, 0.0);
    assert_eq!(flex_item2.rect.origin.x, 50.0 + 8.0);
    assert_eq!(flex_item1.rect.origin.y, flex_item2.rect.origin.y);
}

/// Acid2 P1 §3 paint-order fix: a `position: absolute` child declared BEFORE
/// a static in-flow sibling must still be EMITTED (i.e. painted) after it --
/// CSS 2.1's paint order puts positioned descendants above in-flow content
/// regardless of source order. `children = [a_positioned, b_static]`; both
/// overlap in space (the point of paint order mattering at all) and are
/// distinguished by their unique declared sizes. Before the `emit` fix this
/// fails because `Built::Container`'s child loop walked `children` in plain
/// document order, so `a_positioned`'s Box fragment landed at a LOWER index
/// than `b_static`'s.
#[test]
fn positioned_child_paints_after_static_sibling_regardless_of_source_order() {
    let mut a_positioned = block_style();
    a_positioned.position = Position::Absolute;
    a_positioned.inset = Edges {
        top: LengthPercentageAuto::Px(5.0),
        left: LengthPercentageAuto::Px(5.0),
        right: LengthPercentageAuto::Auto,
        bottom: LengthPercentageAuto::Auto,
    };
    a_positioned.width = Dimension::Px(111.0);
    a_positioned.height = Dimension::Px(22.0);

    let mut b_static = block_style();
    b_static.width = Dimension::Px(77.0);
    b_static.height = Dimension::Px(33.0);

    let root = container(block_style(), vec![leaf_container(a_positioned), leaf_container(b_static)]);
    let fragments = layout(&root, Size { w: 300.0, h: 200.0 });
    let boxes = box_fragments(&fragments);
    assert_eq!(boxes.len(), 3, "root + a_positioned + b_static");

    let a_index = boxes
        .iter()
        .position(|f| f.rect.size.w == 111.0 && f.rect.size.h == 22.0)
        .expect("a_positioned's Box fragment must be present");
    let b_index = boxes
        .iter()
        .position(|f| f.rect.size.w == 77.0 && f.rect.size.h == 33.0)
        .expect("b_static's Box fragment must be present");

    assert!(
        b_index < a_index,
        "static sibling must paint (be emitted) before the positioned child that precedes it in source order: \
         b_static at index {b_index}, a_positioned at index {a_index}"
    );
}
