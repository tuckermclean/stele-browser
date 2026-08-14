//! Block flow + the taffy translation (P6, M2): map a [`LayoutNode`] tree
//! onto taffy (charter §158's flex substrate — block flow is degenerate
//! column flex, flexbox is native), run taffy's layout with our bespoke
//! inline engine hanging off measure-function leaves, and walk the result
//! back into paint-ordered [`Fragment`]s.
//!
//! ## Tree translation
//!
//! Each [`LayoutNode`] becomes one taffy node, with three shapes:
//!  - `Container` with only block/flex-level children (or none): a normal
//!    taffy container node; its own children are translated recursively.
//!  - Inline-level content (a run of `Text` and/or `display: inline`
//!    `Container` children, flattened recursively): folded into ONE taffy
//!    leaf per maximal run, whose measure function runs [`inline::layout_runs`].
//!    A `Container`'s children are scanned left to right; each maximal
//!    sub-run of inline-level children becomes one such leaf, so mixed
//!    block/inline children (rare in the M2 fixtures) still round-trip
//!    without dropping content, at the cost of not modeling real CSS
//!    anonymous-block-box splitting exactly.
//!  - `Replaced { intrinsic }`: a taffy leaf whose `Style.size` is set
//!    directly to the intrinsic px size (no measure function needed — the
//!    size never depends on available width). M2 emits a `Box` placeholder
//!    fragment for these (the frozen `Replaced` variant carries no pixel
//!    data — real image wiring is P9's fb backend).
//!
//! Flex containers (`display: flex`) never get the inline-run folding
//! treatment: every child is its own taffy child node (a bare `Text`/
//! `Replaced` child under a flex parent is auto-wrapped as its own
//! single-run leaf / fixed-size leaf, same as at the tree root).
//!
//! Scope calls (documented in the P6 report / DECISIONS): margin collapsing
//! is NOT implemented (each block's own margins apply independently — v1,
//! per the packet's explicit "collapsing is optional" allowance). Inline
//! elements (`<a>`, `<em>`, ...) do not paint their own background/border in
//! M2 — only block-level boxes get a `Box` fragment; only the text color and
//! font carried per `InlineRun` differs per inline element.

use taffy::prelude::{
    auto, length, percent, AlignItems as TAlignItems, AvailableSpace, Dimension as TDimension,
    Display as TDisplay, FlexDirection as TFlexDirection, FlexWrap as TFlexWrap,
    JustifyContent as TJustifyContent, LengthPercentage as TLengthPercentage,
    LengthPercentageAuto as TLengthPercentageAuto, NodeId as TNodeId, Rect as TRect, Size as TSize, Style as TStyle,
    TaffyTree,
};

use crate::layout::inline::{self, InlineRun};
use crate::layout::{BoxContent, Fragment, FragmentKind, LayoutNode, Point, Rect, Size};
use crate::style::computed::{
    AlignItems, AlignSelf, Display, FlexDirection, FlexWrap, JustifyContent, LengthPercentage, LengthPercentageAuto,
    Dimension as CssDimension,
};
use crate::style::ComputedStyle;
use crate::text::Metrics;

/// A width used to stand in for "no wrap" (taffy `AvailableSpace::MaxContent`)
/// when driving the bespoke inline engine, which wants a finite width. Large
/// enough that no real document line reaches it, small enough that summed
/// advances can't overflow `f32` for any sane fixture.
const MAX_CONTENT_WIDTH: f32 = 1.0e7;

/// The maximum `LayoutNode` nesting depth `translate_any`/
/// `translate_container_children`/`flatten_inline` will descend into.
///
/// This crate's own recursive tree walk (`translate_any` <-> `translate_container_children`,
/// `flatten_inline`, and `emit`), *and* taffy's own recursive
/// `compute_layout_with_measure`, all have per-level stack frames with no
/// built-in depth limit. A chain of nested `Container`s deep enough
/// (empirically ~200+ levels on this host's default thread stack — well
/// within reach of hostile/generated HTML: deeply nested quote threads,
/// WYSIWYG exports, old nested-table markup) blows the stack: a guard-page
/// fault (SIGABRT), not a catchable `panic!`, so `panic = "abort"` gives no
/// mitigation and the *process* aborts.
///
/// 100 is well under that ~180-200 empirical floor, leaving margin for
/// taffy's own per-level frames and for the smaller stack the musl i486
/// target may run with. Past the cap, `translate_any` stops descending and
/// treats the over-deep subtree as an empty leaf (a childless block box) —
/// a pathological (>100-deep) document degrades gracefully instead of
/// crashing, per the fallback-ladder ethos. Because the taffy tree (and the
/// `Built` side-tree `emit` walks) is only ever as deep as what `translate`
/// produced, capping `translate` bounds taffy's compute recursion AND
/// `emit`'s recursion too — one cap covers both walks.
const DEPTH_CAP: usize = 100;

