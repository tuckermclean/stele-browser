// SPDX-License-Identifier: GPL-3.0-or-later

//! Stage-0 tests (packet/block-floats): the real parse->cascade->box_tree->
//! layout pipeline exercised against small hand-written HTML snippets,
//! proving taffy's `float_layout` feature (re-enabled in `Cargo.toml`, wired
//! into `layout::block::base_style` via `map_float`/`map_clear`) places
//! BLOCK-LEVEL `float`/`clear` boxes correctly -- the gap
//! `fixtures/evidence/css1-float-5526c.diagnosis.md` diagnosed (block-level
//! floats/clears were silently no-ops before this packet; only the bespoke
//! `layout::inline` mechanism for floated *inline replaced* atoms, e.g.
//! `<img align=left>`, worked -- see `tests/layout_floats.rs` for that
//! existing, UNTOUCHED coverage).
//!
//! Mirrors `tests/layout_floats.rs`'s own discipline: real pipeline, no
//! pixel golden here (that's `tests/css1_float_golden.rs`, once blessed),
//! `Fragment` rects asserted directly. Every fixture below is a bare list of
//! `<div>`s -- no `<html>`/`<body>` wrapper tags (matching this whole test
//! suite's own fragment convention, e.g. `tests/layout_floats.rs`): `dom::
//! parser::parse`'s root is always a SYNTHETIC `<html>` (`src/dom/parser.rs`
//! docs), and with no explicit `<body>` tag in the source, no `<body>`
//! ELEMENT is ever created, so the UA sheet's `body { margin: 8px; }`
//! (`src/style/ua.rs:39`) never matches anything -- content sits flush
//! against the (unstyled) synthetic `<html>` root's content edges, `(0,
//! 0)`..`(viewport_w, ...)`. This also sidesteps a documented taffy 0.13
//! float_layout rough edge (`compute/block.rs`'s own "TODO: handle nested
//! blocks with different widths" comment) by never introducing an
//! intermediate width-narrower wrapper `<div>` -- exactly how
//! `fixtures/css1-float-5526c.html` itself is shaped too (its floated
//! `dt`/`dd`/`li`/`blockquote`/`h1` are all DIRECT children of an unsized
//! `<dl>`/`<ul>`, which themselves inherit their parent's full width, never
//! narrower).

use std::collections::HashMap;
use std::rc::Rc;

use stele::img::RgbaImage;
use stele::layout::box_tree::build_box_tree;
use stele::layout::{layout, Fragment, FragmentKind, Rect, Size};
use stele::style::cascade;
use stele::surface::Color;

/// Render `html` through the real parse->cascade->box_tree->layout
/// pipeline (mirrors `tests/layout_floats.rs`'s own `render` helper) at a
/// tall-enough viewport that nothing here needs to scroll.
fn render(html: &str, viewport_w: f32) -> Vec<Fragment> {
    let dom = stele::dom::parser::parse(html);
    let styles = cascade::cascade(&dom, &[]);
    let images: HashMap<stele::dom::NodeId, Rc<RgbaImage>> = HashMap::new();
    let root = build_box_tree(&dom, &styles, &images).expect("root present");
    layout(&root, Size { w: viewport_w, h: 100_000.0 })
}

/// Find the one `FragmentKind::Box` painting the given `background-color`
/// -- each test fixture below gives every `<div>` it cares about a unique
/// `rgb(...)` so its fragment can be picked out of the flat paint-ordered
/// list unambiguously (mirrors how `tests/images_golden.rs`/`layout_floats.
/// rs` key fragments off kind/content rather than tree position, since
/// `layout::layout`'s output is a flat `Vec<Fragment>`, not a tree).
fn box_with_bg(fragments: &[Fragment], bg: Color) -> Rect {
    let matches: Vec<&Fragment> = fragments
        .iter()
        .filter(|f| matches!(&f.kind, FragmentKind::Box { style } if style.background_color == bg))
        .collect();
    assert_eq!(matches.len(), 1, "expected exactly one Box fragment with background-color {bg:?}, found {}", matches.len());
    matches[0].rect
}

const RED: Color = Color::rgb(200, 0, 0);
const GREEN: Color = Color::rgb(0, 200, 0);
const BLUE: Color = Color::rgb(0, 0, 200);
const YELLOW: Color = Color::rgb(255, 255, 0);

