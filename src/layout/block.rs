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
//! Scope calls (documented in the P6 report / DECISIONS): inline elements
//! (`<a>`, `<em>`, ...) do not paint their own background/border in M2 —
//! only block-level boxes get a `Box` fragment; only the text color and font
//! carried per `InlineRun` differs per inline element.
//!
//! ## Margin collapsing (D6, packet/t6-margin-collapse)
//!
//! Two adjacent block siblings each carrying their own top/bottom margin
//! got a DOUBLED gap pre-`t6` — a real CSS engine collapses adjoining
//! margins to `max(prev.margin_bottom, next.margin_top)` (CSS2.1 §8.3.1),
//! not their sum. The surprise (discovered mid-packet, see the extensive
//! comment on [`compute_sibling_margin_overrides`] for the full story): the
//! vendored taffy (0.13) `Display::Block` algorithm is NOT margin-collapsing
//! -naive — it already implements REAL CSS collapsing natively. But it
//! collapses every adjoining `Display::Block` sibling pair UNCONDITIONALLY;
//! it has no concept of `float` at all (this engine never communicates float
//! to taffy for block boxes) and its own border/padding checks only gate a
//! DIFFERENT question (whether a box can be "collapsed through" as a
//! pass-through for its PARENT's own margin) — never whether two ordinary
//! siblings' margins should adjoin at all. So taffy's native behavior is
//! "always collapse", not "never collapse" — the opposite problem from what
//! this module's original (pre-fix) doc comment assumed, and NEITHER
//! matches the exclusions this packet's contract wants.
//!
//! [`compute_sibling_margin_overrides`] runs a pre-pass OUTSIDE taffy, over
//! each non-flex container's own in-flow children, and rewrites the margin
//! values fed to taffy so taffy's own native collapsing can never produce
//! anything other than what this function decided: the earlier sibling's
//! margin-bottom becomes the FINAL desired gap (`max` when the pair should
//! collapse, the ordinary `sum` when it shouldn't), and the later sibling's
//! margin-top becomes zero — collapsing anything with a true zero is a
//! no-op (`max(x, 0) == x` for the non-negative margins handled here), so
//! whatever taffy's own algorithm does on top of these values afterward
//! can't change the committed gap.
//!
//! In scope: adjacent in-flow block-level siblings only (CSS2.1's "adjoining
//! margins"). A pair collapses only when nothing else separates them: no
//! border or padding on either box's touching edge (a visible border/
//! non-zero padding always ends adjoinment — this packet's own contract,
//! not literal upstream CSS2.1, which only applies that exclusion to
//! parent/child adjoining, not sibling/sibling; implemented here regardless
//! since taffy's native behavior needs overriding either way), and only
//! whitespace-only text may sit between them in the DOM (real content, a
//! float, a flex item, or a table-internal display — `table`/`table-row`/
//! `table-row-group`/`table-cell`, which belong to the TABLE formatting
//! context, not block flow — breaks the chain; see [`is_collapse_eligible_
//! block`]). Percentage margins are left alone (taffy's own native
//! collapsing behavior applies, unmodified): resolving a percentage margin
//! needs the containing block's width, which isn't known yet at this
//! translate-time pre-pass (only taffy's later layout pass knows it) —
//! `auto` margins (which always compute to `0` for a top/bottom margin in
//! this engine's normal-flow-only box model) collapse normally.
//!
//! Out of scope, NOT implemented by this packet's own pre-pass: a block's
//! own margin collapsing with its first/last in-flow child's margin (CSS2.1
//! §8.3.1's other collapsing case). Note this is subtly different from the
//! sibling case: taffy's native `Display::Block` algorithm already performs
//! SOME parent/child collapsing entirely on its own (`own_margins_collapse_
//! with_children` in taffy's source — a container with zero top/bottom
//! padding/border lets its own margin merge with its first/last child's),
//! independent of and unaffected by anything in this file — this packet
//! neither adds nor removes that behavior, and it predates `t6` (see the
//! parenthetical below: earlier packets' tests already defended against it
//! empirically, before this packet's own diagnosis of taffy's native
//! collapsing existed). What IS out of scope is this pre-pass reaching in
//! to CONTROL parent/child collapsing the way it controls sibling
//! collapsing (e.g. to add this packet's own border/padding/float-style
//! exclusions to that case too) — doing that properly would need parent
//! height/child-origin adjustments threaded back through `emit`, not just a
//! `TStyle.margin` tweak, since the child's margin can move outside the
//! parent's own committed border-box in a way `MarginOverride` alone can't
//! express. Left as a documented follow-up rather than shipped half-working.
//! (The existing `nested_margin_padding_border_produce_expected_rects` /
//! `asymmetric_margins_are_honored` tests in `tests/layout_block.rs`
//! deliberately give their outer container nonzero padding — "blocks
//! parent/child margin collapsing" — which was already true defensively
//! before this packet and remains true now that parent/child collapsing is
//! still unimplemented.)

use std::cell::RefCell;

use taffy::prelude::{
    auto, fr, length, minmax, percent, repeat, AlignItems as TAlignItems, AvailableSpace, Dimension as TDimension,
    Display as TDisplay, FlexDirection as TFlexDirection, FlexWrap as TFlexWrap,
    GridTemplateComponent as TGridTemplateComponent, JustifyContent as TJustifyContent,
    LengthPercentage as TLengthPercentage, LengthPercentageAuto as TLengthPercentageAuto,
    MaxTrackSizingFunction as TMaxTrackSizingFunction, MinTrackSizingFunction as TMinTrackSizingFunction,
    NodeId as TNodeId, Rect as TRect, RepetitionCount as TRepetitionCount, Size as TSize, Style as TStyle,
    TaffyAuto, TaffyTree, TrackSizingFunction as TTrackSizingFunction,
};
// packet/block-floats: `float_layout`'s `Float`/`Clear` aren't re-exported
// from `taffy::prelude` (only flexbox/grid additions are) -- pull them from
// the crate root, same seam the proving spike (spike/taffy-float-layout)
// used.
use taffy::{BoxSizing as TBoxSizing, Clear as TClear, Float as TFloat, Position as TPosition};

use crate::layout::inline::{self, InlineContent, InlineRun};
use crate::layout::table::{self, CellSpec, TableLayout, TableSpec};
use crate::layout::table_layout;
use crate::layout::{BoxContent, Fragment, FragmentKind, Interactive, LayoutNode, Point, Rect, Size};
use crate::style::computed::{
    AlignItems, AlignSelf, BorderCollapse, BorderSide, BorderStyle, BoxSizing, Clear, Display, FlexDirection,
    FlexWrap, Float, GridRepetitionCount, GridTemplateComponent, GridTrack, GridTrackSize, JustifyContent,
    LengthPercentage, LengthPercentageAuto, Dimension as CssDimension, Overflow, Position, TextAlign,
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

/// The maximum number of *tables nested inside table cells* `translate_any`
/// will treat as a real table (build a leaf for + run
/// [`compute_table_cache_entry`] on). Distinct from [`DEPTH_CAP`]:
/// `DEPTH_CAP` bounds one `LayoutNode` parent/child chain's recursion within
/// a single `translate_any` walk, but each table's cell content is
/// measured/laid out via its OWN fresh `translate_any` walk (see
/// `cell_min_max_width`/`cell_content_layout`) — walks that are themselves
/// invoked recursively (from inside the OUTER table's own measure/emit)
/// whenever a cell contains another table. A pathological "table in a cell
/// in a table in a cell in ..." bomb would otherwise nest these walks (and
/// the real native call stack frames that come with each:
/// `compute_layout_with_measure`, `emit`, `translate_any`) without limit —
/// a guard-page fault (SIGABRT), same failure class `DEPTH_CAP` exists to
/// prevent, just via a different recursion path. Once the budget hits `0`,
/// `translate_any` stops treating a `Display::Table` node as a table at
/// all: it falls back to the pre-existing plain-block translation (see
/// `map_display`'s `Display::Table => TDisplay::Block` arm) — an over-deep
/// nested table degrades to its rows/cells rendering as stacked blocks, not
/// a crash.
///
/// Deliberately small (2, not e.g. 8): the guard isn't just against native
/// *stack* depth — solving one table leaf calls `cell_min_max_width` (two
/// full sub-layouts: a `MinContent` and a `MaxContent` taffy query) and
/// `cell_content_layout` (one more, for the real height) PER CELL, and
/// `measure_node` itself may run several times per node as taffy's own
/// algorithm iterates — so the total work below a table-containing-cell
/// multiplies by a small constant factor with EACH nesting level, not just
/// adds. A budget of 8 was measured (the hard way, via a runaway ~15-minute
/// test run) to blow past any reasonable CI time budget; 2 keeps the total
/// work bounded to a small, fast constant for any input, while still
/// rendering the common "one table nested inside another" case for real
/// (a doubly/triply-nested table is rare even in gnarly 1996 markup, and
/// degrading it to stacked blocks is a documented, acceptable M3
/// simplification — see the packet report). This bounds the NESTING axis;
/// [`MAX_TABLE_MEASURED_CELLS`] bounds the WIDTH axis (one table with many
/// cells, no nesting at all) — a distinct cost, capped separately below.
const TABLE_DEPTH_CAP: usize = 2;

/// The maximum number of grid cells (`columns * rows`, effectively — really
/// "however many cells `table_layout::place_grid` actually places")
/// `translate_any` will run the EXPENSIVE per-cell taffy measurement
/// pipeline on. Distinct from `table_layout::place_grid`'s own
/// `MAX_GRID_CELLS` (262_144): that cap only bounds place_grid's own cheap
/// pure-arithmetic bookkeeping (an occupied-slot bitset), sized for that
/// cost. THIS cap bounds a completely different, far more expensive cost:
/// `cell_min_max_width` (two fresh `TaffyTree` + `compute_layout_with_measure`
/// sub-layouts) and `cell_content_layout` (one more) PER CELL — each a real
/// (if individually small) allocation + layout pass, not arithmetic.
/// Empirically (a throwaway stress harness, not committed): a flat
/// (non-nested) table scales roughly linearly at this cost, but the
/// constant factor is large enough that 20_000 cells already took ~4
/// seconds; a "large spreadsheet export" table with tens of thousands of
/// plain `<td>`s (not exotic, not adversarial — just large) would run well
/// past any reasonable time budget, and place_grid's own 262_144-cell cap
/// does nothing to stop it (that cap exists for a DIFFERENT, much cheaper
/// operation). Past this cap, `translate_any` doesn't build a table leaf at
/// all — same graceful degrade as an exhausted `TABLE_DEPTH_CAP`: the
/// table's rows/cells fall through to plain stacked blocks (see
/// `map_display`'s `Display::Table => TDisplay::Block` arm), total and
/// bounded rather than fast-only-until-the-input-is-big-enough. 2_000 is
/// comfortably past any real 1996-era hand-authored data table (these run
/// to hundreds of cells, not thousands) while keeping the worst case (this
/// many cells, each paying the full ~3x-per-cell measurement cost even
/// after the caching fix below) a small, fast, fixed constant.
const MAX_TABLE_MEASURED_CELLS: usize = 2_000;

/// The default horizontal gap between adjacent columns, absent any CSS
/// `border-spacing`/`cellspacing` on the table — see
/// `style::ComputedStyle::border_spacing_x`'s own doc comment (packet/
/// table-spacing FREEZE AMENDMENT) for why this is `8.0` (one full
/// `text::BitmapFont::vga_8x16` cell) rather than CSS's real `2px` initial
/// value: `backend::tty::render` maps continuous layout-pixel x-coordinates
/// to discrete character columns by *rounding* to the nearest cell (`col =
/// round(x / 8.0)`), so any sub-cell gap smaller than half a cell (4px) can
/// round away to nothing whenever the preceding column's own content
/// already exactly fills its whole-character width — e.g. a 2px gap after a
/// column whose widest cell is exactly N characters wide rounds `col0_width
/// + 2` right back down to the same cell `col0_width` lands on, so the next
/// column's text visually touches it with zero rendered gap. A full 8px
/// cell-width gap has no such rounding ambiguity (`round((w + 8) / 8) ==
/// round(w / 8) + 1`, always). Vertical spacing stays `0` by default — rows
/// are already visually separated by moving to a new tty row. These two
/// numbers now live SOLELY as `ComputedStyle::default()`'s
/// `border_spacing_x`/`border_spacing_y` values (no longer duplicated as
/// module-private constants here) — `compute_table_cache_entry`/
/// `measure_node` read a table's own resolved style directly, which falls
/// back to that same default when no `border-spacing`/`cellspacing` was
/// ever set.

/// A table leaf's fully-solved layout: column/row geometry plus each cell's
/// own content (size + paint-ordered fragments, relative to its own `(0,
/// 0)` border-box origin — see `cell_content_layout`). `cell_content[i]`
/// corresponds to `table_layout.cell_rects[i]`, both indexed in the same
/// order `table_layout::place_grid` produces (place_grid is a pure,
/// deterministic function of the table's `LayoutNode`, so recomputing it —
/// cheap, no taffy involved — always reproduces the same order; this cache
/// doesn't need to also store the `Grid` itself).
///
/// This is the fix for the Critical (per-cell measurement cost is
/// unbounded/redundant) flagged in review: computed ONCE by
/// `compute_table_cache_entry` and reused by both `measure_node` (peeks the
/// sizes) and `emit` (consumes the fragments) for a given `avail_w`, rather
/// than each re-running the full per-cell taffy sub-layout pipeline from
/// scratch — see `ensure_table_cache`.
struct TableCacheEntry {
    /// The `available_width` this entry was solved at (see `ensure_table_cache`
    /// — a cached entry is only reused when a new query's `avail_w` matches
    /// this one; `solve_table`'s column resolution genuinely depends on it).
    avail_w: f32,
    columns: usize,
    rows: usize,
    table_layout: TableLayout,
    cell_content: Vec<(Size, Vec<Fragment>)>,
    /// packet/collapse-geometry: the per-axis CELL border width
    /// (`collapse_cell_border_widths`) this entry's `table_layout.cell_rects`
    /// were already adjusted with, when `border_collapse == Collapse`
    /// (`(0.0, 0.0)` — a no-op — for a `Separate` table). `measure_node`'s
    /// `NodeCtx::Table` arm reuses these (rather than re-walking `grid`) to
    /// compute the table's own collapsed total content size.
    collapse_bw_x: f32,
    collapse_bw_y: f32,
}

/// The taffy node-context type: either a folded inline-formatting-context
/// leaf's runs (pre-existing, P6), or a table leaf's source `LayoutNode`,
/// remaining nested-table budget (see [`TABLE_DEPTH_CAP`]), and its lazily-
/// computed, cached solve (see [`TableCacheEntry`]). A table's cell
/// content, min/max-content widths, and row heights are NOT computed eagerly
/// at translate time (the table's own final width depends on parent-supplied
/// available space, only known once taffy visits this leaf during layout) —
/// so, exactly like the pre-existing text/inline leaves, a table leaf defers
/// its real sizing work to the measure function (`measure_node`), which
/// pattern-matches this enum. The `RefCell` gives `emit` (which only holds a
/// shared `&TaffyTree`) the same interior-mutable access to the cache slot
/// that `measure_node` (which taffy calls with `&mut NodeCtx`) has.
enum NodeCtx<'a> {
    /// `TextAlign` is the containing block's own (already-inherited, per
    /// `cascade.rs`) value — threaded straight through from the
    /// `LayoutNode` whose inline-level children this leaf folds together
    /// (see both `translate_any`'s bare-`Text` arm and
    /// `translate_container_children`'s grouping loop) so
    /// `inline::layout_runs` can align each completed line without
    /// re-deriving it from any individual run's own style (a run's style
    /// governs its own font/color, not the containing block's alignment).
    Inline(Vec<InlineRun>, TextAlign),
    Table(&'a LayoutNode, usize, RefCell<Option<TableCacheEntry>>),
}

/// A translated node: enough provenance back to the source [`LayoutNode`]
/// (by style reference) to emit the right fragment at the right rect once
/// taffy has computed final layout.
enum Built<'a> {
    Container {
        style: &'a ComputedStyle,
        taffy_id: TNodeId,
        children: Vec<Built<'a>>,
        interactive: Option<Interactive>,
        /// See [`LayoutNode::id`] — copied from the source node the same way
        /// `interactive` is (Acid2 scroll-to-fragment packet, spec §1/§2).
        id: Option<Box<str>>,
    },
    Inline { taffy_id: TNodeId, runs: Vec<InlineRun>, text_align: TextAlign },
    Replaced {
        style: &'a ComputedStyle,
        taffy_id: TNodeId,
        intrinsic: Size,
        image: Option<std::rc::Rc<crate::img::RgbaImage>>,
        interactive: Option<Interactive>,
        /// See [`Built::Container::id`]'s doc comment.
        id: Option<Box<str>>,
    },
    /// A `display: table` box, translated as a single bespoke leaf (module
    /// docs). `emit` fetches the table's `LayoutNode`, nested-table budget,
    /// and cached solve back out of the taffy tree's own node context for
    /// this leaf (`taffy.get_node_context(taffy_id)`) rather than
    /// duplicating them here — see `NodeCtx::Table`/[`TableCacheEntry`].
    Table { style: &'a ComputedStyle, taffy_id: TNodeId },
}

impl Built<'_> {
    fn taffy_id(&self) -> TNodeId {
        match self {
            Built::Container { taffy_id, .. }
            | Built::Inline { taffy_id, .. }
            | Built::Replaced { taffy_id, .. }
            | Built::Table { taffy_id, .. } => *taffy_id,
        }
    }
}

