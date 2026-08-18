//! Box-tree builder (P7): turn a parsed + cascaded [`Dom`] into the frozen
//! `layout::LayoutNode` tree the layout engine (P6) consumes. This is the
//! production generalization of the `build_layout_tree` reference helper
//! kept local to `tests/layout_integration.rs` — same `display: none`-drop
//! semantics, plus the replaced-element (`img`) mapping that test helper
//! didn't need.
//!
//! Scope calls (see the P7 report / DECISIONS ledger):
//!   - Only `img` is treated as a replaced element in v0 (per the packet
//!     brief). Its intrinsic size comes from `width`/`height` attributes
//!     when present and parseable as a non-negative finite number; otherwise
//!     it defaults to 0x0 — a documented placeholder, not a guess at a "real"
//!     image size, since no image is decoded on this path (that's P9's fb
//!     backend).
//!   - The DOM walk is capped at [`DEPTH_CAP`] nesting levels, mirroring
//!     `layout::block`'s own (private, unexported) cap of the same value: a
//!     deeply-nested/hostile document (thousands of levels) would otherwise
//!     stack-overflow this recursive walk — a guard-page fault (`SIGABRT`)
//!     that `panic = "abort"` gives no mitigation for, exactly the bug class
//!     P6's `DEPTH_CAP` was introduced to fix (see JOURNAL 2026-08-13 / P6).
//!     Past the cap, a subtree is treated as an empty leaf: a childless
//!     `Container` box (matching `layout::block::translate_any`'s own
//!     over-depth fallback), so pathological nesting degrades gracefully
//!     instead of aborting the process.

use std::collections::HashMap;
use std::rc::Rc;

use crate::dom::{Dom, Element, Node, NodeId};
use crate::dom_util;
use crate::img::RgbaImage;
use crate::layout::inline::LINE_BREAK_SENTINEL;
use crate::layout::{BoxContent, Interactive, LayoutNode, Size};
use crate::style::computed::{
    BorderCollapse, BorderSide, BorderStyle, Display, Edges, Float, LengthPercentage, ListStyleType,
};
use crate::style::ComputedStyle;
use crate::surface::Color;

/// Mirrors `layout::block::DEPTH_CAP` (private to that module). This walk is
/// independent recursion — it never goes through `block`'s taffy translation
/// — so it needs its own bound against the same pathological-nesting attack.
const DEPTH_CAP: usize = 100;

/// The intrinsic size given to an `<img>` with no parseable `width`/`height`
/// attribute. Zero-by-zero rather than a guessed placeholder box: no image is
/// decoded on this path (P9 wires real pixel data + real intrinsic sizing),
/// so any nonzero default would be pure fiction reflected into layout.
const DEFAULT_IMG_INTRINSIC: Size = Size { w: 0.0, h: 0.0 };

/// Build the frozen `LayoutNode` box tree from a parsed + cascaded DOM.
/// `images` is the already fetch+decoded `NodeId -> RgbaImage` map (see
/// `crate::images::collect_images`) — an `<img>` whose `NodeId` has an entry
/// gets that image threaded into its `Replaced` box; one with no entry (not
/// looked up at all — e.g. the `--dump-text` path, which passes an empty map
/// to skip needless fetch/decode work — or looked up but not decoded, e.g. a
/// 404 or malformed image) still gets its `Replaced` box, just with `image:
/// None`, falling back to the intrinsic-size placeholder. Returns `None` if
/// the document is empty or its root is `display: none`.
///
/// Total: never panics on any `dom`/`styles` pairing produced by
/// `dom::parser::parse` + `style::cascade::cascade` (the styles slice is
/// always exactly `dom.len()` long from that pipeline; this function is
/// still defensive against a shorter slice via `styles.get`).
pub fn build_box_tree(
    dom: &Dom,
    styles: &[ComputedStyle],
    images: &HashMap<NodeId, Rc<RgbaImage>>,
) -> Option<LayoutNode> {
    if dom.is_empty() {
        return None;
    }
    build_node(dom, styles, images, dom.root(), 0, None)
}

/// `form_action` is the nearest enclosing `<form>`'s raw `action` attribute
/// (see [`Interactive::FormControl`]), threaded down the walk from wherever
/// a `<form>` element was last seen (`is_form`'s branch below) — `None`
/// outside any form. Borrowed straight out of the `Dom` (`el.attrs.get`
/// already has the right lifetime, tied to `dom`, not to any one stack
/// frame), so passing it through many recursive calls costs nothing extra.
fn build_node<'a>(
    dom: &'a Dom,
    styles: &[ComputedStyle],
    images: &HashMap<NodeId, Rc<RgbaImage>>,
    id: NodeId,
    depth: usize,
    form_action: Option<&'a str>,
) -> Option<LayoutNode> {
    let style = styles.get(id)?.clone();
    if style.display == Display::None {
        return None;
    }
    match dom.node(id) {
        Node::Text(text) => Some(LayoutNode {
            style,
            content: BoxContent::Text(text.clone()),
            children: Vec::new(),
            interactive: None,
        }),
        Node::Element(el) => {
            if is_replaced(el) {
                let mut style = style;
                apply_align_float_hint(el, &mut style);
                return Some(LayoutNode {
                    style,
                    content: BoxContent::Replaced { intrinsic: img_intrinsic(el), image: images.get(&id).cloned() },
                    children: Vec::new(),
                    interactive: None,
                });
            }
            if is_form_control(el) {
                return build_form_control(dom, el, style, form_action);
            }
            if is_form(el) {
                // Not itself interactive (only its controls are — see
                // `Interactive::FormControl`) — just updates the
                // `form_action` context for everything built underneath it.
                // A nested `<form>` (invalid HTML, but this dialect stays
                // total) simply overrides the context again for its own
                // subtree, closest-enclosing-form-wins.
                let action = el.attrs.get("action");
                let children = if depth >= DEPTH_CAP {
                    Vec::new()
                } else {
                    el.children
                        .iter()
                        .filter_map(|&child| build_node(dom, styles, images, child, depth + 1, action))
                        .collect()
                };
                return Some(LayoutNode { style, content: BoxContent::Container, children, interactive: None });
            }
            if is_link(el) {
                // `<a href>`: propagate `Interactive::Link` onto this box AND
                // every descendant box built underneath it (see
                // `tag_interactive`) — so wrapped link text that later splits
                // into several `Fragment`s (one per line) still carries the
                // SAME href on each one, and a link wrapping other content
                // (e.g. `<a href><img></a>`) tags that content too.
                let href = el.attrs.get("href").unwrap_or("").to_string();
                let children = if depth >= DEPTH_CAP {
                    Vec::new()
                } else {
                    el.children
                        .iter()
                        .filter_map(|&child| build_node(dom, styles, images, child, depth + 1, form_action))
                        .collect()
                };
                let mut node = LayoutNode { style, content: BoxContent::Container, children, interactive: None };
                tag_interactive(&mut node, &Interactive::Link { href: href.into_boxed_str() });
                return Some(node);
            }
            if is_br(el) {
                // M6 hardening (kitchen-sink coverage found `<br>` was a
                // total no-op — see JOURNAL/DECISIONS): synthesize a leaf
                // carrying `layout::inline::LINE_BREAK_SENTINEL` as its
                // `Text` payload, the same "leaf carrying a stand-in"
                // pattern `build_form_control`/the details marker use above
                // — `BoxContent` is frozen (no dedicated break variant), so
                // the forced-break signal has to ride inside the one payload
                // shape `Text` already offers. `layout::inline::tokenize`
                // recognizes the sentinel and turns it into a forced line
                // break instead of ordinary character data — see that
                // module's doc comments for the full rationale/totality
                // notes. `<br>` is a void element (`dom::parser`'s
                // `VOID_ELEMENTS`), so it never has real children to lose by
                // returning a childless leaf here.
                return Some(LayoutNode {
                    style,
                    content: BoxContent::Text(LINE_BREAK_SENTINEL.to_string()),
                    children: Vec::new(),
                    interactive: None,
                });
            }
            if is_details(el) {
                let node = if depth >= DEPTH_CAP {
                    // Same defensive fallback as the generic branch below:
                    // past the cap, degrade to an empty leaf rather than
                    // recursing into the disclosure logic at all.
                    LayoutNode { style, content: BoxContent::Container, children: Vec::new(), interactive: None }
                } else {
                    build_details_node(dom, styles, images, el, style, depth, form_action)
                };
                return Some(node);
            }
            if is_list_container(el) {
                let node = if depth >= DEPTH_CAP {
                    // Same defensive fallback as `is_details` above: past the
                    // cap, degrade to an empty leaf rather than recursing
                    // into the marker-synthesis logic at all.
                    LayoutNode { style, content: BoxContent::Container, children: Vec::new(), interactive: None }
                } else {
                    build_list_container_node(dom, styles, images, el, style, depth, form_action)
                };
                return Some(node);
            }
            let children = if depth >= DEPTH_CAP {
                Vec::new()
            } else {
                el.children
                    .iter()
                    .filter_map(|&child| build_node(dom, styles, images, child, depth + 1, form_action))
                    .collect()
            };
            let content = if style.display == Display::TableCell {
                let (colspan, rowspan) = cell_spans(el);
                BoxContent::TableCell { colspan, rowspan }
            } else {
                BoxContent::Container
            };
            let mut node = LayoutNode { style, content, children, interactive: None };
            if el.name.as_str() == "table" {
                apply_table_border_attribute(el, &mut node);
                apply_table_cellpadding_attribute(el, &mut node);
                // packet/border-collapse follow-up: only fires when
                // `apply_table_cellpadding_attribute` above did NOT already
                // stamp real cellpadding (see its own doc comment) -- runs
                // right after it for that reason.
                apply_table_border_default_padding(el, &mut node);
                // packet/collapse-geometry: `border-collapse: collapse` no
                // longer dedups (zeros) any cell border side here -- every
                // cell keeps its full cascaded/presentational-hint border,
                // and the actual "shared single line" effect is achieved by
                // `layout::block`'s collapse-aware cell GEOMETRY (overlapping
                // adjacent cells' rects by exactly one border-width) instead.
                // See `layout::block`'s "packet/collapse-geometry" doc
                // comments for the full replacement design and the DECISIONS
                // ledger for why the old dedup-to-an-L approach was wrong
                // (it broke both a bare `<table border>`'s frame -- doubled
                // to 2px -- and any CSS-collapsed table with no table-level
                // border -- lost its right/bottom outer edge entirely).
                //
                // packet/collapse-geometry (tty follow-up): stamp the
                // table's own resolved `border_collapse` onto every cell's
                // OWN `ComputedStyle.border_collapse` too. Real CSS
                // `border-collapse` is a table-level-only concept -- a
                // cell's own value is never otherwise read anywhere in this
                // codebase (`layout::block` only ever reads the TABLE
                // node's own `style.border_collapse`) -- so this is purely
                // an internal signal for `backend::tty::draw_table_grid_lines`,
                // which paints one fragment (and thus one `ComputedStyle`)
                // at a time with no access to its enclosing table: without
                // this, a cell fragment's own (uninherited, always-default-
                // Separate) `border_collapse` can't tell the tty backend
                // whether THIS cell's shared border is meant to coincide
                // with its neighbor's (collapse) or stay visually distinct
                // (separate, e.g. `cellspacing`) — see that function's own
                // "packet/collapse-geometry" doc comment.
                let collapse = node.style.border_collapse;
                stamp_cell_border_collapse(&mut node, collapse, 0);
            }
            Some(node)
        }
    }
}

/// Only `img` is a replaced element in v0 (brief scope: "keep to `img` for
/// now").
fn is_replaced(el: &Element) -> bool {
    el.name.as_str() == "img"
}

/// `<a href="...">` — an anchor with NO `href` attribute at all is not a
/// link in this dialect (matches real browsers: an href-less `<a>` is not
/// focusable/followable), so it takes the ordinary generic-element path
/// with no `Interactive` tagging. An href-less-but-PRESENT attribute (`<a
/// href="">` or `<a href>`) still counts — HTML only requires the attribute
/// be present, not non-empty (mirrors the "malformed/empty href: no panic"
/// totality requirement — an empty `href` is a valid, if unusual, link).
fn is_link(el: &Element) -> bool {
    el.name.as_str() == "a" && el.attrs.get("href").is_some()
}

/// `<form>` — not itself interactive (see `build_node`'s `is_form` branch);
/// exists only to update the `form_action` context threaded down to any
/// form control built underneath it.
fn is_form(el: &Element) -> bool {
    el.name.as_str() == "form"
}

/// Tag `node` AND every descendant in its subtree with `interactive` —
/// used by `<a href>` to mark the whole link's content (its own box plus
/// every recursively-built child box, so wrapped text split across several
/// `Fragment`s downstream all carries the same `Interactive::Link`, and any
/// non-text content nested under the link — e.g. `<a href><img></a>` — is
/// tagged too). Total: `node`'s subtree is already bounded by `DEPTH_CAP`
/// (this only walks an already-built, already-bounded tree), so this
/// recursion can't itself blow the stack on any input `build_node` could
/// have produced.
fn tag_interactive(node: &mut LayoutNode, interactive: &Interactive) {
    node.interactive = Some(interactive.clone());
    for child in &mut node.children {
        tag_interactive(child, interactive);
    }
}

/// `<br>` (M6 hardening) — see the `is_br` call site in `build_node` for the
/// forced-line-break synthesis this gates.
fn is_br(el: &Element) -> bool {
    el.name.as_str() == "br"
}

/// Parse an `<img>`'s intrinsic size off its `width`/`height` attributes.
/// Only non-negative, finite pixel counts are honored (HTML attribute
/// lengths are unitless integers in v0's dialect — no `%`/`px` suffix
/// handling); anything missing, non-numeric, negative, or non-finite falls
/// back to [`DEFAULT_IMG_INTRINSIC`] component-wise.
fn img_intrinsic(el: &Element) -> Size {
    let w = parse_nonneg(el.attrs.get("width")).unwrap_or(DEFAULT_IMG_INTRINSIC.w);
    let h = parse_nonneg(el.attrs.get("height")).unwrap_or(DEFAULT_IMG_INTRINSIC.h);
    Size { w, h }
}

/// Map the real 1996 `<img align=left|right>` presentational HTML attribute
/// onto the CSS `float` this box's `ComputedStyle` already carries (M4 part
/// 1, closing DECISIONS D14's floats deferral) — `align="left"` ->
/// `Float::Left`, `align="right"` -> `Float::Right`, case-insensitively
/// (`ALIGN="LEFT"` was as common as lowercase in real 1996 markup). Only
/// applied when the cascaded `style.float` is still `Float::None`: author
/// CSS `float` (from a `<style>`/author sheet, already resolved by cascade
/// before this function runs) always wins over the presentational hint,
/// matching the general HTML4 rule that CSS overrides presentational
/// attributes. `align="top"/"middle"/"bottom"` (vertical-align, not float)
/// and any other/missing value are left alone — out of scope per the packet
/// brief, and NOT a fallback to any float side.
fn apply_align_float_hint(el: &Element, style: &mut ComputedStyle) {
    if style.float != Float::None {
        return; // author CSS float wins over the attribute hint.
    }
    let Some(align) = el.attrs.get("align") else { return };
    match align.to_ascii_lowercase().as_str() {
        "left" => style.float = Float::Left,
        "right" => style.float = Float::Right,
        _ => {} // top/middle/bottom/unknown: vertical-align territory, ignored here.
    }
}

// ---------------------------------------------------------------------------
// `<table border="N">` presentational attribute (packet/table-border):
// vintage HTML's ruled-table hint. Like `align="left|right"` above, `border`
// is NOT an inherited CSS property, so it can't be handled in the cascade
// (P2) at all -- it's applied here, post-cascade, because this is the one
// place that still has the DOM's TABLE->CELL ancestor relationship in hand
// while a box tree is being built (`ComputedStyle`/`ElementInfo` carry no
// ancestor attributes for the cascade to consult). A `<table>` element's own
// box gets a solid `N`px border on all four sides; every descendant
// `<td>`/`<th>` cell gets a solid 1px border (the classic HTML rendering:
// the attribute's number sets the table's OUTER frame thickness, interior
// cell rules are always 1px, regardless of N) -- both in the same mid-gray
// (`#808080`) the UA sheet's `<hr>` rule already uses (packet/hr-rule),
// since there's no "classic table gray" this dialect tracks separately.
//
// Gating: a box only gets stamped if its CASCADED border is still the CSS
// default (`BorderStyle::None` on all four sides) -- exactly
// `apply_align_float_hint`'s "only if still default" contract for `float`.
// Since `border` isn't inherited, "still default" here correctly means "no
// author rule (inline style, `<style>`, or linked sheet) set a border on
// THIS box" -- author CSS always wins over a presentational attribute. The
// table and each cell are gated independently: an author rule on `td` but
// not on `table` still lets the table's own frame get stamped.
// ---------------------------------------------------------------------------

/// The color both the table's own frame and every cell's 1px rule are
/// stamped with -- matches the UA sheet's `<hr>` rule color (packet/hr-rule)
/// for a consistent "structural gray" across the dialect's presentational
/// rendering, since there's no real "classic table border gray" this v0
/// dialect tracks as its own constant.
const TABLE_BORDER_GRAY: Color = Color::rgb(0x80, 0x80, 0x80);

/// Every descendant `<td>`/`<th>` cell's rule width, per the classic HTML
/// rendering -- see the module doc section above.
const TABLE_CELL_BORDER_WIDTH: f32 = 1.0;

