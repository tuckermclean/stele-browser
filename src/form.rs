//! Form submission serialization (P-forms): a pure function that turns a
//! `<form>` element in a parsed [`Dom`] into an outbound [`fetch::Request`],
//! implementing the "successful controls" algorithm of HTML 4.01 §17.13.2.
//!
//! This module is deliberately IO-free and side-effect-free: given a `Dom`,
//! a form's `NodeId`, a base `Url` to resolve `action` against, and which
//! control (if any) triggered the submit, it returns a `Request` a caller
//! (a future interactive-browser packet) can hand to `fetch::Fetch`. No
//! network, no mutation of the DOM, nothing that could panic on adversarial
//! input — see the module's totality notes below.
//!
//! ## Successful controls (HTML 4.01 §17.13.2)
//!
//! A control contributes a `name=value` pair only if:
//!   - it has a `name` attribute (any value, including empty — HTML4 only
//!     requires the attribute be *present*);
//!   - it is not `disabled`;
//!   - for `checkbox`/`radio`: it is `checked` (an unchecked box/radio, or
//!     an unchecked member of a same-named radio group, contributes
//!     nothing — this falls out of the same per-control check, no separate
//!     grouping logic is needed);
//!   - for `type=submit`/`type=image`/`<button type=submit>`: it is the
//!     `activator` — the one control that actually triggered this submit
//!     (a form can have several submit buttons; only the one that was
//!     activated contributes its name=value, matching every real browser);
//!   - `type=reset`/`type=button` (on `<input>` or `<button>`) never
//!     contribute, activator or not — they don't submit forms at all.
//!
//! `<select>` contributes its selected `<option>`'s value (the `value`
//! attribute if present, else the option's own text), defaulting to the
//! first `<option>` when none is marked `selected` (matching every real
//! browser's behavior, not the spec's silence on the point). `<textarea>`
//! contributes its raw text content. Plain text/password/hidden/etc.
//! `<input>`s contribute their `value` attribute (empty string if absent).
//!
//! ## `type=file` (v0 simplification — DECISIONS-worthy)
//!
//! Real multipart/form-data file upload needs an actual file-picker and a
//! multipart body encoder, neither of which exists in a static-document,
//! script-free browser (there is no user gesture that could ever populate
//! a file input with real bytes). Per the packet brief, `type=file` here
//! sends **the filename only** — whatever the `value` attribute holds — as
//! a plain urlencoded field, never as `multipart/form-data`. Since real
//! browsers refuse to let authors set `value` on a file input at all (for
//! the obvious security reason), this branch is mostly dead code on any
//! real-world document; it exists so a hostile/synthetic one still gets a
//! well-formed, total `Request` rather than being special-cased into a
//! panic.
//!
//! ## Encoding
//!
//! `application/x-www-form-urlencoded` per the classic (pre-WHATWG,
//! RFC 1866-era — the era this browser targets) rule: space becomes `+`,
//! `[A-Za-z0-9._-]` pass through unescaped, everything else (space's own
//! `+` substitution aside) is percent-escaped byte-by-byte from the UTF-8
//! encoding — total over any `str`, including malformed-looking-but-valid
//! Unicode, since Rust's `str` is always valid UTF-8 by construction.
//!
//! ## GET vs POST
//!
//! `GET` (the HTML4 default when `method` is missing/unrecognized) appends
//! `?<query>` to the resolved `action` URL, REPLACING any query the
//! `action` attribute itself already carried (even down to no `?` at all,
//! for a form with zero successful controls) — body is empty. `POST` sends
//! the encoded pairs as the request body against the resolved `action`
//! URL unchanged, with a `Content-Type: application/x-www-form-urlencoded`
//! header. A missing/empty `action` submits back to `base` itself.
//!
//! ## Totality
//!
//! Every helper here is total: out-of-range `NodeId`s (a stale/foreign id
//! handed to `form_id`/`activator`), a form with no controls at all, no
//! `name`s, no `action`, deeply nested control markup (a `CONTROL_DEPTH_CAP`
//! bounds the descendant walk, mirroring `layout::box_tree::DEPTH_CAP` and
//! `layout::block::DEPTH_CAP`'s rationale — hostile/generated markup can
//! nest thousands of levels deep, which would blow the native call stack
//! under plain recursion) all degrade to a well-formed `Request`, never a
//! panic — `panic = "abort"` gives no safety net, so this must hold for any
//! input, not just well-formed forms.