/// A translated node: enough provenance back to the source [`LayoutNode`]
/// (by style reference) to emit the right fragment at the right rect once
/// taffy has computed final layout.
enum Built<'a> {
    Container { style: &'a ComputedStyle, taffy_id: TNodeId, children: Vec<Built<'a>> },
    Inline { taffy_id: TNodeId, runs: Vec<InlineRun> },
    Replaced { style: &'a ComputedStyle, taffy_id: TNodeId, intrinsic: Size },
}

impl Built<'_> {
    fn taffy_id(&self) -> TNodeId {
        match self {
            Built::Container { taffy_id, .. } | Built::Inline { taffy_id, .. } | Built::Replaced { taffy_id, .. } => {
                *taffy_id
            }
        }
    }
}

/// Lay `root` out into `viewport` using `metrics` for text, and return the
/// paint-ordered fragment vector. Total on any tree (see module docs and
/// `inline::layout_runs`'s own totality notes) — degenerate/non-finite
/// viewport sizes are floored to zero rather than propagated into taffy.
pub fn layout_tree<M: Metrics>(root: &LayoutNode, viewport: Size, metrics: &M) -> Vec<Fragment> {
    let mut taffy: TaffyTree<Vec<InlineRun>> = TaffyTree::new();
    let built = translate_any(root, &mut taffy, 0);

    let vw = finite_nonneg(viewport.w);
    let vh = finite_nonneg(viewport.h);
    let available = TSize {
        width: if vw > 0.0 { AvailableSpace::Definite(vw) } else { AvailableSpace::MaxContent },
        height: AvailableSpace::MaxContent,
    };

    // The root itself is stretched to the viewport width regardless of its
    // own `width` style (mirroring a UA-stylesheet-less `<html>`/root box
    // filling the window) — but only when the caller gave us a positive
    // viewport width; a zero/degenerate viewport still computes (shrinking
    // to content) rather than panicking.
    if vw > 0.0 {
        if let Ok(mut style) = taffy.style(built.taffy_id()).cloned() {
            style.size.width = length(vw);
            let _ = taffy.set_style(built.taffy_id(), style);
        }
    }
    let _ = vh; // height is always content-derived for the root (no fixed viewport clamp in M2)

    let _ = taffy.compute_layout_with_measure(
        built.taffy_id(),
        available,
        |known_dimensions, available_space, _node_id, node_context, _style| {
            if let TSize { width: Some(w), height: Some(h) } = known_dimensions {
                return TSize { width: w, height: h };
            }
            let Some(runs) = node_context else {
                return TSize::ZERO;
            };
            let avail_w = match known_dimensions.width {
                Some(w) => w,
                None => match available_space.width {
                    AvailableSpace::Definite(w) => w,
                    AvailableSpace::MaxContent => MAX_CONTENT_WIDTH,
                    AvailableSpace::MinContent => 0.0,
                },
            };
            let out = inline::layout_runs(runs, avail_w, metrics);
            TSize {
                width: known_dimensions.width.unwrap_or(out.size.w),
                height: known_dimensions.height.unwrap_or(out.size.h),
            }
        },
    );

    let mut fragments = Vec::new();
    emit(&built, &taffy, Point { x: 0.0, y: 0.0 }, metrics, &mut fragments);
    fragments
}

fn finite_nonneg(v: f32) -> f32 {
    if v.is_finite() && v > 0.0 {
        v
    } else {
        0.0
    }
}

