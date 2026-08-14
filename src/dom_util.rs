//! Small DOM-walk helpers shared by the form submission serializer
//! (`form.rs`) and the form-control box-tree renderer
//! (`layout::box_tree`) — factored out here after code review flagged the
//! two independent copies drifting (one used a `depth > CAP` boundary, the
//! other `depth >= CAP`, for what was meant to be the exact same bound).
//! Kept deliberately tiny, dependency-free, and semantics-free (no
//! HTML-submission or rendering opinions live here — just "read attributes/
//! text off the DOM, safely") so both callers can share it without either
//! depending on the other.

use crate::dom::{Dom, Element, Node, NodeId};

/// Maximum descent depth for every recursive walk in this module. Hostile/
/// generated markup can nest form-control descendants (a `<select>`'s
/// `<option>`s, a `<textarea>`/`<button>`/`<option>`'s text content)
/// thousands of levels deep; plain Rust-call recursion with no bound would
/// blow the native stack (a guard-page fault, not a catchable `panic!`, so
/// `panic="abort"` gives no mitigation). Matches `layout::box_tree`'s own
/// `DEPTH_CAP` (which bounds a different recursion axis — the box-tree walk
/// itself, not these form-content helpers) and `layout::block::DEPTH_CAP`.
pub(crate) const DEPTH_CAP: usize = 100;

pub(crate) fn is_checked(el: &Element) -> bool {
    el.attrs.get("checked").is_some()
}

pub(crate) fn is_disabled(el: &Element) -> bool {
    el.attrs.get("disabled").is_some()
}

/// `dom.node(id)`, guarded against an out-of-range `id` — the frozen
/// `Dom`'s own `node()` indexes its arena directly and panics on an OOB
/// index, which IS reachable here: ids threaded through form
/// serialization/rendering (a stale `activator`, a hostile/foreign
/// `NodeId`) aren't guaranteed valid by construction the way a plain
/// DOM-driven walk starting from the root is.
pub(crate) fn node_checked(dom: &Dom, id: NodeId) -> Option<&Node> {
    if id < dom.len() {
        Some(dom.node(id))
    } else {
        None
    }
}

/// One `<option>`: its `value` attribute (if present), its own text content
/// (trimmed), and whether it carries `selected`.
pub(crate) struct OptionInfo {
    pub value: Option<String>,
    pub text: String,
    pub selected: bool,
}

/// Depth-first collect every `<option>` reachable under `el` (direct
/// children in the common case; `<optgroup>`-wrapped options too, since any
/// non-`option` element in between is simply recursed through). Bounded by
/// [`DEPTH_CAP`] against pathological nesting.
pub(crate) fn collect_options(dom: &Dom, el: &Element, depth: usize) -> Vec<OptionInfo> {
    let mut out = Vec::new();
    collect_options_into(dom, el, depth, &mut out);
    out
}

fn collect_options_into(dom: &Dom, el: &Element, depth: usize, out: &mut Vec<OptionInfo>) {
    if depth >= DEPTH_CAP {
        return;
    }
    for &child in &el.children {
        let Some(Node::Element(ce)) = node_checked(dom, child) else { continue };
        if ce.name.as_str() == "option" {
            out.push(OptionInfo {
                value: ce.attrs.get("value").map(|s| s.to_string()),
                text: collect_text(dom, ce).trim().to_string(),
                selected: ce.attrs.get("selected").is_some(),
            });
        } else {
            collect_options_into(dom, ce, depth + 1, out);
        }
    }
}

/// Concatenate every text-node descendant of `el`. `<textarea>` is a
/// RAWTEXT element in this parser's dialect (a single already-decoded
/// `Text` child), so this degenerates to reading that one child in the
/// common case; it also handles `<button>`/`<option>`'s ordinary (possibly
/// mixed-markup) children unchanged. Bounded by [`DEPTH_CAP`].
pub(crate) fn collect_text(dom: &Dom, el: &Element) -> String {
    let mut out = String::new();
    collect_text_into(dom, el, 0, &mut out);
    out
}

fn collect_text_into(dom: &Dom, el: &Element, depth: usize, out: &mut String) {
    if depth >= DEPTH_CAP {
        return;
    }
    for &child in &el.children {
        match node_checked(dom, child) {
            Some(Node::Text(t)) => out.push_str(t),
            Some(Node::Element(e)) => collect_text_into(dom, e, depth + 1, out),
            None => {}
        }
    }
}
