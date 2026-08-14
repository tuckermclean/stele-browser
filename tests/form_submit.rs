//! Contract tests for `stele::form::serialize_submit` (P-forms): HTML 4.01
//! §17.13.2 form submission serialization, pure over a parsed `Dom`. Strict
//! test-first (brief §10 TDD protocol) — committed RED against a `todo!()`/
//! missing `src/form.rs`, then GREEN once the implementation lands.
//!
//! Expected strings below are hand-computed, not derived from running the
//! code under test.

use stele::dom::{self, Dom, Node, NodeId};
use stele::fetch::{Method, Url};
use stele::form::serialize_submit;

// -- tiny DOM-walk helpers (mirrors the style of other test files) ---------

fn find_first(dom: &Dom, tag: &str) -> Option<NodeId> {
    find_all(dom, tag).into_iter().next()
}

fn find_all(dom: &Dom, tag: &str) -> Vec<NodeId> {
    let mut out = Vec::new();
    fn walk(dom: &Dom, id: NodeId, tag: &str, out: &mut Vec<NodeId>) {
        if let Node::Element(el) = dom.node(id) {
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

/// First descendant element named `tag` whose `attr` equals `value`.
fn find_by_attr(dom: &Dom, tag: &str, attr: &str, value: &str) -> Option<NodeId> {
    find_all(dom, tag).into_iter().find(|&id| {
        dom.node(id)
            .element()
            .and_then(|e| e.attrs.get(attr))
            .map(|v| v == value)
            .unwrap_or(false)
    })
}

fn body_str(req: &stele::fetch::Request) -> String {
    String::from_utf8_lossy(&req.body).to_string()
}

// -- 1. GET form with two text inputs ---------------------------------------

#[test]
fn get_form_with_two_text_inputs_appends_query_to_action() {
    let d = dom::parser::parse(
        r#"<form action="/submit" method="get">
            <input type="text" name="a" value="1">
            <input type="text" name="b" value="2">
        </form>"#,
    );
    let form = find_first(&d, "form").expect("form");
    let base = Url::new("http://example.com/page");
    let req = serialize_submit(&d, form, &base, None);

    assert_eq!(req.method, Method::Get);
    assert_eq!(req.url.as_str(), "http://example.com/submit?a=1&b=2");
    assert!(req.body.is_empty(), "GET body must be empty");
}

// -- 2. POST form -------------------------------------------------------------

#[test]
fn post_form_sends_urlencoded_body_with_content_type_header() {
    let d = dom::parser::parse(
        r#"<form action="/submit" method="post">
            <input type="text" name="a" value="1">
            <input type="text" name="b" value="2">
        </form>"#,
    );
    let form = find_first(&d, "form").expect("form");
    let base = Url::new("http://example.com/page");
    let req = serialize_submit(&d, form, &base, None);

    assert_eq!(req.method, Method::Post);
    // Action URL is unchanged for POST -- no query appended.
    assert_eq!(req.url.as_str(), "http://example.com/submit");
    assert_eq!(body_str(&req), "a=1&b=2");
    assert_eq!(
        req.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-type")).map(|(_, v)| v.as_str()),
        Some("application/x-www-form-urlencoded")
    );
}

// -- 3. unnamed / disabled / unchecked controls excluded ---------------------

#[test]
fn unnamed_disabled_and_unchecked_controls_are_excluded() {
    let d = dom::parser::parse(
        r#"<form action="/submit" method="get">
            <input type="text" value="no-name-here">
            <input type="text" name="disabled-field" value="nope" disabled>
            <input type="checkbox" name="unchecked-box" value="x">
            <input type="text" name="kept" value="yes">
        </form>"#,
    );
    let form = find_first(&d, "form").expect("form");
    let base = Url::new("http://example.com/page");
    let req = serialize_submit(&d, form, &base, None);

    assert_eq!(req.url.as_str(), "http://example.com/submit?kept=yes");
}

// -- 4. checkbox checked vs unchecked -----------------------------------------

#[test]
fn checkbox_only_submits_when_checked() {
    let d = dom::parser::parse(
        r#"<form action="/submit" method="get">
            <input type="checkbox" name="subscribe" value="yes" checked>
            <input type="checkbox" name="promo" value="yes">
        </form>"#,
    );
    let form = find_first(&d, "form").expect("form");
    let base = Url::new("http://example.com/page");
    let req = serialize_submit(&d, form, &base, None);

    assert_eq!(req.url.as_str(), "http://example.com/submit?subscribe=yes");
}

#[test]
fn checkbox_with_no_value_attr_defaults_to_on() {
    let d = dom::parser::parse(r#"<form action="/submit"><input type="checkbox" name="agree" checked></form>"#);
    let form = find_first(&d, "form").expect("form");
    let base = Url::new("http://example.com/page");
    let req = serialize_submit(&d, form, &base, None);
    assert_eq!(req.url.as_str(), "http://example.com/submit?agree=on");
}

// -- 5. radio group: only the checked one submits -----------------------------

#[test]
fn radio_group_only_checked_member_submits() {
    let d = dom::parser::parse(
        r#"<form action="/submit" method="get">
            <input type="radio" name="size" value="small">
            <input type="radio" name="size" value="medium" checked>
            <input type="radio" name="size" value="large">
        </form>"#,
    );
    let form = find_first(&d, "form").expect("form");
    let base = Url::new("http://example.com/page");
    let req = serialize_submit(&d, form, &base, None);

    assert_eq!(req.url.as_str(), "http://example.com/submit?size=medium");
}

// -- 6. select: selected option -----------------------------------------------

#[test]
fn select_contributes_selected_option_value() {
    let d = dom::parser::parse(
        r#"<form action="/submit" method="get">
            <select name="color">
                <option value="r">Red</option>
                <option value="g" selected>Green</option>
                <option value="b">Blue</option>
            </select>
        </form>"#,
    );
    let form = find_first(&d, "form").expect("form");
    let base = Url::new("http://example.com/page");
    let req = serialize_submit(&d, form, &base, None);

    assert_eq!(req.url.as_str(), "http://example.com/submit?color=g");
}

#[test]
fn select_with_no_selected_option_defaults_to_first() {
    let d = dom::parser::parse(
        r#"<form action="/submit" method="get">
            <select name="color">
                <option value="r">Red</option>
                <option value="g">Green</option>
            </select>
        </form>"#,
    );
    let form = find_first(&d, "form").expect("form");
    let base = Url::new("http://example.com/page");
    let req = serialize_submit(&d, form, &base, None);

    assert_eq!(req.url.as_str(), "http://example.com/submit?color=r");
}

#[test]
fn select_option_with_no_value_attr_falls_back_to_its_text() {
    let d = dom::parser::parse(
        r#"<form action="/submit" method="get">
            <select name="fruit"><option selected>Apple</option></select>
        </form>"#,
    );
    let form = find_first(&d, "form").expect("form");
    let base = Url::new("http://example.com/page");
    let req = serialize_submit(&d, form, &base, None);

    assert_eq!(req.url.as_str(), "http://example.com/submit?fruit=Apple");
}

// -- 6b. <select multiple>: one pair PER selected option (review fix) --------
//
// HTML 4.01 §17.13.2: a multi-select control contributes one name=value
// pair for EVERY selected option, not just one. The original
// `successful_select` only ever emitted a single pair (`.find(selected).or
// (first)`), silently dropping data for any `<select multiple>` with more
// than one option checked. Single-select behavior (covered above) must stay
// exactly as before.

#[test]
fn multi_select_contributes_one_pair_per_selected_option() {
    let d = dom::parser::parse(
        r#"<form action="/submit" method="get">
            <select name="toppings" multiple>
                <option value="olives" selected>Olives</option>
                <option value="cheese">Cheese</option>
                <option value="mushrooms" selected>Mushrooms</option>
            </select>
        </form>"#,
    );
    let form = find_first(&d, "form").expect("form");
    let base = Url::new("http://example.com/page");
    let req = serialize_submit(&d, form, &base, None);

    assert_eq!(req.url.as_str(), "http://example.com/submit?toppings=olives&toppings=mushrooms");
}

#[test]
fn multi_select_with_nothing_selected_contributes_nothing() {
    let d = dom::parser::parse(
        r#"<form action="/submit" method="get">
            <select name="toppings" multiple>
                <option value="olives">Olives</option>
                <option value="cheese">Cheese</option>
            </select>
        </form>"#,
    );
    let form = find_first(&d, "form").expect("form");
    let base = Url::new("http://example.com/page");
    let req = serialize_submit(&d, form, &base, None);

    assert_eq!(req.url.as_str(), "http://example.com/submit");
}

#[test]
fn single_select_still_contributes_at_most_one_pair_after_multi_select_fix() {
    // Regression guard alongside the multi-select fix: a plain (non-
    // `multiple`) select must still contribute exactly one pair (the
    // selected option, or the first if none is marked selected) even
    // though multiple of its options could technically carry `selected`
    // in hostile markup.
    let d = dom::parser::parse(
        r#"<form action="/submit" method="get">
            <select name="color">
                <option value="r" selected>Red</option>
                <option value="g" selected>Green</option>
            </select>
        </form>"#,
    );
    let form = find_first(&d, "form").expect("form");
    let base = Url::new("http://example.com/page");
    let req = serialize_submit(&d, form, &base, None);

    // Not `multiple`: only the FIRST matching (`selected`) option counts.
    assert_eq!(req.url.as_str(), "http://example.com/submit?color=r");
}

// -- 6c. type=image: documented v0 simplification (review fix) ---------------
//
// HTML4 §17.13.2 says an image-button submit contributes click coordinates
// (`name.x`/`name.y`), meaningful only for a mouse-driven, JS-capable
// browser. This is a no-mouse, no-JavaScript, static-document browser: there
// is no click point to report. Per the packet review, `type=image` is
// treated exactly like `type=submit` -- a plain `name=value` pair via the
// activator -- rather than synthesizing fake `.x`/`.y` coordinates.

#[test]
fn image_submit_contributes_plain_name_value_not_fake_coordinates() {
    let d = dom::parser::parse(
        r#"<form action="/submit" method="get"><input type="image" name="go" value="Go" src="go.png"></form>"#,
    );
    let form = find_first(&d, "form").expect("form");
    let img_input = find_by_attr(&d, "input", "name", "go").expect("image input");
    let base = Url::new("http://example.com/page");

    let req = serialize_submit(&d, form, &base, Some(img_input));
    assert_eq!(req.url.as_str(), "http://example.com/submit?go=Go");
    assert!(!req.url.as_str().contains(".x="), "must not synthesize fake click coordinates");
    assert!(!req.url.as_str().contains(".y="), "must not synthesize fake click coordinates");

    // Not the activator: contributes nothing, exactly like type=submit.
    let req_none = serialize_submit(&d, form, &base, None);
    assert_eq!(req_none.url.as_str(), "http://example.com/submit");
}

// -- 7. textarea content --------------------------------------------------------

#[test]
fn textarea_contributes_its_text_content() {
    let d = dom::parser::parse(
        r#"<form action="/submit" method="get"><textarea name="notes">hello world</textarea></form>"#,
    );
    let form = find_first(&d, "form").expect("form");
    let base = Url::new("http://example.com/page");
    let req = serialize_submit(&d, form, &base, None);

    assert_eq!(req.url.as_str(), "http://example.com/submit?notes=hello+world");
}

// -- 8. submit button: only the activator submits ------------------------------

#[test]
fn submit_button_included_only_when_it_is_the_activator() {
    let d = dom::parser::parse(
        r#"<form action="/submit" method="get">
            <input type="text" name="q" value="hi">
            <input type="submit" name="go" value="Go">
            <input type="submit" name="cancel" value="Cancel">
        </form>"#,
    );
    let form = find_first(&d, "form").expect("form");
    let go = find_by_attr(&d, "input", "name", "go").expect("go button");
    let base = Url::new("http://example.com/page");

    let req = serialize_submit(&d, form, &base, Some(go));
    assert_eq!(req.url.as_str(), "http://example.com/submit?q=hi&go=Go");

    // No activator at all: neither submit button contributes.
    let req_none = serialize_submit(&d, form, &base, None);
    assert_eq!(req_none.url.as_str(), "http://example.com/submit?q=hi");
}

#[test]
fn button_element_included_only_when_it_is_the_activator() {
    let d = dom::parser::parse(
        r#"<form action="/submit" method="get">
            <button type="submit" name="act" value="save">Save</button>
        </form>"#,
    );
    let form = find_first(&d, "form").expect("form");
    let btn = find_first(&d, "button").expect("button");
    let base = Url::new("http://example.com/page");

    let req = serialize_submit(&d, form, &base, Some(btn));
    assert_eq!(req.url.as_str(), "http://example.com/submit?act=save");

    let req_none = serialize_submit(&d, form, &base, None);
    assert_eq!(req_none.url.as_str(), "http://example.com/submit");
}

// -- 9. percent-encoding --------------------------------------------------------

#[test]
fn percent_encoding_of_spaces_reserved_and_unicode() {
    // Rust string-literal `\u{...}` escapes are interpreted at compile time,
    // producing the real UTF-8 bytes in the HTML source the parser sees.
    let d = dom::parser::parse(
        "<form action=\"/submit\" method=\"get\">\
            <input type=\"text\" name=\"q\" value=\"a b&amp;c=d/e\">\
            <input type=\"text\" name=\"u\" value=\"caf\u{e9} \u{65e5}\">\
        </form>",
    );
    let form = find_first(&d, "form").expect("form");
    let base = Url::new("http://example.com/page");
    let req = serialize_submit(&d, form, &base, None);

    // space -> '+', '&' -> %26, '=' -> %3D, '/' -> %2F; 'é' and '日' are
    // multi-byte UTF-8, each byte percent-encoded uppercase-hex.
    assert_eq!(
        req.url.as_str(),
        "http://example.com/submit?q=a+b%26c%3Dd%2Fe&u=caf%C3%A9+%E6%97%A5"
    );
}

#[test]
fn percent_encoding_of_name_too() {
    let d = dom::parser::parse(
        r#"<form action="/submit" method="get"><input type="text" name="a b" value="1"></form>"#,
    );
    let form = find_first(&d, "form").expect("form");
    let base = Url::new("http://example.com/page");
    let req = serialize_submit(&d, form, &base, None);
    assert_eq!(req.url.as_str(), "http://example.com/submit?a+b=1");
}

// -- 10. empty form -> bare action -----------------------------------------------

#[test]
fn empty_form_submits_to_bare_action_with_no_query() {
    let d = dom::parser::parse(r#"<form action="/submit" method="get"></form>"#);
    let form = find_first(&d, "form").expect("form");
    let base = Url::new("http://example.com/page");
    let req = serialize_submit(&d, form, &base, None);
    assert_eq!(req.url.as_str(), "http://example.com/submit");
    assert!(req.body.is_empty());
}

#[test]
fn form_with_no_controls_at_all_but_existing_action_query_strips_it_on_get() {
    // "replacing any existing query" -- a GET submit with zero successful
    // controls must not leave a stale "?..." from the action attribute.
    let d = dom::parser::parse(r#"<form action="/submit?stale=1" method="get"></form>"#);
    let form = find_first(&d, "form").expect("form");
    let base = Url::new("http://example.com/page");
    let req = serialize_submit(&d, form, &base, None);
    assert_eq!(req.url.as_str(), "http://example.com/submit");
}

#[test]
fn get_with_existing_action_query_and_named_controls_replaces_not_appends() {
    // Same "replacing any existing query" rule as the zero-controls case
    // above, but with real successful controls this time -- the stale
    // `?existing=1` from the `action` attribute must be fully replaced by
    // the submitted pairs, not left dangling ahead of/behind them.
    let d = dom::parser::parse(
        r#"<form action="/s?existing=1" method="get">
            <input type="text" name="a" value="1">
            <input type="text" name="b" value="2">
        </form>"#,
    );
    let form = find_first(&d, "form").expect("form");
    let base = Url::new("http://example.com/page");
    let req = serialize_submit(&d, form, &base, None);
    assert_eq!(req.url.as_str(), "http://example.com/s?a=1&b=2");
}

// -- totality / defaults ----------------------------------------------------

#[test]
fn missing_action_submits_to_base_itself() {
    let d = dom::parser::parse(r#"<form method="get"><input type="text" name="a" value="1"></form>"#);
    let form = find_first(&d, "form").expect("form");
    let base = Url::new("http://example.com/page");
    let req = serialize_submit(&d, form, &base, None);
    assert_eq!(req.url.as_str(), "http://example.com/page?a=1");
}

#[test]
fn missing_method_defaults_to_get() {
    let d = dom::parser::parse(r#"<form action="/submit"><input type="text" name="a" value="1"></form>"#);
    let form = find_first(&d, "form").expect("form");
    let base = Url::new("http://example.com/page");
    let req = serialize_submit(&d, form, &base, None);
    assert_eq!(req.method, Method::Get);
}

#[test]
fn reset_and_button_type_inputs_never_submit() {
    let d = dom::parser::parse(
        r#"<form action="/submit" method="get">
            <input type="reset" name="reset-me" value="Reset">
            <input type="button" name="click-me" value="Click">
            <input type="text" name="kept" value="ok">
        </form>"#,
    );
    let form = find_first(&d, "form").expect("form");
    let base = Url::new("http://example.com/page");
    let req = serialize_submit(&d, form, &base, None);
    assert_eq!(req.url.as_str(), "http://example.com/submit?kept=ok");
}

#[test]
fn file_input_contributes_filename_only_no_multipart() {
    // v0 simplification: no real file-picker exists in a static-document
    // browser, so this exercises the degenerate but still-total path: a
    // `value` attribute present is passed through as the "filename" (real
    // browsers never let `value` be author-settable on `type=file` for
    // security reasons, so this branch is defensive/documented, not a
    // real upload feature).
    let d = dom::parser::parse(
        r#"<form action="/submit" method="get"><input type="file" name="upload" value="report.txt"></form>"#,
    );
    let form = find_first(&d, "form").expect("form");
    let base = Url::new("http://example.com/page");
    let req = serialize_submit(&d, form, &base, None);
    assert_eq!(req.url.as_str(), "http://example.com/submit?upload=report.txt");
}

#[test]
fn hostile_deeply_nested_controls_never_panics() {
    let mut html = String::from(r#"<form action="/submit" method="get">"#);
    for _ in 0..3000 {
        html.push_str("<div>");
    }
    html.push_str(r#"<input type="text" name="deep" value="x">"#);
    for _ in 0..3000 {
        html.push_str("</div>");
    }
    html.push_str("</form>");
    let d = dom::parser::parse(&html);
    let form = find_first(&d, "form").expect("form");
    let base = Url::new("http://example.com/page");
    let _req = serialize_submit(&d, form, &base, None); // must not panic
}

#[test]
fn out_of_range_form_id_and_activator_never_panic() {
    let d = dom::parser::parse(r#"<form action="/submit" method="get"></form>"#);
    let base = Url::new("http://example.com/page");
    let bogus_id: NodeId = 999_999;
    let req = serialize_submit(&d, bogus_id, &base, Some(bogus_id));
    // No form found at all: total fallback is a bare GET to base.
    assert_eq!(req.method, Method::Get);
    assert_eq!(req.url.as_str(), "http://example.com/page");
}