/// Translate one `LayoutNode` (any content kind) into a taffy node.
/// `depth` is this node's own nesting depth (root = 0); see [`DEPTH_CAP`].
fn translate_any<'a>(node: &'a LayoutNode, taffy: &mut TaffyTree<Vec<InlineRun>>, depth: usize) -> Built<'a> {
    match &node.content {
        BoxContent::Text(text) => {
            let runs = vec![InlineRun { text: text.clone(), style: node.style.clone() }];
            let style = base_style(&node.style);
            let id = taffy
                .new_leaf_with_context(style, runs.clone())
                .expect("taffy leaf alloc is infallible for a fresh tree");
            Built::Inline { taffy_id: id, runs }
        }
        BoxContent::Replaced { intrinsic } => {
            let mut style = base_style(&node.style);
            let iw = finite_nonneg(intrinsic.w);
            let ih = finite_nonneg(intrinsic.h);
            style.size = TSize { width: length(iw), height: length(ih) };
            let id = taffy.new_leaf(style).expect("taffy leaf alloc is infallible for a fresh tree");
            Built::Replaced { style: &node.style, taffy_id: id, intrinsic: Size { w: iw, h: ih } }
        }
        BoxContent::Container => {
            let mut style = base_style(&node.style);
            style.display = map_display(node.style.display);
            apply_flex(&mut style, &node.style);
            // Past DEPTH_CAP, stop descending: an over-deep subtree becomes
            // an empty (childless) box rather than risking a stack
            // overflow. See DEPTH_CAP's doc comment.
            let children = if depth >= DEPTH_CAP || node.children.is_empty() {
                Vec::new()
            } else {
                translate_container_children(node, taffy, depth + 1)
            };
            let child_ids: Vec<TNodeId> = children.iter().map(Built::taffy_id).collect();
            let id = taffy
                .new_with_children(style, &child_ids)
                .expect("taffy container alloc is infallible for a fresh tree");
            Built::Container { style: &node.style, taffy_id: id, children }
        }
    }
}

/// True for the children a container folds into one inline formatting
/// context leaf: bare text, or a nested `display: inline` container.
///
/// M4 deferral (not built now, flagged per code review): `Replaced` is
/// always `false` here, so a non-floated inline replaced element (e.g.
/// `<p>Hello <img> World</p>` with no `align`) breaks out of its line and
/// stacks as its own block box instead of sitting inline between the
/// surrounding text. Real inline-replaced flow arrives with M4's image +
/// float work, when `Replaced` also carries real pixel data — no M2 fixture
/// (`basic.html`) has an inline image, so nothing is silently dropped yet,
/// but this is a known flow gap, not an oversight.
fn is_inline_ish(n: &LayoutNode) -> bool {
    match &n.content {
        BoxContent::Text(_) => true,
        BoxContent::Container => n.style.display == Display::Inline,
        BoxContent::Replaced { .. } => false,
    }
}

/// Flatten a node's inline-level content (itself, if it's `Text`; its
/// children recursively, if it's an inline `Container`) into `InlineRun`s in
/// document order. A `Replaced` child mixed into inline content is skipped
/// here — not built now (M4 scope: inline non-floated replaced content and
/// floats arrive together, when `Replaced` also gets real pixel data). A
/// *direct* `Replaced` child of the block container still gets its own box
/// via the grouping loop in `translate_container_children`; only a
/// `Replaced` *grandchild* nested inside an inline `Container` (e.g.
/// `<em><img></em>`) is silently dropped here — flagged, not fixed, since
/// no M2 fixture has inline images (that's `images.html`, M4).
///
/// `depth` mirrors `translate_any`'s cap (see [`DEPTH_CAP`]): this walk is
/// independent recursion (it never goes through `translate_any`), so it
/// needs its own bound against the same pathological-nesting case.
fn flatten_inline(node: &LayoutNode, out: &mut Vec<InlineRun>, depth: usize) {
    match &node.content {
        BoxContent::Text(text) => out.push(InlineRun { text: text.clone(), style: node.style.clone() }),
        BoxContent::Container => {
            if depth >= DEPTH_CAP {
                return; // over-deep inline subtree: drop gracefully, don't recurse further.
            }
            for child in &node.children {
                flatten_inline(child, out, depth + 1);
            }
        }
        BoxContent::Replaced { .. } => {}
    }
}

/// Translate a container's children, grouping maximal runs of inline-level
/// children into single IFC leaves and translating everything else (block
/// containers, replaced elements) as their own taffy nodes. `display: flex`
/// containers skip grouping entirely — every child is its own flex item.
/// `depth` is the depth at which `node`'s children themselves sit (already
/// incremented by the caller); see [`DEPTH_CAP`].
fn translate_container_children<'a>(
    node: &'a LayoutNode,
    taffy: &mut TaffyTree<Vec<InlineRun>>,
    depth: usize,
) -> Vec<Built<'a>> {
    let mut out = Vec::new();
    if node.style.display == Display::Flex {
        for child in &node.children {
            out.push(translate_any(child, taffy, depth));
        }
        return out;
    }

    let mut i = 0;
    while i < node.children.len() {
        if is_inline_ish(&node.children[i]) {
            let mut runs = Vec::new();
            let mut j = i;
            while j < node.children.len() && is_inline_ish(&node.children[j]) {
                flatten_inline(&node.children[j], &mut runs, depth);
                j += 1;
            }
            let style = TStyle { size: TSize { width: auto(), height: auto() }, ..Default::default() };
            let id = taffy
                .new_leaf_with_context(style, runs.clone())
                .expect("taffy leaf alloc is infallible for a fresh tree");
            out.push(Built::Inline { taffy_id: id, runs });
            i = j;
        } else {
            out.push(translate_any(&node.children[i], taffy, depth));
            i += 1;
        }
    }
    out
}