/// Two `float: left` block siblings must be placed SIDE BY SIDE (the second
/// starting at or after the first's right edge, both occupying overlapping
/// vertical space), not stacked one below the other the way normal block
/// flow (or the pre-packet no-op float handling) would place them.
#[test]
fn two_float_left_siblings_sit_side_by_side_not_stacked() {
    let html = r#"
        <div style="float:left;width:50px;height:20px;background-color:rgb(200,0,0);"></div>
        <div style="float:left;width:60px;height:20px;background-color:rgb(0,200,0);"></div>
    "#;
    let fragments = render(html, 300.0);

    let a = box_with_bg(&fragments, RED);
    let b = box_with_bg(&fragments, GREEN);

    assert_eq!(a.size, Size { w: 50.0, h: 20.0 });
    assert_eq!(b.size, Size { w: 60.0, h: 20.0 });

    // Side by side: B starts at or after A's right edge -- NOT stacked
    // below it (which would put b.origin.y >= a.origin.y + a.size.h and
    // b.origin.x roughly equal to a.origin.x, the pre-packet behavior).
    assert!(
        b.origin.x >= a.origin.x + a.size.w - 0.01,
        "float B (x={}) must start at/after float A's right edge ({})",
        b.origin.x,
        a.origin.x + a.size.w
    );

    // Overlapping y: both floats occupy the same row.
    let overlaps = a.origin.y < b.origin.y + b.size.h && b.origin.y < a.origin.y + a.size.h;
    assert!(overlaps, "float A (y={}) and float B (y={}) must occupy overlapping vertical space", a.origin.y, b.origin.y);
}

/// A `float: right` block sits at its containing block's (here, the
/// synthetic `<html>` root's own content box -- see module docs for why no
/// `<body>` element, and hence no UA-sheet `body` margin, is in play here)
/// RIGHT edge.
#[test]
fn float_right_block_sits_at_containing_blocks_right_edge() {
    let html = r#"<div style="float:right;width:50px;height:20px;background-color:rgb(200,0,0);"></div>"#;
    let viewport_w = 300.0;
    let fragments = render(html, viewport_w);

    let r = box_with_bg(&fragments, RED);
    assert_eq!(r.size, Size { w: 50.0, h: 20.0 });

    // The root's own right content edge -- no margin/padding in play (see
    // module docs), so it's simply the viewport width.
    let body_right_edge = viewport_w;
    assert!(
        (r.origin.x + r.size.w - body_right_edge).abs() < 0.5,
        "float:right's right edge ({}) must sit at body's own content-box right edge ({})",
        r.origin.x + r.size.w,
        body_right_edge
    );
}

/// A `float: left` block plus a normal-flow (non-floated) block sibling:
/// the normal-flow box must not be pushed BELOW the float (the pre-packet
/// no-op float handling, and a hypothetical "clear-like" over-eager fix,
/// would both stack it below). Taffy 0.13's `float_layout` (see its own
/// `compute/block.rs`) does not shrink/offset an ordinary in-flow block
/// sibling's OWN border box around a float at all -- only `clear` (Stage-0
/// case below) or a box establishing its own independent formatting
/// context does that; a plain block's box starts at the SAME normal-flow
/// position it would occupy with no float present (real CSS: only INLINE
/// content -- text/line boxes -- wraps around a float within a shared
/// formatting context, which is unaffected here since this fixture has no
/// text). This test asserts the honest, weaker claim the packet brief
/// itself allows ("the normal box isn't pushed fully below") rather than
/// over-claiming text-wrap-style exclusion this taffy feature doesn't do
/// for block-level siblings.
#[test]
fn float_left_and_normal_flow_sibling_coexist_not_stacked_below() {
    let html = r#"
        <div style="float:left;width:50px;height:100px;background-color:rgb(200,0,0);"></div>
        <div style="width:80px;height:40px;background-color:rgb(0,0,200);"></div>
    "#;
    let fragments = render(html, 300.0);

    let f = box_with_bg(&fragments, RED);
    let normal = box_with_bg(&fragments, BLUE);

    assert!(
        normal.origin.y < f.origin.y + f.size.h,
        "normal-flow sibling (y={}) must not be pushed fully below the float (bottom={})",
        normal.origin.y,
        f.origin.y + f.size.h
    );
}

