//! The cascade (P2, Wave 1): fold the user-agent stylesheet and author sheets
//! onto the DOM to produce one [`ComputedStyle`] per node. The UA sheet is where
//! element semantics live (block vs inline defaults, replaced elements, form
//! controls) — `dom::ast` deliberately stays name-agnostic.

use std::sync::OnceLock;

use crate::dom::{Dom, Node, NodeId};
use crate::style::computed::{
    BorderSide, BorderStyle, BoxSizing, Dimension, Edges, GridRepetitionCount, GridTemplateComponent, GridTrack,
    GridTrackSize, LengthPercentage, LengthPercentageAuto, LineHeight,
};
use crate::style::selector::{ElementInfo, Specificity};
use crate::style::ua::UA_CSS;
use crate::style::value::{
    self, presentational_hints, BorderRaw, Declarations, EdgesRaw, Env, RawGridRepetitionCount,
    RawGridTemplateComponent, RawGridTrack, RawGridTrackSize, RawLength, RawLengthAuto, RawLineHeight,
};
use crate::style::{parser, ComputedStyle, Stylesheet};
use crate::surface::Color;

fn ua_stylesheet() -> &'static Stylesheet {
    static UA: OnceLock<Stylesheet> = OnceLock::new();
    UA.get_or_init(|| parser::parse(UA_CSS))
}

/// Compute a style for every node in `dom`, indexed by `NodeId`. Author sheets
/// apply after the UA sheet, in source order (specificity + order resolved by
/// P2). This is contract-testable and gets strict test-first treatment.
///
/// Total: never panics on any `dom`/`author_sheets` combination (an empty DOM
/// yields an empty `Vec`, unmatched nodes simply fall back to CSS initial
/// values / UA defaults) -- INCLUDING arbitrarily deep nesting. Hostile or
/// generated markup (quote threads, WYSIWYG exports, nested-table soup) can
/// nest thousands of elements deep; `visit` below walks the tree with an
/// explicit heap-allocated stack rather than the call stack, so there is no
/// depth at which this can overflow (unlike a plain recursive walk, which
/// reliably SIGABRTs a few thousand levels down -- the recursion-hardening
/// packet's whole reason for existing). This also keeps cascade fully
/// CORRECT at any depth: every node, however deep, still gets its real
/// resolved-and-inherited style, not a capped/degraded approximation.
pub fn cascade(dom: &Dom, author_sheets: &[Stylesheet]) -> Vec<ComputedStyle> {
    let mut out = vec![ComputedStyle::default(); dom.len()];
    if dom.is_empty() {
        return out;
    }
    let ua = ua_stylesheet();
    visit(dom, dom.root(), ua, author_sheets, &mut out);
    out
}

/// One pending unit of work in the iterative walk: either compute+store the
/// style for `id` (with its parent's already-resolved style, `None` at the
/// root) and descend into its children, or -- once all of a subtree's
/// children have been pushed -- pop that subtree's `ElementInfo` back off
/// `ancestors` now that nothing left on the stack still needs it as an
/// ancestor.
///
/// `parent_env` (packet T1a) threads the inherited custom-property
/// environment down the walk alongside the parent's already-resolved
/// `ComputedStyle` -- `Env` deliberately does NOT become a `ComputedStyle`
/// field (see `Env`'s own doc comment for why: `ComputedStyle` is a frozen
/// contract with layout/paint), so it has to ride along as its own bit of
/// per-frame state instead.
enum Frame {
    Enter { id: NodeId, parent: Option<ComputedStyle>, parent_env: Env },
    Exit,
}

/// Explicit-stack equivalent of the old recursive `visit`: identical
/// semantics (same `ancestors` chain visible to each element when its rules
/// are matched, same parent-style propagation to children, same "text takes
/// the parent's style wholesale" rule), just driven by a `Vec<Frame>` on the
/// heap instead of the native call stack, so nesting depth cannot blow it.
fn visit(dom: &Dom, root: NodeId, ua: &Stylesheet, author: &[Stylesheet], out: &mut [ComputedStyle]) {
    let mut ancestors: Vec<ElementInfo> = Vec::new();
    let mut stack: Vec<Frame> = vec![Frame::Enter { id: root, parent: None, parent_env: Env::default() }];

    while let Some(frame) = stack.pop() {
        match frame {
            Frame::Exit => {
                ancestors.pop();
            }
            Frame::Enter { id, parent, parent_env } => match dom.node(id) {
                // Character data carries no rules of its own; it takes the
                // parent's computed style wholesale (the inline engine reads
                // font/color etc. straight off it).
                Node::Text(_) => {
                    if let Some(p) = &parent {
                        out[id] = p.clone();
                    }
                }
                Node::Element(el) => {
                    let is_root = id == root;
                    let info = ElementInfo::from_element(&el.name, &el.attrs, is_root);
                    // Vintage HTML presentational attributes (packet/
                    // presentational-attrs: <font color/size>, bgcolor,
                    // align=, <body text>) -- computed straight from the
                    // `Element` already in hand (its attrs aren't part of
                    // `ElementInfo`) and folded in as their own cascade tier
                    // below, between the UA sheet and author CSS.
                    let presentational = presentational_hints(el.name.as_str(), &el.attrs);
                    let mut decls = fold_matching_declarations(ua, author, &ancestors, &info, &presentational);
                    // Inline `style="..."` is the highest-precedence CSS
                    // origin (beats both UA and every author sheet
                    // regardless of specificity) — folded in last so it
                    // wins per property, same `Declarations::overlay`
                    // last-writer-wins mechanism `fold_matching_declarations`
                    // already uses for UA vs. author. Reads straight off the
                    // `Element` already in hand; `cascade`'s frozen signature
                    // needs no new parameter for this.
                    if let Some(style_attr) = el.attrs.get("style") {
                        decls.overlay(&parser::parse_inline(style_attr));
                    }
                    // This element's custom-property environment (packet
                    // T1a): the inherited chain (`parent_env`) overlaid with
                    // this element's OWN `--name` declarations
                    // (`decls.custom`) — built BEFORE `resolve`, since
                    // `resolve` needs it to substitute any `var()`-bearing
                    // `deferred` declarations this element itself carries.
                    let env = parent_env.child(&decls.custom);
                    let style = resolve(&decls, parent.as_ref(), &env);
                    out[id] = style.clone();

                    ancestors.push(info);
                    stack.push(Frame::Exit);
                    // Push in reverse so children are popped (and thus
                    // visited) in source order, matching the old recursive
                    // walk's iteration order exactly.
                    for &child in el.children.iter().rev() {
                        stack.push(Frame::Enter { id: child, parent: Some(style.clone()), parent_env: env.clone() });
                    }
                }
            },
        }
    }
}

/// Merge every UA + presentational-hint + author rule that matches this
/// element and fold their declarations onto one accumulator, in cascade
/// precedence order: sorted globally by `(tier, specificity, source order)`,
/// applied low-to-high so the highest-precedence match's fields win
/// (`Declarations::overlay`).
///
/// Three tiers, lowest to highest precedence (packet/presentational-attrs
/// added the middle one): `0` UA sheet, `1` this element's presentational
/// hints (`value::presentational_hints` -- `<font color>`, `bgcolor`, etc.),
/// `2` author `<style>`/linked sheets. We don't support `!important`, so
/// within a tier only specificity (then sheet index, then in-sheet source
/// order) breaks ties -- exactly as before this packet, just with the old
/// `is_author: bool` widened to a `u8` so a middle tier could slot in
/// without disturbing UA-vs-author precedence at all (`0 < 2` still holds
/// exactly like `false < true` did). The presentational-hint tier is a
/// single bundle per element rather than a set of matched selector rules, so
/// its specificity is irrelevant (nothing else shares its tier to break a
/// tie against) -- pushed with `Specificity::default()`.
///
/// *Within* the author tier, every matching rule from *every* author sheet is
/// compared by specificity together, with sheet index (then in-sheet source
/// order) only breaking exact specificity ties — a later sheet must not
/// automatically beat an earlier one on lower specificity (that was the bug:
/// folding one sheet at a time made specificity only work within a single
/// sheet).
fn fold_matching_declarations(
    ua: &Stylesheet,
    author: &[Stylesheet],
    ancestors: &[ElementInfo],
    info: &ElementInfo,
    presentational: &Declarations,
) -> Declarations {
    // (tier, specificity, sheet_index, in-sheet source order, declarations)
    let mut candidates: Vec<(u8, Specificity, usize, u32, &Declarations)> = Vec::new();

    for r in parser::matching_rules(ua, ancestors, info) {
        candidates.push((0, r.selector.specificity(), 0, r.order, &r.declarations));
    }
    candidates.push((1, Specificity::default(), 0, 0, presentational));
    for (sheet_index, sheet) in author.iter().enumerate() {
        for r in parser::matching_rules(sheet, ancestors, info) {
            candidates.push((2, r.selector.specificity(), sheet_index, r.order, &r.declarations));
        }
    }
    candidates.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)).then(a.3.cmp(&b.3)));

    let mut decls = Declarations::default();
    for (_, _, _, _, d) in candidates {
        decls.overlay(d);
    }
    decls
}