/// The box-model + display-independent parts of a taffy `Style` shared by
/// every node kind: size, margin, padding, border.
fn base_style(cs: &ComputedStyle) -> TStyle {
    TStyle {
        size: TSize { width: map_dimension(cs.width), height: map_dimension(cs.height) },
        margin: TRect {
            left: map_lpa(cs.margin.left),
            right: map_lpa(cs.margin.right),
            top: map_lpa(cs.margin.top),
            bottom: map_lpa(cs.margin.bottom),
        },
        padding: TRect {
            left: map_lp(cs.padding.left),
            right: map_lp(cs.padding.right),
            top: map_lp(cs.padding.top),
            bottom: map_lp(cs.padding.bottom),
        },
        border: TRect {
            left: TLengthPercentage::length(finite_nonneg(cs.border.left.width)),
            right: TLengthPercentage::length(finite_nonneg(cs.border.right.width)),
            top: TLengthPercentage::length(finite_nonneg(cs.border.top.width)),
            bottom: TLengthPercentage::length(finite_nonneg(cs.border.bottom.width)),
        },
        ..Default::default()
    }
}

fn apply_flex(style: &mut TStyle, cs: &ComputedStyle) {
    style.flex_direction = match cs.flex_direction {
        FlexDirection::Row => TFlexDirection::Row,
        FlexDirection::RowReverse => TFlexDirection::RowReverse,
        FlexDirection::Column => TFlexDirection::Column,
        FlexDirection::ColumnReverse => TFlexDirection::ColumnReverse,
    };
    style.flex_wrap = match cs.flex_wrap {
        FlexWrap::NoWrap => TFlexWrap::NoWrap,
        FlexWrap::Wrap => TFlexWrap::Wrap,
        FlexWrap::WrapReverse => TFlexWrap::WrapReverse,
    };
    style.justify_content = Some(match cs.justify_content {
        JustifyContent::FlexStart => TJustifyContent::FLEX_START,
        JustifyContent::FlexEnd => TJustifyContent::FLEX_END,
        JustifyContent::Center => TJustifyContent::CENTER,
        JustifyContent::SpaceBetween => TJustifyContent::SPACE_BETWEEN,
        JustifyContent::SpaceAround => TJustifyContent::SPACE_AROUND,
        JustifyContent::SpaceEvenly => TJustifyContent::SPACE_EVENLY,
    });
    style.align_items = Some(match cs.align_items {
        AlignItems::FlexStart => TAlignItems::FLEX_START,
        AlignItems::FlexEnd => TAlignItems::FLEX_END,
        AlignItems::Center => TAlignItems::CENTER,
        AlignItems::Stretch => TAlignItems::STRETCH,
        AlignItems::Baseline => TAlignItems::BASELINE,
    });
    style.align_self = match cs.align_self {
        AlignSelf::Auto => None,
        AlignSelf::FlexStart => Some(TAlignItems::FLEX_START),
        AlignSelf::FlexEnd => Some(TAlignItems::FLEX_END),
        AlignSelf::Center => Some(TAlignItems::CENTER),
        AlignSelf::Stretch => Some(TAlignItems::STRETCH),
        AlignSelf::Baseline => Some(TAlignItems::BASELINE),
    };
    let grow = if cs.flex_grow.is_finite() { cs.flex_grow.max(0.0) } else { 0.0 };
    let shrink = if cs.flex_shrink.is_finite() { cs.flex_shrink.max(0.0) } else { 1.0 };
    style.flex_grow = grow;
    style.flex_shrink = shrink;
    style.flex_basis = map_dimension(cs.flex_basis);
    let gap = if cs.gap.is_finite() { cs.gap.max(0.0) } else { 0.0 };
    style.gap = TSize { width: TLengthPercentage::length(gap), height: TLengthPercentage::length(gap) };
}

