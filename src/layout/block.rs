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
use crate::layout::table::{self, CellSpec, TableLayout, TableSpec};
use crate::layout::table_layout::{self, Grid};
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

/// The maximum number of *tables nested inside table cells* `translate_any`
/// will treat as a real table (build a leaf for + run [`solve_table_for_node`]
/// on). Distinct from [`DEPTH_CAP`]: `DEPTH_CAP` bounds one `LayoutNode`
/// parent/child chain's recursion within a single `translate_any` walk, but
/// each table's cell content is measured/laid out via its OWN fresh
/// `translate_any` walk (see `cell_min_max_width`/`cell_content_layout`) —
/// walks that are themselves invoked recursively (from inside the OUTER
/// table's own measure/emit) whenever a cell contains another table. A
/// pathological "table in a cell in a table in a cell in ..." bomb would
/// otherwise nest these walks (and the real native call stack frames that
/// come with each: `compute_layout_with_measure`, `emit`, `translate_any`)
/// without limit — a guard-page fault (SIGABRT), same failure class
/// `DEPTH_CAP` exists to prevent, just via a different recursion path. Once
/// the budget hits `0`, `translate_any` stops treating a `Display::Table`
/// node as a table at all: it falls back to the pre-existing plain-block
/// translation (see `map_display`'s `Display::Table => TDisplay::Block`
/// arm) — an over-deep nested table degrades to its rows/cells rendering as
/// stacked blocks, not a crash.
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
/// simplification — see the packet report).
const TABLE_DEPTH_CAP: usize = 2;

/// Horizontal gap inserted between adjacent columns — see
/// `solve_table_for_node`'s "Border-spacing" doc note. `8.0` (one full
/// `text::BitmapFont::vga_8x16` cell), not CSS's real `2px` initial value:
/// `backend::tty::render` maps continuous layout-pixel x-coordinates to
/// discrete character columns by *rounding* to the nearest cell (`col =
/// round(x / 8.0)`), so any sub-cell gap smaller than half a cell (4px) can
/// round away to nothing whenever the preceding column's own content
/// already exactly fills its whole-character width — e.g. a 2px gap after a
/// column whose widest cell is exactly N characters wide rounds `col0_width
/// + 2` right back down to the same cell `col0_width` lands on, so the next
/// column's text visually touches it with zero rendered gap. A full 8px
/// cell-width gap has no such rounding ambiguity (`round((w + 8) / 8) ==
/// round(w / 8) + 1`, always). Vertical spacing stays `0` — rows are
/// already visually separated by moving to a new tty row.
const BORDER_SPACING_X: f32 = 8.0;
const BORDER_SPACING_Y: f32 = 0.0;

/// The taffy node-context type: either a folded inline-formatting-context
/// leaf's runs (pre-existing, P6), or a table leaf's source `LayoutNode` +
/// remaining nested-table budget (see [`TABLE_DEPTH_CAP`]). A table's cell
/// content, min/max-content widths, and row heights are NOT computed eagerly
/// at translate time (the table's own final width depends on parent-supplied
/// available space, only known once taffy visits this leaf during layout) —
/// so, exactly like the pre-existing text/inline leaves, a table leaf defers
/// its real sizing work to the measure function (`measure_node`), which
/// pattern-matches this enum.
enum NodeCtx<'a> {
    Inline(Vec<InlineRun>),
    Table(&'a LayoutNode, usize),
}

