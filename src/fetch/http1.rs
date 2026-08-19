//! Bespoke HTTP/1.1 client over `std::net::TcpStream` (P3, Wave 1).
//!
//! DECISIONS.md records the bespoke-vs-`httparse` call for this packet: we
//! hand-roll HTTP/1.1 parsing, std-only, to keep P3 zero-dependency ahead of
//! the crate-vendoring apparatus (landing before P4). `https://` is served by
//! delegating TLS to an `openssl s_client` child (see `fetch::https`) — zero
//! cryptography embedded (charter C2 amendment / DECISIONS D53).
//! Blocking IO only (brief §9); no async runtime anywhere in this program.
//! gzip (`Content-Encoding`) is deferred to a later packet per DECISIONS.md:
//! we advertise `Accept-Encoding: identity` only, and the fixture server
//! (like any polite v0 origin) answers with identity encoding.
//!
//! Redirects (301/302/303/307/308) are followed up to `MAX_REDIRECTS` times,
//! applying the usual method-preservation rules; the cookie jar is consulted
//! and updated at every hop, against that hop's URL (not just the final one).
//!
//! Response parsing (status line, headers, Content-Length body, chunked
//! body) is TOTAL: malformed input from the wire always yields a
//! `FetchError`, never a panic (brief §9).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use super::cookies::CookieJar;
use super::transport::ByteStream;
use super::{Fetch, FetchError, Method, Request, Response, Url};

/// Maximum redirects followed before `FetchError::TooManyRedirects`.
pub const MAX_REDIRECTS: u32 = 5;

const IO_TIMEOUT: Duration = Duration::from_secs(15);

/// Hard cap on how much of a single response (headers + body) we'll buffer,
/// so a malformed or malicious peer can't run us out of memory. Well above
/// anything the v0 dialect's fixtures need.
const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;

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
        let mut url = request.url.clone();
        let mut method = request.method;
        let mut body = request.body.clone();
        let mut extra_headers = request.headers.clone();

        for hop in 0..=MAX_REDIRECTS {
            let scheme = url.scheme();
            if scheme != "http" && scheme != "https" {
                return Err(FetchError::UnsupportedScheme(scheme));
            }

            let cookie_header = self.cookies.header_for(&url);
            let raw = send_one(&url, method, &extra_headers, &body, cookie_header.as_deref())?;

            for (name, value) in raw.headers.iter().filter(|(k, _)| k.eq_ignore_ascii_case("set-cookie")) {
                self.cookies.set_from_header(&url, value);
                let _ = name;
            }

            if is_redirect(raw.status) {
                if hop == MAX_REDIRECTS {
                    return Err(FetchError::TooManyRedirects);
                }
                let location = raw
                    .headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("location"))
                    .map(|(_, v)| v.clone())
                    .ok_or_else(|| {
                        FetchError::Protocol("redirect response missing Location header".to_string())
                    })?;
                let next_url = url.resolve(&location);

                if scheme == "https" && next_url.scheme() == "http" {
                    eprintln!(
                        "stele: security downgrade — following an https→http redirect to {}",
                        next_url.as_str()
                    );
                }

                match raw.status {
                    303 => {
                        method = Method::Get;
                        body = Vec::new();
                    }
                    301 | 302 if method == Method::Post => {
                        method = Method::Get;
                        body = Vec::new();
                    }
                    // 307/308 (and 301/302 with GET already): preserve as-is.
                    _ => {}
                }
                if body.is_empty() {
                    extra_headers.retain(|(k, _)| !k.eq_ignore_ascii_case("content-type"));
                }
                url = next_url;
                continue;
            }

            return Ok(Response {
                status: raw.status,
                final_url: url,
                headers: raw.headers,
                body: raw.body,
            });
        }
        // Unreachable: the loop above always returns before falling off the
        // end (either a non-redirect response or TooManyRedirects at hop ==
        // MAX_REDIRECTS). Kept total rather than `unreachable!()`-panicking.
        Err(FetchError::TooManyRedirects)
    }
}

fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
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
    let host = url.host();
    if host.is_empty() {
        return Err(FetchError::Protocol("URL has no host".to_string()));
    }
    let scheme = url.scheme();
    let default_port = if scheme == "https" { 443 } else { 80 };
    let port = url.port(default_port);

    let request_bytes = format_request(url, method, extra_headers, body, cookie_header, &host, port);

    if scheme == "https" {
        // Delegated TLS: the openssl child is just another ByteStream. Close its
        // stdin after writing (half_close = true) so -no_ign_eof propagates our
        // end-of-request. On failure, recover openssl's own reason (T4).
        let mut stream = super::https::connect(&host, port)?;
        match exchange(&mut stream, &request_bytes, true) {
            Ok(r) => Ok(r),
            Err(e) => Err(stream.tls_error_or(e)),
        }
    } else {
        let mut stream = TcpStream::connect((host.as_str(), port))
            .map_err(|e| FetchError::Io(format!("connect {}:{}: {}", host, port, e)))?;
        let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
        let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
        // half_close = false: preserve http's exact PR-1 wire behavior.
        exchange(&mut stream, &request_bytes, false)
    }
}

/// Transport-agnostic core: write the formatted request, optionally half-close
/// the write side (openssl child stdin → EOF → server-visible close via
/// -no_ign_eof), then read the framed response. `half_close = false` preserves
/// http's exact PR-1 wire behavior (no FIN-after-request).
pub(crate) fn exchange<S: ByteStream>(
    stream: &mut S,
    request_bytes: &[u8],
    half_close: bool,
) -> Result<RawResponse, FetchError> {
    stream
        .write_all(request_bytes)
        .map_err(|e| FetchError::Io(format!("write: {}", e)))?;
    stream.flush().map_err(|e| FetchError::Io(format!("flush: {}", e)))?;
    if half_close {
        let _ = stream.shutdown_write();
    }
    read_response(stream)
}

