//! M5: pull author CSS out of the DOM so `cascade` can actually be fed it.
//! Until this packet, every `cascade()` call site passed `&[]` for
//! `author_sheets` — a `<style>` block's text sat in the DOM (P1 kept it as
//! raw text, per `dom::parser`'s `RAWTEXT_ELEMENTS` treatment) but nothing
//! ever read it back out and parsed it. This module is that missing step.
//!
//! Scope: inline `<style>` elements only. `<link rel="stylesheet" href=...>`
//! needs a fetch pre-pass (like `images::collect_images`) that this packet
//! does not add — deferred to a follow-up (see the M5 report/DECISIONS
//! entry). A `<link>` with no matching fetch pass just sits inert in the
//! DOM, exactly as before this packet.

use crate::dom::{Dom, Node, NodeId};
use crate::style::{media, parser, Stylesheet};

/// Walk `dom` in document order, parse every `<style>` element's raw-text
/// content into a [`Stylesheet`], and return them in document order — later
/// `<style>` blocks win ties against earlier ones, matching the cascade's
/// existing "later author sheet wins specificity ties" source-order
/// behavior (`cascade::fold_matching_declarations`'s doc comment).
///
/// Total: an empty DOM or one with no `<style>` element returns an empty
/// `Vec`; a `<style>` with empty or malformed CSS still parses (via
/// `parser::parse`, itself total per its own doc comment) to a
/// mostly/fully-empty `Stylesheet` rather than being skipped or panicking.
/// Depth-safe like `cascade::visit`: an explicit heap stack drives the walk,
/// not call-stack recursion, so pathologically deep markup can't overflow
/// it.
pub fn collect_author_sheets(dom: &Dom) -> Vec<Stylesheet> {
    let mut sheets = Vec::new();
    if dom.is_empty() {
        return sheets;
    }

    // Explicit stack, pushed in reverse child order so children pop (and
    // thus get visited) left-to-right — the same document-order walk
    // `cascade::visit` drives, just without needing its Enter/Exit ancestor
    // bookkeeping (nothing here needs an ancestor chain).
    let mut stack: Vec<NodeId> = vec![dom.root()];
    while let Some(id) = stack.pop() {
        let Node::Element(el) = dom.node(id) else {
            continue;
        };
        if el.name.as_str() == "style" {
            sheets.push(parser::parse(&style_text(dom, id)));
        }
        for &child in el.children.iter().rev() {
            stack.push(child);
        }
    }
    sheets
}

/// Concatenate a `<style>` element's direct `Text` children's raw content.
/// In practice `dom::parser` always gives a `<style>` exactly one `Text`
/// child (it's a RAWTEXT element — see that module's doc comment), but
/// concatenating defensively rather than indexing `children[0]` keeps this
/// total even if that invariant ever loosens, and costs nothing when it
/// doesn't.
/// Same document-order walk as [`collect_author_sheets`], but each
/// resulting `Stylesheet` is run through `style::media::flatten_media`
/// against `viewport_width_px` before being returned (M5) — so any `@media`
/// rule inside a `<style>` block actually takes effect (or doesn't) per the
/// real render viewport, instead of sitting inert.
///
/// Every real render entry point (`main.rs`'s `dump_text`/`dump_png`/
/// `render_fb_surface`, `frames.rs`'s `render_single_document`) calls THIS
/// rather than the plain [`collect_author_sheets`] — plain
/// `collect_author_sheets` is kept as-is (and still used directly by
/// pre-M5 tests/call sites that have no viewport to give and no `@media` in
/// their fixtures) so this packet adds a new entry point rather than
/// changing an existing one's behavior/signature.
///
/// Total: delegates entirely to `collect_author_sheets` (already total —
/// see its own doc comment) and `flatten_media` (also total, bounded by the
/// sheet's own size — see that function's doc comment); no new failure mode.
pub fn collect_author_sheets_for_viewport(dom: &Dom, viewport_width_px: f32) -> Vec<Stylesheet> {
    collect_author_sheets(dom).into_iter().map(|s| media::flatten_media(&s, viewport_width_px)).collect()
}