use crate::dom::{Dom, Element, Node, NodeId};
use crate::fetch::{Method, Request, Url};

/// Maximum descent depth for the form's own control walk, and for the
/// (much shallower in practice) text/option collection helpers below.
/// Mirrors `layout::box_tree::DEPTH_CAP` / `layout::block::DEPTH_CAP`'s
/// rationale: this crate's own recursive walk has no built-in bound, and a
/// hostile/generated document can nest markup thousands of levels deep —
/// well past the point plain Rust-call recursion would blow the native
/// stack (a guard-page fault, not a catchable `panic!`, so `panic="abort"`
/// gives no mitigation). Past the cap, descent simply stops — a
/// pathologically deep subtree contributes no further controls rather than
/// crashing the process.
const CONTROL_DEPTH_CAP: usize = 100;

/// Build the [`Request`] a submission of `form_id` (in `dom`) would send.
///
/// `base` is the document's own URL (used both to resolve a relative/
/// missing `action` and as the fallback destination when `form_id` doesn't
/// resolve to a real `<form>` element at all). `activator` is the `NodeId`
/// of the specific submit control (an `<input type=submit|image>` or
/// `<button type=submit>`) that triggered this submission, if any — only
/// that one control's name=value is included, matching real browsers.
///
/// Total on any `dom`/`form_id`/`activator` combination — see the module
/// docs' Totality section.
pub fn serialize_submit(dom: &Dom, form_id: NodeId, base: &Url, activator: Option<NodeId>) -> Request {
    let (action_attr, method) = form_attrs(dom, form_id);

    let action_url = match action_attr {
        Some(a) if !a.is_empty() => base.resolve(&a),
        _ => base.clone(),
    };

    let mut pairs: Vec<(String, String)> = Vec::new();
    if let Some(Node::Element(form_el)) = dom_node_checked(dom, form_id) {
        for &child in &form_el.children {
            walk_controls(dom, child, 0, activator, &mut pairs);
        }
    }
    let query = build_query(&pairs);

    match method {
        Method::Get => Request::get(replace_query(&action_url, &query)),
        Method::Post => {
            let mut req = Request::get(action_url);
            req.method = Method::Post;
            req.body = query.into_bytes();
            req.headers.push(("Content-Type".to_string(), "application/x-www-form-urlencoded".to_string()));
            req
        }
    }
}

/// This form's `action` attribute (raw, unresolved) and its `method`
/// (`Post` only for a literal case-insensitive `"post"`; anything else,
/// including absent, is `Get` — HTML4's own default). `form_id` out of
/// range, or not actually an element, degrades to `(None, Method::Get)`
/// rather than panicking.
fn form_attrs(dom: &Dom, form_id: NodeId) -> (Option<String>, Method) {
    match dom_node_checked(dom, form_id) {
        Some(Node::Element(el)) => {
            let action = el.attrs.get("action").map(|s| s.to_string());
            let method = match el.attrs.get("method") {
                Some(m) if m.eq_ignore_ascii_case("post") => Method::Post,
                _ => Method::Get,
            };
            (action, method)
        }
        _ => (None, Method::Get),
    }
}

/// `dom.node(id)`, guarded against an out-of-range `id` (the frozen `Dom`'s
/// own `node()` indexes its arena directly and panics on an OOB index — a
/// reachable case here since `form_id`/descendant ids threaded through this
/// module aren't guaranteed valid by construction the way `box_tree`'s
/// DOM-driven walk is).
fn dom_node_checked(dom: &Dom, id: NodeId) -> Option<&Node> {
    if id < dom.len() {
        Some(dom.node(id))
    } else {
        None
    }
}