/// A `clear: both` block below a `float: left` AND a `float: right` box
/// must be pushed down PAST both floats' bottom edges (CSS 2.1 §9.5.2
/// clearance) -- not placed at the normal-flow position it would occupy
/// with no floats present.
#[test]
fn clear_both_is_pushed_below_both_floats_bottom_edges() {
    let html = r#"
        <div style="float:left;width:50px;height:100px;background-color:rgb(200,0,0);"></div>
        <div style="float:right;width:50px;height:60px;background-color:rgb(0,200,0);"></div>
        <div style="clear:both;width:80px;height:20px;background-color:rgb(0,0,200);"></div>
    "#;
    let fragments = render(html, 300.0);

    let left_float = box_with_bg(&fragments, RED);
    let right_float = box_with_bg(&fragments, GREEN);
    let cleared = box_with_bg(&fragments, BLUE);

    // First prove the two floats actually floated (occupy overlapping
    // vertical space, side by side) rather than stacking normally -- a
    // no-op float implementation would stack left_float then right_float
    // then cleared strictly in document order, which would ALSO happen to
    // satisfy the two clearance assertions below (vacuously: plain
    // monotonic stacking already puts everything after everything else).
    // This overlap check is what makes this a real red/green test rather
    // than one that accidentally passes pre-fix.
    let floats_overlap = left_float.origin.y < right_float.origin.y + right_float.size.h
        && right_float.origin.y < left_float.origin.y + left_float.size.h;
    assert!(
        floats_overlap,
        "the left (y={}) and right (y={}) floats must occupy overlapping vertical space (i.e. actually float, not stack)",
        left_float.origin.y,
        right_float.origin.y
    );

    let left_bottom = left_float.origin.y + left_float.size.h;
    let right_bottom = right_float.origin.y + right_float.size.h;
    assert!(
        cleared.origin.y + 0.01 >= left_bottom,
        "clear:both box (y={}) must clear the LEFT float's bottom edge ({})",
        cleared.origin.y,
        left_bottom
    );
    assert!(
        cleared.origin.y + 0.01 >= right_bottom,
        "clear:both box (y={}) must clear the RIGHT float's bottom edge ({})",
        cleared.origin.y,
        right_bottom
    );
}

