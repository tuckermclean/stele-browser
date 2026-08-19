//! Bespoke HTTP/1.1 client (P3, brief §10: contract-tested, strict test-
//! first). Every test talks to the in-process fixture server on
//! `127.0.0.1:<port>` (tests/support/mod.rs) — this file never touches the
//! external network, per brief §9/§7.

mod support;

use stele::fetch::http1::Http1Client;
use stele::fetch::{Fetch, FetchError, Method, Request, Url};
use support::FixtureServer;

fn get(client: &mut Http1Client, url: &str) -> stele::fetch::Response {
    client
        .fetch(&Request::get(Url::new(url)))
        .unwrap_or_else(|e| panic!("GET {} failed: {:?}", url, e))
}

#[test]
fn get_with_content_length_body() {
    let server = FixtureServer::start();
    let mut client = Http1Client::new();
    let resp = get(&mut client, &server.url("/content-length"));
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, b"content-length body test\n");
    assert_eq!(resp.header("Content-Type"), Some("text/plain"));
}

#[test]
fn get_fixture_file_through_the_server() {
    let server = FixtureServer::start();
    let mut client = Http1Client::new();
    let resp = get(&mut client, &server.url("/fixtures/basic.html"));
    assert_eq!(resp.status, 200);
    let on_disk = std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/basic.html"),
    )
    .unwrap();
    assert_eq!(resp.body, on_disk);
}

#[test]
fn chunked_response_is_decoded() {
    let server = FixtureServer::start();
    let mut client = Http1Client::new();
    let resp = get(&mut client, &server.url("/chunked"));
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, b"Hello, chunked world!");
}

#[test]
fn post_with_body_is_echoed_back() {
    let server = FixtureServer::start();
    let mut client = Http1Client::new();
    let req = Request {
        method: Method::Post,
        url: Url::new(server.url("/echo")),
        headers: Vec::new(),
        body: b"name=stele&mode=headless".to_vec(),
    };
    let resp = client.fetch(&req).expect("POST /echo must succeed");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, b"name=stele&mode=headless");
    // Default Content-Type when the caller doesn't set one (brief §Task 1).
    assert_eq!(
        resp.header("Content-Type"),
        Some("application/x-www-form-urlencoded")
    );
}

#[test]
fn post_respects_caller_supplied_content_type() {
    let server = FixtureServer::start();
    let mut client = Http1Client::new();
    let req = Request {
        method: Method::Post,
        url: Url::new(server.url("/echo")),
        headers: vec![("Content-Type".to_string(), "application/json".to_string())],
        body: b"{\"a\":1}".to_vec(),
    };
    let resp = client.fetch(&req).expect("POST /echo must succeed");
    assert_eq!(resp.header("Content-Type"), Some("application/json"));
    assert_eq!(resp.body, b"{\"a\":1}");
}

#[test]
fn redirect_chain_within_limit_is_followed_to_completion() {
    let server = FixtureServer::start();
    let mut client = Http1Client::new();
    let resp = get(&mut client, &server.url("/redirect/3"));
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, b"redirect target reached\n");
    assert_eq!(resp.final_url, Url::new(server.url("/redirect/0")));
}

#[test]
fn redirect_chain_exceeding_max_five_is_too_many_redirects() {
    let server = FixtureServer::start();
    let mut client = Http1Client::new();
    let result = client.fetch(&Request::get(Url::new(server.url("/redirect/6"))));
    match result {
        Err(FetchError::TooManyRedirects) => {}
        other => panic!("expected TooManyRedirects, got {:?}", other),
    }
}

#[test]
fn redirect_at_exactly_five_still_succeeds() {
    let server = FixtureServer::start();
    let mut client = Http1Client::new();
    let resp = get(&mut client, &server.url("/redirect/5"));
    assert_eq!(resp.status, 200);
}

#[test]
fn status_303_converts_post_to_get_and_drops_body() {
    let server = FixtureServer::start();
    let mut client = Http1Client::new();
    let req = Request {
        method: Method::Post,
        url: Url::new(server.url("/redirect-303")),
        headers: Vec::new(),
        body: b"should-be-dropped".to_vec(),
    };
    let resp = client.fetch(&req).expect("redirect must succeed");
    let body = String::from_utf8(resp.body).unwrap();
    assert!(body.contains("METHOD=GET"), "body was: {}", body);
    assert!(body.contains("BODY=\n"), "body was: {}", body);
}