/// A translated node: enough provenance back to the source [`LayoutNode`]
/// (by style reference) to emit the right fragment at the right rect once
/// taffy has computed final layout.
enum Built<'a> {
    Container { style: &'a ComputedStyle, taffy_id: TNodeId, children: Vec<Built<'a>> },
    Inline { taffy_id: TNodeId, runs: Vec<InlineRun> },
    Replaced { style: &'a ComputedStyle, taffy_id: TNodeId, intrinsic: Size },
    /// A `display: table` box, translated as a single bespoke leaf (module
    /// docs): `node` is the table's own `LayoutNode` (re-walked by `emit` to
    /// paint cells — the grid/solve isn't cached between measure and emit,
    /// see the DECISIONS note on this documented perf simplification), and
    /// `table_budget` is the remaining nested-table budget cell content may
    /// use (see [`TABLE_DEPTH_CAP`]).
    Table { style: &'a ComputedStyle, taffy_id: TNodeId, node: &'a LayoutNode, table_budget: usize },
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
    let mut taffy: TaffyTree<NodeCtx> = TaffyTree::new();
    let built = translate_any(root, &mut taffy, 0, TABLE_DEPTH_CAP);

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
        |known_dimensions, available_space, node_id, node_context, style| {
            measure_node(known_dimensions, available_space, node_id, node_context, style, metrics)
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

/// Translate one `LayoutNode` (any content kind) into a taffy node. `depth`
/// is this node's own `LayoutNode`-chain nesting depth (root = 0); see
/// [`DEPTH_CAP`]. `table_budget` is the remaining nested-table budget (see
/// [`TABLE_DEPTH_CAP`]) — carried through unchanged for ordinary
/// descent, and consumed by one when a `Display::Table` node is actually
/// turned into a table leaf.
fn translate_any<'a>(
    node: &'a LayoutNode,
    taffy: &mut TaffyTree<NodeCtx<'a>>,
    depth: usize,
    table_budget: usize,
) -> Built<'a> {
    match &node.content {
        BoxContent::Text(text) => {
            let runs = vec![InlineRun { text: text.clone(), style: node.style.clone() }];
            let style = base_style(&node.style);
            let id = taffy
                .new_leaf_with_context(style, NodeCtx::Inline(runs.clone()))
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
        // A `display: table` box (real HTML `<table>`, or any element styled
        // `display: table`) becomes a single bespoke leaf — see the module
        // docs' "table" bullet and `measure_node`'s `NodeCtx::Table` arm —
        // UNLESS the nested-table budget is exhausted, in which case it
        // falls through to the plain-block translation below exactly like
        // it did before this packet (see [`TABLE_DEPTH_CAP`]).
        BoxContent::Container if node.style.display == Display::Table && table_budget > 0 => {
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
            let id = taffy
                .new_leaf_with_context(style, NodeCtx::Table(node, table_budget - 1))
                .expect("taffy leaf alloc is infallible for a fresh tree");
            Built::Table { style: &node.style, taffy_id: id, node, table_budget: table_budget - 1 }
        }
        // TableCell reached here means it's outside a table-leaf's own cell
        // walk (an orphan `<td>` with no table ancestor, or one under a
        // budget-exhausted table) — translates exactly like a Container, a
        // plain stacked block, matching pre-table-layout-packet behavior.
        BoxContent::Container | BoxContent::TableCell { .. } => {
            let mut style = base_style(&node.style);
            style.display = map_display(node.style.display);
            apply_flex(&mut style, &node.style);
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
        // A table (`display: table`) is never inline-ish — it's routed to
        // `translate_any`'s dedicated table-leaf branch, same as any other
        // non-inline `Container`, regardless of this check. A `TableCell`
        // reaching here (an orphan `<td>`, see `translate_any`) is routed
        // exactly like a plain Container, matching pre-table-layout-packet
        // behavior.
        BoxContent::Container | BoxContent::TableCell { .. } => n.style.display == Display::Inline,
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
    taffy: &mut TaffyTree<NodeCtx<'a>>,
    depth: usize,
    table_budget: usize,
) -> Vec<Built<'a>> {
    let mut out = Vec::new();
    if node.style.display == Display::Flex {
        for child in &node.children {
            out.push(translate_any(child, taffy, depth, table_budget));
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
                .new_leaf_with_context(style, NodeCtx::Inline(runs.clone()))
                .expect("taffy leaf alloc is infallible for a fresh tree");
            out.push(Built::Inline { taffy_id: id, runs });
            i = j;
        } else {
            out.push(translate_any(&node.children[i], taffy, depth, table_budget));
            i += 1;
        }
    }
    out
}

/// The measure function threaded through every `compute_layout_with_measure`
/// call in this module — the top-level `layout_tree`, and every nested
/// per-cell/per-table sub-tree built while solving a table (see
/// `cell_query_width`/`cell_content_layout`/`solve_table_for_node`).
/// Dispatches on the leaf's [`NodeCtx`]: `Inline` runs go through the
/// bespoke inline engine (pre-existing, P6, unchanged); `Table` leaves run
/// the whole grid-placement + column/row solve pipeline
/// (`solve_table_for_node`) and report its total content size.
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
        Some(NodeCtx::Inline(runs)) => {
            let out = inline::layout_runs(runs, avail_w, metrics);
            TSize {
                width: known_dimensions.width.unwrap_or(out.size.w),
                height: known_dimensions.height.unwrap_or(out.size.h),
            }
        }
        Some(NodeCtx::Table(table_node, table_budget)) => {
            let (grid, solved) = solve_table_for_node(table_node, finite_nonneg(avail_w), metrics, *table_budget);
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
            let col_gaps = grid.columns.saturating_sub(1) as f32;
            let row_gaps = grid.rows.saturating_sub(1) as f32;
            let total_w = finite_nonneg(solved.col_widths.iter().sum::<f32>() + col_gaps * BORDER_SPACING_X);
            let total_h = finite_nonneg(solved.row_heights.iter().sum::<f32>() + row_gaps * BORDER_SPACING_Y);
            TSize {
                width: known_dimensions.width.unwrap_or(total_w),
                height: known_dimensions.height.unwrap_or(total_h),
            }
        }
    }
}