/// Turn one node's folded `Declarations` (still raw: px/pt/em/% units, not
/// yet resolved to pixels) into a full `ComputedStyle`, given the parent's
/// already-resolved style (`None` at the root).
///
/// Two CSS rules drive every field below:
///   - inherited properties fall back to the *parent's computed value*
///     (or the CSS initial value at the root);
///   - non-inherited (box) properties fall back straight to the CSS initial
///     value regardless of the parent — brief §4's "box properties do not
///     inherit".
/// `em`/`%` on `font-size` resolve against the *parent's* font size; every
/// other `em` resolves against *this node's own* resolved font size, per
/// CSS (computed here first, so everything else can use it).
fn resolve(d: &Declarations, parent: Option<&ComputedStyle>, env: &Env) -> ComputedStyle {
    // Deferred var()-bearing declarations (packet T1a): substitute each
    // one's `var()` calls against this element's full custom-property
    // environment (`env`, built by the caller — `cascade::visit` — from the
    // inherited chain plus this element's own `--name` declarations), now
    // that it's finally available (it couldn't be, at parse time). A
    // successful substitution is applied to a LOCAL clone of `d` via
    // `value::apply_property`, exactly as if it had been parsed eagerly —
    // every macro/field below this point then sees it precisely like any
    // other declaration. A failed substitution ("invalid at computed-value
    // time", in CSS's own terms — an undefined custom property with no
    // fallback, or a cyclic/pathologically deep chain — see `value::
    // substitute_vars`'s own doc comment) simply leaves that field unset on
    // this local `d`, so it falls through to the ordinary inherited/CSS-
    // initial fallback exactly like a declaration that was never present at
    // all — never a crash, never a garbage value.
    let mut d = d.clone();
    for (name, tokens) in d.deferred.clone() {
        if let Some(substituted) = value::substitute_vars(&tokens, env, &[], 0) {
            value::apply_property(&name, &substituted, &mut d);
        }
    }
    let d = &d; // shadow back to `&Declarations` so every existing line below (which all read `d.field`) needs zero further changes

    let default = ComputedStyle::default();
    let parent_font_size = parent.map(|p| p.font_size).unwrap_or(default.font_size);

    let font_size = match d.font_size {
        Some(raw) => resolve_font_size(raw, parent_font_size),
        None => parent_font_size,
    };

    let line_height = match d.line_height {
        Some(RawLineHeight::Normal) => LineHeight::Normal,
        Some(RawLineHeight::Number(n)) => LineHeight::Px(n * font_size),
        // `%` on line-height is a percentage of the element's own font-size
        // (same as `em`), not of the containing block — unlike width/margin/
        // padding, there's no deferred `Percent` variant on `LineHeight` for
        // layout to resolve later, so this must resolve eagerly here.
        Some(RawLineHeight::Length(RawLength::Percent(p))) => LineHeight::Px(font_size * p / 100.0),
        Some(RawLineHeight::Length(raw)) => LineHeight::Px(raw_to_px(raw, font_size)),
        None => parent.map(|p| p.line_height).unwrap_or(default.line_height),
    };

    macro_rules! inherited {
        ($field:ident) => {
            d.$field.unwrap_or_else(|| parent.map(|p| p.$field).unwrap_or(default.$field))
        };
    }
    macro_rules! own {
        ($field:ident) => {
            d.$field.unwrap_or(default.$field)
        };
    }

    ComputedStyle {
        color: inherited!(color),
        background_color: own!(background_color),
        // Not `Copy` (unlike every other `own!`-eligible field), and not
        // inherited (CSS: `background-image` is a box-level property, same
        // treatment as `background_color` right above) — cloned by hand
        // rather than through the `own!` macro. `Declarations::default()`'s
        // `background_image` is already `None`, matching
        // `ComputedStyle::default().background_image`, so there's no
        // separate "default" to fall back to the way `own!` needs one.
        background_image: d.background_image.clone(),
        font_family: inherited!(font_family),
        font_size,
        font_weight: inherited!(font_weight),
        font_style: inherited!(font_style),
        line_height,
        text_align: inherited!(text_align),
        text_decoration: own!(text_decoration),
        white_space: inherited!(white_space),
        vertical_align: own!(vertical_align),
        list_style_type: inherited!(list_style_type),

        display: own!(display),
        width: resolve_dimension(d.width, font_size, default.width),
        height: resolve_dimension(d.height, font_size, default.height),
        margin: Edges {
            top: resolve_lpa(d.margin.top, font_size, default.margin.top),
            right: resolve_lpa(d.margin.right, font_size, default.margin.right),
            bottom: resolve_lpa(d.margin.bottom, font_size, default.margin.bottom),
            left: resolve_lpa(d.margin.left, font_size, default.margin.left),
        },
        padding: Edges {
            top: resolve_lp(d.padding.top, font_size, default.padding.top),
            right: resolve_lp(d.padding.right, font_size, default.padding.right),
            bottom: resolve_lp(d.padding.bottom, font_size, default.padding.bottom),
            left: resolve_lp(d.padding.left, font_size, default.padding.left),
        },
        border: resolve_border(d.border, d.border_top, &d.border_width, d.border_style, d.border_color, font_size),
        // packet/table-spacing: non-inherited ("own") resolution -- see
        // `ComputedStyle::border_spacing_x/y`'s own doc comment for why.
        // `raw_to_px` is safe here because `value::apply_property`/
        // `presentational_hints` never produce a `RawLength::Percent` for
        // this field (border-spacing has no percentage form in CSS, and the
        // parser rejects `%` tokens outright) -- so there's no percent case
        // silently collapsing to 0 in practice.
        border_spacing_x: d.border_spacing_x.map(|l| raw_to_px(l, font_size)).unwrap_or(default.border_spacing_x),
        border_spacing_y: d.border_spacing_y.map(|l| raw_to_px(l, font_size)).unwrap_or(default.border_spacing_y),
        // packet/border-collapse: non-inherited ("own") resolution -- see
        // `ComputedStyle::border_collapse`'s own doc comment for why.
        border_collapse: own!(border_collapse),
        float: own!(float),
        clear: own!(clear),
        // packet/acid1-content-box: non-inherited ("own"), same shape as
        // `border_collapse`/`float`/`clear` right above -- `own!` falls
        // back to `ComputedStyle::default().box_sizing` (`BoxSizing::
        // ContentBox`, CSS's real initial value) whenever nothing declared
        // `box-sizing` at all.
        box_sizing: own!(box_sizing),

        flex_direction: own!(flex_direction),
        flex_wrap: own!(flex_wrap),
        justify_content: own!(justify_content),
        align_items: own!(align_items),
        align_self: own!(align_self),
        flex_grow: own!(flex_grow),
        flex_shrink: own!(flex_shrink),
        flex_basis: resolve_dimension(d.flex_basis, font_size, default.flex_basis),
        gap: d.gap.map(|l| raw_to_px(l, font_size)).unwrap_or(default.gap),
        // packet/t3-inline-spacing: `column_gap` only carries a value when
        // the two-value `gap` shorthand actually declared a distinct
        // column-gap -- `None` (not `default.column_gap`, which is always
        // `None` anyway) is the correct un-set result, mirroring `d.gap`'s
        // own `Option` shape one line up.
        column_gap: d.column_gap.map(|l| raw_to_px(l, font_size)),

        // packet/css-grid: non-inherited ("own") resolution, same treatment
        // `flex_direction`/etc. above already get -- real CSS `grid-
        // template-columns`/`rows` aren't inherited either.
        grid_template_columns: resolve_grid_template(&d.grid_template_columns, font_size),
        grid_template_rows: resolve_grid_template(&d.grid_template_rows, font_size),
    }
}