/// Lay `root` out into `viewport` using `metrics` for text, and return the
/// paint-ordered fragment vector. Total on any tree (see module docs and
/// `inline::layout_runs`'s own totality notes) — degenerate/non-finite
/// viewport sizes are floored to zero rather than propagated into taffy.
pub fn layout_tree<M: Metrics>(root: &LayoutNode, viewport: Size, metrics: &M) -> Vec<Fragment> {
    layout_tree_impl(root, viewport, metrics, false)
}

/// [`layout_tree`]'s opt-in fixed-viewport sibling (packet/fixed-viewport):
/// identical content-driven WIDTH clamp, but ALSO clamps the root's HEIGHT
/// to `viewport.h` (when positive) the same way — so a document root with
/// `overflow: hidden` (P5's own clip machinery) clips its descendants into a
/// fixed `viewport.w` x `viewport.h` window instead of sprawling to content
/// height. Every existing caller keeps using [`layout_tree`] (clamp_height =
/// `false`, unchanged behavior byte-for-byte); this is purely additive.
pub fn layout_tree_viewport<M: Metrics>(root: &LayoutNode, viewport: Size, metrics: &M) -> Vec<Fragment> {
    layout_tree_impl(root, viewport, metrics, true)
}

/// The shared implementation behind [`layout_tree`] (`clamp_height = false`,
/// today's content-height behavior, unchanged) and [`layout_tree_viewport`]
/// (`clamp_height = true`, packet/fixed-viewport). See [`layout_tree`]'s own
/// doc comment for the width-clamp rationale this mirrors for height.
fn layout_tree_impl<M: Metrics>(root: &LayoutNode, viewport: Size, metrics: &M, clamp_height: bool) -> Vec<Fragment> {
    let mut taffy: TaffyTree<NodeCtx> = TaffyTree::new();
    let built = translate_any(root, &mut taffy, 0, TABLE_DEPTH_CAP, MarginOverride::default());

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
    //
    // packet/acid1-content-box: `box_sizing` is forced to `BorderBox` for
    // this ONE synthetic assignment, regardless of `box_sizing_for`'s
    // otherwise-CSS-correct `ContentBox` default (and regardless of
    // whatever the page itself declared). `vw` is unambiguously a
    // BORDER-BOX target -- "the root's rendered box fills the viewport,
    // padding/border inward, content shrinking to accommodate" -- the same
    // way a real browser's initial containing block works, and the same
    // way a real CSS `width: auto` block stretch-fit ALREADY works
    // regardless of `box-sizing` (that property only ever reinterprets an
    // EXPLICIT declared length, never the auto/stretch-fit case -- see
    // `box_sizing_for`'s own doc comment, which this comment complements
    // rather than duplicates). Before this line existed, every node
    // defaulted to taffy's own `BoxSizing::BorderBox`, so setting a bare
    // `style.size.width` here was implicitly already correct; now that
    // `ContentBox` is the real default, the SAME assignment would silently
    // get reinterpreted as a CONTENT width, growing the root's border-box
    // PAST the viewport by its own padding+border (caught by `tests/
    // layout_block.rs`'s `nested_margin_padding_border_produce_expected_
    // rects`: a 1px-padding-all-round root grew from the correct 300 to a
    // wrong 302 -- exactly `2 * 1px` padding -- before this fix).
    // packet/fixed-viewport: when the caller opted into the fixed-viewport
    // path (`clamp_height`) AND gave a positive `vh`, the root's HEIGHT is
    // ALSO pinned to the viewport — same synthetic BorderBox assignment as
    // the width clamp directly above (see that block's own comment for why
    // BorderBox, not whatever `box_sizing_for` would otherwise resolve, is
    // the correct target here), done in the SAME style-clone/set_style round
    // trip so both axes land atomically. `layout_tree` (clamp_height =
    // `false`) never takes this branch — `vh` stays inert for it, exactly as
    // before this packet (every existing golden's root is still pure
    // content-height).
    if vw > 0.0 || (clamp_height && vh > 0.0) {
        if let Ok(mut style) = taffy.style(built.taffy_id()).cloned() {
            if vw > 0.0 {
                style.size.width = length(vw);
                style.box_sizing = TBoxSizing::BorderBox;
            }
            if clamp_height && vh > 0.0 {
                style.size.height = length(vh);
                style.box_sizing = TBoxSizing::BorderBox;
            }
            let _ = taffy.set_style(built.taffy_id(), style);
        }
    }

    let _ = taffy.compute_layout_with_measure(
        built.taffy_id(),
        available,
        |known_dimensions, available_space, node_id, node_context, style| {
            measure_node(known_dimensions, available_space, node_id, node_context, style, metrics)
        },
    );

    // Acid2 scroll-to-fragment packet, Task 3: the initial containing
    // block's origin is `(0, 0)` by definition -- the SAME value already
    // passed as this call's `parent_origin` below, computed once up front so
    // `emit`'s `Container` arm can reparent a `position:fixed` child onto it
    // regardless of how deep that child sits. `viewport_clip` mirrors
    // exactly what the root's OWN `Container` arm invocation would derive as
    // `child_clip` for its immediate children (`intersect_clip`, with no
    // ancestor clip to intersect against) -- i.e. the root's own border box,
    // IF the root itself is `overflow: hidden`, else `None` (unclipped).
    let viewport_origin = Point { x: 0.0, y: 0.0 };
    let viewport_clip = built_style(&built).filter(|s| s.overflow == Overflow::Hidden).and_then(|_| {
        taffy.layout(built.taffy_id()).ok().map(|l| Rect {
            origin: viewport_origin,
            size: Size { w: l.size.width.max(0.0), h: l.size.height.max(0.0) },
        })
    });

    let mut fragments = Vec::new();
    emit(&built, &taffy, viewport_origin, metrics, &mut fragments, None, false, viewport_origin, viewport_clip);
    fragments
}

/// A `Built` node's own `ComputedStyle`, when it has one (`Built::Inline`
/// doesn't carry an independent style of its own -- see that variant's own
/// doc comment) — used by [`layout_tree_impl`] to read the ROOT's own
/// `overflow` when computing `viewport_clip` (Acid2 scroll-to-fragment
/// packet, Task 3, spec §3).
fn built_style<'a>(built: &'a Built) -> Option<&'a ComputedStyle> {
    match built {
        Built::Container { style, .. } | Built::Replaced { style, .. } | Built::Table { style, .. } => Some(style),
        Built::Inline { .. } => None,
    }
}

/// `pub(crate)` (not `pub`, and not just `fn`): Acid2 scroll-to-fragment
/// packet's `layout::find_fragment_top` reuses this exact clamp for a
/// `Fragment`'s border-top width, rather than re-deriving an equivalent
/// helper — same totality posture (`is_finite() && >= 0.0`, floor to `0.0`
/// otherwise) this module already applies to every other geometry input.
pub(crate) fn finite_nonneg(v: f32) -> f32 {
    if v.is_finite() && v > 0.0 {
        v
    } else {
        0.0
    }
}

/// The `(x, y)` border-spacing gap `table_node`'s own resolved style feeds
/// the table solver, per CSS `border-collapse` (packet/border-collapse): a
/// `Collapse` table ignores `border-spacing`/`cellspacing` entirely (CSS
/// spec behavior -- adjacent cells become flush against each other, sharing
/// one border line, see `layout::box_tree`'s collapse-dedup step for the
/// border-sharing half of this), so this returns `(0.0, 0.0)` regardless of
/// what `border_spacing_x/y` resolved to. A `Separate` table (the default --
/// every table that doesn't opt into `border-collapse: collapse`) is
/// byte-identical to before this packet: straight `finite_nonneg` off the
/// table's own resolved `border_spacing_x/y` (see `ComputedStyle::
/// border_spacing_x`'s own doc comment).
fn effective_border_spacing(style: &ComputedStyle) -> (f32, f32) {
    if style.border_collapse == BorderCollapse::Collapse {
        (0.0, 0.0)
    } else {
        (finite_nonneg(style.border_spacing_x), finite_nonneg(style.border_spacing_y))
    }
}

// ---------------------------------------------------------------------------
// packet/collapse-geometry: `border-collapse: collapse` shared-grid-line cell
// geometry. Replaces the earlier (removed) `box_tree::apply_border_collapse`
// dedup step, which zeroed each cell's right/bottom border and leaned on the
// table's own frame box to close off the grid — architecturally wrong (see
// that removed function's own former doc comment, preserved in DECISIONS):
// it doubled a bare `<table border>`'s top/left edge to 2px (the frame AND
// the first cell's own top/left both drew), and it lost the right/bottom
// outer edge entirely on any collapsed table with NO table-level border
// (kitchen-sink's shape) since zeroing was the only thing that ever drew
// those edges.
//
// The fix keeps every cell's FULL 4-side border (no style mutation at all)
// and instead POSITIONS cells so adjacent borders coincide on the same
// pixel. `raster.rs`'s own border painter draws each side INSET from the
// box's own edge (e.g. a right border occupies the box's own last
// `border_width` pixels, not pixels just past it) — which means two boxes
// that merely ABUT (zero gap, zero overlap, the "textbook" grid-line tiling
// `layout::table::solve_table` already does for `border-spacing`) do NOT
// produce a coincident line under that painter: box A's right border and
// box B's (immediately following) left border land on two DIFFERENT,
// adjacent pixel columns/rows — a 2px-wide seam, not a shared 1px line.
// Coincidence instead requires the two boxes to OVERLAP by exactly one
// border-width at their shared edge, so each box's own inset border paints
// the SAME pixel range. See `collapse_grid_lines`/`collapse_cell_extent`'s
// own doc comments for the exact (closed-form, algebraically verified —
// see the JOURNAL/DECISIONS entry) construction, and `tests/layout_table.rs`'s
// "packet/collapse-geometry" section for the pixel-exact proof.
//
// Scope (documented, matches the packet brief): a single uniform border
// width per axis is assumed — read as the MAX actually-visible cell border
// width on that axis (`collapse_cell_border_widths`) and, separately, the
// table's own frame border width if it has one (`collapse_table_border_widths`).
// Genuinely differing per-cell border widths/styles would need real CSS
// border-conflict resolution (widest/style-priority wins per shared edge) —
// out of scope, same documented limitation the removed dedup step already
// carried.
// ---------------------------------------------------------------------------

/// `true` iff `side` will actually paint a visible line — mirrors
/// `backend::raster::border_px`'s own gate (`Solid` style, finite positive
/// width) — so collapse geometry only shifts/overlaps cells to coincide with
/// a border that will really be drawn, never for an invisible (`None`-style
/// or zero-width) side.
fn paints_visible_border(side: &BorderSide) -> bool {
    side.style == BorderStyle::Solid && side.width.is_finite() && side.width > 0.0
}

fn effective_border_width(side: &BorderSide) -> f32 {
    if paints_visible_border(side) {
        side.width
    } else {
        0.0
    }
}

/// The uniform per-axis CELL border width used for collapse geometry: the
/// max actually-visible left/right (x) or top/bottom (y) border width across
/// every cell in `grid` — see this section's own "Scope" note. `(0.0, 0.0)`
/// if no cell has any visible border on that axis (collapse geometry then
/// reduces to a no-op: every `collapse_grid_lines`/`collapse_cell_extent`
/// call below degrades to the plain, non-overlapping tiling `solve_table`
/// already produces).
fn collapse_cell_border_widths(grid: &table_layout::Grid) -> (f32, f32) {
    let mut bw_x = 0.0f32;
    let mut bw_y = 0.0f32;
    for gc in &grid.cells {
        let b = &gc.node.style.border;
        bw_x = bw_x.max(effective_border_width(&b.left)).max(effective_border_width(&b.right));
        bw_y = bw_y.max(effective_border_width(&b.top)).max(effective_border_width(&b.bottom));
    }
    (bw_x, bw_y)
}

/// The table's own frame border width per axis (`0.0` if the table itself
/// has no visible border on that axis — e.g. any CSS-only-collapsed table
/// with no `border` on the `<table>` element itself, like kitchen-sink's
/// shape): used to shift the WHOLE cell grid so it overlaps the table's own
/// frame instead of sitting flush against its inner (content-box) edge — see
/// `emit`'s `Built::Table` arm and `measure_node`'s `NodeCtx::Table` arm,
/// both gated on `border_collapse == Collapse`.
fn collapse_table_border_widths(style: &ComputedStyle) -> (f32, f32) {
    let bw_x = effective_border_width(&style.border.left).max(effective_border_width(&style.border.right));
    let bw_y = effective_border_width(&style.border.top).max(effective_border_width(&style.border.bottom));
    (bw_x, bw_y)
}

/// Grid-line boundary positions for `border-collapse: collapse` geometry:
/// `lines[k]` is the pixel offset (relative to column/row `0`'s own
/// un-collapsed start) of the boundary BEFORE column/row `k`, for `k` in
/// `0..=widths.len()`. Every INTERIOR boundary (there are
/// `widths.len().saturating_sub(1)` of them — indices `1..widths.len()`) is
/// pulled in by exactly `bw`; the outermost boundary (`k == 0` and `k ==
/// widths.len()`) is left at its natural (un-pulled) position, since there's
/// no neighboring cell there to share a line with (see this section's own
/// module doc for why a naive "pull in EVERY boundary including the outer
/// ones by `k * bw`" formula over-shrinks a single-column/row table, or any
/// table's true outer edge, by one spurious border-width).
///
/// Total: `widths` (already `solve_table`'s own finite/non-negative output)
/// is defensively re-sanitized anyway; `bw` non-finite/negative sanitizes to
/// `0.0` (a no-op shift). Never empty — always `widths.len() + 1` entries,
/// so `lines[widths.len()]` (the far/total boundary) is always valid to
/// index.
fn collapse_grid_lines(widths: &[f32], bw: f32) -> Vec<f32> {
    let n = widths.len();
    let bw = finite_nonneg(bw);
    let cap = n.saturating_sub(1);
    let mut lines = Vec::with_capacity(n + 1);
    lines.push(0.0f32);
    let mut s = 0.0f32;
    for (k, w) in widths.iter().enumerate() {
        s += finite_nonneg(*w);
        let shrink = (k + 1).min(cap) as f32 * bw;
        lines.push((s - shrink).max(0.0));
    }
    lines
}

/// One cell's collapse-adjusted `(offset, length)` along one axis: `lines`
/// is that axis's [`collapse_grid_lines`] output, `start`/`span` the cell's
/// column/row origin and colspan/rowspan (already grid-clamped by
/// `table_layout::place_grid`), `bw` that axis's border width.
///
/// Every cell that does NOT reach the axis's far edge (`start + span <
/// lines.len() - 1`, i.e. it has a real neighbor immediately after it)
/// OVERSHOOTS the plain grid-line difference by `bw` — deliberately
/// extending `bw` pixels PAST its own nominal grid line, into where the
/// next cell begins, so the two cells overlap by exactly one border-width.
/// Under this renderer's "border painted inset from the box's own edge"
/// convention (see this section's module doc), that overlap is exactly what
/// makes the two independently-painted borders land on the SAME pixel — a
/// flush, non-overlapping tiling (the naive `lines[end] - lines[start]`
/// with no overshoot) provably does NOT coincide under that convention
/// (verified algebraically, see JOURNAL/DECISIONS). A cell that DOES reach
/// the far edge gets no overshoot (there's no neighbor beyond it to overlap
/// with — overshooting there would extend the cell past the table's own
/// true outer edge, leaving a gap between where the cell's far border
/// actually lands and where the table reports its own total size).
fn collapse_cell_extent(lines: &[f32], start: usize, span: usize, bw: f32) -> (f32, f32) {
    let n = lines.len().saturating_sub(1);
    let start = start.min(n);
    let end = start.saturating_add(span).min(n);
    let x0 = lines.get(start).copied().unwrap_or(0.0);
    let x1 = lines.get(end).copied().unwrap_or(x0);
    let overshoot = if end < n { finite_nonneg(bw) } else { 0.0 };
    (x0, (x1 - x0 + overshoot).max(0.0))
}

/// The collapsed grid's total extent along one axis: the far (`k ==
/// widths.len()`) [`collapse_grid_lines`] boundary — the raw sum minus one
/// border-width per INTERIOR boundary only (a single-column/row table, with
/// no interior boundary at all, reports its untouched raw total). This is
/// the table's own collapsed content-box size along that axis (before
/// `collapse_table_border_widths`' further frame-overlap adjustment — see
/// `measure_node`'s `NodeCtx::Table` arm).
fn collapse_total(widths: &[f32], bw: f32) -> f32 {
    collapse_grid_lines(widths, bw).last().copied().unwrap_or(0.0)
}

/// Recompute every cell's rect for `border-collapse: collapse` (module doc
/// above): same 1:1 index correspondence with `grid.cells` that
/// `table::solve_table`'s own `cell_rects` already promises (both are keyed
/// by the SAME `grid.cells` iteration order — see `compute_table_cache_entry`).
/// `col_widths`/`row_heights` are `solve_table`'s own (unmodified) column-
/// width/row-height solve — this is purely a POSITIONING re-derivation, the
/// underlying min/max-content column/row sizing algorithm is untouched.
fn collapse_adjust_cell_rects(
    grid: &table_layout::Grid,
    col_widths: &[f32],
    row_heights: &[f32],
    bw_x: f32,
    bw_y: f32,
) -> Vec<Rect> {
    let lines_x = collapse_grid_lines(col_widths, bw_x);
    let lines_y = collapse_grid_lines(row_heights, bw_y);
    grid.cells
        .iter()
        .map(|gc| {
            let (x, w) = collapse_cell_extent(&lines_x, gc.col, gc.colspan, bw_x);
            let (y, h) = collapse_cell_extent(&lines_y, gc.row, gc.rowspan, bw_y);
            Rect { origin: Point { x, y }, size: Size { w, h } }
        })
        .collect()
}

