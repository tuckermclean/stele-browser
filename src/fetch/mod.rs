//! Fetching: the `Request`/`Response` interface P3 (Wave 1) implements over
//! plain TCP + httparse (`http1`), `file://` (`file`), and the cookie jar
//! (`cookies`). No TLS ever — the proxy owns modernity (charter). No async —
//! blocking IO is correct for a single-threaded program (brief §9).

pub mod cookies;
pub mod file;
pub mod http1;
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
