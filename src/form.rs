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
//! attribute if present, else the option's own text). A plain (non-
//! `multiple`) select contributes at most one pair: the selected option, or
//! the first `<option>` when none is marked `selected` (matching every real
//! browser's behavior, not the spec's silence on the point). A `<select
//! multiple>` is genuinely multi-valued per §17.13.2: it contributes ONE
//! pair per selected `<option>`, in document order — zero pairs if nothing
//! is selected (no "fall back to the first option" default for a multi-
//! select; that default only exists for the single-select case, where
//! *something* must be chosen). `<textarea>` contributes its raw text
//! content. Plain text/password/hidden/etc. `<input>`s contribute their
//! `value` attribute (empty string if absent).
//!
//! ## `type=image` (v0 simplification — DECISIONS-worthy)
//!
//! HTML4 §17.13.2 says an image-button submit contributes CLICK
//! COORDINATES (`name.x`/`name.y`), not a `name=value` pair — meaningful
//! only for a mouse-driven, JavaScript-capable browser where a user
//! actually clicks a pixel on the rendered image. This is a no-mouse,
//! no-JavaScript, static-document browser (charter C3): there is no click
//! point to report, and synthesizing a fake `name.x=0&name.y=0` pair would
//! be worse than doing nothing special — it would look like real
//! coordinate data to a server with no way to tell it was fabricated. So
//! `type=image` is treated exactly like `type=submit` here: a plain
//! `name=value` pair from its `value` attribute, contributed only when it
//! is the `activator`. This is a deliberate, documented v0 simplification
//! (flagged in code review, pinned by a dedicated test), not spec-compliant
//! coordinate reporting — the same spirit as the `type=file` simplification
//! below (a capability the browser's own no-mouse/no-JS nature makes moot).
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
//! `name`s, no `action`, deeply nested control markup (the descendant walk
//! is bounded by [`dom_util::DEPTH_CAP`], shared with `layout::box_tree`'s
//! own form-content helpers and mirroring `layout::block::DEPTH_CAP`'s
//! rationale — hostile/generated markup can nest thousands of levels deep,
//! which would blow the native call stack under plain recursion) all
//! degrade to a well-formed `Request`, never a panic — `panic = "abort"`
//! gives no safety net, so this must hold for any input, not just
//! well-formed forms.

use crate::dom::{Dom, Element, Node, NodeId};
use crate::dom_util;
use crate::fetch::{Method, Request, Url};

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
    serialize_submit_with_overrides(dom, form_id, base, activator, &[])
}