/// Nested float contexts: a floated block containing further floated
/// children resolves those INNER floats against the INNER containing
/// block's width (the floated parent's own resolved content width), not
/// the OUTER (viewport) containing block's width. Four 40px-wide inner
/// floats (160px total) do not fit in the 150px-wide floated parent, so the
/// 4th must wrap onto a new row -- if the inner floats were (wrongly)
/// resolving against the outer 300px viewport width (comfortably >= 160px)
/// all four would fit on one row and none would wrap. A floated block
/// always establishes its own new block
/// formatting context (CSS 2.1 §9.4 / taffy's `compute_block_layout`: a
/// floated child is laid out via `perform_child_layout`, never
/// `compute_block_child_layout`, so it always gets a FRESH root
/// `BlockFormattingContext` sized to its own resolved width -- see
/// `compute/block.rs`), so this is exactly the behavior taffy's
/// `float_layout` should produce.
#[test]
fn nested_floats_resolve_against_inner_containing_block_width() {
    let html = r#"
        <div style="float:left;width:150px;height:300px;background-color:rgb(200,0,0);">
            <div style="float:left;width:40px;height:20px;background-color:rgb(0,200,0);"></div>
            <div style="float:left;width:40px;height:20px;background-color:rgb(0,0,200);"></div>
            <div style="float:left;width:40px;height:20px;background-color:rgb(255,255,0);"></div>
            <div style="float:left;width:40px;height:20px;background-color:rgb(128,0,128);"></div>
        </div>
    "#;
    let fragments = render(html, 300.0);

    let outer = box_with_bg(&fragments, RED);
    let inner_a = box_with_bg(&fragments, GREEN);
    let inner_b = box_with_bg(&fragments, BLUE);
    let inner_c = box_with_bg(&fragments, YELLOW);
    let inner_d = box_with_bg(&fragments, Color::rgb(128, 0, 128));

    // Every inner float must stay within the OUTER floated block's own
    // border box -- never anywhere near the 300px viewport's own right
    // edge, which would be the tell if they'd resolved against the outer
    // (wrong) containing block instead.
    for (name, inner) in [("a", inner_a), ("b", inner_b), ("c", inner_c), ("d", inner_d)] {
        assert!(
            inner.origin.x + inner.size.w <= outer.origin.x + outer.size.w + 0.01,
            "inner float {name} (right edge={}) must stay within the outer floated block's own width (right edge={})",
            inner.origin.x + inner.size.w,
            outer.origin.x + outer.size.w
        );
    }

    // First prove a/b/c actually floated side by side (overlapping y, each
    // starting at/after the previous one's right edge) -- a no-op float
    // implementation stacks every inner div on its own row in document
    // order, which would ALSO happen to satisfy the "4th wraps below the
    // first" assertion below (vacuously: d is simply the last of four
    // already-stacked rows). This is what makes the wrap assertion below a
    // real red/green signal rather than one that accidentally passes
    // pre-fix.
    let ab_overlap =
        inner_a.origin.y < inner_b.origin.y + inner_b.size.h && inner_b.origin.y < inner_a.origin.y + inner_a.size.h;
    assert!(ab_overlap, "inner floats a (y={}) and b (y={}) must be on the same row (float, not stack)", inner_a.origin.y, inner_b.origin.y);
    let bc_overlap =
        inner_b.origin.y < inner_c.origin.y + inner_c.size.h && inner_c.origin.y < inner_b.origin.y + inner_b.size.h;
    assert!(bc_overlap, "inner floats b (y={}) and c (y={}) must be on the same row (float, not stack)", inner_b.origin.y, inner_c.origin.y);
    assert!(
        inner_b.origin.x >= inner_a.origin.x + inner_a.size.w - 0.01,
        "inner float b (x={}) must sit at/after inner float a's right edge ({})",
        inner_b.origin.x,
        inner_a.origin.x + inner_a.size.w
    );

    // 4th inner float (160px of floats crammed into a 150px inner
    // containing block) must have wrapped onto a new row below the first
    // three -- proof the 150px inner width, not the much wider outer one,
    // governed placement.
    assert!(
        inner_d.origin.y > inner_a.origin.y + 0.01,
        "4th inner float (y={}) must wrap onto a new row below the first row (y={}) -- proves it resolved against the 150px INNER containing block, not the ~284px outer one",
        inner_d.origin.y,
        inner_a.origin.y
    );
}