fn map_display(d: Display) -> TDisplay {
    match d {
        Display::None => TDisplay::None,
        Display::Block => TDisplay::Block,
        Display::Flex => TDisplay::Flex,
        // A `display: inline` container reaching its own taffy node means it
        // wasn't folded into a parent's IFC (e.g. it's the tree root, or a
        // bare child of a `display: flex` parent). Blockify it — a
        // documented M2 simplification; real anonymous inline-to-block
        // promotion is out of scope.
        Display::Inline => TDisplay::Block,
        // TODO(table-layout packet): real table layout via solve_table.
        // These four variants are the marker landed by the display-table
        // freeze amendment; box-tree/taffy integration with
        // `layout::table::solve_table` is deferred to the next packet. Until
        // then, table boxes fall back to stacked block boxes — visually
        // wrong for real tables, but total and green.
        Display::Table => TDisplay::Block,
        Display::TableRow => TDisplay::Block,
        Display::TableCell => TDisplay::Block,
        Display::TableRowGroup => TDisplay::Block,
    }
}

fn map_dimension(d: CssDimension) -> TDimension {
    match d {
        CssDimension::Px(v) if v.is_finite() => length(v.max(0.0)),
        CssDimension::Px(_) => auto(),
        CssDimension::Percent(p) if p.is_finite() => percent(p / 100.0),
        CssDimension::Percent(_) => auto(),
        CssDimension::Auto => auto(),
    }
}

fn map_lp(v: LengthPercentage) -> TLengthPercentage {
    match v {
        LengthPercentage::Px(p) if p.is_finite() => TLengthPercentage::length(p.max(0.0)),
        LengthPercentage::Px(_) => TLengthPercentage::length(0.0),
        LengthPercentage::Percent(p) if p.is_finite() => TLengthPercentage::percent(p / 100.0),
        LengthPercentage::Percent(_) => TLengthPercentage::length(0.0),
    }
}

fn map_lpa(v: LengthPercentageAuto) -> TLengthPercentageAuto {
    match v {
        LengthPercentageAuto::Px(p) if p.is_finite() => TLengthPercentageAuto::length(p),
        LengthPercentageAuto::Px(_) => TLengthPercentageAuto::length(0.0),
        LengthPercentageAuto::Percent(p) if p.is_finite() => TLengthPercentageAuto::percent(p / 100.0),
        LengthPercentageAuto::Percent(_) => TLengthPercentageAuto::length(0.0),
        LengthPercentageAuto::Auto => auto(),
    }
}

/// Walk `built` (already laid out by taffy) and push paint-ordered
/// fragments: a box's own background/border before its children, inline
/// text runs positioned within their line boxes, and replaced-element
/// placeholders.
///
/// Recursion depth here is bounded by construction: `built` is a `Built`
/// tree, and `translate_any` never produces one deeper than [`DEPTH_CAP`]
/// (it stops descending past the cap instead of recursing further), so this
/// walk inherits the same bound without needing its own check.
fn emit<M: Metrics>(
    built: &Built,
    taffy: &TaffyTree<Vec<InlineRun>>,
    parent_origin: Point,
    metrics: &M,
    out: &mut Vec<Fragment>,
) {
    let Ok(layout) = taffy.layout(built.taffy_id()) else { return };
    let origin = Point { x: parent_origin.x + layout.location.x, y: parent_origin.y + layout.location.y };
    let size = Size { w: layout.size.width.max(0.0), h: layout.size.height.max(0.0) };

    match built {
        Built::Container { style, children, .. } => {
            out.push(Fragment { rect: Rect { origin, size }, kind: FragmentKind::Box { style: (*style).clone() } });
            for child in children {
                emit(child, taffy, origin, metrics, out);
            }
        }
        Built::Replaced { style, .. } => {
            // M2 scope: no pixel data on the frozen `Replaced` node yet, so
            // a replaced element paints as a plain box at its intrinsic
            // rect (real image blitting is P9's fb backend).
            out.push(Fragment { rect: Rect { origin, size }, kind: FragmentKind::Box { style: (*style).clone() } });
        }
        Built::Inline { runs, .. } => {
            let available_w = size.w;
            let laid_out = inline::layout_runs(runs, available_w, metrics);
            for line in &laid_out.lines {
                for run in &line.runs {
                    let text_origin = Point {
                        x: origin.x + line.rect.origin.x + run.x,
                        y: origin.y + line.rect.origin.y,
                    };
                    out.push(Fragment {
                        rect: Rect {
                            origin: text_origin,
                            size: Size { w: run.width, h: line.rect.size.h },
                        },
                        kind: FragmentKind::Text {
                            text: run.text.clone(),
                            baseline: line.baseline,
                            style: runs[run.run_index].style.clone(),
                        },
                    });
                }
            }
        }
    }
}
