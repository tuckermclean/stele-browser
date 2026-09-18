//! `view-source:<url>` scheme (packet/view-source, charter C7 teaching
//! keys): re-fetches whatever URL follows the `view-source:` prefix through
//! the normal scheme table (`fetch::fetch`) and renders its RAW bytes as
//! escaped text inside a single `<pre>`, instead of parsing them as markup.
//! This is the cheapest of the three C7 surfaces (view-source, Provenance,
//! Transcript) because it needs no new engine plumbing — it reuses real
//! `white-space: pre` layout (`style/ua.rs`'s UA rule for `<pre>`, enforced
//! by `layout::inline`), the same mechanism `<pre>` already gets for any
//! other document.
//!
//! Unlike `about::fetch`, this is NOT total: the wrapped URL might name an
//! unsupported scheme or a network/file error, and that failure is exactly
//! as real for `view-source:<url>` as it is for `<url>` itself — so it
//! propagates the inner `FetchError` unchanged rather than manufacturing a
//! synthetic page, matching how every other scheme in `fetch::fetch`
//! behaves.
use super::{fetch as dispatch_fetch, FetchError, Request, Response, Url};

/// Render `body` (arbitrary bytes, not assumed to be valid UTF-8 or even
/// text) as one escaped `<pre>` block. Lossy UTF-8 decoding matches how the
/// rest of the pipeline already treats fetched bodies (`main.rs`'s
/// `String::from_utf8_lossy(&response.body)` at every `dump_text`/`dump_png`
/// call site) — never a panic on hostile/binary input.
fn render_source(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let mut out = String::with_capacity(text.len() + 32);
    out.push_str("<!DOCTYPE html><html><body><pre>");
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out.push_str("</pre></body></html>");
    out
}

/// `view-source:` with nothing (or nothing but whitespace) after it: rather
/// than dispatching an empty-scheme `Url` into `fetch::fetch` (which would
/// misparse `""` as a schemeless opaque path), treat it as the same shape
/// of failure as any other malformed navigation target.
pub fn fetch(request: &Request) -> Result<Response, FetchError> {
    // Split on the FIRST `:` rather than a literal `"view-source:"` prefix
    // match, so this stays correct regardless of how the scheme itself was
    // cased on the way in (dispatch in `fetch::fetch` already routed here
    // via `Url::scheme()`'s case-insensitive lowercasing).
    let raw = request.url.as_str();
    let wrapped = raw.split_once(':').map(|(_, rest)| rest).unwrap_or("").trim();
    if wrapped.is_empty() {
        return Err(FetchError::Protocol("view-source: needs a URL after the scheme".to_string()));
    }

    let inner_request = Request::get(Url::new(wrapped));
    let inner_response = dispatch_fetch(&inner_request)?;

    Ok(Response {
        status: inner_response.status,
        final_url: request.url.clone(),
        headers: vec![("content-type".to_string(), "text/html".to_string())],
        body: render_source(&inner_response.body).into_bytes(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::{Method, Url};

    fn get(url: &str) -> Request {
        Request {
            method: Method::Get,
            url: Url::new(url),
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    #[test]
    fn renders_raw_bytes_as_escaped_pre_not_parsed_markup() {
        let abs = std::env::current_dir().unwrap().join("fixtures/basic.html");
        let url = format!("view-source:file://{}", abs.display());
        let resp = fetch(&get(&url)).expect("file fetch must succeed for the fixture");
        let body = String::from_utf8_lossy(&resp.body);
        // The literal tag soup must survive escaped, not be interpreted --
        // "<h1>" as text, never a real <h1> element.
        assert!(body.contains("&lt;h1&gt;Welcome&lt;/h1&gt;"), "body: {body}");
        assert!(body.starts_with("<!DOCTYPE html><html><body><pre>"));
    }

    #[test]
    fn dispatches_through_the_real_scheme_table_not_a_bespoke_fetcher() {
        // `about:` is infallible and cheap -- proves view_source re-enters
        // `fetch::fetch` (any content-bearing scheme would do; `about:` just
        // needs no fixture file).
        let resp = fetch(&get("view-source:about:attestations")).expect("must dispatch to about::fetch");
        let body = String::from_utf8_lossy(&resp.body);
        assert!(body.contains("&lt;h1&gt;"), "expected escaped markup from the real about:attestations page, got: {body}");
    }

    #[test]
    fn ampersand_and_angle_brackets_in_the_body_are_all_escaped() {
        // data: URLs let us control the exact wrapped body without touching
        // the filesystem or network.
        let resp = fetch(&get("view-source:data:text/plain,<a & b>")).expect("data: must dispatch");
        let body = String::from_utf8_lossy(&resp.body);
        assert!(body.contains("&lt;a &amp; b&gt;"), "body: {body}");
        assert!(!body.contains("<a & b>"), "raw unescaped text leaked into the page: {body}");
    }

    #[test]
    fn unsupported_inner_scheme_propagates_as_a_real_error_not_a_silent_blank_page() {
        let err = fetch(&get("view-source:gopher://example.com/")).expect_err("gopher is unsupported");
        assert!(matches!(err, FetchError::UnsupportedScheme(ref s) if s == "gopher"), "got {err:?}");
    }

    #[test]
    fn bare_view_source_with_no_wrapped_url_is_a_clean_error_not_a_panic() {
        let err = fetch(&get("view-source:")).expect_err("empty wrapped URL must error");
        assert!(matches!(err, FetchError::Protocol(_)), "got {err:?}");
    }

    #[test]
    fn is_total_over_hostile_wrapped_urls_never_panics() {
        let inputs = [
            "view-source:",
            "view-source:   ",
            "view-source:not a url at all",
            "view-source:view-source:about:attestations",
            "view-source:\u{2603}\u{1F4A5}\u{975E}ASCII",
            &format!("view-source:{}", "x".repeat(10_000)),
        ];
        for input in inputs {
            // Total means "never panics" here, not "never errors" -- an
            // unparseable/unsupported wrapped URL is a legitimate Err.
            let _ = fetch(&get(input));
        }
    }
}