/// The exact shape `nested_floats_resolve_against_inner_containing_block_
/// width` above deliberately sidesteps (see this module's own doc comment):
/// the floated children are NOT direct children of the floated parent --
/// there is an intermediate, ordinary (`float: none`) block WRAPPER between
/// them, exactly like `fixtures/css1-float-5526c.html`'s `<dd>` (floated) >
/// `<ul>` (plain block, no float/width/border/padding of its own) > `<li>`
/// (floated) shape. The wrapper is unsized (`width: auto`) and has no
/// border/padding/margin, so it stretches to fill the floated parent's full
/// content width and contributes NO extra inset -- if block-level float
/// placement is correct, this must place identically to the direct-child
/// case above (same 150px inner containing block, same wrap-at-4th-float
/// behavior). If it does NOT, the bug is specific to floats nested BENEATH
/// a non-floated wrapper inside a floated container, not to nesting itself.
#[test]
fn nested_floats_beneath_a_plain_wrapper_still_wrap_at_inner_width() {
    let html = r#"
        <div style="float:left;width:150px;height:300px;background-color:rgb(200,0,0);">
            <div>
                <div style="float:left;width:40px;height:20px;background-color:rgb(0,200,0);"></div>
                <div style="float:left;width:40px;height:20px;background-color:rgb(0,0,200);"></div>
                <div style="float:left;width:40px;height:20px;background-color:rgb(255,255,0);"></div>
                <div style="float:left;width:40px;height:20px;background-color:rgb(128,0,128);"></div>
            </div>
        </div>
    "#;
    let fragments = render(html, 300.0);

    let outer = box_with_bg(&fragments, RED);
    let inner_a = box_with_bg(&fragments, GREEN);
    let inner_b = box_with_bg(&fragments, BLUE);
    let inner_c = box_with_bg(&fragments, YELLOW);
    let inner_d = box_with_bg(&fragments, Color::rgb(128, 0, 128));

    for (name, inner) in [("a", inner_a), ("b", inner_b), ("c", inner_c), ("d", inner_d)] {
        assert!(
            inner.origin.x + inner.size.w <= outer.origin.x + outer.size.w + 0.01,
            "inner float {name} (right edge={}) must stay within the outer floated block's own width (right edge={}) -- through a plain wrapper",
            inner.origin.x + inner.size.w,
            outer.origin.x + outer.size.w
        );
    }

    let ab_overlap =
        inner_a.origin.y < inner_b.origin.y + inner_b.size.h && inner_b.origin.y < inner_a.origin.y + inner_a.size.h;
    assert!(ab_overlap, "inner floats a (y={}) and b (y={}) must be on the same row through a plain wrapper", inner_a.origin.y, inner_b.origin.y);
    let bc_overlap =
        inner_b.origin.y < inner_c.origin.y + inner_c.size.h && inner_c.origin.y < inner_b.origin.y + inner_b.size.h;
    assert!(bc_overlap, "inner floats b (y={}) and c (y={}) must be on the same row through a plain wrapper", inner_b.origin.y, inner_c.origin.y);
    assert!(
        inner_b.origin.x >= inner_a.origin.x + inner_a.size.w - 0.01,
        "inner float b (x={}) must sit at/after inner float a's right edge ({}) through a plain wrapper",
        inner_b.origin.x,
        inner_a.origin.x + inner_a.size.w
    );

    assert!(
        inner_d.origin.y > inner_a.origin.y + 0.01,
        "4th inner float (y={}) must wrap onto a new row below the first row (y={}) through a plain wrapper -- proves nesting beneath a non-floated wrapper still resolves against the 150px inner containing block",
        inner_d.origin.y,
        inner_a.origin.y
    );
}

/// packet/acid1-content-box (re-applying packet/acid1-coherence's proof): a
/// floated box's declared `width` must be its CONTENT size, with
/// padding/border adding on top to reach the rendered (border-box) width --
/// CSS's `box-sizing: content-box` initial value, which is the ONLY value
/// this engine can mean (there is no `box-sizing` CSS property support at
/// all, see `layout::block::float_box_sizing`'s own doc comment). Before
/// this packet, `layout::block::base_style` never set taffy's `box_sizing`
/// field at all, so every node silently fell back to taffy's OWN default
/// (`BoxSizing::BorderBox`): the declared `width` pinned the BORDER-BOX
/// size instead, so padding/border ate INTO the content area rather than
/// adding beyond it.
#[test]
fn floated_box_with_padding_and_border_uses_content_box_sizing() {
    let html = r#"
        <div style="float:left;width:100px;height:50px;padding:10px;border:5px solid black;background-color:rgb(200,0,0);"></div>
    "#;
    let fragments = render(html, 300.0);
    let outer = box_with_bg(&fragments, RED);
    // content-box: border-box width = 100 (declared content width) + 20
    // (padding, 2*10px) + 10 (border, 2*5px) = 130. The pre-fix
    // (BorderBox-default) behavior would render this at exactly 100 --
    // the declared `width` swallowing its own padding/border instead of
    // growing past them.
    assert_eq!(
        outer.size.w, 130.0,
        "floated box's rendered (border-box) width must add padding+border on top of the declared content width, not subtract them from it"
    );
}

