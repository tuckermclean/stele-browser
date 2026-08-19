//! Fetching: the `Request`/`Response` interface P3 (Wave 1) implements over
//! plain TCP + httparse (`http1`), `file://` (`file`), and the cookie jar
//! (`cookies`). No TLS ever — the proxy owns modernity (charter). No async —
//! blocking IO is correct for a single-threaded program (brief §9).

pub mod cookies;
pub mod file;
pub mod http1;
pub mod transport;
pub mod url;

/// A minimal URL. P3 decides bespoke-vs-crate for the real parser and may swap
/// the internals; this newtype is the frozen shape callers hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Url(String);

impl Url {
    pub fn new(raw: impl Into<String>) -> Self {
        Url(raw.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// HTTP methods in the v0 dialect (GET/POST only; brief §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

/// An outbound request.
#[derive(Debug, Clone)]
pub struct Request {
    pub method: Method,
    pub url: Url,
    pub headers: Vec<(String, String)>,
    /// POST body; empty for GET.
    pub body: Vec<u8>,
}

impl Request {
    pub fn get(url: Url) -> Self {
        Request {
            method: Method::Get,
            url,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }
}

/// A completed response, redirects already followed (max 5; brief §4). `body`
/// is the decoded entity (gzip already inflated).
#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    /// The URL the body actually came from, after any redirects.
    pub final_url: Url,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    /// First header value matching `name` (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Clone)]
pub enum FetchError {
    Io(String),
    Protocol(String),
    /// More than the allowed number of redirects.
    TooManyRedirects,
    /// A scheme this build does not serve (e.g. https — the proxy's job).
    UnsupportedScheme(String),
}

/// Anything that can turn a `Request` into a `Response`. `http1` and `file`
/// implement it; tests use an in-repo fixture server (brief §7) — never the
/// real network.
pub trait Fetch {
    fn fetch(&mut self, request: &Request) -> Result<Response, FetchError>;
}

/// The single scheme -> fetcher table. Every driver-level wrapper delegates
/// here, so a new scheme (PR 2 adds `https`) is one arm in ONE place instead
/// of six. Cookie jars are intentionally per-call (fresh `Http1Client`), which
/// matches the pre-centralization behavior — no jar was shared across these
/// call sites before.
pub fn fetch(request: &Request) -> Result<Response, FetchError> {
    match request.url.scheme().as_str() {
        "file" => file::FileFetcher::new().fetch(request),
        "http" => http1::Http1Client::new().fetch(request),
        other => Err(FetchError::UnsupportedScheme(other.to_string())),
    }
}

/// Render a `FetchError` as the driver-level `String` the call sites use.
/// Preserves the EXACT text each site produced before centralization: an
/// unsupported scheme reads "unsupported scheme: <scheme>"; everything else
/// is the `Debug` form the sites already showed via `format!("{e:?}")`.
pub fn err_to_string(err: FetchError) -> String {
    match err {
        FetchError::UnsupportedScheme(s) => format!("unsupported scheme: {s}"),
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
mod dispatch_tests {
    use super::*;

    #[test]
    fn fetch_routes_file_scheme_to_the_file_fetcher() {
        // A file:// URL to a path that does not exist must reach FileFetcher
        // (=> Io error), proving dispatch — NOT UnsupportedScheme.
        let err = fetch(&Request::get(Url::new("file:///stele/does/not/exist")))
            .expect_err("nonexistent file must error");
        assert!(
            matches!(err, FetchError::Io(_)),
            "expected Io (dispatched to FileFetcher), got {err:?}"
        );
    }

    #[test]
    fn fetch_rejects_unknown_scheme() {
        let err = fetch(&Request::get(Url::new("gopher://example.com/")))
            .expect_err("unknown scheme must error");
        assert!(matches!(err, FetchError::UnsupportedScheme(ref s) if s == "gopher"), "got {err:?}");
    }

    #[test]
    fn err_to_string_preserves_unsupported_scheme_text() {
        // Must match the exact string the six call sites produced before
        // centralization: "unsupported scheme: <scheme>".
        let s = err_to_string(FetchError::UnsupportedScheme("gopher".to_string()));
        assert_eq!(s, "unsupported scheme: gopher");
    }

    #[test]
    fn err_to_string_debug_formats_other_errors() {
        let s = err_to_string(FetchError::Protocol("boom".to_string()));
        assert_eq!(s, "Protocol(\"boom\")");
    }
}
