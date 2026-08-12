//! CSS parsing (P2, Wave 1). Full syntax is parsed; unknown declarations are
//! counted and dropped (the IGNORE-UNKNOWN treaty, charter C2). Selectors in
//! scope: element, `.class`, `#id`, descendant, grouping, `a:link`/`:visited`
//! (brief §4).

use crate::style::selector::{Compound, ElementInfo, Pseudo, Selector};
use crate::style::tokenizer::{tokenize, Token};
use crate::style::value::{self, Declarations};

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

/// Parse a stylesheet. Total: never panics, on any input. Full CSS syntax is
/// tokenized and walked; only the curated declarations (brief §4) survive
/// into `rules` — everything else is counted (charter C2's ignore-unknown
/// treaty) and dropped. Recovery is per brief §10: a bad declaration skips to
/// the next `;`; a bad rule (no `{` reachable) skips to the next `}`.
pub fn parse(css: &str) -> Stylesheet {
    let tokens = tokenize(css);
    let mut sheet = Stylesheet::default();
    let mut pos = 0usize;
    let mut order = 0u32;
    let len = tokens.len();

    while pos < len {
        skip_ws(&tokens, &mut pos);
        if pos >= len {
            break;
        }
        match &tokens[pos] {
            Token::AtKeyword(name) => {
                let is_media = name.eq_ignore_ascii_case("media");
                pos += 1;
                skip_at_rule_body(&tokens, &mut pos, is_media, &mut sheet);
            }
            // A stray close-brace at the top level: nothing to close, drop it.
            Token::RBrace => pos += 1,
            _ => parse_rule(&tokens, &mut pos, &mut sheet, &mut order),
        }
    }
    sheet
}

/// Every rule in `sheet` whose selector matches `target` (given its ancestor
/// chain), unordered. The cascade needs to merge these against matches from
/// *other* sheets (UA vs. every author sheet) before sorting by precedence —
/// sorting per-sheet here would let a later sheet win regardless of
/// specificity, which is wrong (specificity is compared globally within an
/// origin; only ties fall back to source order — see `cascade::visit`).
pub(crate) fn matching_rules<'a>(sheet: &'a Stylesheet, ancestors: &[ElementInfo], target: &ElementInfo) -> Vec<&'a StyleRule> {
    sheet.rules.iter().filter(|r| r.selector.matches(ancestors, target)).collect()
}

fn skip_ws(tokens: &[Token], pos: &mut usize) {
    while *pos < tokens.len() && tokens[*pos] == Token::Whitespace {
        *pos += 1;
    }
}

/// Consume one at-rule's body after its keyword: either up to and including
/// a top-level `;`, or — if a `{` appears first — a balanced-brace block
/// (needed for `@media { ... }`, whose block contains whole nested rules).
/// Never panics; if neither terminator appears, consumes to EOF.
fn skip_at_rule_body(tokens: &[Token], pos: &mut usize, is_media: bool, sheet: &mut Stylesheet) {
    let len = tokens.len();
    while *pos < len {
        match &tokens[*pos] {
            Token::Semicolon => {
                *pos += 1;
                bump_at_rule_counter(sheet, is_media);
                return;
            }
            Token::LBrace => {
                *pos += 1;
                let mut depth = 1i32;
                while *pos < len && depth > 0 {
                    match &tokens[*pos] {
                        Token::LBrace => depth += 1,
                        Token::RBrace => depth -= 1,
                        _ => {}
                    }
                    *pos += 1;
                }
                bump_at_rule_counter(sheet, is_media);
                return;
            }
            _ => *pos += 1,
        }
    }
    // Ran off the end without a terminator — still counts; nothing left to skip.
    bump_at_rule_counter(sheet, is_media);
}

fn bump_at_rule_counter(sheet: &mut Stylesheet, is_media: bool) {
    if is_media {
        sheet.media_at_rules += 1;
    } else {
        sheet.ignored_at_rules += 1;
    }
}