/// Translate one `LayoutNode` (any content kind) into a taffy node. `depth`
/// is this node's own `LayoutNode`-chain nesting depth (root = 0); see
/// [`DEPTH_CAP`]. `table_budget` is the remaining nested-table budget (see
/// [`TABLE_DEPTH_CAP`]) — carried through unchanged for ordinary
/// descent, and consumed by one when a `Display::Table` node is actually
/// turned into a table leaf. `margin_override` (D6, margin collapsing) is a
/// per-call override of this node's OWN top/bottom margin, computed by the
/// caller (`translate_container_children`'s non-flex branch, via
/// [`compute_sibling_margin_overrides`]) when `node` is adjoining a sibling
/// whose margin it collapses with; every other call site passes
/// `MarginOverride::default()` (no override — the node's cascaded margin is
/// used as-is, today's pre-collapsing behavior).
fn translate_any<'a>(
    node: &'a LayoutNode,
    taffy: &mut TaffyTree<NodeCtx<'a>>,
    depth: usize,
    table_budget: usize,
    margin_override: MarginOverride,
) -> Built<'a> {
    match &node.content {
        BoxContent::Text(text) => {
            let runs = vec![InlineRun {
                content: InlineContent::Text(text.clone()),
                style: node.style.clone(),
                interactive: node.interactive.clone(),
            }];
            let style = base_style(&node.style);
            let text_align = node.style.text_align;
            let id = taffy
                .new_leaf_with_context(style, NodeCtx::Inline(runs.clone(), text_align))
                .expect("taffy leaf alloc is infallible for a fresh tree");
            Built::Inline { taffy_id: id, runs, text_align }
        }
        BoxContent::Replaced { intrinsic, image } => {
            let mut style = base_style(&node.style);
            let iw = finite_nonneg(intrinsic.w);
            let ih = finite_nonneg(intrinsic.h);
            style.size = TSize { width: length(iw), height: length(ih) };
            let id = taffy.new_leaf(style).expect("taffy leaf alloc is infallible for a fresh tree");
            Built::Replaced {
                style: &node.style,
                taffy_id: id,
                intrinsic: Size { w: iw, h: ih },
                image: image.clone(),
                interactive: node.interactive.clone(),
                id: node.id.clone(),
            }
        }
        // A `display: table` box (real HTML `<table>`, or any element styled
        // `display: table`) becomes a single bespoke leaf — see the module
        // docs' "table" bullet and `measure_node`'s `NodeCtx::Table` arm —
        // UNLESS the nested-table budget is exhausted (`TABLE_DEPTH_CAP`) OR
        // the table has more cells than `MAX_TABLE_MEASURED_CELLS` (a
        // pathologically wide table — the expensive per-cell taffy
        // measurement pipeline isn't safe to run unboundedly-many times; see
        // that constant's doc comment), in which case it falls through to
        // the plain-block translation below exactly like it did before this
        // packet. `place_grid` here is cheap (pure arithmetic, itself capped
        // — see `table_layout::MAX_GRID_CELLS`) — only a cell COUNT, not the
        // per-cell measurement, needs computing to decide.
        BoxContent::Container
            if node.style.display == Display::Table
                && table_budget > 0
                && table_layout::place_grid(node).cells.len() <= MAX_TABLE_MEASURED_CELLS =>
        {
            let mut style = base_style(&node.style);
            // Tables are shrink-to-fit, not stretch-sized, for an auto
            // width (CSS 2.1 §17.4/§10.3.3 — a table is one of the classic
            // "shrink-to-fit" box types alongside floats/inline-block/
            // absolutely-positioned boxes). Taffy's block algorithm only
            // knows to skip its normal "stretch an auto-width block child to
            // the container's content width" behavior — and defer to this
            // leaf's own measured (`measure_node`) size instead — when this
            // flag is set; without it, `layout.size.width` for this leaf
            // would come back full-container-width even though
            // `measure_node` faithfully returned the solved (narrower)
            // content sum. Discovered empirically: a colspan test's
            // "table's own box" showed up 640px wide (the viewport) instead
            // of its ~88px solved content width until this flag was set.
            style.item_is_table = true;
            margin_override.apply(&mut style);
            let id = taffy
                .new_leaf_with_context(style, NodeCtx::Table(node, table_budget - 1, RefCell::new(None)))
                .expect("taffy leaf alloc is infallible for a fresh tree");
            Built::Table { style: &node.style, taffy_id: id }
        }
        // TableCell reached here means it's outside a table-leaf's own cell
        // walk (an orphan `<td>` with no table ancestor, one under a
        // budget-exhausted table, or one belonging to an over-
        // `MAX_TABLE_MEASURED_CELLS` table) — translates exactly like a
        // Container, a plain stacked block, matching pre-table-layout-packet
        // behavior.
        BoxContent::Container | BoxContent::TableCell { .. } => {
            let mut style = base_style(&node.style);
            style.display = map_display(node.style.display);
            apply_flex(&mut style, &node.style);
            // packet/css-grid: unconditional, exactly like `apply_flex`
            // right above -- `apply_grid` itself is a no-op whenever
            // `cs.grid_template_columns`/`rows` are both empty (every
            // non-grid container, unchanged behavior), see its own doc
            // comment.
            apply_grid(&mut style, &node.style);
            margin_override.apply(&mut style);
            // Past DEPTH_CAP, stop descending: an over-deep subtree becomes
            // an empty (childless) box rather than risking a stack
            // overflow. See DEPTH_CAP's doc comment.
            let children = if depth >= DEPTH_CAP || node.children.is_empty() {
                Vec::new()
            } else {
                translate_container_children(node, taffy, depth + 1, table_budget)
            };
            let child_ids: Vec<TNodeId> = children.iter().map(Built::taffy_id).collect();
            let id = taffy
                .new_with_children(style, &child_ids)
                .expect("taffy container alloc is infallible for a fresh tree");
            Built::Container {
                style: &node.style,
                taffy_id: id,
                children,
                interactive: node.interactive.clone(),
                id: node.id.clone(),
            }
        }
    }
}

/// A per-node override of the used top/bottom margin fed to taffy, in px —
/// see the module docs' "Margin collapsing" section and
/// [`compute_sibling_margin_overrides`]. `None` on a field means "use the
/// node's own cascaded margin, unchanged" (the default, and what every call
/// site other than the collapsing pre-pass passes).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct MarginOverride {
    top: Option<f32>,
    bottom: Option<f32>,
}

impl MarginOverride {
    /// Overwrite `style.margin.top`/`.bottom` with this override's values,
    /// where set. Applied AFTER `base_style` (and any other margin-touching
    /// setup) so it always wins.
    fn apply(self, style: &mut TStyle) {
        if let Some(top) = self.top {
            style.margin.top = TLengthPercentageAuto::length(top);
        }
        if let Some(bottom) = self.bottom {
            style.margin.bottom = TLengthPercentageAuto::length(bottom);
        }
    }
}

/// True for an in-flow, block-level box that's eligible to have its margin
/// collapse with an adjoining sibling's (D6, CSS2.1 §8.3.1's "adjoining
/// margins" — the sibling half only, see the module docs): not inline-level
/// content (that's folded into an `Inline` taffy leaf, which has no margin
/// of its own to collapse), not floated (CSS2.1 §8.3.1 excludes floats
/// from collapsing outright), and not a table-internal display (`table`,
/// `table-row`, `table-row-group`, `table-cell`). This float check IS
/// load-bearing, not merely defensive: taffy's own native `Display::Block`
/// collapsing has no concept of `float` at all (this engine never
/// communicates float status into taffy's `Style` for a block box — see
/// `is_inline_ish`'s own doc comment on how float is handled today,
/// bespoke-inline-only), so without this check and
/// [`compute_sibling_margin_overrides`] explicitly overriding the pair to a
/// summed (not collapsed) gap, taffy would collapse a floated sibling's
/// margin with its neighbor's anyway, on its own initiative.
///
/// The table-display exclusion is similarly load-bearing (found via
/// review, not just spec pedantry): CSS2.1 scopes ordinary sibling margin
/// collapsing to boxes participating in a block formatting context — a
/// table/table-row/table-row-group/table-cell participates in the TABLE
/// formatting context instead, and never collapses margins with a sibling
/// (in practice, tables mostly just don't have a meaningful margin on those
/// internal boxes at all — CSS itself ignores `margin` on `table-cell`).
/// Without this exclusion, an ordinary `<table>` element sitting next to
/// another block sibling (e.g. a preceding `<h2>`) would be treated as an
/// ordinary collapse-eligible box by this function, which is wrong even
/// though it's usually a no-op in THIS engine today (a bare `<table>`'s own
/// `margin-top` is `0` with no UA rule setting otherwise, and `max(x, 0) ==
/// x` either way) — the exclusion is what makes that a documented
/// non-issue rather than an accident.
///
/// Flex items are excluded implicitly: `compute_sibling_margin_overrides`
/// is only ever invoked over a NON-flex container's children (see
/// `translate_container_children`), so a flex item is never a candidate
/// regardless of this function — consistent either way, since taffy's OWN
/// flexbox algorithm never does margin collapsing at all (confirmed by
/// `flex_column_item_margins_do_not_collapse` in `tests/layout_block.rs`).
fn is_collapse_eligible_block(n: &LayoutNode) -> bool {
    !is_inline_ish(n)
        && n.style.float == Float::None
        && !matches!(
            n.style.display,
            Display::Table | Display::TableRow | Display::TableRowGroup | Display::TableCell
        )
}

/// `true` for a zero-length `LengthPercentage` — used by
/// [`margins_may_collapse`] to check "no padding on the touching edge".
/// Conservatively `false` for any percentage value (even `0%`): resolving a
/// percentage needs the containing block's width, not available at this
/// translate-time pre-pass, so a declared percentage padding is treated as
/// "might not be zero" and blocks collapsing rather than risking a wrong
/// collapse.
fn is_zero_padding(v: LengthPercentage) -> bool {
    matches!(v, LengthPercentage::Px(p) if p == 0.0)
}

/// `true` iff nothing sits between `prev`'s bottom edge and `next`'s top
/// edge that would end adjoinment (CSS2.1 §8.3.1): no visible border and no
/// non-zero padding on either touching side. Both boxes must clear both
/// checks — e.g. `prev`'s bottom border blocks collapsing exactly as much
/// as `next`'s top border would.
fn margins_may_collapse(prev: &LayoutNode, next: &LayoutNode) -> bool {
    !paints_visible_border(&prev.style.border.bottom)
        && is_zero_padding(prev.style.padding.bottom)
        && !paints_visible_border(&next.style.border.top)
        && is_zero_padding(next.style.padding.top)
}

/// The used px value of a top/bottom margin for collapsing purposes, or
/// `None` if it can't be resolved at this translate-time pre-pass. `Auto`
/// always resolves to `0` — CSS2.1 §10.6.3: a top/bottom margin's `auto`
/// only ever gets a nonzero used value for an absolutely-positioned box,
/// which this engine (no `position` support) never produces, so in this
/// engine's model `auto` on `margin-top`/`margin-bottom` is unconditionally
/// `0`. `Percent` is left unresolved (`None`, see [`is_zero_padding`]'s
/// doc comment for the same containing-block-width reasoning) — a pair
/// involving one is simply left uncollapsed (summed, the pre-existing
/// behavior) by the caller.
fn margin_px_for_collapse(v: LengthPercentageAuto) -> Option<f32> {
    match v {
        LengthPercentageAuto::Auto => Some(0.0),
        LengthPercentageAuto::Px(p) if p.is_finite() => Some(p),
        _ => None,
    }
}

/// The pre-pass at the heart of D6: for `children` (one non-flex
/// container's own in-flow children, in document order — see
/// `translate_container_children`'s non-flex branch, the only caller),
/// compute a [`MarginOverride`] per child that makes each adjoining pair of
/// real (non-whitespace-only) block-level siblings resolve to EXACTLY the
/// gap CSS2.1 wants: `max(prev.margin_bottom, next.margin_top)` when the
/// pair is collapse-eligible and nothing separates them ([`margins_may_
/// collapse`]), the ordinary summed `prev.margin_bottom + next.margin_top`
/// otherwise. The returned `Vec` is parallel to `children` (same length,
/// same order).
///
/// **Every** adjoining pair of NON-FLOATED real blocks gets an explicit
/// override, not just the ones that collapse — this is load-bearing, not
/// belt-and-suspenders. Taffy 0.13's `Display::Block` algorithm
/// (`taffy::compute::block`) implements real CSS margin collapsing
/// NATIVELY, and unlike this function it does NOT gate sibling-to-sibling
/// collapsing on border/padding at all (those only affect taffy's own "can
/// this box be collapsed THROUGH" pass-through check, a different question
/// — see `has_styles_preventing_being_collapsed_through` in taffy's
/// source, which only ever gates a PARENT/child or pass-through
/// relationship, never a plain sibling pair). So taffy will happily
/// collapse two directly-adjacent `Display::Block` items' margins on its
/// own initiative REGARDLESS of what this function decides — leaving a
/// "should NOT collapse" pair's margins untouched (the pre-`t6` fix's
/// original approach) does not make taffy sum them, it just lets taffy
/// collapse them anyway. The only way to reliably override taffy's own
/// decision is to feed it a `(sum, 0)` pair instead of `(a, b)`: taffy's
/// own `collapse_with_margin`/`collapse_with_set` calls always resolve to
/// `max`, and `max(sum, 0) == sum` for the non-negative margins this
/// function handles — so committing the desired final value into the
/// EARLIER box's margin-bottom and zeroing the LATER box's margin-top
/// makes any further collapsing taffy performs on top of that a no-op,
/// regardless of which formula (max or sum) produced the committed value.
/// The same trick is what makes the actually-eligible pairs work too
/// (`max(a, b)` committed with a trailing `0` collapses to itself) — the
/// two cases share one code path below, differing only in which formula
/// computes `gap`.
///
/// Walks `children` once, tracking the most recent REAL (non-whitespace,
/// non-inline) block-level box still adjacent to the position being
/// considered: a whitespace-only text child (CSS2.1 §9.2.2.1: generates no
/// box) is transparent and doesn't break adjacency; real inline content
/// does (never actually reached here in practice — such content is folded
/// into an `Inline` run by the caller's own grouping loop before this
/// function ever sees it, but the check is kept for clarity/defense).
///
/// packet/acid1-content-box regression fix: a pair where EITHER side is
/// floated (`style.float != Float::None`) gets NO override at all, on
/// EITHER index — this used to not be true (a float still got zeroed as
/// the "later" sibling, or had its "earlier" sibling's margin summed into
/// its own bottom), on the theory that "taffy doesn't know it's floated
/// and would otherwise collapse across it regardless." That theory is
/// FALSE for the float's OWN margin specifically: reading taffy 0.13's
/// vendored `compute::block::perform_final_layout_on_in_flow_children`
/// directly shows a floated item's branch (`if let Some(float_direction) =
/// item.float.float_direction() { ... continue; }`) is checked FIRST and
/// unconditionally `continue`s BEFORE any of the margin-collapsing
/// bookkeeping (`active_collapsible_margin_set`, `collapse_with_margin`,
/// ...) ever runs for it — a floated item's own margin is NEVER a
/// candidate for taffy-native collapsing in the first place; it goes
/// straight into `margin_box = item_layout.size + item_non_auto_margin.
/// sum_axes()` (its RAW cascaded margin, `top` AND `bottom` both), and
/// `location.y += item_non_auto_margin.top` converts that margin-box
/// position back to the float's own border-box position. Zeroing a
/// float's `top` override (or inflating its `bottom` override with a
/// neighbor's margin, as the old code did as `prev` in the NEXT pair) does
/// not neutralize any taffy-native collapsing (there was never any to
/// neutralize) — it just silently DELETES or DISTORTS the float's real
/// declared margin, which taffy then faithfully (and now wrongly)
/// consumes. This is exactly the bug `fixtures/css1-float-5526c.html`'s
/// `blockquote`/`h1` (both `float:left`, both `margin: 1em ...`, i.e. a
/// real 10px top margin) surfaced: the old code zeroed their margin-top,
/// so row 2 of `dd`'s floated content started 10px higher than it should
/// have, leaving a 20px gap at `dd`'s bottom interior edge instead of the
/// uniform 10px every other side already had (Chrome's own reference
/// rendering, pixel-verified). The (real, taffy-native) concern the old
/// code's "chain like `[A, float, C]`" reasoning was trying to guard
/// against — `A`'s trailing margin potentially collapsing THROUGH the
/// float into `C`'s leading margin, since the float's `continue` also
/// means it never resets `active_collapsible_margin_set` — is a genuine,
/// SEPARATE question about the two NON-floated neighbors of a float, not
/// about the float's own margin; no fixture in this repo's golden set
/// exercises a `[non-float, float, non-float]` sibling chain that would
/// need it resolved, so this fix scopes cleanly to "never touch a float's
/// own margin override" without having to also solve that separate,
/// currently-unexercised case.
///
/// Each child can accumulate up to two independent overrides across the
/// whole walk — one as the "later" sibling of the pair before it (its
/// `top`), one as the "earlier" sibling of the pair after it (its
/// `bottom`) — computed from that child's own ORIGINAL cascaded margin in
/// both cases (an already-overridden `top` never feeds a later `bottom`
/// computation or vice versa), matching real CSS2.1 behavior for a chain of
/// several touching boxes (each pair resolved independently) short of the
/// "empty self-collapsing box" refinement (CSS2.1 §8.3.1's own further
/// special case for a box with zero height/border/padding/content, which
/// can let THREE OR MORE margins collapse into one) — not implemented here,
/// no fixture in this packet's golden set exercises it.
///
/// A pair whose margin isn't resolvable to a px value at this translate-time
/// pre-pass (a `Percent` margin on either side — see [`margin_px_for_
/// collapse`]) is left with NO override at all, on either box: this is the
/// one case where taffy's own native collapsing behavior is NOT overridden
/// (a documented limitation, not a choice — there's no containing-block
/// width available here to resolve a percentage against).
fn compute_sibling_margin_overrides(children: &[LayoutNode]) -> Vec<MarginOverride> {
    let mut overrides = vec![MarginOverride::default(); children.len()];
    let mut prev: Option<usize> = None;
    for (i, child) in children.iter().enumerate() {
        if is_whitespace_only_text(child) {
            continue;
        }
        if is_inline_ish(child) {
            prev = None;
            continue;
        }
        if let Some(p) = prev {
            // packet/acid1-content-box: a float's own margin is NEVER a
            // candidate for taffy-native collapsing (see this function's
            // own doc comment for the full "why", verified against taffy
            // 0.13's vendored source) -- skip the whole pair, on EITHER
            // index, whenever either side is floated, rather than zeroing/
            // distorting the float's real declared margin.
            let touches_float = children[p].style.float != Float::None || child.style.float != Float::None;
            if !touches_float {
                let eligible = is_collapse_eligible_block(&children[p])
                    && is_collapse_eligible_block(child)
                    && margins_may_collapse(&children[p], child);
                let prev_bottom = margin_px_for_collapse(children[p].style.margin.bottom);
                let next_top = margin_px_for_collapse(child.style.margin.top);
                if let (Some(pb), Some(nt)) = (prev_bottom, next_top) {
                    let gap = if eligible { pb.max(nt) } else { pb + nt };
                    overrides[p].bottom = Some(gap);
                    overrides[i].top = Some(0.0);
                }
            }
        }
        prev = Some(i);
    }
    overrides
}

