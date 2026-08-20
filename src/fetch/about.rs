//! `about:` scheme (packet/attestation-modal): purely in-process, no
//! network, no filesystem. `about:attestations` serves the embedded
//! attestation page (Stele's own short license notice, the generated Cargo
//! dependency/license roster, and Terminus's full OFL-1.1 text, assembled
//! in Task 3); any other `about:<x>`, including the bare `about:` (empty
//! path), degrades to a small, honest "unknown about: page" instead of an
//! error.
//!
//! Same free-function shape as `data::fetch` (no `Fetch` trait — this is a
//! pure function, no state, no I/O). **Total**: `fetch` here NEVER returns
//! `Err` for any input, mirroring the "never vanish" contract
//! `text::glyphs::lookup`/`text::translit::resolve` already guarantee
//! elsewhere (AGENTS.md rule 5) — a malformed/hostile/unknown `about:` URL
//! is strictly better served by a small real page than by propagating a
//! `FetchError` (which would otherwise surface as `dump_text`'s empty
//! string / `dump_png`'s blank 1x1 fallback — see `main.rs`'s own
//! `blank_png` doc comment for that worse shape).
use super::{FetchError, Request, Response};

/// Placeholder body for `about:attestations`, Task 1 of packet/attestation-
/// modal — proves the scheme-dispatch seam end to end before the real page
/// exists. Task 3 replaces this with the real assembled page; the marker
/// string below is this task's own greppable proof the placeholder (not
/// stale cached content) was actually served.
const PLACEHOLDER_ATTESTATIONS_BODY: &str =
    "<!DOCTYPE html><html><body><h1>Attestations (placeholder)</h1><p>The real attestation page lands in Task 3 of packet/attestation-modal.</p></body></html>";

/// Every `about:` path other than `attestations` (including the bare,
/// empty path `about:` itself) — a small, static "this page doesn't exist"
/// fragment linking back to the one page that does. Matching is exact and
/// case-sensitive on the URL's PATH component (the scheme itself is already
/// lowercased by `Url::scheme()`, but the path is not — `about:Attestations`
/// is therefore deliberately treated as unknown, not as a case-insensitive
/// alias for `about:attestations`; this choice is asserted by a test below).
fn unknown_about_page(path: &str) -> String {
    format!(
        "<!DOCTYPE html><html><body><h1>Unknown about: page</h1><p>There is no about: page named \"{}\". Try <a href=\"about:attestations\">about:attestations</a>.</p></body></html>",
        escape_html(path)
    )
}

/// Minimal HTML-entity escaping for interpolating arbitrary (possibly
/// hostile) URL path text into the unknown-page body — total over any
/// `&str`, including non-ASCII and pathological lengths (no allocation
/// beyond one `String`, no indexing that could panic).
fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

pub fn fetch(request: &Request) -> Result<Response, FetchError> {
    let path = request.url.path();
    let body = if path == "attestations" {
        PLACEHOLDER_ATTESTATIONS_BODY.to_string()
    } else {
        unknown_about_page(&path)
    };
    Ok(Response {
        status: 200,
        final_url: request.url.clone(),
        headers: vec![("content-type".to_string(), "text/html".to_string())],
        body: body.into_bytes(),
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
    fn fetch_attestations_returns_ok_200_html_nonempty() {
        let resp = fetch(&get("about:attestations")).expect("about: must never error");
        assert_eq!(resp.status, 200);
        assert_eq!(resp.header("content-type"), Some("text/html"));
        assert!(!resp.body.is_empty());
    }

    #[test]
    fn fetch_attestations_placeholder_body_contains_task1_marker() {
        // Task 3 swaps this for the real content; this test only proves the
        // dispatch path end to end while the placeholder is still in place.
        let resp = fetch(&get("about:attestations")).unwrap();
        let body = String::from_utf8_lossy(&resp.body);
        assert!(body.contains("Attestations (placeholder)"), "body: {body}");
    }

    #[test]
    fn fetch_bare_about_scheme_is_the_unknown_page() {
        let resp = fetch(&get("about:")).expect("about: must never error");
        assert_eq!(resp.status, 200);
        let body = String::from_utf8_lossy(&resp.body);
        assert!(body.contains("Unknown about:"), "body: {body}");
    }

    #[test]
    fn fetch_path_case_is_significant_about_attestations_capitalized_is_unknown() {
        // Deliberate choice (documented on `unknown_about_page`): the
        // scheme is lowercased by `Url::scheme()`, but the PATH is not, so
        // `about:Attestations` does NOT alias `about:attestations`.
        let resp = fetch(&get("about:Attestations")).expect("about: must never error");
        let body = String::from_utf8_lossy(&resp.body);
        assert!(
            body.contains("Unknown about:"),
            "about:Attestations (capitalized path) must be treated as unknown, got: {body}"
        );
    }

    #[test]
    fn fetch_is_total_over_hostile_about_inputs() {
        let long_garbage: String = "x".repeat(10_000);
        let inputs: &[&str] = &[
            "about:",
            "about:blank",
            "about:xyz",
            "about:Attestations",
            &long_garbage,
            "about:\u{2603}\u{1F4A5}\u{975E}ASCII", // snowman, boom emoji, non-ASCII CJK
            "about:attestations/../../etc/passwd",
            "about:attestations?x=1#frag",
        ];
        for input in inputs {
            let url = if input.starts_with("about:") {
                input.to_string()
            } else {
                format!("about:{input}")
            };
            let resp = fetch(&get(&url)).unwrap_or_else(|e| panic!("about: fetch must never error, input {url:?}: {e:?}"));
            assert_eq!(resp.status, 200, "input {url:?}");
            assert!(!resp.body.is_empty(), "input {url:?}");
        }
    }
}