/// Parse one rule: a selector list (comma-separated), then a `{ ... }`
/// declaration block. If no `{` is reachable before a boundary that can't be
/// part of a selector (`}`, `;`, or EOF), the whole prelude is a bad rule —
/// recover by skipping to the next `}` per brief §10.
fn parse_rule(tokens: &[Token], pos: &mut usize, sheet: &mut Stylesheet, order: &mut u32) {
    let len = tokens.len();
    let selectors = parse_selector_list(tokens, pos);
    skip_ws(tokens, pos);

    if *pos >= len || tokens[*pos] != Token::LBrace {
        while *pos < len && tokens[*pos] != Token::RBrace {
            *pos += 1;
        }
        if *pos < len {
            *pos += 1; // consume the recovering '}'
        }
        return;
    }

    *pos += 1; // consume '{'
    let decls = parse_declaration_block(tokens, pos, sheet);

    let this_order = *order;
    *order += 1;
    for sel in selectors {
        sheet.rules.push(StyleRule {
            selector: sel,
            order: this_order,
            declarations: decls.clone(),
        });
    }
}

fn parse_selector_list(tokens: &[Token], pos: &mut usize) -> Vec<Selector> {
    let mut selectors = vec![parse_selector(tokens, pos)];
    loop {
        skip_ws(tokens, pos);
        if *pos < tokens.len() && tokens[*pos] == Token::Comma {
            *pos += 1;
            skip_ws(tokens, pos);
            selectors.push(parse_selector(tokens, pos));
        } else {
            break;
        }
    }
    selectors
}

/// Parse one selector (up to `{`/`}`/`;`/`,`/EOF). Constructs outside brief
/// §4's scope (child/sibling combinators, attribute selectors, pseudo-
/// elements, unknown pseudo-classes, …) mark the selector `supported: false`
/// but never abort the parse — the surrounding rule's declarations still get
/// counted correctly, the selector just never matches (charter C2 applied to
/// selectors, not just declarations).
#[allow(unused_assignments)] // `flush!()`'s reset-to-false is always immediately followed by a fresh true
fn parse_selector(tokens: &[Token], pos: &mut usize) -> Selector {
    let len = tokens.len();
    let mut compounds: Vec<Compound> = Vec::new();
    let mut supported = true;
    let mut cur = Compound::default();
    let mut cur_has_content = false;
    let mut pending_descendant = false;

    macro_rules! flush {
        () => {
            if cur_has_content {
                compounds.push(std::mem::take(&mut cur));
                cur_has_content = false;
            }
        };
    }

    while *pos < len {
        match &tokens[*pos] {
            Token::Whitespace => {
                if cur_has_content {
                    pending_descendant = true;
                }
                *pos += 1;
            }
            Token::LBrace | Token::RBrace | Token::Semicolon | Token::Comma => break,
            Token::Ident(name) => {
                if pending_descendant {
                    flush!();
                    pending_descendant = false;
                }
                cur.element = Some(name.to_ascii_lowercase());
                cur_has_content = true;
                *pos += 1;
            }
            Token::Star => {
                if pending_descendant {
                    flush!();
                    pending_descendant = false;
                }
                cur.element = None;
                cur_has_content = true;
                *pos += 1;
            }
            Token::Dot => {
                *pos += 1;
                if let Some(Token::Ident(name)) = tokens.get(*pos) {
                    if pending_descendant {
                        flush!();
                        pending_descendant = false;
                    }
                    cur.classes.push(name.to_ascii_lowercase());
                    cur_has_content = true;
                    *pos += 1;
                } else {
                    supported = false;
                }
            }
            Token::Hash(id) => {
                if pending_descendant {
                    flush!();
                    pending_descendant = false;
                }
                cur.id = Some(id.to_ascii_lowercase());
                cur_has_content = true;
                *pos += 1;
            }
            Token::Colon => {
                *pos += 1;
                if tokens.get(*pos) == Some(&Token::Colon) {
                    *pos += 1; // pseudo-element `::x` — unsupported
                    supported = false;
                }
                if let Some(Token::Ident(name)) = tokens.get(*pos) {
                    if pending_descendant {
                        flush!();
                        pending_descendant = false;
                    }
                    match name.to_ascii_lowercase().as_str() {
                        "link" => cur.pseudo.push(Pseudo::Link),
                        "visited" => cur.pseudo.push(Pseudo::Visited),
                        _ => supported = false,
                    }
                    cur_has_content = true;
                    *pos += 1;
                    if tokens.get(*pos) == Some(&Token::LParen) {
                        // Functional pseudo-class (`:nth-child(...)`, etc.) —
                        // unsupported; skip its balanced parens.
                        supported = false;
                        *pos += 1;
                        let mut depth = 1i32;
                        while *pos < len && depth > 0 {
                            match &tokens[*pos] {
                                Token::LParen => depth += 1,
                                Token::RParen => depth -= 1,
                                _ => {}
                            }
                            *pos += 1;
                        }
                    }
                } else {
                    supported = false;
                }
            }
            Token::Function(_) => {
                supported = false;
                *pos += 1;
                let mut depth = 1i32;
                while *pos < len && depth > 0 {
                    match &tokens[*pos] {
                        Token::LParen | Token::Function(_) => depth += 1,
                        Token::RParen => depth -= 1,
                        _ => {}
                    }
                    *pos += 1;
                }
            }
            Token::Delim('>') | Token::Delim('+') | Token::Delim('~') => {
                flush!();
                supported = false;
                pending_descendant = false;
                *pos += 1;
            }
            Token::Delim('[') => {
                supported = false;
                *pos += 1;
                while *pos < len && tokens[*pos] != Token::Delim(']') {
                    *pos += 1;
                }
                if *pos < len {
                    *pos += 1;
                }
                cur_has_content = true;
            }
            _ => {
                supported = false;
                *pos += 1;
            }
        }
    }
    flush!();
    let has_compounds = !compounds.is_empty();
    Selector {
        compounds,
        supported: supported && has_compounds,
    }
}