/// True for the children a container folds into one inline formatting
/// context leaf: bare text, a nested `display: inline` container, or a
/// `Replaced` element (M4: closes the D14 gap). `Replaced` is inline-ish
/// REGARDLESS of `float` — a non-floated one becomes an inline atom sitting
/// on the line (`inline::InlineContent::Replaced` with `style.float ==
/// Float::None`, see `inline::tokenize`), a floated one (`align=left`/
/// `float: left|right`) is pulled out of line flow by `inline::layout_runs`
/// itself and placed at the containing block's edge — either way it belongs
/// in the SAME inline formatting context as any surrounding text (the
/// `<p><img align=left>text...</p>` shape needs the float and the wrapping
/// text folded into one taffy leaf so `inline::layout_runs` sees both
/// together), not broken out into its own stacked block box.
/// True for a `Text` `LayoutNode` whose entire content is whitespace (or
/// which is empty) — see `translate_container_children`'s flex branch for
/// why this matters: such a node must not become its own flex item. Only
/// `Text` nodes qualify; any other content kind is never whitespace-only by
/// definition.
fn is_whitespace_only_text(n: &LayoutNode) -> bool {
    matches!(&n.content, BoxContent::Text(t) if t.trim().is_empty())
}

fn is_inline_ish(n: &LayoutNode) -> bool {
    match &n.content {
        BoxContent::Text(_) => true,
        // A table (`display: table`) is never inline-ish — it's routed to
        // `translate_any`'s dedicated table-leaf branch, same as any other
        // non-inline `Container`, regardless of this check. A `TableCell`
        // reaching here (an orphan `<td>`, see `translate_any`) is routed
        // exactly like a plain Container, matching pre-table-layout-packet
        // behavior.
        //
        // An inline-display `Container`/`TableCell` is only inline-ish if it
        // has no block-level descendant (block-in-inline resolution, CSS2.1
        // §9.2.1.1 / CSS Display Level 3 §2.7) — see
        // `contains_block_descendant`'s doc comment for why: without this,
        // an inline wrapper (`<font>`, `<b>`, ...) around a block list
        // (`<ol>`/`<li>`) gets folded whole into the inline formatting
        // context by `flatten_inline`, silently dropping the list items'
        // block-level line breaks (confirmed real-world breakage:
        // http://68k.news/ wraps every news list in
        // `<font size="4"><ol><li>...`).
        BoxContent::Container | BoxContent::TableCell { .. } => {
            n.style.display == Display::Inline && !contains_block_descendant(n, 0)
        }
        BoxContent::Replaced { .. } => true,
    }
}

/// True if `n` (typically an inline-display container) contains a
/// block-level box somewhere in its inline subtree — in which case CSS's
/// "block-in-inline" resolution (CSS2.1 §9.2.1.1 / CSS Display Level 3 §2.7)
/// means `n` can't be folded whole into one inline formatting context leaf:
/// `<font>` wrapping an `<ol>` must still produce real block list-item
/// boxes (each on its own line), not run every item together on one line
/// the way folding `<font>`'s entire subtree into `flatten_inline` would.
///
/// A `Text` or `Replaced` child is an inline ATOM, never block-level —
/// `<em><img></em>` must stay foldable into one inline run (the D14 fix
/// this helper must not regress: see `flatten_inline`'s doc comment). Only
/// a `Container`/`TableCell` child whose own `style.display` is anything
/// other than `Inline` (`Block`, `Flex`, `Table`, `TableRow`, `TableCell`,
/// `TableRowGroup`, ...) counts as block-level itself; an inline
/// `Container` child is not itself block-level but IS recursed into — a
/// block box can be nested arbitrarily deep inside a chain of inline
/// wrappers (`<font><b><ol>...`).
///
/// `depth` mirrors `flatten_inline`/`translate_any`'s own cap (see
/// [`DEPTH_CAP`]) — this is independent recursion (it never goes through
/// `translate_any`), so it needs its own bound against a hostile/
/// pathologically deep inline nest; past the cap it degrades gracefully
/// (returns `false`, i.e. "not blockified") rather than risking a stack
/// overflow.
fn contains_block_descendant(n: &LayoutNode, depth: usize) -> bool {
    if depth >= DEPTH_CAP {
        return false;
    }
    n.children.iter().any(|child| match &child.content {
        BoxContent::Container | BoxContent::TableCell { .. } => {
            child.style.display != Display::Inline || contains_block_descendant(child, depth + 1)
        }
        BoxContent::Text(_) | BoxContent::Replaced { .. } => false,
    })
}

/// Flatten a node's inline-level content (itself, if it's `Text` or
/// `Replaced`; its children recursively, if it's an inline `Container`) into
/// `InlineRun`s in document order — `inline::layout_runs` sorts out text vs.
/// non-floated atom vs. floated-out-of-flow purely from each run's
/// `content`/`style.float`, so this walk just needs to carry every leaf
/// through untouched (M4: closes the D14 "grandchild dropped" gap — a
/// `Replaced` nested inside an inline `Container`, e.g. `<em><img></em>`,
/// now gets a run here instead of being silently skipped).
///
/// `depth` mirrors `translate_any`'s cap (see [`DEPTH_CAP`]): this walk is
/// independent recursion (it never goes through `translate_any`), so it
/// needs its own bound against the same pathological-nesting case.
fn flatten_inline(node: &LayoutNode, out: &mut Vec<InlineRun>, depth: usize) {
    match &node.content {
        BoxContent::Text(text) => out.push(InlineRun {
            content: InlineContent::Text(text.clone()),
            style: node.style.clone(),
            interactive: node.interactive.clone(),
        }),
        // An orphan `TableCell` reaching this path (see `is_inline_ish`) is
        // routed exactly like Container.
        BoxContent::Container | BoxContent::TableCell { .. } => {
            if depth >= DEPTH_CAP {
                return; // over-deep inline subtree: drop gracefully, don't recurse further.
            }
            for child in &node.children {
                flatten_inline(child, out, depth + 1);
            }
        }
        BoxContent::Replaced { intrinsic, image } => out.push(InlineRun {
            content: InlineContent::Replaced { intrinsic: *intrinsic, image: image.clone() },
            style: node.style.clone(),
            interactive: node.interactive.clone(),
        }),
    }
}

/// Translate a container's children, grouping maximal runs of inline-level
/// children into single IFC leaves and translating everything else (block
/// containers, replaced elements) as their own taffy nodes. `display: flex`
/// AND `display: grid` (packet/css-grid: a grid container needs the exact
/// same "every child is its own item" treatment a flex container already
/// gets — an un-grouped `Text`/inline run inside a grid container would
/// otherwise get folded into one taffy leaf per module docs' default
/// IFC-grouping rule below, instead of becoming its own placeable grid
/// item) containers skip grouping entirely — every child is its own
/// flex/grid item. `depth` is the depth at which `node`'s children
/// themselves sit (already incremented by the caller); see [`DEPTH_CAP`].
fn translate_container_children<'a>(
    node: &'a LayoutNode,
    taffy: &mut TaffyTree<NodeCtx<'a>>,
    depth: usize,
    table_budget: usize,
) -> Vec<Built<'a>> {
    let mut out = Vec::new();
    if matches!(node.style.display, Display::Flex | Display::Grid) {
        for child in &node.children {
            // CSS Flexbox (§4 "Flex Items"): "a child text node consisting
            // entirely of collapsible white space is not rendered, i.e. it
            // does not generate an anonymous flex item" — skip it entirely
            // rather than giving it its own taffy flex-item node. Without
            // this, ordinary document-formatted markup (any HTML with
            // newlines/indentation between flex children — the overwhelming
            // common case, not a contrived one) turns every whitespace-only
            // `Text` node between real children into a phantom zero-width
            // flex item that still counts toward `gap` on both sides,
            // silently doubling the visual gap between real items (found via
            // `fixtures/flex-polite.html`'s `<nav>` links: M5 flex-polite
            // packet). packet/css-grid: CSS Grid's own "Grid Items" section
            // makes the identical carve-out for a whitespace-only text
            // child, so this applies unchanged to a grid container's
            // children too. A non-whitespace text node (real inline content
            // directly inside a flex/grid container, e.g. `<div
            // style="display: flex">hello<span>world</span></div>`) is
            // untouched — it still becomes its own flex/grid item, matching
            // the module docs' existing "every child is its own taffy child
            // node" contract.
            if is_whitespace_only_text(child) {
                continue;
            }
            // Flex/grid items never participate in margin collapsing (CSS
            // Flexbox §4; CSS Grid §11 makes the identical carve-out) —
            // `MarginOverride::default()` (no-op) here keeps this branch's
            // pre-existing behavior byte-for-byte.
            out.push(translate_any(child, taffy, depth, table_budget, MarginOverride::default()));
        }
        return out;
    }

    // D6: adjacent in-flow block-level siblings collapse their touching
    // margins (module docs' "Margin collapsing" section) — computed once,
    // up front, over this container's own children so the per-child
    // `translate_any` calls below just apply whatever override (possibly
    // none) this pre-pass decided for that index.
    let margin_overrides = compute_sibling_margin_overrides(&node.children);

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
            let text_align = node.style.text_align;
            let id = taffy
                .new_leaf_with_context(style, NodeCtx::Inline(runs.clone(), text_align))
                .expect("taffy leaf alloc is infallible for a fresh tree");
            out.push(Built::Inline { taffy_id: id, runs, text_align });
            i = j;
        } else {
            out.push(translate_any(&node.children[i], taffy, depth, table_budget, margin_overrides[i]));
            i += 1;
        }
    }
    out
}

/// The measure function threaded through every `compute_layout_with_measure`
/// call in this module — the top-level `layout_tree`, and every nested
/// per-cell/per-table sub-tree built while solving a table (see
/// `cell_query_width`/`cell_content_layout`/`ensure_table_cache`).
/// Dispatches on the leaf's [`NodeCtx`]: `Inline` runs go through the
/// bespoke inline engine (pre-existing, P6, unchanged); `Table` leaves
/// ensure the whole grid-placement + column/row solve pipeline has been run
/// (reusing the cached result if one already covers this `avail_w` — see
/// [`ensure_table_cache`]) and report its total content size.
fn measure_node<M: Metrics>(
    known_dimensions: TSize<Option<f32>>,
    available_space: TSize<AvailableSpace>,
    _node_id: TNodeId,
    node_context: Option<&mut NodeCtx>,
    _style: &TStyle,
    metrics: &M,
) -> TSize<f32> {
    if let TSize { width: Some(w), height: Some(h) } = known_dimensions {
        return TSize { width: w, height: h };
    }
    let avail_w = match known_dimensions.width {
        Some(w) => w,
        None => match available_space.width {
            AvailableSpace::Definite(w) => w,
            AvailableSpace::MaxContent => MAX_CONTENT_WIDTH,
            AvailableSpace::MinContent => 0.0,
        },
    };
    match node_context {
        None => TSize::ZERO,
        Some(NodeCtx::Inline(runs, text_align)) => {
            let out = inline::layout_runs(runs, avail_w, *text_align, metrics);
            TSize {
                width: known_dimensions.width.unwrap_or(out.size.w),
                height: known_dimensions.height.unwrap_or(out.size.h),
            }
        }
        Some(NodeCtx::Table(table_node, table_budget, cache)) => {
            ensure_table_cache(table_node, finite_nonneg(avail_w), metrics, *table_budget, cache);
            let borrowed = cache.borrow();
            // `ensure_table_cache` always leaves `Some` behind; a missing
            // entry here would be a bug in that function, not reachable
            // input — degrade to zero rather than unwrap/panic regardless.
            let Some(entry) = borrowed.as_ref() else { return TSize::ZERO };
            // Total size must include the border-spacing gaps *between*
            // columns/rows too (`(columns - 1)` gaps of `BORDER_SPACING_X`,
            // `(rows - 1)` of `BORDER_SPACING_Y`) — matching exactly how
            // `solve_table` itself computes `sum_min`/`sum_max` internally
            // and how a colspan/rowspan cell's own rect already includes
            // its *inner* gaps (`table::solve_table`'s `cell_rects`
            // formula). Summing bare `col_widths`/`row_heights` alone
            // undercounts by the OUTER gaps between every adjacent pair of
            // columns/rows, making the table's own reported box narrower/
            // shorter than a colspan/rowspan cell that spans (and thus
            // already includes the gaps for) the whole grid.
            let col_gaps = entry.columns.saturating_sub(1) as f32;
            let row_gaps = entry.rows.saturating_sub(1) as f32;
            // packet/table-spacing: the table's OWN resolved style, not a
            // hardcoded constant — see `ComputedStyle::border_spacing_x`'s
            // doc comment (falls back to the same 8.0/0.0 default when
            // nothing set it, so this is a no-op change for every table
            // that doesn't use `border-spacing`/`cellspacing`). packet/
            // border-collapse: a `Collapse` table gets `(0.0, 0.0)` instead
            // — see `effective_border_spacing`'s own doc comment.
            let (spacing_x, spacing_y) = effective_border_spacing(&table_node.style);
            let (total_w, total_h) = if table_node.style.border_collapse == BorderCollapse::Collapse {
                // packet/collapse-geometry: the collapsed grid's own total
                // (raw sum minus one border-width per INTERIOR boundary —
                // see `collapse_total`), further pulled in by the table's
                // OWN frame border width on each axis (if it has one) so
                // `emit`'s later `content_box_x/y() - table_bw` positioning
                // (which overlaps the whole grid onto the table's own frame,
                // not just its content-box inset) reconstructs a border-box
                // size that exactly closes over the collapsed grid — see
                // `emit`'s `Built::Table` arm and this module's own
                // "packet/collapse-geometry" doc section above.
                let (table_bw_x, table_bw_y) = collapse_table_border_widths(&table_node.style);
                let w = collapse_total(&entry.table_layout.col_widths, entry.collapse_bw_x) - 2.0 * table_bw_x;
                let h = collapse_total(&entry.table_layout.row_heights, entry.collapse_bw_y) - 2.0 * table_bw_y;
                (finite_nonneg(w), finite_nonneg(h))
            } else {
                (
                    finite_nonneg(entry.table_layout.col_widths.iter().sum::<f32>() + col_gaps * spacing_x),
                    finite_nonneg(entry.table_layout.row_heights.iter().sum::<f32>() + row_gaps * spacing_y),
                )
            };
            TSize {
                width: known_dimensions.width.unwrap_or(total_w),
                height: known_dimensions.height.unwrap_or(total_h),
            }
        }
    }
}

/// Ensure `cache` holds a [`TableCacheEntry`] solved at `available_width`
/// (within a small float epsilon), (re)computing it via
/// [`compute_table_cache_entry`] only if the cached entry is missing or was
/// solved at a different width. This is the Critical-C1 fix (review): the
/// full per-cell measurement pipeline is expensive (see
/// [`MAX_TABLE_MEASURED_CELLS`]'s doc comment) and was previously re-run
/// from scratch by both `measure_node` (possibly several times — taffy's
/// own layout algorithm may query a leaf's intrinsic size more than once)
/// AND `emit` (once more, unconditionally) — up to ~7 full per-cell taffy
/// sub-layouts. Caching collapses this to ~3 per cell in the common case
/// (one `avail_w` throughout: computed once during measure, reused for free
/// by `emit`), and at most ~3 per DISTINCT `avail_w` a real taffy layout
/// pass ends up probing — never re-paying the cost for a width already
/// solved.
///
/// The cache is keyed on `avail_w` (not "computed once, ever") because
/// `solve_table`'s column resolution genuinely depends on it (the
/// under-constrained/over-constrained/interpolated branches) — reusing a
/// stale entry solved at a different width would silently produce the
/// wrong geometry, not just suboptimal caching.
fn ensure_table_cache<M: Metrics>(
    table_node: &LayoutNode,
    available_width: f32,
    metrics: &M,
    table_budget: usize,
    cache: &RefCell<Option<TableCacheEntry>>,
) {
    let stale = match &*cache.borrow() {
        Some(entry) => (entry.avail_w - available_width).abs() > 0.01,
        None => true,
    };
    if stale {
        let fresh = compute_table_cache_entry(table_node, available_width, metrics, table_budget);
        *cache.borrow_mut() = Some(fresh);
    }
}