/// Run the full table pipeline for `table_node`'s own subtree at
/// `available_width` (the CSS auto-table-layout "available width" the
/// packet brief's step 3 asks for, sourced from taffy/its parent — see
/// `measure_node`'s `NodeCtx::Table` arm and `emit`'s `Built::Table` arm,
/// the two callers): place the grid
/// ([`table_layout::place_grid`]), measure each cell's min/max content
/// width ([`cell_min_max_width`]), solve column widths (pass 1), re-measure
/// each cell's real content height at its solved width
/// ([`cell_content_layout`]), then solve again with real heights to get
/// final row heights + cell rects (pass 2) — the two-stage phasing
/// documented in the packet report. `table_budget` is the nested-table
/// budget cell content may spend (see [`TABLE_DEPTH_CAP`]).
///
/// Border-spacing (documented M3 simplification, see the packet report):
/// `style::ComputedStyle` has no `border-spacing` property to read (a
/// frozen type this packet may not extend), so spacing is a fixed constant
/// rather than CSS-driven — [`BORDER_SPACING_X`]/[`BORDER_SPACING_Y`] (see
/// their own doc comments for why `8px`/`0px`, not CSS's real `2px 2px`
/// initial value). Without SOME nonzero horizontal spacing, two abutting
/// cells whose content exactly fills their column can visually run
/// together in the tty text-mode dump — which paints no backgrounds/
/// borders at all (see `backend::tty`'s own documented scope call) — e.g.
/// `"Qty"` immediately followed by `"Notes"` reading as `"QtyNotes"`.
///
/// Total: every step this calls (`place_grid`, `solve_table`,
/// `cell_min_max_width`, `cell_content_layout`) is itself total; this
/// function adds no new panic surface.
fn solve_table_for_node<'a, M: Metrics>(
    table_node: &'a LayoutNode,
    available_width: f32,
    metrics: &M,
    table_budget: usize,
) -> (Grid<'a>, TableLayout) {
    let grid = table_layout::place_grid(table_node);
    let spacing_x = BORDER_SPACING_X;
    let spacing_y = BORDER_SPACING_Y;

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

    let pass1 = table::solve_table(&TableSpec {
        columns: grid.columns,
        rows: grid.rows,
        cells: cells.clone(),
        available_width,
        border_spacing_x: spacing_x,
        border_spacing_y: spacing_y,
    });

    for (i, gc) in grid.cells.iter().enumerate() {
        let assigned_w = pass1.cell_rects.get(i).map(|r| r.size.w).unwrap_or(0.0);
        let (size, _fragments) = cell_content_layout(gc.node, assigned_w, metrics, table_budget);
        if let Some(cell) = cells.get_mut(i) {
            cell.intrinsic_height = size.h;
        }
    }

    let layout = table::solve_table(&TableSpec {
        columns: grid.columns,
        rows: grid.rows,
        cells,
        available_width,
        border_spacing_x: spacing_x,
        border_spacing_y: spacing_y,
    });

    (grid, layout)
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
    let built = translate_any(node, &mut taffy, 0, table_budget);
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
/// track. Called twice per cell during a full table solve (once for
/// `intrinsic_height` during the measure phase via `solve_table_for_node`,
/// once more during `emit` for the real fragments) — a documented perf
/// simplification (see the packet report): nothing is cached across the
/// split. Fine at this scope (small documents, not a hot path); the first
/// thing to fix in a follow-up perf pass.
fn cell_content_layout<M: Metrics>(node: &LayoutNode, width: f32, metrics: &M, table_budget: usize) -> (Size, Vec<Fragment>) {
    let mut taffy: TaffyTree<NodeCtx> = TaffyTree::new();
    let built = translate_any(node, &mut taffy, 0, table_budget);
    let w = finite_nonneg(width);
    if let Ok(mut style) = taffy.style(built.taffy_id()).cloned() {
        style.size.width = length(w);
        let _ = taffy.set_style(built.taffy_id(), style);
    }
    let available = TSize { width: AvailableSpace::Definite(w), height: AvailableSpace::MaxContent };
    let _ = taffy.compute_layout_with_measure(built.taffy_id(), available, |kd, av, id, ctx, style| {
        measure_node(kd, av, id, ctx, style, metrics)
    });
    let mut fragments = Vec::new();
    emit(&built, &taffy, Point { x: 0.0, y: 0.0 }, metrics, &mut fragments);
    let size = taffy
        .layout(built.taffy_id())
        .map(|l| Size { w: finite_nonneg(l.size.width), h: finite_nonneg(l.size.height) })
        .unwrap_or_default();
    (size, fragments)
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
fn emit<M: Metrics>(built: &Built, taffy: &TaffyTree<NodeCtx>, parent_origin: Point, metrics: &M, out: &mut Vec<Fragment>) {
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
        Built::Table { style, node, table_budget, .. } => {
            // The table's own box first (paint order: table, then cells).
            out.push(Fragment { rect: Rect { origin, size }, kind: FragmentKind::Box { style: (*style).clone() } });

            // Re-solve (grid placement + column/row solve) at the table's
            // now-final content-box width/geometry — not cached from
            // measure time; see `cell_content_layout`'s doc comment on this
            // documented perf simplification.
            let content_origin =
                Point { x: parent_origin.x + layout.content_box_x(), y: parent_origin.y + layout.content_box_y() };
            let avail_w = finite_nonneg(layout.content_box_width());
            let node: &LayoutNode = *node;
            let budget = *table_budget;
            let (grid, solved) = solve_table_for_node(node, avail_w, metrics, budget);

            for (i, gc) in grid.cells.iter().enumerate() {
                let Some(rect) = solved.cell_rects.get(i) else { continue };
                let cell_origin = Point { x: content_origin.x + rect.origin.x, y: content_origin.y + rect.origin.y };
                // Each cell's own box + content, translated into place —
                // `cell_content_layout` already produced this exactly as
                // `layout_tree` would for a standalone document, relative to
                // the cell's own (0, 0) border-box origin.
                let (_, cell_fragments) = cell_content_layout(gc.node, rect.size.w, metrics, budget);
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
                    });
                }
            }
        }
    }
}