/// Parse a `{ ... }` declaration block; `*pos` starts just past the `{` and
/// ends just past the matching `}` (or at EOF, tolerated). Each declaration
/// that fails to parse (bad property/colon/value) or names a property
/// outside the curated set counts against `sheet.ignored_declarations` and
/// recovers by skipping to the next `;`.
fn parse_declaration_block(tokens: &[Token], pos: &mut usize, sheet: &mut Stylesheet) -> Declarations {
    let mut decls = Declarations::default();
    let len = tokens.len();
    loop {
        skip_ws(tokens, pos);
        if *pos >= len {
            return decls;
        }
        match &tokens[*pos] {
            Token::RBrace => {
                *pos += 1;
                return decls;
            }
            Token::Semicolon => {
                *pos += 1;
            }
            Token::Ident(name) => {
                let name = name.to_ascii_lowercase();
                *pos += 1;
                skip_ws(tokens, pos);
                if *pos < len && tokens[*pos] == Token::Colon {
                    *pos += 1;
                } else {
                    sheet.ignored_declarations += 1;
                    skip_to_decl_boundary(tokens, pos);
                    continue;
                }
                skip_ws(tokens, pos);
                let value_start = *pos;
                while *pos < len && tokens[*pos] != Token::Semicolon && tokens[*pos] != Token::RBrace {
                    *pos += 1;
                }
                let value_tokens: Vec<Token> = tokens[value_start..*pos].iter().filter(|t| **t != Token::Whitespace).cloned().collect();
                if *pos < len && tokens[*pos] == Token::Semicolon {
                    *pos += 1;
                }
                if !value::apply_property(&name, &value_tokens, &mut decls) {
                    sheet.ignored_declarations += 1;
                }
            }
            _ => {
                sheet.ignored_declarations += 1;
                skip_to_decl_boundary(tokens, pos);
            }
        }
    }
}

fn skip_to_decl_boundary(tokens: &[Token], pos: &mut usize) {
    let len = tokens.len();
    while *pos < len && tokens[*pos] != Token::Semicolon && tokens[*pos] != Token::RBrace {
        *pos += 1;
    }
    if *pos < len && tokens[*pos] == Token::Semicolon {
        *pos += 1;
    }
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
    fn bad_shorthand_token_counts_as_ignored_and_does_not_apply_partially() {
        // `div` (unlike `p`) has no UA-sheet margin default, so a rejected
        // author declaration should leave it at the CSS initial `0`.
        let dom = crate::dom::parser::parse("<div>t</div>");
        let sheet = parse("div { margin: 1px bogus 2px 3px; }");
        assert_eq!(sheet.ignored_declarations, 1);
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let div = find(&dom, "div").unwrap();
        assert_eq!(styles[div].margin.top, LengthPercentageAuto::Px(0.0)); // CSS initial, not `1px`

        let sheet = parse("p { border: 5% solid red; }");
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
