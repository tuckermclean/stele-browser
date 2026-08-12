//! A tiny in-process HTTP fixture server for P3's tests (brief §7): binds
//! `127.0.0.1:0`, serves on a background thread, and exercises redirects,
//! Set-Cookie/Cookie round-trips, and POST echo. Tests point `Http1Client` at
//! `127.0.0.1:<port>` — this process NEVER touches the external network, and
//! neither does anything that uses it.
//!
//! This is test-support scaffolding, not a packet deliverable under TDD: it
//! exists to give the (test-first) HTTP/1.1 client tests something real to
//! talk to.

#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::thread;

/// A running fixture server. Dropping this does not stop the background
/// thread (test binaries are short-lived; the OS reclaims the socket at
/// process exit), but each test binds its own ephemeral port so tests never
/// collide.
pub struct FixtureServer {
    pub addr: SocketAddr,
}

impl FixtureServer {
    /// Start the server on an OS-assigned loopback port and return once it is
    /// ready to accept connections.
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
        let addr = listener.local_addr().expect("local_addr");
        thread::spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        thread::spawn(move || {
                            let _ = handle_connection(stream);
                        });
                    }
                    Err(_) => continue,
                }
            }
        });
        FixtureServer { addr }
    }

    /// `http://127.0.0.1:<port>` with no trailing slash.
    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// `http://127.0.0.1:<port><path>`.
    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url(), path)
    }
}