/// Parse `<table>`'s `border` attribute: a non-negative integer pixel count.
/// Absent, unparseable, or `0` all mean "no borders at all" (`None`);
/// anything `>= 1` is Some(that width). Unlike `img_intrinsic`'s
/// `width`/`height` (which accept any non-negative finite float), `border`
/// is parsed as a plain integer -- HTML4's own grammar for this attribute --
/// so `border="1.5"` is treated as unparseable (no borders), not rounded.
fn table_border_attribute(el: &Element) -> Option<f32> {
    let raw = el.attrs.get("border")?;
    let n: u32 = raw.trim().parse().ok()?;
    if n == 0 {
        None
    } else {
        Some(n as f32)
    }
}

/// `true` iff `border` carries no solid side at all -- the CSS initial
/// value, and therefore the signal "no author rule touched this box's
/// border" (border isn't inherited, so a non-default value here can only
/// have come from an explicit author declaration on this exact element).
fn border_is_cascade_default(border: &Edges<BorderSide>) -> bool {
    [border.top, border.right, border.bottom, border.left].iter().all(|side| side.style == BorderStyle::None)
}

fn solid_border_side(width: f32) -> BorderSide {
    BorderSide { width, style: BorderStyle::Solid, color: TABLE_BORDER_GRAY }
}

/// Stamp `<table border="N">`'s borders onto `table_box` (its own frame) and
/// every `TableCell` box in its already-built subtree, per the module doc
/// section above. `table_box` is the just-built `LayoutNode` for a `<table>`
/// element -- its `children` are already fully constructed (recursion is
/// bottom-up: `build_node` builds every child before the parent branch that
/// called it returns), so any nested `<table>` underneath has ALREADY had
/// this same function applied to itself, using its own `border` attribute.
/// This function's own subtree walk (`stamp_cell_borders`) stops the instant
/// it meets another `Display::Table` box, so it never re-stamps (or, worse,
/// un-stamps) a nested table's own cells with the outer table's `N` -- that
/// inner table's `border` attribute alone governs its own cells.
fn apply_table_border_attribute(el: &Element, table_box: &mut LayoutNode) {
    let Some(n) = table_border_attribute(el) else { return };
    if border_is_cascade_default(&table_box.style.border) {
        table_box.style.border = Edges::all(solid_border_side(n));
    }
    stamp_cell_borders(table_box, 0);
}

/// Walk `node`'s children (NOT `node` itself -- that's the table's own box,
/// already handled by the caller) stamping a 1px gray border onto every
/// still-default `TableCell` box, without descending into a nested
/// `<table>`'s subtree (its own `Display::Table` box governs its own cells
/// via its own `apply_table_border_attribute` call, already applied when
/// THAT table was built). `DEPTH_CAP`-bounded like every other recursive
/// walk in this module -- this only walks a subtree `build_node` already
/// built (and therefore already bounded), but the bound is kept anyway so
/// this function stays total on its own terms, not just by inheriting a
/// caller's guarantee.
fn stamp_cell_borders(node: &mut LayoutNode, depth: usize) {
    if depth >= DEPTH_CAP {
        return;
    }
    for child in &mut node.children {
        if child.style.display == Display::Table {
            // A nested table's own `border` attribute (or its absence)
            // governs its own cells -- do not descend into it.
            continue;
        }
        if matches!(child.content, BoxContent::TableCell { .. }) && border_is_cascade_default(&child.style.border) {
            child.style.border = Edges::all(solid_border_side(TABLE_CELL_BORDER_WIDTH));
        }
        stamp_cell_borders(child, depth + 1);
    }
}

/// Walk `node`'s children (NOT `node` itself) stamping `collapse` onto every
/// `TableCell` box's OWN `border_collapse` field, without descending into a
/// nested `<table>`'s subtree (its own `border-collapse`, or lack of one,
/// governs its own cells -- exactly `stamp_cell_borders`'s own "stop at a
/// nested Display::Table" rule). See the call site's own doc comment for why
/// this purely-internal stamp exists. `DEPTH_CAP`-bounded for the same
/// reason every other recursive walk in this module is.
fn stamp_cell_border_collapse(node: &mut LayoutNode, collapse: BorderCollapse, depth: usize) {
    if depth >= DEPTH_CAP {
        return;
    }
    for child in &mut node.children {
        if child.style.display == Display::Table {
            continue;
        }
        if matches!(child.content, BoxContent::TableCell { .. }) {
            child.style.border_collapse = collapse;
        }
        stamp_cell_border_collapse(child, collapse, depth + 1);
    }
}

// ---------------------------------------------------------------------------
// `<table cellpadding="N">` presentational attribute (packet/table-spacing):
// mirrors `<table border="N">`'s stamping pattern directly above almost
// exactly, with one difference -- `cellpadding` only ever touches CELLS
// (there's no analogous "table's own frame" concept for padding the way
// `border`'s outer-frame-vs-cell-rule distinction works), so there is no
// "apply to the table's own box" step here at all.
//
// Like `border`, `padding` is NOT an inherited CSS property, so it can't be
// resolved in the cascade (P2) -- it's stamped here, post-cascade, for the
// same reason: this is the one place that still has the DOM's TABLE->CELL
// ancestor relationship in hand while the box tree is being built.
//
// Gating: a cell only gets stamped if its CASCADED padding is still the CSS
// default (`LengthPercentage::Px(0.0)` on all four sides) -- exactly
// `apply_table_border_attribute`'s "still cascade-default" contract, so
// author CSS/inline `style="padding:...">` on the `<td>`/`<th>` itself
// always wins over the presentational hint.
// ---------------------------------------------------------------------------

/// Parse `<table>`'s `cellpadding` attribute: a non-negative integer pixel
/// count, same plain-integer HTML4 grammar as `border` (see
/// `table_border_attribute`'s own doc comment for why `"1.5"` is
/// unparseable rather than rounded). Absent or unparseable (including
/// negative) means "stamp nothing" (`None`); `0` is a valid explicit value,
/// though stamping `0px` padding is observably a no-op either way.
fn table_cellpadding_attribute(el: &Element) -> Option<f32> {
    let raw = el.attrs.get("cellpadding")?;
    let n: u32 = raw.trim().parse().ok()?;
    Some(n as f32)
}

/// `true` iff `padding` carries the CSS initial value on all four sides --
/// the signal "no author rule (inline style, `<style>`, or linked sheet)
/// touched this box's padding" (padding is NOT inherited, so a non-zero
/// value here can only have come from an explicit declaration on this exact
/// element -- there is no earlier stamp to worry about double-applying,
/// since each table's own `apply_table_cellpadding_attribute` call only
/// ever walks and stamps ONCE, at table-build time).
fn padding_is_cascade_default(padding: &Edges<LengthPercentage>) -> bool {
    let zero = LengthPercentage::Px(0.0);
    padding.top == zero && padding.right == zero && padding.bottom == zero && padding.left == zero
}

/// Stamp `<table cellpadding="N">`'s padding onto every `TableCell` box in
/// `table_box`'s already-built subtree (module doc section above).
/// `table_box` is the just-built `LayoutNode` for a `<table>` element -- its
/// `children` are already fully constructed (bottom-up recursion, see
/// `apply_table_border_attribute`'s own doc comment for the same point about
/// nested tables already having had this applied to themselves).
fn apply_table_cellpadding_attribute(el: &Element, table_box: &mut LayoutNode) {
    let Some(n) = table_cellpadding_attribute(el) else { return };
    stamp_cell_padding(table_box, n, 0);
}

/// Walk `node`'s children (NOT `node` itself) stamping `n`px padding on all
/// four sides of every still-cascade-default `TableCell` box, without
/// descending into a nested `<table>`'s subtree (its own `cellpadding`
/// attribute, or lack of one, governs its own cells -- exactly
/// `stamp_cell_borders`'s own "stop at a nested Display::Table" rule).
/// `DEPTH_CAP`-bounded for the same reason `stamp_cell_borders` is: total on
/// its own terms, not just by inheriting the caller's already-bounded
/// subtree.
fn stamp_cell_padding(node: &mut LayoutNode, n: f32, depth: usize) {
    if depth >= DEPTH_CAP {
        return;
    }
    for child in &mut node.children {
        if child.style.display == Display::Table {
            continue;
        }
        if matches!(child.content, BoxContent::TableCell { .. }) && padding_is_cascade_default(&child.style.padding) {
            child.style.padding = Edges::all(LengthPercentage::Px(n));
        }
        stamp_cell_padding(child, n, depth + 1);
    }
}

// ---------------------------------------------------------------------------
// `<table border>` default cell padding (packet/border-collapse follow-up):
// a bare `<table border="N">` with no `cellpadding` at all has ZERO cell
// padding by CSS default, which (combined with `border-collapse`'s own
// zeroed border-spacing) leaves NO free tty character-column between a
// cell's own border and its neighbor's text -- e.g. "Widget" immediately
// followed by "4" with no separator at all (`backend::tty::
// draw_table_grid_lines`'s box-drawing separator has nowhere to land; see
// that function's own doc comment for the full tty-resolution analysis).
// This stamps a small default padding, calibrated (see `DEFAULT_TABLE_
// BORDER_CELL_PADDING`'s own doc comment) to reliably reserve at least one
// free tty column, so the SAME box-drawing separator this packet already
// draws actually has room to show. Purely a rendering nicety for the
// text-mode grid -- pixel/fb rendering also gets slightly padded cells as a
// side effect, which reads as more legible there too (no downside).
//
// Mirrors `apply_table_cellpadding_attribute`'s own stamping pattern almost
// exactly, with two extra gates: only when `border` is present (packet
// brief -- author-CSS-only-bordered tables, e.g. kitchen-sink's `td {
// border: 1px solid }`, get NOTHING from this: no `border` ATTRIBUTE means
// this whole function is a no-op) AND only when `cellpadding` is NOT present
// AT ALL (not just "parses to zero" -- an explicit `cellpadding="0"`, even
// though it stamps literal `0px` padding either way, still counts as
// "author asked for padding" and must suppress this default, matching the
// packet brief's literal "no cellpadding attribute" wording). `stamp_cell_
// padding`'s own "still cascade-default" gate (padding_is_cascade_default)
// is reused unchanged, so real author CSS/inline `style="padding:...">`
// still wins over this default exactly like it wins over `cellpadding`
// itself.
//
// KNOWN LIMITATION shared with `cellpadding` itself: `padding_is_cascade_
// default` can't distinguish "the author explicitly wrote `padding: 0`" from
// "nothing touched padding at all" -- both resolve to the same `Px(0.0)` in
// `ComputedStyle`. So an author rule of EXACTLY `td { padding: 0 }` would
// still get overwritten by this default (and, pre-existing, by a real
// `cellpadding="N"` attribute too) -- this is not a new gap this packet
// introduces, just an existing one `apply_table_cellpadding_attribute`
// already has, inherited unchanged. A non-zero author padding (e.g. `td {
// padding: 2px }`) is unaffected and correctly wins (see this section's own
// test).
// ---------------------------------------------------------------------------

/// Default per-side cell padding stamped by `apply_table_border_default_
/// padding` (module doc section above). Calibrated empirically against
/// `fixtures/table-border.html` (`<table border="1">`, no cellpadding) run
/// through the real fetch->parse->cascade->box-tree->layout->tty pipeline:
/// with `0px` padding, adjacent cells' solved column widths exactly hug
/// their own text (zero slack), so a cell's own left border and the
/// following cell's first text character round to the SAME 8px tty column
/// (`backend::tty::CELL_W`) -- the border character is then invisible
/// (real text, painted after, always wins the write). `4px` reliably pushes
/// the FOLLOWING cell's own text start at least one whole tty column past
/// its border/corner column in every row of that fixture (verified via a
/// one-off fragment-rect dump: 1px border + 4px padding = 5px inset, enough
/// to cross an 8px column's round-to-nearest boundary regardless of exactly
/// where the border's own sub-pixel position falls within its column) --
/// confirmed against the ACTUAL rendered `--headless --dump-text` output:
/// `Widget`/`4` (and `Gadget`/`2`) now render with a real box-drawing
/// character between them, no more cells running together. Deliberately
/// smaller than `table-spacing`'s own convention-only `cellpadding="6"`
/// (real HTML content, not calibrated for this purpose) -- 4px is the
/// minimum this packet's own testing found sufficient, keeping the visual
/// padding as unobtrusive as possible while still fixing the separator.
const DEFAULT_TABLE_BORDER_CELL_PADDING: f32 = 4.0;

/// Stamp `DEFAULT_TABLE_BORDER_CELL_PADDING` onto every still-cascade-
/// default `TableCell` box in `table_box`'s subtree, but ONLY when `el` (the
/// `<table>` element) has a `border` ATTRIBUTE present AND no `cellpadding`
/// attribute at all -- see the module doc section above for the full
/// rationale and each gate's own reasoning. `table_box` is the just-built
/// `LayoutNode`, already having had `apply_table_border_attribute`/`apply_
/// table_cellpadding_attribute` applied to it (see the call site in
/// `build_node`), so this sees their result -- in particular, if `cell
/// padding` WAS present, `apply_table_cellpadding_attribute` already
/// stamped every still-default cell with that real value, and this function
/// bails out immediately without ever touching that (or any) cell.
fn apply_table_border_default_padding(el: &Element, table_box: &mut LayoutNode) {
    if el.attrs.get("border").is_none() {
        return; // no `border` attribute: not this packet's scope at all
    }
    if el.attrs.get("cellpadding").is_some() {
        // Author explicitly named a `cellpadding` (even `"0"`, or an
        // unparseable value) -- that request (or its total absence of
        // effect, for `"0"`/garbage) always wins; this default never
        // second-guesses it.
        return;
    }
    stamp_cell_padding(table_box, DEFAULT_TABLE_BORDER_CELL_PADDING, 0);
}

// ---------------------------------------------------------------------------
// `border-collapse: collapse` (packet/collapse-geometry): NOT handled here
// at all anymore. The earlier "top+left only + table outer border" dedup
// model (`apply_border_collapse`/`dedup_cell_borders`, removed by this
// packet) zeroed every cell's right/bottom border, relying on the table's
// own frame box to close off the grid's right/bottom edges. That was
// architecturally wrong in two ways the pixel goldens exposed: (1) a bare
// `<table border>` (which HAS its own frame border) then drew that frame
// AND the first row/column cells' top/left borders on top of each other --
// a doubled 2px edge instead of 1px; (2) a CSS-only-collapsed table with NO
// table-level border (e.g. kitchen-sink's `table { border-collapse:
// collapse } td { border: 1px solid }`) lost its right/bottom outer edge
// entirely, since zeroing was the only thing that ever drew those edges.
//
// The replacement keeps every cell's FULL 4-side border untouched (real CSS
// `border-collapse: collapse` cells still have a border on every side; it's
// the LAYOUT that makes adjacent borders land on the same pixels, not the
// style) and fixes the geometry instead, in `layout::block` (see that
// module's own "packet/collapse-geometry" doc comments): cells are
// positioned so each interior grid line is shared -- adjacent cells'
// abutting edges are pulled together by exactly one border-width, so their
// independently-painted borders coincide into a single line -- and, when
// the table itself also carries a border (the `<table border>` case), the
// whole cell grid is shifted to overlap the table's own frame the same way,
// so the frame and the edge cells' own borders coincide instead of
// stacking. Both fixes only need a uniform border width (the only case this
// packet's brief asks for; genuinely conflicting per-cell border widths/
// styles would need real CSS border-conflict resolution -- out of scope,
// documented follow-up, same as before).
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// <details>/<summary> disclosure (M5 dialect-completeness, part 1).
//
// Real browsers give `<details>` interactive click-to-toggle behavior; there
// is no interactive shell in Stele yet (out of scope per the packet brief),
// so this is the STATIC render of whichever state the `open` attribute (a
// plain HTML boolean attribute -- present with any/no value means open,
// absent means closed) already names:
//   - WITHOUT `open`: collapsed. Only the first direct-child `<summary>` is
//     built into the box tree (the clickable label); every other child is
//     dropped entirely -- not laid out, not present in the tree at all,
//     mirroring `display: none`'s own "absent entirely" contract so nothing
//     hidden can leak into a tty dump.
//   - WITH `open` (any/no value): expanded. The summary AND every other
//     child are built normally, in original document order.
//   - No `<summary>` DIRECT child at all: real browsers show a default,
//     localized "Details" label. This dialect has one locale, so the
//     default is always the literal string "Details" (documented choice --
//     the alternative, rendering nothing at all for the label, would make a
//     collapsed no-summary `<details>` silently vanish from the page, which
//     is worse than a slightly-wrong-sounding default).
//   - Multiple `<summary>` children: only the FIRST becomes the disclosure
//     label (matches every real browser). Any later `<summary>` is just
//     ordinary content -- shown (unmarked) when open, dropped when
//     collapsed, exactly like any other non-summary child.
//   - A `<summary>` nested inside some OTHER element (`<details><div>
//     <summary>...` ) is not recognized as the direct-child label at all
//     (matches the HTML5 "first summary element child" rule) -- falls back
//     to the default "Details" label, with the wrapper + buried summary
//     themselves treated as ordinary content.
//
// Disclosure marker (documented convention, since there is no click/icon
// affordance in a text-mode dump to show open/closed otherwise): the
// rendered summary is prefixed with `"> "` when collapsed or `"v "` when
// open -- ASCII-only (matching the bitmap font's own ASCII-only glyph set),
// deterministic, and legible as a crude "twisty" in a tty golden (`v` reads
// as an open-triangle stand-in, `>` as a closed one). The marker is
// synthesized as its own leading `Text` child glued onto the summary box's
// existing children (the same "synthesize a leaf carrying a stand-in"
// pattern `build_form_control` already uses for form placeholders above),
// not spliced into the summary's own text content -- keeps the marker
// independent of whatever markup a `<summary>` happens to contain.
// ---------------------------------------------------------------------------

