//! CSS parsing (P2, Wave 1). Full syntax is parsed; unknown declarations are
//! counted and dropped (the IGNORE-UNKNOWN treaty, charter C2). Selectors in
//! scope: element, `.class`, `#id`, descendant, grouping, `a:link`/`:visited`
//! (brief §4).

use crate::style::selector::Selector;
use crate::style::value::Declarations;

/// One rule after comma-grouping has been expanded: a single selector, its
/// source-order index (for cascade tie-breaking), and the declarations it
/// carries. `Stylesheet` owns a flat `Vec` of these — see the module doc for
/// why the shape here is P2's to define.
#[derive(Debug, Clone)]
pub(crate) struct StyleRule {
    pub selector: Selector,
    pub order: u32,
    pub declarations: Declarations,
}

/// A parsed stylesheet — rules plus the count of declarations parsed-then-
/// ignored (feeds the future Provenance pane / `--stats`). P2 fills this in.
#[derive(Debug, Clone, Default)]
pub struct Stylesheet {
    /// Declarations parsed successfully but outside the curated set, or
    /// syntactically broken beyond recovery (brief §10 error recovery).
    pub ignored_declarations: u32,
    /// `@media` blocks: parsed syntactically (their nested rules are fully
    /// tokenized so a malformed one can't wedge the parser) but never
    /// evaluated or applied — that needs a viewport the frozen `parse`/
    /// `cascade` signatures don't carry (scoped to M5).
    /// TODO(M5): evaluate @media against surface size instead of discarding.
    pub media_at_rules: u32,
    /// Any other at-rule (`@import`, `@font-face`, `@keyframes`, …): parsed
    /// syntactically and discarded — none are in the curated dialect (§4).
    pub ignored_at_rules: u32,
    pub(crate) rules: Vec<StyleRule>,
}

