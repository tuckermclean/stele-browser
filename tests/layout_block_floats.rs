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
//! `<div>`s as the ONLY body content (no intermediate width-narrower wrapper
//! `<div>`) so each float's containing block is `<body>` itself (UA sheet
//! default `body { margin: 8px; }`, `src/style/ua.rs:39`) -- this sidesteps
//! a documented taffy 0.13 float_layout rough edge (`compute/block.rs`'s own
//! "TODO: handle nested blocks with different widths" comment) that isn't
//! exercised by `fixtures/css1-float-5526c.html` either (its own floated
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

/// A `float: right` block sits at its containing block's (here, `<body>`'s
/// own content box, UA-sheet `margin: 8px` on every side) RIGHT edge.
#[test]
fn float_right_block_sits_at_containing_blocks_right_edge() {
    let html = r#"<div style="float:right;width:50px;height:20px;background-color:rgb(200,0,0);"></div>"#;
    let viewport_w = 300.0;
    let fragments = render(html, viewport_w);

    let r = box_with_bg(&fragments, RED);
    assert_eq!(r.size, Size { w: 50.0, h: 20.0 });

    // body's own right content edge = viewport width - the UA sheet's 8px
    // margin (src/style/ua.rs:39).
    let body_right_edge = viewport_w - 8.0;
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
/// the OUTER (viewport/body) containing block's width. Four 40px-wide
/// inner floats (160px total) do not fit in the 150px-wide floated parent,
/// so the 4th must wrap onto a new row -- if the inner floats were (wrongly)
/// resolving against the outer body content width (300 - 16px margin =
/// 284px, comfortably >= 160px) all four would fit on one row and none
/// would wrap. A floated block always establishes its own new block
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