fn format_request(
    url: &Url,
    method: Method,
    extra_headers: &[(String, String)],
    body: &[u8],
    cookie_header: Option<&str>,
    host: &str,
    port: u16,
) -> Vec<u8> {
    let mut path = url.path();
    if path.is_empty() {
        path = "/".to_string();
    }
    if let Some(q) = url.query() {
        path.push('?');
        path.push_str(&q);
    }

    let method_str = match method {
        Method::Get => "GET",
        Method::Post => "POST",
    };

    let mut out = String::new();
    out.push_str(&format!("{} {} HTTP/1.1\r\n", method_str, path));
    let default_port = if url.scheme() == "https" { 443 } else { 80 };
    let host_header = if port == default_port { host.to_string() } else { format!("{}:{}", host, port) };
    out.push_str(&format!("Host: {}\r\n", host_header));
    out.push_str("User-Agent: Stele/0.1\r\n");
    out.push_str("Accept: */*\r\n");
    // gzip is deferred (DECISIONS.md); advertise identity only.
    out.push_str("Accept-Encoding: identity\r\n");
    out.push_str("Connection: close\r\n");

    let has_content_type = extra_headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type"));
    if method == Method::Post {
        if !has_content_type {
            out.push_str("Content-Type: application/x-www-form-urlencoded\r\n");
        }
        out.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    if let Some(c) = cookie_header {
        if !c.is_empty() {
            out.push_str(&format!("Cookie: {}\r\n", c));
        }
    }

    // Headers we already control above are not duplicated from the caller.
    for (k, v) in extra_headers {
        let lk = k.to_ascii_lowercase();
        if matches!(
            lk.as_str(),
            "host" | "user-agent" | "accept" | "accept-encoding" | "connection" | "content-length" | "cookie"
        ) {
            continue;
        }
        // No caller reaches this today (all extra_headers are static
        // strings), but layout/forms will eventually route page-supplied
        // data into request headers. A bare CR or LF in a name/value could
        // smuggle extra header lines — or a whole extra request — into the
        // outbound stream, so reject the header outright rather than write
        // it partially or unescaped.
        if contains_crlf(k) || contains_crlf(v) {
            continue;
        }
        out.push_str(&format!("{}: {}\r\n", k, v));
    }
    out.push_str("\r\n");

    let mut bytes = out.into_bytes();
    if method == Method::Post {
        bytes.extend_from_slice(body);
    }
    bytes
}

pub(crate) fn read_response<R: Read>(stream: &mut R) -> Result<RawResponse, FetchError> {
    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 4096];

    let header_end = loop {
        if let Some(idx) = find(&buf, b"\r\n\r\n") {
            break idx + 4;
        }
        if buf.len() > MAX_RESPONSE_BYTES {
            return Err(FetchError::Protocol("response headers too large".to_string()));
        }
        let n = stream
            .read(&mut tmp)
            .map_err(|e| FetchError::Io(format!("read headers: {}", e)))?;
        if n == 0 {
            return Err(FetchError::Protocol(
                "connection closed before headers completed".to_string(),
            ));
        }
        buf.extend_from_slice(&tmp[..n]);
    };

    let (status, headers) = parse_status_and_headers(&buf[..header_end])?;

    let content_length = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.trim().parse::<usize>().ok());
    let is_chunked = headers.iter().any(|(k, v)| {
        k.eq_ignore_ascii_case("transfer-encoding") && v.to_ascii_lowercase().contains("chunked")
    });

    let body = if is_chunked {
        read_chunked_body(stream, &mut buf, header_end)?
    } else if let Some(len) = content_length {
        read_fixed_body(stream, &mut buf, header_end, len)?
    } else {
        read_until_eof_body(stream, &mut buf, header_end)?
    };

    Ok(RawResponse { status, headers, body })
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Parse the status line + headers from the raw bytes up to and including the
/// terminating blank line. Total: any malformed line is either rejected with
/// a `FetchError::Protocol` (status line) or silently skipped (a stray
/// header line with no `:`), never a panic.
fn parse_status_and_headers(raw: &[u8]) -> Result<(u16, Vec<(String, String)>), FetchError> {
    // Lossy: HTTP headers are meant to be ASCII; invalid bytes become U+FFFD
    // rather than a parse failure, so slicing on the resulting &str (all
    // char-boundary-safe by construction) never panics either.
    let text = String::from_utf8_lossy(raw);
    let mut lines = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l));

    let status_line = lines
        .next()
        .ok_or_else(|| FetchError::Protocol("empty response".to_string()))?;
    let mut parts = status_line.splitn(3, ' ');
    let http_version = parts.next().unwrap_or("");
    if !http_version.starts_with("HTTP/") {
        return Err(FetchError::Protocol(format!("bad status line: {:?}", status_line)));
    }
    let code_str = parts
        .next()
        .ok_or_else(|| FetchError::Protocol("status line missing status code".to_string()))?;
    let status: u16 = code_str
        .trim()
        .parse()
        .map_err(|_| FetchError::Protocol(format!("bad status code: {:?}", code_str)))?;

    let mut headers: Vec<(String, String)> = Vec::new();
    for line in lines {
        if line.is_empty() {
            break; // the blank line terminating the header block
        }
        if (line.starts_with(' ') || line.starts_with('\t')) && !headers.is_empty() {
            // Obsolete line folding: continuation of the previous header's
            // value (RFC 7230 §3.2.4 — still seen in the wild).
            let last = headers.len() - 1;
            headers[last].1.push(' ');
            headers[last].1.push_str(line.trim());
            continue;
        }
        match line.find(':') {
            Some(i) => {
                let name = line[..i].trim().to_string();
                let value = line[i + 1..].trim().to_string();
                if !name.is_empty() {
                    headers.push((name, value));
                }
                // Empty name: malformed, silently skipped.
            }
            None => {
                // No ':' at all: malformed, silently skipped (total).
            }
        }
    }

    Ok((status, headers))
}