/// Closed-state marker -- see the module doc section above.
const SUMMARY_CLOSED_MARKER: &str = "> ";
/// Open-state marker -- see the module doc section above.
const SUMMARY_OPEN_MARKER: &str = "v ";
/// The default disclosure label shown when a `<details>` has no direct-child
/// `<summary>` at all -- matches real browsers' own default ("Details" in
/// English; this dialect has one locale).
const DEFAULT_SUMMARY_LABEL: &str = "Details";

fn is_details(el: &Element) -> bool {
    el.name.as_str() == "details"
}

/// The first `<summary>` DIRECT child of a `<details>` element's own
/// `children`, if any -- see the module doc section above for why only a
/// direct child counts.
fn find_first_summary(dom: &Dom, el: &Element) -> Option<NodeId> {
    el.children
        .iter()
        .copied()
        .find(|&c| matches!(dom.node(c), Node::Element(e) if e.name.as_str() == "summary"))
}

/// Build a `<details>` element's box per the disclosure rules documented
/// above. `depth` is guaranteed `< DEPTH_CAP` by the caller (`build_node`
/// handles the past-cap fallback itself, exactly like the generic element
/// branch does), so every recursive `build_node` call here is safe without
/// its own extra depth check.
fn build_details_node<'a>(
    dom: &'a Dom,
    styles: &[ComputedStyle],
    images: &HashMap<NodeId, Rc<RgbaImage>>,
    el: &'a Element,
    style: ComputedStyle,
    depth: usize,
    form_action: Option<&'a str>,
) -> LayoutNode {
    let is_open = el.attrs.get("open").is_some();
    let marker = if is_open { SUMMARY_OPEN_MARKER } else { SUMMARY_CLOSED_MARKER };
    let summary_id = find_first_summary(dom, el);

    let summary_box = summary_id
        .and_then(|sid| build_node(dom, styles, images, sid, depth + 1, form_action))
        .map(|mut node| {
            node.children.insert(0, marker_node(marker, &node.style));
            node
        })
        .unwrap_or_else(|| default_summary_node(marker, &style));

    let mut children = vec![summary_box];
    if is_open {
        for &child in &el.children {
            if Some(child) == summary_id {
                continue; // already placed above, markered.
            }
            if let Some(node) = build_node(dom, styles, images, child, depth + 1, form_action) {
                children.push(node);
            }
        }
    }

    LayoutNode { style, content: BoxContent::Container, children, interactive: None }
}

/// The synthesized marker box glued in front of a real `<summary>`'s own
/// (recursively built) children -- see the module doc section above.
fn marker_node(marker: &str, style: &ComputedStyle) -> LayoutNode {
    LayoutNode { style: style.clone(), content: BoxContent::Text(marker.to_string()), children: Vec::new(), interactive: None }
}

/// The synthesized `"> Details"`/`"v Details"` label shown in place of a
/// missing `<summary>` -- see the module doc section above.
fn default_summary_node(marker: &str, style: &ComputedStyle) -> LayoutNode {
    LayoutNode {
        style: style.clone(),
        content: BoxContent::Container,
        children: vec![LayoutNode {
            style: style.clone(),
            content: BoxContent::Text(format!("{marker}{DEFAULT_SUMMARY_LABEL}")),
            children: Vec::new(),
            interactive: None,
        }],
        interactive: None,
    }
}

