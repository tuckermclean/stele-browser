//! Bespoke HTTP/1.1 client over `std::net::TcpStream` (P3, Wave 1).
//!
//! DECISIONS.md records the bespoke-vs-`httparse` call for this packet: we
//! hand-roll HTTP/1.1 parsing, std-only, to keep P3 zero-dependency ahead of
//! the crate-vendoring apparatus (landing before P4). `https://` is always
//! `FetchError::UnsupportedScheme` — no TLS, ever (charter: the proxy's job).
//! Blocking IO only (brief §9); no async runtime anywhere in this program.
//!
//! Redirects (301/302/303/307/308) are followed up to `MAX_REDIRECTS` times,
//! applying the usual method-preservation rules; the cookie jar is consulted
//! and updated at every hop, against that hop's URL (not just the final one).

use std::net::TcpStream;

use super::cookies::CookieJar;
use super::{Fetch, FetchError, Method, Request, Response, Url};

/// Maximum redirects followed before `FetchError::TooManyRedirects`.
pub const MAX_REDIRECTS: u32 = 5;

/// An HTTP/1.1 client. Owns the cookie jar it sends/receives against, since
/// the frozen `Fetch::fetch` signature takes no side channel for one.
#[derive(Debug, Clone, Default)]
pub struct Http1Client {
    pub cookies: CookieJar,
}

impl Http1Client {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_cookie_jar(cookies: CookieJar) -> Self {
        Http1Client { cookies }
    }
}

impl Fetch for Http1Client {
    fn fetch(&mut self, request: &Request) -> Result<Response, FetchError> {
        let _ = request;
        todo!("P3: HTTP/1.1 request/response cycle with redirects + cookies")
    }
}

/// One raw HTTP/1.1 exchange over a fresh `TcpStream` — no redirect-following,
/// no cookie handling (that's `Http1Client::fetch`'s job); this is the total,
/// low-level parse-never-panics core.
#[derive(Debug, Clone)]
pub(crate) struct RawResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

pub(crate) fn send_one(
    url: &Url,
    method: Method,
    extra_headers: &[(String, String)],
    body: &[u8],
    cookie_header: Option<&str>,
) -> Result<RawResponse, FetchError> {
    let _ = (url, method, extra_headers, body, cookie_header);
    todo!("P3: connect, format request, read+parse response")
}

pub(crate) fn read_response(stream: &mut TcpStream) -> Result<RawResponse, FetchError> {
    let _ = stream;
    todo!("P3: status line + headers (folded) + Content-Length/chunked body")
}
