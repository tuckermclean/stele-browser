//! Block-flow geometry tests (P6, M2): exact `Rect`s for nested block boxes
//! translated onto taffy's degenerate-column-flex substrate, plus replaced
//! sizing and totality on degenerate input. Hand-built `LayoutNode` trees —
//! no parsing/cascade involved (that's `layout_integration.rs`).

use std::rc::Rc;

use stele::img::RgbaImage;
use stele::layout::{layout, BoxContent, Fragment, FragmentKind, LayoutNode, Size};
use stele::style::computed::{
    BorderSide, BorderStyle, Dimension, Edges, FlexDirection, LengthPercentage, LengthPercentageAuto,
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