/// Parse a stylesheet. One-shot media queries are evaluated against the surface
/// size at load (brief §4); that evaluation is P2's remit.
pub fn parse(_css: &str) -> Stylesheet {
    todo!("P2: CSS tokenizer + parser (full syntax, curated semantics)")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::computed::*;
    use crate::surface::Color;

    fn find(dom: &crate::dom::Dom, tag: &str) -> Option<crate::dom::NodeId> {
        find_all(dom, tag).into_iter().next()
    }

    fn find_all(dom: &crate::dom::Dom, tag: &str) -> Vec<crate::dom::NodeId> {
        let mut out = Vec::new();
        fn walk(dom: &crate::dom::Dom, id: crate::dom::NodeId, tag: &str, out: &mut Vec<crate::dom::NodeId>) {
            if let Some(el) = dom.node(id).element() {
                if el.name.as_str() == tag {
                    out.push(id);
                }
                for &c in &el.children {
                    walk(dom, c, tag, out);
                }
            }
        }
        walk(dom, dom.root(), tag, &mut out);
        out
    }

    #[test]
    fn parsing_a_clean_rule_ignores_nothing() {
        let sheet = parse("p { color: red; }");
        assert_eq!(sheet.ignored_declarations, 0);
        assert_eq!(sheet.rules.len(), 1);
    }

    #[test]
    fn element_selector_matches() {
        let dom = crate::dom::parser::parse("<p>hi</p>");
        let sheet = parse("p { color: red; }");
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let p = find(&dom, "p").unwrap();
        assert_eq!(styles[p].color, Color::rgb(255, 0, 0));
    }

    #[test]
    fn class_selector_matches() {
        let dom = crate::dom::parser::parse(r#"<p class="a">hi</p>"#);
        let sheet = parse(".a { color: red; }");
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let p = find(&dom, "p").unwrap();
        assert_eq!(styles[p].color, Color::rgb(255, 0, 0));
    }

    #[test]
    fn id_selector_matches() {
        let dom = crate::dom::parser::parse(r#"<p id="x">hi</p>"#);
        let sheet = parse("#x { color: red; }");
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let p = find(&dom, "p").unwrap();
        assert_eq!(styles[p].color, Color::rgb(255, 0, 0));
    }

    #[test]
    fn descendant_selector_matches_only_nested() {
        let dom = crate::dom::parser::parse(r#"<div><p>in</p></div><p>out</p>"#);
        let sheet = parse("div p { color: red; }");
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let ps = find_all(&dom, "p");
        assert_eq!(ps.len(), 2);
        assert_eq!(styles[ps[0]].color, Color::rgb(255, 0, 0));
        assert_eq!(styles[ps[1]].color, Color::BLACK);
    }

    #[test]
    fn grouped_selectors_both_match() {
        let dom = crate::dom::parser::parse("<p>a</p><span>b</span>");
        let sheet = parse("p, span { color: red; }");
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let p = find(&dom, "p").unwrap();
        let span = find(&dom, "span").unwrap();
        assert_eq!(styles[p].color, Color::rgb(255, 0, 0));
        assert_eq!(styles[span].color, Color::rgb(255, 0, 0));
    }

    #[test]
    fn pseudo_link_matches_anchor_with_href_only() {
        let dom = crate::dom::parser::parse(r#"<a href="x">l</a><a>nohref</a>"#);
        let sheet = parse("a:link { color: red; }");
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let anchors = find_all(&dom, "a");
        assert_eq!(styles[anchors[0]].color, Color::rgb(255, 0, 0));
        assert_ne!(styles[anchors[1]].color, Color::rgb(255, 0, 0));
    }

    #[test]
    fn pseudo_visited_never_matches_without_history() {
        let dom = crate::dom::parser::parse(r#"<a href="x">l</a>"#);
        let sheet = parse("a:visited { color: red; }");
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let a = find(&dom, "a").unwrap();
        assert_ne!(styles[a].color, Color::rgb(255, 0, 0));
    }

    #[test]
    fn specificity_id_beats_class() {
        let dom = crate::dom::parser::parse(r#"<p id="x" class="a">t</p>"#);
        let sheet = parse("#x { color: red; } .a { color: blue; }");
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let p = find(&dom, "p").unwrap();
        assert_eq!(styles[p].color, Color::rgb(255, 0, 0));
    }

    #[test]
    fn specificity_class_beats_element() {
        let dom = crate::dom::parser::parse(r#"<p class="a">t</p>"#);
        let sheet = parse("p { color: blue; } .a { color: red; }");
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let p = find(&dom, "p").unwrap();
        assert_eq!(styles[p].color, Color::rgb(255, 0, 0));
    }

    #[test]
    fn later_source_order_wins_specificity_ties() {
        let dom = crate::dom::parser::parse("<p>t</p>");
        let sheet = parse("p { color: red; } p { color: blue; }");
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let p = find(&dom, "p").unwrap();
        assert_eq!(styles[p].color, Color::rgb(0, 0, 255));
    }

    #[test]
    fn margin_shorthand_one_value_applies_to_all_edges() {
        let dom = crate::dom::parser::parse("<p>t</p>");
        let sheet = parse("p { margin: 5px; }");
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let p = find(&dom, "p").unwrap();
        assert_eq!(styles[p].margin.top, LengthPercentageAuto::Px(5.0));
        assert_eq!(styles[p].margin.right, LengthPercentageAuto::Px(5.0));
        assert_eq!(styles[p].margin.bottom, LengthPercentageAuto::Px(5.0));
        assert_eq!(styles[p].margin.left, LengthPercentageAuto::Px(5.0));
    }

    #[test]
    fn margin_shorthand_four_values_map_top_right_bottom_left() {
        let dom = crate::dom::parser::parse("<p>t</p>");
        let sheet = parse("p { margin: 1px 2px 3px 4px; }");
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let p = find(&dom, "p").unwrap();
        assert_eq!(styles[p].margin.top, LengthPercentageAuto::Px(1.0));
        assert_eq!(styles[p].margin.right, LengthPercentageAuto::Px(2.0));
        assert_eq!(styles[p].margin.bottom, LengthPercentageAuto::Px(3.0));
        assert_eq!(styles[p].margin.left, LengthPercentageAuto::Px(4.0));
    }

    #[test]
    fn padding_shorthand_two_values_map_vertical_horizontal() {
        let dom = crate::dom::parser::parse("<p>t</p>");
        let sheet = parse("p { padding: 2px 6px; }");
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let p = find(&dom, "p").unwrap();
        assert_eq!(styles[p].padding.top, LengthPercentage::Px(2.0));
        assert_eq!(styles[p].padding.bottom, LengthPercentage::Px(2.0));
        assert_eq!(styles[p].padding.right, LengthPercentage::Px(6.0));
        assert_eq!(styles[p].padding.left, LengthPercentage::Px(6.0));
    }

    #[test]
    fn border_shorthand_solid_only() {
        let dom = crate::dom::parser::parse("<p>t</p>");
        let sheet = parse("p { border: 2px solid red; }");
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let p = find(&dom, "p").unwrap();
        assert_eq!(styles[p].border.top.style, BorderStyle::Solid);
        assert_eq!(styles[p].border.top.width, 2.0);
        assert_eq!(styles[p].border.top.color, Color::rgb(255, 0, 0));
    }

    #[test]
    fn border_shorthand_non_solid_style_renders_as_none() {
        let dom = crate::dom::parser::parse("<p>t</p>");
        let sheet = parse("p { border: 2px dashed red; }");
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let p = find(&dom, "p").unwrap();
        assert_eq!(styles[p].border.top.style, BorderStyle::None);
    }

    #[test]
    fn color_named_hex_and_rgb_forms() {
        let dom = crate::dom::parser::parse("<p>t</p>");
        let cases = [
            ("color: red;", Color::rgb(255, 0, 0)),
            ("color: #0f0;", Color::rgb(0, 255, 0)),
            ("color: #0000ff;", Color::rgb(0, 0, 255)),
            ("color: rgb(10, 20, 30);", Color::rgb(10, 20, 30)),
        ];
        for (css, expect) in cases {
            let sheet = parse(&format!("p {{ {css} }}"));
            let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
            let p = find(&dom, "p").unwrap();
            assert_eq!(styles[p].color, expect, "for {css}");
        }
    }

    #[test]
    fn ignore_unknown_property_increments_counter() {
        let sheet = parse("p { flibbertigibbet: 1; color: red; }");
        assert_eq!(sheet.ignored_declarations, 1);
    }

    #[test]
    fn ignore_unknown_at_rule() {
        let sheet = parse("@font-face { font-family: X; src: url(x.woff); } p { color: red; }");
        assert_eq!(sheet.ignored_at_rules, 1);
        assert_eq!(sheet.rules.len(), 1); // the trailing rule still parses
    }

    #[test]
    fn media_query_is_parsed_but_never_applied() {
        let sheet = parse("@media (min-width: 800px) { p { color: red; } } p { color: blue; }");
        assert_eq!(sheet.media_at_rules, 1);
        // The rule inside @media must not leak into the flat rule list —
        // it is inert until M5 wires up viewport evaluation.
        assert_eq!(sheet.rules.len(), 1);

        let dom = crate::dom::parser::parse("<p>t</p>");
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let p = find(&dom, "p").unwrap();
        assert_eq!(styles[p].color, Color::rgb(0, 0, 255));
    }

    #[test]
    fn malformed_declaration_is_skipped_and_rest_still_parses() {
        let dom = crate::dom::parser::parse("<p>t</p>");
        let sheet = parse("p { color : ; color: red; }");
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let p = find(&dom, "p").unwrap();
        assert_eq!(styles[p].color, Color::rgb(255, 0, 0));
    }

    #[test]
    fn malformed_rule_is_skipped_to_next_brace_and_next_rule_still_parses() {
        let dom = crate::dom::parser::parse("<p>t</p>");
        let sheet = parse("!!!broken!!! } p { color: red; }");
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let p = find(&dom, "p").unwrap();
        assert_eq!(styles[p].color, Color::rgb(255, 0, 0));
    }

    #[test]
    fn unsupported_selector_kinds_parse_without_choking_and_do_not_match() {
        let dom = crate::dom::parser::parse("<p>t</p>");
        for css in ["p > span { color: red; }", "a[href='x'] { color: red; }", "p::before { color: red; }", "p:hover { color: red; }"] {
            let sheet = parse(css);
            let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
            let p = find(&dom, "p").unwrap();
            assert_ne!(styles[p].color, Color::rgb(255, 0, 0), "for {css}");
        }
    }

    #[test]
    fn does_not_panic_on_malformed_css_sweep() {
        let inputs = [
            "",
            "{",
            "}",
            "p {",
            "p color: red; }",
            "p { color",
            "@",
            "@media",
            "/* unterminated",
            "\"unterminated string",
            "p { color: red",
            ".. {}",
            "####{}",
            "p{color:red;;;;}",
            "p{}{}{}",
            ":::: {}",
            "a[href='x'] { color: red; }",
            "p > span { color: red; }",
            "\0\0\0",
            "*{color:red}",
            "@import url(x.css);",
            "@charset \"utf-8\";",
        ];
        for i in inputs {
            let _ = parse(i);
        }
    }
}