fn read_fixed_body<R: Read>(
    stream: &mut R,
    buf: &mut Vec<u8>,
    start: usize,
    len: usize,
) -> Result<Vec<u8>, FetchError> {
    let capped_len = len.min(MAX_RESPONSE_BYTES);
    let mut tmp = [0u8; 4096];
    while buf.len() - start < capped_len {
        let n = stream
            .read(&mut tmp)
            .map_err(|e| FetchError::Io(format!("read body: {}", e)))?;
        if n == 0 {
            break; // peer closed early: return what we got (total, not fatal)
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let end = (start + capped_len).min(buf.len());
    Ok(buf[start..end].to_vec())
}

fn read_until_eof_body<R: Read>(stream: &mut R, buf: &mut Vec<u8>, start: usize) -> Result<Vec<u8>, FetchError> {
    let mut tmp = [0u8; 4096];
    loop {
        let n = stream
            .read(&mut tmp)
            .map_err(|e| FetchError::Io(format!("read body: {}", e)))?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.len() - start > MAX_RESPONSE_BYTES {
            return Err(FetchError::Protocol("response body too large".to_string()));
        }
    }
    Ok(buf[start..].to_vec())
}

/// Decode a chunked body (RFC 7230 §4.1). `pos` tracks how much of `buf` has
/// been consumed; more bytes are pulled from `stream` on demand. Every index
/// used here is derived from a successful `find` within already-buffered
/// bytes, so `pos <= buf.len()` holds throughout and no slice can panic.
fn read_chunked_body<R: Read>(stream: &mut R, buf: &mut Vec<u8>, start: usize) -> Result<Vec<u8>, FetchError> {
    let mut pos = start;
    let mut out = Vec::new();
    let mut tmp = [0u8; 4096];

    loop {
        let line_end = read_line_end(stream, buf, &mut tmp, pos)?;
        let size_line = String::from_utf8_lossy(&buf[pos..line_end]);
        let size_str = size_line.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_str, 16)
            .map_err(|_| FetchError::Protocol(format!("bad chunk size: {:?}", size_str)))?;
        pos = line_end + 2;

        if size == 0 {
            // Trailer headers (possibly none), terminated by a blank line.
            loop {
                let l_end = read_line_end(stream, buf, &mut tmp, pos)?;
                let is_blank = l_end == pos;
                pos = l_end + 2;
                if is_blank {
                    break;
                }
            }
            break;
        }
        if size > MAX_RESPONSE_BYTES || out.len() + size > MAX_RESPONSE_BYTES {
            return Err(FetchError::Protocol("chunk size too large".to_string()));
        }

        while buf.len() < pos + size + 2 {
            let n = stream
                .read(&mut tmp)
                .map_err(|e| FetchError::Io(format!("read chunk data: {}", e)))?;
            if n == 0 {
                return Err(FetchError::Protocol("truncated chunk data".to_string()));
            }
            buf.extend_from_slice(&tmp[..n]);
        }
        out.extend_from_slice(&buf[pos..pos + size]);
        pos += size + 2; // skip the chunk data's trailing CRLF
    }

    Ok(out)
}

/// Ensure `buf` contains a `\r\n`-terminated line starting at `pos`, reading
/// more from `stream` as needed; returns the index of the `\r` (so the line
/// text is `buf[pos..line_end]` and the next position after it is
/// `line_end + 2`).
fn read_line_end<R: Read>(
    stream: &mut R,
    buf: &mut Vec<u8>,
    tmp: &mut [u8; 4096],
    pos: usize,
) -> Result<usize, FetchError> {
    loop {
        if let Some(idx) = find(&buf[pos..], b"\r\n") {
            return Ok(pos + idx);
        }
        if buf.len() > MAX_RESPONSE_BYTES {
            return Err(FetchError::Protocol("chunked response too large".to_string()));
        }
        let n = stream
            .read(tmp)
            .map_err(|e| FetchError::Io(format!("read chunked line: {}", e)))?;
        if n == 0 {
            return Err(FetchError::Protocol("truncated chunked response".to_string()));
        }
        buf.extend_from_slice(&tmp[..n]);
    }
}

/// `true` if `s` contains a bare CR or LF — writing either into a header
/// name/value would let it smuggle extra header lines (or a whole extra
/// request) into the outbound stream.
fn contains_crlf(s: &str) -> bool {
    s.bytes().any(|b| b == b'\r' || b == b'\n')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_request_drops_headers_with_crlf_in_the_value() {
        let url = Url::new("http://example.com/");
        let malicious = vec![(
            "X-Evil".to_string(),
            "value\r\nX-Injected: pwned".to_string(),
        )];
        let bytes = format_request(&url, Method::Get, &malicious, &[], None, "example.com", 80);
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            !text.contains("X-Injected"),
            "CRLF-injected header line must never reach the wire:\n{}",
            text
        );
        assert!(
            !text.to_ascii_lowercase().contains("x-evil"),
            "the malicious header must be dropped whole, not partially written:\n{}",
            text
        );
    }

    #[test]
    fn format_request_drops_headers_with_crlf_in_the_name() {
        let url = Url::new("http://example.com/");
        let malicious = vec![(
            "X-Evil\r\nX-Injected".to_string(),
            "value".to_string(),
        )];
        let bytes = format_request(&url, Method::Get, &malicious, &[], None, "example.com", 80);
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("X-Injected"), "request was:\n{}", text);
    }

    #[test]
    fn format_request_keeps_legitimate_custom_headers() {
        let url = Url::new("http://example.com/");
        let headers = vec![("X-Custom".to_string(), "safe-value".to_string())];
        let bytes = format_request(&url, Method::Get, &headers, &[], None, "example.com", 80);
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("X-Custom: safe-value\r\n"), "request was:\n{}", text);
    }
}
