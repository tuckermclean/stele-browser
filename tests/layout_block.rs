//! Block-flow geometry tests (P6, M2): exact `Rect`s for nested block boxes
//! translated onto taffy's degenerate-column-flex substrate, plus replaced
//! sizing and totality on degenerate input. Hand-built `LayoutNode` trees —
//! no parsing/cascade involved (that's `layout_integration.rs`).

use stele::layout::{layout, BoxContent, Fragment, FragmentKind, LayoutNode, Size};
use stele::style::computed::{BorderSide, BorderStyle, Dimension, Edges, LengthPercentage, LengthPercentageAuto};
use stele::style::ComputedStyle;

/// A `display: block` style with everything else at CSS initial values.
fn block_style() -> ComputedStyle {
    ComputedStyle { display: stele::style::computed::Display::Block, ..ComputedStyle::default() }
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
    LayoutNode { style, content: BoxContent::Container, children }
}

fn leaf_container(style: ComputedStyle) -> LayoutNode {
    container(style, Vec::new())
}

fn replaced(style: ComputedStyle, w: f32, h: f32) -> LayoutNode {
    LayoutNode { style, content: BoxContent::Replaced { intrinsic: Size { w, h }, image: None }, children: Vec::new() }
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
    // (so it fills outer's content box minus its own margins), explicit
    // border-box height of 40px (box-sizing is border-box, matching taffy's
    // default — see block.rs module docs).
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
    assert_eq!(outer.rect.size.w, 300.0, "root stretches to viewport width");
    // height = padding-top(1) + inner margin-box height (10+40+10) + padding-bottom(1)
    assert_eq!(outer.rect.size.h, 62.0);

    let inner = boxes[1];
    // origin = outer padding (1,1) + inner margin (10,10)
    assert_eq!(inner.rect.origin.x, 11.0);
    assert_eq!(inner.rect.origin.y, 11.0);
    // width(auto) = outer content width (300 - 2*1=298) - inner margins (10+10) = 278
    assert_eq!(inner.rect.size.w, 278.0);
    assert_eq!(inner.rect.size.h, 40.0);
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
        vec![LayoutNode { style: ComputedStyle::default(), content: BoxContent::Text(String::new()), children: Vec::new() }],
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
