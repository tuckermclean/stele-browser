//! The cascade (P2, Wave 1): fold the user-agent stylesheet and author sheets
//! onto the DOM to produce one [`ComputedStyle`] per node. The UA sheet is where
//! element semantics live (block vs inline defaults, replaced elements, form
//! controls) — `dom::ast` deliberately stays name-agnostic.

use std::sync::OnceLock;

use crate::dom::{Dom, Node, NodeId};
use crate::style::computed::{
    BorderSide, BorderStyle, Dimension, Edges, LengthPercentage, LengthPercentageAuto, LineHeight,
};
use crate::style::selector::{ElementInfo, Specificity};
use crate::style::ua::UA_CSS;
use crate::style::value::{BorderRaw, Declarations, RawLength, RawLengthAuto, RawLineHeight};
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
enum Frame {
    Enter { id: NodeId, parent: Option<ComputedStyle> },
    Exit,
}

/// Explicit-stack equivalent of the old recursive `visit`: identical
/// semantics (same `ancestors` chain visible to each element when its rules
/// are matched, same parent-style propagation to children, same "text takes
/// the parent's style wholesale" rule), just driven by a `Vec<Frame>` on the
/// heap instead of the native call stack, so nesting depth cannot blow it.
fn visit(dom: &Dom, root: NodeId, ua: &Stylesheet, author: &[Stylesheet], out: &mut [ComputedStyle]) {
    let mut ancestors: Vec<ElementInfo> = Vec::new();
    let mut stack: Vec<Frame> = vec![Frame::Enter { id: root, parent: None }];

    while let Some(frame) = stack.pop() {
        match frame {
            Frame::Exit => {
                ancestors.pop();
            }
            Frame::Enter { id, parent } => match dom.node(id) {
                // Character data carries no rules of its own; it takes the
                // parent's computed style wholesale (the inline engine reads
                // font/color etc. straight off it).
                Node::Text(_) => {
                    if let Some(p) = &parent {
                        out[id] = p.clone();
                    }
                }
                Node::Element(el) => {
                    let info = ElementInfo::from_element(&el.name, &el.attrs);
                    // RED (test-first): inline `style="..."` is not folded
                    // in yet -- see the immediately-following GREEN commit.
                    let decls = fold_matching_declarations(ua, author, &ancestors, &info);
                    let style = resolve(&decls, parent.as_ref());
                    out[id] = style.clone();

                    ancestors.push(info);
                    stack.push(Frame::Exit);
                    // Push in reverse so children are popped (and thus
                    // visited) in source order, matching the old recursive
                    // walk's iteration order exactly.
                    for &child in el.children.iter().rev() {
                        stack.push(Frame::Enter { id: child, parent: Some(style.clone()) });
                    }
                }
            },
        }
    }
}

/// Merge every UA + author rule that matches this element and fold their
/// declarations onto one accumulator, in cascade precedence order: sorted
/// globally by `(origin, specificity, source order)`, applied low-to-high so
/// the highest-precedence match's fields win (`Declarations::overlay`).
///
/// Author sheets always outrank the UA sheet regardless of specificity (real
/// CSS origin ordering — we don't support `!important`, so normal-weight
/// author always beats normal-weight UA). *Within* the author origin, every
/// matching rule from *every* author sheet is compared by specificity
/// together, with sheet index (then in-sheet source order) only breaking
/// exact specificity ties — a later sheet must not automatically beat an
/// earlier one on lower specificity (that was the bug: folding one sheet at
/// a time made specificity only work within a single sheet).
fn fold_matching_declarations(ua: &Stylesheet, author: &[Stylesheet], ancestors: &[ElementInfo], info: &ElementInfo) -> Declarations {
    // (is_author_origin, specificity, sheet_index, in-sheet source order, declarations)
    let mut candidates: Vec<(bool, Specificity, usize, u32, &Declarations)> = Vec::new();

    for r in parser::matching_rules(ua, ancestors, info) {
        candidates.push((false, r.selector.specificity(), 0, r.order, &r.declarations));
    }
    for (sheet_index, sheet) in author.iter().enumerate() {
        for r in parser::matching_rules(sheet, ancestors, info) {
            candidates.push((true, r.selector.specificity(), sheet_index, r.order, &r.declarations));
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
fn resolve(d: &Declarations, parent: Option<&ComputedStyle>) -> ComputedStyle {
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
        border: resolve_border(d.border, font_size),
        float: own!(float),
        clear: own!(clear),

        flex_direction: own!(flex_direction),
        flex_wrap: own!(flex_wrap),
        justify_content: own!(justify_content),
        align_items: own!(align_items),
        align_self: own!(align_self),
        flex_grow: own!(flex_grow),
        flex_shrink: own!(flex_shrink),
        flex_basis: resolve_dimension(d.flex_basis, font_size, default.flex_basis),
        gap: d.gap.map(|l| raw_to_px(l, font_size)).unwrap_or(default.gap),
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
fn resolve_border(v: Option<BorderRaw>, font_size: f32) -> Edges<BorderSide> {
    match v {
        None => Edges::all(BorderSide::default()),
        Some(b) => {
            let style = b.style.unwrap_or(BorderStyle::None);
            let width = if style == BorderStyle::Solid {
                b.width.map(|w| raw_to_px(w, font_size)).unwrap_or(3.0)
            } else {
                0.0
            };
            let color = b.color.unwrap_or(Color::BLACK);
            Edges::all(BorderSide { width, style, color })
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
    fn child_inherits_color_and_font_family_from_parent() {
        let d = dom::parser::parse(r#"<div><span>x</span></div>"#);
        let sheet = parser::parse("div { color: green; font-family: monospace; }");
        let styles = cascade(&d, std::slice::from_ref(&sheet));
        let span = find(&d, "span");
        assert_eq!(styles[span].color, Color::rgb(0, 128, 0));
        assert_eq!(styles[span].font_family, FontFamily::Monospace);
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
}