/// Walk `id`'s subtree collecting successful-control `(name, value)` pairs
/// in document order. Recognized control elements (`input`/`textarea`/
/// `select`/`button`) are leaves of this walk — their own subtrees are
/// never searched for further "nested" controls (not valid HTML4 content
/// anyway; a hostile document nesting e.g. `<input>` inside `<button>`
/// degrades to just the outer control being considered, a documented v0
/// simplification). Every other element (`div`, `fieldset`, `label`, `p`,
/// `table`, ...) is transparent: its children are walked in turn.
fn walk_controls(dom: &Dom, id: NodeId, depth: usize, activator: Option<NodeId>, out: &mut Vec<(String, String)>) {
    if depth > CONTROL_DEPTH_CAP {
        return;
    }
    let Some(Node::Element(el)) = dom_node_checked(dom, id) else { return };
    match el.name.as_str() {
        "input" => {
            if let Some(pair) = successful_input(el, id, activator) {
                out.push(pair);
            }
        }
        "textarea" => {
            if let Some(pair) = successful_textarea(dom, el) {
                out.push(pair);
            }
        }
        "select" => {
            if let Some(pair) = successful_select(dom, el) {
                out.push(pair);
            }
        }
        "button" => {
            if let Some(pair) = successful_button(el, id, activator) {
                out.push(pair);
            }
        }
        _ => {
            for &child in &el.children {
                walk_controls(dom, child, depth + 1, activator, out);
            }
        }
    }
}

fn is_disabled(el: &Element) -> bool {
    el.attrs.get("disabled").is_some()
}

fn is_checked(el: &Element) -> bool {
    el.attrs.get("checked").is_some()
}

/// `<input>` is a void element (`dom::parser::VOID_ELEMENTS`) — it never has
/// children, so this is a pure attribute read, no subtree walk needed.
fn successful_input(el: &Element, id: NodeId, activator: Option<NodeId>) -> Option<(String, String)> {
    if is_disabled(el) {
        return None;
    }
    let name = el.attrs.get("name")?;
    let ty = el.attrs.get("type").unwrap_or("text").to_ascii_lowercase();
    match ty.as_str() {
        "reset" | "button" => None,
        "checkbox" | "radio" => {
            if is_checked(el) {
                // Real browsers default an absent `value` to "on" for
                // checkbox/radio (not literally in the HTML4 text, but
                // universal practice — an author who cares about the value
                // always sets one, so this only matters for the rare
                // valueless case, which still needs *some* defined value).
                Some((name.to_string(), el.attrs.get("value").unwrap_or("on").to_string()))
            } else {
                None
            }
        }
        "submit" | "image" => {
            if Some(id) == activator {
                Some((name.to_string(), el.attrs.get("value").unwrap_or("").to_string()))
            } else {
                None
            }
        }
        // v0 simplification: filename only, no multipart body — see the
        // module docs' "type=file" section.
        "file" => Some((name.to_string(), el.attrs.get("value").unwrap_or("").to_string())),
        // text, password, hidden, email, url, tel, number, search, and any
        // unrecognized/future type all behave the same for submission
        // purposes: the raw `value` attribute, defaulting to empty.
        _ => Some((name.to_string(), el.attrs.get("value").unwrap_or("").to_string())),
    }
}

fn successful_textarea(dom: &Dom, el: &Element) -> Option<(String, String)> {
    if is_disabled(el) {
        return None;
    }
    let name = el.attrs.get("name")?;
    Some((name.to_string(), collect_text(dom, el)))
}

fn successful_select(dom: &Dom, el: &Element) -> Option<(String, String)> {
    if is_disabled(el) {
        return None;
    }
    let name = el.attrs.get("name")?;
    let options = collect_options(dom, el, 0);
    let chosen = options.iter().find(|o| o.selected).or_else(|| options.first())?;
    let value = chosen.value.clone().unwrap_or_else(|| chosen.text.clone());
    Some((name.to_string(), value))
}