fn resolve_font_size(raw: RawLength, parent_font_size: f32) -> f32 {
    match raw {
        RawLength::Px(v) => v,
        RawLength::Pt(v) => pt_to_px(v),
        RawLength::Em(v) => v * parent_font_size,
        RawLength::Percent(v) => parent_font_size * v / 100.0,
    }
}

fn pt_to_px(v: f32) -> f32 {
    v * 96.0 / 72.0
}

/// Resolve a raw length against `font_size` (this node's own, per CSS `em`
/// semantics for properties other than `font-size` itself). `%` is left as
/// `LengthPercentage::Percent`/`Dimension::Percent` — layout resolves that
/// against the containing block; only unit *conversion* (em/pt → px) happens
/// here.
fn raw_to_px(raw: RawLength, font_size: f32) -> f32 {
    match raw {
        RawLength::Px(v) => v,
        RawLength::Pt(v) => pt_to_px(v),
        RawLength::Em(v) => v * font_size,
        RawLength::Percent(_) => 0.0, // callers needing percent use the *_lp/_dimension helpers
    }
}

/// packet/css-grid: resolve a raw `grid-template-columns`/`rows` value
/// (`None` if the declaration was never set) into `ComputedStyle`'s own
/// `Vec<GridTemplateComponent>` -- `None` resolves to an empty `Vec`,
/// matching `ComputedStyle::default()`'s own "no explicit template" value
/// (this field's own doc comment, `style/computed.rs`).
fn resolve_grid_template(raw: &Option<Vec<RawGridTemplateComponent>>, font_size: f32) -> Vec<GridTemplateComponent> {
    match raw {
        None => Vec::new(),
        Some(components) => components.iter().map(|c| resolve_grid_component(c, font_size)).collect(),
    }
}

fn resolve_grid_component(c: &RawGridTemplateComponent, font_size: f32) -> GridTemplateComponent {
    match c {
        RawGridTemplateComponent::Single(track) => GridTemplateComponent::Single(resolve_grid_track(track, font_size)),
        RawGridTemplateComponent::Repeat(count, tracks) => GridTemplateComponent::Repeat(
            resolve_grid_repetition(*count),
            tracks.iter().map(|t| resolve_grid_track(t, font_size)).collect(),
        ),
    }
}

fn resolve_grid_repetition(count: RawGridRepetitionCount) -> GridRepetitionCount {
    match count {
        RawGridRepetitionCount::Count(n) => GridRepetitionCount::Count(n),
        RawGridRepetitionCount::AutoFill => GridRepetitionCount::AutoFill,
        RawGridRepetitionCount::AutoFit => GridRepetitionCount::AutoFit,
    }
}

fn resolve_grid_track(track: &RawGridTrack, font_size: f32) -> GridTrack {
    match track {
        RawGridTrack::Bare(size) => GridTrack::Bare(resolve_grid_track_size(*size, font_size)),
        RawGridTrack::MinMax(min, max) => {
            GridTrack::MinMax(resolve_grid_track_size(*min, font_size), resolve_grid_track_size(*max, font_size))
        }
    }
}

/// `RawGridTrackSize::Length(RawLength::Percent(_))` deliberately does NOT
/// go through `raw_to_px` -- that helper collapses a percent to `0.0`
/// (correct for its OTHER callers, which resolve percent separately via
/// `resolve_lp`/`resolve_dimension`), but a grid track's own percent has no
/// such separate path -- it must survive as `GridTrackSize::Percent` all
/// the way to taffy, which resolves it against the grid container's own
/// size during layout (`layout::block::map_grid_track`).
fn resolve_grid_track_size(size: RawGridTrackSize, font_size: f32) -> GridTrackSize {
    match size {
        RawGridTrackSize::Fr(f) => GridTrackSize::Fr(f),
        RawGridTrackSize::Length(RawLength::Percent(p)) => GridTrackSize::Percent(p),
        RawGridTrackSize::Length(l) => GridTrackSize::Length(raw_to_px(l, font_size)),
    }
}

fn resolve_dimension(v: Option<RawLengthAuto>, font_size: f32, default: Dimension) -> Dimension {
    match v {
        None => default,
        Some(RawLengthAuto::Auto) => Dimension::Auto,
        Some(RawLengthAuto::Length(RawLength::Percent(p))) => Dimension::Percent(p),
        Some(RawLengthAuto::Length(l)) => Dimension::Px(raw_to_px(l, font_size)),
    }
}

fn resolve_lpa(v: Option<RawLengthAuto>, font_size: f32, default: LengthPercentageAuto) -> LengthPercentageAuto {
    match v {
        None => default,
        Some(RawLengthAuto::Auto) => LengthPercentageAuto::Auto,
        Some(RawLengthAuto::Length(RawLength::Percent(p))) => LengthPercentageAuto::Percent(p),
        Some(RawLengthAuto::Length(l)) => LengthPercentageAuto::Px(raw_to_px(l, font_size)),
    }
}

fn resolve_lp(v: Option<RawLength>, font_size: f32, default: LengthPercentage) -> LengthPercentage {
    match v {
        None => default,
        Some(RawLength::Percent(p)) => LengthPercentage::Percent(p),
        Some(l) => LengthPercentage::Px(raw_to_px(l, font_size)),
    }
}

/// `border` is curated as solid-only (brief §4): any other named style
/// resolves to `BorderStyle::None` upstream in `value::apply_property`, and
/// an unset style here (declared width/color with no keyword) also means
/// "no visible border" — CSS's own initial `border-style: none`. A solid
/// border with no explicit width falls back to the classic "medium" ≈3px.
///
/// `top` (packet/hr-rule: the `border-top` longhand) overrides ONLY
/// `Edges.top` after `v` (the `border` shorthand) has set its uniform
/// baseline for all four sides — mirrors real CSS's longhand-wins-over-
/// shorthand behavior for the one side both could touch. `<hr>`'s UA rule is
/// the only caller that ever sets `top` without `v`, so this is exercised
/// today as "no `border` at all, `border-top` sets just the top side, the
/// other three stay `BorderSide::default()`".
///
/// `width`/`style`/`color` (packet/acid1-content-box: `border-width`/
/// `border-style`/`border-color`, `Declarations::border_width`'s own doc
/// comment has the full "why") layer on top of `v`/`top` LAST, per side,
/// touching only the sub-fields those longhands actually declared —
/// `fixtures/css1-float-5526c.html`'s `blockquote` never sets `border`/
/// `border-top` at all, so for it `v`/`top` resolve to four `BorderSide::
/// default()`s (invisible) and these three longhands are the ONLY signal;
/// but the override is written generically (works whether or not a `border`
/// shorthand is also present) rather than special-cased to "no shorthand",
/// so a future fixture mixing both keeps the real CSS longhand-wins-over-
/// shorthand behavior `top` already established above. Every existing
/// fixture leaves `width`/`style`/`color` at their all-`None` default, so
/// this whole step is a no-op for them (`None.unwrap_or(existing)` on every
/// sub-field) — zero golden churn.
fn resolve_border(
    v: Option<BorderRaw>,
    top: Option<BorderRaw>,
    width: &EdgesRaw<RawLength>,
    style: Option<BorderStyle>,
    color: Option<Color>,
    font_size: f32,
) -> Edges<BorderSide> {
    let base = Edges::all(resolve_border_side(v, font_size));
    let with_top = match top {
        None => base,
        Some(t) => Edges { top: resolve_border_side(Some(t), font_size), ..base },
    };
    Edges {
        top: apply_border_side_longhands(with_top.top, width.top, style, color, font_size),
        right: apply_border_side_longhands(with_top.right, width.right, style, color, font_size),
        bottom: apply_border_side_longhands(with_top.bottom, width.bottom, style, color, font_size),
        left: apply_border_side_longhands(with_top.left, width.left, style, color, font_size),
    }
}

/// Overrides `side`'s width/style/color with whichever of `width`/`style`/
/// `color` the `border-width`/`border-style`/`border-color` longhands
/// actually declared for this edge, leaving the rest of `side` (already
/// resolved from the `border`/`border-top` shorthand, or `BorderSide::
/// default()` if neither was declared) untouched. `None` for any of the
/// three params is "this longhand wasn't declared", not "clear it" — same
/// convention as every other `Option`-shaped cascade input in this module.
fn apply_border_side_longhands(
    side: BorderSide,
    width: Option<RawLength>,
    style: Option<BorderStyle>,
    color: Option<Color>,
    font_size: f32,
) -> BorderSide {
    BorderSide {
        width: width.map(|w| raw_to_px(w, font_size)).unwrap_or(side.width),
        style: style.unwrap_or(side.style),
        color: color.unwrap_or(side.color),
    }
}

