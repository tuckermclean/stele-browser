//! The bespoke frozen-dialect HTML parser with 1996-grade tag-soup recovery.
//!
//! P1 (Wave 1) owns this file. Its contract: consume arbitrary HTML text and
//! produce a [`Dom`]. Full syntax is parsed; a curated semantic set (brief §4)
//! is kept; the remainder is skipped per the standards' forward-compat rules.
//!
//! Two consumed-at-parse elements deserve note here, since this is where the
//! covenant is actually applied (the AST cannot express what this file refuses
//! to build): `<style>` contents are handed to the CSS layer, and executable
//! wire elements are discarded outright — no node is ever constructed for them,
//! which is exactly why `dom::ast` has no variant to hold one. `<noscript>`
//! content, by contrast, is rendered first-class (charter C3, the JS treaty).

use crate::dom::Dom;

/// Parse a document. Recovery rules (implied close for `p`/`li`/`td`/`tr`,
/// b/i mis-nesting tolerance, unclosed-everything at EOF) are P1's remit.
pub fn parse(_input: &str) -> Dom {
    todo!("P1: bespoke tag-soup parser")
}

// ---------------------------------------------------------------------------
// Tests (strict test-first: committed against the `todo!()` stub above, red
// until `parse` is implemented — brief §10 TDD protocol).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom::{Dom, Node, NodeId};

    // ---- tree-walk helpers (no pixel goldens; structural assertions only) --

    /// The element children of `id` (empty slice if `id` is text or absent).
    fn children_of(dom: &Dom, id: NodeId) -> Vec<NodeId> {
        match dom.node(id) {
            Node::Element(e) => e.children.clone(),
            Node::Text(_) => Vec::new(),
        }
    }

    fn elem_name<'a>(dom: &'a Dom, id: NodeId) -> Option<&'a str> {
        dom.node(id).element().map(|e| e.name.as_str())
    }

    /// First direct child element named `name`, if any.
    fn find_child(dom: &Dom, parent: NodeId, name: &str) -> Option<NodeId> {
        children_of(dom, parent)
            .into_iter()
            .find(|&c| elem_name(dom, c) == Some(name))
    }

    /// All direct child elements named `name`, in order.
    fn find_children(dom: &Dom, parent: NodeId, name: &str) -> Vec<NodeId> {
        children_of(dom, parent)
            .into_iter()
            .filter(|&c| elem_name(dom, c) == Some(name))
            .collect()
    }

    /// Depth-first search for the first descendant element named `name`.
    fn find_descendant(dom: &Dom, start: NodeId, name: &str) -> Option<NodeId> {
        if elem_name(dom, start) == Some(name) {
            return Some(start);
        }
        for c in children_of(dom, start) {
            if let Some(found) = find_descendant(dom, c, name) {
                return Some(found);
            }
        }
        None
    }

    /// Concatenated text of every text-node descendant of `id`.
    fn text_of(dom: &Dom, id: NodeId) -> String {
        let mut out = String::new();
        collect_text(dom, id, &mut out);
        out
    }

    fn collect_text(dom: &Dom, id: NodeId, out: &mut String) {
        match dom.node(id) {
            Node::Text(t) => out.push_str(t),
            Node::Element(e) => {
                for &c in &e.children {
                    collect_text(dom, c, out);
                }
            }
        }
    }

    // ------------------------------- well-formed nesting --------------------

    #[test]
    fn well_formed_nesting() {
        let dom = parse("<html><body><div><p>hello <b>world</b></p></div></body></html>");
        let root = dom.root();
        let body = find_descendant(&dom, root, "body").expect("body");
        let div = find_child(&dom, body, "div").expect("div");
        let p = find_child(&dom, div, "p").expect("p");
        assert_eq!(text_of(&dom, p), "hello world");
        let b = find_child(&dom, p, "b").expect("b");
        assert_eq!(text_of(&dom, b), "world");
    }

    // ------------------------------- implied close ---------------------------

    #[test]
    fn implied_close_p_in_p() {
        // <p>a<p>b -- second <p> implicitly closes the first.
        let dom = parse("<body><p>a<p>b</body>");
        let root = dom.root();
        let body = find_descendant(&dom, root, "body").expect("body");
        let ps = find_children(&dom, body, "p");
        assert_eq!(ps.len(), 2, "expected two sibling <p>s, not nested");
        assert_eq!(text_of(&dom, ps[0]), "a");
        assert_eq!(text_of(&dom, ps[1]), "b");
    }

    #[test]
    fn implied_close_li_runs() {
        let dom = parse("<ul><li>one<li>two<li>three</ul>");
        let root = dom.root();
        let ul = find_descendant(&dom, root, "ul").expect("ul");
        let lis = find_children(&dom, ul, "li");
        assert_eq!(lis.len(), 3, "each <li> should close the previous one");
        assert_eq!(text_of(&dom, lis[0]), "one");
        assert_eq!(text_of(&dom, lis[1]), "two");
        assert_eq!(text_of(&dom, lis[2]), "three");
    }

    #[test]
    fn implied_close_tr_td() {
        let dom = parse("<table><tr><td>A<td>B<tr><td>C<td>D</table>");
        let root = dom.root();
        let table = find_descendant(&dom, root, "table").expect("table");
        let trs = find_children(&dom, table, "tr");
        assert_eq!(trs.len(), 2, "second <tr> should close the first");
        let tds0 = find_children(&dom, trs[0], "td");
        assert_eq!(tds0.len(), 2, "second <td> should close the first within a row");
        assert_eq!(text_of(&dom, tds0[0]), "A");
        assert_eq!(text_of(&dom, tds0[1]), "B");
        let tds1 = find_children(&dom, trs[1], "td");
        assert_eq!(tds1.len(), 2);
        assert_eq!(text_of(&dom, tds1[0]), "C");
        assert_eq!(text_of(&dom, tds1[1]), "D");
    }

    #[test]
    fn implied_close_dt_dd() {
        let dom = parse("<dl><dt>Term<dd>Definition</dl>");
        let root = dom.root();
        let dl = find_descendant(&dom, root, "dl").expect("dl");
        let dt = find_child(&dom, dl, "dt").expect("dt");
        let dd = find_child(&dom, dl, "dd").expect("dd");
        assert_eq!(text_of(&dom, dt), "Term");
        assert_eq!(text_of(&dom, dd), "Definition");
    }

    #[test]
    fn implied_close_option() {
        let dom = parse("<select><option>a<option>b<option>c</select>");
        let root = dom.root();
        let select = find_descendant(&dom, root, "select").expect("select");
        let opts = find_children(&dom, select, "option");
        assert_eq!(opts.len(), 3);
    }

    // ------------------------------- mis-nesting ------------------------------

    #[test]
    fn b_i_misnesting_keeps_all_text() {
        // <b><i>...</b>...</i> -- overlapping close tags. Text must survive
        // even though the nesting cannot be made well-formed without a full
        // adoption-agency algorithm.
        let dom = parse("<p><b>bold <i>both</b> only-i</i> tail</p>");
        let root = dom.root();
        let p = find_descendant(&dom, root, "p").expect("p");
        assert_eq!(text_of(&dom, p), "bold both only-i tail");
    }

    // ------------------------------- EOF recovery -----------------------------

    #[test]
    fn unclosed_everything_at_eof() {
        let dom = parse("<div><p>a<span>b");
        let root = dom.root();
        let div = find_descendant(&dom, root, "div").expect("div");
        let p = find_child(&dom, div, "p").expect("p");
        let span = find_child(&dom, p, "span").expect("span");
        assert_eq!(text_of(&dom, span), "b");
        assert_eq!(text_of(&dom, div), "ab");
    }

    #[test]
    fn does_not_panic_on_hostile_or_truncated_input() {
        let hostiles = [
            "",
            "<",
            "</",
            "<!--",
            "<!-- unterminated comment",
            "<div",
            "<div ",
            "<div attr",
            "<div attr=",
            "<div attr='unterminated",
            "<div attr=\"unterminated",
            "<a href=",
            "<&&&<<<>>>",
            "&",
            "&#",
            "&#x",
            "&amp",
            "&;",
            "&#zzzz;",
            "1 < 2 and 3 > 1",
            "<3 <<< weird ascii art >>>",
            "<script>",
            "<script><div>",
            "<style",
            "</html></html></html>",
            "<p><p><p><p><p><p>",
            "<<<<<<<<<<<<<<<<<<<<<<<<",
            "\0\0\0null bytes\0\0\0",
            "<div class=\"a\" class=\"b\" class",
        ];
        for h in hostiles {
            let dom = parse(h);
            // Just proving we returned a Dom without panicking is the point;
            // sanity-check root is still readable.
            let _ = dom.node(dom.root());
        }
    }

    // ------------------------------- entities ---------------------------------

    #[test]
    fn named_and_numeric_entities_in_text() {
        let dom = parse("<p>&amp; &lt; &gt; &quot; &nbsp; &copy; &#169; &#xA9; &#XA9;</p>");
        let root = dom.root();
        let p = find_descendant(&dom, root, "p").expect("p");
        assert_eq!(text_of(&dom, p), "& < > \" \u{00A0} \u{00A9} \u{00A9} \u{00A9} \u{00A9}");
    }

    #[test]
    fn entities_in_attribute_values() {
        let dom = parse("<a href=\"/x?a=1&amp;b=2\" title=\"&copy; 2026\">link</a>");
        let root = dom.root();
        let a = find_descendant(&dom, root, "a").expect("a");
        let el = dom.node(a).element().unwrap();
        assert_eq!(el.attrs.get("href"), Some("/x?a=1&b=2"));
        assert_eq!(el.attrs.get("title"), Some("\u{00A9} 2026"));
    }

    // ------------------------------- script / style / noscript ---------------

    #[test]
    fn script_is_fully_discarded() {
        let dom = parse("<body><script>if (1 < 2) { alert('hi'); }</script><p>after</p></body>");
        let root = dom.root();
        let body = find_descendant(&dom, root, "body").expect("body");
        assert!(
            find_child(&dom, body, "script").is_none(),
            "no script element should ever be constructed"
        );
        // The parser must not have gotten confused by markup-looking text
        // inside the script and lost the sibling paragraph.
        let p = find_child(&dom, body, "p").expect("p survives after script");
        assert_eq!(text_of(&dom, p), "after");
        // Walk the WHOLE tree: nothing named "script" is reachable anywhere.
        assert!(find_descendant(&dom, root, "script").is_none());
    }

    #[test]
    fn style_is_kept_with_raw_text() {
        let dom = parse("<head><style>body { color: red; } /* <div>not a tag</div> */</style></head>");
        let root = dom.root();
        let style = find_descendant(&dom, root, "style").expect("style kept as an element");
        let raw = text_of(&dom, style);
        assert!(raw.contains("color: red"));
        assert!(raw.contains("<div>not a tag</div>"), "raw contents must not be tag-parsed");
    }

    #[test]
    fn noscript_is_first_class() {
        let dom = parse("<noscript><p>fallback content</p></noscript>");
        let root = dom.root();
        let noscript = find_descendant(&dom, root, "noscript").expect("noscript kept");
        let p = find_child(&dom, noscript, "p").expect("noscript's children parsed normally");
        assert_eq!(text_of(&dom, p), "fallback content");
    }

    // ------------------------------- void elements ----------------------------

    #[test]
    fn void_elements_have_no_children_and_no_end_tag_needed() {
        let dom = parse("<p>line one<br>line two<hr>after</p>");
        let root = dom.root();
        let p = find_descendant(&dom, root, "p").expect("p");
        let br = find_child(&dom, p, "br").expect("br");
        assert!(children_of(&dom, br).is_empty());
        let hr = find_child(&dom, p, "hr").expect("hr");
        assert!(children_of(&dom, hr).is_empty());
        // br/hr must not have swallowed "line two"/"after" as children.
        assert_eq!(text_of(&dom, p), "line oneline twoafter");
    }

    #[test]
    fn img_is_void_with_attrs() {
        let dom = parse("<img src=\"pic.gif\" alt=\"a picture\">");
        let root = dom.root();
        let img = find_descendant(&dom, root, "img").expect("img");
        let el = dom.node(img).element().unwrap();
        assert_eq!(el.attrs.get("src"), Some("pic.gif"));
        assert_eq!(el.attrs.get("alt"), Some("a picture"));
        assert!(el.children.is_empty());
    }

    // ------------------------------- comments / doctype -----------------------

    #[test]
    fn comments_and_doctype_are_dropped() {
        let dom = parse("<!doctype html><!-- top comment --><p>a<!-- mid --> b</p>");
        let root = dom.root();
        let p = find_descendant(&dom, root, "p").expect("p");
        assert_eq!(text_of(&dom, p), "a b");
        // No element anywhere should be named after doctype/comment markers.
        assert!(find_descendant(&dom, root, "!--").is_none());
        assert!(find_descendant(&dom, root, "!doctype").is_none());
    }

    // ------------------------------- attributes --------------------------------

    #[test]
    fn quoted_unquoted_and_empty_attributes() {
        let dom = parse(
            "<input type=\"text\" value=unquoted disabled placeholder='single quoted'>",
        );
        let root = dom.root();
        let input = find_descendant(&dom, root, "input").expect("input");
        let el = dom.node(input).element().unwrap();
        assert_eq!(el.attrs.get("type"), Some("text"));
        assert_eq!(el.attrs.get("value"), Some("unquoted"));
        assert_eq!(el.attrs.get("disabled"), Some(""));
        assert_eq!(el.attrs.get("placeholder"), Some("single quoted"));
    }

    #[test]
    fn attribute_names_fold_to_lowercase_and_first_wins() {
        let dom = parse("<a HREF=\"one\" href=\"two\">x</a>");
        let root = dom.root();
        let a = find_descendant(&dom, root, "a").expect("a");
        let el = dom.node(a).element().unwrap();
        assert_eq!(el.attrs.get("href"), Some("one"));
    }

    // ------------------------------- fixtures ----------------------------------

    fn fixture(name: &str) -> String {
        let path = format!("{}/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading fixture {path}: {e}"))
    }

    #[test]
    fn fixture_basic_html_structure() {
        let src = fixture("basic.html");
        let dom = parse(&src);
        let root = dom.root();
        let head = find_descendant(&dom, root, "head").expect("head");
        let title = find_child(&dom, head, "title").expect("title");
        assert_eq!(text_of(&dom, title), "Basic Fixture");

        let body = find_descendant(&dom, root, "body").expect("body");
        let h1 = find_child(&dom, body, "h1").expect("h1");
        assert_eq!(text_of(&dom, h1), "Welcome");
        let h2 = find_child(&dom, body, "h2").expect("h2");
        assert_eq!(text_of(&dom, h2), "Section One");

        let ps = find_children(&dom, body, "p");
        assert_eq!(ps.len(), 2);
        let a = find_child(&dom, ps[0], "a").expect("link inside first paragraph");
        let el = dom.node(a).element().unwrap();
        assert_eq!(el.attrs.get("href"), Some("https://example.com/"));
        assert_eq!(text_of(&dom, a), "link");
    }

    #[test]
    fn fixture_soup_html_structure() {
        let src = fixture("soup.html");
        let dom = parse(&src);
        let root = dom.root();

        // script fully discarded, even though it contains "<b>" text soup.
        assert!(find_descendant(&dom, root, "script").is_none());

        // style kept with raw (untouched) CSS text.
        let style = find_descendant(&dom, root, "style").expect("style kept");
        assert!(text_of(&dom, style).contains("color: red"));

        let body = find_descendant(&dom, root, "body").expect("body");

        // Two <p>s at the top implied-close each other rather than nesting.
        let top_ps = find_children(&dom, body, "p");
        assert!(top_ps.len() >= 2, "implied-close should have produced sibling <p>s");

        // <li> run implicitly closes.
        let ul = find_descendant(&dom, root, "ul").expect("ul");
        assert_eq!(find_children(&dom, ul, "li").len(), 3);

        // <tr>/<td> implicit closes.
        let table = find_descendant(&dom, root, "table").expect("table");
        let trs = find_children(&dom, table, "tr");
        assert_eq!(trs.len(), 2);
        assert_eq!(find_children(&dom, trs[0], "td").len(), 2);
        assert_eq!(find_children(&dom, trs[1], "td").len(), 2);

        // <dt>/<dd> implicit close.
        let dl = find_descendant(&dom, root, "dl").expect("dl");
        assert!(find_child(&dom, dl, "dt").is_some());
        assert!(find_child(&dom, dl, "dd").is_some());

        // void elements present with no children.
        let img = find_descendant(&dom, root, "img").expect("img");
        assert!(dom.node(img).element().unwrap().children.is_empty());

        // entities decoded somewhere in the document text.
        let body_text = text_of(&dom, body);
        assert!(body_text.contains('\u{00A9}'), "copy entity should decode");
        assert!(body_text.contains('\u{00A0}'), "nbsp entity should decode");
    }
}
