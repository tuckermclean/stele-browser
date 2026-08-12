//! Bespoke minimal URL parsing and relative-reference resolution over the
//! frozen `Url` newtype (brief §4: "URL parsing: bespoke minimal ... your
//! call, record it" — DECISIONS.md records the bespoke-vs-crate call for the
//! HTTP layer; this module is the same spirit applied to URLs: enough syntax
//! to drive `http1`/`file` and to resolve `Location` headers, not a full
//! RFC 3986 implementation).
//!
//! Syntax handled: `scheme:[//[userinfo@]host[:port]]path[?query][#fragment]`.
//! Never panics: malformed input degrades to best-effort/empty components
//! rather than erroring (brief §9 — parsing is TOTAL).

use super::Url;

/// The pieces of a parsed URL, all owned (parsing is cheap and infrequent
/// relative to network IO, so we favor simplicity over borrowing).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UrlParts {
    pub scheme: String,
    pub host: String,
    pub port: Option<u16>,
    /// Always empty or starting with `/`.
    pub path: String,
    pub query: Option<String>,
}

impl Url {
    /// Parse this URL into its component parts. Total: never panics.
    pub fn parts(&self) -> UrlParts {
        let _ = self;
        todo!("P3: parse URL into scheme/host/port/path/query")
    }

    /// The scheme, lowercased (e.g. `"http"`, `"file"`). Empty if unparseable.
    pub fn scheme(&self) -> String {
        self.parts().scheme
    }

    /// The host, as written (no punycode/normalization). Empty if absent.
    pub fn host(&self) -> String {
        self.parts().host
    }

    /// The port, or `default` if none was given.
    pub fn port(&self, default: u16) -> u16 {
        self.parts().port.unwrap_or(default)
    }

    /// The path, always starting with `/` unless the URL genuinely has none
    /// (in which case this is empty and callers should treat it as `/`).
    pub fn path(&self) -> String {
        self.parts().path
    }

    /// The query string, without the leading `?`.
    pub fn query(&self) -> Option<String> {
        self.parts().query
    }

    /// Resolve `reference` (as found in e.g. a `Location:` header or an
    /// `href`) against `self` as the base URL, per RFC 3986 §5 (simplified:
    /// no fragment is retained, since `Url`/`Request` never need one).
    pub fn resolve(&self, reference: &str) -> Url {
        let _ = reference;
        todo!("P3: relative reference resolution against a base URL")
    }
}