#[test]
fn status_301_with_post_downgrades_to_get() {
    let server = FixtureServer::start();
    let mut client = Http1Client::new();
    let req = Request {
        method: Method::Post,
        url: Url::new(server.url("/redirect-301")),
        headers: Vec::new(),
        body: b"x=1".to_vec(),
    };
    let resp = client.fetch(&req).expect("redirect must succeed");
    let body = String::from_utf8(resp.body).unwrap();
    assert!(body.contains("METHOD=GET"), "body was: {}", body);
}

#[test]
fn status_307_preserves_method_and_body() {
    let server = FixtureServer::start();
    let mut client = Http1Client::new();
    let req = Request {
        method: Method::Post,
        url: Url::new(server.url("/redirect-307")),
        headers: Vec::new(),
        body: b"preserve=me".to_vec(),
    };
    let resp = client.fetch(&req).expect("redirect must succeed");
    let body = String::from_utf8(resp.body).unwrap();
    assert!(body.contains("METHOD=POST"), "body was: {}", body);
    assert!(body.contains("BODY=preserve=me"), "body was: {}", body);
}

#[test]
fn https_scheme_is_dispatched_not_unsupported() {
    // PR 2: https is served via the openssl child, so it is no longer rejected
    // as UnsupportedScheme. Loopback port 1 (nothing listening) keeps this off
    // the external network — it fails at the TLS/connect layer, not at dispatch.
    let mut client = Http1Client::new();
    let err = client
        .fetch(&Request::get(Url::new("https://127.0.0.1:1/")))
        .expect_err("connect to a dead port must fail");
    assert!(
        !matches!(err, FetchError::UnsupportedScheme(_)),
        "https must be dispatched to the TLS transport now, got {err:?}"
    );
}

#[test]
fn malformed_response_never_panics() {
    let addr = support::spawn_raw(b"this is not http at all\r\n\r\n");
    let mut client = Http1Client::new();
    let result = client.fetch(&Request::get(Url::new(format!(
        "http://{}/",
        addr
    ))));
    assert!(result.is_err(), "garbage bytes must be a FetchError, not a crash");
}

#[test]
fn truncated_headers_never_panics() {
    let addr = support::spawn_raw(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain");
    let mut client = Http1Client::new();
    let result = client.fetch(&Request::get(Url::new(format!(
        "http://{}/",
        addr
    ))));
    // No blank line ever arrives before the fixture closes the socket: this
    // must surface as an error, never a panic.
    assert!(result.is_err());
}

#[test]
fn content_length_longer_than_actual_body_never_panics() {
    let addr = support::spawn_raw(
        b"HTTP/1.1 200 OK\r\nContent-Length: 999999\r\nConnection: close\r\n\r\nshort",
    );
    let mut client = Http1Client::new();
    // Either a truncated-but-Ok body or an Err is acceptable; a panic is not.
    let _ = client.fetch(&Request::get(Url::new(format!(
        "http://{}/",
        addr
    ))));
}

#[test]
fn bad_chunk_size_never_panics() {
    let addr = support::spawn_raw(
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\nZZZ\r\ngarbage\r\n0\r\n\r\n",
    );
    let mut client = Http1Client::new();
    let result = client.fetch(&Request::get(Url::new(format!(
        "http://{}/",
        addr
    ))));
    assert!(result.is_err());
}

#[test]
fn cookies_round_trip_through_a_real_hop() {
    let server = FixtureServer::start();
    let mut client = Http1Client::new();

    let set = get(&mut client, &server.url("/set-cookie"));
    assert_eq!(set.status, 200);

    // The jar must have picked up the top-level "/"-scoped cookie.
    let echoed = get(&mut client, &server.url("/echo-cookie"));
    let body = String::from_utf8(echoed.body).unwrap();
    assert!(body.contains("a=1"), "body was: {}", body);
}