fn resolve_border_side(v: Option<BorderRaw>, font_size: f32) -> BorderSide {
    match v {
        None => BorderSide::default(),
        Some(b) => {
            let style = b.style.unwrap_or(BorderStyle::None);
            let width = if style == BorderStyle::Solid {
                b.width.map(|w| raw_to_px(w, font_size)).unwrap_or(3.0)
            } else {
                0.0
            };
            let color = b.color.unwrap_or(Color::BLACK);
            BorderSide { width, style, color }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom;
    use crate::style::computed::*;
    use crate::style::parser;
    use crate::surface::Color;

    fn find(d: &dom::Dom, tag: &str) -> dom::NodeId {
        find_all(d, tag)[0]
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

    #[test]
    fn div_is_block_span_is_inline_by_ua_defaults() {
        let d = dom::parser::parse("<div>x</div><span>y</span>");
        let styles = cascade(&d, &[]);
        assert_eq!(styles[find(&d, "div")].display, Display::Block);
        assert_eq!(styles[find(&d, "span")].display, Display::Inline);
    }

    #[test]
    fn head_and_style_are_display_none() {
        let d = dom::parser::parse("<html><head><style>p{color:red}</style></head><body>x</body></html>");
        let styles = cascade(&d, &[]);
        assert_eq!(styles[find(&d, "head")].display, Display::None);
        assert_eq!(styles[find(&d, "style")].display, Display::None);
    }

    #[test]
    fn table_elements_get_table_display_values_from_ua_sheet() {
        let d = dom::parser::parse(
            "<table><thead><tr><th>h</th></tr></thead><tbody><tr><td>x</td></tr></tbody></table>",
        );
        let styles = cascade(&d, &[]);
        assert_eq!(styles[find(&d, "table")].display, Display::Table);
        assert_eq!(styles[find(&d, "thead")].display, Display::TableRowGroup);
        assert_eq!(styles[find(&d, "tbody")].display, Display::TableRowGroup);
        for tr in find_all(&d, "tr") {
            assert_eq!(styles[tr].display, Display::TableRow);
        }
        assert_eq!(styles[find(&d, "th")].display, Display::TableCell);
        assert_eq!(styles[find(&d, "td")].display, Display::TableCell);
    }

    #[test]
    fn author_display_grid_and_grid_template_cascade_onto_computed_style() {
        // packet/css-grid: `grid-template-columns`/`rows` aren't inherited
        // (own doc comment on `resolve`'s struct literal) -- the child
        // `<div>` here has no author rule of its own, so it must see the
        // DEFAULT empty template, not its grid-container parent's.
        let d = dom::parser::parse("<div class=\"g\"><div>item</div></div>");
        let sheet = parser::parse(
            ".g { display: grid; grid-template-columns: repeat(auto-fill, minmax(200px, 1fr)); gap: 10px; }",
        );
        let styles = cascade(&d, &[sheet]);
        let grid_id = find(&d, "div"); // first <div> in document order is `.g`
        assert_eq!(styles[grid_id].display, Display::Grid);
        assert_eq!(
            styles[grid_id].grid_template_columns,
            vec![GridTemplateComponent::Repeat(
                GridRepetitionCount::AutoFill,
                vec![GridTrack::MinMax(GridTrackSize::Length(200.0), GridTrackSize::Fr(1.0))]
            )]
        );
        assert_eq!(styles[grid_id].gap, 10.0);

        let child_id = find_all(&d, "div")[1];
        assert_eq!(styles[child_id].grid_template_columns, Vec::new(), "grid-template-columns must not inherit");
    }

    #[test]
    fn li_gets_list_item_display_from_ua_sheet_by_default() {
        // packet/display-list-item: the UA default for `<li>` is `display:
        // list-item` (real CSS), not `block` -- see `Display::ListItem`'s
        // own doc comment (`style/computed.rs`) for why the distinction
        // matters (list-marker emission, `layout::box_tree::
        // build_list_container_node`).
        let d = dom::parser::parse("<ul><li>a</li></ul>");
        let styles = cascade(&d, &[]);
        assert_eq!(styles[find(&d, "li")].display, Display::ListItem);
    }

    #[test]
    fn author_css_can_override_li_display_away_from_list_item() {
        // Author CSS wins over the UA default, same as any other property --
        // `display: block`/`display: inline` on an `<li>` must resolve to
        // exactly that value, not stay pinned at `ListItem`.
        let d = dom::parser::parse(r#"<ul><li style="display: block;">a</li><li style="display: inline;">b</li></ul>"#);
        let styles = cascade(&d, &[]);
        let items: Vec<_> = find_all(&d, "li");
        assert_eq!(styles[items[0]].display, Display::Block);
        assert_eq!(styles[items[1]].display, Display::Inline);
    }

    #[test]
    fn child_inherits_color_and_font_family_from_parent() {
        let d = dom::parser::parse(r#"<div><span>x</span></div>"#);
        let sheet = parser::parse("div { color: green; font-family: monospace; }");
        let styles = cascade(&d, std::slice::from_ref(&sheet));
        let span = find(&d, "span");
        assert_eq!(styles[span].color, Color::rgb(0, 128, 0));
        assert_eq!(styles[span].font_family, FontFamily::Monospace);
    }

    #[test]
    fn background_image_does_not_inherit() {
        // Packet bg-image: background_image is a box property (like
        // background_color right above it in ComputedStyle) -- a child with
        // no background-image declaration of its own must NOT pick up its
        // parent's, even though it inherits `color` from the very same
        // parent in the very same cascade pass.
        let d = dom::parser::parse(r#"<div><span>x</span></div>"#);
        let sheet = parser::parse("div { background-image: url(x.png); color: green; }");
        let styles = cascade(&d, std::slice::from_ref(&sheet));
        let div = find(&d, "div");
        let span = find(&d, "span");
        assert_eq!(styles[div].background_image.as_deref(), Some("x.png"));
        assert_eq!(styles[span].background_image, None);
        assert_eq!(styles[span].color, Color::rgb(0, 128, 0), "color still inherits normally");
    }

    #[test]
    fn border_top_declaration_sets_only_the_top_side() {
        // packet/hr-rule: `border-top` must override only `Edges.top`,
        // leaving right/bottom/left at the CSS initial (`BorderStyle::None`)
        // -- unlike the `border` shorthand, which sets all four uniformly.
        let d = dom::parser::parse("<div>x</div>");
        let sheet = parser::parse("div { border-top: 1px solid #808080; }");
        let styles = cascade(&d, std::slice::from_ref(&sheet));
        let div = find(&d, "div");
        let top = styles[div].border.top;
        assert_eq!(top.style, BorderStyle::Solid);
        assert_eq!(top.width, 1.0);
        assert_eq!(top.color, Color::rgb(0x80, 0x80, 0x80));
        for side in [styles[div].border.right, styles[div].border.bottom, styles[div].border.left] {
            assert_eq!(side, BorderSide::default(), "non-top sides must stay unset");
        }
    }

    #[test]
    fn border_width_style_color_longhands_resolve_per_side_with_no_border_shorthand() {
        // packet/acid1-content-box: `fixtures/css1-float-5526c.html`'s
        // `blockquote` shape -- `border-width: 1em 1.5em 2em .5em;
        // border-style: solid; border-color: black;`, no `border`/
        // `border-top` shorthand declared at all. Before this packet, none
        // of these three properties parsed (no `apply_property` arm), so
        // `styles[div].border` stayed four `BorderSide::default()`s
        // (invisible, width 0) regardless of this declaration.
        let d = dom::parser::parse("<div>x</div>");
        let sheet = parser::parse(
            "html { font-size: 10px; } \
             div { border-width: 1em 1.5em 2em .5em; border-style: solid; border-color: black; }",
        );
        let styles = cascade(&d, std::slice::from_ref(&sheet));
        let div = find(&d, "div");
        let b = styles[div].border;
        assert_eq!(b.top.width, 10.0, "top: 1em @ 10px font-size");
        assert_eq!(b.right.width, 15.0, "right: 1.5em @ 10px font-size");
        assert_eq!(b.bottom.width, 20.0, "bottom: 2em @ 10px font-size");
        assert_eq!(b.left.width, 5.0, "left: .5em @ 10px font-size");
        for side in [b.top, b.right, b.bottom, b.left] {
            assert_eq!(side.style, BorderStyle::Solid);
            assert_eq!(side.color, Color::BLACK);
        }
    }

    #[test]
    fn border_shorthand_alone_is_unaffected_by_the_new_longhand_fields() {
        // Zero-golden-churn guardrail: an element that only ever uses the
        // `border` shorthand (every OTHER fixture in this repo) must resolve
        // identically now that `resolve_border` also consults `border-
        // width`/`-style`/`-color` -- those three stay `None`/empty for it,
        // so `apply_border_side_longhands` must be a strict no-op.
        let d = dom::parser::parse("<div>x</div>");
        let sheet = parser::parse("div { border: 2px solid red; }");
        let styles = cascade(&d, std::slice::from_ref(&sheet));
        let div = find(&d, "div");
        for side in [styles[div].border.top, styles[div].border.right, styles[div].border.bottom, styles[div].border.left]
        {
            assert_eq!(side.width, 2.0);
            assert_eq!(side.style, BorderStyle::Solid);
            assert_eq!(side.color, Color::rgb(255, 0, 0));
        }
    }

    #[test]
    fn box_sizing_defaults_to_content_box_when_never_declared() {
        // CSS's real initial value -- packet/acid1-content-box.
        let d = dom::parser::parse("<div>x</div>");
        let styles = cascade(&d, &[]);
        let div = find(&d, "div");
        assert_eq!(styles[div].box_sizing, BoxSizing::ContentBox);
    }

    #[test]
    fn box_sizing_border_box_can_be_declared_via_a_universal_selector() {
        // fixtures/grid.html's own `* { box-sizing: border-box; }`.
        let d = dom::parser::parse("<div>x</div>");
        let sheet = parser::parse("* { box-sizing: border-box; }");
        let styles = cascade(&d, std::slice::from_ref(&sheet));
        let div = find(&d, "div");
        assert_eq!(styles[div].box_sizing, BoxSizing::BorderBox);
    }

    #[test]
    fn hr_ua_rule_renders_as_a_zero_height_box_with_a_solid_top_border() {
        // The UA sheet's own `hr` rule (packet/hr-rule): a thin full-width
        // rule line, not the old blank-space rendering.
        let d = dom::parser::parse("<hr>");
        let styles = cascade(&d, &[]);
        let hr = find(&d, "hr");
        assert_eq!(styles[hr].display, Display::Block);
        assert_eq!(styles[hr].height, Dimension::Px(0.0));
        let top = styles[hr].border.top;
        assert_eq!(top.style, BorderStyle::Solid);
        assert_eq!(top.width, 1.0);
        assert_eq!(top.color, Color::rgb(0x80, 0x80, 0x80));
        assert_eq!(styles[hr].border.right, BorderSide::default());
        assert_eq!(styles[hr].border.bottom, BorderSide::default());
        assert_eq!(styles[hr].border.left, BorderSide::default());
    }

    #[test]
    fn box_properties_do_not_inherit() {
        let d = dom::parser::parse(r#"<div><span>x</span></div>"#);
        let sheet = parser::parse("div { margin: 10px; }");
        let styles = cascade(&d, std::slice::from_ref(&sheet));
        let div = find(&d, "div");
        let span = find(&d, "span");
        assert_eq!(styles[div].margin.top, LengthPercentageAuto::Px(10.0));
        assert_eq!(styles[span].margin.top, LengthPercentageAuto::Px(0.0));
    }

    #[test]
    fn em_font_size_resolves_against_parent_computed_size() {
        let d = dom::parser::parse(r#"<div><span>x</span></div>"#);
        let sheet = parser::parse("div { font-size: 20px; } span { font-size: 2em; }");
        let styles = cascade(&d, std::slice::from_ref(&sheet));
        let span = find(&d, "span");
        assert_eq!(styles[span].font_size, 40.0);
    }

    /// packet/text-min-8x8 regression guard: `text::text_render_px`'s
    /// glyph-RENDER floor (never rasterize below 16px, see that function's
    /// own doc comment) must NEVER leak into `em`/`%`-box resolution here.
    /// A sub-16px `font-size` (Acid1's own shape: `html { font: 10px/1 }`,
    /// then `width`/`height`/`padding` in `em` against that 10px) must
    /// resolve every box dimension against the RAW 10px font-size, not a
    /// floored 16px one -- otherwise every `em`-sized box on a small-font
    /// page would silently blow up (e.g. this test's `20em` width would
    /// become 320px instead of 200px).
    #[test]
    fn width_height_padding_em_resolve_against_the_raw_unfloored_font_size() {
        let d = dom::parser::parse(r#"<div>x</div>"#);
        let sheet = parser::parse(
            "div { font-size: 10px; width: 20em; height: 5em; padding: 2em; margin: 1em; border-width: 0.5em; border-style: solid; }",
        );
        let styles = cascade(&d, std::slice::from_ref(&sheet));
        let div = find(&d, "div");
        assert_eq!(styles[div].font_size, 10.0, "font_size itself must never be floored");
        assert_eq!(styles[div].width, Dimension::Px(200.0), "20em * 10px, NOT 20em * 16px (320.0)");
        assert_eq!(styles[div].height, Dimension::Px(50.0), "5em * 10px, NOT 5em * 16px (80.0)");
        assert_eq!(styles[div].padding.top, LengthPercentage::Px(20.0), "2em * 10px, NOT 2em * 16px (32.0)");
        assert_eq!(styles[div].margin.top, LengthPercentageAuto::Px(10.0), "1em * 10px, NOT 1em * 16px (16.0)");
        assert_eq!(styles[div].border.top.width, 5.0, "0.5em * 10px, NOT 0.5em * 16px (8.0)");
    }

    #[test]
    fn percent_font_size_resolves_against_parent_computed_size() {
        let d = dom::parser::parse(r#"<div><span>x</span></div>"#);
        let sheet = parser::parse("div { font-size: 20px; } span { font-size: 150%; }");
        let styles = cascade(&d, std::slice::from_ref(&sheet));
        let span = find(&d, "span");
        assert_eq!(styles[span].font_size, 30.0);
    }

    #[test]
    fn font_size_inherits_when_not_set() {
        let d = dom::parser::parse(r#"<div><span>x</span></div>"#);
        let sheet = parser::parse("div { font-size: 24px; }");
        let styles = cascade(&d, std::slice::from_ref(&sheet));
        let span = find(&d, "span");
        assert_eq!(styles[span].font_size, 24.0);
    }

    #[test]
    fn ua_heading_font_size_is_relative_em_against_root() {
        let d = dom::parser::parse("<h1>x</h1>");
        let styles = cascade(&d, &[]);
        let h1 = find(&d, "h1");
        assert_eq!(styles[h1].font_size, 32.0); // UA h1 { font-size: 2em } * 16px default
    }

    #[test]
    fn bold_and_italic_ua_defaults() {
        let d = dom::parser::parse("<b>x</b><i>y</i>");
        let styles = cascade(&d, &[]);
        assert_eq!(styles[find(&d, "b")].font_weight, FontWeight::Bold);
        assert_eq!(styles[find(&d, "i")].font_style, FontStyle::Italic);
    }

    #[test]
    fn anchor_has_underline_by_default() {
        let d = dom::parser::parse(r#"<a href="x">x</a>"#);
        let styles = cascade(&d, &[]);
        assert!(styles[find(&d, "a")].text_decoration.underline);
    }

    #[test]
    fn pre_is_white_space_pre_and_monospace_by_default() {
        let d = dom::parser::parse("<pre>x</pre>");
        let styles = cascade(&d, &[]);
        let s = &styles[find(&d, "pre")];
        assert_eq!(s.white_space, WhiteSpace::Pre);
        assert_eq!(s.font_family, FontFamily::Monospace);
    }

    #[test]
    fn list_style_type_defaults_disc_and_decimal() {
        let d = dom::parser::parse("<ul><li>a</li></ul><ol><li>b</li></ol>");
        let styles = cascade(&d, &[]);
        let lists = find_all(&d, "ul");
        assert_eq!(styles[lists[0]].list_style_type, ListStyleType::Disc);
        let ols = find_all(&d, "ol");
        assert_eq!(styles[ols[0]].list_style_type, ListStyleType::Decimal);
    }

    #[test]
    fn author_sheet_overrides_ua_defaults() {
        let d = dom::parser::parse("<span>x</span>");
        let sheet = parser::parse("span { display: block; }");
        let styles = cascade(&d, std::slice::from_ref(&sheet));
        assert_eq!(styles[find(&d, "span")].display, Display::Block);
    }

    #[test]
    fn multiple_author_sheets_apply_in_order() {
        let d = dom::parser::parse("<p>x</p>");
        let sheet1 = parser::parse("p { color: red; }");
        let sheet2 = parser::parse("p { color: blue; }");
        let styles = cascade(&d, &[sheet1, sheet2]);
        assert_eq!(styles[find(&d, "p")].color, Color::rgb(0, 0, 255));
    }

    #[test]
    fn cross_sheet_specificity_wins_regardless_of_sheet_order() {
        // A higher-specificity rule in an EARLIER sheet must still beat a
        // lower-specificity rule in a LATER sheet — specificity is compared
        // globally across all author sheets, not sheet-by-sheet with the
        // last sheet always winning. Source order only breaks *ties*.
        let d = dom::parser::parse(r#"<p id="foo">x</p>"#);
        let sheet1 = parser::parse("#foo { color: red; }");
        let sheet2 = parser::parse("p { color: blue; }");
        let styles = cascade(&d, &[sheet1, sheet2]);
        assert_eq!(styles[find(&d, "p")].color, Color::rgb(255, 0, 0));
    }

    #[test]
    fn cross_sheet_specificity_also_holds_in_reverse_sheet_order() {
        let d = dom::parser::parse(r#"<p id="foo">x</p>"#);
        let sheet1 = parser::parse("p { color: blue; }");
        let sheet2 = parser::parse("#foo { color: red; }");
        let styles = cascade(&d, &[sheet1, sheet2]);
        assert_eq!(styles[find(&d, "p")].color, Color::rgb(255, 0, 0));
    }

    #[test]
    fn line_height_percent_resolves_against_font_size() {
        let d = dom::parser::parse("<p>x</p>");
        let sheet = parser::parse("p { font-size: 20px; line-height: 150%; }");
        let styles = cascade(&d, std::slice::from_ref(&sheet));
        assert_eq!(styles[find(&d, "p")].line_height, LineHeight::Px(30.0));
    }

    // ---- M5: inline `style="..."` — highest-precedence origin ----------

    #[test]
    fn inline_style_applies_with_no_author_sheet() {
        let d = dom::parser::parse(r#"<p style="color: blue">x</p>"#);
        let styles = cascade(&d, &[]);
        assert_eq!(styles[find(&d, "p")].color, Color::rgb(0, 0, 255));
    }

    #[test]
    fn inline_style_beats_author_sheet() {
        let d = dom::parser::parse(r#"<p style="color: blue">x</p>"#);
        let sheet = parser::parse("p { color: red; }");
        let styles = cascade(&d, std::slice::from_ref(&sheet));
        assert_eq!(styles[find(&d, "p")].color, Color::rgb(0, 0, 255));
    }

    #[test]
    fn inline_style_beats_ua_author_and_everything_else_end_to_end() {
        // `a` gets `color: blue` from the UA sheet itself; an author sheet
        // overrides it to red; an inline style must win over BOTH and
        // resolve to green — UA < author `<style>` < inline `style=`.
        let d = dom::parser::parse(r#"<a href="x" style="color: green">link</a>"#);
        let sheet = parser::parse("a { color: red; }");
        let styles = cascade(&d, std::slice::from_ref(&sheet));
        assert_eq!(styles[find(&d, "a")].color, Color::rgb(0, 128, 0));
    }

    #[test]
    fn malformed_inline_style_is_ignored_other_properties_still_apply_no_panic() {
        // A trailing empty value (`color:`) is a bad declaration; the well-
        // formed `font-size` declaration right after it must still apply,
        // and the whole thing must not panic (parser::parse_declaration_block
        // is already total; this just confirms cascade's inline wiring
        // doesn't add a panic).
        let d = dom::parser::parse(r#"<p style="color: ; font-size: 20px">x</p>"#);
        let styles = cascade(&d, &[]);
        let p = &styles[find(&d, "p")];
        assert_eq!(p.font_size, 20.0);
        assert_eq!(p.color, Color::BLACK); // untouched: falls back to CSS initial
    }

    #[test]
    fn garbage_inline_style_does_not_panic() {
        let d = dom::parser::parse(r#"<p style="!!!not css at all!!!">x</p>"#);
        let styles = cascade(&d, &[]);
        assert_eq!(styles.len(), d.len());
    }

    #[test]
    fn huge_pathological_inline_style_does_not_panic() {
        // Thousands of bogus + a few valid declarations crammed into one
        // `style=` attribute value; must still resolve the valid ones and
        // never panic.
        let mut style = String::new();
        for i in 0..5000 {
            style.push_str(&format!("bogus-{i}: nonsense;"));
        }
        style.push_str("color: green;");
        let html = format!(r#"<p style="{style}">x</p>"#);
        let d = dom::parser::parse(&html);
        let styles = cascade(&d, &[]);
        assert_eq!(styles[find(&d, "p")].color, Color::rgb(0, 128, 0));
    }

    #[test]
    fn no_style_attribute_is_a_total_no_op() {
        let d = dom::parser::parse("<p>x</p>");
        let styles = cascade(&d, &[]);
        assert_eq!(styles[find(&d, "p")].color, Color::BLACK);
    }

    #[test]
    fn cascade_does_not_panic_on_empty_dom_and_no_sheets() {
        let d = dom::Dom::new();
        let styles = cascade(&d, &[]);
        assert_eq!(styles.len(), d.len());
    }

    #[test]
    fn cascade_output_len_matches_dom_len() {
        let d = dom::parser::parse("<p>hello <b>world</b></p>");
        let styles = cascade(&d, &[]);
        assert_eq!(styles.len(), d.len());
    }

    /// Hostile/generated input (quote threads, WYSIWYG exports, nested-table
    /// markup) can nest thousands of `<div>`s deep. `cascade`'s internal
    /// `visit` walk used plain Rust-call recursion with no depth cap and
    /// reliably SIGABRTs the process on input like this (confirmed
    /// empirically before the fix — see the recursion-hardening packet
    /// report). `cascade` must be TOTAL: it must return a `Vec<ComputedStyle>`
    /// for any nesting depth, never blow the call stack.
    fn nested_div_html(depth: usize) -> String {
        let mut s = String::with_capacity(depth * 11 + 16);
        for _ in 0..depth {
            s.push_str("<div>");
        }
        s.push('x');
        for _ in 0..depth {
            s.push_str("</div>");
        }
        s
    }

    #[test]
    fn cascade_is_total_on_dom_nested_3000_deep() {
        let html = nested_div_html(3000);
        let d = dom::parser::parse(&html);
        let styles = cascade(&d, &[]);
        assert_eq!(styles.len(), d.len());
    }

    #[test]
    fn cascade_is_total_on_dom_nested_5000_deep() {
        let html = nested_div_html(5000);
        let d = dom::parser::parse(&html);
        let styles = cascade(&d, &[]);
        assert_eq!(styles.len(), d.len());
    }

    /// Totality alone isn't enough: `cascade` must stay fully CORRECT at any
    /// depth too (per the packet's preference for an iterative rewrite over a
    /// cap-and-degrade fallback). Inherit a property from the outermost `div`
    /// and confirm it still reaches a node thousands of levels down, past any
    /// depth a naive cap (e.g. reusing layout's DEPTH_CAP=100) would have
    /// stopped resolving real inheritance for.
    #[test]
    fn cascade_inherits_correctly_past_any_naive_depth_cap() {
        let html = nested_div_html(3000);
        let d = dom::parser::parse(&html);
        let sheet = parser::parse("div { color: green; }");
        let styles = cascade(&d, std::slice::from_ref(&sheet));
        // Deepest element is the innermost <div> — walk the arena's NodeIds
        // iteratively (no recursion in the test either) to find it: the
        // parser allocates root=0, then each opened <div> in nesting order,
        // so the innermost element is the one whose single child is the text
        // node "x".
        let mut deepest = d.root();
        loop {
            let el = d.node(deepest).element().expect("element");
            let child = el.children[0];
            if d.node(child).text().is_some() {
                break;
            }
            deepest = child;
        }
        assert_eq!(styles[deepest].color, Color::rgb(0, 128, 0));
    }

    // ---- packet/presentational-attrs: presentational-hint cascade tier ----
    //
    // Vintage HTML presentational attributes (`<font color/size>`, `bgcolor`,
    // `align`, `<body text>`) must fold in as a real cascade tier -- UA <
    // presentational hints < author CSS < inline `style=` -- so they
    // participate in inheritance/override correctly instead of being patched
    // onto `ComputedStyle` after the cascade has already run.

    #[test]
    fn font_color_attribute_sets_computed_color() {
        let d = dom::parser::parse(r#"<font color="red">text</font>"#);
        let styles = cascade(&d, &[]);
        assert_eq!(styles[find(&d, "font")].color, Color::rgb(255, 0, 0));
    }

    #[test]
    fn presentational_hint_overrides_inherited_color_but_plain_sibling_still_inherits() {
        // The key inheritance-correctness case the cascade-tier approach
        // fixes vs. post-cascade mutation: the <font>'s own hint must beat
        // the inherited blue, while a plain <span> sibling with no hint of
        // its own still inherits blue from the div normally.
        let d = dom::parser::parse(r#"<div style="color:blue"><font color="red">x</font><span>y</span></div>"#);
        let styles = cascade(&d, &[]);
        assert_eq!(styles[find(&d, "font")].color, Color::rgb(255, 0, 0), "hint beats inherited color");
        assert_eq!(styles[find(&d, "span")].color, Color::rgb(0, 0, 255), "plain sibling still inherits from the div");
    }

    #[test]
    fn author_css_beats_presentational_hint() {
        let d = dom::parser::parse(r#"<font color="red">x</font>"#);
        let sheet = parser::parse("font { color: green; }");
        let styles = cascade(&d, std::slice::from_ref(&sheet));
        assert_eq!(styles[find(&d, "font")].color, Color::rgb(0, 128, 0));
    }

    #[test]
    fn inline_style_beats_presentational_hint_too() {
        let d = dom::parser::parse(r#"<font color="red" style="color:green">x</font>"#);
        let styles = cascade(&d, &[]);
        assert_eq!(styles[find(&d, "font")].color, Color::rgb(0, 128, 0));
    }

    #[test]
    fn bgcolor_variants_agree_and_do_not_inherit() {
        for (val, expect) in [
            ("#ffcc00", Color::rgb(0xff, 0xcc, 0x00)),
            ("ffcc00", Color::rgb(0xff, 0xcc, 0x00)),
            ("yellow", Color::rgb(255, 255, 0)),
        ] {
            let html = format!(r#"<div bgcolor="{val}"><span>x</span></div>"#);
            let d = dom::parser::parse(&html);
            let styles = cascade(&d, &[]);
            assert_eq!(styles[find(&d, "div")].background_color, expect, "bgcolor={val}");
            assert_eq!(styles[find(&d, "span")].background_color, Color::TRANSPARENT, "bgcolor must not inherit");
        }
    }

    #[test]
    fn font_size_attribute_scale_absolute_and_relative() {
        let cases = [("1", 10.0_f32), ("7", 48.0), ("+1", 18.0), ("-1", 13.0)];
        for (attr, px) in cases {
            let html = format!(r#"<font size="{attr}">a</font>"#);
            let d = dom::parser::parse(&html);
            let styles = cascade(&d, &[]);
            assert_eq!(styles[find(&d, "font")].font_size, px, "size={attr}");
        }
    }

    #[test]
    fn align_center_sets_text_align_on_block_elements() {
        let d = dom::parser::parse(r#"<p align="center">x</p>"#);
        let styles = cascade(&d, &[]);
        assert_eq!(styles[find(&d, "p")].text_align, TextAlign::Center);

        let d = dom::parser::parse(r#"<div align="center">x</div>"#);
        let styles = cascade(&d, &[]);
        assert_eq!(styles[find(&d, "div")].text_align, TextAlign::Center);

        let d = dom::parser::parse(r#"<table><tr><td align="center">x</td></tr></table>"#);
        let styles = cascade(&d, &[]);
        assert_eq!(styles[find(&d, "td")].text_align, TextAlign::Center);
    }

    #[test]
    fn img_align_does_not_set_text_align_stays_left_default() {
        // <img align> stays float-only (box_tree::apply_align_float_hint,
        // unchanged by this packet); the presentational-hint tier must not
        // ALSO set text-align for <img>.
        let d = dom::parser::parse(r#"<img src="x.png" align="left">"#);
        let styles = cascade(&d, &[]);
        assert_eq!(styles[find(&d, "img")].text_align, TextAlign::Left);
    }

    #[test]
    fn body_text_attribute_sets_color_and_inherits_to_descendants() {
        let d = dom::parser::parse(r##"<body text="#333333"><p>x</p></body>"##);
        let styles = cascade(&d, &[]);
        assert_eq!(styles[find(&d, "body")].color, Color::rgb(0x33, 0x33, 0x33));
        assert_eq!(styles[find(&d, "p")].color, Color::rgb(0x33, 0x33, 0x33));
    }

    // ---- packet/table-spacing: `border-spacing` / `cellspacing="N"` ----

    #[test]
    fn table_with_no_cellspacing_or_css_keeps_the_default_border_spacing() {
        let d = dom::parser::parse("<table><tr><td>x</td></tr></table>");
        let styles = cascade(&d, &[]);
        let table = &styles[find(&d, "table")];
        assert_eq!(table.border_spacing_x, 8.0, "default preserved: no golden churn");
        assert_eq!(table.border_spacing_y, 0.0, "default preserved: no golden churn");
    }

    #[test]
    fn css_border_spacing_property_resolves_onto_the_table() {
        let d = dom::parser::parse("<table><tr><td>x</td></tr></table>");
        let sheet = parser::parse("table { border-spacing: 4px 2px; }");
        let styles = cascade(&d, std::slice::from_ref(&sheet));
        let table = &styles[find(&d, "table")];
        assert_eq!(table.border_spacing_x, 4.0);
        assert_eq!(table.border_spacing_y, 2.0);
    }

    #[test]
    fn table_cellspacing_attribute_resolves_onto_the_table_style() {
        let d = dom::parser::parse(r#"<table cellspacing="6"><tr><td>x</td></tr></table>"#);
        let styles = cascade(&d, &[]);
        let table = &styles[find(&d, "table")];
        assert_eq!(table.border_spacing_x, 6.0);
        assert_eq!(table.border_spacing_y, 6.0);
    }

    #[test]
    fn css_border_spacing_beats_cellspacing_attribute() {
        let d = dom::parser::parse(r#"<table cellspacing="6"><tr><td>x</td></tr></table>"#);
        let sheet = parser::parse("table { border-spacing: 12px; }");
        let styles = cascade(&d, std::slice::from_ref(&sheet));
        let table = &styles[find(&d, "table")];
        assert_eq!(table.border_spacing_x, 12.0, "author CSS wins over the presentational hint");
        assert_eq!(table.border_spacing_y, 12.0);
    }

    // ---- packet/border-collapse: `border-collapse` / `<table border>` ----

    #[test]
    fn plain_table_defaults_to_separate() {
        let d = dom::parser::parse("<table><tr><td>x</td></tr></table>");
        let styles = cascade(&d, &[]);
        assert_eq!(styles[find(&d, "table")].border_collapse, BorderCollapse::Separate);
    }

    #[test]
    fn table_border_attribute_alone_resolves_to_collapse() {
        let d = dom::parser::parse(r#"<table border="1"><tr><td>x</td></tr></table>"#);
        let styles = cascade(&d, &[]);
        assert_eq!(styles[find(&d, "table")].border_collapse, BorderCollapse::Collapse);
    }

    #[test]
    fn table_border_with_cellspacing_stays_separate() {
        let d = dom::parser::parse(r#"<table border="1" cellspacing="4"><tr><td>x</td></tr></table>"#);
        let styles = cascade(&d, &[]);
        assert_eq!(styles[find(&d, "table")].border_collapse, BorderCollapse::Separate);
    }

    #[test]
    fn author_css_border_collapse_property_resolves_onto_the_table() {
        let d = dom::parser::parse("<table><tr><td>x</td></tr></table>");
        let sheet = parser::parse("table { border-collapse: collapse; }");
        let styles = cascade(&d, std::slice::from_ref(&sheet));
        assert_eq!(styles[find(&d, "table")].border_collapse, BorderCollapse::Collapse);
    }

    #[test]
    fn author_css_separate_overrides_the_table_border_presentational_hint() {
        let d = dom::parser::parse(r#"<table border="1"><tr><td>x</td></tr></table>"#);
        let sheet = parser::parse("table { border-collapse: separate; }");
        let styles = cascade(&d, std::slice::from_ref(&sheet));
        assert_eq!(
            styles[find(&d, "table")].border_collapse,
            BorderCollapse::Separate,
            "author CSS wins over the presentational collapse hint"
        );
    }

    #[test]
    fn presentational_hints_do_not_disturb_pre_existing_cascade_behavior() {
        // A belt-and-suspenders re-check (alongside every earlier test in
        // this module still passing unchanged) that adding the new tier
        // didn't disturb ordinary author-vs-UA precedence for an element with
        // no presentational attributes at all.
        let d = dom::parser::parse("<span>x</span>");
        let sheet = parser::parse("span { display: block; }");
        let styles = cascade(&d, std::slice::from_ref(&sheet));
        assert_eq!(styles[find(&d, "span")].display, Display::Block);
    }

    // ---- packet T1a: CSS custom properties + var() resolution ----

    #[test]
    fn root_custom_property_resolves_via_var_on_a_descendant() {
        // `:root` matches `dom.root()`, which is ALWAYS the literal `<html>`
        // element (see `dom::ast::Dom::new`'s own doc comment) -- this is the
        // packet's motivating shape: a theme variable declared once at the
        // top and consumed several levels down.
        let d = dom::parser::parse("<html><body>x</body></html>");
        let sheet = parser::parse(":root { --ink: #123456; } body { color: var(--ink); }");
        let styles = cascade(&d, std::slice::from_ref(&sheet));
        let body = find(&d, "body");
        assert_eq!(styles[body].color, Color::rgb(0x12, 0x34, 0x56));
    }

    #[test]
    fn custom_property_inherits_through_intermediate_elements_with_no_custom_properties_of_their_own() {
        // `--c` is declared on the `<div>`; `<section>`/`<article>` in
        // between set no custom properties of their own, yet `var(--c)`
        // still resolves several levels down on `<span>` -- `Env::child`
        // threading the inherited chain down `cascade::visit`'s walk.
        let d = dom::parser::parse("<div><section><article><span>x</span></article></section></div>");
        let sheet = parser::parse("div { --c: teal; } span { color: var(--c); }");
        let styles = cascade(&d, std::slice::from_ref(&sheet));
        let span = find(&d, "span");
        assert_eq!(styles[span].color, Color::rgb(0, 128, 128));
    }

    #[test]
    fn var_fallback_is_used_when_the_custom_property_is_never_declared_anywhere() {
        let d = dom::parser::parse("<p>x</p>");
        let sheet = parser::parse("p { color: var(--missing, green); }");
        let styles = cascade(&d, std::slice::from_ref(&sheet));
        let p = find(&d, "p");
        assert_eq!(styles[p].color, Color::rgb(0, 128, 0));
    }

    #[test]
    fn var_with_no_fallback_and_undefined_custom_property_falls_back_to_css_initial() {
        // "Invalid at computed-value time" (CSS's own term): the whole
        // `color` declaration simply doesn't apply -- `color` falls back to
        // whatever it would resolve to with NO declaration at all (here:
        // the CSS initial `Color::BLACK`, there being no other color source
        // in this document), never a crash and never a garbage value.
        let d = dom::parser::parse("<p>x</p>");
        let sheet = parser::parse("p { color: var(--missing); }");
        let styles = cascade(&d, std::slice::from_ref(&sheet));
        let p = find(&d, "p");
        assert_eq!(styles[p].color, Color::BLACK);
    }

    #[test]
    fn var_chain_resolves_through_another_custom_property() {
        // `--a` doesn't hold a color directly -- it holds a reference to
        // `--b` -- so resolving `color: var(--a)` must recurse one level
        // through `--b`'s own raw value before it bottoms out at `blue`.
        let d = dom::parser::parse("<p>x</p>");
        let sheet = parser::parse("p { --a: var(--b); --b: blue; color: var(--a); }");
        let styles = cascade(&d, std::slice::from_ref(&sheet));
        let p = find(&d, "p");
        assert_eq!(styles[p].color, Color::rgb(0, 0, 255));
    }

    #[test]
    fn var_cycles_are_invalid_and_the_cascade_never_hangs() {
        // Two-step cycle: `--a` and `--b` reference each other, so neither
        // ever bottoms out at a real value -- `substitute_vars`'s `visiting`
        // chain catches this the instant it closes, well before
        // `VAR_SUBSTITUTION_DEPTH_CAP` would even matter. This test's very
        // existence and return (not a hang) is itself part of the totality
        // proof -- see also the dedicated pathological test below.
        let d = dom::parser::parse("<p>x</p>");
        let sheet = parser::parse("p { --a: var(--b); --b: var(--a); color: var(--a); }");
        let styles = cascade(&d, std::slice::from_ref(&sheet));
        let p = find(&d, "p");
        assert_eq!(styles[p].color, Color::BLACK, "a two-step cycle must fall back to CSS initial, never hang");

        // Direct self-reference: `--a` references itself.
        let d = dom::parser::parse("<p>x</p>");
        let sheet = parser::parse("p { --a: var(--a); color: var(--a); }");
        let styles = cascade(&d, std::slice::from_ref(&sheet));
        let p = find(&d, "p");
        assert_eq!(styles[p].color, Color::BLACK, "a direct self-cycle must fall back to CSS initial, never hang");
    }

    #[test]
    fn attribute_scoped_custom_property_override_gates_on_the_attribute() {
        // Packet-motivating theming shape: `:root` sets a light-mode
        // default, `html[data-theme="dark"]` overrides it (higher
        // specificity: an attribute selector counts like a class, so it
        // beats the bare `:root` compound), and `body` consumes the
        // resulting value via `var()`. Testing both the attribute's PRESENCE
        // and its ABSENCE confirms the attribute selector is actually gating
        // the override, not just always matching regardless of the DOM.
        let sheet = parser::parse(
            r#":root { --bg: #fbfcfb; } html[data-theme="dark"] { --bg: #0f1211; } body { background-color: var(--bg); }"#,
        );

        let dark = dom::parser::parse(r#"<html data-theme="dark"><body>x</body></html>"#);
        let styles = cascade(&dark, std::slice::from_ref(&sheet));
        let body = find(&dark, "body");
        assert_eq!(styles[body].background_color, Color::rgb(0x0f, 0x12, 0x11));

        let light = dom::parser::parse("<html><body>x</body></html>");
        let styles = cascade(&light, std::slice::from_ref(&sheet));
        let body = find(&light, "body");
        assert_eq!(
            styles[body].background_color,
            Color::rgb(0xfb, 0xfc, 0xfb),
            "without the data-theme attribute the dark override must not apply"
        );
    }

    #[test]
    fn pathological_custom_property_chain_and_unused_cycles_do_not_panic_or_hang() {
        // A long but strictly ACYCLIC chain of custom properties, each
        // referencing the next, plus one real usage. `VAR_SUBSTITUTION_
        // DEPTH_CAP` means this specific chain resolves to the fallback/
        // initial rather than `red` (it's far longer than the cap) -- this
        // only asserts TOTALITY (the call completes, every node still gets a
        // style), never a specific color.
        let mut css = String::from("p { ");
        let depth = 3000;
        for i in 0..depth {
            css.push_str(&format!("--v{i}: var(--v{next});", next = i + 1));
        }
        css.push_str(&format!("--v{depth}: red; color: var(--v0); }}"));
        let d = dom::parser::parse("<p>x</p>");
        let sheet = parser::parse(&css);
        let styles = cascade(&d, std::slice::from_ref(&sheet));
        assert_eq!(styles.len(), d.len());

        // Thousands of self-referential custom properties, declared but
        // never even consumed by any real property -- confirms parsing and
        // cascading a stylesheet like this alone doesn't hang, independent
        // of whether anything ever actually resolves.
        let mut css2 = String::from(":root { ");
        for i in 0..depth {
            css2.push_str(&format!("--u{i}: var(--u{i});"));
        }
        css2.push_str(" }");
        let d2 = dom::parser::parse("<p>x</p>");
        let sheet2 = parser::parse(&css2);
        let styles2 = cascade(&d2, std::slice::from_ref(&sheet2));
        assert_eq!(styles2.len(), d2.len());
    }
}
