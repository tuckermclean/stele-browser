//! The cascade (P2, Wave 1): fold the user-agent stylesheet and author sheets
//! onto the DOM to produce one [`ComputedStyle`] per node. The UA sheet is where
//! element semantics live (block vs inline defaults, replaced elements, form
//! controls) — `dom::ast` deliberately stays name-agnostic.

use crate::dom::Dom;
use crate::style::{ComputedStyle, Stylesheet};

/// Compute a style for every node in `dom`, indexed by `NodeId`. Author sheets
/// apply after the UA sheet, in source order (specificity + order resolved by
/// P2). This is contract-testable and gets strict test-first treatment.
pub fn cascade(_dom: &Dom, _author_sheets: &[Stylesheet]) -> Vec<ComputedStyle> {
    todo!("P2: UA sheet + author cascade -> per-node ComputedStyle")
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
}