fn style_text(dom: &Dom, style_id: NodeId) -> String {
    let mut css = String::new();
    if let Node::Element(el) = dom.node(style_id) {
        for &child in &el.children {
            if let Node::Text(t) = dom.node(child) {
                css.push_str(t);
            }
        }
    }
    css
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom;
    use crate::style::cascade;
    use crate::style::computed::Display;
    use crate::surface::Color;

    fn find(d: &dom::Dom, tag: &str) -> dom::NodeId {
        fn walk(d: &dom::Dom, id: dom::NodeId, tag: &str) -> Option<dom::NodeId> {
            let el = d.node(id).element()?;
            if el.name.as_str() == tag {
                return Some(id);
            }
            for &c in &el.children {
                if let Some(found) = walk(d, c, tag) {
                    return Some(found);
                }
            }
            None
        }
        walk(d, d.root(), tag).unwrap_or_else(|| panic!("no <{tag}> found"))
    }

    #[test]
    fn no_style_element_yields_no_sheets() {
        let d = dom::parser::parse("<p>hello</p>");
        assert!(collect_author_sheets(&d).is_empty());
    }

    #[test]
    fn empty_dom_yields_no_sheets() {
        let d = dom::Dom::new();
        assert!(collect_author_sheets(&d).is_empty());
    }

    #[test]
    fn single_style_block_is_collected_and_makes_p_red_via_cascade() {
        let d = dom::parser::parse("<head><style>p { color: red }</style></head><body><p>x</p></body>");
        let sheets = collect_author_sheets(&d);
        assert_eq!(sheets.len(), 1);
        let styles = cascade::cascade(&d, &sheets);
        assert_eq!(styles[find(&d, "p")].color, Color::rgb(255, 0, 0));
    }

    #[test]
    fn multiple_style_blocks_apply_in_document_order() {
        // Same selector/property in two <style> blocks: the later one in
        // document order must win, exactly like two separately-passed
        // author sheets already do (cascade::multiple_author_sheets_apply_in_order).
        let d = dom::parser::parse("<style>p { color: red }</style><style>p { color: blue }</style><p>x</p>");
        let sheets = collect_author_sheets(&d);
        assert_eq!(sheets.len(), 2);
        let styles = cascade::cascade(&d, &sheets);
        assert_eq!(styles[find(&d, "p")].color, Color::rgb(0, 0, 255));
    }

    #[test]
    fn malformed_style_block_yields_a_stylesheet_not_a_panic() {
        let d = dom::parser::parse("<style>!!!broken!!!</style><p>x</p>");
        let sheets = collect_author_sheets(&d);
        assert_eq!(sheets.len(), 1);
        let _ = cascade::cascade(&d, &sheets); // must not panic
    }

    #[test]
    fn empty_style_block_yields_an_empty_stylesheet() {
        let d = dom::parser::parse("<style></style><p>x</p>");
        let sheets = collect_author_sheets(&d);
        assert_eq!(sheets.len(), 1);
        assert_eq!(sheets[0].rules.len(), 0);
    }

    #[test]
    fn author_sheet_from_style_element_still_overridden_by_higher_specificity_ua_free_case() {
        // Sanity: a <style> block reaching cascade really changes display,
        // not just color -- confirms the wiring isn't accidentally scoped
        // to one property.
        let d = dom::parser::parse("<style>span { display: block; }</style><span>x</span>");
        let sheets = collect_author_sheets(&d);
        let styles = cascade::cascade(&d, &sheets);
        assert_eq!(styles[find(&d, "span")].display, Display::Block);
    }

    // ---- M5: collect_author_sheets_for_viewport / @media -----------------

    #[test]
    fn viewport_variant_matches_plain_collect_when_no_media_present() {
        let d = dom::parser::parse("<style>p { color: red }</style><p>x</p>");
        let plain = collect_author_sheets(&d);
        let viewport = collect_author_sheets_for_viewport(&d, 320.0);
        assert_eq!(plain.len(), viewport.len());
        let styles_plain = cascade::cascade(&d, &plain);
        let styles_viewport = cascade::cascade(&d, &viewport);
        assert_eq!(styles_plain[find(&d, "p")].color, styles_viewport[find(&d, "p")].color);
    }

    #[test]
    fn end_to_end_media_query_produces_a_responsive_result() {
        let html = r#"<style>
            p { color: blue }
            @media (max-width: 500px) { p { color: red } }
        </style><p>x</p>"#;
        let d = dom::parser::parse(html);

        let narrow = collect_author_sheets_for_viewport(&d, 320.0);
        let narrow_styles = cascade::cascade(&d, &narrow);
        assert_eq!(narrow_styles[find(&d, "p")].color, Color::rgb(255, 0, 0));

        let wide = collect_author_sheets_for_viewport(&d, 640.0);
        let wide_styles = cascade::cascade(&d, &wide);
        assert_eq!(wide_styles[find(&d, "p")].color, Color::rgb(0, 0, 255));
    }

    #[test]
    fn huge_pathological_style_block_with_media_and_deep_dom_do_not_panic() {
        let mut css = String::new();
        for i in 0..3000 {
            css.push_str(&format!("@media (min-width: {i}px) {{ .c{i} {{ color: red; }} }}\n"));
        }
        let mut html = format!("<style>{css}</style>");
        for _ in 0..2000 {
            html.push_str("<div>");
        }
        html.push('x');
        for _ in 0..2000 {
            html.push_str("</div>");
        }
        let d = dom::parser::parse(&html);
        let sheets = collect_author_sheets_for_viewport(&d, 1024.0);
        assert_eq!(sheets.len(), 1);
        let styles = cascade::cascade(&d, &sheets);
        assert_eq!(styles.len(), d.len());
    }

    /// Totality: a huge, pathological `<style>` block (thousands of rules)
    /// plus a deep DOM must not panic collecting or cascading it. The
    /// per-piece parsers/`cascade` are already independently total (their
    /// own doc comments say so); this just confirms the M5 wiring between
    /// them doesn't introduce a new failure mode (e.g. an accidental
    /// recursive walk in `collect_author_sheets` that could overflow on
    /// deep markup the way the pre-recursion-hardening `cascade` once did).
    #[test]
    fn huge_pathological_style_block_and_deep_dom_do_not_panic() {
        let mut css = String::new();
        for i in 0..5000 {
            css.push_str(&format!(".c{i} {{ color: red; }}\n"));
        }
        let mut html = format!("<style>{css}</style>");
        for _ in 0..3000 {
            html.push_str("<div>");
        }
        html.push('x');
        for _ in 0..3000 {
            html.push_str("</div>");
        }
        let d = dom::parser::parse(&html);
        let sheets = collect_author_sheets(&d);
        assert_eq!(sheets.len(), 1);
        assert_eq!(sheets[0].rules.len(), 5000);
        let styles = cascade::cascade(&d, &sheets);
        assert_eq!(styles.len(), d.len());
    }
}
