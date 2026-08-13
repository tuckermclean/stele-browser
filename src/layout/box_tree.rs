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

use crate::dom::{Dom, Element, Node, NodeId};
use crate::layout::{BoxContent, LayoutNode, Size};
use crate::style::computed::Display;
use crate::style::ComputedStyle;

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
/// Returns `None` if the document is empty or its root is `display: none`.
///
/// Total: never panics on any `dom`/`styles` pairing produced by
/// `dom::parser::parse` + `style::cascade::cascade` (the styles slice is
/// always exactly `dom.len()` long from that pipeline; this function is
/// still defensive against a shorter slice via `styles.get`).
pub fn build_box_tree(_dom: &Dom, _styles: &[ComputedStyle]) -> Option<LayoutNode> {
    todo!("P7 RED: build_box_tree")
}

fn build_node(dom: &Dom, styles: &[ComputedStyle], id: NodeId, depth: usize) -> Option<LayoutNode> {
    let style = styles.get(id)?.clone();
    if style.display == Display::None {
        return None;
    }
    match dom.node(id) {
        Node::Text(text) => Some(LayoutNode {
            style,
            content: BoxContent::Text(text.clone()),
            children: Vec::new(),
        }),
        Node::Element(el) => {
            if is_replaced(el) {
                return Some(LayoutNode {
                    style,
                    content: BoxContent::Replaced { intrinsic: img_intrinsic(el) },
                    children: Vec::new(),
                });
            }
            let children = if depth >= DEPTH_CAP {
                Vec::new()
            } else {
                el.children
                    .iter()
                    .filter_map(|&child| build_node(dom, styles, child, depth + 1))
                    .collect()
            };
            Some(LayoutNode { style, content: BoxContent::Container, children })
        }
    }
}

/// Only `img` is a replaced element in v0 (brief scope: "keep to `img` for
/// now").
fn is_replaced(el: &Element) -> bool {
    el.name.as_str() == "img"
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

fn parse_nonneg(raw: Option<&str>) -> Option<f32> {
    let v: f32 = raw?.trim().parse().ok()?;
    if v.is_finite() && v >= 0.0 {
        Some(v)
    } else {
        None
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
        let root = build_box_tree(&d, &styles).expect("root not display:none");
        assert!(find_text(&root, "keep").is_some());
        assert!(find_text(&root, "drop").is_none());
        assert!(find_text(&root, "me").is_none());
    }

    #[test]
    fn text_node_maps_to_box_content_text() {
        let d = dom::parser::parse("<p>hello</p>");
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles).expect("root present");
        let text_node = find_text(&root, "hello").expect("text fragment present");
        assert!(matches!(&text_node.content, BoxContent::Text(t) if t == "hello"));
    }

    #[test]
    fn plain_element_maps_to_container_with_recursive_children() {
        let d = dom::parser::parse("<div><span>a</span><span>b</span></div>");
        let styles = cascade::cascade(&d, &[]);
        assert!(find(&d, "div").is_some());
        let root = build_box_tree(&d, &styles).expect("root present");
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
        let root = build_box_tree(&d, &styles).expect("root present");

        fn find_img(node: &LayoutNode) -> Option<&LayoutNode> {
            if matches!(node.content, BoxContent::Replaced { .. }) {
                return Some(node);
            }
            node.children.iter().find_map(find_img)
        }
        let img = find_img(&root).expect("img box present");
        match img.content {
            BoxContent::Replaced { intrinsic } => {
                assert_eq!(intrinsic.w, 120.0);
                assert_eq!(intrinsic.h, 80.0);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn img_element_without_dimensions_defaults_to_zero_intrinsic() {
        let d = dom::parser::parse(r#"<img src="x.png">"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles).expect("root present");

        fn find_img(node: &LayoutNode) -> Option<&LayoutNode> {
            if matches!(node.content, BoxContent::Replaced { .. }) {
                return Some(node);
            }
            node.children.iter().find_map(find_img)
        }
        let img = find_img(&root).expect("img box present");
        match img.content {
            BoxContent::Replaced { intrinsic } => {
                assert_eq!(intrinsic.w, 0.0);
                assert_eq!(intrinsic.h, 0.0);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn nested_structure_and_order_are_preserved() {
        let d = dom::parser::parse("<ul><li>one</li><li>two</li><li>three</li></ul>");
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles).expect("root present");

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
        let root = build_box_tree(&d, &styles).expect("bare <html> root is not display:none");
        assert!(root.children.is_empty());
        assert!(matches!(root.content, BoxContent::Container));
    }

    #[test]
    fn display_none_root_yields_none() {
        let d = dom::parser::parse("<html><body>x</body></html>");
        let sheet = parser::parse("html { display: none; }");
        let styles = cascade::cascade(&d, std::slice::from_ref(&sheet));
        assert!(build_box_tree(&d, &styles).is_none());
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
        let root = build_box_tree(&d, &styles);
        assert!(root.is_some());
        let total = count_nodes(&root.unwrap());
        assert!(total > 0);
    }
}