fn parse_nonneg(raw: Option<&str>) -> Option<f32> {
    let v: f32 = raw?.trim().parse().ok()?;
    if v.is_finite() && v >= 0.0 {
        Some(v)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// List markers (M6): `<ul>/<ol>/<li>` render with NO bullets/numbers at all
// otherwise -- the kitchen-sink render exposed this gap. `layout::block`'s
// curated `Display` set has no `list-item` value (only block/inline/flex/
// table*, brief §4), so a marker can't be a real CSS `::marker` pseudo-box;
// instead it's synthesized as its own leading `Text` child glued onto an
// `<li>`'s (recursively built) children, the exact same "synthesize a leaf
// carrying a stand-in" convention `build_details_node`'s disclosure marker
// and `build_form_control`'s placeholder labels already use above.
//
// Only a `<ul>`/`<ol>`'s DIRECT `<li>` children are recognized as list
// items (matches the details/summary "direct child" convention above, and
// real HTML's own list content model) -- an `<li>` buried inside some other
// wrapper element, or with no `<ul>`/`<ol>` ancestor at all, is built by the
// ordinary generic-element path with no marker synthesis at all (documented
// choice: no marker, not a panic, not some invented default numbering with
// no list to count against).
//
// packet/display-list-item: being the `<li>` TAG is necessary but no longer
// SUFFICIENT -- `build_list_container_node` also requires the built node's
// own `ComputedStyle.display == Display::ListItem` before treating it as a
// list item. Real CSS only puts a marker on a `display: list-item` box, and
// this engine now HAS that value (`style/computed.rs`'s `Display::
// ListItem`; `style/ua.rs`'s `li { display: list-item; }` is the UA default
// for `<li>`). An `<li>` whose author CSS overrides `display` to something
// else (`block`, `inline`, `flex`, a table-ish value, ...) is no longer
// list-item-shaped and gets neither a marker nor a consumed ordinal, while
// its own content still renders normally -- this now correctly covers the
// W3C CSS1 float test's `li{display:block; /* i.e., suppress marker */
// ...}` (`fixtures/css1-float-5526c.html`), which packet #58's `tag_is_li &&
// display == Display::Block` stopgap could not: that guard could not tell
// "author wrote block on purpose" apart from the UA default, because both
// used to resolve to the identical `Display::Block` and `ComputedStyle`
// carried no provenance to distinguish them. See `fixtures/evidence/
// css1-float-5526c.diagnosis.md` for the prior packet's own account of the
// gap this closes.
//
// Ordinal counting is per-list: each call to `build_list_container_node`
// (one per `<ul>`/`<ol>` box built) keeps its own local counter, so a
// `<ul>`/`<ol>` nested inside an `<li>` naturally restarts at 1 -- there is
// no global/shared counter across sibling or ancestor lists. Only `<li>`
// children that actually produce a box AND are still list-item-shaped (the
// `Display::Block` check above) count towards the ordinal (a `display:
// none` `<li>` -- dropped like any other display:none subtree -- does NOT
// consume a number, so the visible sequence in a tty dump never has a gap);
// a non-`<li>` child of a `<ul>`/`<ol>` (invalid HTML, but this dialect
// stays total rather than rejecting it) is still built as ordinary content,
// just without a marker and without advancing the counter.
//
// Marker glyph choice, per `ComputedStyle::list_style_type` (the value is
// already inherited by cascade -- `cascade.rs`'s `inherited!(list_style_type)`
// -- so a nested list's own UA-sheet `list-style-type` correctly overrides
// what it inherited from an ancestor list; this function only owns WHICH
// item within the nearest enclosing list, not which style applies):
//   - `None`                    -> no marker at all.
//   - `Disc`/`Circle`/`Square`  -> `"* "`/`"o "`/`"# "` respectively. Real
//     CSS's disc/circle/square are `•`/`◦`/`▪` (Unicode), but this dialect's
//     bitmap font is ASCII-only (brief §4/`fb` backend) -- `•` would render
//     as a tofu box in the PNG golden. ASCII stand-ins are used instead so
//     the PNG golden shows a real, distinct glyph per type, not three
//     identical tofu boxes. (The tty text dump always shows the literal
//     character regardless of font coverage; this choice only matters for
//     the PNG path.)
//   - `Decimal`                 -> `"{ordinal}. "` (`"1. "`, `"2. "`, ...).
//   - `LowerAlpha`/`UpperAlpha` -> `"{a..z, aa..az, ...}. "`, bijective
//     base-26 (`alpha_ordinal`) -- CSS's own `lower-alpha`/`upper-alpha`
//     algorithm. Bounded/cheap even for a huge list (`log26(n)` digits).
//     `lower-roman`/`upper-roman` are NOT in the curated `ListStyleType`
//     enum at all (frozen type, brief §4) -- there is nothing to implement;
//     documented here rather than silently absent.
//
// `<ol start="N">` (optional per the packet brief, cheap to support: it's
// one attribute read) seeds the per-list counter at `N` instead of `1`;
// missing/unparseable `start` defaults to `1`, exactly like no attribute at
// all. A non-positive `start` is honored verbatim for `Decimal` (CSS allows
// negative starts; `i64::to_string()` handles it directly) but clamped up to
// `1` before alpha conversion (`alpha_ordinal`'s doc comment) since there is
// no meaningful "zeroth"/negative letter.
// ---------------------------------------------------------------------------

/// ASCII stand-in glyphs for the three CSS bullet styles -- see the module
/// doc section above for why ASCII rather than the real Unicode bullets.
const BULLET_DISC: &str = "* ";
const BULLET_CIRCLE: &str = "o ";
const BULLET_SQUARE: &str = "# ";

fn is_list_container(el: &Element) -> bool {
    matches!(el.name.as_str(), "ul" | "ol")
}

fn is_li(el: &Element) -> bool {
    el.name.as_str() == "li"
}

/// `<ol start="N">` (also harmlessly honored on `<ul>`, which has no real
/// `start` semantics -- reading a nonexistent attribute is a no-op) -- see
/// the module doc section above.
fn list_start(el: &Element) -> i64 {
    el.attrs.get("start").and_then(|s| s.trim().parse::<i64>().ok()).unwrap_or(1)
}

/// Build a `<ul>`/`<ol>` element's box: recurse into every child normally,
/// but glue a synthesized marker onto each DIRECT `<li>` child per the
/// module doc section above. `depth` is guaranteed `< DEPTH_CAP` by the
/// caller (`build_node` handles the past-cap fallback itself, exactly like
/// `build_details_node`'s caller does), so every recursive `build_node` call
/// here is safe without its own extra depth check.
fn build_list_container_node<'a>(
    dom: &'a Dom,
    styles: &[ComputedStyle],
    images: &HashMap<NodeId, Rc<RgbaImage>>,
    el: &'a Element,
    style: ComputedStyle,
    depth: usize,
    form_action: Option<&'a str>,
) -> LayoutNode {
    let mut ordinal = list_start(el);
    let mut children = Vec::with_capacity(el.children.len());
    for &child in &el.children {
        let tag_is_li = matches!(dom.node(child), Node::Element(e) if is_li(e));
        let Some(mut node) = build_node(dom, styles, images, child, depth + 1, form_action) else {
            continue; // display:none (or any other total-absence case): no box, no number consumed.
        };
        // A marker (and the ordinal it would consume) is only for a box
        // that's still acting as a list item -- real CSS restricts markers
        // to `display: list-item`, and this engine's `ComputedStyle` now
        // carries that exact value (`Display::ListItem`, packet/
        // display-list-item), the UA sheet's own default for `<li>`
        // (`style/ua.rs`'s `li { display: list-item; }`). An `<li>` whose
        // author CSS overrides `display` to anything else (`block`,
        // `inline`, `flex`, a table-ish value, ...) is no longer
        // list-item-shaped, so it gets neither a marker nor a consumed
        // ordinal -- it still renders as ordinary content via the
        // `children.push(node)` below, just without either. This now
        // correctly distinguishes "author CSS explicitly re-asserts
        // `display: block`" (e.g. the W3C CSS1 float test's `li{display:
        // block; /* i.e., suppress marker */ ...}`, `fixtures/
        // css1-float-5526c.html`) from an ordinary `<li>` with no CSS at
        // all -- the two resolve to different `Display` values now (`Block`
        // vs. `ListItem`), where packet #58's `tag_is_li && display ==
        // Display::Block` stopgap could not tell them apart (see
        // `fixtures/evidence/css1-float-5526c.diagnosis.md` for that
        // prior gap).
        let is_item = tag_is_li && node.style.display == Display::ListItem;
        if is_item {
            if let Some(marker) = marker_text(node.style.list_style_type, ordinal) {
                node.children.insert(0, marker_node(&marker, &node.style));
            }
            ordinal += 1;
        }
        children.push(node);
    }
    LayoutNode { style, content: BoxContent::Container, children, interactive: None }
}

/// The marker text for one list item, or `None` when `list_style_type` is
/// `ListStyleType::None` (no marker at all) -- see the module doc section
/// above for the full glyph/format convention per variant.
fn marker_text(list_style_type: ListStyleType, ordinal: i64) -> Option<String> {
    Some(match list_style_type {
        ListStyleType::None => return None,
        ListStyleType::Disc => BULLET_DISC.to_string(),
        ListStyleType::Circle => BULLET_CIRCLE.to_string(),
        ListStyleType::Square => BULLET_SQUARE.to_string(),
        ListStyleType::Decimal => format!("{ordinal}. "),
        ListStyleType::LowerAlpha => format!("{}. ", alpha_ordinal(ordinal, false)),
        ListStyleType::UpperAlpha => format!("{}. ", alpha_ordinal(ordinal, true)),
    })
}

/// CSS's `lower-alpha`/`upper-alpha` counter style: bijective base-26 (`1` ->
/// `"a"`, ..., `26` -> `"z"`, `27` -> `"aa"`, `28` -> `"ab"`, ...) -- NOT
/// ordinary base-26, which would have no representation for `26` without a
/// "zero" digit. Non-positive `n` (only reachable via a negative/zero `<ol
/// start>`, since the per-list counter otherwise only ever counts up from
/// `1`) is clamped to `1` first -- see the module doc section above.
fn alpha_ordinal(n: i64, upper: bool) -> String {
    let mut n = if n < 1 { 1 } else { n };
    let mut letters = Vec::new();
    while n > 0 {
        let rem = ((n - 1) % 26) as u8;
        letters.push(if upper { b'A' + rem } else { b'a' + rem } as char);
        n = (n - 1) / 26;
    }
    letters.iter().rev().collect()
}

// ---------------------------------------------------------------------------
// Form-control rendering (P-forms, part 2).
//
// Real form widgets (a text field you can type into, a real dropdown) are
// the fb backend's job (M4) -- there is no pixel surface here, only a box
// tree that eventually becomes tty text. `backend::tty` paints NOTHING for
// a plain `Box` fragment (DECISIONS D17), so a control left as an empty
// generated box would be invisible in the golden. Instead, each control
// synthesizes a small `Container` holding exactly one synthesized
// `BoxContent::Text` placeholder label (mirroring the `img` -> `Replaced`
// mapping above: a DOM element becomes a leaf-ish box carrying a stand-in,
// not its literal DOM children) -- the label flows through the ordinary
// inline/text pipeline and shows up as real characters in the tty dump.
//
// Placeholder convention (documented for the DECISIONS ledger -- see the
// packet report):
//   - text-like `<input>` (`text`/`password`/`search`/`email`/`url`/`tel`/
//     `number`, or no `type` at all): `[<value>]`, tight brackets; with no
//     `value`, `[<underscores sized to the `size` attr, default 10>]` --
//     UNDERSCORES, not literal spaces: `layout::inline`'s bespoke
//     whitespace-collapsing is unconditional in v1 ("v1 always collapses; a
//     Pre fast-path is a follow-up", per that module's own docs) -- it
//     collapses ANY run of whitespace to a single space regardless of a
//     node's `white-space` style, so a run of literal spaces here would
//     silently collapse down to one space by the time it reaches the tty
//     grid, defeating the whole point of a size-sized placeholder. `_` is
//     never whitespace, so it survives that collapsing untouched, and reads
//     just as clearly as a blank field (`[____________]`) in a text-mode
//     dump -- arguably more so, since bare spaces between `[`/`]` are
//     visually indistinguishable from the surrounding background anyway.
//     `password` masks its value with one `*` per character instead of the
//     literal text (no `value` still falls back to the same underscore
//     blank -- nothing to mask).
//   - `<input type=checkbox>`: `[x]` checked, `[ ]` unchecked.
//   - `<input type=radio>`: `(*)` checked, `( )` unchecked.
//   - `<input type=submit|image>`, `<input type=reset>`, `<input
//     type=button>`, and `<button>` (any/no `type`): `[ <label> ]` --
//     spaced brackets, distinct from the tight text-field brackets so a
//     document with both a text field and a submit button doesn't read as
//     visually identical. Label resolution (T4 button-honesty amendment --
//     see `resolve_bracket_label`'s own doc comment for the full rationale;
//     this replaced an earlier "value, else child text, else always-
//     'Submit'" rule that invented a word for controls the author never
//     labeled, e.g. an icon-only `<button type=button aria-label="Theme">`):
//     first non-empty of (1) the element's own text content, (2) its
//     `value` attribute, (3) its `aria-label` attribute, (4) its `title`
//     attribute, (5) the literal "Submit" -- ONLY for a genuine submit
//     control (`type=submit|image`, or a `<button>` whose effective type is
//     `submit`) sitting inside an enclosing `<form>`, matching real
//     browsers' own UA default for that one case -- else (6) empty (`[ ]`),
//     never an invented word.
//   - `<input type=hidden>`: no box, no text at all -- hidden really means
//     invisible, even in a text-mode dump.
//   - `<textarea>`: its own text content verbatim if short and single-line;
//     otherwise the first line, hard-truncated to
//     [`MAX_TEXTAREA_CHARS`] characters, with a trailing `[...]` marker
//     (also appended, untruncated, when the first line is short but more
//     lines follow) -- "first line + [...] if long", per the packet brief.
//   - `<select>`: `[ <selected option's text> v]` -- the `v` is a crude
//     ASCII stand-in for a dropdown affordance. The selected option is the
//     first descendant `<option selected>`; with none marked, the first
//     `<option>` at all (matching every real browser's fallback, not
//     spec-mandated but universal practice); with no options whatsoever,
//     the text is simply empty (`[  v]`) -- total, not a special case.
//
// `display` for all of these (UUA sheet, `style/ua.rs`): `inline`. This is
// CSS's own initial value already (`ComputedStyle::default().display ==
// Display::Inline`), so the explicit UA rule is redundant with that
// default -- it's added anyway, spelled out, so the choice is a documented
// decision rather than an accident of what the initial value happens to
// be. Inline lets a control flow naturally after its `<label>` text on the
// same line (`Name: [__________]`), which is legible in a text-mode dump;
// `block` would force every control onto its own line, wasting vertical
// space a 25x80 terminal doesn't have to spare. Real widget geometry
// (fixed pixel width matching `size`, a real dropdown affordance) is the
// fb backend's job (M4) -- this is a text placeholder, not a form widget.
// ---------------------------------------------------------------------------

/// Default `<input>` "visible width" (the `size` attribute) when absent --
/// matches the packet brief's "default ~10".
const DEFAULT_CONTROL_SIZE: usize = 10;
/// Upper bound on a (possibly hostile) `size` attribute, so a document
/// author (or attacker) can't make this synthesize an arbitrarily long
/// placeholder string. Far past any real form field.
const MAX_CONTROL_SIZE: usize = 100;
/// How many characters of a `<textarea>`'s first line are shown before
/// truncating with `[...]` -- generous enough to be legible, short enough
/// to keep a form fixture's golden readable in an 80-column dump.
const MAX_TEXTAREA_CHARS: usize = 20;

fn is_form_control(el: &Element) -> bool {
    matches!(el.name.as_str(), "input" | "button" | "textarea" | "select")
}

/// Build a form control's box: a childless-in-DOM `Container` holding
/// exactly one synthesized `Text` child (the placeholder label). Returns
/// `None` for `<input type=hidden>` (and, defensively, any future control
/// kind added to [`is_form_control`] without a matching arm below) -- no
/// box at all, matching `display: none`'s own "absent entirely" contract
/// so a hidden field can never leak its value into the tty dump.
///
/// Both the outer wrapper AND the inner label `Text` child are tagged with
/// the same `Interactive::FormControl` (P7 interactive-provenance freeze
/// amendment): the wrapper is `display: inline` (UA sheet, see the module
/// doc section above), so a parent walking its children almost always
/// folds it straight into an inline-formatting-context leaf via
/// `layout::block::flatten_inline` -- which reads the label child's own
/// `interactive`, never the wrapper's -- but the wrapper is tagged too in
/// case it ever becomes its own taffy node (e.g. a `display: flex` parent,
/// which gives every child its own node instead of folding it), so its own
/// `Box` fragment carries the marker as well either way.
fn build_form_control(dom: &Dom, el: &Element, style: ComputedStyle, form_action: Option<&str>) -> Option<LayoutNode> {
    let label = control_label(dom, el, form_action)?;
    let interactive = Interactive::FormControl {
        kind: control_kind(el).into_boxed_str(),
        name: el.attrs.get("name").map(|s| s.to_string().into_boxed_str()),
        form_action: form_action.map(|s| s.to_string().into_boxed_str()),
    };
    Some(LayoutNode {
        content: BoxContent::Container,
        children: vec![LayoutNode {
            style: style.clone(),
            content: BoxContent::Text(label),
            children: Vec::new(),
            interactive: Some(interactive.clone()),
        }],
        style,
        interactive: Some(interactive),
    })
}

/// The control's effective type -- what a later submit/focus shell needs to
/// tell a text field from a checkbox from a submit button, per
/// [`Interactive::FormControl::kind`]'s own doc comment. `<input>`'s
/// default (no `type` attribute) is `"text"`, matching HTML's own default;
/// `<button>`'s default is `"submit"`, likewise matching HTML's own default
/// for a `<button>` with no `type` attribute (the same default
/// `button_label`'s sibling logic doesn't need to re-derive, since the
/// TYPE and the LABEL are independent facts about a `<button>`).
fn control_kind(el: &Element) -> String {
    match el.name.as_str() {
        "input" => el.attrs.get("type").map(|s| s.to_ascii_lowercase()).unwrap_or_else(|| "text".to_string()),
        "button" => el.attrs.get("type").map(|s| s.to_ascii_lowercase()).unwrap_or_else(|| "submit".to_string()),
        // "textarea"/"select", and defensively any future control kind
        // added to `is_form_control` without a matching arm here -- the
        // element's own tag name is already a perfectly good `kind`.
        other => other.to_string(),
    }
}

fn control_label(dom: &Dom, el: &Element, form_action: Option<&str>) -> Option<String> {
    match el.name.as_str() {
        "input" => input_label(el, form_action),
        "button" => Some(bracket_spaced(&button_label(dom, el, form_action))),
        "textarea" => Some(textarea_label(dom, el)),
        "select" => Some(select_label(dom, el)),
        _ => None,
    }
}

/// `<input>` is a void element -- it can never carry child text -- so this
/// takes no `dom` reference at all (unlike `button_label`, which needs the
/// tree to walk a `<button>`'s children). Rung (1) of `resolve_bracket_label`
/// (`own_text`) is passed as `""` for every `<input>` variant below,
/// meaning rungs (2)-(6) (`value`, `aria-label`, `title`, the submit
/// default, empty) are the only ones ever in play here.
fn input_label(el: &Element, form_action: Option<&str>) -> Option<String> {
    let ty = el.attrs.get("type").unwrap_or("text").to_ascii_lowercase();
    Some(match ty.as_str() {
        "hidden" => return None,
        "checkbox" => {
            if dom_util::is_checked(el) {
                "[x]".to_string()
            } else {
                "[ ]".to_string()
            }
        }
        "radio" => {
            if dom_util::is_checked(el) {
                "(*)".to_string()
            } else {
                "( )".to_string()
            }
        }
        // `submit`/`image` are the only `<input>` types that count as a
        // genuine submit control for `resolve_bracket_label`'s rung (5)
        // ("Submit") default -- an `<input type=image>` submits the form
        // exactly like a `type=submit` one, just with a graphical face.
        "submit" | "image" => bracket_spaced(&resolve_bracket_label(el, "", form_action.is_some())),
        "reset" => bracket_spaced(&resolve_bracket_label(el, "", false)),
        "button" => bracket_spaced(&resolve_bracket_label(el, "", false)),
        "password" => bracket_tight(&password_mask(el)),
        // text/search/email/url/tel/number, and any unrecognized/future
        // type, all render as a plain text field.
        _ => bracket_tight(&text_field_value(el)),
    })
}

fn bracket_tight(s: &str) -> String {
    format!("[{s}]")
}

/// `[ <label> ]`, spaced brackets -- see the module's "Placeholder
/// convention" doc section. An empty label collapses to the bare `[ ]`
/// (single space, matching the unchecked-checkbox convention) rather than
/// `[  ]` (two spaces around nothing) -- this is `resolve_bracket_label`'s
/// rung (6) landing here, an honest empty control, never invented text.
fn bracket_spaced(s: &str) -> String {
    if s.is_empty() {
        "[ ]".to_string()
    } else {
        format!("[ {s} ]")
    }
}

/// Author-honest label resolution for every bracket-spaced control
/// (`<button>`, `<input type=submit|image|reset|button>`) -- the T4
/// button-honesty amendment. Stele must never invent a word the author
/// didn't write (D4: httpforever's icon-only `<button type=button
/// aria-label="Theme">` was rendering as `[ Submit ]`, a fabricated label
/// for a control that isn't even a submit button). Rungs, in order --
/// first non-empty wins:
///   1. `own_text` -- the element's own text content (empty for a void
///      `<input>`, which can never have children; only `<button>` ever
///      supplies a non-empty value here),
///   2. the `value` attribute,
///   3. the `aria-label` attribute -- author-provided, exactly what an
///      icon-only button carries in the wild,
///   4. the `title` attribute,
///   5. ONLY when `is_submit_default` is true -- the literal "Submit",
///      matching every real browser's own UA default for an actual submit
///      control (`type=submit|image`, or a `<button>` whose effective type
///      is `submit`) with no author-supplied label. Callers gate this on
///      the control also sitting inside an enclosing `<form>`
///      (`form_action.is_some()`) -- a submit-typed control with no form to
///      submit is, semantically, no more a "Submit" button than a bare
///      `type=button` one, so inventing that word for it would be exactly
///      the D4 bug in a different costume,
///   6. otherwise empty -- `bracket_spaced`'s own empty-string branch turns
///      this into the bare `[ ]`, never a fabricated word.
fn resolve_bracket_label(el: &Element, own_text: &str, is_submit_default: bool) -> String {
    let trimmed_text = own_text.trim();
    if !trimmed_text.is_empty() {
        return trimmed_text.to_string();
    }
    for attr in ["value", "aria-label", "title"] {
        if let Some(v) = el.attrs.get(attr) {
            let v = v.trim();
            if !v.is_empty() {
                return v.to_string();
            }
        }
    }
    if is_submit_default {
        "Submit".to_string()
    } else {
        String::new()
    }
}

/// `size` attribute, defaulted and clamped -- see [`DEFAULT_CONTROL_SIZE`]/
/// [`MAX_CONTROL_SIZE`]. `0` (technically meaningless as a field width) and
/// anything unparseable also fall back to the default rather than
/// synthesizing an empty/zero-width placeholder.
fn control_size(el: &Element) -> usize {
    el.attrs
        .get("size")
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .map(|n| n.min(MAX_CONTROL_SIZE))
        .unwrap_or(DEFAULT_CONTROL_SIZE)
}

/// Blank-field filler character -- see the module doc comment's "Placeholder
/// convention" section for why this is `_` rather than a literal space.
const BLANK_FILL: &str = "_";

fn text_field_value(el: &Element) -> String {
    match el.attrs.get("value") {
        Some(v) if !v.is_empty() => v.to_string(),
        _ => BLANK_FILL.repeat(control_size(el)),
    }
}

fn password_mask(el: &Element) -> String {
    match el.attrs.get("value") {
        Some(v) if !v.is_empty() => "*".repeat(v.chars().count()),
        _ => BLANK_FILL.repeat(control_size(el)),
    }
}

/// `<button>`'s label: see `resolve_bracket_label`'s doc comment for the
/// full rung order (own text, then `value`, then `aria-label`, then
/// `title`, then -- only for a submit-typed button inside a `<form>` --
/// the literal "Submit"). `<button>` is the one form control that can hold
/// markup/text content, so it's also the one control kind whose rung (1)
/// (`own_text`, via `dom_util::collect_text`) is ever non-empty; every
/// `<input>` variant is a void element and always passes `""` for that
/// rung instead (see `input_label`). A `<button>`'s effective type --
/// `control_kind`'s own default ("submit" absent a `type` attribute,
/// matching HTML's own default) -- decides whether rung (5) is even in
/// play.
fn button_label(dom: &Dom, el: &Element, form_action: Option<&str>) -> String {
    let text = dom_util::collect_text(dom, el);
    let is_submit_default = control_kind(el) == "submit" && form_action.is_some();
    resolve_bracket_label(el, &text, is_submit_default)
}

/// See the module-level "Placeholder convention" doc comment above for the
/// exact truncation rule.
fn textarea_label(dom: &Dom, el: &Element) -> String {
    let text = dom_util::collect_text(dom, el);
    let mut lines = text.lines();
    let first = lines.next().unwrap_or("");
    let has_more_lines = lines.next().is_some();
    let char_count = first.chars().count();
    if char_count > MAX_TEXTAREA_CHARS {
        let truncated: String = first.chars().take(MAX_TEXTAREA_CHARS).collect();
        format!("{truncated}[...]")
    } else if has_more_lines {
        format!("{first}[...]")
    } else {
        first.to_string()
    }
}

/// Rendering convention for a `<select>`, single- or multi-valued alike:
/// always show just the FIRST selected `<option>`'s text (or the first
/// option at all, with none marked `selected`) — this is a text-mode
/// placeholder, not a real scrollable widget (that's the fb backend's job,
/// M4), so there's no legible way to show "3 of 7 options selected" in one
/// line without real widget geometry. `dom_util::collect_options` (shared
/// with `form.rs`, which DOES need every selected option for real multi-
/// select submission) is reused here for just its first-match lookup.
fn select_label(dom: &Dom, el: &Element) -> String {
    let options = dom_util::collect_options(dom, el, 0);
    let chosen = options.iter().find(|o| o.selected).or_else(|| options.first());
    let text = chosen.map(|o| o.text.as_str()).unwrap_or("");
    format!("[ {text} v]")
}

/// Max `colspan`/`rowspan` a table cell is allowed to carry, per the HTML
/// spec's own limits on these attributes. Clamping here — rather than
/// trusting the wire — keeps the eventual column solver (P8) from being
/// handed an attacker-controlled grid width/height to iterate over.
const MAX_COLSPAN: u16 = 1000;
const MAX_ROWSPAN: u16 = 65534;

/// Parse a `<td>`/`<th>`'s `colspan`/`rowspan` attributes. Missing,
/// unparseable, or zero values default to `1` (HTML's own default and floor
/// for both attributes — a span of 0 has no visual meaning); out-of-range
/// values clamp to [`MAX_COLSPAN`]/[`MAX_ROWSPAN`] rather than being rejected
/// outright, so a hostile document degrades to a large-but-bounded cell
/// instead of losing the cell's content entirely.
fn cell_spans(el: &Element) -> (u16, u16) {
    (parse_span(el.attrs.get("colspan"), MAX_COLSPAN), parse_span(el.attrs.get("rowspan"), MAX_ROWSPAN))
}

fn parse_span(raw: Option<&str>, max: u16) -> u16 {
    // Parse as u32 first so an absurdly large literal (more digits than a
    // u16 holds) parses successfully and then clamps, rather than failing
    // to parse and silently falling back to the same default (1) a
    // deliberately malformed value would — clamping and defaulting are
    // different outcomes worth keeping distinct even though both are safe.
    let v: u32 = match raw.and_then(|s| s.trim().parse().ok()) {
        Some(v) => v,
        None => return 1,
    };
    if v == 0 {
        1
    } else {
        v.min(max as u32) as u16
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom;
    use crate::style::cascade;
    use crate::style::parser;

    fn find(d: &dom::Dom, tag: &str) -> Option<dom::NodeId> {
        find_all(d, tag).into_iter().next()
    }

    fn find_all(d: &dom::Dom, tag: &str) -> Vec<dom::NodeId> {
        let mut out = Vec::new();
        fn walk(d: &dom::Dom, id: dom::NodeId, tag: &str, out: &mut Vec<dom::NodeId>) {
            if let Some(el) = d.node(id).element() {
                if el.name.as_str() == tag {
                    out.push(id);
                }
                for &c in &el.children {
                    walk(d, c, tag, out);
                }
            }
        }
        walk(d, d.root(), tag, &mut out);
        out
    }

    fn count_nodes(node: &LayoutNode) -> usize {
        1 + node.children.iter().map(count_nodes).sum::<usize>()
    }

    /// Concatenate every `Text` box's content across a `LayoutNode` subtree,
    /// in tree order, with NO separator inserted between sibling text nodes
    /// -- unlike `find_text` (which only ever looks inside one node's own
    /// text at a time), this lets a test assert on text that spans multiple
    /// adjacent boxes (e.g. a details-disclosure marker box glued in front
    /// of a `<summary>`'s own text box) the same way it will actually read
    /// once laid out inline.
    fn collect_all_text(node: &LayoutNode, out: &mut String) {
        if let BoxContent::Text(t) = &node.content {
            out.push_str(t);
        }
        for c in &node.children {
            collect_all_text(c, out);
        }
    }

    fn find_text<'a>(node: &'a LayoutNode, needle: &str) -> Option<&'a LayoutNode> {
        if let BoxContent::Text(t) = &node.content {
            if t.contains(needle) {
                return Some(node);
            }
        }
        for c in &node.children {
            if let Some(found) = find_text(c, needle) {
                return Some(found);
            }
        }
        None
    }

    #[test]
    fn display_none_element_and_its_subtree_are_dropped() {
        let d = dom::parser::parse("<div>keep</div><div id=\"gone\">drop <b>me</b></div>");
        let sheet = parser::parse("#gone { display: none; }");
        let styles = cascade::cascade(&d, std::slice::from_ref(&sheet));
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root not display:none");
        assert!(find_text(&root, "keep").is_some());
        assert!(find_text(&root, "drop").is_none());
        assert!(find_text(&root, "me").is_none());
    }

    #[test]
    fn text_node_maps_to_box_content_text() {
        let d = dom::parser::parse("<p>hello</p>");
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        let text_node = find_text(&root, "hello").expect("text fragment present");
        assert!(matches!(&text_node.content, BoxContent::Text(t) if t == "hello"));
    }

    #[test]
    fn plain_element_maps_to_container_with_recursive_children() {
        let d = dom::parser::parse("<div><span>a</span><span>b</span></div>");
        let styles = cascade::cascade(&d, &[]);
        assert!(find(&d, "div").is_some());
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        // Walk down to the div's box by structural shape: it has exactly two
        // Container children, each containing one Text("a"/"b").
        let div_box = {
            fn find_div(node: &LayoutNode) -> Option<&LayoutNode> {
                if matches!(node.content, BoxContent::Container)
                    && node.children.len() == 2
                    && node.children.iter().all(|c| matches!(c.content, BoxContent::Container))
                {
                    return Some(node);
                }
                for c in &node.children {
                    if let Some(found) = find_div(c) {
                        return Some(found);
                    }
                }
                None
            }
            find_div(&root).expect("div-shaped container present")
        };
        assert!(find_text(&div_box.children[0], "a").is_some());
        assert!(find_text(&div_box.children[1], "b").is_some());
    }

    #[test]
    fn img_element_maps_to_replaced_with_attribute_intrinsic_size() {
        let d = dom::parser::parse(r#"<img src="x.png" width="120" height="80">"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");

        fn find_img(node: &LayoutNode) -> Option<&LayoutNode> {
            if matches!(node.content, BoxContent::Replaced { .. }) {
                return Some(node);
            }
            node.children.iter().find_map(find_img)
        }
        let img = find_img(&root).expect("img box present");
        match img.content {
            BoxContent::Replaced { intrinsic, .. } => {
                assert_eq!(intrinsic.w, 120.0);
                assert_eq!(intrinsic.h, 80.0);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn img_element_with_a_decoded_entry_in_the_images_map_carries_that_image() {
        let d = dom::parser::parse(r#"<img src="x.png" width="2" height="2">"#);
        let styles = cascade::cascade(&d, &[]);
        let img_id = find(&d, "img").expect("img node present");

        let decoded = Rc::new(RgbaImage { width: 2, height: 2, pixels: vec![9, 9, 9, 255].repeat(4) });
        let mut images = HashMap::new();
        images.insert(img_id, decoded.clone());

        let root = build_box_tree(&d, &styles, &images).expect("root present");
        fn find_img(node: &LayoutNode) -> Option<&LayoutNode> {
            if matches!(node.content, BoxContent::Replaced { .. }) {
                return Some(node);
            }
            node.children.iter().find_map(find_img)
        }
        let img = find_img(&root).expect("img box present");
        match &img.content {
            BoxContent::Replaced { image, .. } => {
                let got = image.as_ref().expect("decoded image should be threaded through");
                assert!(Rc::ptr_eq(got, &decoded), "should carry the exact Rc from the images map, not a copy");
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn img_element_with_no_images_map_entry_has_no_image() {
        // No entry for this <img>'s NodeId (e.g. the --dump-text path, which
        // always passes an empty map) -> `image` stays `None`, falling back
        // to the intrinsic-size placeholder, exactly like before this
        // packet's threading landed.
        let d = dom::parser::parse(r#"<img src="x.png" width="2" height="2">"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        fn find_img(node: &LayoutNode) -> Option<&LayoutNode> {
            if matches!(node.content, BoxContent::Replaced { .. }) {
                return Some(node);
            }
            node.children.iter().find_map(find_img)
        }
        let img = find_img(&root).expect("img box present");
        match &img.content {
            BoxContent::Replaced { image, .. } => assert!(image.is_none()),
            _ => unreachable!(),
        }
    }

    #[test]
    fn img_element_without_dimensions_defaults_to_zero_intrinsic() {
        let d = dom::parser::parse(r#"<img src="x.png">"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");

        fn find_img(node: &LayoutNode) -> Option<&LayoutNode> {
            if matches!(node.content, BoxContent::Replaced { .. }) {
                return Some(node);
            }
            node.children.iter().find_map(find_img)
        }
        let img = find_img(&root).expect("img box present");
        match img.content {
            BoxContent::Replaced { intrinsic, .. } => {
                assert_eq!(intrinsic.w, 0.0);
                assert_eq!(intrinsic.h, 0.0);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn img_align_left_maps_to_float_left_presentational_hint() {
        let d = dom::parser::parse(r#"<img src="x.png" align="left" width="10" height="10">"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        fn find_img(node: &LayoutNode) -> Option<&LayoutNode> {
            if matches!(node.content, BoxContent::Replaced { .. }) {
                return Some(node);
            }
            node.children.iter().find_map(find_img)
        }
        let img = find_img(&root).expect("img box present");
        assert_eq!(img.style.float, crate::style::computed::Float::Left);
    }

    #[test]
    fn img_align_right_maps_to_float_right_presentational_hint() {
        let d = dom::parser::parse(r#"<img src="x.png" align="right" width="10" height="10">"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        fn find_img(node: &LayoutNode) -> Option<&LayoutNode> {
            if matches!(node.content, BoxContent::Replaced { .. }) {
                return Some(node);
            }
            node.children.iter().find_map(find_img)
        }
        let img = find_img(&root).expect("img box present");
        assert_eq!(img.style.float, crate::style::computed::Float::Right);
    }

    #[test]
    fn img_align_is_case_insensitive() {
        let d = dom::parser::parse(r#"<img src="x.png" align="LEFT" width="10" height="10">"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        fn find_img(node: &LayoutNode) -> Option<&LayoutNode> {
            if matches!(node.content, BoxContent::Replaced { .. }) {
                return Some(node);
            }
            node.children.iter().find_map(find_img)
        }
        let img = find_img(&root).expect("img box present");
        assert_eq!(img.style.float, crate::style::computed::Float::Left);
    }

    #[test]
    fn img_align_top_middle_bottom_are_ignored_not_mapped_to_float() {
        // Out of scope (vertical-align, not float) per the packet brief.
        for v in ["top", "middle", "bottom", "nonsense"] {
            let html = format!(r#"<img src="x.png" align="{v}" width="10" height="10">"#);
            let d = dom::parser::parse(&html);
            let styles = cascade::cascade(&d, &[]);
            let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
            fn find_img(node: &LayoutNode) -> Option<&LayoutNode> {
                if matches!(node.content, BoxContent::Replaced { .. }) {
                    return Some(node);
                }
                node.children.iter().find_map(find_img)
            }
            let img = find_img(&root).expect("img box present");
            assert_eq!(img.style.float, crate::style::computed::Float::None, "align={v} must not set float");
        }
    }

    #[test]
    fn author_css_float_wins_over_align_attribute_hint() {
        // Author CSS `float: right` must win over the `align="left"`
        // presentational hint (the hint only applies when the cascaded
        // `style.float` is still `Float::None`).
        let d = dom::parser::parse(r#"<img src="x.png" align="left" width="10" height="10">"#);
        let sheet = crate::style::parser::parse("img { float: right; }");
        let styles = cascade::cascade(&d, &[sheet]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        fn find_img(node: &LayoutNode) -> Option<&LayoutNode> {
            if matches!(node.content, BoxContent::Replaced { .. }) {
                return Some(node);
            }
            node.children.iter().find_map(find_img)
        }
        let img = find_img(&root).expect("img box present");
        assert_eq!(img.style.float, crate::style::computed::Float::Right, "author CSS float must win over align hint");
    }

    #[test]
    fn img_with_no_align_attribute_keeps_float_none() {
        let d = dom::parser::parse(r#"<img src="x.png" width="10" height="10">"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        fn find_img(node: &LayoutNode) -> Option<&LayoutNode> {
            if matches!(node.content, BoxContent::Replaced { .. }) {
                return Some(node);
            }
            node.children.iter().find_map(find_img)
        }
        let img = find_img(&root).expect("img box present");
        assert_eq!(img.style.float, crate::style::computed::Float::None);
    }

    #[test]
    fn nested_structure_and_order_are_preserved() {
        let d = dom::parser::parse("<ul><li>one</li><li>two</li><li>three</li></ul>");
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");

        fn find_ul(node: &LayoutNode) -> Option<&LayoutNode> {
            if matches!(node.content, BoxContent::Container) && node.children.len() == 3 {
                return Some(node);
            }
            node.children.iter().find_map(find_ul)
        }
        let ul = find_ul(&root).expect("ul-shaped container present");
        assert!(find_text(&ul.children[0], "one").is_some());
        assert!(find_text(&ul.children[1], "two").is_some());
        assert!(find_text(&ul.children[2], "three").is_some());
    }

    #[test]
    fn empty_document_yields_a_root_with_no_children() {
        let d = dom::Dom::new(); // seeded with a bare <html> root, no children
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("bare <html> root is not display:none");
        assert!(root.children.is_empty());
        assert!(matches!(root.content, BoxContent::Container));
    }

    #[test]
    fn table_cell_maps_to_box_content_table_cell_with_spans() {
        let d = dom::parser::parse(
            r#"<table><tr><td colspan="2" rowspan="3">x</td><td>y</td></tr></table>"#,
        );
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");

        fn find_cells<'a>(node: &'a LayoutNode, out: &mut Vec<&'a LayoutNode>) {
            if matches!(node.content, BoxContent::TableCell { .. }) {
                out.push(node);
            }
            for c in &node.children {
                find_cells(c, out);
            }
        }
        let mut cells = Vec::new();
        find_cells(&root, &mut cells);
        assert_eq!(cells.len(), 2, "expected two table cells");

        match cells[0].content {
            BoxContent::TableCell { colspan, rowspan } => {
                assert_eq!(colspan, 2);
                assert_eq!(rowspan, 3);
            }
            _ => unreachable!(),
        }
        // The cell's children (its text content) are still built underneath
        // it, exactly as a Container's would be.
        assert!(find_text(cells[0], "x").is_some());

        match cells[1].content {
            BoxContent::TableCell { colspan, rowspan } => {
                assert_eq!(colspan, 1, "missing colspan defaults to 1");
                assert_eq!(rowspan, 1, "missing rowspan defaults to 1");
            }
            _ => unreachable!(),
        }
        assert!(find_text(cells[1], "y").is_some());
    }

    #[test]
    fn table_cell_span_parsing_defaults_and_clamps() {
        let d = dom::parser::parse(
            r#"<table><tr>
                <td colspan="0" rowspan="0">a</td>
                <td colspan="abc" rowspan="xyz">b</td>
                <td colspan="99999" rowspan="99999">c</td>
            </tr></table>"#,
        );
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");

        fn find_cells<'a>(node: &'a LayoutNode, out: &mut Vec<&'a LayoutNode>) {
            if matches!(node.content, BoxContent::TableCell { .. }) {
                out.push(node);
            }
            for c in &node.children {
                find_cells(c, out);
            }
        }
        let mut cells = Vec::new();
        find_cells(&root, &mut cells);
        assert_eq!(cells.len(), 3);

        // colspan="0"/rowspan="0" -> min 1, never 0.
        match cells[0].content {
            BoxContent::TableCell { colspan, rowspan } => {
                assert_eq!(colspan, 1);
                assert_eq!(rowspan, 1);
            }
            _ => unreachable!(),
        }
        // Unparseable -> default 1.
        match cells[1].content {
            BoxContent::TableCell { colspan, rowspan } => {
                assert_eq!(colspan, 1);
                assert_eq!(rowspan, 1);
            }
            _ => unreachable!(),
        }
        // Absurdly large -> clamped (colspan <= 1000, rowspan <= 65534).
        match cells[2].content {
            BoxContent::TableCell { colspan, rowspan } => {
                assert_eq!(colspan, 1000);
                assert_eq!(rowspan, 65534);
            }
            _ => unreachable!(),
        }
    }

    // ------------------------------------------------------------------
    // `<table border="N">` presentational attribute (packet/table-border):
    // a vintage-HTML ruled-table hint. `border` is NOT an inherited
    // property, so this is applied post-cascade here (in box_tree, which
    // alone knows the TABLE->CELL ancestor relationship `ComputedStyle`/
    // `ElementInfo` don't carry) -- exactly the same rationale as
    // `apply_align_float_hint`'s `float` hint above. See that section's own
    // doc comment for the "post-cascade, gated on cascaded-default" pattern
    // this mirrors.
    // ------------------------------------------------------------------

    /// Collect every node in `node`'s subtree (`node` included) matching
    /// `pred`, in pre-order (a node is visited/pushed BEFORE its own
    /// children are walked) -- tests below rely on that ordering to tell an
    /// outer `<table>`'s own cell apart from a nested inner `<table>`'s cell
    /// purely by which one appears first.
    fn find_by<'a>(node: &'a LayoutNode, pred: &dyn Fn(&LayoutNode) -> bool, out: &mut Vec<&'a LayoutNode>) {
        if pred(node) {
            out.push(node);
        }
        for c in &node.children {
            find_by(c, pred, out);
        }
    }

    fn is_table_box(n: &LayoutNode) -> bool {
        n.style.display == Display::Table
    }

    fn is_cell_box(n: &LayoutNode) -> bool {
        matches!(n.content, BoxContent::TableCell { .. })
    }

    fn assert_border_all(border: &Edges<BorderSide>, width: f32) {
        assert_border_all_colored(border, width, Color::rgb(0x80, 0x80, 0x80));
    }

    fn assert_border_all_colored(border: &Edges<BorderSide>, width: f32, color: Color) {
        for (side, name) in [
            (border.top, "top"),
            (border.right, "right"),
            (border.bottom, "bottom"),
            (border.left, "left"),
        ] {
            assert_eq!(side.style, BorderStyle::Solid, "{name} border should be solid");
            assert_eq!(side.width, width, "{name} border width");
            assert_eq!(side.color, color, "{name} border color");
        }
    }

    fn assert_no_border(border: &Edges<BorderSide>) {
        for (side, name) in [
            (border.top, "top"),
            (border.right, "right"),
            (border.bottom, "bottom"),
            (border.left, "left"),
        ] {
            assert_eq!(side.style, BorderStyle::None, "{name} border should stay unset");
        }
    }

    #[test]
    fn table_border_attribute_stamps_table_and_1px_cell_borders() {
        // A bare `<table border="1">` (no cellspacing) resolves to
        // `border-collapse: collapse` (packet/border-collapse), but
        // (packet/collapse-geometry) the box tree no longer dedups any
        // side for that -- every cell keeps its full 1px four-sided border,
        // same as the table's own frame.
        let d = dom::parser::parse(r#"<table border="1"><tr><td>a</td><td>b</td></tr></table>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");

        let mut tables = Vec::new();
        find_by(&root, &is_table_box, &mut tables);
        assert_eq!(tables.len(), 1, "expected one table box");
        assert_border_all(&tables[0].style.border, 1.0);

        let mut cells = Vec::new();
        find_by(&root, &is_cell_box, &mut cells);
        assert_eq!(cells.len(), 2, "expected two td boxes");
        for cell in cells {
            assert_border_all(&cell.style.border, 1.0);
        }
    }

    #[test]
    fn table_border_n_controls_table_frame_width_cells_stay_1px() {
        // The cell's own border WIDTH stays 1px regardless of the table
        // frame's own width N (`stamp_cell_borders` is untouched by this
        // packet); (packet/collapse-geometry) all four sides stay solid too,
        // dedup no longer applies.
        let d = dom::parser::parse(r#"<table border="3"><tr><td>a</td></tr></table>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");

        let mut tables = Vec::new();
        find_by(&root, &is_table_box, &mut tables);
        assert_eq!(tables.len(), 1);
        assert_border_all(&tables[0].style.border, 3.0);

        let mut cells = Vec::new();
        find_by(&root, &is_cell_box, &mut cells);
        assert_eq!(cells.len(), 1);
        assert_border_all(&cells[0].style.border, 1.0);
    }

    #[test]
    fn table_border_zero_or_absent_yields_no_borders() {
        for html in [
            r#"<table border="0"><tr><td>a</td></tr></table>"#,
            r#"<table><tr><td>a</td></tr></table>"#,
        ] {
            let d = dom::parser::parse(html);
            let styles = cascade::cascade(&d, &[]);
            let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");

            let mut tables = Vec::new();
            find_by(&root, &is_table_box, &mut tables);
            assert_eq!(tables.len(), 1);
            assert_no_border(&tables[0].style.border);

            let mut cells = Vec::new();
            find_by(&root, &is_cell_box, &mut cells);
            assert_eq!(cells.len(), 1);
            assert_no_border(&cells[0].style.border);
        }
    }

    #[test]
    fn table_border_unparseable_attribute_yields_no_borders() {
        let d = dom::parser::parse(r#"<table border="banana"><tr><td>a</td></tr></table>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");

        let mut tables = Vec::new();
        find_by(&root, &is_table_box, &mut tables);
        assert_no_border(&tables[0].style.border);
        let mut cells = Vec::new();
        find_by(&root, &is_cell_box, &mut cells);
        assert_no_border(&cells[0].style.border);
    }

    #[test]
    fn table_border_attribute_never_overrides_author_css_border() {
        // `<table border="1">` (no cellspacing, no author `border-collapse`)
        // still resolves to `collapse`, but (packet/collapse-geometry) that
        // no longer dedups any side -- the td's own AUTHOR border (2px black
        // -- wins over the presentational 1px gray stamp, unaffected by this
        // packet) keeps all four sides.
        let d = dom::parser::parse(r#"<table border="1"><tr><td>a</td></tr></table>"#);
        let sheet = parser::parse("td { border: 2px solid #000000; }");
        let styles = cascade::cascade(&d, std::slice::from_ref(&sheet));
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");

        let mut cells = Vec::new();
        find_by(&root, &is_cell_box, &mut cells);
        assert_eq!(cells.len(), 1);
        // Author CSS wins: the td keeps its own 2px black border color/width
        // on all four sides, NOT the 1px gray presentational-attribute stamp.
        assert_border_all_colored(&cells[0].style.border, 2.0, Color::rgb(0, 0, 0));

        // The table itself has no author border, so it still gets the
        // presentational 1px frame -- gating is per-box, not all-or-nothing
        // for the whole table subtree.
        let mut tables = Vec::new();
        find_by(&root, &is_table_box, &mut tables);
        assert_eq!(tables.len(), 1);
        assert_border_all(&tables[0].style.border, 1.0);
    }

    #[test]
    fn table_border_nested_table_border_zero_does_not_inherit_outer_cells() {
        let d = dom::parser::parse(
            r#"<table border="1"><tr><td><table border="0"><tr><td>inner</td></tr></table></td></tr></table>"#,
        );
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");

        let mut tables = Vec::new();
        find_by(&root, &is_table_box, &mut tables);
        assert_eq!(tables.len(), 2, "expected outer + inner table boxes");
        // Pre-order: the outer table is visited before it recurses into the
        // inner one.
        assert_border_all(&tables[0].style.border, 1.0);
        assert_no_border(&tables[1].style.border);

        let mut cells = Vec::new();
        find_by(&root, &is_cell_box, &mut cells);
        assert_eq!(cells.len(), 2, "expected outer td + inner td");
        // Pre-order: the outer td (wrapping the whole inner table) is
        // visited before the inner td nested inside it. The OUTER table
        // (border="1", no cellspacing) collapses, but (packet/
        // collapse-geometry) that no longer dedups its own td's border --
        // it keeps all four solid sides; the inner table (border="0") has
        // no visible border to begin with, so its td stays fully unset
        // regardless of collapse.
        assert_border_all(&cells[0].style.border, 1.0);
        assert_no_border(&cells[1].style.border);
        assert!(find_text(cells[1], "inner").is_some(), "cells[1] should be the inner td");
    }

    // ------------------------------------------------------------------
    // `<table cellpadding="N">` presentational attribute (packet/
    // table-spacing): mirrors the `<table border="N">` section directly
    // above -- same post-cascade, DEPTH_CAP-bounded, stop-at-nested-table
    // walk, same "gated on cascaded-default" precedence rule (author CSS/
    // inline `style=` wins over the presentational hint).
    // ------------------------------------------------------------------

    fn assert_padding_all(padding: &Edges<LengthPercentage>, px: f32) {
        for (side, name) in
            [(padding.top, "top"), (padding.right, "right"), (padding.bottom, "bottom"), (padding.left, "left")]
        {
            assert_eq!(side, LengthPercentage::Px(px), "{name} padding");
        }
    }

    #[test]
    fn table_cellpadding_attribute_stamps_every_cell_all_sides() {
        let d = dom::parser::parse(r#"<table cellpadding="10"><tr><td>a</td><td>b</td></tr></table>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");

        let mut cells = Vec::new();
        find_by(&root, &is_cell_box, &mut cells);
        assert_eq!(cells.len(), 2, "expected two td boxes");
        for cell in cells {
            assert_padding_all(&cell.style.padding, 10.0);
        }

        // The table's OWN box gets no padding stamped -- only cellpadding
        // affects CELLS, unlike `border` (which stamps both the table's own
        // frame and its cells).
        let mut tables = Vec::new();
        find_by(&root, &is_table_box, &mut tables);
        assert_padding_all(&tables[0].style.padding, 0.0);
    }

    #[test]
    fn table_cellpadding_zero_or_absent_yields_no_padding() {
        for html in [
            r#"<table cellpadding="0"><tr><td>a</td></tr></table>"#,
            r#"<table><tr><td>a</td></tr></table>"#,
        ] {
            let d = dom::parser::parse(html);
            let styles = cascade::cascade(&d, &[]);
            let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
            let mut cells = Vec::new();
            find_by(&root, &is_cell_box, &mut cells);
            assert_eq!(cells.len(), 1);
            assert_padding_all(&cells[0].style.padding, 0.0);
        }
    }

    #[test]
    fn table_cellpadding_unparseable_or_negative_attribute_yields_no_padding() {
        for html in [
            r#"<table cellpadding="banana"><tr><td>a</td></tr></table>"#,
            r#"<table cellpadding="-5"><tr><td>a</td></tr></table>"#,
            r#"<table cellpadding="1.5"><tr><td>a</td></tr></table>"#,
        ] {
            let d = dom::parser::parse(html);
            let styles = cascade::cascade(&d, &[]);
            let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
            let mut cells = Vec::new();
            find_by(&root, &is_cell_box, &mut cells);
            assert_padding_all(&cells[0].style.padding, 0.0);
        }
    }

    #[test]
    fn table_cellpadding_attribute_never_overrides_author_css_padding() {
        let d = dom::parser::parse(r#"<table cellpadding="10"><tr><td>a</td></tr></table>"#);
        let sheet = parser::parse("td { padding: 3px; }");
        let styles = cascade::cascade(&d, std::slice::from_ref(&sheet));
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");

        let mut cells = Vec::new();
        find_by(&root, &is_cell_box, &mut cells);
        assert_eq!(cells.len(), 1);
        // Author CSS wins: the td keeps its own 3px padding, NOT the 10px
        // cellpadding stamp.
        assert_padding_all(&cells[0].style.padding, 3.0);
    }

    #[test]
    fn table_cellpadding_nested_table_padding_is_independent() {
        let d = dom::parser::parse(
            r#"<table cellpadding="10"><tr><td><table cellpadding="2"><tr><td>inner</td></tr></table></td></tr></table>"#,
        );
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");

        let mut cells = Vec::new();
        find_by(&root, &is_cell_box, &mut cells);
        assert_eq!(cells.len(), 2, "expected outer td + inner td");
        // Pre-order: the outer td (wrapping the whole inner table) is
        // visited before the inner td nested inside it.
        assert_padding_all(&cells[0].style.padding, 10.0);
        assert_padding_all(&cells[1].style.padding, 2.0);
        assert!(find_text(cells[1], "inner").is_some(), "cells[1] should be the inner td");
    }

    // ------------------------------------------------------------------
    // `<table border>` default cell padding (packet/border-collapse
    // follow-up): mirrors the `cellpadding` section directly above.
    // ------------------------------------------------------------------

    #[test]
    fn table_border_with_no_cellpadding_gets_the_default_padding() {
        let d = dom::parser::parse(r#"<table border="1"><tr><td>a</td><td>b</td></tr></table>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");

        let mut cells = Vec::new();
        find_by(&root, &is_cell_box, &mut cells);
        assert_eq!(cells.len(), 2);
        for cell in cells {
            assert_padding_all(&cell.style.padding, DEFAULT_TABLE_BORDER_CELL_PADDING);
        }
    }

    #[test]
    fn table_border_with_explicit_cellpadding_zero_suppresses_the_default() {
        // `cellpadding="0"` is a present attribute (even though it stamps
        // literal 0px either way) -- its mere PRESENCE must suppress the
        // default, per this function's own "no cellpadding attribute AT
        // ALL" gate.
        let d = dom::parser::parse(r#"<table border="1" cellpadding="0"><tr><td>a</td></tr></table>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");

        let mut cells = Vec::new();
        find_by(&root, &is_cell_box, &mut cells);
        assert_eq!(cells.len(), 1);
        assert_padding_all(&cells[0].style.padding, 0.0);
    }

    #[test]
    fn table_border_with_unparseable_cellpadding_still_suppresses_the_default() {
        // Presence, not validity: an unparseable `cellpadding` still counts
        // as "the author named this attribute" and must suppress the
        // default too, even though `apply_table_cellpadding_attribute`
        // itself stamps nothing for it (leaving cells at the CSS 0px
        // default either way -- same observable padding, different reason).
        let d = dom::parser::parse(r#"<table border="1" cellpadding="banana"><tr><td>a</td></tr></table>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");

        let mut cells = Vec::new();
        find_by(&root, &is_cell_box, &mut cells);
        assert_padding_all(&cells[0].style.padding, 0.0);
    }

    #[test]
    fn table_border_author_css_padding_wins_over_the_default() {
        let d = dom::parser::parse(r#"<table border="1"><tr><td>a</td></tr></table>"#);
        let sheet = parser::parse("td { padding: 2px; }");
        let styles = cascade::cascade(&d, std::slice::from_ref(&sheet));
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");

        let mut cells = Vec::new();
        find_by(&root, &is_cell_box, &mut cells);
        assert_eq!(cells.len(), 1);
        assert_padding_all(&cells[0].style.padding, 2.0);
    }

    #[test]
    fn table_without_a_border_attribute_gets_no_default_padding() {
        // Plain `<table>` (no `border` attribute at all): completely out of
        // this function's scope, regardless of any author CSS border.
        let d = dom::parser::parse(r#"<table><tr><td>a</td></tr></table>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");

        let mut cells = Vec::new();
        find_by(&root, &is_cell_box, &mut cells);
        assert_padding_all(&cells[0].style.padding, 0.0);
    }

    #[test]
    fn author_css_bordered_table_with_no_border_attribute_gets_no_default_padding() {
        // The kitchen-sink shape: `border-collapse: collapse` + `td {
        // border: 1px solid }` in author CSS, but NO `border` HTML
        // attribute anywhere -- this packet's default-padding step must
        // stay completely out of it (gated on the ATTRIBUTE, not the
        // cascaded border), leaving the author's own (unset) padding
        // exactly as written.
        let d = dom::parser::parse(r#"<table><tr><td>a</td></tr></table>"#);
        let sheet = parser::parse("table { border-collapse: collapse; } td { border: 1px solid #000000; }");
        let styles = cascade::cascade(&d, std::slice::from_ref(&sheet));
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");

        let mut cells = Vec::new();
        find_by(&root, &is_cell_box, &mut cells);
        assert_padding_all(&cells[0].style.padding, 0.0);
    }

    // ------------------------------------------------------------------
    // `border-collapse: collapse` (packet/collapse-geometry): the box tree
    // no longer dedups (zeros) any cell border side for a collapsed table --
    // every cell keeps its FULL cascaded border regardless of `border-
    // collapse`. These tests replace the old dedup-assertion tests (which
    // pinned the removed `apply_border_collapse`/`dedup_cell_borders`
    // behavior) with the new invariant: box-tree construction is completely
    // insensitive to `border_collapse` for border purposes -- the "shared
    // single line" effect is a `layout::block` GEOMETRY concern now (see
    // that module's own tests), not a box-tree style-mutation concern.
    // ------------------------------------------------------------------

    fn assert_border_side(side: &BorderSide, style: BorderStyle, name: &str) {
        assert_eq!(side.style, style, "{name} border style");
    }

    #[test]
    fn table_border_attribute_keeps_full_four_sided_cell_borders_under_collapse() {
        // `<table border="1">` (no cellspacing) resolves to
        // `border-collapse: collapse` (the presentational hint), but the
        // box tree must NOT dedup any side anymore -- every cell keeps all
        // four solid sides, same as the table's own frame.
        let d = dom::parser::parse(r#"<table border="1"><tr><td>a</td><td>b</td></tr></table>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");

        let mut tables = Vec::new();
        find_by(&root, &is_table_box, &mut tables);
        assert_eq!(tables.len(), 1);
        assert_border_all(&tables[0].style.border, 1.0);

        let mut cells = Vec::new();
        find_by(&root, &is_cell_box, &mut cells);
        assert_eq!(cells.len(), 2, "expected two td boxes");
        for cell in cells {
            assert_border_all(&cell.style.border, 1.0);
        }
    }

    #[test]
    fn table_border_with_cellspacing_stays_separate_all_four_cell_sides_solid() {
        // `<table border="1" cellspacing="4">` stays `separate` (the
        // presentational hint only fires with no `cellspacing`) -- every
        // cell keeps all four solid sides, exactly like the collapsed case
        // above (border-collapse never touches cell border STYLE anymore,
        // collapsed or not).
        let d = dom::parser::parse(r#"<table border="1" cellspacing="4"><tr><td>a</td><td>b</td></tr></table>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");

        let mut cells = Vec::new();
        find_by(&root, &is_cell_box, &mut cells);
        assert_eq!(cells.len(), 2);
        for cell in cells {
            assert_border_all(&cell.style.border, 1.0);
        }
    }

    #[test]
    fn author_css_border_collapse_keeps_full_four_sided_cell_borders() {
        // Author-CSS collapse: `table { border-collapse: collapse }` + `td {
        // border: 1px solid #000 }` -- the CASCADED cell border is left
        // completely alone regardless of the table's `border-collapse`.
        let d = dom::parser::parse(r#"<table><tr><td>a</td><td>b</td></tr></table>"#);
        let sheet = parser::parse("table { border-collapse: collapse; } td { border: 1px solid #000000; }");
        let styles = cascade::cascade(&d, std::slice::from_ref(&sheet));
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");

        let mut cells = Vec::new();
        find_by(&root, &is_cell_box, &mut cells);
        assert_eq!(cells.len(), 2);
        for cell in cells {
            assert_border_all_colored(&cell.style.border, 1.0, Color::rgb(0, 0, 0));
        }
    }

    #[test]
    fn separate_mode_table_border_unchanged_no_dedup() {
        // Belt-and-suspenders: a table that never opts into collapse (no
        // `border-collapse` anywhere, no `<table border>`) keeps every
        // cell's border exactly as cascaded -- this packet must not touch
        // `Separate` tables at all.
        let d = dom::parser::parse(r#"<table><tr><td>a</td></tr></table>"#);
        let sheet = parser::parse("td { border: 2px solid #000000; }");
        let styles = cascade::cascade(&d, std::slice::from_ref(&sheet));
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");

        let mut cells = Vec::new();
        find_by(&root, &is_cell_box, &mut cells);
        assert_eq!(cells.len(), 1);
        assert_border_all_colored(&cells[0].style.border, 2.0, Color::rgb(0, 0, 0));
    }

    #[test]
    fn table_border_collapse_nested_table_keeps_full_borders_independent_of_outer() {
        // Same nested-table independence as `stamp_cell_borders`'s own test:
        // the outer table's `border-collapse` must not reach into an inner
        // table that stays separate (has its own `cellspacing`) -- both
        // keep every cell's full four-sided border either way now.
        let d = dom::parser::parse(
            r#"<table border="1"><tr><td><table border="1" cellspacing="4"><tr><td>inner</td></tr></table></td></tr></table>"#,
        );
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");

        let mut cells = Vec::new();
        find_by(&root, &is_cell_box, &mut cells);
        assert_eq!(cells.len(), 2, "expected outer td + inner td");
        // Pre-order: outer td (collapsed) first, inner td (separate) second.
        assert_border_all(&cells[0].style.border, 1.0); // outer td: collapsed, still full border
        assert_border_all(&cells[1].style.border, 1.0); // inner td: separate, all four sides solid
        assert!(find_text(cells[1], "inner").is_some(), "cells[1] should be the inner td");
    }

    #[test]
    fn display_none_root_yields_none() {
        let d = dom::parser::parse("<html><body>x</body></html>");
        let sheet = parser::parse("html { display: none; }");
        let styles = cascade::cascade(&d, std::slice::from_ref(&sheet));
        assert!(build_box_tree(&d, &styles, &HashMap::new()).is_none());
    }

    // ------------------------------------------------------------------
    // Form-control rendering (P-forms, part 2): each control synthesizes a
    // placeholder `Text` label instead of laying out its DOM children (which
    // for `<input>` don't exist at all -- it's a void element -- and for
    // `<button>`/`<textarea>`/`<select>` are submission-only content, not
    // meant to be walked as ordinary boxes). See `build_form_control`'s doc
    // comment for the exact bracket convention asserted below.
    // ------------------------------------------------------------------

    #[test]
    fn text_input_renders_bracketed_value() {
        let d = dom::parser::parse(r#"<input type="text" name="a" value="hi">"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "[hi]").is_some());
    }

    #[test]
    fn text_input_without_type_defaults_to_text_behavior() {
        let d = dom::parser::parse(r#"<input name="a" value="hi">"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "[hi]").is_some());
    }

    #[test]
    fn text_input_without_value_renders_underscores_sized_to_size_attr() {
        // Underscores, not literal spaces: `layout::inline` unconditionally
        // collapses whitespace runs in v1 (see that module's own docs), so
        // a run of plain spaces here would collapse down to one space by
        // the time it reaches the tty grid -- `_` is never whitespace, so
        // it survives untouched. See `build_form_control`'s module doc
        // comment ("Placeholder convention") for the full rationale.
        let d = dom::parser::parse(r#"<input type="text" name="a" size="4">"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "[____]").is_some(), "expected 4 underscores inside brackets");
    }

    #[test]
    fn text_input_without_value_or_size_defaults_to_ten_underscores() {
        let d = dom::parser::parse(r#"<input type="text" name="a">"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        let expected = format!("[{}]", "_".repeat(10));
        assert!(find_text(&root, &expected).is_some());
    }

    #[test]
    fn password_input_masks_value_with_asterisks() {
        let d = dom::parser::parse(r#"<input type="password" name="p" value="secret">"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "[******]").is_some());
    }

    #[test]
    fn checkbox_shows_x_when_checked_and_blank_when_not() {
        let d = dom::parser::parse(r#"<input type="checkbox" name="c" checked>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "[x]").is_some());

        let d2 = dom::parser::parse(r#"<input type="checkbox" name="c">"#);
        let styles2 = cascade::cascade(&d2, &[]);
        let root2 = build_box_tree(&d2, &styles2, &HashMap::new()).expect("root present");
        assert!(find_text(&root2, "[ ]").is_some());
    }

    #[test]
    fn radio_shows_star_when_checked_and_blank_when_not() {
        let d = dom::parser::parse(r#"<input type="radio" name="r" checked>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "(*)").is_some());

        let d2 = dom::parser::parse(r#"<input type="radio" name="r">"#);
        let styles2 = cascade::cascade(&d2, &[]);
        let root2 = build_box_tree(&d2, &styles2, &HashMap::new()).expect("root present");
        assert!(find_text(&root2, "( )").is_some());
    }

    #[test]
    fn submit_input_shows_value_or_default_submit_label() {
        let d = dom::parser::parse(r#"<input type="submit" value="Go">"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "[ Go ]").is_some());

        // T4 contract: a bare `<input type=submit>` with no author-supplied
        // label falls back to the literal "Submit" -- HTML's own UA
        // default for a genuine submit control -- but ONLY inside an
        // enclosing `<form>` (see `resolve_bracket_label`'s doc comment).
        let d2 = dom::parser::parse(r#"<form action="/s"><input type="submit"></form>"#);
        let styles2 = cascade::cascade(&d2, &[]);
        let root2 = build_box_tree(&d2, &styles2, &HashMap::new()).expect("root present");
        assert!(find_text(&root2, "[ Submit ]").is_some());
    }

    #[test]
    fn submit_input_outside_a_form_never_invents_submit() {
        // Same bare `<input type=submit>`, but with no enclosing `<form>`
        // to submit -- semantically no more a "Submit" button than a bare
        // `type=button` one, so the literal default must NOT fire (the D4
        // bug in a different costume: inventing a word the author never
        // wrote, even one that "sounds right" for the element).
        let d = dom::parser::parse(r#"<input type="submit">"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "[ ]").is_some(), "no invented \"Submit\" outside a form");
        assert!(find_text(&root, "[ Submit ]").is_none());
    }

    #[test]
    fn reset_and_button_type_inputs_never_invent_a_default_label() {
        // Neither `reset` nor `button` input types are ever the literal-
        // "Submit" rung -- only a genuine submit control gets that. With
        // nothing else to go on (no value/aria-label/title), both render
        // an honest empty control instead of a fabricated word.
        let d = dom::parser::parse(r#"<input type="reset">"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "[ ]").is_some(), "no invented \"Reset\" label");

        let d2 = dom::parser::parse(r#"<input type="button" value="Click">"#);
        let styles2 = cascade::cascade(&d2, &[]);
        let root2 = build_box_tree(&d2, &styles2, &HashMap::new()).expect("root present");
        assert!(find_text(&root2, "[ Click ]").is_some());

        let d3 = dom::parser::parse(r#"<input type="button">"#);
        let styles3 = cascade::cascade(&d3, &[]);
        let root3 = build_box_tree(&d3, &styles3, &HashMap::new()).expect("root present");
        assert!(find_text(&root3, "[ ]").is_some(), "value-less type=button is an empty control, NOT \"Button\"");
    }

    #[test]
    fn input_label_honors_aria_label_and_title_rungs() {
        // Rung (3): aria-label wins over the submit default -- this is the
        // exact D4 shape (an icon-only control with no value/text, just an
        // author-supplied accessible name).
        let d = dom::parser::parse(r#"<form action="/s"><input type="submit" aria-label="Go now"></form>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "[ Go now ]").is_some());

        // Rung (4): title wins over the submit default when there's no
        // aria-label either.
        let d2 = dom::parser::parse(r#"<form action="/s"><input type="submit" title="Proceed"></form>"#);
        let styles2 = cascade::cascade(&d2, &[]);
        let root2 = build_box_tree(&d2, &styles2, &HashMap::new()).expect("root present");
        assert!(find_text(&root2, "[ Proceed ]").is_some());

        // value (rung 2) still beats aria-label (rung 3) and title (rung 4).
        let d3 = dom::parser::parse(r#"<input type="submit" value="Go" aria-label="Ignored" title="Also ignored">"#);
        let styles3 = cascade::cascade(&d3, &[]);
        let root3 = build_box_tree(&d3, &styles3, &HashMap::new()).expect("root present");
        assert!(find_text(&root3, "[ Go ]").is_some());
    }

    #[test]
    fn hidden_input_renders_nothing() {
        let d = dom::parser::parse(r#"<div>before<input type="hidden" name="x" value="topsecret123"><span>after</span></div>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "before").is_some());
        assert!(find_text(&root, "after").is_some());
        assert!(find_text(&root, "topsecret123").is_none(), "hidden input's value must never appear");
    }

    #[test]
    fn button_element_text_beats_value_beats_default() {
        // Rung (1): own text content beats EVERY other rung, including
        // `value` -- this is the T4 reordering (the old rule had `value`
        // win over child text; the new rule matches "the element's own
        // text content ... wins" from the packet brief).
        let d = dom::parser::parse(r#"<button value="X">Send</button>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "[ Send ]").is_some(), "own text content beats the value attribute");

        // Rung (2): with no text content, `value` wins.
        let d2 = dom::parser::parse(r#"<button value="X"></button>"#);
        let styles2 = cascade::cascade(&d2, &[]);
        let root2 = build_box_tree(&d2, &styles2, &HashMap::new()).expect("root present");
        assert!(find_text(&root2, "[ X ]").is_some(), "value attr wins when there is no text content");

        // Rung (5): the literal "Submit" default only fires for a
        // submit-typed button (HTML's own default `type`) inside a `<form>`,
        // with nothing else to go on.
        let d3 = dom::parser::parse(r#"<form action="/s"><button></button></form>"#);
        let styles3 = cascade::cascade(&d3, &[]);
        let root3 = build_box_tree(&d3, &styles3, &HashMap::new()).expect("root present");
        assert!(find_text(&root3, "[ Submit ]").is_some(), "default \"Submit\" only inside a form, with nothing else");

        // Same empty `<button>`, no enclosing `<form>` this time -- D4's
        // exact failure mode (an empty/unlabeled button rendering an
        // invented "Submit") must not recur.
        let d4 = dom::parser::parse(r#"<button></button>"#);
        let styles4 = cascade::cascade(&d4, &[]);
        let root4 = build_box_tree(&d4, &styles4, &HashMap::new()).expect("root present");
        assert!(find_text(&root4, "[ ]").is_some(), "outside a form, an empty submit-typed button is honest, not \"Submit\"");
        assert!(find_text(&root4, "[ Submit ]").is_none());
    }

    #[test]
    fn icon_button_uses_aria_label_not_submit() {
        // The exact D4 repro shape: httpforever's icon buttons
        // (`<button type=button aria-label="Theme">`, no text content, no
        // value) were rendering `[ Submit ]` -- a fabricated label for a
        // control that isn't even a submit button (explicit
        // `type=button`). Rung (3) (aria-label) must win, and the literal
        // "Submit" default must never fire for a non-submit type at all.
        let d = dom::parser::parse(r#"<button type="button" aria-label="Theme"><svg></svg></button>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "[ Theme ]").is_some());
        assert!(find_text(&root, "[ Submit ]").is_none());
    }

    #[test]
    fn button_type_button_with_nothing_renders_empty_not_submit() {
        let d = dom::parser::parse(r#"<button type="button"></button>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "[ ]").is_some());
        assert!(find_text(&root, "[ Submit ]").is_none());
    }

    #[test]
    fn button_label_honors_title_rung() {
        // Rung (4): title wins over the submit default when there's no
        // text/value/aria-label.
        let d = dom::parser::parse(r#"<form action="/s"><button type="submit" title="Proceed"></button></form>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "[ Proceed ]").is_some());
    }

    #[test]
    fn textarea_shows_short_text_verbatim() {
        let d = dom::parser::parse(r#"<textarea name="n">hello</textarea>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "hello").is_some());
    }

    #[test]
    fn textarea_truncates_long_first_line() {
        let d = dom::parser::parse(r#"<textarea name="n">this line is definitely longer than twenty chars</textarea>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "[...]").is_some(), "long content should be truncated with an ellipsis marker");
    }

    #[test]
    fn textarea_marks_multiline_content_even_if_first_line_is_short() {
        let d = dom::parser::parse("<textarea name=\"n\">line one\nline two</textarea>");
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "line one[...]").is_some());
    }

    #[test]
    fn select_shows_selected_option_text() {
        let d = dom::parser::parse(
            r#"<select name="color"><option value="r">Red</option><option value="g" selected>Green</option></select>"#,
        );
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "[ Green v]").is_some());
    }

    #[test]
    fn select_with_no_selected_option_defaults_to_first() {
        let d = dom::parser::parse(
            r#"<select name="color"><option value="r">Red</option><option value="g">Green</option></select>"#,
        );
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "[ Red v]").is_some());
    }

    #[test]
    fn select_with_no_options_renders_without_panicking() {
        let d = dom::parser::parse(r#"<select name="color"></select>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "[  v]").is_some());
    }

    #[test]
    fn form_controls_never_recurse_into_their_own_dom_children_as_generic_boxes() {
        // A <select>'s <option>s must not show up as their own independent
        // Container/Text boxes distinct from the synthesized label -- the
        // whole control is exactly one Container + one Text child.
        let d = dom::parser::parse(r#"<select name="c"><option>Only</option></select>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");

        fn find_select_box(node: &LayoutNode) -> Option<&LayoutNode> {
            let is_select_shaped = node.children.len() == 1
                && matches!(&node.children[0].content, BoxContent::Text(t) if t.contains('[') && t.contains('v'));
            if is_select_shaped {
                return Some(node);
            }
            for c in &node.children {
                if let Some(found) = find_select_box(c) {
                    return Some(found);
                }
            }
            None
        }
        let select_box = find_select_box(&root).expect("select-shaped container present");
        assert_eq!(select_box.children.len(), 1, "select must synthesize exactly one label child");
    }


    #[test]
    fn deeply_nested_dom_does_not_abort_and_returns() {
        let depth = 3000;
        let mut html = String::new();
        for _ in 0..depth {
            html.push_str("<div>");
        }
        html.push_str("leaf");
        for _ in 0..depth {
            html.push_str("</div>");
        }
        // `dom::parser::parse` is iterative (a `Vec`-backed stack, not
        // program-stack recursion) so it handles this depth fine — verified
        // separately. `style::cascade::cascade`'s `visit`, however, IS
        // recursive with no depth cap of its own (a pre-existing gap this
        // packet does not own/fix — flagged to the orchestrator; see the P7
        // report) and reliably stack-overflows (SIGABRT) on a DOM this deep,
        // independent of anything `build_box_tree` does. To isolate exactly
        // what THIS function is responsible for, synthesize a same-length,
        // all-default styles vector here instead of calling cascade.
        let d = dom::parser::parse(&html);
        let styles = vec![ComputedStyle::default(); d.len()];

        // Must return (not abort/hang) even though the DOM nests far past
        // DEPTH_CAP.
        let root = build_box_tree(&d, &styles, &HashMap::new());
        assert!(root.is_some());
        let root = root.unwrap();

        // M1 (reviewer follow-up): don't just assert "it returned" — that
        // alone wouldn't catch a regression that silently dropped the depth
        // cap but happened not to crash at this particular depth/stack size.
        // Positively confirm the cap actually fired: the "leaf" text sits
        // 3000 levels deep, far past DEPTH_CAP, so it must be ABSENT from
        // the built tree (the over-deep subtree was truncated to an empty
        // container before ever reaching it) ...
        assert!(find_text(&root, "leaf").is_none(), "content past DEPTH_CAP should have been dropped, not built");
        // ... and the total node count must stay bounded near DEPTH_CAP, not
        // anywhere close to the full 3000-deep chain.
        let total = count_nodes(&root);
        assert!(total > 0);
        assert!(
            total <= DEPTH_CAP + 5,
            "expected the tree to be truncated near DEPTH_CAP ({DEPTH_CAP}), got {total} nodes — the depth cap may not be firing"
        );
    }

    // ------------------------------------------------------------------
    // <details>/<summary> disclosure (M5 part 1): a <details> WITHOUT an
    // `open` attribute is collapsed -- only its first <summary> child (the
    // clickable label) is built into the box tree; every other child is
    // dropped. `<details open>` (any/no value) is expanded -- the summary
    // AND every other child are built normally. See `build_details_node`'s
    // doc comment for the full disclosure-marker convention asserted below.
    // ------------------------------------------------------------------

    #[test]
    fn collapsed_details_without_open_shows_only_the_summary() {
        let d = dom::parser::parse("<details><summary>Label</summary><p>Hidden content</p></details>");
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "Label").is_some());
        assert!(find_text(&root, "Hidden content").is_none(), "non-summary children must be dropped when collapsed");
    }

    #[test]
    fn open_details_shows_the_summary_and_every_other_child() {
        let d = dom::parser::parse(r#"<details open><summary>Label</summary><p>Shown content</p></details>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "Label").is_some());
        assert!(find_text(&root, "Shown content").is_some(), "non-summary children must be kept when open");
    }

    #[test]
    fn details_open_attribute_with_any_value_still_counts_as_open() {
        // HTML boolean attribute semantics: `open`, `open=""`, `open="open"`
        // are all equally "present" -- only its ABSENCE means collapsed.
        for markup in ["<details open>", "<details open=\"\">", "<details open=\"open\">"] {
            let html = format!("{markup}<summary>Label</summary><p>Shown</p></details>");
            let d = dom::parser::parse(&html);
            let styles = cascade::cascade(&d, &[]);
            let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
            assert!(find_text(&root, "Shown").is_some(), "for {markup}");
        }
    }

    #[test]
    fn details_disclosure_marker_reflects_open_vs_closed_state() {
        // Closed: `> ` prefix. Open: `v ` prefix (ASCII, deterministic --
        // see the module doc section on the disclosure-marker convention).
        let closed = dom::parser::parse("<details><summary>Label</summary></details>");
        let styles = cascade::cascade(&closed, &[]);
        let root = build_box_tree(&closed, &styles, &HashMap::new()).expect("root present");
        let mut text = String::new();
        collect_all_text(&root, &mut text);
        assert!(text.contains("> Label"), "closed details should show a '> ' marker, got: {text:?}");

        let open = dom::parser::parse("<details open><summary>Label</summary></details>");
        let styles2 = cascade::cascade(&open, &[]);
        let root2 = build_box_tree(&open, &styles2, &HashMap::new()).expect("root present");
        let mut text2 = String::new();
        collect_all_text(&root2, &mut text2);
        assert!(text2.contains("v Label"), "open details should show a 'v ' marker, got: {text2:?}");
    }

    #[test]
    fn details_with_no_summary_uses_the_default_label_when_collapsed() {
        let d = dom::parser::parse("<details><p>Only content</p></details>");
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        let mut text = String::new();
        collect_all_text(&root, &mut text);
        assert!(text.contains("> Details"), "no <summary> should fall back to the default label, got: {text:?}");
        assert!(!text.contains("Only content"), "still collapsed: non-summary content must not appear");
    }

    #[test]
    fn details_with_no_summary_uses_the_default_label_when_open() {
        let d = dom::parser::parse("<details open><p>Only content</p></details>");
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        let mut text = String::new();
        collect_all_text(&root, &mut text);
        assert!(text.contains("v Details"), "no <summary> should fall back to the default label, got: {text:?}");
        assert!(text.contains("Only content"), "open: the real content should still show alongside the default label");
    }

    #[test]
    fn multiple_summaries_only_the_first_becomes_the_disclosure_label_when_open() {
        let d = dom::parser::parse("<details open><summary>First</summary><summary>Second</summary></details>");
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        let mut text = String::new();
        collect_all_text(&root, &mut text);
        assert!(text.contains("v First"), "the FIRST summary carries the marker, got: {text:?}");
        assert!(text.contains("Second"), "a second <summary> still renders as ordinary content when open");
        assert!(!text.contains("v Second"), "only the first summary gets the disclosure marker");
    }

    #[test]
    fn multiple_summaries_collapsed_shows_only_the_first_summary() {
        let d = dom::parser::parse("<details><summary>First</summary><summary>Second</summary></details>");
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        let mut text = String::new();
        collect_all_text(&root, &mut text);
        assert!(text.contains("> First"));
        assert!(!text.contains("Second"), "everything past the first summary must be dropped when collapsed");
    }

    #[test]
    fn summary_not_a_direct_child_is_not_recognized_as_the_disclosure_label() {
        // Only a DIRECT-child <summary> is the disclosure label (matches the
        // HTML5 "first summary element child" rule) -- one buried inside a
        // wrapper element falls back to the default label, and (since it's
        // not treated as the summary) is itself just ordinary content shown
        // or dropped along with everything else per the open/closed state.
        let d = dom::parser::parse("<details open><div><summary>Buried</summary></div></details>");
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        let mut text = String::new();
        collect_all_text(&root, &mut text);
        assert!(text.contains("v Details"), "no direct-child summary -> default label, got: {text:?}");
        assert!(text.contains("Buried"), "the buried summary still renders as ordinary content when open");
    }

    #[test]
    fn details_with_no_children_at_all_does_not_panic() {
        let d = dom::parser::parse("<details></details>");
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        let mut text = String::new();
        collect_all_text(&root, &mut text);
        assert!(text.contains("> Details"));
    }

    #[test]
    fn nested_details_do_not_panic() {
        let d = dom::parser::parse(
            "<details open><summary>Outer</summary><details><summary>Inner</summary><p>Inner content</p></details></details>",
        );
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new());
        assert!(root.is_some());
        let root = root.unwrap();
        let mut text = String::new();
        collect_all_text(&root, &mut text);
        assert!(text.contains("v Outer"));
        assert!(text.contains("> Inner"), "the nested (collapsed) details keeps its own marker/summary");
        assert!(!text.contains("Inner content"), "the nested details is itself collapsed (no open attr)");
    }

    // ------------------------------------------------------------------
    // <noscript> (M5 part 2): Stele has no JavaScript by construction, so
    // <noscript> content is exactly "what to show when scripting is
    // unavailable" -- always, here. It must render like any other block
    // container, never like <script>/<style>/<head> (display: none).
    // ------------------------------------------------------------------

    #[test]
    fn noscript_content_is_visible_not_hidden() {
        let d = dom::parser::parse("<noscript><p>fallback content</p></noscript>");
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        assert!(find_text(&root, "fallback content").is_some());
    }

    #[test]
    fn noscript_is_block_level_per_the_ua_sheet() {
        let d = dom::parser::parse("<noscript>x</noscript>");
        let styles = cascade::cascade(&d, &[]);
        let noscript_id = find(&d, "noscript").expect("noscript present");
        assert_eq!(styles[noscript_id].display, Display::Block);
    }

    // ------------------------------------------------------------------
    // List markers (M6): `<ul>/<ol>/<li>` render with no bullets/numbers at
    // all today -- kitchen-sink coverage flagged this gap. Each `<li>`
    // built as a direct child of a `<ul>`/`<ol>` gets a synthesized leading
    // `Text` marker glued onto its children (same "synthesize a leaf
    // carrying a stand-in" convention `build_details_node`'s disclosure
    // marker and `build_form_control`'s placeholder labels already use),
    // chosen from that `<li>`'s own (possibly author-overridden, inherited)
    // `ComputedStyle::list_style_type` and an ordinal counted per-list, in
    // document order, over `<li>` DIRECT children only. See
    // `build_list_container_node`'s doc comment for the full convention.
    // ------------------------------------------------------------------

    #[test]
    fn ul_items_each_get_a_leading_bullet_marker() {
        let d = dom::parser::parse("<ul><li>a</li><li>b</li></ul>");
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        let mut text = String::new();
        collect_all_text(&root, &mut text);
        assert!(text.contains("* a"), "expected a bullet marker before item a, got: {text:?}");
        assert!(text.contains("* b"), "expected a bullet marker before item b, got: {text:?}");
    }

    #[test]
    fn ol_items_get_sequential_ordinal_markers() {
        let d = dom::parser::parse("<ol><li>a</li><li>b</li><li>c</li></ol>");
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        let mut text = String::new();
        collect_all_text(&root, &mut text);
        assert!(text.contains("1. a"), "got: {text:?}");
        assert!(text.contains("2. b"), "got: {text:?}");
        assert!(text.contains("3. c"), "got: {text:?}");
    }

    #[test]
    fn list_style_type_none_suppresses_the_marker() {
        let d = dom::parser::parse(r#"<ul style="list-style-type: none;"><li>a</li></ul>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        let mut text = String::new();
        collect_all_text(&root, &mut text);
        assert_eq!(text.trim(), "a", "list-style-type: none must suppress the marker entirely, got: {text:?}");
    }

    #[test]
    fn nested_list_ordinals_restart_at_one_per_list() {
        let d = dom::parser::parse(
            "<ol><li>outer-1<ol><li>inner-1</li><li>inner-2</li></ol></li><li>outer-2</li></ol>",
        );
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        let mut text = String::new();
        collect_all_text(&root, &mut text);
        assert!(text.contains("1. outer-1"), "got: {text:?}");
        assert!(text.contains("1. inner-1"), "nested list must restart its own ordinal at 1, got: {text:?}");
        assert!(text.contains("2. inner-2"), "got: {text:?}");
        assert!(text.contains("2. outer-2"), "outer list's own counter must not be perturbed by the nested list, got: {text:?}");
    }

    #[test]
    fn li_with_no_list_parent_gets_no_marker_and_does_not_panic() {
        let d = dom::parser::parse("<div><li>stray</li></div>");
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        let mut text = String::new();
        collect_all_text(&root, &mut text);
        assert_eq!(text.trim(), "stray", "an <li> outside any list must render with no synthesized marker");
    }

    #[test]
    fn li_with_non_block_display_gets_no_marker() {
        // packet/layout-float-recon (marker synthesis previously keyed off
        // the `<li>` TAG alone, `is_li`, never `ComputedStyle.display`) and
        // packet/display-list-item (the UA default is now `display:
        // list-item`, `src/style/ua.rs`, not `block`): CSS only puts a
        // marker on a list-item box. Contract, direction 1: an ORDINARY
        // `<li>` (default UA `display: list-item`) still keeps its marker.
        let d = dom::parser::parse(r#"<ul><li>a</li><li style="display: inline;">b</li></ul>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        let mut text = String::new();
        collect_all_text(&root, &mut text);
        assert!(text.contains("* a"), "an ordinary <li> (default display: list-item) must still get its marker, got: {text:?}");
        // Contract, direction 2: an <li> whose OWN computed display is no
        // longer list-item-shaped must lose the marker, while its own
        // content still renders (no marker, not no box).
        assert!(!text.contains("* b"), "an <li> with display overridden away from list-item must not get a marker, got: {text:?}");
        assert!(text.contains('b'), "the display-overridden <li>'s own content must still render, got: {text:?}");
    }

    #[test]
    fn ol_item_with_non_block_display_does_not_consume_an_ordinal() {
        // Companion to `li_with_non_block_display_gets_no_marker`: a
        // non-block-display <li> is no longer a counted list item at all
        // (real CSS: only a list-item box advances the list's counter), so
        // the ordinal sequence must skip straight over it rather than
        // leaving a gap or double-counting.
        let d = dom::parser::parse(r#"<ol><li>a</li><li style="display: inline;">skip</li><li>c</li></ol>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        let mut text = String::new();
        collect_all_text(&root, &mut text);
        assert!(text.contains("1. a"), "got: {text:?}");
        assert!(!text.contains(". skip"), "a non-block-display <li> must not receive an ordinal marker, got: {text:?}");
        assert!(text.contains("2. c"), "the ordinal must not be consumed by the non-block-display <li>, got: {text:?}");
    }

    #[test]
    fn li_with_block_display_gets_no_marker() {
        // packet/display-list-item: this is the case packet #58's stopgap
        // (`tag_is_li && display == Display::Block`) could NOT handle -- an
        // `<li>` whose author CSS explicitly re-asserts `display: block`
        // (mirroring the W3C CSS1 float test's `li{display:block; /* i.e.,
        // suppress marker */ ...}`, `fixtures/css1-float-5526c.html`) used
        // to be indistinguishable from an ordinary `<li>`, because both
        // resolved to the identical `Display::Block`. Now that the UA
        // default is `Display::ListItem` (`style/ua.rs`), an author `display:
        // block` override is a real, detectable departure from list-item-
        // ness, and must suppress the marker while still rendering content.
        let d = dom::parser::parse(r#"<ul><li>a</li><li style="display: block;">b</li></ul>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        let mut text = String::new();
        collect_all_text(&root, &mut text);
        assert!(text.contains("* a"), "an ordinary <li> (default display: list-item) must still get its marker, got: {text:?}");
        assert!(!text.contains("* b"), "an <li> with display overridden to block must not get a marker, got: {text:?}");
        assert!(text.contains('b'), "the display-overridden <li>'s own content must still render, got: {text:?}");
    }

    #[test]
    fn ol_item_with_block_display_does_not_consume_an_ordinal() {
        // Companion to `li_with_block_display_gets_no_marker`, mirroring
        // `ol_item_with_non_block_display_does_not_consume_an_ordinal` for
        // the `display: block` case specifically (the case #58 could not
        // detect): a `display: block` <li> is no longer counted, so
        // numbering must skip straight over it, matching real CSS (only a
        // `list-item` box advances the list's counter).
        let d = dom::parser::parse(r#"<ol><li>a</li><li style="display: block;">skip</li><li>c</li></ol>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        let mut text = String::new();
        collect_all_text(&root, &mut text);
        assert!(text.contains("1. a"), "got: {text:?}");
        assert!(!text.contains(". skip"), "a block-display <li> must not receive an ordinal marker, got: {text:?}");
        assert!(text.contains("2. c"), "the ordinal must not be consumed by the block-display <li>, got: {text:?}");
    }

    #[test]
    fn empty_list_does_not_panic() {
        let d = dom::parser::parse("<ul></ul><ol></ol>");
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new());
        assert!(root.is_some());
    }

    #[test]
    fn huge_ordered_list_does_not_panic_and_numbers_the_last_item() {
        let mut html = String::from("<ol>");
        for i in 0..10_000 {
            html.push_str(&format!("<li>item{i}</li>"));
        }
        html.push_str("</ol>");
        let d = dom::parser::parse(&html);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new());
        assert!(root.is_some());
        let root = root.unwrap();
        let mut text = String::new();
        collect_all_text(&root, &mut text);
        assert!(text.contains("1. item0"), "got prefix: {:?}", &text[..text.len().min(40)]);
        assert!(text.contains("10000. item9999"), "expected the last item's ordinal to reach 10000");
    }

    #[test]
    fn deeply_nested_lists_do_not_abort() {
        let depth = 3000;
        let mut html = String::new();
        for _ in 0..depth {
            html.push_str("<ul><li>");
        }
        html.push_str("leaf");
        for _ in 0..depth {
            html.push_str("</li></ul>");
        }
        let d = dom::parser::parse(&html);
        let styles = vec![ComputedStyle::default(); d.len()];
        let root = build_box_tree(&d, &styles, &HashMap::new());
        assert!(root.is_some());
    }

    #[test]
    fn ordered_list_honors_the_start_attribute() {
        let d = dom::parser::parse(r#"<ol start="5"><li>a</li><li>b</li></ol>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        let mut text = String::new();
        collect_all_text(&root, &mut text);
        assert!(text.contains("5. a"), "got: {text:?}");
        assert!(text.contains("6. b"), "got: {text:?}");
    }

    // ------------------------------------------------------------------
    // Interactive provenance (P7 freeze amendment): `<a href>` links and
    // form controls carry `Interactive` provenance from the DOM into the
    // rendered `Fragment` stream. This packet only lands the carrier +
    // populates it -- no click/focus/submit behavior yet -- so these tests
    // check the `LayoutNode`/`Fragment` data, not any interaction.
    // ------------------------------------------------------------------

    #[test]
    fn link_text_fragments_carry_the_interactive_link_marker() {
        let d = dom::parser::parse(r#"<a href="/x">link text</a>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        let fragments = crate::layout::layout(&root, Size { w: 640.0, h: 480.0 });
        let text_fragments: Vec<_> =
            fragments.iter().filter(|f| matches!(f.kind, crate::layout::FragmentKind::Text { .. })).collect();
        assert!(!text_fragments.is_empty(), "expected at least one text fragment");
        for f in &text_fragments {
            match &f.interactive {
                Some(Interactive::Link { href }) => assert_eq!(&**href, "/x"),
                other => panic!("expected every link-text fragment to carry Interactive::Link, got {other:?}"),
            }
        }
    }

    #[test]
    fn wrapped_link_text_spanning_multiple_lines_carries_the_same_href_on_every_line() {
        // Narrow viewport forces this link's text onto more than one line --
        // every resulting line fragment must carry the SAME href, not just
        // the first.
        let d = dom::parser::parse(r#"<a href="/wrap">aaaaaaaaaa bbbbbbbbbb cccccccccc</a>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        let fragments = crate::layout::layout(&root, Size { w: 90.0, h: 480.0 });
        let text_fragments: Vec<_> =
            fragments.iter().filter(|f| matches!(f.kind, crate::layout::FragmentKind::Text { .. })).collect();
        assert!(
            text_fragments.len() >= 2,
            "expected the link's text to wrap across multiple line fragments, got {}",
            text_fragments.len()
        );
        for f in &text_fragments {
            match &f.interactive {
                Some(Interactive::Link { href }) => assert_eq!(&**href, "/wrap"),
                other => panic!("expected every wrapped line to carry the same Interactive::Link, got {other:?}"),
            }
        }
    }

    #[test]
    fn form_controls_carry_the_interactive_form_control_marker_with_kind_name_and_action() {
        let d = dom::parser::parse(
            r#"<form action="/s"><input name="q" type="text" value="hi"><input type="submit"></form>"#,
        );
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");

        let text_input = find_text(&root, "[hi]").expect("text input placeholder present");
        match &text_input.interactive {
            Some(Interactive::FormControl { kind, name, form_action }) => {
                assert_eq!(&**kind, "text");
                assert_eq!(name.as_deref(), Some("q"));
                assert_eq!(form_action.as_deref(), Some("/s"));
            }
            other => panic!("expected Interactive::FormControl on the text input, got {other:?}"),
        }

        let submit = find_text(&root, "[ Submit ]").expect("submit button placeholder present");
        match &submit.interactive {
            Some(Interactive::FormControl { kind, name, form_action }) => {
                assert_eq!(&**kind, "submit");
                assert_eq!(*name, None, "an unnamed submit control has no `name`");
                assert_eq!(form_action.as_deref(), Some("/s"));
            }
            other => panic!("expected Interactive::FormControl on the submit button, got {other:?}"),
        }
    }

    #[test]
    fn form_control_outside_any_form_has_no_form_action() {
        let d = dom::parser::parse(r#"<input name="q" type="text" value="hi">"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        let text_input = find_text(&root, "[hi]").expect("text input placeholder present");
        match &text_input.interactive {
            Some(Interactive::FormControl { form_action, .. }) => {
                assert_eq!(*form_action, None, "a control outside any <form> has no enclosing action");
            }
            other => panic!("expected Interactive::FormControl, got {other:?}"),
        }
    }

    #[test]
    fn ordinary_text_and_box_have_no_interactive_marker() {
        let d = dom::parser::parse("<p>hello</p>");
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        let text_node = find_text(&root, "hello").expect("text present");
        assert!(text_node.interactive.is_none());
        assert!(root.interactive.is_none(), "the plain root container is not interactive either");
    }

    #[test]
    fn link_nested_in_paragraph_in_list_item_is_tagged_sibling_text_is_not() {
        let d = dom::parser::parse(r#"<ul><li><p>before <a href="/deep">link</a> after</p></li></ul>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");

        let link_text = find_text(&root, "link").expect("link text present");
        match &link_text.interactive {
            Some(Interactive::Link { href }) => assert_eq!(&**href, "/deep"),
            other => panic!("expected the nested link's text to carry Interactive::Link, got {other:?}"),
        }

        let before_text = find_text(&root, "before").expect("sibling text present");
        assert!(before_text.interactive.is_none(), "non-link sibling text must not be tagged");
        let after_text = find_text(&root, "after").expect("sibling text present");
        assert!(after_text.interactive.is_none(), "non-link sibling text must not be tagged");
    }

    #[test]
    fn empty_href_is_still_a_link_and_does_not_panic() {
        let d = dom::parser::parse(r#"<a href="">empty</a>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        let link_text = find_text(&root, "empty").expect("text present");
        match &link_text.interactive {
            Some(Interactive::Link { href }) => assert_eq!(&**href, ""),
            other => panic!("expected Interactive::Link with an empty (but present) href, got {other:?}"),
        }
    }

    #[test]
    fn anchor_without_href_attribute_is_not_tagged_as_a_link() {
        // Only a PRESENT `href` (even empty) makes an `<a>` a link -- see
        // `is_link`'s doc comment.
        let d = dom::parser::parse("<a>not a link</a>");
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");
        let text_node = find_text(&root, "not a link").expect("text present");
        assert!(text_node.interactive.is_none(), "an <a> with no href attribute at all must not be tagged as a link");
    }

    #[test]
    fn link_wrapping_an_image_tags_the_image_too() {
        let d = dom::parser::parse(r#"<a href="/img"><img src="x.png" width="4" height="4"></a>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles, &HashMap::new()).expect("root present");

        fn find_img(node: &LayoutNode) -> Option<&LayoutNode> {
            if matches!(node.content, BoxContent::Replaced { .. }) {
                return Some(node);
            }
            node.children.iter().find_map(find_img)
        }
        let img = find_img(&root).expect("img box present");
        match &img.interactive {
            Some(Interactive::Link { href }) => assert_eq!(&**href, "/img"),
            other => panic!("expected the image nested under the link to carry Interactive::Link too, got {other:?}"),
        }
    }

    #[test]
    fn deeply_nested_link_does_not_panic() {
        let depth = 3000;
        let mut html = String::new();
        for _ in 0..depth {
            html.push_str("<div>");
        }
        html.push_str(r#"<a href="/deep">leaf</a>"#);
        for _ in 0..depth {
            html.push_str("</div>");
        }
        let d = dom::parser::parse(&html);
        let styles = vec![ComputedStyle::default(); d.len()];
        let root = build_box_tree(&d, &styles, &HashMap::new());
        assert!(root.is_some(), "must not panic/abort on a pathologically deep link ancestor chain");
    }

    #[test]
    fn deeply_nested_form_does_not_panic() {
        let depth = 3000;
        let mut html = String::new();
        for _ in 0..depth {
            html.push_str("<div>");
        }
        html.push_str(r#"<form action="/s"><input name="q" type="text"></form>"#);
        for _ in 0..depth {
            html.push_str("</div>");
        }
        let d = dom::parser::parse(&html);
        let styles = vec![ComputedStyle::default(); d.len()];
        let root = build_box_tree(&d, &styles, &HashMap::new());
        assert!(root.is_some(), "must not panic/abort on a pathologically deep form ancestor chain");
    }
}
