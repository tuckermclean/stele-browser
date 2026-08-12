//! `file://` loading (P3, Wave 1). Reads a local file into a `Response`
//! (status 200) with a minimal content-type guess by extension. Never TLS,
//! never network — this is the local half of `Fetch`.

use super::{Fetch, FetchError, Request, Response, Url};

/// Fetches `file://` URLs from the local filesystem.
#[derive(Debug, Clone, Copy, Default)]
pub struct FileFetcher;

impl FileFetcher {
    pub fn new() -> Self {
        FileFetcher
    }
}

impl Fetch for FileFetcher {
    fn fetch(&mut self, request: &Request) -> Result<Response, FetchError> {
        let _ = request;
        todo!("P3: read a local file per a file:// Url into a Response")
    }
}

/// Extract a filesystem path from a `file://` URL.
pub fn file_path(url: &Url) -> Result<String, FetchError> {
    let _ = url;
    todo!("P3: file:// URL -> filesystem path")
}

/// A minimal content-type guess by file extension (brief §4: "html/css/gif/
/// jpeg/png/txt").
pub fn content_type_for_path(path: &str) -> &'static str {
    let _ = path;
    todo!("P3: extension -> content-type")
}