/// Same as [`serialize_submit`], but for a plain-value `<input>` control
/// (text/password/hidden/email/url/tel/number/search/unrecognized — the
/// same catch-all arm [`successful_input`] already reads `value` from for
/// every OTHER caller) whose `NodeId` appears in `overrides`, use the
/// paired `String` instead of reading the DOM `value` attribute. This is
/// the seam the interactive shell (`browser::apply_key`'s edit-mode
/// handling) uses to submit what the user actually TYPED into a text
/// field, without this module needing to know anything about `ViewState`s,
/// edit buffers, or key handling — it only ever sees a plain `NodeId ->
/// String` association. An override for a `NodeId` this walk never visits
/// (wrong form, or a control kind that doesn't read `value` at all —
/// checkbox/radio/submit/image/file all keep their own dedicated rules) is
/// simply never consulted, not an error. `overrides` is a slice (not a
/// map): callers only ever have a handful of edited fields, so a linear
/// scan per plain-value control is simpler than threading a `HashMap`
/// through for no measurable benefit.
pub fn serialize_submit_with_overrides(dom: &Dom, form_id: NodeId, base: &Url, activator: Option<NodeId>, overrides: &[(NodeId, String)]) -> Request {
    let (action_attr, method) = form_attrs(dom, form_id);

    let action_url = match action_attr {
        Some(a) if !a.is_empty() => base.resolve(&a),
        _ => base.clone(),
    };

    let mut pairs: Vec<(String, String)> = Vec::new();
    if let Some(Node::Element(form_el)) = dom_util::node_checked(dom, form_id) {
        for &child in &form_el.children {
            walk_controls(dom, child, 0, activator, overrides, &mut pairs);
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
    match dom_util::node_checked(dom, form_id) {
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

/// Walk `id`'s subtree collecting successful-control `(name, value)` pairs
/// in document order. Recognized control elements (`input`/`textarea`/
/// `select`/`button`) are leaves of this walk — their own subtrees are
/// never searched for further "nested" controls (not valid HTML4 content
/// anyway; a hostile document nesting e.g. `<input>` inside `<button>`
/// degrades to just the outer control being considered, a documented v0
/// simplification). Every other element (`div`, `fieldset`, `label`, `p`,
/// `table`, ...) is transparent: its children are walked in turn.
fn walk_controls(dom: &Dom, id: NodeId, depth: usize, activator: Option<NodeId>, overrides: &[(NodeId, String)], out: &mut Vec<(String, String)>) {
    if depth >= dom_util::DEPTH_CAP {
        return;
    }
    let Some(Node::Element(el)) = dom_util::node_checked(dom, id) else { return };
    match el.name.as_str() {
        "input" => {
            if let Some(pair) = successful_input(el, id, activator, overrides) {
                out.push(pair);
            }
        }
        "textarea" => {
            if let Some(pair) = successful_textarea(dom, el) {
                out.push(pair);
            }
        }
        "select" => out.extend(successful_select(dom, el)),
        "button" => {
            if let Some(pair) = successful_button(el, id, activator) {
                out.push(pair);
            }
        }
        _ => {
            for &child in &el.children {
                walk_controls(dom, child, depth + 1, activator, overrides, out);
            }
        }
    }
}

/// Linear-scan `overrides` for `id` — see [`serialize_submit_with_overrides`]'s
/// own doc comment for why a slice, not a map.
fn find_override(overrides: &[(NodeId, String)], id: NodeId) -> Option<&str> {
    overrides.iter().find(|(nid, _)| *nid == id).map(|(_, v)| v.as_str())
}

/// `<input>` is a void element (`dom::parser::VOID_ELEMENTS`) — it never has
/// children, so this is a pure attribute read, no subtree walk needed.
fn successful_input(el: &Element, id: NodeId, activator: Option<NodeId>, overrides: &[(NodeId, String)]) -> Option<(String, String)> {
    if dom_util::is_disabled(el) {
        return None;
    }
    let name = el.attrs.get("name")?;
    let ty = el.attrs.get("type").unwrap_or("text").to_ascii_lowercase();
    match ty.as_str() {
        "reset" | "button" => None,
        "checkbox" | "radio" => {
            if dom_util::is_checked(el) {
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
        // v0 simplification: no click coordinates (no mouse, no JS) — see
        // the module docs' "type=image" section. Treated exactly like
        // type=submit.
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
        // purposes: the raw `value` attribute, defaulting to empty -- UNLESS
        // an override (a user-typed edit buffer, from the interactive
        // shell) names this exact control, in which case that wins.
        _ => {
            let value = find_override(overrides, id).map(|v| v.to_string()).unwrap_or_else(|| el.attrs.get("value").unwrap_or("").to_string());
            Some((name.to_string(), value))
        }
    }
}

fn successful_textarea(dom: &Dom, el: &Element) -> Option<(String, String)> {
    if dom_util::is_disabled(el) {
        return None;
    }
    let name = el.attrs.get("name")?;
    Some((name.to_string(), dom_util::collect_text(dom, el)))
}

/// `<select>` contributes ONE pair per selected `<option>` when it carries
/// the `multiple` attribute (HTML4 §17.13.2 — a multi-select is genuinely
/// multi-valued, unlike every other control this module handles; review
/// caught the original single-pair implementation silently dropping every
/// selected option past the first), and at most one pair otherwise (the
/// single selected option, defaulting to the first option when none is
/// marked `selected`, matching every real browser). A `multiple` select
/// with nothing selected contributes nothing at all — unlike the single-
/// select case, there is no "fall back to the first option" default here
/// (a multi-select can legitimately submit zero values; a single-select
/// cannot, since *some* option is always the browser's rendered choice).
fn successful_select(dom: &Dom, el: &Element) -> Vec<(String, String)> {
    if dom_util::is_disabled(el) {
        return Vec::new();
    }
    let Some(name) = el.attrs.get("name") else { return Vec::new() };
    let options = dom_util::collect_options(dom, el, 0);
    if el.attrs.get("multiple").is_some() {
        options
            .iter()
            .filter(|o| o.selected)
            .map(|o| (name.to_string(), o.value.clone().unwrap_or_else(|| o.text.clone())))
            .collect()
    } else {
        let chosen = options.iter().find(|o| o.selected).or_else(|| options.first());
        match chosen {
            Some(o) => vec![(name.to_string(), o.value.clone().unwrap_or_else(|| o.text.clone()))],
            None => Vec::new(),
        }
    }
}

/// `<button>`'s default `type` (when the attribute is absent) is `submit`
/// per HTML4/HTML5 — only an explicit non-`submit` type opts out.
fn successful_button(el: &Element, id: NodeId, activator: Option<NodeId>) -> Option<(String, String)> {
    if dom_util::is_disabled(el) {
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