/// Run the full table pipeline for `table_node`'s own subtree at
/// `available_width` (the CSS auto-table-layout "available width" the
/// packet brief's step 3 asks for, sourced from taffy/its parent — see
/// `measure_node`'s `NodeCtx::Table` arm and `emit`'s `Built::Table` arm,
/// the two callers via [`ensure_table_cache`]): place the grid
/// ([`table_layout::place_grid`]), measure each cell's min/max content
/// width ([`cell_min_max_width`]), solve column widths (pass 1), re-measure
/// each cell's real content (size AND, this time, its paint-ordered
/// fragments too — see [`cell_content_layout`]) at its solved width, then
/// solve again with real heights to get final row heights + cell rects
/// (pass 2) — the two-stage phasing documented in the packet report.
/// `table_budget` is the nested-table budget cell content may spend (see
/// [`TABLE_DEPTH_CAP`]).
///
/// Border-spacing (packet/table-spacing: CSS `border-spacing`/HTML
/// `cellspacing="N"`, superseding the earlier M3 "fixed constant" doc note
/// this replaces): read straight off `table_node.style.border_spacing_x/y`
/// — a FREEZE AMENDMENT (`style::ComputedStyle`'s own doc comment) added
/// exactly so this could stop being a hardcoded constant. The default
/// (`8px`/`0px`, not CSS's real `2px 2px` initial value — see that field's
/// doc comment for the full "why 8px" rationale, preserved verbatim from
/// the old `BORDER_SPACING_X`/`BORDER_SPACING_Y` constants) is what every
/// table with no `border-spacing`/`cellspacing` of its own still resolves
/// to, so this change is a no-op for any such table. Without SOME nonzero
/// horizontal spacing, two abutting cells whose content exactly fills their
/// column can visually run together in the tty text-mode dump — which
/// paints no backgrounds/borders at all (see `backend::tty`'s own
/// documented scope call) — e.g. `"Qty"` immediately followed by `"Notes"`
/// reading as `"QtyNotes"`.
///
/// Total: every step this calls (`place_grid`, `solve_table`,
/// `cell_min_max_width`, `cell_content_layout`) is itself total; this
/// function adds no new panic surface. Never called with more cells than
/// [`MAX_TABLE_MEASURED_CELLS`] — `translate_any` only ever builds a
/// `NodeCtx::Table` leaf (the only way this function gets invoked, via
/// `ensure_table_cache`) for a table within that cap.
fn compute_table_cache_entry<M: Metrics>(
    table_node: &LayoutNode,
    available_width: f32,
    metrics: &M,
    table_budget: usize,
) -> TableCacheEntry {
    let grid = table_layout::place_grid(table_node);
    // packet/table-spacing: read straight off the table's own resolved
    // style (falls back to the pre-existing 8.0/0.0 default — see
    // `ComputedStyle::border_spacing_x`'s doc comment), sanitized the same
    // way every other layout-space scalar in this module is. packet/
    // border-collapse: `(0.0, 0.0)` instead when the table is collapsed —
    // see `effective_border_spacing`'s own doc comment.
    let (spacing_x, spacing_y) = effective_border_spacing(&table_node.style);
    // packet/collapse-geometry: the uniform per-axis CELL border width this
    // table's collapse geometry (if any) is built from — `(0.0, 0.0)` for a
    // `Separate` table, making every `collapse_adjust_cell_rects` call below
    // a total no-op (see `collapse_grid_lines`/`collapse_cell_extent`'s own
    // "bw == 0" degenerate case).
    let collapse = table_node.style.border_collapse == BorderCollapse::Collapse;
    let (bw_x, bw_y) = if collapse { collapse_cell_border_widths(&grid) } else { (0.0, 0.0) };

    let mut cells: Vec<CellSpec> = grid
        .cells
        .iter()
        .map(|gc| {
            let (min_content, max_content) = cell_min_max_width(gc.node, metrics, table_budget);
            CellSpec {
                col: gc.col,
                row: gc.row,
                colspan: gc.colspan,
                rowspan: gc.rowspan,
                min_content,
                max_content,
                intrinsic_height: 0.0,
            }
        })
        .collect();

    let mut pass1 = table::solve_table(&TableSpec {
        columns: grid.columns,
        rows: grid.rows,
        cells: cells.clone(),
        available_width,
        border_spacing_x: spacing_x,
        border_spacing_y: spacing_y,
    });
    if collapse {
        // packet/collapse-geometry: re-derive pass1's cell rects BEFORE
        // using them to assign each cell's content-layout width below, so
        // the width a cell's own content is laid out at matches its FINAL
        // collapsed rect (same collapse geometry re-applied to the final
        // solve just below) — otherwise a cell's content would wrap against
        // a wider (pre-collapse) width than its actually-painted box.
        pass1.cell_rects = collapse_adjust_cell_rects(&grid, &pass1.col_widths, &pass1.row_heights, bw_x, bw_y);
    }

    // One sub-layout per cell here (not two — see the module report):
    // `cell_content_layout`'s fragments are KEPT (not discarded) so `emit`
    // never needs to re-lay this cell's content out again.
    let mut cell_content: Vec<(Size, Vec<Fragment>)> = Vec::with_capacity(grid.cells.len());
    for (i, gc) in grid.cells.iter().enumerate() {
        let assigned_w = pass1.cell_rects.get(i).map(|r| r.size.w).unwrap_or(0.0);
        let (size, fragments) = cell_content_layout(gc.node, assigned_w, metrics, table_budget);
        if let Some(cell) = cells.get_mut(i) {
            cell.intrinsic_height = size.h;
        }
        cell_content.push((size, fragments));
    }

    let mut table_layout = table::solve_table(&TableSpec {
        columns: grid.columns,
        rows: grid.rows,
        cells,
        available_width,
        border_spacing_x: spacing_x,
        border_spacing_y: spacing_y,
    });
    if collapse {
        table_layout.cell_rects =
            collapse_adjust_cell_rects(&grid, &table_layout.col_widths, &table_layout.row_heights, bw_x, bw_y);
    }

    TableCacheEntry {
        avail_w: available_width,
        columns: grid.columns,
        rows: grid.rows,
        table_layout,
        cell_content,
        collapse_bw_x: bw_x,
        collapse_bw_y: bw_y,
    }
}

/// A cell's min-content width (every soft-wrap opportunity taken) and
/// max-content width (no wrapping) — via taffy's own intrinsic-sizing query
/// (`AvailableSpace::MinContent`/`MaxContent`) over a fresh translation of
/// the cell's own subtree. This is the "cleanest mechanism" choice flagged
/// in the packet brief: taffy already implements automatic min/max-content
/// sizing for block/flex containers (needed internally for e.g.
/// `flex-basis: content`), so reusing it here — rather than hand-rolling a
/// second intrinsic-width algorithm — gets margins/padding/borders/nested
/// blocks/nested tables all correct for free, and for zero extra code
/// (`measure_node` already handles both leaf kinds).
fn cell_min_max_width<M: Metrics>(node: &LayoutNode, metrics: &M, table_budget: usize) -> (f32, f32) {
    let min_w = finite_nonneg(cell_query_width(node, metrics, table_budget, AvailableSpace::MinContent));
    let max_w = finite_nonneg(cell_query_width(node, metrics, table_budget, AvailableSpace::MaxContent)).max(min_w);
    (min_w, max_w)
}

fn cell_query_width<M: Metrics>(node: &LayoutNode, metrics: &M, table_budget: usize, query: AvailableSpace) -> f32 {
    let mut taffy: TaffyTree<NodeCtx> = TaffyTree::new();
    let built = translate_any(node, &mut taffy, 0, table_budget, MarginOverride::default());
    let available = TSize { width: query, height: AvailableSpace::MaxContent };
    let _ = taffy.compute_layout_with_measure(built.taffy_id(), available, |kd, av, id, ctx, style| {
        measure_node(kd, av, id, ctx, style, metrics)
    });
    taffy.layout(built.taffy_id()).map(|l| l.size.width).unwrap_or(0.0)
}

/// Lay a cell's own subtree out at a fixed `width` (its solved column-span
/// width) and return its total size plus its paint-ordered fragments
/// (relative to the cell's own border-box origin, `(0, 0)`) — the same shape
/// `layout_tree` produces for a whole document, since a cell's content is
/// just another (small) box tree; a cell's own margin is not honored here
/// (matching real CSS, which ignores margin on table cells entirely) so
/// `(0, 0)` really is the cell's painted origin, no root-margin caveat to
/// track.
///
/// Called exactly ONCE per cell per solved `avail_w`, from
/// [`compute_table_cache_entry`] (which keeps this call's fragments in the
/// resulting [`TableCacheEntry`] rather than discarding them) — NOT twice,
/// and not separately at `emit` time. Before the Critical-C1 fix (review),
/// this was called here once for `intrinsic_height` (fragments discarded)
/// AND again, unconditionally, from `emit` for the real fragments; combined
/// with `cell_min_max_width`'s own two sub-layouts and `emit` re-running
/// the WHOLE per-cell pipeline a second time, one cell could pay for up to
/// ~7 full taffy sub-layouts. Reusing this call's fragments via the cache
/// (see [`ensure_table_cache`]) collapses that to the ~3 sub-layouts
/// (`cell_min_max_width` ×2 + this ×1) that are genuinely unavoidable per
/// distinct `avail_w` — see [`MAX_TABLE_MEASURED_CELLS`] for the hard cap
/// that bounds the remaining (still real, still per-cell) cost.
fn cell_content_layout<M: Metrics>(node: &LayoutNode, width: f32, metrics: &M, table_budget: usize) -> (Size, Vec<Fragment>) {
    let mut taffy: TaffyTree<NodeCtx> = TaffyTree::new();
    let built = translate_any(node, &mut taffy, 0, table_budget, MarginOverride::default());
    let w = finite_nonneg(width);
    if let Ok(mut style) = taffy.style(built.taffy_id()).cloned() {
        style.size.width = length(w);
        let _ = taffy.set_style(built.taffy_id(), style);
    }
    let available = TSize { width: AvailableSpace::Definite(w), height: AvailableSpace::MaxContent };
    let _ = taffy.compute_layout_with_measure(built.taffy_id(), available, |kd, av, id, ctx, style| {
        measure_node(kd, av, id, ctx, style, metrics)
    });
    // Acid2 scroll-to-fragment packet, Task 3: a table cell has no
    // independent viewport concept of its own -- `(Point::default(), None)`,
    // same documented approximation as `f.clip`'s own re-origining carve-out
    // just below this function's own caller (no fixture nests
    // `position:fixed` inside a `<td>`). `is_fixed_ctx` starts `false` here
    // too: a cell's own content starts outside any fixed subtree by
    // definition (this cell IS the "viewport" as far as this isolated
    // sub-layout is concerned).
    let mut fragments = Vec::new();
    emit(&built, &taffy, Point { x: 0.0, y: 0.0 }, metrics, &mut fragments, None, false, Point { x: 0.0, y: 0.0 }, None);
    let size = taffy
        .layout(built.taffy_id())
        .map(|l| Size { w: finite_nonneg(l.size.width), h: finite_nonneg(l.size.height) })
        .unwrap_or_default();
    (size, fragments)
}

/// The box-model + display-independent parts of a taffy `Style` shared by
/// every node kind: size, margin, padding, border, `float`/`clear`.
///
/// packet/block-floats: `float`/`clear` are display-independent too --
/// they're honored by taffy's `block_layout` algorithm for ANY block-level
/// box (the target `Display::Block` maps every block-level `display` value
/// onto, `map_display` below), same as size/margin/padding/border, so they
/// belong here rather than in a display-specific helper like `apply_flex`.
/// This is the wiring the spike (spike/taffy-float-layout, PR #65) proved
/// out: taffy's own block-level float placement (the `float_layout` cargo
/// feature re-enabled in `Cargo.toml`) replaces what was a complete no-op
/// for block-level boxes before this packet -- see
/// `fixtures/evidence/css1-float-5526c.diagnosis.md` for the full
/// diagnosis. The bespoke `layout::inline` float mechanism (floated
/// *inline replaced* atoms, e.g. `<img align=left>`) is untouched: it never
/// reaches this function (inline-level content is folded into a measure-
/// function leaf, see this module's own doc comment), so it keeps handling
/// its own narrower case exactly as before.
///
/// packet/acid1-content-box: `box_sizing` reflects `cs.box_sizing` (real
/// `box-sizing` CSS parsing, `map_box_sizing` below) -- CSS's `ContentBox`
/// initial value applies to every node unless the page explicitly declares
/// `box-sizing: border-box` on it.
fn base_style(cs: &ComputedStyle) -> TStyle {
    TStyle {
        size: TSize { width: map_dimension(cs.width), height: map_dimension(cs.height) },
        // Acid2 Packet 5, Task 1: `min-width`/`max-width`/`min-height`/
        // `max-height` via taffy's OWN native `min_size`/`max_size` -- same
        // `map_dimension` conversion `size` above already uses.
        // `map_dimension(Dimension::Auto)` yields taffy's `auto()`, which is
        // ALSO taffy's own `Style::DEFAULT` for `min_size`/`max_size` (no
        // constraint) -- so an element that never declares any of the four
        // (i.e. `cs.min_width`/etc. are all `Dimension::Auto`, the default)
        // maps byte-identically to the pre-existing `..Default::default()`
        // this literal already falls back to, no golden churn.
        min_size: TSize { width: map_dimension(cs.min_width), height: map_dimension(cs.min_height) },
        max_size: TSize { width: map_dimension(cs.max_width), height: map_dimension(cs.max_height) },
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
        float: map_float(cs.float),
        clear: map_clear(cs.clear),
        box_sizing: box_sizing_for(cs),
        position: map_position(cs.position),
        // CSS 2.1 §9.4.2: `top`/`right`/`bottom`/`left` have NO effect on a
        // statically-positioned box. taffy has no `Static` (we map it to
        // `Relative`, see `map_position`) and WOULD fold any inset into the
        // flow offset, so a static box must map to taffy's auto/no-op insets
        // regardless of its computed `inset`. Positioned boxes use the real
        // offsets.
        inset: if cs.position == Position::Static {
            let auto = map_lpa(LengthPercentageAuto::Auto);
            TRect { left: auto, right: auto, top: auto, bottom: auto }
        } else {
            TRect {
                left: map_lpa(cs.inset.left),
                right: map_lpa(cs.inset.right),
                top: map_lpa(cs.inset.top),
                bottom: map_lpa(cs.inset.bottom),
            }
        },
        ..Default::default()
    }
}

/// `box_sizing` for a node: `map_box_sizing(cs.box_sizing)` (real CSS
/// `box-sizing`, honoring an explicit author declaration) for everything
/// EXCEPT table-internal display types (`Display::Table`/`TableRow`/
/// `TableCell`/`TableRowGroup`), which are hardcoded `BorderBox` regardless
/// of `cs.box_sizing` -- `layout::table`/this module's own `cell_query_
/// width`/`cell_content_layout`/`compute_table_cache_entry` compute a
/// cell's/table's own box dimensions ASSUMING taffy's reported width is a
/// BORDER-box (they add/subtract padding+border themselves on top of that
/// assumption -- see e.g. `cell_content_layout`'s `style.size.width =
/// length(w)` immediately followed by reading the resulting layout's own
/// border/padding back out). Flipping every node to `ContentBox` (this
/// packet's own first attempt, verified by the user's own pixel
/// measurement) makes that path double-count padding+border: a cell grows
/// by its padding+border TWICE (once from taffy's real `ContentBox`
/// growth, once again from the table code's own border-box-shaped
/// arithmetic on top), which doesn't just resize cells -- it desyncs the
/// column solver's column-width sum from what actually got painted,
/// producing a phantom extra (empty, or content-overlapping) column at the
/// table's right edge (`goldens/table-spacing.png`/`table-border.png`, and
/// `kitchen-sink.png`'s embedded table). No fixture ever declares
/// `box-sizing` on a table/row/cell (`grep -c "box-sizing" fixtures/
/// table-*.html fixtures/kitchen-sink.html` is all zeros), so this
/// hardcoding costs nothing real today -- it's the table engine's own
/// well-established (if implicit) border-box contract, not a regression;
/// giving table cells real per-declaration `box-sizing` support is a
/// separate, out-of-scope rabbit hole (the table solver's own arithmetic
/// would need to branch on it too, not just this one field) with zero
/// user-visible benefit -- every fixture's tables already render correctly
/// under the implicit border-box assumption they were written against.
fn box_sizing_for(cs: &ComputedStyle) -> TBoxSizing {
    match cs.display {
        Display::Table | Display::TableRow | Display::TableCell | Display::TableRowGroup => TBoxSizing::BorderBox,
        _ => map_box_sizing(cs.box_sizing),
    }
}