/// A minimal parsed HTTP/1.x request (server side; deliberately independent
/// of the client's own parser under test).
struct RawRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl RawRequest {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

fn handle_connection(mut stream: TcpStream) -> std::io::Result<()> {
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
    let request = match read_request(&mut stream) {
        Some(r) => r,
        None => return Ok(()),
    };

    if request.path == "/chunked" {
        write_chunked_response(&mut stream);
        return Ok(());
    }

    let (status, reason, mut headers, body) = route(&request);
    headers.push(("Content-Length".to_string(), body.len().to_string()));
    headers.push(("Connection".to_string(), "close".to_string()));

    let mut out = format!("HTTP/1.1 {} {}\r\n", status, reason);
    for (k, v) in &headers {
        out.push_str(&format!("{}: {}\r\n", k, v));
    }
    out.push_str("\r\n");
    let _ = stream.write_all(out.as_bytes());
    let _ = stream.write_all(&body);
    let _ = stream.flush();
    Ok(())
}

fn read_request(stream: &mut TcpStream) -> Option<RawRequest> {
    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 4096];
    let header_end = loop {
        if let Some(idx) = find(&buf, b"\r\n\r\n") {
            break idx + 4;
        }
        if buf.len() > 8 * 1024 * 1024 {
            return None;
        }
        let n = stream.read(&mut tmp).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
    };

    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next()?;
    let mut rl_parts = request_line.split_whitespace();
    let method = rl_parts.next()?.to_string();
    let path = rl_parts.next()?.to_string();

    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some(i) = line.find(':') {
            let name = line[..i].trim().to_string();
            let value = line[i + 1..].trim().to_string();
            headers.push((name, value));
        }
    }

    let content_length: usize = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.trim().parse().ok())
        .unwrap_or(0);

    while buf.len() - header_end < content_length {
        let n = stream.read(&mut tmp).ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let body_end = (header_end + content_length).min(buf.len());
    let body = buf[header_end..body_end].to_vec();

    Some(RawRequest {
        method,
        path,
        headers,
        body,
    })
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn content_type_for(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("");
    match ext.to_ascii_lowercase().as_str() {
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "txt" => "text/plain",
        _ => "application/octet-stream",
    }
}

/// Split "/path" from an optional "?query".
fn split_path(full: &str) -> (&str, Option<&str>) {
    match full.find('?') {
        Some(i) => (&full[..i], Some(&full[i + 1..])),
        None => (full, None),
    }
}

fn route(req: &RawRequest) -> (u16, &'static str, Vec<(String, String)>, Vec<u8>) {
    let (path, _query) = split_path(&req.path);

    if let Some(name) = path.strip_prefix("/fixtures/") {
        let file_path = fixtures_dir().join(name);
        return match std::fs::read(&file_path) {
            Ok(bytes) => (
                200,
                "OK",
                vec![("Content-Type".to_string(), content_type_for(name).to_string())],
                bytes,
            ),
            Err(_) => (404, "Not Found", vec![], b"not found".to_vec()),
        };
    }

    if path == "/content-length" {
        return (
            200,
            "OK",
            vec![("Content-Type".to_string(), "text/plain".to_string())],
            b"content-length body test\n".to_vec(),
        );
    }

    if let Some(rest) = path.strip_prefix("/redirect/") {
        let n: u32 = rest.parse().unwrap_or(0);
        return if n == 0 {
            (
                200,
                "OK",
                vec![("Content-Type".to_string(), "text/plain".to_string())],
                b"redirect target reached\n".to_vec(),
            )
        } else {
            (
                302,
                "Found",
                vec![("Location".to_string(), format!("/redirect/{}", n - 1))],
                Vec::new(),
            )
        };
    }

    if path == "/redirect-303" {
        return (
            303,
            "See Other",
            vec![("Location".to_string(), "/echo-method".to_string())],
            Vec::new(),
        );
    }
    if path == "/redirect-301" {
        return (
            301,
            "Moved Permanently",
            vec![("Location".to_string(), "/echo-method".to_string())],
            Vec::new(),
        );
    }
    if path == "/redirect-307" {
        return (
            307,
            "Temporary Redirect",
            vec![("Location".to_string(), "/echo-method".to_string())],
            Vec::new(),
        );
    }

    if path == "/echo-method" {
        let ct = req.header("content-type").unwrap_or("<none>");
        let body = format!(
            "METHOD={}\nCONTENT-TYPE={}\nBODY={}\n",
            req.method,
            ct,
            String::from_utf8_lossy(&req.body)
        );
        return (
            200,
            "OK",
            vec![("Content-Type".to_string(), "text/plain".to_string())],
            body.into_bytes(),
        );
    }

    if path == "/echo" && req.method == "POST" {
        let ct = req
            .header("content-type")
            .unwrap_or("application/octet-stream")
            .to_string();
        return (200, "OK", vec![("Content-Type".to_string(), ct)], req.body.clone());
    }

    if path == "/set-cookie" {
        return (
            200,
            "OK",
            vec![
                ("Set-Cookie".to_string(), "a=1; Path=/".to_string()),
                (
                    "Set-Cookie".to_string(),
                    "b=2; Path=/sub; Domain=127.0.0.1".to_string(),
                ),
            ],
            b"cookies set\n".to_vec(),
        );
    }

    if path == "/echo-cookie" {
        let cookie = req.header("cookie").unwrap_or("<none>");
        return (
            200,
            "OK",
            vec![("Content-Type".to_string(), "text/plain".to_string())],
            format!("COOKIE={}\n", cookie).into_bytes(),
        );
    }

    (404, "Not Found", vec![], b"not found\n".to_vec())
}

fn write_chunked_response(stream: &mut TcpStream) {
    let chunks: [&[u8]; 3] = [b"Hello, ", b"chunked ", b"world!"];
    let mut out = String::new();
    out.push_str("HTTP/1.1 200 OK\r\n");
    out.push_str("Content-Type: text/plain\r\n");
    out.push_str("Transfer-Encoding: chunked\r\n");
    out.push_str("Connection: close\r\n");
    out.push_str("\r\n");
    let _ = stream.write_all(out.as_bytes());
    for c in chunks {
        let framing = format!("{:x}\r\n", c.len());
        let _ = stream.write_all(framing.as_bytes());
        let _ = stream.write_all(c);
        let _ = stream.write_all(b"\r\n");
    }
    let _ = stream.write_all(b"0\r\n\r\n");
    let _ = stream.flush();
}

/// Spawn a one-shot listener that accepts a single connection, ignores
/// whatever the client sends, writes `bytes` verbatim, and closes. Used to
/// feed the client deliberately malformed responses (totality tests).
pub fn spawn_raw(bytes: &'static [u8]) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind raw fixture");
    let addr = listener.local_addr().expect("local_addr");
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut sink = [0u8; 4096];
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(200)));
            let _ = stream.read(&mut sink); // drain (and ignore) whatever the client sent
            let _ = stream.write_all(bytes);
            let _ = stream.flush();
        }
    });
    addr
}
