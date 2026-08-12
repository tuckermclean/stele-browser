//! The cookie jar: Netscape-format text the user owns (charter C6). P3 (Wave 1)
//! implements domain/path matching and Set-Cookie parsing; no third-party
//! cookies in v0 (brief §4 — a DECISIONS note if that bites a fixture). Cookie
//! rules are contract-tested, so this arrives as a typed skeleton.

use super::Url;

/// One stored cookie.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cookie {
    pub domain: String,
    pub path: String,
    pub name: String,
    pub value: String,
    pub secure: bool,
}

/// The jar. Backed on disk by a Netscape-format text file; grep is the API.
#[derive(Debug, Clone, Default)]
pub struct CookieJar {
    cookies: Vec<Cookie>,
}

impl CookieJar {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ingest one `Set-Cookie` header value against the response URL.
    pub fn set_from_header(&mut self, _url: &Url, _set_cookie: &str) {
        todo!("P3: parse Set-Cookie, apply domain/path rules")
    }

    /// The `Cookie:` header value to send for `url`, or `None` if the jar has
    /// nothing that matches.
    pub fn header_for(&self, _url: &Url) -> Option<String> {
        todo!("P3: domain/path match + serialize")
    }

    /// Serialize the whole jar to Netscape jar text.
    pub fn to_netscape(&self) -> String {
        todo!("P3: Netscape jar format")
    }
}
