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
pub fn build_box_tree(dom: &Dom, styles: &[ComputedStyle]) -> Option<LayoutNode> {
    if dom.is_empty() {
        return None;
    }
    build_node(dom, styles, dom.root(), 0)
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
            let content = if style.display == Display::TableCell {
                let (colspan, rowspan) = cell_spans(el);
                BoxContent::TableCell { colspan, rowspan }
            } else {
                BoxContent::Container
            };
            Some(LayoutNode { style, content, children })
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
    fn table_cell_maps_to_box_content_table_cell_with_spans() {
        let d = dom::parser::parse(
            r#"<table><tr><td colspan="2" rowspan="3">x</td><td>y</td></tr></table>"#,
        );
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles).expect("root present");

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
        let root = build_box_tree(&d, &styles).expect("root present");

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

    #[test]
    fn display_none_root_yields_none() {
        let d = dom::parser::parse("<html><body>x</body></html>");
        let sheet = parser::parse("html { display: none; }");
        let styles = cascade::cascade(&d, std::slice::from_ref(&sheet));
        assert!(build_box_tree(&d, &styles).is_none());
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
        let root = build_box_tree(&d, &styles).expect("root present");
        assert!(find_text(&root, "[hi]").is_some());
    }

    #[test]
    fn text_input_without_type_defaults_to_text_behavior() {
        let d = dom::parser::parse(r#"<input name="a" value="hi">"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles).expect("root present");
        assert!(find_text(&root, "[hi]").is_some());
    }

    #[test]
    fn text_input_without_value_renders_spaces_sized_to_size_attr() {
        let d = dom::parser::parse(r#"<input type="text" name="a" size="4">"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles).expect("root present");
        assert!(find_text(&root, "[    ]").is_some(), "expected 4 spaces inside brackets");
    }

    #[test]
    fn text_input_without_value_or_size_defaults_to_ten_spaces() {
        let d = dom::parser::parse(r#"<input type="text" name="a">"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles).expect("root present");
        let expected = format!("[{}]", " ".repeat(10));
        assert!(find_text(&root, &expected).is_some());
    }

    #[test]
    fn password_input_masks_value_with_asterisks() {
        let d = dom::parser::parse(r#"<input type="password" name="p" value="secret">"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles).expect("root present");
        assert!(find_text(&root, "[******]").is_some());
    }

    #[test]
    fn checkbox_shows_x_when_checked_and_blank_when_not() {
        let d = dom::parser::parse(r#"<input type="checkbox" name="c" checked>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles).expect("root present");
        assert!(find_text(&root, "[x]").is_some());

        let d2 = dom::parser::parse(r#"<input type="checkbox" name="c">"#);
        let styles2 = cascade::cascade(&d2, &[]);
        let root2 = build_box_tree(&d2, &styles2).expect("root present");
        assert!(find_text(&root2, "[ ]").is_some());
    }

    #[test]
    fn radio_shows_star_when_checked_and_blank_when_not() {
        let d = dom::parser::parse(r#"<input type="radio" name="r" checked>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles).expect("root present");
        assert!(find_text(&root, "(*)").is_some());

        let d2 = dom::parser::parse(r#"<input type="radio" name="r">"#);
        let styles2 = cascade::cascade(&d2, &[]);
        let root2 = build_box_tree(&d2, &styles2).expect("root present");
        assert!(find_text(&root2, "( )").is_some());
    }

    #[test]
    fn submit_input_shows_value_or_default_submit_label() {
        let d = dom::parser::parse(r#"<input type="submit" value="Go">"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles).expect("root present");
        assert!(find_text(&root, "[ Go ]").is_some());

        let d2 = dom::parser::parse(r#"<input type="submit">"#);
        let styles2 = cascade::cascade(&d2, &[]);
        let root2 = build_box_tree(&d2, &styles2).expect("root present");
        assert!(find_text(&root2, "[ Submit ]").is_some());
    }

    #[test]
    fn reset_and_button_type_inputs_show_bracketed_labels() {
        let d = dom::parser::parse(r#"<input type="reset">"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles).expect("root present");
        assert!(find_text(&root, "[ Reset ]").is_some());

        let d2 = dom::parser::parse(r#"<input type="button" value="Click">"#);
        let styles2 = cascade::cascade(&d2, &[]);
        let root2 = build_box_tree(&d2, &styles2).expect("root present");
        assert!(find_text(&root2, "[ Click ]").is_some());
    }

    #[test]
    fn hidden_input_renders_nothing() {
        let d = dom::parser::parse(r#"<div>before<input type="hidden" name="x" value="topsecret123"><span>after</span></div>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles).expect("root present");
        assert!(find_text(&root, "before").is_some());
        assert!(find_text(&root, "after").is_some());
        assert!(find_text(&root, "topsecret123").is_none(), "hidden input's value must never appear");
    }

    #[test]
    fn button_element_shows_value_then_child_text_then_default() {
        let d = dom::parser::parse(r#"<button>Send</button>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles).expect("root present");
        assert!(find_text(&root, "[ Send ]").is_some());

        let d2 = dom::parser::parse(r#"<button value="X">Ignored</button>"#);
        let styles2 = cascade::cascade(&d2, &[]);
        let root2 = build_box_tree(&d2, &styles2).expect("root present");
        assert!(find_text(&root2, "[ X ]").is_some(), "value attr takes priority over child text");

        let d3 = dom::parser::parse(r#"<button></button>"#);
        let styles3 = cascade::cascade(&d3, &[]);
        let root3 = build_box_tree(&d3, &styles3).expect("root present");
        assert!(find_text(&root3, "[ Submit ]").is_some(), "default when no value/child text");
    }

    #[test]
    fn textarea_shows_short_text_verbatim() {
        let d = dom::parser::parse(r#"<textarea name="n">hello</textarea>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles).expect("root present");
        assert!(find_text(&root, "hello").is_some());
    }

    #[test]
    fn textarea_truncates_long_first_line() {
        let d = dom::parser::parse(r#"<textarea name="n">this line is definitely longer than twenty chars</textarea>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles).expect("root present");
        assert!(find_text(&root, "[...]").is_some(), "long content should be truncated with an ellipsis marker");
    }

    #[test]
    fn textarea_marks_multiline_content_even_if_first_line_is_short() {
        let d = dom::parser::parse("<textarea name=\"n\">line one\nline two</textarea>");
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles).expect("root present");
        assert!(find_text(&root, "line one[...]").is_some());
    }

    #[test]
    fn select_shows_selected_option_text() {
        let d = dom::parser::parse(
            r#"<select name="color"><option value="r">Red</option><option value="g" selected>Green</option></select>"#,
        );
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles).expect("root present");
        assert!(find_text(&root, "[ Green v]").is_some());
    }

    #[test]
    fn select_with_no_selected_option_defaults_to_first() {
        let d = dom::parser::parse(
            r#"<select name="color"><option value="r">Red</option><option value="g">Green</option></select>"#,
        );
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles).expect("root present");
        assert!(find_text(&root, "[ Red v]").is_some());
    }

    #[test]
    fn select_with_no_options_renders_without_panicking() {
        let d = dom::parser::parse(r#"<select name="color"></select>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles).expect("root present");
        assert!(find_text(&root, "[  v]").is_some());
    }

    #[test]
    fn form_controls_never_recurse_into_their_own_dom_children_as_generic_boxes() {
        // A <select>'s <option>s must not show up as their own independent
        // Container/Text boxes distinct from the synthesized label -- the
        // whole control is exactly one Container + one Text child.
        let d = dom::parser::parse(r#"<select name="c"><option>Only</option></select>"#);
        let styles = cascade::cascade(&d, &[]);
        let root = build_box_tree(&d, &styles).expect("root present");

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
        let root = build_box_tree(&d, &styles);
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
}