/// Maps Stele's `BoxSizing` (`src/style/computed.rs`, packet/acid1-
/// content-box) onto taffy's `BoxSizing` -- the two enums are shape-
/// identical (`ContentBox`/`BorderBox`), so this is a straight rename, not
/// a semantic translation (mirrors `map_float`/`map_clear` right below).
///
/// This used to be a hand-picked-per-node decision (`float_box_sizing`,
/// later `box_sizing_for`, packet/acid1-coherence's `b88f9cd` and this
/// packet's own first attempt) because the engine had no `box-sizing` CSS
/// property support at all, and taffy 0.13's OWN default (`BoxSizing::
/// BorderBox` -- see `taffy::style::Style`'s own doc comment on the field)
/// disagrees with CSS's real initial value (`content-box`: a declared
/// `width`/`height` is the CONTENT size, padding/border add on top, rather
/// than being subtracted back out of a border-box-pinned declared size).
///
/// `fixtures/css1-float-5526c.html` (the W3C CSS1 Acid1 test) is what
/// surfaced this: EVERY element in it pairs an explicit `width`/`height`
/// with non-zero `padding`/`border`, exactly where `BorderBox` vs
/// `ContentBox` diverge. Scoping the fix to floated elements only (`b88f9cd`)
/// fixed `dd`'s own size but left `dt`/`dd` stacking vertically instead of
/// sitting side by side at their intended exact 470px fit -- instrumenting
/// real fragment geometry (a temporary diagnostic dump, since removed)
/// traced this to `<body>` (`width: 48em; border: .5em solid black`, NOT
/// floated): still `BorderBox`, so its content box came out 470px instead
/// of the CSS-correct 480px -- a real 10px shortfall (`<body>`'s own
/// border), not sub-pixel rounding, that propagated through `<dl>` (auto-
/// width) to shrink `dt`/`dd`'s actual containing block by the same 10px,
/// turning "fits with ~1px to spare" into "overflows by ~9px" against
/// taffy's own (correctly inclusive, `<=` -- verified by reading taffy
/// 0.13's vendored `compute::float::float_fits_horizontally` directly)
/// float-fit check, which correctly wrapped `dd` onto a new line.
///
/// Given that, the CSS-correct fix is unconditional: `ContentBox` is the
/// real default for every element, not just floats -- so this packet adds
/// real `box-sizing` CSS parsing (`Declarations::box_sizing`, `cascade::
/// resolve`) instead of continuing to hand-pick which nodes get which
/// model. `ComputedStyle::default().box_sizing` is `ContentBox`, so any
/// element that never mentions the property gets CSS's real default; a
/// page that explicitly asks for `border-box` (`fixtures/grid.html`'s own
/// `* { box-sizing: border-box; }`) gets exactly that instead. Every OTHER
/// fixture with a byte-compared PNG golden that combines an explicit
/// `width`/`height` with non-zero `padding`/`border` on a plain (non-
/// grid.html) element now legitimately renders BIGGER than before (padding
/// and border add outside the declared size instead of eating into it) --
/// `fixtures/flex-polite.html`'s `aside` and `fixtures/kitchen-sink.html`'s
/// `.flexrow .fixed` -- and their goldens were re-blessed to match, each
/// pixel-verified (this packet's PR description has the measurements) to
/// have grown by EXACTLY their own padding+border, with the rest of the
/// page's layout staying coherent, not merely "different."
fn map_box_sizing(box_sizing: BoxSizing) -> TBoxSizing {
    match box_sizing {
        BoxSizing::ContentBox => TBoxSizing::ContentBox,
        BoxSizing::BorderBox => TBoxSizing::BorderBox,
    }
}

/// Maps Stele's `Float` (`src/style/computed.rs`) onto taffy's `Float`
/// (`float_layout` feature) -- the two enums are shape-identical
/// (`None`/`Left`/`Right`), so this is a straight rename, not a semantic
/// translation.
fn map_float(float: Float) -> TFloat {
    match float {
        Float::None => TFloat::None,
        Float::Left => TFloat::Left,
        Float::Right => TFloat::Right,
    }
}

/// Maps Stele's `Clear` (`src/style/computed.rs`) onto taffy's `Clear`
/// (`float_layout` feature) -- shape-identical (`None`/`Left`/`Right`/
/// `Both`), a straight rename.
fn map_clear(clear: Clear) -> TClear {
    match clear {
        Clear::None => TClear::None,
        Clear::Left => TClear::Left,
        Clear::Right => TClear::Right,
        Clear::Both => TClear::Both,
    }
}

/// Maps Stele's `Position` (`src/style/computed.rs`) onto taffy's
/// `Position`, which -- unlike CSS -- has only two variants: `Relative`
/// (in-flow, offsets applied as a post-layout correction) and `Absolute`
/// (out-of-flow, positioned against the nearest positioned ancestor).
/// `Static` and `Relative` are both in-flow in CSS, so both map to taffy's
/// `Relative`; `base_style` then forces a `Static` box's taffy `inset` to
/// `Auto` (a no-op offset -- CSS 2.1 §9.4.2: offset properties have no
/// effect on a statically-positioned box), so any declared
/// `top`/`right`/`bottom`/`left` is ignored. Only `Relative`/`Absolute`/
/// `Fixed` boxes receive their computed `cs.inset`. `Absolute` and `Fixed`
/// are both out-of-flow, so both map to taffy's `Absolute`; `Fixed`'s
/// viewport-relative containing block (as opposed to `Absolute`'s
/// nearest-positioned-ancestor one) is handled in layout, not here (no
/// scroll in the static render -- see the packet spec).
fn map_position(p: Position) -> TPosition {
    match p {
        Position::Static | Position::Relative => TPosition::Relative,
        Position::Absolute | Position::Fixed => TPosition::Absolute,
    }
}

/// A `Built` node's CSS `position` for paint-order partitioning in `emit`
/// (design §3). `Inline` runs are never positioned blocks, so they count as
/// `Static` (in-flow).
fn built_position(b: &Built) -> Position {
    match b {
        Built::Container { style, .. }
        | Built::Replaced { style, .. }
        | Built::Table { style, .. } => style.position,
        Built::Inline { .. } => Position::Static,
    }
}