/// packet/acid1-content-box regression: this is `fixtures/css1-float-5526c.
/// html`'s `dd{width:34em;padding:1em;border:1em}` containing floated
/// `<li>`s shape, shrunk to round numbers. The OUTER floated box has an
/// explicit `width` AND non-zero padding/border (like `dd`) -- before this
/// packet's box_sizing fix, its rendered content width was narrower than
/// its declared `width` (padding/border eating INTO it, not adding beyond
/// it), which starved its floated children of row width they should have
/// had: with the bug, the outer's content width computes to 100 - 2*10
/// (padding) - 2*5 (border) = 70px, too narrow to fit even TWO 40px inner
/// floats side by side (80 > 70) -- all three would stack one per row.
/// Content-box sizing (this packet's fix) gives the outer its full declared
/// 100px content width, wide enough for exactly two 40px floats per row
/// (80 <= 100), so the third wraps onto a second row instead of every one
/// stacking individually.
#[test]
fn nested_floats_wrap_against_padded_outer_floats_content_width() {
    let html = r#"
        <div style="float:left;width:100px;height:200px;padding:10px;border:5px solid black;background-color:rgb(200,0,0);">
            <div style="float:left;width:40px;height:20px;background-color:rgb(0,200,0);"></div>
            <div style="float:left;width:40px;height:20px;background-color:rgb(0,0,200);"></div>
            <div style="float:left;width:40px;height:20px;background-color:rgb(255,255,0);"></div>
        </div>
    "#;
    let fragments = render(html, 300.0);
    let inner_a = box_with_bg(&fragments, GREEN);
    let inner_b = box_with_bg(&fragments, BLUE);
    let inner_c = box_with_bg(&fragments, YELLOW);

    let ab_overlap =
        inner_a.origin.y < inner_b.origin.y + inner_b.size.h && inner_b.origin.y < inner_a.origin.y + inner_a.size.h;
    assert!(
        ab_overlap,
        "inner floats a (y={}) and b (y={}) must share the first row -- needs the outer's full 100px content width, not the pre-fix 70px",
        inner_a.origin.y,
        inner_b.origin.y
    );
    assert!(
        inner_b.origin.x >= inner_a.origin.x + inner_a.size.w - 0.01,
        "inner float b (x={}) must sit at/after inner float a's right edge ({})",
        inner_b.origin.x,
        inner_a.origin.x + inner_a.size.w
    );
    assert!(
        inner_c.origin.y > inner_a.origin.y + 0.01,
        "3rd inner float (y={}) must wrap onto a new row below the first row (y={}) -- two 40px floats fill the 100px content width, no room for a third",
        inner_c.origin.y,
        inner_a.origin.y
    );
}

/// packet/acid1-content-box: `dt`/`dd`-shape exact-fit side-by-side floats.
/// Reproduces `fixtures/css1-float-5526c.html`'s dt/dd geometry, shrunk to
/// round numbers matching the fixture's own arithmetic exactly: a `dl`
/// with 470px of content width containing a `float:left` box whose
/// border-box is 80px wide and a `float:right` box (`margin-left:10px`)
/// whose border-box is 380px wide -- 80 + 10 + 380 = 470, a ZERO-slack
/// exact fit. Real UAs keep an exact edge-to-edge fit on the same line
/// (floats only wrap when they would actually overlap); this proves this
/// engine does too, rather than the two floats stacking vertically because
/// of sub-pixel arithmetic noise upstream of taffy's own (inclusive, `<=`)
/// float-fit comparison.
#[test]
fn exact_fit_floats_stay_side_by_side_not_stacked() {
    let html = r#"
        <div style="width:470px;">
            <div style="float:left;width:50px;height:280px;padding:10px;border:5px solid black;background-color:rgb(200,0,0);"></div>
            <div style="float:right;width:340px;height:270px;margin-left:10px;padding:10px;border:10px solid black;background-color:rgb(255,255,255);"></div>
        </div>
    "#;
    let fragments = render(html, 500.0);
    let left = box_with_bg(&fragments, RED);
    let right = box_with_bg(&fragments, Color::rgb(255, 255, 255));

    assert_eq!(left.size.w, 80.0, "left float's content-box width must be 50+2*10(padding)+2*5(border) = 80px");
    assert_eq!(right.size.w, 380.0, "right float's content-box width must be 340+2*10(padding)+2*10(border) = 380px");

    let overlap_y = left.origin.y < right.origin.y + right.size.h && right.origin.y < left.origin.y + left.size.h;
    assert!(
        overlap_y,
        "left (y={}) and right (y={}) floats must share the same row at an exact-fit 470px containing block -- an exact fit must NOT wrap",
        left.origin.y,
        right.origin.y
    );
    assert!(
        right.origin.x >= left.origin.x + left.size.w,
        "right float (x={}) must sit at/after left float's right edge ({}) -- no overlap",
        right.origin.x,
        left.origin.x + left.size.w
    );
}