/// `<button>`'s default `type` (when the attribute is absent) is `submit`
/// per HTML4/HTML5 — only an explicit non-`submit` type opts out.
fn successful_button(el: &Element, id: NodeId, activator: Option<NodeId>) -> Option<(String, String)> {
    if is_disabled(el) {
        return None;
    }
    let ty = el.attrs.get("type").map(|s| s.to_ascii_lowercase()).unwrap_or_else(|| "submit".to_string());
    if ty != "submit" {
        return None;
    }
    let name = el.attrs.get("name")?;
    if Some(id) != activator {
        return None;
    }
    Some((name.to_string(), el.attrs.get("value").unwrap_or("").to_string()))
}

struct OptionInfo {
    value: Option<String>,
    text: String,
    selected: bool,
}

/// Depth-first collect every `<option>` reachable under `el` (direct
/// children in the common case; `<optgroup>`-wrapped options too, since
/// this simply recurses through any non-`option` element it meets) —
/// bounded by [`CONTROL_DEPTH_CAP`] against pathological nesting.
fn collect_options(dom: &Dom, el: &Element, depth: usize) -> Vec<OptionInfo> {
    let mut out = Vec::new();
    collect_options_into(dom, el, depth, &mut out);
    out
}

fn collect_options_into(dom: &Dom, el: &Element, depth: usize, out: &mut Vec<OptionInfo>) {
    if depth > CONTROL_DEPTH_CAP {
        return;
    }
    for &child in &el.children {
        let Some(Node::Element(ce)) = dom_node_checked(dom, child) else { continue };
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

/// Concatenate every text-node descendant of `el`, depth-bounded by
/// [`CONTROL_DEPTH_CAP`]. `<textarea>` is a RAWTEXT element in this
/// parser's dialect (a single already-decoded `Text` child), so this
/// degenerates to reading that one child for the common case; it also
/// works unchanged for `<option>`/`<button>`'s ordinary (possibly
/// mixed-markup) children.
fn collect_text(dom: &Dom, el: &Element) -> String {
    let mut out = String::new();
    collect_text_into(dom, el, 0, &mut out);
    out
}

fn collect_text_into(dom: &Dom, el: &Element, depth: usize, out: &mut String) {
    if depth > CONTROL_DEPTH_CAP {
        return;
    }
    for &child in &el.children {
        match dom_node_checked(dom, child) {
            Some(Node::Text(t)) => out.push_str(t),
            Some(Node::Element(e)) => collect_text_into(dom, e, depth + 1, out),
            None => {}
        }
    }
}

/// Join `pairs` into one `application/x-www-form-urlencoded` query string
/// (no leading `?`), both name and value percent-encoded.
fn build_query(pairs: &[(String, String)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", encode_www_form(k), encode_www_form(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Percent-encode one component of an `application/x-www-form-urlencoded`
/// body: `[A-Za-z0-9._-]` pass through unescaped, a literal space becomes
/// `+`, everything else is escaped `%XX` (uppercase hex) byte-by-byte over
/// the UTF-8 encoding. Total over any `&str` (Rust strings are always valid
/// UTF-8, so there is no "invalid byte" case to handle) — bytes, not
/// `char`s, are what get escaped, so a multi-byte scalar becomes several
/// `%XX` triples in a row, exactly matching every real implementation.
fn encode_www_form(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// Return `url` with its query string replaced by `query` (or removed
/// entirely, if `query` is empty) — string surgery on the URL's own textual
/// form rather than a full reparse/rebuild, so this works even for a `url`
/// whose shape `UrlParts` can't fully reconstruct (e.g. a schemeless/opaque
/// one) as long as it's well-formed enough to contain at most one `?`.
/// Fragments never appear here: every `Url` this module ever resolves goes
/// through `Url::resolve`, which never retains one (see `fetch::url`'s own
/// docs).
fn replace_query(url: &Url, query: &str) -> Url {
    let s = url.as_str();
    let base = match s.find('?') {
        Some(i) => &s[..i],
        None => s,
    };
    if query.is_empty() {
        Url::new(base.to_string())
    } else {
        Url::new(format!("{}?{}", base, query))
    }
}
