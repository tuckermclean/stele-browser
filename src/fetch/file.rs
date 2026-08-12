//! `file://` loading (P3, Wave 1). Reads a local file into a `Response`
//! (status 200) with a minimal content-type guess by extension. Never TLS,
//! never network — this is the local half of `Fetch`.

use std::fs;
use std::path::Path;

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
        let scheme = request.url.scheme();
        if scheme != "file" {
            return Err(FetchError::UnsupportedScheme(scheme));
        }
        let path = file_path(&request.url)?;
        let bytes = fs::read(&path).map_err(|e| FetchError::Io(format!("{}: {}", path, e)))?;
        let content_type = content_type_for_path(&path);
        Ok(Response {
            status: 200,
            final_url: request.url.clone(),
            headers: vec![("Content-Type".to_string(), content_type.to_string())],
            body: bytes,
        })
    }
}

/// Extract a filesystem path from a `file://` URL. Supports the common
/// `file:///abs/path` form, `file://host/abs/path` (host ignored — no remote
/// file shares), and the no-authority `file:relative/path` form.
pub fn file_path(url: &Url) -> Result<String, FetchError> {
    let raw = url.as_str();
    let rest = raw
        .strip_prefix("file:")
        .ok_or_else(|| FetchError::UnsupportedScheme("not a file: URL".to_string()))?;
    let rest = rest.strip_prefix("//").unwrap_or(rest);

    // A bare "file:///path" begins with '/' after stripping "file://": the
    // authority is empty and this IS the path. Otherwise, if there's an
    // authority (host) segment, skip to its first '/'.
    let path = if rest.starts_with('/') {
        rest.to_string()
    } else {
        match rest.find('/') {
            Some(i) => rest[i..].to_string(),
            None => rest.to_string(),
        }
    };

    if path.is_empty() {
        return Err(FetchError::Protocol("file:// URL has no path".to_string()));
    }
    Ok(path)
}

/// A minimal content-type guess by file extension (brief §4: "html/css/gif/
/// jpeg/png/txt").
pub fn content_type_for_path(path: &str) -> &'static str {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "gif" => "image/gif",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "txt" => "text/plain",
        _ => "application/octet-stream",
    }
}