/// A `Built` child's z-index paint layer: a positioned element's computed
/// z-index (`Auto` == 0 via `ZIndex::layer`), or 0 for a static/inline child
/// (z-index has no effect on non-positioned boxes; static children are emitted
/// in the in-flow pass regardless). Used only to bucket/sort positioned
/// children in `emit`'s CSS 2.1 Appendix-E paint order.
fn z_layer(b: &Built) -> i32 {
    match b {
        Built::Container { style, .. }
        | Built::Replaced { style, .. }
        | Built::Table { style, .. } => style.z_index.layer(),
        Built::Inline { .. } => 0,
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
    // packet/t3-inline-spacing (the D3 fix): `column_gap` -- when a
    // two-value `gap: <row-gap> <column-gap>` shorthand set one -- governs
    // the WIDTH axis; `gap` alone governs the HEIGHT axis. Before this
    // packet both axes fed from the same single `cs.gap` scalar, which for
    // a two-value declaration (e.g. `fixtures/httpforever.html`'s
    // `.footer__projects { gap: .35rem 1.1rem; }`) meant the row-gap value
    // silently did double duty as the column-gap too -- see
    // `ComputedStyle::column_gap`'s own doc comment and `value::apply_
    // property`'s `"gap"` arm for the full D3 diagnosis.
    let gap = if cs.gap.is_finite() { cs.gap.max(0.0) } else { 0.0 };
    let column_gap = match cs.column_gap {
        Some(v) if v.is_finite() => v.max(0.0),
        _ => gap,
    };
    style.gap = TSize { width: TLengthPercentage::length(column_gap), height: TLengthPercentage::length(gap) };
}

/// packet/css-grid: wires `cs.grid_template_columns`/`.grid_template_rows`
/// onto taffy's `Style.grid_template_columns`/`.grid_template_rows` (the
/// `grid` cargo feature enabled in `Cargo.toml`). A no-op for any container
/// that never declared either property (`cs.grid_template_columns`/`rows`
/// both start `Vec::new()` -- `ComputedStyle::default`) -- taffy's own
/// `Style::DEFAULT` already leaves both empty too, so calling this
/// unconditionally for every container (`translate_any`'s `Container` arm,
/// right after `apply_flex`) changes nothing for a non-grid container,
/// exactly like `apply_flex` itself is already a no-op for one that never
/// set `flex-*`. `gap` needs no separate wiring here -- `apply_flex`
/// (called right before this) already sets `style.gap` unconditionally,
/// and taffy's grid algorithm reads the SAME `Style.gap` field a flex
/// container's does.
fn apply_grid(style: &mut TStyle, cs: &ComputedStyle) {
    if !cs.grid_template_columns.is_empty() {
        style.grid_template_columns = cs.grid_template_columns.iter().map(map_grid_template_component).collect();
    }
    if !cs.grid_template_rows.is_empty() {
        style.grid_template_rows = cs.grid_template_rows.iter().map(map_grid_template_component).collect();
    }
}

/// Maps one Stele `GridTemplateComponent` (`src/style/computed.rs`) onto
/// taffy's own `GridTemplateComponent` -- `Single` maps through
/// [`map_grid_track`] directly, `Repeat` through the same plus taffy's own
/// `repeat()` helper for the count/keyword. Taffy's `GridTemplateComponent`
/// is generic over its named-line-identifier string type (`S:
/// CheapCloneStr`, only used by named grid lines -- unparsed by this
/// packet); `String` (the same type taffy's own `Style::grid_template_
/// columns`/`.rows` field defaults to under the `std` feature this crate
/// already builds with) is the only concretization ever needed here.
fn map_grid_template_component(c: &GridTemplateComponent) -> TGridTemplateComponent<String> {
    match c {
        GridTemplateComponent::Single(track) => TGridTemplateComponent::Single(map_grid_track(track)),
        GridTemplateComponent::Repeat(count, tracks) => {
            let count = match count {
                GridRepetitionCount::Count(n) => TRepetitionCount::Count(*n),
                GridRepetitionCount::AutoFill => TRepetitionCount::AutoFill,
                GridRepetitionCount::AutoFit => TRepetitionCount::AutoFit,
            };
            repeat(count, tracks.iter().map(map_grid_track).collect())
        }
    }
}

/// Maps one Stele `GridTrack` (`src/style/computed.rs`) onto taffy's
/// `TrackSizingFunction`. `Bare` goes through [`map_bare_track_size`] (the
/// generic `length`/`percent`/`fr` helpers construct a WHOLE
/// `TrackSizingFunction` directly, so a bare `1fr` correctly becomes
/// `minmax(auto, 1fr)` -- CSS Grid's own automatic-minimum rule for a bare
/// `<flex>` track, see `GridTrack`'s own doc comment); `MinMax` goes
/// through taffy's `minmax()` helper with each half mapped independently
/// via [`map_grid_min`]/[`map_grid_max`] (an explicit `minmax()` has no
/// automatic-minimum rule to apply -- the author wrote both halves).
fn map_grid_track(track: &GridTrack) -> TTrackSizingFunction {
    match track {
        GridTrack::Bare(size) => map_bare_track_size(*size),
        GridTrack::MinMax(min, max) => minmax(map_grid_min(*min), map_grid_max(*max)),
    }
}

fn map_bare_track_size(size: GridTrackSize) -> TTrackSizingFunction {
    match size {
        GridTrackSize::Length(v) => length(v.max(0.0)),
        GridTrackSize::Percent(p) => percent((p / 100.0).max(0.0)),
        GridTrackSize::Fr(f) => fr(f64::from(f).max(0.0)),
    }
}

/// `MinTrackSizingFunction` has no `fr` constructor (CSS Grid §7.2.3: `fr`
/// is only valid as a track's MAXIMUM) -- a `minmax(<fr>, ...)` first
/// argument is invalid CSS to begin with, so degrading it to `auto` here
/// (rather than making this function fallible and invalidating the whole
/// declaration over one malformed argument) is a safe, total default.
fn map_grid_min(size: GridTrackSize) -> TMinTrackSizingFunction {
    match size {
        GridTrackSize::Length(v) => length(v.max(0.0)),
        GridTrackSize::Percent(p) => percent((p / 100.0).max(0.0)),
        GridTrackSize::Fr(_) => TMinTrackSizingFunction::AUTO,
    }
}

fn map_grid_max(size: GridTrackSize) -> TMaxTrackSizingFunction {
    match size {
        GridTrackSize::Length(v) => length(v.max(0.0)),
        GridTrackSize::Percent(p) => percent((p / 100.0).max(0.0)),
        GridTrackSize::Fr(f) => fr(f64::from(f).max(0.0)),
    }
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
        // These four variants are the marker landed by the display-table
        // freeze amendment. `Display::Table` itself is handled BEFORE this
        // function is ever reached whenever `translate_any` decides to build
        // a real table leaf (see `translate_any`'s `NodeCtx::Table` arm) —
        // this `Block` mapping only fires as the graceful-degrade fallback
        // when the nested-table budget is exhausted (`TABLE_DEPTH_CAP`), or
        // for a `TableRow`/`TableCell`/`TableRowGroup` reached with no table
        // ancestor (orphan markup) — both cases fall back to stacked blocks,
        // total and green, matching pre-table-layout-packet behavior.
        Display::Table => TDisplay::Block,
        Display::TableRow => TDisplay::Block,
        Display::TableCell => TDisplay::Block,
        Display::TableRowGroup => TDisplay::Block,
        // packet/display-list-item: a `display: list-item` box is ordinary
        // block flow for layout purposes -- CSS only special-cases it for
        // marker generation (`layout::box_tree::build_list_container_node`
        // owns that entirely; this function is never consulted for it).
        // Mapping it to the SAME `TDisplay::Block` taffy maps `Display::
        // Block` to means an `<li>` (now `list-item` by default, `style/
        // ua.rs`) occupies the exact same position/size a `display: block`
        // `<li>` always has -- no layout shift for any existing list.
        Display::ListItem => TDisplay::Block,
        // packet/css-grid: hands off to taffy's own grid algorithm (the
        // `grid` cargo feature enabled in `Cargo.toml`) -- see
        // `Display::Grid`'s own doc comment (`style/computed.rs`) for the
        // full contract.
        Display::Grid => TDisplay::Grid,
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
/// text runs positioned within their line boxes, replaced-element
/// placeholders, and (for a table leaf) each cell's own box + content,
/// painted table-box-then-cells (each cell: its own box, then its content),
/// matching the packet brief's paint order.
///
/// Recursion depth here is bounded by construction for the `LayoutNode`-chain
/// axis: `built` is a `Built` tree, and `translate_any` never produces one
/// deeper than [`DEPTH_CAP`] (it stops descending past the cap instead of
/// recursing further), so this walk inherits that bound without needing its
/// own check. The OTHER recursion axis — a table leaf's `emit` arm calling
/// back into `cell_content_layout`, which may itself build+emit a nested
/// table — is bounded by [`TABLE_DEPTH_CAP`] the same way it is during
/// measure (`translate_any` simply stops treating a table as a table once
/// the budget carried in `Built::Table`/`NodeCtx::Table` hits zero).
/// Push either a real `FragmentKind::Image` (when `image` is `Some`) or an
/// M2-era placeholder `FragmentKind::Box` (when `None` — not fetched, fetch/
/// decode failed, or the tty-only pipeline that never populates the images
/// map) at `rect`, for any replaced element: a block-level `Replaced`
/// (`Built::Replaced`), a non-floated inline atom, or a floated atom (M4
/// parts 2/3) all funnel through here so the fallback rule stays in exactly
/// one place. The image itself may be a different pixel size than `rect`
/// (the `width`/`height` attributes that set `intrinsic` aren't required to
/// match the real decoded dimensions) — that mismatch is exactly what
/// `MemSurface::blit`'s nearest-neighbor scaling exists to absorb at paint
/// time; this just reports the box's own rect.
fn push_replaced_fragment(
    out: &mut Vec<Fragment>,
    rect: Rect,
    image: Option<std::rc::Rc<crate::img::RgbaImage>>,
    style: &ComputedStyle,
    interactive: Option<Interactive>,
    clip: Option<Rect>,
    id: Option<Box<str>>,
    is_fixed: bool,
) {
    match image {
        Some(img) => out.push(Fragment {
            rect,
            kind: FragmentKind::Image { image: (*img).clone() },
            interactive,
            clip,
            id,
            is_fixed,
        }),
        None => out.push(Fragment {
            rect,
            kind: FragmentKind::Box { style: style.clone() },
            interactive,
            clip,
            id,
            is_fixed,
        }),
    }
}

/// Intersect an optional clip with a rect (Acid2 Packet 5, Task 2). `None` ∩
/// `r` = `r` (the first clipping ancestor establishes the clip outright);
/// `Some(c)` ∩ `r` = their overlap, floored to a zero-size (not negative-size)
/// rect when they're disjoint -- `emit`'s `Built::Container` arm is this
/// function's only caller, using it to fold a new `overflow:hidden`
/// container's own border box into whatever clip was already in force from
/// an ANCESTOR `overflow:hidden`, so nested clips compose (the innermost
/// always wins, never widens back out).
fn intersect_clip(clip: Option<Rect>, r: Rect) -> Option<Rect> {
    match clip {
        None => Some(r),
        Some(c) => {
            let x0 = c.origin.x.max(r.origin.x);
            let y0 = c.origin.y.max(r.origin.y);
            let x1 = (c.origin.x + c.size.w).min(r.origin.x + r.size.w);
            let y1 = (c.origin.y + c.size.h).min(r.origin.y + r.size.h);
            Some(Rect { origin: Point { x: x0, y: y0 }, size: Size { w: (x1 - x0).max(0.0), h: (y1 - y0).max(0.0) } })
        }
    }
}

/// `is_fixed_ctx`: Acid2 scroll-to-fragment packet, Task 3 (spec §2/§3) —
/// `true` when THIS call is already inside a `position:fixed` subtree (this
/// node's own ancestor chain, up to and including any `Fixed` node itself,
/// has already been reparented onto the viewport). Threaded UNCHANGED
/// through ordinary recursion (a `Fixed` element's own descendants share the
/// SAME frame of reference it does — nesting `position:fixed` inside
/// `position:fixed` is vanishingly rare and CSS still roots both at the
/// ICB), and flipped to `true` for exactly the ONE recursive call the
/// `Container` arm makes into a `position:fixed` child (see that arm's own
/// paint-order loops below). Every `Fragment` this call (or any of its
/// descendants) pushes is stamped `is_fixed: is_fixed_ctx` directly — this
/// single ambient flag is strictly more correct than re-deriving
/// `built_position(built) == Position::Fixed` locally at each push site
/// (Task 1's placeholder): it already covers `Built::Inline`'s text/replaced
/// runs (which have no independently-positioned `style` of their own to
/// read `built_position` from at all) for free, with no separate carve-out
/// needed.
///
/// `viewport_origin`/`viewport_clip`: the initial containing block's origin
/// and (optional) clip, computed ONCE by the top-level caller
/// (`layout_tree_impl`)/`cell_content_layout` and threaded UNCHANGED through
/// every recursive call — the viewport is one fixed frame of reference, not
/// a per-ancestor one. Consulted only by the `Container` arm, to reparent a
/// `position:fixed` child's `parent_origin`/`clip` onto them instead of onto
/// this node's own (DOM-parent-relative) `origin`/`child_clip` (spec §3;
/// closes D55 Finding A: "`Fixed` anchors to its containing block [the body
/// content box], not the viewport root").
fn emit<M: Metrics>(
    built: &Built,
    taffy: &TaffyTree<NodeCtx>,
    parent_origin: Point,
    metrics: &M,
    out: &mut Vec<Fragment>,
    clip: Option<Rect>,
    is_fixed_ctx: bool,
    viewport_origin: Point,
    viewport_clip: Option<Rect>,
) {
    let Ok(layout) = taffy.layout(built.taffy_id()) else { return };
    let origin = Point { x: parent_origin.x + layout.location.x, y: parent_origin.y + layout.location.y };
    let size = Size { w: layout.size.width.max(0.0), h: layout.size.height.max(0.0) };

    match built {
        Built::Container { style, children, interactive, id, .. } => {
            out.push(Fragment {
                rect: Rect { origin, size },
                kind: FragmentKind::Box { style: (*style).clone() },
                interactive: interactive.clone(),
                clip,
                id: id.clone(),
                is_fixed: is_fixed_ctx,
            });
            // Acid2 Packet 5, Task 2: `overflow:hidden` clips this
            // container's DESCENDANTS (not the container's own box fragment
            // above, which still uses the incoming `clip`) to its own
            // border box, intersected with whatever clip was already in
            // force from an ancestor -- see `intersect_clip`'s own doc
            // comment.
            let child_clip = if style.overflow == Overflow::Hidden {
                intersect_clip(clip, Rect { origin, size })
            } else {
                clip
            };
            // CSS 2.1 Appendix E stacking order (back to front). z-index
            // affects only positioned children; static children paint in the
            // in-flow pass regardless. emit() paints each child's whole
            // subtree contiguously (atomic), so ordering positioned siblings
            // by z-index approximates nested stacking contexts (no z-index
            // ⇒ every child z_layer==0 ⇒ identical to P1's [static][positioned]).
            let is_pos = |c: &&Built| built_position(c) != Position::Static;
            // Acid2 scroll-to-fragment packet, Task 3 (spec §3): a
            // `position:fixed` child's containing block is the viewport, NOT
            // this container's own (DOM-parent-relative) `origin`/
            // `child_clip` -- reparent exactly that ONE recursive call onto
            // `viewport_origin`/`viewport_clip`, and flip `is_fixed_ctx` to
            // `true` for it (propagating to every one of ITS OWN
            // descendants, in-flow or not). A non-fixed child keeps the
            // ordinary parent-relative `origin`/`child_clip`, and the
            // ambient `is_fixed_ctx` unchanged (already `true` if THIS whole
            // container is itself nested inside a fixed ancestor). Inlined
            // (not a closure) at each of the four paint-order loops below --
            // a closure capturing `out: &mut Vec<Fragment>` and re-invoked
            // across multiple loops is exactly the kind of `&mut` reborrow
            // subtlety this packet's own CI-only build/test loop (no local
            // `cargo build`) can't afford to gamble on; a plain `if`
            // reproduces the SAME parent-relative reborrow pattern the four
            // loops already used before this packet, just conditionally.
            // 2. negative-z positioned, most-negative first (stable), BEFORE in-flow
            let mut neg: Vec<&Built> = children.iter().filter(|c| is_pos(c) && z_layer(c) < 0).collect();
            neg.sort_by_key(|c| z_layer(c));
            for child in neg {
                if built_position(child) == Position::Fixed {
                    emit(child, taffy, viewport_origin, metrics, out, viewport_clip, true, viewport_origin, viewport_clip);
                } else {
                    emit(child, taffy, origin, metrics, out, child_clip, is_fixed_ctx, viewport_origin, viewport_clip);
                }
            }
            // 3-5. in-flow (static) children, source order -- this bucket is
            // filtered to `Position::Static` children ONLY, so it can never
            // contain a `Fixed` one; no reparenting branch needed here.
            for child in children.iter().filter(|c| built_position(c) == Position::Static) {
                emit(child, taffy, origin, metrics, out, child_clip, is_fixed_ctx, viewport_origin, viewport_clip);
            }
            // 6. z-index auto/0 positioned children, source order
            for child in children.iter().filter(|c| is_pos(c) && z_layer(c) == 0) {
                if built_position(child) == Position::Fixed {
                    emit(child, taffy, viewport_origin, metrics, out, viewport_clip, true, viewport_origin, viewport_clip);
                } else {
                    emit(child, taffy, origin, metrics, out, child_clip, is_fixed_ctx, viewport_origin, viewport_clip);
                }
            }
            // 7. positive-z positioned children, least-positive first (stable)
            let mut pos: Vec<&Built> = children.iter().filter(|c| is_pos(c) && z_layer(c) > 0).collect();
            pos.sort_by_key(|c| z_layer(c));
            for child in pos {
                if built_position(child) == Position::Fixed {
                    emit(child, taffy, viewport_origin, metrics, out, viewport_clip, true, viewport_origin, viewport_clip);
                } else {
                    emit(child, taffy, origin, metrics, out, child_clip, is_fixed_ctx, viewport_origin, viewport_clip);
                }
            }
        }
        Built::Replaced { style, image, interactive, id, .. } => {
            push_replaced_fragment(
                out,
                Rect { origin, size },
                image.clone(),
                style,
                interactive.clone(),
                clip,
                id.clone(),
                is_fixed_ctx,
            );
        }
        Built::Inline { runs, text_align, .. } => {
            let available_w = size.w;
            let laid_out = inline::layout_runs(runs, available_w, *text_align, metrics);
            // Acid2 scroll-to-fragment packet, Task 3: every fragment pushed
            // in this arm shares the ambient `is_fixed_ctx` (see `emit`'s
            // own doc comment) — this resolves Task 1's TODO placeholder:
            // `built_position(built)` on a `Built::Inline` is unconditionally
            // `Position::Static` and could never see a `position:fixed`
            // ANCESTOR's fixed-ness, but `is_fixed_ctx` was already
            // correctly propagated down to this call by the `Container`
            // arm's per-child branch above, regardless of tree depth.
            let is_fixed = is_fixed_ctx;
            for line in &laid_out.lines {
                for run in &line.runs {
                    let run_origin =
                        Point { x: origin.x + line.rect.origin.x + run.x, y: origin.y + line.rect.origin.y };
                    match &runs[run.run_index].content {
                        InlineContent::Text(_) => {
                            out.push(Fragment {
                                rect: Rect { origin: run_origin, size: Size { w: run.width, h: line.rect.size.h } },
                                kind: FragmentKind::Text {
                                    text: run.text.clone(),
                                    baseline: line.baseline,
                                    style: runs[run.run_index].style.clone(),
                                },
                                interactive: runs[run.run_index].interactive.clone(),
                                clip,
                                // No per-run `id` carrier exists for inline
                                // text/replaced runs (documented scope limit
                                // — `find_fragment_top`/`--scroll-to` only
                                // ever need to resolve a Container/Replaced/
                                // Table box's own id, not an inline run's).
                                id: None,
                                is_fixed,
                            });
                        }
                        // A non-floated replaced atom (M4 part 2, the D14
                        // gap): bottom-aligned on the line's baseline (see
                        // `inline::word_metrics`'s "ascent := height,
                        // descent := 0" convention), painted as a real
                        // image when decoded or a placeholder `Box`
                        // otherwise — same fallback rule `Built::Replaced`
                        // already uses below. Height goes through
                        // `inline::clamp_dim` (not the plain `finite_nonneg`
                        // used elsewhere in this file), matching the SAME
                        // clamp `inline::word_metrics` already applies to
                        // this atom's `intrinsic.h` when it sizes the line
                        // box — code review defense-in-depth: `finite_nonneg`
                        // floors negative/non-finite to zero but has no
                        // upper bound, so a hostile `<img height=1e13>`
                        // would otherwise reach `Fragment::rect` uncapped
                        // (harmless today only because `MemSurface::blit`
                        // happens to clip to surface bounds — a downstream
                        // consumer shouldn't be the only guard).
                        InlineContent::Replaced { intrinsic, image } => {
                            let h = inline::clamp_dim(intrinsic.h);
                            let atom_origin =
                                Point { x: run_origin.x, y: origin.y + line.rect.origin.y + (line.baseline - h) };
                            let rect = Rect { origin: atom_origin, size: Size { w: run.width, h } };
                            push_replaced_fragment(
                                out,
                                rect,
                                image.clone(),
                                &runs[run.run_index].style,
                                runs[run.run_index].interactive.clone(),
                                clip,
                                None, // no per-run `id` carrier for inline text/replaced runs (see the Text push's own comment above).
                                is_fixed,
                            );
                        }
                    }
                }
            }
            // Floated atoms (M4 part 3): pulled out of line flow by
            // `inline::layout_runs`, positioned relative to this leaf's own
            // origin exactly like a `LineBox` is.
            for f in &laid_out.floats {
                let float_origin =
                    Point { x: origin.x + f.rect.origin.x, y: origin.y + f.rect.origin.y };
                let rect = Rect { origin: float_origin, size: f.rect.size };
                match &runs[f.run_index].content {
                    InlineContent::Replaced { image, .. } => push_replaced_fragment(
                        out,
                        rect,
                        image.clone(),
                        &runs[f.run_index].style,
                        runs[f.run_index].interactive.clone(),
                        clip,
                        None, // no per-run `id` carrier for inline text/replaced runs (see the Text push's own comment above).
                        is_fixed,
                    ),
                    InlineContent::Text(_) => {} // not reachable: only `Replaced` runs are ever floated.
                }
            }
        }
        Built::Table { style, .. } => {
            // Fetch this leaf's `node`/`table_budget`/cache straight out of
            // the taffy tree's own node-context storage (see `Built::Table`'s
            // doc comment) rather than duplicating them on `Built` — moved
            // ABOVE the table's own box push (below) so `node.id` is in hand
            // for it (Acid2 scroll-to-fragment packet, Task 1).
            let Some(NodeCtx::Table(node, table_budget, cache)) = taffy.get_node_context(built.taffy_id()) else {
                return; // not reachable given how `built` is constructed; degrade rather than panic regardless.
            };
            let node: &LayoutNode = *node;
            let budget = *table_budget;

            // The table's own box first (paint order: table, then cells).
            // Tables aren't themselves interactive in this design (only a
            // link/form-control INSIDE a cell is — see that cell's own
            // fragments below, which carry whatever `interactive` their own
            // source `LayoutNode`s were tagged with via `emit`'s recursive
            // `cell_content_layout` call), so the table's own box fragment
            // carries `None`.
            out.push(Fragment {
                rect: Rect { origin, size },
                kind: FragmentKind::Box { style: (*style).clone() },
                interactive: None,
                clip,
                id: node.id.clone(),
                is_fixed: is_fixed_ctx,
            });

            let content_origin =
                Point { x: parent_origin.x + layout.content_box_x(), y: parent_origin.y + layout.content_box_y() };
            // packet/collapse-geometry: in collapse mode, the whole cell
            // grid is based at the table's own BORDER-BOX origin minus its
            // own frame border width (`content_origin`, shifted back by the
            // border reservation `content_box_x/y()` already added) rather
            // than at the content-box origin — so cell (0,0)'s own border
            // overlaps (and thus coincides with, see this module's own
            // "packet/collapse-geometry" doc section) the table's own frame
            // border instead of sitting flush just past its inner edge. A
            // `Separate` table, or a `Collapse` table with no table-level
            // border (`table_bw == (0.0, 0.0)`), gets `cell_base ==
            // content_origin` exactly — this is a no-op for both.
            let cell_base = if node.style.border_collapse == BorderCollapse::Collapse {
                let (table_bw_x, table_bw_y) = collapse_table_border_widths(&node.style);
                Point { x: content_origin.x - table_bw_x, y: content_origin.y - table_bw_y }
            } else {
                content_origin
            };
            let avail_w = finite_nonneg(layout.content_box_width());
            // Critical-C1 fix (review): reuse the cached solve from measure
            // time (same `avail_w` in the overwhelmingly common case — see
            // `ensure_table_cache`) instead of re-running the whole per-cell
            // measurement pipeline here. `.take()` moves the (non-`Clone`)
            // `Fragment`s out rather than cloning them; `emit` is the
            // terminal consumer of this leaf's cache (a table is only ever
            // emitted once per `layout()` call), so nothing needs it back.
            ensure_table_cache(node, avail_w, metrics, budget, cache);
            let Some(entry) = cache.borrow_mut().take() else { return };

            let cell_rects = entry.table_layout.cell_rects;
            for (i, (_, cell_fragments)) in entry.cell_content.into_iter().enumerate() {
                let Some(rect) = cell_rects.get(i).copied() else { continue };
                let cell_origin = Point { x: cell_base.x + rect.origin.x, y: cell_base.y + rect.origin.y };
                for (fi, f) in cell_fragments.into_iter().enumerate() {
                    // `cell_content_layout`'s FIRST fragment is always the
                    // cell's own root `Box` (see `emit`'s `Container` arm:
                    // it pushes its own box before any child) — sized to the
                    // cell's natural content height, which can be shorter
                    // than its solved `cell_rect` (a rowspan cell's content
                    // is one row tall; its box must cover every row it
                    // spans, like a real table cell's background/border
                    // does). Stretch just that one fragment's height up to
                    // the solved rect; everything else (text, nested boxes)
                    // stays at its natural position — top-aligned within the
                    // taller box (documented M3 simplification: real CSS's
                    // `vertical-align: middle` default for table cells is
                    // not implemented).
                    let h = if fi == 0 { f.rect.size.h.max(rect.size.h) } else { f.rect.size.h };
                    out.push(Fragment {
                        rect: Rect {
                            origin: Point { x: cell_origin.x + f.rect.origin.x, y: cell_origin.y + f.rect.origin.y },
                            size: Size { w: f.rect.size.w, h },
                        },
                        kind: f.kind,
                        interactive: f.interactive,
                        // `f.id`/`f.is_fixed` (Acid2 scroll-to-fragment
                        // packet, Task 1) were already computed correctly by
                        // `cell_content_layout`'s OWN isolated `emit` call —
                        // carried straight through, same as `f.kind` above.
                        id: f.id,
                        is_fixed: f.is_fixed,
                        // Acid2 Packet 5, Task 2: `f.clip` (if any) was
                        // computed by `cell_content_layout`'s OWN isolated
                        // `emit` call, rooted at local origin (0,0) -- it is
                        // NOT in this table's coordinate space (unlike
                        // `f.rect`, which this loop explicitly re-origins via
                        // `cell_origin` above) and can't simply be reused
                        // as-is. Table cells aren't a Task-2 clipping
                        // scenario this packet targets (no fixture nests
                        // `overflow:hidden` inside a `<td>`), so this instead
                        // stamps the ambient `clip` already in force at the
                        // table itself -- the same value the table's own box
                        // fragment just above uses -- which is correct for
                        // every case this packet needs to handle (a table
                        // sitting inside an ancestor `overflow:hidden`).
                        clip,
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_node(s: &str) -> LayoutNode {
        LayoutNode { style: ComputedStyle::default(), content: BoxContent::Text(s.to_string()), children: Vec::new(), interactive: None, id: None }
    }

    fn block_style() -> ComputedStyle {
        ComputedStyle { display: Display::Block, ..ComputedStyle::default() }
    }

    /// CSS's initial `display` value is `inline` (`ComputedStyle::default()`
    /// already carries it — see `computed.rs`'s `impl Default`), matching a
    /// real `<font>`/`<em>`/`<b>` element with no UA/author override.
    fn inline_style() -> ComputedStyle {
        ComputedStyle::default()
    }

    fn container(style: ComputedStyle, children: Vec<LayoutNode>) -> LayoutNode {
        LayoutNode { style, content: BoxContent::Container, children, interactive: None, id: None }
    }

    fn replaced(style: ComputedStyle) -> LayoutNode {
        LayoutNode {
            style,
            content: BoxContent::Replaced { intrinsic: Size { w: 10.0, h: 10.0 }, image: None },
            children: Vec::new(),
            interactive: None, id: None,
        }
    }

    #[test]
    fn list_item_display_lays_out_identically_to_block() {
        // packet/display-list-item: `Display::ListItem` must be
        // layout-equivalent to `Display::Block` -- `map_display` maps both
        // to the SAME `TDisplay::Block` (see its own doc comment), and
        // marker emission is entirely `layout::box_tree`'s concern (never
        // consulted here). Swapping one `<li>`'s `display` between the two
        // values, with otherwise-identical content, must not move or resize
        // ANY fragment -- a real list's items must not shift position now
        // that the UA default changed from `Block` to `ListItem`.
        fn tree_with(li_display: Display) -> LayoutNode {
            let li_style = ComputedStyle { display: li_display, ..ComputedStyle::default() };
            let li = container(li_style, vec![text_node("item")]);
            container(block_style(), vec![li])
        }
        let font = crate::text::BitmapFont::vga_8x16();
        let viewport = Size { w: 400.0, h: 300.0 };
        let block_fragments = layout_tree(&tree_with(Display::Block), viewport, &font);
        let list_item_fragments = layout_tree(&tree_with(Display::ListItem), viewport, &font);
        assert_eq!(block_fragments.len(), list_item_fragments.len(), "same shape must produce the same fragment count");
        for (b, l) in block_fragments.iter().zip(list_item_fragments.iter()) {
            assert_eq!(b.rect, l.rect, "Display::ListItem must lay out at the exact position/size Display::Block would");
        }
    }

    /// Acid2 Packet 5, Task 2: an `overflow:hidden` container clips its
    /// DESCENDANTS, never its own box fragment -- `emit`'s `Built::
    /// Container` arm pushes the container's own box with the INCOMING
    /// clip (here `None`, the tree's root), then computes a NEW clip
    /// (`intersect_clip`'d to the container's own border box) for its
    /// children only.
    #[test]
    fn overflow_hidden_clips_descendants_not_the_container_itself() {
        let hidden_style = ComputedStyle { display: Display::Block, overflow: Overflow::Hidden, ..ComputedStyle::default() };
        let tree = container(hidden_style, vec![text_node("hi")]);
        let font = crate::text::BitmapFont::vga_8x16();
        let viewport = Size { w: 200.0, h: 200.0 };
        let fragments = layout_tree(&tree, viewport, &font);

        let container_box =
            fragments.iter().find(|f| matches!(f.kind, FragmentKind::Box { .. })).expect("container's own box fragment");
        assert_eq!(container_box.clip, None, "the container is NOT clipped by its own overflow:hidden");

        let child_text =
            fragments.iter().find(|f| matches!(f.kind, FragmentKind::Text { .. })).expect("the child text fragment");
        assert_eq!(
            child_text.clip,
            Some(container_box.rect),
            "the child must be clipped to the container's own border box"
        );
    }

    // ---- Acid2 scroll-to-fragment packet, Task 1: `id`/`is_fixed` carriers ----

    /// A `<div id="target">`'s id must land on exactly one `Fragment` — its
    /// own `Box` fragment — never on a descendant text run's `Fragment`
    /// (`find_fragment_top`, Task 2, relies on this to unambiguously resolve
    /// an id to a single box).
    #[test]
    fn emit_stamps_id_onto_the_owning_box_fragment_only() {
        let mut target = container(block_style(), vec![text_node("hi")]);
        target.id = Some("target".into());
        let root = container(block_style(), vec![target]);
        let font = crate::text::BitmapFont::vga_8x16();
        let fragments = layout_tree(&root, Size { w: 200.0, h: 100.0 }, &font);

        let matches: Vec<&Fragment> = fragments.iter().filter(|f| f.id.as_deref() == Some("target")).collect();
        assert_eq!(matches.len(), 1, "exactly one fragment should carry #target's id, got {}", matches.len());
        assert!(
            matches!(matches[0].kind, FragmentKind::Box { .. }),
            "#target's id must land on its own Box fragment, not a descendant text run"
        );
    }

    /// A `position: fixed` node's own `Box` fragment must carry
    /// `is_fixed == true`; an ordinary `position: static` sibling's must not.
    #[test]
    fn fixed_positioned_box_fragment_is_flagged_is_fixed() {
        let fixed_style = ComputedStyle { position: Position::Fixed, ..ComputedStyle::default() };
        let mut fixed_child = container(fixed_style, vec![text_node("fixed")]);
        fixed_child.id = Some("fixed".into());
        let mut static_child = container(block_style(), vec![text_node("static")]);
        static_child.id = Some("static".into());
        // `id`s (not paint-order position) locate each child's own box below
        // -- a `position:fixed` child is a POSITIONED child, so CSS 2.1
        // Appendix E's paint order (`emit`'s own z-index bucketing) emits it
        // AFTER the in-flow static sibling, regardless of source order.
        let root = container(block_style(), vec![fixed_child, static_child]);
        let font = crate::text::BitmapFont::vga_8x16();
        let fragments = layout_tree(&root, Size { w: 200.0, h: 100.0 }, &font);

        let box_with_id = |id: &str| -> &Fragment {
            fragments
                .iter()
                .find(|f| f.id.as_deref() == Some(id) && matches!(f.kind, FragmentKind::Box { .. }))
                .unwrap_or_else(|| panic!("expected a Box fragment with id {id:?}"))
        };
        assert!(box_with_id("fixed").is_fixed, "the position:fixed child's own box must be flagged is_fixed");
        assert!(!box_with_id("static").is_fixed, "the position:static sibling's own box must NOT be flagged is_fixed");
    }

    // ---- Acid2 scroll-to-fragment packet, Task 3: viewport-anchored position:fixed ----

    fn relative_style(width: f32, height: f32) -> ComputedStyle {
        ComputedStyle { display: Display::Block, position: Position::Relative, width: CssDimension::Px(width), height: CssDimension::Px(height), ..ComputedStyle::default() }
    }

    /// A `position:fixed;top:N;left:M` box with no `right`/`bottom` --
    /// mirrors `fixtures/pos-nested.html`'s own shape (a leading spacer
    /// pushes a `position:relative` ancestor away from the viewport origin,
    /// which then wraps the fixed descendant).
    fn fixed_box(id: &str, top: f32, left: f32, size: f32) -> LayoutNode {
        let mut style = ComputedStyle {
            display: Display::Block,
            position: Position::Fixed,
            width: CssDimension::Px(size),
            height: CssDimension::Px(size),
            ..ComputedStyle::default()
        };
        style.inset.top = LengthPercentageAuto::Px(top);
        style.inset.left = LengthPercentageAuto::Px(left);
        let mut node = container(style, Vec::new());
        node.id = Some(id.into());
        node
    }

    fn find_box<'a>(fragments: &'a [Fragment], id: &str) -> &'a Fragment {
        fragments
            .iter()
            .find(|f| f.id.as_deref() == Some(id) && matches!(f.kind, FragmentKind::Box { .. }))
            .unwrap_or_else(|| panic!("expected a Box fragment with id {id:?}"))
    }

    /// The core D55 Finding A regression test: a `position:fixed` box
    /// nested inside a `position:relative` ancestor that itself sits far
    /// from the viewport origin (pushed down by 300px of leading flow
    /// content, mirroring `pos-nested.html`) must resolve to its OWN
    /// `top`/`left` insets, viewport-relative -- NOT
    /// `ancestor_origin + (left, top)`, today's wrong parent-relative answer
    /// (this test is red against pre-Task-3 `emit`).
    #[test]
    fn position_fixed_anchors_to_the_viewport_not_the_dom_parent() {
        let spacer_tall = {
            let mut s = block_style();
            s.height = CssDimension::Px(300.0);
            container(s, Vec::new())
        };
        let fixed = fixed_box("fixed", 20.0, 15.0, 50.0);
        let relative_ancestor = container(relative_style(200.0, 200.0), vec![fixed]);
        let root = container(block_style(), vec![spacer_tall, relative_ancestor]);

        let font = crate::text::BitmapFont::vga_8x16();
        let fragments = layout_tree(&root, Size { w: 400.0, h: 800.0 }, &font);

        let fixed_frag = find_box(&fragments, "fixed");
        assert_eq!(
            fixed_frag.rect.origin,
            Point { x: 15.0, y: 20.0 },
            "a position:fixed box's origin must be its OWN top/left insets, viewport-relative -- \
             NOT offset by its (300px-pushed-down) DOM ancestor's own origin"
        );
    }

    /// Same shape, but the fixed descendant sits TWO `position:relative`
    /// levels deep (fixed inside relative inside relative), each pushed
    /// further from the origin -- proves `viewport_origin`/`viewport_clip`
    /// really do pass through `emit`'s recursion UNCHANGED regardless of
    /// depth, not just one level.
    #[test]
    fn position_fixed_anchoring_is_unaffected_by_ancestor_nesting_depth() {
        let spacer_tall = {
            let mut s = block_style();
            s.height = CssDimension::Px(300.0);
            container(s, Vec::new())
        };
        let fixed = fixed_box("fixed", 20.0, 15.0, 50.0);
        let inner_relative = container(relative_style(150.0, 150.0), vec![fixed]);
        let outer_relative = container(relative_style(250.0, 250.0), vec![inner_relative]);
        let root = container(block_style(), vec![spacer_tall, outer_relative]);

        let font = crate::text::BitmapFont::vga_8x16();
        let fragments = layout_tree(&root, Size { w: 400.0, h: 900.0 }, &font);

        let fixed_frag = find_box(&fragments, "fixed");
        assert_eq!(
            fixed_frag.rect.origin,
            Point { x: 15.0, y: 20.0 },
            "two levels of position:relative ancestor nesting must not change a fixed \
             descendant's viewport-anchored origin"
        );
    }

    /// The clip half of §3 (load-bearing for Task 4's own clip-gate test):
    /// a `position:fixed` element inside an `overflow:hidden` root (via
    /// `layout_viewport`'s clamped-height path, reused from packet/
    /// fixed-viewport) must carry the VIEWPORT's own clip rect, not `None`
    /// and not some intermediate ancestor's clip.
    #[test]
    fn position_fixed_inside_an_overflow_hidden_root_gets_the_viewport_clip() {
        let hidden_root_style = ComputedStyle { display: Display::Block, overflow: Overflow::Hidden, ..ComputedStyle::default() };
        let fixed = fixed_box("fixed", 10.0, 10.0, 20.0);
        let root = container(hidden_root_style, vec![fixed]);

        let font = crate::text::BitmapFont::vga_8x16();
        let viewport = Size { w: 200.0, h: 150.0 };
        let fragments = layout_tree_viewport(&root, viewport, &font);

        let fixed_frag = find_box(&fragments, "fixed");
        let expected_viewport_rect = Rect { origin: Point { x: 0.0, y: 0.0 }, size: viewport };
        assert_eq!(
            fixed_frag.clip,
            Some(expected_viewport_rect),
            "a position:fixed descendant of an overflow:hidden root must carry the viewport's own clip"
        );
    }

    #[test]
    fn contains_block_descendant_true_for_inline_wrapping_a_block_list() {
        // <font><ol><li>a</li></ol></font> -- the exact 68k.news shape.
        let li = container(block_style(), vec![text_node("a")]);
        let ol = container(block_style(), vec![li]);
        let font = container(inline_style(), vec![ol]);
        assert!(contains_block_descendant(&font, 0));
    }

    #[test]
    fn contains_block_descendant_true_through_a_chain_of_inline_wrappers() {
        // <font><b><ol><li>a</li></ol></b></font> -- the block descendant is
        // two inline levels down, not a direct child.
        let li = container(block_style(), vec![text_node("a")]);
        let ol = container(block_style(), vec![li]);
        let b = container(inline_style(), vec![ol]);
        let font = container(inline_style(), vec![b]);
        assert!(contains_block_descendant(&font, 0));
    }

    #[test]
    fn contains_block_descendant_false_for_inline_wrapping_only_text() {
        // <em>hello</em>
        let em = container(inline_style(), vec![text_node("hello")]);
        assert!(!contains_block_descendant(&em, 0));
    }

    #[test]
    fn contains_block_descendant_false_for_inline_wrapping_a_replaced_atom() {
        // <em><img></em> -- a Replaced element is an inline atom, never
        // block-level, regardless of nesting (D14 regression guard).
        let em = container(inline_style(), vec![replaced(inline_style())]);
        assert!(!contains_block_descendant(&em, 0));
    }

    #[test]
    fn contains_block_descendant_false_for_a_leaf_inline_container() {
        let em = container(inline_style(), Vec::new());
        assert!(!contains_block_descendant(&em, 0));
    }

    #[test]
    fn is_inline_ish_false_for_inline_container_with_a_block_descendant() {
        let li = container(block_style(), vec![text_node("a")]);
        let ol = container(block_style(), vec![li]);
        let font = container(inline_style(), vec![ol]);
        assert!(!is_inline_ish(&font), "an inline container holding a block box must not be folded into an IFC leaf");
    }

    #[test]
    fn is_inline_ish_true_for_inline_container_with_only_text() {
        let em = container(inline_style(), vec![text_node("hello")]);
        assert!(is_inline_ish(&em));
    }

    /// D14 regression guard: `<em><img></em>` (an inline container wrapping
    /// a non-block `Replaced` atom) must still fold into one inline run.
    #[test]
    fn is_inline_ish_true_for_inline_container_wrapping_a_replaced_atom() {
        let em = container(inline_style(), vec![replaced(inline_style())]);
        assert!(is_inline_ish(&em), "em wrapping an img must stay inline-ish (D14)");
    }

    #[test]
    fn is_inline_ish_false_for_a_block_container() {
        let div = container(block_style(), vec![text_node("hello")]);
        assert!(!is_inline_ish(&div));
    }

    // ---- packet/acid1-content-box: `map_box_sizing` ----

    #[test]
    fn map_box_sizing_is_a_straight_rename() {
        assert_eq!(map_box_sizing(BoxSizing::ContentBox), TBoxSizing::ContentBox);
        assert_eq!(map_box_sizing(BoxSizing::BorderBox), TBoxSizing::BorderBox);
    }

    #[test]
    fn base_style_defaults_to_content_box_for_an_ordinary_element() {
        // ComputedStyle::default().box_sizing is ContentBox (CSS's real
        // initial value) -- an element that never declares `box-sizing`
        // (e.g. `<body>`/`<dl>` in fixtures/css1-float-5526c.html) must get
        // it, not taffy's own BorderBox default.
        let style = base_style(&ComputedStyle::default());
        assert_eq!(style.box_sizing, TBoxSizing::ContentBox);
    }

    #[test]
    fn base_style_maps_min_max_width_height_to_taffys_native_min_max_size() {
        // Acid2 Packet 5, Task 1. Default (Dimension::Auto on all four)
        // must map to taffy's own no-constraint `auto()` -- identical to
        // what `..Default::default()` already produced, so no golden churn
        // for any element that never declares min-/max-width/height.
        let default_style = base_style(&ComputedStyle::default());
        assert_eq!(default_style.min_size.width, auto());
        assert_eq!(default_style.min_size.height, auto());
        assert_eq!(default_style.max_size.width, auto());
        assert_eq!(default_style.max_size.height, auto());

        let cs = ComputedStyle { min_width: CssDimension::Px(80.0), ..ComputedStyle::default() };
        let style = base_style(&cs);
        assert_eq!(style.min_size.width, length(80.0));
        // Untouched fields stay at their no-constraint default.
        assert_eq!(style.min_size.height, auto());
        assert_eq!(style.max_size.width, auto());
        assert_eq!(style.max_size.height, auto());
    }

    #[test]
    fn base_style_honors_an_explicit_border_box_declaration() {
        // fixtures/grid.html's `* { box-sizing: border-box; }`.
        let cs = ComputedStyle { box_sizing: BoxSizing::BorderBox, ..ComputedStyle::default() };
        let style = base_style(&cs);
        assert_eq!(style.box_sizing, TBoxSizing::BorderBox);
    }

    // ---- Acid2 P1 Task 2: `position`/`inset` -> taffy ----

    #[test]
    fn base_style_maps_position_and_inset_to_taffy() {
        // Static (the default) and Relative are both in-flow in CSS, and
        // taffy has no `Static` variant -- both map to taffy's `Relative`.
        assert_eq!(base_style(&ComputedStyle::default()).position, TPosition::Relative);
        let rel = ComputedStyle { position: Position::Relative, ..ComputedStyle::default() };
        assert_eq!(base_style(&rel).position, TPosition::Relative);

        // Absolute and Fixed are both out-of-flow -- both map to taffy's
        // Absolute (Fixed's viewport containing block is a layout concern,
        // not this mapping's).
        let abs = ComputedStyle { position: Position::Absolute, ..ComputedStyle::default() };
        assert_eq!(base_style(&abs).position, TPosition::Absolute);
        let fixed = ComputedStyle { position: Position::Fixed, ..ComputedStyle::default() };
        assert_eq!(base_style(&fixed).position, TPosition::Absolute);

        // A static box's insets are all Auto (ComputedStyle::default()),
        // which must map to taffy's own all-Auto default inset -- i.e. no
        // offset, golden-safe for every existing (non-positioned) fixture.
        let default_inset = base_style(&ComputedStyle::default()).inset;
        let auto = map_lpa(LengthPercentageAuto::Auto);
        assert_eq!(default_inset.top, auto);
        assert_eq!(default_inset.right, auto);
        assert_eq!(default_inset.bottom, auto);
        assert_eq!(default_inset.left, auto);

        // `inset.top` uses the SAME per-edge `LengthPercentageAuto` ->
        // taffy conversion `margin` already uses -- setting both to the
        // same length must produce identical taffy outputs.
        let mut cs = ComputedStyle { position: Position::Absolute, ..ComputedStyle::default() };
        cs.inset.top = LengthPercentageAuto::Px(10.0);
        cs.margin.top = LengthPercentageAuto::Px(10.0);
        let ts = base_style(&cs);
        assert_eq!(ts.inset.top, ts.margin.top);
        assert_eq!(ts.inset.top, TLengthPercentageAuto::length(10.0));
    }

    #[test]
    fn static_box_ignores_inset_offsets() {
        // CSS 2.1 §9.4.2: top/right/bottom/left have no effect on a static
        // box, even though they have computed values. A static box must map
        // to taffy's all-Auto inset regardless of what `cs.inset` says.
        let mut cs = ComputedStyle::default(); // position defaults to Static
        cs.inset.top = LengthPercentageAuto::Px(50.0);
        cs.inset.left = LengthPercentageAuto::Px(10.0);
        let auto = map_lpa(LengthPercentageAuto::Auto);
        let ts = base_style(&cs);
        assert_eq!(ts.position, TPosition::Relative);
        assert_eq!(ts.inset.top, auto, "static box must ignore `top`");
        assert_eq!(ts.inset.left, auto, "static box must ignore `left`");
        assert_eq!(ts.inset.right, auto);
        assert_eq!(ts.inset.bottom, auto);
    }
}
